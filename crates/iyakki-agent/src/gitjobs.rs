//! Cloning, fetching and pulling many repositories as detached jobs (F-GIT-02, F-GIT-03).
//!
//! A [`Batch`] is one request from the person ("clone these five", "fetch all"): a list of [`Item`]s, one git command each, run as
//! jobs of the jobs engine (`crate::jobs`: the top bar's installer indicator and the live log), at most [`MAX_PARALLEL`] at a time. One
//! item failing never stops the others, and a failed item can be retried. The steps of a batch:
//!
//! 1. `Probing`: the SSH hosts of the list are asked for their host key (in a background thread; a first-time host needs the
//!    person's confirmation with its `SHA256:` fingerprint, a changed key fails that host's items with the explanation).
//! 2. `Confirm`: the person trusts or refuses each new host ([`Batch::trust_host`], [`Batch::refuse_host`]).
//! 3. `Running`: an in-memory agent ([`crate::gitagent`]) is started with only the keys and tokens this batch needs, and each item
//!    is started as a job whose environment points git at it. When nothing is running or queued the agent (and the keys in memory)
//!    is dropped; a retry starts a new one.
//!
//! git is started with an argument array (never a shell), a fixed environment (`GIT_TERMINAL_PROMPT=0`, no credential helper, only
//! `https`, `http` and `ssh` transports), and `--` before the address, so an address can never become an option. No credential is in
//! an argument, a URL, the log or the audit: the SSH key is reached through the agent socket, a token through the askpass helper.
//! A job survives a restart of the shell, but its agent does not: a clone that needs a key and outlives the shell fails and is
//! retried.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use crate::gitagent::{AgentKey, GitAgent, TokenEntry, ASKPASS_ENV};
use crate::gitkeys::{Auth, GitKeys, HostCheck, Who};
use crate::gitsite::{self, ParseOptions, RepoEntry, RepoUrl};
use crate::jobs::{Cmd, Jobs, Spec, State};
use crate::ssh::HostKey;
use crate::sshkey::KeyError;

pub const MAX_PARALLEL: usize = 2;
/// Most repositories in one request.
pub const MAX_ITEMS: usize = 40;
/// Where the program the IYAKKI images carry for `ssh` with agent support lives.
pub const IMAGE_GIT_SSH: &str = "/usr/libexec/iyakki/git-ssh";

// ------------------------------------------------------------------ the programs

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCheck {
    Ready { path: PathBuf, version: String },
    Missing { message: String },
}

/// Looks for git in `search` (a PATH-style list) and says plainly when it is missing and what can be done about it.
pub fn check_git(search: &str) -> GitCheck {
    let Some(path) = crate::ssh::find_program("git", search) else { return GitCheck::Missing { message: missing_message() } };
    let version = std::process::Command::new(&path).arg("--version").env_clear().output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("git version ").to_string()).unwrap_or_default();
    GitCheck::Ready { path, version }
}

pub fn missing_message() -> String {
    if Path::new("/etc/iyakki-release").exists() {
        "git is not installed on this IYAKKI image. The web and Mac images include it; the libre image leaves out GPL programs that are not needed to boot, and no in-system install route exists for git yet. Use an image with git, or work with the files without cloning.".to_string()
    } else {
        "git is not installed on this computer. Install it with your system's package manager (Debian/Ubuntu `sudo apt install git`, Fedora `sudo dnf install git`, Arch `sudo pacman -S git`, macOS `xcode-select --install`, Windows https://git-scm.com); IYAKKI never runs sudo for you.".to_string()
    }
}

/// The `ssh` program git should use. Git needs one that speaks the ssh-agent protocol: OpenSSH's. The images carry it as
/// `/usr/libexec/iyakki/git-ssh` (Dropbear's `dbclient`, the `ssh` of the PATH there, cannot use an agent).
pub fn find_ssh(search: &str) -> Result<PathBuf, String> {
    let image = std::env::var("IYAKKI_GIT_SSH").unwrap_or_else(|_| IMAGE_GIT_SSH.to_string());
    if Path::new(&image).is_file() {
        return Ok(PathBuf::from(image));
    }
    match crate::ssh::detect(search) {
        Some(c) if c.flavor == crate::ssh::Flavor::OpenSsh => Ok(c.path),
        Some(_) => Err("this system has only Dropbear's ssh client, which cannot use the key service that keeps your keys in memory. SSH addresses need OpenSSH's ssh (the web and Mac images carry it as /usr/libexec/iyakki/git-ssh); HTTPS addresses with a token work now.".into()),
        None => Err(format!("no SSH client was found. {}", crate::ssh::install_hint(std::env::consts::OS))),
    }
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub git: PathBuf,
    /// Needed only for SSH addresses; the reason it is missing is kept for the message.
    pub ssh: Result<PathBuf, String>,
    /// Tests may clone from local paths.
    pub allow_local: bool,
}

impl Tools {
    pub fn detect(search: &str) -> Result<Tools, String> {
        match check_git(search) {
            GitCheck::Ready { path, .. } => Ok(Tools { git: path, ssh: find_ssh(search), allow_local: false }),
            GitCheck::Missing { message } => Err(message),
        }
    }
}

pub type Probe = Arc<dyn Fn(&str, u16) -> Result<Vec<HostKey>, String> + Send + Sync>;

/// The real probe: asks the SSH client (or `ssh-keyscan`) what the server presents.
pub fn real_probe(search: String) -> Probe {
    Arc::new(move |host, port| {
        let c = crate::ssh::detect(&search).ok_or_else(|| format!("no SSH client was found. {}", crate::ssh::install_hint(std::env::consts::OS)))?;
        crate::ssh::probe(&c, host, port)
    })
}

// ------------------------------------------------------------------ a platform service by name

/// `@gitea-iyakki-gitea/team/app` (or `gitea:gitea-iyakki-gitea/team/app`): a repository of a git service of the platform services
/// (`docs/PLATFORM-SERVICES.md`) named by its instance, resolved to its clone address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceRef {
    pub instance: String,
    /// The clone address (`https://host/team/app.git`; the service's own web address with the path added).
    pub url: String,
    pub host: String,
    /// The vault entry holding IYAKKI's token for that service, when it has one.
    pub token_vault: Option<String>,
}

/// `None` when `text` is not an `@instance/…` reference. `Some(Err(..))` names the instances that exist.
pub fn expand_instance(dir: &Path, user: &str, text: &str) -> Option<Result<InstanceRef, String>> {
    let t = text.trim();
    let rest = t.strip_prefix('@').or_else(|| t.strip_prefix("gitea:"))?;
    let (name, path) = rest.split_once('/').unwrap_or((rest, ""));
    let all = crate::platform::installed::load(dir, user);
    let git: Vec<&crate::platform::installed::Installed> = all.iter().filter(|i| i.web_url().is_some() && i.service.contains("gitea")).collect();
    let Some(inst) = git.iter().find(|i| i.id == name) else {
        let known: Vec<&str> = git.iter().map(|i| i.id.as_str()).collect();
        return Some(Err(if known.is_empty() { format!("`{name}` is not a platform service of yours (no Gitea is set up: IDE, K8S panel, Services)") } else { format!("`{name}` is not one of your git services; they are: {}", known.join(", ")) }));
    };
    let path = path.trim_matches('/');
    if path.split('/').filter(|p| !p.is_empty()).count() < 2 {
        return Some(Err(format!("write @{name}/owner/repo")));
    }
    let base = inst.web_url().unwrap_or_default().trim_end_matches('/');
    let path = if path.ends_with(".git") { path.to_string() } else { format!("{path}.git") };
    let url = format!("{base}/{path}");
    match gitsite::parse(&url, ParseOptions::default()) {
        Ok(u) => Some(Ok(InstanceRef { instance: inst.id.clone(), url: u.url.clone(), host: u.host, token_vault: inst.token_vault().map(String::from) })),
        Err(e) => Some(Err(e)),
    }
}

// ------------------------------------------------------------------ the request

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloneSpec {
    pub url: String,
    pub branch: String,
    pub depth: u32,
    /// The folder name the person chose; empty: from the address.
    pub folder: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Clone,
    Fetch,
    Pull,
    /// Send the branch of the repository to its `origin` (the workspace's own repository, after the review).
    Push,
}

impl Op {
    pub fn verb(self) -> &'static str {
        match self {
            Op::Clone => "clone",
            Op::Fetch => "fetch",
            Op::Pull => "pull",
            Op::Push => "push",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Queued,
    Running,
    Done,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub op: Op,
    pub spec: CloneSpec,
    pub url: RepoUrl,
    /// The folder inside the workspace.
    pub folder: String,
    pub auth: Auth,
    pub state: ItemState,
    pub job: Option<String>,
    /// The last line git printed (credentials removed).
    pub last: String,
    pub progress: Option<f32>,
    pub tries: u32,
}

impl Item {
    pub fn title(&self) -> String {
        format!("git {} {}", self.op.verb(), self.url.short())
    }
}

/// A host whose key the person has to look at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPrompt {
    pub host: String,
    pub port: u16,
    pub keys: Vec<HostKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Probing,
    Confirm,
    Running,
    Done,
}

/// Everything a batch needs from the IDE.
#[derive(Clone)]
pub struct Ctx {
    pub keys: Arc<GitKeys>,
    pub tools: Tools,
    pub owner: String,
    pub workspace: String,
    pub root: PathBuf,
    /// This workspace's own key choices (host -> key id).
    pub ws_keys: BTreeMap<String, String>,
    pub who: Who,
    /// Where the per-batch agent folder is made (on the data volume, private).
    pub run_dir: PathBuf,
    pub probe: Probe,
    pub scope: Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>,
}

pub struct Batch {
    pub ctx: Ctx,
    pub items: Vec<Item>,
    pub phase: Phase,
    pub prompts: Vec<HostPrompt>,
    probe_rx: Option<Receiver<(String, u16, Result<Vec<HostKey>, String>)>>,
    probing: Vec<(String, u16)>,
    agent: Option<GitAgent>,
    /// Hosts the person refused.
    refused: HashSet<String>,
    /// Entries to write into project.json as soon as they are done (clone only).
    pub changed: bool,
}

/// What is wrong with a line of the request, by line number (1-based), before anything runs.
pub type LineErrors = Vec<(usize, String)>;

impl Batch {
    /// Checks every line and the folders. Nothing is started and no network is used. All the problems are returned together.
    pub fn plan_clone(ctx: Ctx, specs: &[CloneSpec]) -> Result<Batch, LineErrors> {
        let mut errors = LineErrors::new();
        if specs.is_empty() {
            return Err(vec![(0, "there is no repository address in the list".into())]);
        }
        if specs.len() > MAX_ITEMS {
            return Err(vec![(0, format!("at most {MAX_ITEMS} repositories at a time"))]);
        }
        let mut items = Vec::new();
        let mut taken: HashSet<String> = ctx.keys_taken();
        for (i, s) in specs.iter().enumerate() {
            let n = i + 1;
            let url = match gitsite::parse(&s.url, ParseOptions { allow_local: ctx.tools.allow_local }) {
                Ok(u) => u,
                Err(e) => {
                    errors.push((n, e));
                    continue;
                }
            };
            if !gitsite::valid_branch(&s.branch) {
                errors.push((n, format!("`{}` is not a branch name", s.branch)));
                continue;
            }
            if s.depth > 1_000_000 {
                errors.push((n, "the depth is too large".into()));
                continue;
            }
            let folder = match gitsite::folder_for(&ctx.root, &url, &s.folder, &taken) {
                Ok(f) => f,
                Err(e) => {
                    errors.push((n, e));
                    continue;
                }
            };
            taken.insert(folder.to_ascii_lowercase());
            let auth = ctx.keys.resolve(&url, &ctx.ws_keys);
            items.push(Item { op: Op::Clone, spec: s.clone(), url, folder, auth, state: ItemState::Queued, job: None, last: String::new(), progress: None, tries: 0 });
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Batch::new(ctx, items))
    }

    /// Fetch or pull for repositories already in the workspace.
    pub fn plan_update(ctx: Ctx, repos: &[RepoEntry], op: Op) -> Result<Batch, LineErrors> {
        let mut items = Vec::new();
        let mut errors = LineErrors::new();
        for (i, r) in repos.iter().enumerate() {
            let url = match gitsite::parse(&r.url, ParseOptions { allow_local: ctx.tools.allow_local }) {
                Ok(u) => u,
                Err(e) => {
                    errors.push((i + 1, format!("{}: {e}", r.path)));
                    continue;
                }
            };
            if crate::workspace::resolve_in(&ctx.root, &r.path).is_err() {
                errors.push((i + 1, format!("{} is outside the workspace", r.path)));
                continue;
            }
            let auth = ctx.keys.resolve(&url, &ctx.ws_keys);
            items.push(Item { op, spec: CloneSpec { url: r.url.clone(), branch: r.branch.clone(), depth: r.depth, folder: r.path.clone() }, url, folder: r.path.clone(), auth, state: ItemState::Queued, job: None, last: String::new(), progress: None, tries: 0 });
        }
        if !errors.is_empty() && items.is_empty() {
            return Err(errors);
        }
        Ok(Batch::new(ctx, items))
    }

    /// Push `branch` of the repository in `folder` (`.` is the workspace itself) to its `origin`, whose address is `url`. The caller
    /// has shown the review; this checks the pieces and starts nothing (no network).
    pub fn plan_push(ctx: Ctx, url: &str, folder: &str, branch: &str) -> Result<Batch, LineErrors> {
        let url = gitsite::parse(url, ParseOptions { allow_local: ctx.tools.allow_local }).map_err(|e| vec![(0, e)])?;
        if branch.is_empty() || !gitsite::valid_branch(branch) {
            return Err(vec![(0, format!("`{branch}` is not a branch name"))]);
        }
        if let Err(e) = crate::workspace::resolve_in(&ctx.root, folder) {
            return Err(vec![(0, e)]);
        }
        let auth = ctx.keys.resolve(&url, &ctx.ws_keys);
        let item = Item { op: Op::Push, spec: CloneSpec { url: url.url.clone(), branch: branch.to_string(), depth: 0, folder: folder.to_string() }, url, folder: folder.to_string(), auth, state: ItemState::Queued, job: None, last: String::new(), progress: None, tries: 0 };
        Ok(Batch::new(ctx, vec![item]))
    }

    fn new(ctx: Ctx, items: Vec<Item>) -> Batch {
        let mut b = Batch { ctx, items, phase: Phase::Probing, prompts: vec![], probe_rx: None, probing: vec![], agent: None, refused: HashSet::new(), changed: false };
        b.start_probes();
        b
    }

    fn start_probes(&mut self) {
        let mut hosts: Vec<(String, u16)> = Vec::new();
        for it in &self.items {
            if it.url.is_ssh() && !hosts.contains(&(it.url.host.clone(), it.url.ssh_port())) {
                hosts.push((it.url.host.clone(), it.url.ssh_port()));
            }
        }
        if hosts.is_empty() {
            self.phase = Phase::Running;
            return;
        }
        let (tx, rx) = channel();
        for (h, p) in hosts.clone() {
            let (tx, probe) = (tx.clone(), self.ctx.probe.clone());
            std::thread::spawn(move || {
                let r = probe(&h, p);
                let _ = tx.send((h, p, r));
            });
        }
        self.probing = hosts;
        self.probe_rx = Some(rx);
    }

    fn fail_host(&mut self, host: &str, why: &str) {
        for it in self.items.iter_mut().filter(|i| i.url.is_ssh() && i.url.host == host && i.state == ItemState::Queued) {
            it.state = ItemState::Failed(why.to_string());
            it.last = why.chars().take(160).collect();
        }
    }

    fn collect_probes(&mut self) {
        let Some(rx) = self.probe_rx.take() else { return };
        while let Ok((h, p, r)) = rx.try_recv() {
            self.probing.retain(|x| *x != (h.clone(), p));
            match r {
                Err(e) => self.fail_host(&h, &format!("could not check the host key of {h}: {e}")),
                Ok(presented) => match crate::gitkeys::host_check(&self.ctx.keys.known_hosts(), &h, p, presented) {
                    HostCheck::Trusted => {}
                    HostCheck::New(keys) => self.prompts.push(HostPrompt { host: h, port: p, keys }),
                    HostCheck::Changed { recorded, presented } => {
                        let text = crate::gitkeys::changed_text(&h, p, &recorded, &presented);
                        self.ctx.keys.audit("host.changed", &format!("{h}:{p} refused"));
                        self.fail_host(&h, &text);
                    }
                },
            }
        }
        if !self.probing.is_empty() {
            self.probe_rx = Some(rx);
        } else if self.phase == Phase::Probing {
            self.phase = if self.prompts.is_empty() { Phase::Running } else { Phase::Confirm };
        }
    }

    /// The person compared the fingerprint and trusts the host.
    pub fn trust_host(&mut self, host: &str, port: u16) -> Result<(), String> {
        let Some(i) = self.prompts.iter().position(|p| p.host == host && p.port == port) else { return Err("that host is not waiting for a decision".into()) };
        let p = self.prompts.remove(i);
        self.ctx.keys.known_hosts().trust(host, port, &p.keys)?;
        self.ctx.keys.audit("host.trust", &format!("{host}:{port} {}", p.keys.iter().map(|k| k.fingerprint()).collect::<Vec<_>>().join(",")));
        self.after_decision();
        Ok(())
    }

    pub fn refuse_host(&mut self, host: &str, port: u16) {
        if let Some(i) = self.prompts.iter().position(|p| p.host == host && p.port == port) {
            self.prompts.remove(i);
            self.refused.insert(host.to_string());
            self.fail_host(host, &format!("you did not trust the host key of {host}, so nothing was sent to it"));
            self.after_decision();
        }
    }

    fn after_decision(&mut self) {
        if self.prompts.is_empty() && self.phase == Phase::Confirm {
            self.phase = Phase::Running;
        }
    }

    pub fn running_count(&self) -> usize {
        self.items.iter().filter(|i| i.state == ItemState::Running).count()
    }
    pub fn pending(&self) -> bool {
        self.items.iter().any(|i| matches!(i.state, ItemState::Queued | ItemState::Running)) || matches!(self.phase, Phase::Probing | Phase::Confirm)
    }
    pub fn failed(&self) -> usize {
        self.items.iter().filter(|i| matches!(i.state, ItemState::Failed(_))).count()
    }
    pub fn done(&self) -> usize {
        self.items.iter().filter(|i| i.state == ItemState::Done).count()
    }
    /// The agent's socket directory, for tests and the audit.
    pub fn agent_dir(&self) -> Option<&Path> {
        self.agent.as_ref().map(|a| a.dir())
    }

    /// Makes a failed item run again (a new job).
    pub fn retry(&mut self, i: usize) -> bool {
        let Some(it) = self.items.get_mut(i) else { return false };
        if !matches!(it.state, ItemState::Failed(_)) || self.refused.contains(&it.url.host) {
            return false;
        }
        it.state = ItemState::Queued;
        it.job = None;
        it.progress = None;
        it.tries += 1;
        if self.phase == Phase::Done {
            self.phase = Phase::Running;
        }
        true
    }

    /// Does the work that is due: collects probe answers, starts queued items (at most [`MAX_PARALLEL`] running), notes finished
    /// jobs and writes project.json for cloned repositories. Call it often (every tick); it never blocks.
    pub fn tick(&mut self, jobs: &Jobs) {
        if matches!(self.phase, Phase::Probing) {
            self.collect_probes();
        }
        if self.phase != Phase::Running {
            return;
        }
        // finished jobs first
        for i in 0..self.items.len() {
            if self.items[i].state != ItemState::Running {
                continue;
            }
            let Some(id) = self.items[i].job.clone() else { continue };
            let Ok(j) = jobs.get(&self.ctx.owner, &id) else {
                self.items[i].state = ItemState::Failed("the job is gone".into());
                continue;
            };
            self.items[i].last = j.last_line.clone();
            self.items[i].progress = j.progress;
            match j.state {
                State::Running => {}
                State::Succeeded => {
                    self.items[i].state = ItemState::Done;
                    self.items[i].progress = Some(100.0);
                    if self.items[i].op == Op::Clone {
                        self.record(i);
                    }
                }
                State::Failed(c) => {
                    let why = if j.last_line.is_empty() { format!("git exited with code {c}") } else { j.last_line.clone() };
                    self.items[i].state = ItemState::Failed(why);
                }
                State::Stopped => self.items[i].state = ItemState::Failed("stopped".into()),
                State::Lost => self.items[i].state = ItemState::Failed("the job was lost (the computer was restarted?)".into()),
            }
        }
        // start queued ones
        while self.running_count() < MAX_PARALLEL {
            let Some(i) = self.items.iter().position(|i| i.state == ItemState::Queued) else { break };
            if let Err(e) = self.start_item(i, jobs) {
                self.items[i].state = ItemState::Failed(e.clone());
                self.items[i].last = e.chars().take(160).collect();
            }
        }
        if !self.pending() {
            self.phase = Phase::Done;
            self.agent = None; // the keys leave memory
        }
    }

    fn record(&mut self, i: usize) {
        let it = &self.items[i];
        let entry = RepoEntry { url: it.url.url.clone(), path: it.folder.clone(), branch: it.spec.branch.clone(), key: it.auth.label(), depth: it.spec.depth, test: String::new(), run: String::new(), state: "cloned".into() };
        match crate::workspace::ProjectConfig::load(&self.ctx.root) {
            Ok(mut c) => {
                gitsite::upsert(&mut c.repos, entry);
                if c.save(&self.ctx.root).is_ok() {
                    self.changed = true;
                }
            }
            Err(e) => self.items[i].last = format!("cloned, but project.json could not be updated: {e}"),
        }
    }

    fn ensure_agent(&mut self) -> Result<(), String> {
        if self.agent.is_some() {
            return Ok(());
        }
        let mut keys: Vec<AgentKey> = Vec::new();
        let mut tokens: Vec<TokenEntry> = Vec::new();
        let mut seen = HashSet::new();
        for it in self.items.iter().filter(|i| i.state == ItemState::Queued || i.state == ItemState::Running) {
            match &it.auth {
                Auth::Key { id, .. } if seen.insert(id.clone()) => match self.ctx.keys.pair(id) {
                    Ok(pair) => keys.push(AgentKey { id: id.clone(), pair }),
                    Err(KeyError::NeedPassphrase) => return Err(format!("the key `{id}` is protected by a passphrase: enter it in Settings → Git and SSH keys, then retry")),
                    Err(e) => return Err(format!("the key `{id}` cannot be used: {e}")),
                },
                Auth::Token { host } if seen.insert(format!("token:{host}")) => {
                    let (user, token) = self.ctx.keys.token(host)?;
                    tokens.push(TokenEntry { host: host.clone(), user, token });
                }
                _ => {}
            }
        }
        let mut r = [0u8; 4];
        iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
        let dir = self.ctx.run_dir.join(format!("g{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        let k2 = self.ctx.keys.clone();
        let t2 = self.ctx.keys.clone();
        let agent = GitAgent::start(&dir, keys, tokens, Arc::new(move |id, fp| k2.audit("sign", &format!("key {id} {fp}"))), Arc::new(move |h| t2.audit("token", &format!("{h} handed to git"))))?;
        self.agent = Some(agent);
        Ok(())
    }

    fn start_item(&mut self, i: usize, jobs: &Jobs) -> Result<(), String> {
        let it = self.items[i].clone();
        if it.url.is_ssh() {
            if matches!(it.auth, Auth::None) {
                return Err(format!("no SSH key is set for {}. Choose one in Settings → Git and SSH keys (Host table), or use an https:// address.", it.url.host));
            }
            self.ctx.tools.ssh.clone().map_err(|e| e.clone())?;
        }
        // a person's own action needs no more approval; an assistant needs it once per key per session
        if let Auth::Key { id, .. } = &it.auth {
            if self.ctx.keys.session.needs_approval(self.ctx.who, id) {
                return Err(format!("the assistant needs your approval to use the key `{id}` (it asks once per session)"));
            }
        }
        let cwd = match it.op {
            Op::Clone => {
                // a retried clone must not land on files
                let p = self.ctx.root.join(&it.folder);
                if p.exists() && !gitsite::folder_free(&self.ctx.root, &it.folder, &HashSet::new()) {
                    return Err(format!("the folder `{}` has files in it now; remove it or choose another name", it.folder));
                }
                self.ctx.root.clone()
            }
            _ => crate::workspace::resolve_in(&self.ctx.root, &it.folder)?,
        };
        self.ensure_agent()?;
        let agent = self.agent.as_ref().ok_or("no key service")?;
        let argv = command(&self.ctx, &it, agent)?;
        let spec = Spec { label: it.title(), cmd: Cmd::Argv(argv), cwd, workspace: self.ctx.workspace.clone() };
        let scope = self.ctx.scope.clone();
        let job = jobs.start(&self.ctx.owner, &spec, &|p: &Path| scope(p))?;
        self.ctx.keys.audit_use(it.op.verb(), &it.url, &it.auth);
        let item = &mut self.items[i];
        item.job = Some(job.id);
        item.state = ItemState::Running;
        item.last.clear();
        Ok(())
    }
}

impl Ctx {
    /// Folder names already used by the repositories of the workspace's project.json (lower case).
    fn keys_taken(&self) -> HashSet<String> {
        crate::workspace::ProjectConfig::load(&self.root).map(|c| c.repos.iter().map(|r| r.path.to_ascii_lowercase()).collect()).unwrap_or_default()
    }
}

// ------------------------------------------------------------------ the command

/// Single-quote a word for `GIT_SSH_COMMAND` (git runs it through a shell).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `GIT_SSH_COMMAND` text: OpenSSH's ssh, the person's known hosts only, strict checking, no password prompts, the agent socket.
pub fn ssh_command(ssh: &Path, known_hosts: &Path, agent_sock: &Path) -> String {
    let opts = [
        "-F /dev/null".to_string(),
        "-o BatchMode=yes".into(),
        "-o StrictHostKeyChecking=yes".into(),
        "-o UpdateHostKeys=no".into(),
        "-o VerifyHostKeyDNS=no".into(),
        format!("-o UserKnownHostsFile={}", sh_quote(&known_hosts.display().to_string())),
        "-o GlobalKnownHostsFile=/dev/null".into(),
        format!("-o IdentityAgent={}", sh_quote(&agent_sock.display().to_string())),
        "-o IdentitiesOnly=no".into(),
        "-o PreferredAuthentications=publickey".into(),
        "-o ForwardAgent=no".into(),
        "-o ClearAllForwardings=yes".into(),
        "-o ConnectTimeout=20".into(),
        "-o ServerAliveInterval=15".into(),
    ];
    format!("{} {}", sh_quote(&ssh.display().to_string()), opts.join(" "))
}

/// The argument array of one job: `env VAR=… git …`. No secret is in it (sockets are paths, tokens come through askpass).
pub fn command(ctx: &Ctx, it: &Item, agent: &GitAgent) -> Result<Vec<String>, String> {
    let mut a: Vec<String> = vec!["env".into(), "GIT_TERMINAL_PROMPT=0".into(), "GIT_OPTIONAL_LOCKS=0".into(), "LC_ALL=C".into(), "GIT_CONFIG_COUNT=1".into(), "GIT_CONFIG_KEY_0=credential.helper".into(), "GIT_CONFIG_VALUE_0=".into()];
    a.push(if ctx.tools.allow_local { "GIT_ALLOW_PROTOCOL=https:http:ssh:file".into() } else { "GIT_ALLOW_PROTOCOL=https:http:ssh".into() });
    if !it.url.is_ssh() {
        // a server whose certificate the person chose to trust (Settings, Git, Connections): that certificate only
        let conns = ctx.keys.conns();
        if let Some(cert) = conns.for_host(&it.url.site()).and_then(|c| conns.pinned_cert(&c)) {
            a.push(format!("GIT_SSL_CAINFO={}", cert.display()));
        }
    }
    a.push(format!("GIT_ASKPASS={}", agent.askpass_program().display()));
    a.push(format!("{ASKPASS_ENV}={}", agent.askpass_sock().display()));
    if it.url.is_ssh() {
        let ssh = ctx.tools.ssh.clone()?;
        a.push(format!("SSH_AUTH_SOCK={}", agent.ssh_sock().display()));
        a.push(format!("GIT_SSH_COMMAND={}", ssh_command(&ssh, &ctx.keys.known_hosts_path(), &agent.ssh_sock())));
    }
    a.push(ctx.tools.git.display().to_string());
    match it.op {
        Op::Clone => {
            a.extend(["clone".into(), "--progress".into()]);
            if !it.spec.branch.is_empty() {
                a.extend(["--branch".into(), it.spec.branch.clone()]);
            }
            if it.spec.depth > 0 {
                a.extend(["--depth".into(), it.spec.depth.to_string()]);
            }
            a.extend(["--".into(), it.url.url.clone(), it.folder.clone()]);
        }
        Op::Fetch => a.extend(["fetch".into(), "--all".into(), "--prune".into(), "--progress".into()]),
        Op::Pull => a.extend(["pull".into(), "--ff-only".into(), "--no-rebase".into(), "--progress".into()]),
        Op::Push => a.extend(["push".into(), "--set-upstream".into(), "--progress".into(), "origin".into(), it.spec.branch.clone()]),
    }
    Ok(a)
}

// ------------------------------------------------------------------ status

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub branch: String,
    pub detached: bool,
    pub upstream: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

impl RepoStatus {
    pub fn dirty(&self) -> bool {
        self.changed + self.untracked + self.conflicted > 0
    }
    /// `main ↑1 ↓2 ✎3`.
    pub fn summary(&self) -> String {
        let mut s = if self.detached { "detached".to_string() } else if self.branch.is_empty() { "no commits".to_string() } else { self.branch.clone() };
        if self.ahead > 0 {
            s.push_str(&format!(" ↑{}", self.ahead));
        }
        if self.behind > 0 {
            s.push_str(&format!(" ↓{}", self.behind));
        }
        if self.changed + self.conflicted > 0 {
            s.push_str(&format!(" ✎{}", self.changed + self.conflicted));
        }
        if self.untracked > 0 {
            s.push_str(&format!(" +{}", self.untracked));
        }
        s
    }
}

/// Reads `git status --porcelain=v2 --branch`.
pub fn parse_status(out: &str) -> RepoStatus {
    let mut s = RepoStatus::default();
    for l in out.lines() {
        if let Some(r) = l.strip_prefix("# branch.head ") {
            if r == "(detached)" {
                s.detached = true;
            } else {
                s.branch = r.to_string();
            }
        } else if let Some(r) = l.strip_prefix("# branch.upstream ") {
            s.upstream = r.to_string();
        } else if let Some(r) = l.strip_prefix("# branch.ab ") {
            let mut it = r.split_whitespace();
            s.ahead = it.next().and_then(|x| x.trim_start_matches('+').parse().ok()).unwrap_or(0);
            s.behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if l.starts_with("1 ") || l.starts_with("2 ") {
            s.changed += 1;
        } else if l.starts_with("u ") {
            s.conflicted += 1;
        } else if l.starts_with("? ") {
            s.untracked += 1;
        }
    }
    s
}

/// Runs `git status` in a repository folder (read-only, no network, a few seconds at most).
pub fn status(git: &Path, dir: &Path) -> Result<RepoStatus, String> {
    let argv = vec![git.display().to_string(), "status".into(), "--porcelain=v2".into(), "--branch".into()];
    let env = vec![("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string()), ("PATH".to_string(), std::env::var("PATH").unwrap_or_default())];
    let o = crate::wsexec::run_argv(dir, &argv, &env, Duration::from_secs(20), 256 * 1024);
    if o.ok() {
        Ok(parse_status(&o.stdout))
    } else if o.timed_out {
        Err("git status took too long".into())
    } else {
        Err(crate::bugs::redact(o.combined().trim()).chars().take(200).collect())
    }
}

/// What would be lost by deleting a repository folder: a line for the confirmation card.
pub fn removal_risks(git: &Path, dir: &Path) -> String {
    match status(git, dir) {
        Ok(s) => {
            let mut v = vec![];
            if s.changed + s.conflicted > 0 {
                v.push(format!("{} file(s) with uncommitted changes", s.changed + s.conflicted));
            }
            if s.untracked > 0 {
                v.push(format!("{} untracked file(s)", s.untracked));
            }
            if s.ahead > 0 {
                v.push(format!("{} commit(s) not pushed", s.ahead));
            }
            if s.upstream.is_empty() && !s.branch.is_empty() {
                v.push("the branch has no upstream (its commits may exist only here)".into());
            }
            if v.is_empty() { "Nothing unsaved was found.".into() } else { format!("This would lose: {}.", v.join(", ")) }
        }
        Err(e) => format!("The folder could not be checked ({e}); anything not pushed would be lost."),
    }
}

/// Deletes a repository folder of the workspace and its entry in project.json. `typed` must be the folder name.
pub fn remove_repo(root: &Path, folder: &str, typed: &str) -> Result<(), String> {
    if typed.trim() != folder {
        return Err(format!("type the folder name `{folder}` exactly to confirm"));
    }
    let p = crate::workspace::resolve_in(root, folder)?;
    let root_real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if p == root_real || !p.starts_with(&root_real) {
        return Err("that folder is not inside the workspace".into());
    }
    let meta = std::fs::symlink_metadata(root.join(folder)).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        return Err("that folder is a link; remove the link yourself".into());
    }
    std::fs::remove_dir_all(&p).map_err(|e| format!("could not remove {folder}: {e}"))?;
    let mut c = crate::workspace::ProjectConfig::load(root)?;
    c.repos.retain(|r| r.path != folder);
    c.save(root)
}

#[cfg(test)]
#[path = "gitjobs_tests.rs"]
mod tests;
