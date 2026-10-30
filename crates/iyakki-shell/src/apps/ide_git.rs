//! The AI IDE's git support: clone several repositories into one workspace, see their state, fetch and pull them all, remove one.
//! The rules and the work are in `iyakki_agent::{gitjobs, gitkeys, gitsite, gitagent}` (see `docs/GIT-AND-SSH.md`); the dialogs are in
//! `ide_git_ui.rs`; this file is the glue: it makes the batch for a request, ticks it, shows the cards that need a decision (a new
//! host key, a pull over local changes, removing a folder) and keeps the tree's badges and project.json's `repos` list current.
//!
//! Everything that leaves the machine or deletes files waits for a click: a first-time SSH host shows its `SHA256:` fingerprint, a
//! pull over local changes asks, and removing a folder needs its name typed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::gitjobs::{self, Batch, GitCheck, ItemState, Op, Phase, RepoStatus};
use iyakki_agent::gitkeys::{GitKeys, Session, Who};
use iyakki_agent::gitsite::RepoEntry;
use iyakki_agent::workspace::ProjectConfig;

use super::card::{Action, Card, Tone};
use super::ide::{FormKind, IdeApp, Modal};
use super::git_wizard::{GitWizard, WizEnv, WizOut};
use super::ide_git_connect::{ConnectFlow, FlowEnv, FlowOut};
use super::ide_git_ui::{CloneDialog, DlgEvent, RepoRow, ReposAct, ReposPanel, WsHead};
use iyakki_agent::gitconn::{self, WsGit};
use iyakki_agent::platform::Platform;
use iyakki_agent::wsexec::Runner;
use super::ide_tile::{CardAction, CardModal};
use super::widgets::Form;
use super::AppCx;
use crate::fx::AMBER;
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

const OKC: [u8; 4] = rgb(110, 224, 160);
const BADC: [u8; 4] = rgb(255, 110, 120);
const MUTE: [u8; 4] = rgb(150, 160, 190);
/// How often the repositories' state is read again while the workspace is on screen.
const STATUS_EVERY: Duration = Duration::from_secs(30);

type StatusRows = Vec<(String, Result<RepoStatus, String>)>;

#[derive(Default)]
struct StatusBoard {
    rows: StatusRows,
    at: Option<Instant>,
    rx: Option<Receiver<StatusRows>>,
    stale: bool,
}

/// What the project's own git setup looks like, read in the background (the header of the panel, the top bar's chip).
#[derive(Default)]
struct HeadBoard {
    state: Option<WsGit>,
    head: Option<WsHead>,
    at: Option<Instant>,
    rx: Option<Receiver<WsGit>>,
    stale: bool,
}

/// What the IDE keeps for git, per workspace id.
pub struct GitUi {
    /// Tests: runs git for the project's own repository (`origin`, the first commit) instead of the system's.
    pub runner: Option<Arc<dyn Runner>>,
    /// Tests: answers the host's API (GitHub, GitLab, Gitea) instead of the network.
    pub http: Option<Arc<dyn gitconn::GitHttp>>,
    /// Tests: the platform services' context for connecting a workspace to the cluster's Gitea.
    pub platform: Option<Arc<Platform>>,
    heads: HashMap<String, HeadBoard>,
    pub session: Arc<Session>,
    /// Tests: what answers "which host key does this server present" instead of the real SSH client.
    pub probe: Option<gitjobs::Probe>,
    batches: HashMap<String, Batch>,
    status: HashMap<String, StatusBoard>,
    /// A line for the panel (what the last button did).
    pub note: String,
    announced: bool,
    /// The host-key card for this workspace is open (so it is not opened twice).
    card_open: bool,
}

impl Default for GitUi {
    fn default() -> Self {
        GitUi { runner: None, http: None, platform: None, heads: HashMap::new(), session: iyakki_agent::gitkeys::shared_session(), probe: None, batches: HashMap::new(), status: HashMap::new(), note: String::new(), announced: false, card_open: false }
    }
}

impl GitUi {
    /// A finished batch (or none) for this workspace id.
    pub fn batches_finished(&self, id: &str) -> bool {
        self.batches.get(id).is_some_and(|b| !b.pending())
    }
    pub fn batches_empty(&self) -> bool {
        self.batches.is_empty()
    }
    #[cfg(test)]
    pub fn set_status_for_test(&mut self, id: &str, rows: StatusRows) {
        let b = self.status.entry(id.to_string()).or_default();
        b.rows = rows;
        b.at = Some(Instant::now());
        b.stale = false;
    }
}

pub enum GitModal {
    Clone(Box<CloneDialog>),
    Repos(ReposPanel),
    /// "Connect Git / Gitea" for the open project (`ide_git_connect.rs`).
    Connect(Box<ConnectFlow>),
    /// The connection wizard of Settings → Git, in place.
    Wizard(Box<GitWizard>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitForm {
    /// Test and run commands of the repository in this folder.
    Commands(String),
    /// Type the folder name to remove it.
    RemoveType(String),
    /// The passphrase of this key.
    Passphrase(String),
    /// This workspace's own key for a host.
    WsKey,
}

pub enum GitCard {
    /// A host key never seen before: (host, port).
    Host(String, u16),
    /// Pull: some repositories have local changes. (the clean ones, all of them)
    PullDirty(Vec<RepoEntry>, Vec<RepoEntry>),
    /// Removing a folder: what would be lost, then the typed confirmation.
    RemoveRisk(String),
    /// Disconnecting the project from its git host (removes `origin`).
    Disconnect,
}

static ANNOUNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl GitModal {
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool, rows: &[RepoRow], note: &str) {
        match self {
            GitModal::Clone(d) => d.render(g, area, s, accent, blink),
            GitModal::Repos(p) => p.render(g, area, s, accent, rows, note),
            GitModal::Connect(f) => f.render(g, area, s, accent, blink),
            GitModal::Wizard(w) => w.render(g, area, s, accent, blink),
        }
    }
}

impl IdeApp {
    // ------------------------------------------------------------------------------------------------ the pieces

    pub fn git_keys(&self, cx: &AppCx) -> Arc<GitKeys> {
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        Arc::new(GitKeys::for_services(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)), self.git.session.clone()))
    }

    pub fn git_check(&self, cx: &AppCx) -> GitCheck {
        gitjobs::check_git(&cx.services.clis.search_path())
    }

    /// Said once per start of the shell when git is missing (the libre image): the AI IDE still works, cloning does not.
    pub fn git_announce_missing(&mut self, cx: &AppCx) {
        if ANNOUNCED.swap(true, std::sync::atomic::Ordering::SeqCst) || self.git.announced {
            return;
        }
        self.git.announced = true;
        if let GitCheck::Missing { message } = self.git_check(cx) {
            self.notice = Some((message, false));
        }
    }

    fn git_ctx(&self, i: usize, cx: &AppCx) -> Result<gitjobs::Ctx, String> {
        let w = self.ws.get(i).ok_or("Open a workspace first")?;
        let search = cx.services.clis.search_path();
        let tools = gitjobs::Tools::detect(&search)?;
        let user = cx.session.user.clone();
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        let policy = cx.session.policy.clone();
        let u2 = user.clone();
        Ok(gitjobs::Ctx {
            keys: self.git_keys(cx),
            tools,
            owner: user,
            workspace: w.meta.name.clone(),
            root: std::fs::canonicalize(w.root()).unwrap_or_else(|_| w.root()),
            ws_keys: w.cfg.git_keys.clone(),
            who: Who::Person,
            run_dir: cx.services.dir().join("git-run").join(safe),
            probe: self.git.probe.clone().unwrap_or_else(|| gitjobs::real_probe(search.clone())),
            scope: Arc::new(move |p: &Path| iyakki_agent::scope::check_write(&policy, &u2, p)),
        })
    }

    fn ws_id(&self, i: usize) -> String {
        self.ws.get(i).map(|w| w.meta.id.clone()).unwrap_or_default()
    }

    fn reload_repos(&mut self, i: usize) {
        if let Some(w) = self.ws.get_mut(i) {
            if let Ok(c) = ProjectConfig::load(&w.root()) {
                w.cfg.repos = c.repos;
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ opening

    /// "Clone repositories…" (top bar ⋯ menu, the workspace switcher, the new-workspace form).
    pub fn open_clone_dialog(&mut self, new_workspace: bool, cx: &AppCx) {
        let git = self.git_check(cx);
        let (name, ws_keys) = match self.active_ws() {
            Some(w) if !new_workspace => (Some(w.meta.name.clone()), w.cfg.git_keys.clone()),
            _ => (None, Default::default()),
        };
        let keys = self.git_keys(cx);
        let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
        let d = CloneDialog::new(keys, ws_keys, name, &git, "").with_instances(Arc::new(move |t| gitjobs::expand_instance(&dir, &user, t)));
        self.modal = Some(Modal::Git(Box::new(GitModal::Clone(Box::new(d)))));
    }

    pub fn open_repos_panel(&mut self, cx: &AppCx) {
        if self.active_ws().is_none() {
            self.notice = Some(("Open a workspace first.".into(), false));
            return;
        }
        self.git.note.clear();
        self.show_repos(cx);
    }

    /// The panel again, keeping the line the last action left in it.
    fn show_repos(&mut self, cx: &AppCx) {
        let git = self.git_check(cx);
        if let Some(i) = self.active {
            self.git_mark_stale(i);
            self.git_mark_head_stale(i);
        }
        self.modal = Some(Modal::Git(Box::new(GitModal::Repos(ReposPanel::new(git)))));
    }

    fn git_mark_stale(&mut self, i: usize) {
        let id = self.ws_id(i);
        self.git.status.entry(id).or_default().stale = true;
    }

    // ------------------------------------------------------------------------------------------------ the rows

    /// The panel's lines for the active workspace, and for each the repository index and the batch item it stands for.
    pub fn repo_rows(&self) -> (Vec<RepoRow>, Vec<(Option<usize>, Option<usize>)>) {
        let Some(i) = self.active else { return (vec![], vec![]) };
        let w = &self.ws[i];
        let id = &w.meta.id;
        let batch = self.git.batches.get(id);
        let status = self.git.status.get(id);
        let mut rows = Vec::new();
        let mut map = Vec::new();
        let mut used = vec![false; batch.map_or(0, |b| b.items.len())];
        for (ri, r) in w.cfg.repos.iter().enumerate() {
            let item = batch.and_then(|b| b.items.iter().position(|it| it.folder == r.path && it.state != ItemState::Done));
            let item = item.or_else(|| batch.and_then(|b| b.items.iter().position(|it| it.folder == r.path)));
            if let Some(k) = item {
                used[k] = true;
            }
            let st = status.and_then(|s| s.rows.iter().find(|(p, _)| *p == r.path)).map(|(_, v)| match v {
                Ok(s) => s.summary(),
                Err(e) => format!("({e})"),
            });
            let (work, failed, last) = item.and_then(|k| batch.map(|b| work_text(&b.items[k]))).unwrap_or_default();
            rows.push(RepoRow { folder: r.path.clone(), short: gitsite_short(&r.url), status: st.unwrap_or_else(|| "…".into()), work, work_failed: failed, last, test: r.test.clone(), run: r.run.clone() });
            map.push((Some(ri), item));
        }
        if let Some(b) = batch {
            for (k, it) in b.items.iter().enumerate() {
                if used[k] || !matches!(it.op, Op::Clone | Op::Push) {
                    continue;
                }
                let (work, failed, last) = work_text(it);
                rows.push(RepoRow { folder: it.folder.clone(), short: it.url.short(), status: String::new(), work, work_failed: failed, last, test: String::new(), run: String::new() });
                map.push((None, Some(k)));
            }
        }
        (rows, map)
    }

    // ------------------------------------------------------------------------------------------------ events

    pub fn git_modal_event(&mut self, gm: GitModal, ev: &Event, cx: &AppCx) {
        match gm {
            GitModal::Clone(mut d) => match d.handle(ev) {
                DlgEvent::Cancel => {}
                DlgEvent::None => self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d)))),
                DlgEvent::Submit => self.submit_clone(d, cx),
            },
            GitModal::Connect(f) => self.connect_flow_event(f, ev, cx),
            GitModal::Wizard(w) => self.wizard_event(w, ev, cx),
            GitModal::Repos(mut p) => {
                let (rows, map) = self.repo_rows();
                let act = p.handle(ev, &rows);
                self.modal = Some(Modal::Git(Box::new(GitModal::Repos(p))));
                self.repos_act(act, &map, cx);
            }
        }
    }

    fn submit_clone(&mut self, mut d: Box<CloneDialog>, cx: &AppCx) {
        if d.git_missing.is_some() {
            self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
            return;
        }
        let specs = match d.specs() {
            Ok(s) => s,
            Err(errs) => {
                d.error = Some(errs.iter().take(3).map(|(n, e)| if *n == 0 { e.clone() } else { format!("line {n}: {e}") }).collect::<Vec<_>>().join("   "));
                self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
                return;
            }
        };
        // a line that names a platform service uses IYAKKI's token for it (copied vault to vault, when the host has none yet)
        let keys = self.git_keys(cx);
        for r in &d.rows {
            if let Some(v) = &r.via {
                if let Some(vault) = &v.token_vault {
                    match keys.adopt_token(&v.host, "git", vault) {
                        Ok(true) => cx.audit.record(&cx.session.user, "git.token.adopt", &format!("{} from {}", v.host, v.instance)),
                        Ok(false) => {}
                        Err(e) => {
                            d.error = Some(format!("The token of {} could not be used: {e}", v.instance));
                            self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
                            return;
                        }
                    }
                }
            }
        }
        if d.target_new {
            let name = d.new_name.value.trim().to_string();
            if name.is_empty() {
                d.error = Some("Give the new workspace a name.".into());
                d.focus = super::ide_git_ui::Foc::NewName;
                self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
                return;
            }
            let parent = self.default_parent(cx);
            if let Err(e) = self.create_workspace(&name, &parent, false, cx) {
                d.error = Some(e);
                self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
                return;
            }
        }
        let Some(i) = self.active else { return };
        if let Err(e) = self.start_clone(i, &specs, cx) {
            d.error = Some(e);
            d.target_new = false;
            self.modal = Some(Modal::Git(Box::new(GitModal::Clone(d))));
        }
    }

    /// Starts the clones of a request in workspace `i`. Returns the problems with the list, or says why nothing could start.
    pub fn start_clone(&mut self, i: usize, specs: &[gitjobs::CloneSpec], cx: &AppCx) -> Result<(), String> {
        let id = self.ws_id(i);
        if self.git.batches.get(&id).is_some_and(Batch::pending) {
            return Err("Repositories are still being cloned, fetched or pulled in this workspace; wait for them to finish or stop them from the installer indicator.".into());
        }
        let ctx = self.git_ctx(i, cx)?;
        match Batch::plan_clone(ctx, specs) {
            Err(errs) => Err(errs.iter().take(4).map(|(n, e)| if *n == 0 { e.clone() } else { format!("line {n}: {e}") }).collect::<Vec<_>>().join("   ")),
            Ok(b) => {
                cx.audit.record(&cx.session.user, "git.clone.start", &format!("{} repositories into {}", b.items.len(), self.ws[i].meta.name));
                self.git.batches.insert(id, b);
                self.git.note.clear();
                self.open_repos_panel(cx);
                Ok(())
            }
        }
    }

    fn start_update(&mut self, op: Op, repos: Vec<RepoEntry>, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let id = self.ws_id(i);
        if self.git.batches.get(&id).is_some_and(Batch::pending) {
            self.git.note = "Something is still running here; wait for it to finish.".into();
            return;
        }
        if repos.is_empty() {
            self.git.note = "There is nothing to do: no repository in this workspace.".into();
            return;
        }
        match self.git_ctx(i, cx).and_then(|c| Batch::plan_update(c, &repos, op).map_err(|e| e.into_iter().map(|x| x.1).collect::<Vec<_>>().join("; "))) {
            Ok(b) => {
                cx.audit.record(&cx.session.user, &format!("git.{}.start", op.verb()), &format!("{} repositories in {}", b.items.len(), self.ws[i].meta.name));
                self.git.batches.insert(id, b);
                self.git.note.clear();
            }
            Err(e) => self.git.note = e,
        }
    }

    fn repos_act(&mut self, act: ReposAct, map: &[(Option<usize>, Option<usize>)], cx: &AppCx) {
        let Some(i) = self.active else { return };
        let id = self.ws_id(i);
        match act {
            ReposAct::None => {}
            ReposAct::Close => self.modal = None,
            ReposAct::Connect => self.open_connect_flow(None, cx),
            ReposAct::Push => self.open_push_review(cx),
            ReposAct::LocalGitea => self.open_local_gitea(cx),
            ReposAct::Disconnect => self.disconnect_card(),
            ReposAct::CloneMore => self.open_clone_dialog(false, cx),
            ReposAct::Refresh => self.git_mark_stale(i),
            ReposAct::Keys => {
                let ids: Vec<String> = self.git_keys(cx).keys().into_iter().map(|k| k.id).collect();
                if ids.is_empty() {
                    self.git.note = "There is no key yet: make or import one in Settings → Git and SSH keys.".into();
                } else {
                    let mut opts: Vec<&str> = vec!["(my default for this host)"];
                    opts.extend(ids.iter().map(String::as_str));
                    let f = Form::new("Which key does this workspace use for a host?").text("Host", "", "github.com, git.example.org").choice("Key", &opts, 0);
                    self.modal = Some(Modal::Form(FormKind::Git(GitForm::WsKey), f));
                }
            }
            ReposAct::FetchAll => self.start_update(Op::Fetch, self.with_root(i, self.ws[i].cfg.repos.clone()), cx),
            ReposAct::PullAll => {
                let repos = self.with_root(i, self.ws[i].cfg.repos.clone());
                let dirty: Vec<String> = self.git.status.get(&id).map(|s| s.rows.iter().filter(|(_, r)| r.as_ref().is_ok_and(RepoStatus::dirty)).map(|(p, _)| p.clone()).collect()).unwrap_or_default();
                if dirty.is_empty() {
                    self.start_update(Op::Pull, repos, cx);
                } else {
                    let clean: Vec<RepoEntry> = repos.iter().filter(|r| !dirty.contains(&r.path)).cloned().collect();
                    let mut card = Card::new("Pull over local changes?", AMBER).line("These repositories have changes that are not committed:", Tone::Normal);
                    for d in &dirty {
                        card = card.line(&format!("  {d}/"), Tone::Warn);
                    }
                    card = card
                        .line("A pull here only moves forward (fast-forward only); it never merges and never rebases. Git stops by itself when a changed file would be overwritten, and tells you which.", Tone::Dim)
                        .action(Action::new("Pull the clean ones only  c", Key::Char('c'), OKC))
                        .action(Action::new("Pull all, fast-forward only  a", Key::Char('a'), AMBER))
                        .action(Action::new("Cancel  Esc", Key::Esc, MUTE));
                    self.modal = Some(Modal::Card(Box::new(CardModal { card, action: CardAction::Git(GitCard::PullDirty(clean, repos)) })));
                }
            }
            ReposAct::Retry(k) => {
                if let Some(item) = map.get(k).and_then(|m| m.1) {
                    if let Some(b) = self.git.batches.get_mut(&id) {
                        if !b.retry(item) {
                            self.git.note = "That one cannot be retried (it did not fail, or you did not trust its host).".into();
                        }
                    }
                }
            }
            ReposAct::Commands(k) => {
                if let Some(r) = map.get(k).and_then(|m| m.0).and_then(|ri| self.ws[i].cfg.repos.get(ri).cloned()) {
                    let f = Form::new(&format!("Commands for {}/", r.path)).text("Test command", &r.test, "run inside this repository's folder, e.g. cargo test").text("Run command", &r.run, "run inside this repository's folder, e.g. npm start");
                    self.modal = Some(Modal::Form(FormKind::Git(GitForm::Commands(r.path)), f));
                } else {
                    self.git.note = "Commands can be set once the repository is cloned.".into();
                }
            }
            ReposAct::Remove(k) => {
                if let Some(r) = map.get(k).and_then(|m| m.0).and_then(|ri| self.ws[i].cfg.repos.get(ri).cloned()) {
                    let dir = self.ws[i].root().join(&r.path);
                    let risks = match gitjobs::Tools::detect(&cx.services.clis.search_path()) {
                        Ok(t) => gitjobs::removal_risks(&t.git, &dir),
                        Err(_) => "git is not available to check what would be lost.".to_string(),
                    };
                    let card = Card::new(&format!("Remove {}/ ?", r.path), BADC)
                        .line(&format!("The folder {}/ and everything in it is deleted from the disk. This cannot be undone.", r.path), Tone::Normal)
                        .line(&risks, Tone::Warn)
                        .line("The repository stays on its server; only this copy goes.", Tone::Dim)
                        .action(Action::new("Continue…  c", Key::Char('c'), BADC))
                        .action(Action::new("Cancel  Esc", Key::Esc, MUTE))
                        .danger();
                    self.modal = Some(Modal::Card(Box::new(CardModal { card, action: CardAction::Git(GitCard::RemoveRisk(r.path)) })));
                } else {
                    self.git.note = "Only a cloned repository can be removed here.".into();
                }
            }
        }
    }

    pub fn git_card_action(&mut self, c: GitCard, key: Key, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let id = self.ws_id(i);
        match (c, key) {
            (GitCard::Host(h, p), Key::Char('y')) => {
                if let Some(b) = self.git.batches.get_mut(&id) {
                    match b.trust_host(&h, p) {
                        Ok(()) => self.notice = Some((format!("Trusted {h}. Its key is recorded in your known hosts; a different key later is refused."), true)),
                        Err(e) => self.notice = Some((e, false)),
                    }
                }
                self.git.card_open = false;
            }
            (GitCard::Host(h, p), _) => {
                if let Some(b) = self.git.batches.get_mut(&id) {
                    b.refuse_host(&h, p);
                }
                self.git.card_open = false;
            }
            (GitCard::PullDirty(clean, _), Key::Char('c')) => self.start_update(Op::Pull, clean, cx),
            (GitCard::PullDirty(_, all), Key::Char('a')) => self.start_update(Op::Pull, all, cx),
            (GitCard::PullDirty(..), _) => {}
            (GitCard::RemoveRisk(folder), Key::Char('c')) => {
                let f = Form::new(&format!("Remove {folder}/")).text(&format!("Type the folder name ({folder}) to delete it"), "", &folder);
                self.modal = Some(Modal::Form(FormKind::Git(GitForm::RemoveType(folder)), f));
            }
            (GitCard::RemoveRisk(_), _) => {}
            (GitCard::Disconnect, Key::Char('y')) => {
                let root = self.ws[i].root();
                let origin = self.git_head().and_then(|h| h.origin);
                match gitconn::remove_origin(&*self.git_runner(), &root) {
                    Ok(()) => {
                        let what = origin.and_then(|o| iyakki_agent::gitsite::parse(&o, Default::default()).ok()).map(|u| format!("{} {}", u.host, u.short())).unwrap_or_default();
                        cx.audit.record(&cx.session.user, "git.disconnect", &what);
                        self.git.note = "Disconnected: origin was removed. The connection stays in Settings, Git.".into();
                        self.git_mark_head_stale(i);
                    }
                    Err(e) => self.git.note = e,
                }
                self.show_repos(cx);
            }
            (GitCard::Disconnect, _) => self.show_repos(cx),
        }
        self.redraw = true;
    }

    pub fn git_submit_form(&mut self, g: GitForm, f: Form, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let fail = |app: &mut IdeApp, g: GitForm, mut f: Form, e: String| {
            f.error = Some(e);
            app.modal = Some(Modal::Form(FormKind::Git(g), f));
        };
        match g.clone() {
            GitForm::Commands(folder) => {
                let root = self.ws[i].root();
                match ProjectConfig::load(&root) {
                    Ok(mut c) => {
                        if let Some(r) = c.repos.iter_mut().find(|r| r.path == folder) {
                            r.test = f.value(0).trim().to_string();
                            r.run = f.value(1).trim().to_string();
                        }
                        match c.save(&root) {
                            Ok(()) => {
                                self.reload_repos(i);
                                self.git.note = format!("Saved the commands of {folder}/.");
                                self.open_repos_panel(cx);
                            }
                            Err(e) => fail(self, g, f, e),
                        }
                    }
                    Err(e) => fail(self, g, f, e),
                }
            }
            GitForm::RemoveType(folder) => match gitjobs::remove_repo(&self.ws[i].root(), &folder, &f.value(0)) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "git.remove", &format!("{folder} from {}", self.ws[i].meta.name));
                    self.reload_repos(i);
                    self.ws[i].tree.refresh();
                    self.git.note = format!("Removed {folder}/.");
                    self.open_repos_panel(cx);
                }
                Err(e) => fail(self, g, f, e),
            },
            GitForm::WsKey => {
                let host = f.value(0).trim().to_ascii_lowercase();
                let ids: Vec<String> = self.git_keys(cx).keys().into_iter().map(|k| k.id).collect();
                let idx = f.choice_idx(1);
                if !iyakki_agent::gitkeys::valid_host_key(&host) {
                    return fail(self, g, f, format!("`{host}` is not a host name (for example github.com)."));
                }
                let root = self.ws[i].root();
                match ProjectConfig::load(&root) {
                    Ok(mut c) => {
                        if idx == 0 {
                            c.git_keys.remove(&host);
                        } else if let Some(k) = ids.get(idx - 1) {
                            c.git_keys.insert(host.clone(), k.clone());
                        }
                        match c.save(&root) {
                            Ok(()) => {
                                self.ws[i].cfg.git_keys = c.git_keys.clone();
                                cx.audit.record(&cx.session.user, "git.workspace.key", &format!("{host} in {}", self.ws[i].meta.name));
                                self.git.note = if idx == 0 { format!("{host} uses your default key again in this workspace.") } else { format!("{host} uses the key {} in this workspace.", ids[idx - 1]) };
                                self.open_repos_panel(cx);
                            }
                            Err(e) => fail(self, g, f, e),
                        }
                    }
                    Err(e) => fail(self, g, f, e),
                }
            }
            GitForm::Passphrase(id) => {
                let remember = f.choice_idx(1) == 1;
                match self.git_keys(cx).unlock_key(&id, &f.value(0), remember) {
                    Ok(()) => self.notice = Some((format!("The key {id} is unlocked for this session{}.", if remember { " and its passphrase is kept in your vault" } else { "" }), true)),
                    Err(e) => fail(self, g, f, e),
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ the heartbeat

    pub fn git_tick(&mut self, cx: &AppCx) {
        if self.ticks % 6 != 0 {
            return;
        }
        if let Some(Modal::Git(g)) = &mut self.modal {
            let moved = match &mut **g {
                GitModal::Connect(f) => f.poll(),
                GitModal::Wizard(w) => {
                    let was = w.busy();
                    w.poll();
                    was != w.busy()
                }
                _ => false,
            };
            if moved {
                self.redraw = true;
            }
        }
        let ids: Vec<(usize, String)> = self.ws.iter().enumerate().map(|(i, w)| (i, w.meta.id.clone())).collect();
        for (i, id) in ids {
            // batches
            let mut changed = false;
            let mut prompt = None;
            if let Some(b) = self.git.batches.get_mut(&id) {
                let was = (b.done(), b.failed(), b.running_count());
                b.tick(&cx.services.jobs);
                if was != (b.done(), b.failed(), b.running_count()) {
                    changed = true;
                }
                if b.changed {
                    b.changed = false;
                    changed = true;
                }
                if self.active == Some(i) && b.phase == Phase::Confirm {
                    prompt = b.prompts.first().cloned();
                }
            }
            if changed {
                self.reload_repos(i);
                self.ws[i].tree.refresh();
                self.git_mark_stale(i);
                self.git_mark_head_stale(i);
                self.redraw = true;
            }
            if let Some(p) = prompt {
                let free = self.modal.is_none() || matches!(&self.modal, Some(Modal::Git(g)) if matches!(**g, GitModal::Repos(_)));
                if free && !self.git.card_open {
                    self.git.card_open = true;
                    let mut card = Card::new(&format!("A new host: {}", p.host), AMBER).line(&format!("You have not connected to {}{} before. It presents:", p.host, if p.port == 22 { String::new() } else { format!(":{}", p.port) }), Tone::Normal);
                    for k in &p.keys {
                        card = card.line(&format!("  {}  {}", k.kind, k.fingerprint()), Tone::Bold);
                    }
                    card = card
                        .line("Compare this fingerprint with the one the site publishes (GitHub, GitLab and others list theirs in their documentation) or your server's owner gave you. Trust it only if it is the same: nothing is sent to the host before that.", Tone::Dim)
                        .action(Action::new("Trust this host  y", Key::Char('y'), OKC))
                        .action(Action::new("Do not trust  n", Key::Char('n'), BADC));
                    self.modal = Some(Modal::Card(Box::new(CardModal { card, action: CardAction::Git(GitCard::Host(p.host.clone(), p.port)) })));
                    self.redraw = true;
                }
            }
            self.git_poll_status(i, cx);
            self.git_poll_head(i, cx);
        }
    }

    fn git_poll_status(&mut self, i: usize, cx: &AppCx) {
        let id = self.ws_id(i);
        let repos: Vec<RepoEntry> = self.ws[i].cfg.repos.clone();
        let board = self.git.status.entry(id).or_default();
        // collect a finished read
        if let Some(rx) = &board.rx {
            if let Ok(rows) = rx.try_recv() {
                board.rows = rows;
                board.at = Some(Instant::now());
                board.rx = None;
                let badges: HashMap<String, String> = board.rows.iter().filter_map(|(p, r)| r.as_ref().ok().map(|s| (p.clone(), s.summary()))).collect();
                self.ws[i].tree.badges = badges;
                self.redraw = true;
            }
            return;
        }
        if repos.is_empty() || self.active != Some(i) {
            return;
        }
        let due = board.stale || board.at.is_none_or(|t| t.elapsed() > STATUS_EVERY);
        if !due {
            return;
        }
        let Ok(tools) = gitjobs::Tools::detect(&cx.services.clis.search_path()) else { return };
        board.stale = false;
        let root = self.ws[i].root();
        let (tx, rx) = channel();
        board.rx = Some(rx);
        std::thread::spawn(move || {
            let rows: StatusRows = repos.iter().map(|r| (r.path.clone(), gitjobs::status(&tools.git, &root.join(&r.path)))).collect();
            let _ = tx.send(rows);
        });
    }

    /// The repository (folder) a file of the workspace belongs to, and its commands.
    pub fn repo_of(&self, rel: &str) -> Option<RepoEntry> {
        let w = self.active_ws()?;
        w.cfg.repos.iter().filter(|r| rel == r.path || rel.starts_with(&format!("{}/", r.path))).max_by_key(|r| r.path.len()).cloned()
    }
}

impl IdeApp {
    // ------------------------------------------------------------------------------------------------ Connect Git / Gitea

    /// Runs git for the project's own repository (`origin`, the first commit): the system's, or a fake in tests.
    pub fn git_runner(&self) -> Arc<dyn Runner> {
        self.git.runner.clone().unwrap_or_else(|| Arc::new(iyakki_agent::wsexec::System))
    }

    fn wiz_env(&self, cx: &AppCx) -> WizEnv {
        WizEnv::real(cx, self.git_keys(cx), self.git.http.clone())
    }

    fn platform_ctx(&self, cx: &AppCx) -> Arc<Platform> {
        self.git.platform.clone().unwrap_or_else(|| {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            Arc::new(Platform::real(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d))))
        })
    }

    /// "Connect Git / Gitea…" (top bar ⋯ menu, the Repositories panel, the chip of the top bar): choose a connection, then create a
    /// repository or connect to an existing one. `pick` names a connection to start on (just made in the wizard).
    pub fn open_connect_flow(&mut self, pick: Option<&str>, cx: &AppCx) {
        let Some(i) = self.active else {
            self.notice = Some(("Open a project first.".into(), false));
            return;
        };
        if self.git.runner.is_none() {
            if let GitCheck::Missing { message } = self.git_check(cx) {
                self.notice = Some((message, false));
                return;
            }
        }
        let w = &self.ws[i];
        let e = FlowEnv { wiz: self.wiz_env(cx), run: self.git_runner(), platform: Some(self.platform_ctx(cx)) };
        let f = ConnectFlow::new(e, &w.meta.name, std::fs::canonicalize(w.root()).unwrap_or_else(|_| w.root()), pick);
        self.git_mark_head_stale(i);
        self.modal = Some(Modal::Git(Box::new(GitModal::Connect(Box::new(f)))));
        self.redraw = true;
    }

    /// "Put this project under version control on Local Gitea": offered while the project has no remote. With the Quick install item
    /// Gitea installed, the connection flow starts on the connection "Local Gitea" at the review of the new repository (nothing is
    /// made or pushed before the person confirms each review); without it, a plain message names Store → Setup.
    pub fn open_local_gitea(&mut self, cx: &AppCx) {
        use iyakki_agent::quickinstall::gitea::{offer, Offer};
        let has_remote = self.git_head().is_some_and(|h| h.origin.is_some());
        let conns = self.git_keys(cx).conns().list();
        match offer(has_remote, &conns) {
            Offer::None => self.git.note = "This project already has a remote; use Change (o) to connect it somewhere else.".into(),
            Offer::Install(m) => {
                self.git.note = m.clone();
                self.notice = Some((m, false));
            }
            Offer::Ready(id) => {
                self.open_connect_flow(Some(&id), cx);
                if let Some(Modal::Git(g)) = &mut self.modal {
                    if let GitModal::Connect(f) = &mut **g {
                        f.start_create();
                    }
                }
            }
        }
        self.redraw = true;
    }

    /// The same flow, straight to the push review (the panel's Push button).
    pub fn open_push_review(&mut self, cx: &AppCx) {
        self.open_connect_flow(None, cx);
        if let Some(Modal::Git(g)) = &mut self.modal {
            if let GitModal::Connect(f) = &mut **g {
                f.start_at_push();
            }
        }
    }

    fn open_wizard(&mut self, cx: &AppCx) {
        self.modal = Some(Modal::Git(Box::new(GitModal::Wizard(Box::new(GitWizard::new(self.wiz_env(cx)))))));
    }

    fn connect_flow_event(&mut self, mut f: Box<ConnectFlow>, ev: &Event, cx: &AppCx) {
        match f.event(ev) {
            FlowOut::None => self.modal = Some(Modal::Git(Box::new(GitModal::Connect(f)))),
            FlowOut::Changed => {
                if let Some(i) = self.active {
                    self.git_mark_head_stale(i);
                }
                self.modal = Some(Modal::Git(Box::new(GitModal::Connect(f))));
            }
            FlowOut::Close => {
                if let Some(i) = self.active {
                    self.git_mark_head_stale(i);
                }
            }
            FlowOut::AddConnection => self.open_wizard(cx),
            FlowOut::OpenPlatform => {
                if let Some(i) = self.active {
                    self.open_platform(i, false, cx);
                }
            }
            FlowOut::Push { remote, branch, host, repo } => self.start_push(&remote, &branch, &host, &repo, cx),
        }
        self.redraw = true;
    }

    fn wizard_event(&mut self, mut w: Box<GitWizard>, ev: &Event, cx: &AppCx) {
        match w.event(ev) {
            WizOut::None => self.modal = Some(Modal::Git(Box::new(GitModal::Wizard(w)))),
            WizOut::Cancel => self.open_connect_flow(None, cx),
            WizOut::Done(c) => self.open_connect_flow(Some(&c.id), cx),
            WizOut::OpenPlatform => {
                if let Some(i) = self.active {
                    self.open_platform(i, false, cx);
                }
            }
            WizOut::QuickInstallGitea => {
                self.git.note = match super::git_wizard::start_quick_install_gitea(cx) {
                    Ok(m) | Err(m) => m,
                };
                self.open_connect_flow(None, cx);
            }
        }
        self.redraw = true;
    }

    /// The push was reviewed and confirmed: a normal git job (key service, host-key card, token through askpass), audited by host and
    /// repository name.
    fn start_push(&mut self, remote: &str, branch: &str, host: &str, repo: &str, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let id = self.ws_id(i);
        if self.git.batches.get(&id).is_some_and(Batch::pending) {
            self.git.note = "Something is still running here; wait for it to finish, then push.".into();
            self.show_repos(cx);
            return;
        }
        match self.git_ctx(i, cx).and_then(|c| Batch::plan_push(c, remote, ".", branch).map_err(|e| e.into_iter().map(|x| x.1).collect::<Vec<_>>().join("; "))) {
            Ok(b) => {
                cx.audit.record(&cx.session.user, "git.push", &format!("{host} {repo}"));
                self.git.batches.insert(id, b);
                self.git.note.clear();
            }
            Err(e) => self.git.note = e,
        }
        self.git_mark_head_stale(i);
        self.show_repos(cx);
    }

    fn git_mark_head_stale(&mut self, i: usize) {
        let id = self.ws_id(i);
        self.git.heads.entry(id).or_default().stale = true;
    }

    /// The project's own git setup as the panel and the top bar show it (read in the background; `None` before the first read).
    pub fn git_head(&self) -> Option<WsHead> {
        let i = self.active?;
        self.git.heads.get(&self.ws.get(i)?.meta.id)?.head.clone()
    }

    fn git_poll_head(&mut self, i: usize, cx: &AppCx) {
        let id = self.ws_id(i);
        let root = self.ws[i].root();
        let run = self.git_runner();
        let keys = self.git_keys(cx);
        let active = self.active == Some(i);
        let board = self.git.heads.entry(id).or_default();
        if let Some(rx) = &board.rx {
            if let Ok(st) = rx.try_recv() {
                let origin = st.origin.as_ref().map(|o| iyakki_agent::gitsite::redact_url(o));
                let (host, c) = match st.origin.as_ref().and_then(|o| iyakki_agent::gitsite::parse(o, Default::default()).ok()) {
                    Some(u) => (u.host.clone(), keys.conns().for_host(&u.site())),
                    None => (String::new(), None),
                };
                let head = WsHead { is_repo: st.is_repo, branch: st.branch.clone(), origin, commits: st.commits, changed: st.changed, conn: c.as_ref().map(|c| c.name.clone()), host, kind: c.as_ref().map(|c| c.kind.label().to_string()).unwrap_or_default(), place: c.as_ref().map(|c| c.placement().label().to_string()).unwrap_or_default() };
                board.head = Some(head);
                board.state = Some(st);
                board.at = Some(Instant::now());
                board.rx = None;
                self.redraw = true;
            }
            return;
        }
        if !active {
            return;
        }
        if !(board.stale || board.at.is_none_or(|t| t.elapsed() > STATUS_EVERY)) {
            return;
        }
        board.stale = false;
        let (tx, rx) = channel();
        board.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(gitconn::ws_git(&*run, &root));
        });
    }

    /// The root of the project as a repository entry for Fetch and Pull, when it has an `origin`.
    fn with_root(&self, i: usize, mut repos: Vec<RepoEntry>) -> Vec<RepoEntry> {
        let origin = self.git.heads.get(&self.ws_id(i)).and_then(|b| b.state.as_ref()).and_then(|s| s.origin.clone());
        if let Some(url) = origin {
            if !repos.iter().any(|r| r.path == ".") {
                repos.insert(0, RepoEntry { url, path: ".".into(), ..Default::default() });
            }
        }
        repos
    }

    fn disconnect_card(&mut self) {
        let Some(h) = self.git_head() else { return };
        let Some(o) = h.origin else {
            self.git.note = "This project is not connected to Git.".into();
            return;
        };
        let card = Card::new("Disconnect from Git?", AMBER)
            .line(&format!("This removes origin ({o}) from the project's git settings."), Tone::Normal)
            .line("Nothing on the server and none of your files change, and the commits stay. You can connect again any time. The connection and its token stay in Settings, Git.", Tone::Dim)
            .action(Action::new("Disconnect  y", Key::Char('y'), BADC))
            .action(Action::new("Keep it  Esc", Key::Esc, MUTE))
            .danger();
        self.modal = Some(Modal::Card(Box::new(CardModal { card, action: CardAction::Git(GitCard::Disconnect) })));
    }
}

fn work_text(it: &gitjobs::Item) -> (String, bool, String) {
    let verb = match it.op {
        Op::Clone => ("cloning", "cloned"),
        Op::Fetch => ("fetching", "fetched"),
        Op::Pull => ("pulling", "pulled"),
        Op::Push => ("pushing", "pushed"),
    };
    match &it.state {
        ItemState::Queued => ("queued".into(), false, String::new()),
        ItemState::Running => (match it.progress {
            Some(p) => format!("{} {p:.0}%", verb.0),
            None => verb.0.to_string(),
        }, false, it.last.clone()),
        ItemState::Done => (verb.1.to_string(), false, it.last.clone()),
        ItemState::Failed(m) => ("failed".into(), true, m.chars().take(200).collect()),
    }
}

fn gitsite_short(url: &str) -> String {
    iyakki_agent::gitsite::parse(url, iyakki_agent::gitsite::ParseOptions { allow_local: true }).map_or_else(|_| iyakki_agent::gitsite::redact_url(url), |u| u.short())
}

#[cfg(test)]
#[path = "ide_git_tests.rs"]
mod tests;
