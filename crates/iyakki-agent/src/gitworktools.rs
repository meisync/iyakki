//! The lead assistant's tools for getting a repository to work on (see `docs/GIT-TILE.md`, `docs/AI-IDE.md`): `git_clone_repo` copies
//! a repository (optionally with its submodules) into the person's Repositories folder, `ide_workspace_add` registers a folder as an AI
//! IDE workspace, and `git_clones_list` and `ide_workspace_list` read. Together with `read_file`, `cluster_status` and `cluster_start`
//! a request such as "make a workspace from this repository, clone its submodules, read the deployment documents and start k3s" is
//! done from the chat, each risky step on a card.
//!
//! * `git_clone_repo` and `ide_workspace_add` are on the hard-ask list (`agent::hard_ask`): they ask every time, in Auto mode too, and
//!   "Allow for this session" does not remember them. The card says what, from which host, into which folder, with or without
//!   submodules, and how the clone signs in.
//! * One implementation: the clone is the Git tile's own job ([`crate::gitjobs::Batch`]), set up by [`crate::gitjobs::user_ctx`].
//! * No secret passes through the chat. A private repository is cloned with the person's saved Git connection for that host (the card
//!   names it); text that looks like a credential is refused without being echoed. Only `https://` addresses are cloned from here
//!   (an ssh clone needs the person to confirm the host key: the Git tile does that), and a clone with submodules only follows
//!   `https` submodules ([`crate::gitjobs::submodule_protocols`]).
//! * A folder is registered only inside the person's own roots ([`crate::scope::FileRoots`] or their Repositories folder) and only
//!   where their account may write.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::gitjobs::{Batch, CloneSpec, Ctx, ItemState};
use crate::gitkeys::AuditFn;
use crate::gitsite;
use crate::wsexec::Runner;

pub const PROMPT_RULE: &str = "\nGIT AND WORKSPACES: git_clone_repo copies a repository into the person's Repositories folder (`submodules`: true also clones its submodules; leave it out unless asked). It asks every time and takes an https:// address only; for a private repository the person's saved Git connection for that host is used by itself, so never ask for or send a token or password (if there is none, tell the person to add a connection in the Git tile). git_clones_list shows the clones on this computer. ide_workspace_add registers a folder as an AI IDE workspace (once, it asks); ide_workspace_list shows them. After a clone, read the deployment documents with read_file and use cluster_status and cluster_start for the cluster.";

pub const WRITE_TOOLS: [&str; 2] = ["git_clone_repo", "ide_workspace_add"];
pub const READ_TOOLS: [&str; 2] = ["git_clones_list", "ide_workspace_list"];

pub fn is_gitwork_tool(n: &str) -> bool {
    WRITE_TOOLS.contains(&n) || READ_TOOLS.contains(&n)
}

/// Whether this account is offered the tools at all (the Git tile and the AI IDE are apps of the role).
pub fn offered(p: &Policy) -> bool {
    p.allows_app(App::Git) || p.allows_app(App::Ide)
}

/// `/home/<user>/Repositories`, or the account's folder under the data folder when it has no home to write in; the second value is the
/// folder the jobs' write check must also allow. The Git tile and these tools clone into the same place.
pub fn default_repos_root(data_dir: &Path, user: &str, policy: &Policy) -> (PathBuf, Option<PathBuf>) {
    let home = PathBuf::from(format!("/home/{user}/Repositories"));
    let fb = data_dir.join("repositories").join(user);
    let usable = policy.allows_write(user, &home.to_string_lossy()) && home.parent().is_some_and(|p| p.is_dir());
    if usable { (home, None) } else { (fb.clone(), Some(fb)) }
}

/// Everything the tools need from the system. The shell builds it with [`GitWorkEnv::for_services`]; tests give fakes.
pub struct GitWorkEnv {
    pub user: String,
    pub policy: Policy,
    pub audit: AuditFn,
    /// The services folder (the workspace list lives there).
    pub dir: PathBuf,
    /// The person's home folder (a root for workspaces).
    pub home: PathBuf,
    pub repos_root: PathBuf,
    /// Passed to [`Ctx`]'s write check besides the account's scope.
    pub extra_write: Option<PathBuf>,
    /// The jobs' context for a folder and its name.
    pub make_ctx: Arc<dyn Fn(&Path, &str) -> Result<Ctx, String> + Send + Sync>,
    /// One step of a running batch (`batch.tick(&jobs)`).
    pub tick: Arc<dyn Fn(&mut Batch) + Send + Sync>,
    pub runner: Arc<dyn Runner>,
    /// How long a clone call waits for the end before it says the clone goes on.
    pub wait: Duration,
}

impl GitWorkEnv {
    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, policy: &Policy, audit: AuditFn) -> GitWorkEnv {
        let (repos_root, extra_write) = default_repos_root(svc.dir(), user, policy);
        let (s1, s2, p1, u1, a1, e1) = (svc.clone(), svc.clone(), policy.clone(), user.to_string(), audit.clone(), extra_write.clone());
        GitWorkEnv {
            user: user.to_string(),
            policy: policy.clone(),
            audit,
            dir: svc.dir().to_path_buf(),
            home: crate::scope::home_root(user, svc.dir()),
            repos_root,
            extra_write,
            make_ctx: Arc::new(move |root, name| {
                crate::gitjobs::user_ctx(&s1, crate::gitjobs::CtxFor { user: &u1, policy: &p1, audit: a1.clone(), root, name, extra_write: e1.as_deref(), who: crate::gitkeys::Who::Assistant, tools: None, probe: None })
            }),
            tick: Arc::new(move |b| b.tick(&s2.jobs)),
            runner: Arc::new(crate::wsexec::System),
            wait: Duration::from_secs(25),
        }
    }
}

// ------------------------------------------------------------------ clones in progress

#[derive(Clone, Debug, PartialEq, Eq)]
enum RunState {
    Running(String),
    Done(String),
    Failed(String),
}

#[derive(Clone)]
struct CloneRun {
    user: String,
    /// The Repositories folder it goes into.
    root: PathBuf,
    label: String,
    state: Arc<Mutex<RunState>>,
}

/// The clones this program started, newest last (the list tool shows them; a clone outlives the chat run that started it).
fn runs() -> &'static Mutex<Vec<CloneRun>> {
    static R: std::sync::OnceLock<Mutex<Vec<CloneRun>>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// What a request for a clone comes to, after every check and before anything is started.
struct Plan {
    url: gitsite::RepoUrl,
    spec: CloneSpec,
    folder: String,
    root: PathBuf,
    signs_in: String,
}

impl Plan {
    fn dir(&self) -> PathBuf {
        self.root.join(&self.folder)
    }
}

pub struct GitWorkTools {
    inner: Arc<dyn Tools>,
    env: Arc<GitWorkEnv>,
}

impl GitWorkTools {
    pub fn new(inner: Arc<dyn Tools>, env: GitWorkEnv) -> GitWorkTools {
        GitWorkTools { inner, env: Arc::new(env) }
    }

    /// The same tools over an environment that is shared (a chat builds one per message; tests wrap different inner tools).
    pub fn with_env(inner: Arc<dyn Tools>, env: Arc<GitWorkEnv>) -> GitWorkTools {
        GitWorkTools { inner, env }
    }

    fn may(&self, app: App, what: &str) -> Result<(), String> {
        if self.env.policy.allows_app(app) { Ok(()) } else { Err(format!("this account may not {what}")) }
    }

    /// Every check of a clone request; nothing is created and no network is used.
    fn plan(&self, args: &Value) -> Result<Plan, String> {
        reject_secrets(args)?;
        self.may(App::Git, "use Git")?;
        let e = &self.env;
        let ctx = (e.make_ctx)(&e.repos_root, "Repositories")?;
        let raw = text(args, "url");
        if raw.contains('?') || raw.contains('#') {
            return Err("a repository address has no `?` or `#` part (a secret could hide in it): give the plain https:// address".into());
        }
        let url = gitsite::parse(&raw, gitsite::ParseOptions { allow_local: ctx.tools.allow_local })?;
        match url.scheme {
            gitsite::Scheme::Https | gitsite::Scheme::Local => {}
            gitsite::Scheme::Http => return Err("http:// is not encrypted; use the https:// address".into()),
            gitsite::Scheme::Ssh => return Err("an ssh address needs your key and your confirmation of the host key: clone it in the Git tile, or give the https:// address".into()),
        }
        if !url.host.is_empty() && !e.policy.allows_host(&url.host) {
            return Err(format!("this account may not reach {}", url.host));
        }
        if !e.policy.allows_write(&e.user, &e.repos_root.to_string_lossy()) && e.extra_write.is_none() {
            return Err(format!("this account may not write in the Repositories folder ({})", e.repos_root.display()));
        }
        let depth = match args.get("depth") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|d| (1..=1_000_000).contains(d)).ok_or("depth is a number of commits, 1 or more")? as u32,
        };
        let spec = CloneSpec { url: url.url.clone(), branch: text(args, "branch"), depth, folder: String::new(), submodules: args.get("submodules").and_then(Value::as_bool).unwrap_or(false) };
        let conns = ctx.keys.conns();
        let conn = conns.for_host(&url.host);
        let named = text(args, "connection");
        if !named.is_empty() {
            let all = conns.list();
            let Some(c) = all.iter().find(|c| c.name.eq_ignore_ascii_case(&named) || c.id == named) else {
                let names: Vec<String> = all.iter().map(|c| c.name.clone()).collect();
                return Err(format!("there is no Git connection called {named}{}", if names.is_empty() { " (add one in the Git tile)".into() } else { format!("; the person's are: {}", names.join(", ")) }));
            };
            if !c.host().eq_ignore_ascii_case(&url.host) {
                return Err(format!("the connection {} is for {}, not {}; a connection is only used for its own host", c.name, c.host(), url.host));
            }
        }
        let batch = Batch::plan_clone(ctx.clone(), std::slice::from_ref(&spec)).map_err(|errs| errs.into_iter().map(|x| x.1).collect::<Vec<_>>().join("; "))?;
        let folder = batch.items.first().map(|i| i.folder.clone()).unwrap_or_default();
        let signs_in = match conn {
            Some(c) if c.has_token() => format!("your saved connection “{}” ({})", c.name, c.auth_label()),
            Some(c) => format!("your saved connection “{}” has no token, so only public repositories can be cloned", c.name),
            None => "nothing (no saved Git connection for this host: public repositories only)".into(),
        };
        Ok(Plan { url, spec, folder, root: ctx.root.clone(), signs_in })
    }

    fn clone_card(&self, p: &Plan) -> String {
        let s = &p.spec;
        format!(
            "Clone {} from {} into your Repositories folder, {} submodules.\n• Address: {}\n• From host: {}\n• Into: {} on this computer\n• Branch: {}; history: {}\n• Submodules: {}\n• Signs in with: {}\n• The files come to this computer; nothing on the host changes.",
            p.url.short(),
            if p.url.host.is_empty() { "a local path".to_string() } else { p.url.host.clone() },
            if s.submodules { "with" } else { "without" },
            gitsite::redact_url(&p.url.url),
            if p.url.host.is_empty() { "this computer".to_string() } else { p.url.host.clone() },
            p.dir().display(),
            if s.branch.is_empty() { "the default branch".to_string() } else { s.branch.clone() },
            if s.depth > 0 { format!("only the last {} commit(s)", s.depth) } else { "all".to_string() },
            if s.submodules { "ALSO cloned: the repositories it links to (listed in .gitmodules), over https only; file:// and program addresses are refused; they may be large" } else { "not cloned (their folders stay empty)" },
            p.signs_in,
        )
    }

    // ---- the workspace

    /// The folder, its name and the registry, after every check of an add.
    fn wanted_workspace(&self, args: &Value) -> Result<(PathBuf, String), String> {
        reject_secrets(args)?;
        self.may(App::Ide, "use the AI IDE")?;
        let e = &self.env;
        let raw = text(args, "path");
        if raw.is_empty() {
            return Err("give the folder: its full path, or the name of a folder in the Repositories folder (git_clones_list shows them)".into());
        }
        let given = if let Some(rest) = raw.strip_prefix("~/") { e.home.join(rest) } else { PathBuf::from(&raw) };
        let given = if given.is_absolute() { given } else { e.repos_root.join(given) };
        let real = std::fs::canonicalize(&given).map_err(|_| format!("there is no folder {}", given.display()))?;
        if !real.is_dir() {
            return Err(format!("{} is not a folder", real.display()));
        }
        let repos_real = std::fs::canonicalize(&e.repos_root).unwrap_or_else(|_| e.repos_root.clone());
        let roots = crate::scope::FileRoots::with_home(&e.policy, &e.user, &e.dir, e.home.clone());
        let inside = (roots.contains(&real) || real.starts_with(&repos_real)) && real != roots.home && real != repos_real;
        let fb = e.extra_write.as_ref().is_some_and(|f| real.starts_with(std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())));
        if !inside || !(roots.write_ok(&e.policy, &e.user, &real) || fb) {
            return Err(format!("{} is outside the folders this account may use for a workspace (its home and Repositories folders)", real.display()));
        }
        let name = {
            let n = text(args, "name");
            if n.is_empty() { real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { n }
        };
        crate::workspace::Registry::load(&e.dir, &e.user).check_add(&name, &real).map_err(|m| if m.contains("is already the workspace") { format!("{m} (nothing to do)") } else { m })?;
        Ok((real, name))
    }

    fn workspace_card(&self, real: &Path, name: &str) -> String {
        format!("Add the folder {} as the AI IDE workspace “{name}”.\n• Folder: {}\n• The AI IDE then lists it with its own chat and board; nothing in the folder changes.\n• It is added once; remove it from the AI IDE's list later if you do not want it.", real.display(), real.display())
    }

    // ---- running a clone

    fn start_clone(&self, args: &Value) -> Result<String, String> {
        let plan = self.plan(args)?;
        let e = &self.env;
        std::fs::create_dir_all(&e.repos_root).map_err(|er| format!("the Repositories folder {} could not be made: {er}", e.repos_root.display()))?;
        crate::workspace::ProjectConfig::ensure(&e.repos_root, "Repositories")?;
        let ctx = (e.make_ctx)(&e.repos_root, "Repositories")?;
        let batch = Batch::plan_clone(ctx, std::slice::from_ref(&plan.spec)).map_err(|errs| errs.into_iter().map(|x| x.1).collect::<Vec<_>>().join("; "))?;
        let label = format!("{}{}", plan.url.short(), if plan.spec.submodules { " with submodules" } else { "" });
        (e.audit)("assistant.git.clone", &format!("{} from {} into Repositories/{}{}", plan.url.short(), if plan.url.host.is_empty() { "a local path" } else { &plan.url.host }, plan.folder, if plan.spec.submodules { " with submodules" } else { "" }));
        let state = Arc::new(Mutex::new(RunState::Running("starting".into())));
        runs().lock().unwrap_or_else(|p| p.into_inner()).push(CloneRun { user: e.user.clone(), root: e.repos_root.clone(), label: label.clone(), state: state.clone() });
        let (tick, dir, submodules, watch) = (e.tick.clone(), plan.dir(), plan.spec.submodules, state.clone());
        std::thread::spawn(move || {
            let mut b = batch;
            loop {
                tick(&mut b);
                if let Some(it) = b.items.first() {
                    *state.lock().unwrap_or_else(|p| p.into_inner()) = RunState::Running(format!("{}{}", it.last, it.progress.map(|p| format!(" ({p:.0}%)")).unwrap_or_default()));
                }
                // a host key to decide about is the person's to trust, in the Git tile: never from here
                for pr in b.prompts.clone() {
                    b.refuse_host(&pr.host, pr.port);
                }
                if !b.pending() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            let end = match b.items.first().map(|i| i.state.clone()) {
                Some(ItemState::Done) => RunState::Done(done_text(&dir, submodules)),
                Some(ItemState::Failed(m)) => RunState::Failed(m),
                _ => RunState::Failed("the clone did not run".into()),
            };
            *state.lock().unwrap_or_else(|p| p.into_inner()) = end;
        });
        let t = Instant::now();
        loop {
            let s = Some(watch.lock().unwrap_or_else(|p| p.into_inner()).clone());
            match s {
                Some(RunState::Done(m)) => return Ok(format!("Cloned {label} into {}. {m} Register it with ide_workspace_add (path: {}).", plan.dir().display(), plan.folder)),
                Some(RunState::Failed(m)) => return Err(format!("The clone of {label} failed: {m}")),
                _ => {}
            }
            if t.elapsed() >= e.wait {
                return Ok(format!("The clone of {label} into {} is still going on in the background. git_clones_list shows how far it is.", plan.dir().display()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn list_clones(&self) -> Result<String, String> {
        let e = &self.env;
        let mut out: Vec<String> = vec![];
        let reg = crate::workspace::Registry::load(&e.dir, &e.user);
        for c in crate::gitbrowse::find_clones(&e.runner, &e.repos_root, &[]) {
            let real = std::fs::canonicalize(&c.dir).unwrap_or_else(|_| c.dir.clone());
            let ws = reg.items.iter().find(|w| Path::new(&w.root) == real).map(|w| format!("AI IDE workspace “{}”", w.name)).unwrap_or_else(|| "not an AI IDE workspace yet".into());
            let subs = std::fs::read_to_string(c.dir.join(".gitmodules")).map(|t| gitsite::submodule_urls(&t).len()).unwrap_or(0);
            out.push(format!("{} · {} · origin {} · branch {} · {} submodule(s) · {ws}", c.name, c.dir.display(), c.remote.as_deref().unwrap_or("none"), if c.branch.is_empty() { "none" } else { &c.branch }, subs));
        }
        let mine: Vec<String> = runs().lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|r| r.user == e.user && r.root == e.repos_root).map(|r| match r.state.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            RunState::Running(m) => format!("{}: cloning now ({m})", r.label),
            RunState::Done(_) => format!("{}: finished", r.label),
            RunState::Failed(m) => format!("{}: failed ({m})", r.label),
        }).collect();
        if out.is_empty() && mine.is_empty() {
            return Ok(format!("No clones in {} yet.", e.repos_root.display()));
        }
        if !mine.is_empty() {
            out.push(format!("Started from the chat: {}", mine.join("; ")));
        }
        Ok(out.join("\n"))
    }

    fn list_workspaces(&self) -> Result<String, String> {
        let reg = crate::workspace::Registry::load(&self.env.dir, &self.env.user);
        if reg.items.is_empty() {
            return Ok("No AI IDE workspaces yet.".into());
        }
        Ok(reg.ordered().iter().map(|w| format!("{} · {}{}", w.name, w.root, if w.pinned { " · pinned" } else { "" })).collect::<Vec<_>>().join("\n"))
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "git_clone_repo" => self.plan(args).map(|p| self.clone_card(&p)),
            "ide_workspace_add" => self.wanted_workspace(args).map(|(r, n)| self.workspace_card(&r, &n)),
            _ => Err("not a git tool".into()),
        }
    }
}

/// What the end of a good clone says about the submodules.
fn done_text(dir: &Path, asked: bool) -> String {
    let urls = std::fs::read_to_string(dir.join(".gitmodules")).map(|t| gitsite::submodule_urls(&t)).unwrap_or_default();
    match (asked, urls.len()) {
        (true, 0) => "It has no submodules (.gitmodules is empty or missing).".into(),
        (true, n) => format!("Its {n} submodule(s) from .gitmodules were cloned too."),
        (false, 0) => String::new(),
        (false, n) => format!("It lists {n} submodule(s) in .gitmodules that were not cloned (clone again with submodules: true to get them)."),
    }
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Text that looks like a token, a password or a key is refused, and never repeated; so is anything that is not a plain short setting.
fn reject_secrets(args: &Value) -> Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::String(s) => s.len() > 600 || s.contains('\n') || crate::credguard::detect(s).is_some(),
            Value::Array(a) => a.iter().any(walk),
            Value::Object(o) => o.values().any(walk),
            _ => false,
        }
    }
    if walk(args) {
        return Err("a value looks like a token, a password or a key, or is not a plain short setting. Never send secrets in the chat: a private repository uses the person's saved Git connection for its host (the person adds it in the Git tile).".into());
    }
    Ok(())
}

impl Tools for GitWorkTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_gitwork_tool(&d.spec.name));
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        let safe = |name: &str, desc: &str| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false };
        let p = &self.env.policy;
        if p.allows_app(App::Git) {
            v.push(ask(
                "git_clone_repo",
                "Clone a git repository into the person's Repositories folder. The person approves a card that says which repository, from which host, into which folder, with or without submodules. https:// addresses only. A private repository uses the person's saved Git connection for that host: never ask for or send a token. Add submodules: true only when asked for them.",
                json!({"type": "object", "properties": {
                    "url": {"type": "string", "description": "The https:// address of the repository."},
                    "branch": {"type": "string", "description": "A branch or tag to check out (the default branch if left out)."},
                    "depth": {"type": "integer", "description": "Only the last N commits (a shallow clone; faster)."},
                    "submodules": {"type": "boolean", "description": "Also clone its submodules (default false)."},
                    "connection": {"type": "string", "description": "The NAME of a saved Git connection to check it is the one for this host (optional; the host's own connection is used anyway)."}
                }, "required": ["url"]}),
            ));
            v.push(safe("git_clones_list", "List the git clones on this computer (Repositories folder) with their origin, branch, submodules and whether each is an AI IDE workspace, and the clones started from the chat."));
        }
        if p.allows_app(App::Ide) {
            v.push(ask(
                "ide_workspace_add",
                "Register an existing folder as an AI IDE workspace (once). The folder must be in the person's home or Repositories folder. Returns the workspace name.",
                json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "The folder's full path, or the name of a folder in the Repositories folder."},
                    "name": {"type": "string", "description": "The workspace's name (1 to 40 characters); the folder's name if left out."}
                }, "required": ["path"]}),
            ));
            v.push(safe("ide_workspace_list", "List the person's AI IDE workspaces (name and folder)."));
        }
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "git_clone_repo" => self.start_clone(args),
            "git_clones_list" => self.may(App::Git, "use Git").and_then(|_| self.list_clones()),
            "ide_workspace_add" => {
                let (real, wname) = self.wanted_workspace(args)?;
                let ws = crate::workspace::Registry::load(&self.env.dir, &self.env.user).add(&wname, &real)?;
                (self.env.audit)("ide.workspace.add", &ws.root);
                Ok(format!("Registered {} as the AI IDE workspace “{}”. Open it from the AI IDE tile.", ws.root, ws.name))
            }
            "ide_workspace_list" => self.may(App::Ide, "use the AI IDE").and_then(|_| self.list_workspaces()),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        match name {
            "git_clones_list" => "List the git clones on this computer".into(),
            "ide_workspace_list" => "List the AI IDE workspaces".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if READ_TOOLS.contains(&name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if READ_TOOLS.contains(&name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_gitwork_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("cloning repositories and adding workspaces asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }

    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }

    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}
