//! AI IDE: a futuristic multi-pane workbench for working with the assistant on products. See `docs/AI-IDE.md`.
//!
//! * A **workspace** is one product: a folder, its own editor tabs, file tree, terminal, test command, Kubernetes view,
//!   feature board and chat history. Several are open at once and never mix (`iyakki_agent::workspace`).
//! * The editor is the ordinary EDIT app, the chat is the ordinary CHAT app limited to the workspace folder
//!   (`ProjectScope`), the board is `board.rs`, the cluster view `ide_k8s.rs`.
//! * This file holds the state, the event routing and the actions; the drawing is in `ide_ui.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_agent::devcluster::{self, ClusterProfile, Env as ClusterEnv, Slot};
use iyakki_agent::k8s::{Kube, Target};
use iyakki_agent::k8stools::K8sTools;
use iyakki_agent::wsexec::Runner as ExecRunner;
use iyakki_agent::workspace::{self, LockInfo, LockState, ProjectConfig, Registry, Workspace};
use serde::{Deserialize, Serialize};

use super::board::{BoardAct, BoardView};
use super::chat::{ChatApp, ProjectScope};
use super::edit::EditApp;
use super::ide_bottom::{Problems, SearchPanel, Tab as BTab, Term, TermSaved};
use super::ide_k8s::K8sView;
use super::ide_notes::{NotesAct, NotesView};
use super::ide_layout::{self as lay, Div, Pane, Prefs};
use super::ide_tasks::{TasksAct, TasksView};
use super::ide_tree::{Tree, TreeAct};
use super::widgets::{Form, FormEvent};
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx, Icon};
use crate::ui::{Event, Key, Nav, R};

pub const ACCENT: [u8; 4] = rgb(100, 230, 255);
/// A child app's click ids are moved up by `pane code * STRIDE` so the editor's and the chat's never collide.
pub const STRIDE: usize = 1_000_000;
pub const T_RAIL: usize = 9_000_000;
pub const T_RAIL_ADD: usize = 9_001_000;
pub const T_TOP: usize = 9_002_000;
pub const T_MAIN: usize = 9_003_000;
pub const T_BOTTOM: usize = 9_004_000;
pub const T_PANE: usize = 9_005_000;
pub const A_DIV: usize = 9_006_000;
pub const ROW_SWITCH: usize = 500_000;
pub const ROW_QUICK: usize = 510_000;
/// `Event::Sub` code of the IDE's own buttons.
pub const SUB: u8 = 33;

static KUBE_OVERRIDE: Mutex<Option<Arc<dyn Kube>>> = Mutex::new(None);

/// Use another cluster client (tests and screenshots give a fake one); `None` goes back to `kubectl`.
pub fn use_kube(k: Option<Arc<dyn Kube>>) {
    *KUBE_OVERRIDE.lock().unwrap() = k;
}

fn kube() -> Arc<dyn Kube> {
    if let Some(k) = KUBE_OVERRIDE.lock().unwrap().clone() {
        return k;
    }
    let run: Arc<dyn ExecRunner> = CLUSTER_OVERRIDE.lock().unwrap().as_ref().map_or_else(|| Arc::new(iyakki_agent::wsexec::System) as Arc<dyn ExecRunner>, |w| w.0.clone());
    Arc::new(devcluster::RunnerKube { run })
}

type ClusterWorld = (Arc<dyn ExecRunner>, ClusterEnv);
static CLUSTER_OVERRIDE: Mutex<Option<ClusterWorld>> = Mutex::new(None);

/// Tests and screenshots give the IDE a fake executor and fake kubeconfig locations; `None` goes back to the real machine
/// (and, in unit tests, to "look nowhere").
pub fn use_cluster(w: Option<ClusterWorld>) {
    *CLUSTER_OVERRIDE.lock().unwrap() = w;
}

/// Where the IDE looks for a cluster: the override, the real machine, or (unit tests without an override) nowhere, so a
/// test run never talks to a developer's own cluster.
fn cluster_world() -> Option<ClusterWorld> {
    if let Some(w) = CLUSTER_OVERRIDE.lock().unwrap().clone() {
        return Some(w);
    }
    if cfg!(test) {
        return None;
    }
    Some((Arc::new(iyakki_agent::wsexec::System), ClusterEnv::real()))
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MainTab {
    Editor,
    Board,
    /// Parallel tasks: isolated copies of the workspace worked on at the same time, reviewed and merged here (`ide_tasks`).
    Tasks,
    /// The workspace's shared project context: what the team and every agent must remember (`ide_notes`).
    Notes,
    /// A web page inside the IDE (the app just deployed, or whatever the assistant is testing): see `ide_browser`.
    Browser,
    K8s,
}

/// Who has the keyboard. Exactly one owner at a time; a blinking caret is shown only for an owner that is a text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The file tree (single-letter keys: n new, r rename, d delete, / find, h hidden).
    Tree,
    /// The code editor: a text input.
    Editor,
    /// The feature board's lanes and table (single-letter keys: a add, e comment, b blocker, x delete...).
    Board,
    /// The board's filter box: a text input.
    BoardFilter,
    /// The Tasks tab (single-letter keys: n new, s stop, p pause, r resume, m merge, d discard, f follow-up, y/x allow or deny); no text field.
    Tasks,
    /// The Notes tab (single-letter keys: n new, e edit, p pin, a archive, x delete, t trust, r reviewed, / search).
    Notes,
    /// A note being written or searched for: the title, text, source and search fields are text inputs.
    NotesText,
    /// The cluster view's table (single-letter keys: r refresh, l logs, n namespace, c cluster, d deploy, g registry, R restart).
    K8s,
    /// A dialog of the cluster view (chooser or deploy steps: y/a approve).
    K8sDialog,
    /// The Browser tab's address field: a text input.
    BrowserAddr,
    /// The page in the Browser tab: letters, clicks and the wheel go to the page (Esc hands the keyboard back).
    BrowserPage,
    /// A form of the board or of the IDE: text inputs.
    Dialog,
    /// The assistant's composer: a text input (answer keys 1-9, y, n act only while it is empty).
    Composer,
    /// The terminal's command line: a text input.
    Terminal,
    /// The bottom panel's cluster status strip (Enter opens the K8s view).
    K8sStrip,
    /// The list of problems (n / p step, T run tests).
    Problems,
    /// The project search box: a text input.
    Search,
}

impl Owner {
    /// A text input: every printable key is typed into it, and its caret blinks.
    pub fn is_text(self) -> bool {
        matches!(self, Owner::Editor | Owner::BoardFilter | Owner::Dialog | Owner::Composer | Owner::Terminal | Owner::Search | Owner::BrowserAddr | Owner::NotesText)
    }
}

impl Ws {
    /// Move the keyboard to `p`. The board's filter box gives up its caret when the keyboard leaves the board.
    pub fn set_focus(&mut self, p: Pane) {
        if p != Pane::Main {
            self.board.blur();
            self.notes.blur();
        }
        self.focus = p;
    }

    /// The one owner of the keyboard in this workspace.
    pub fn owner(&self) -> Owner {
        match self.focus {
            Pane::Tree => Owner::Tree,
            Pane::Chat => Owner::Composer,
            Pane::Main => match self.main_tab {
                MainTab::Editor => Owner::Editor,
                MainTab::Board if self.board.has_modal() => Owner::Dialog,
                MainTab::Board if self.board.wants_text() => Owner::BoardFilter,
                MainTab::Board => Owner::Board,
                MainTab::Tasks => Owner::Tasks,
                MainTab::Notes if self.notes.wants_text() => Owner::NotesText,
                MainTab::Notes => Owner::Notes,
                MainTab::Browser if self.browser.as_ref().is_some_and(|b| b.address_focused()) => Owner::BrowserAddr,
                MainTab::Browser => Owner::BrowserPage,
                MainTab::K8s if self.k8s.as_ref().is_some_and(|k| k.has_modal()) => Owner::K8sDialog,
                MainTab::K8s => Owner::K8s,
            },
            Pane::Bottom => match self.bottom_tab {
                BTab::Terminal => Owner::Terminal,
                BTab::K8s => Owner::K8sStrip,
                BTab::Problems => Owner::Problems,
                BTab::Search => Owner::Search,
            },
        }
    }
}

impl MainTab {
    pub const ALL: [MainTab; 6] = [MainTab::Editor, MainTab::Board, MainTab::Tasks, MainTab::Notes, MainTab::Browser, MainTab::K8s];
    pub fn label(self) -> &'static str {
        match self {
            MainTab::Editor => "Editor",
            MainTab::Board => "Board",
            MainTab::Tasks => "Tasks",
            MainTab::Notes => "Context",
            MainTab::Browser => "Browser",
            MainTab::K8s => "K8s",
        }
    }
}

/// What a workspace is doing, for the dot in the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Running,
    Waiting,
    /// The assistant finished while the person was elsewhere.
    Done,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WsSaved {
    pub prefs: Prefs,
    pub focus: Option<Pane>,
    pub main_tab: Option<MainTab>,
    pub bottom_tab: Option<BTab>,
    pub files: Vec<String>,
    pub expanded: Vec<String>,
    pub term: TermSaved,
    pub board_mode: u8,
}

/// Everything that belongs to one workspace. Nothing here is shared with another workspace.
pub struct Ws {
    pub meta: Workspace,
    pub cfg: ProjectConfig,
    pub tree: Tree,
    pub editor: EditApp,
    pub chat: ChatApp,
    pub term: Term,
    pub problems: Problems,
    /// The coverage run in the workspace folder, and the plan whose report is read when it ends.
    pub cov: super::ide_bottom::Runner,
    pub cov_plan: Option<iyakki_agent::coverage::Plan>,
    pub search: SearchPanel,
    pub k8s: Option<K8sView>,
    /// The Browser tab's page, made when the tab is first shown (one per workspace: there is never a second browser surface).
    pub browser: Option<super::ide_browser::IdeBrowser>,
    /// Addresses offered as buttons in the Browser tab: found by the dev-server scan, the cluster's Services and the last page.
    pub dev_ports: super::ide_browser::DevScan,
    pub board: BoardView,
    /// The Tasks tab: this workspace's parallel tasks (the scheduler lives in `iyakki_agent::tasks`).
    pub tasks: TasksView,
    /// The Notes tab: the workspace's project notes (`.iyakki/notes.json`).
    pub notes: NotesView,
    /// The project context changes made by what the person said in the chat: the entry touched last ("pin that"), how to take the last change
    /// back, and an ambiguous request waiting for a numbered answer.
    pub ctx_last: Option<String>,
    pub ctx_undo: Option<iyakki_agent::contextintent::Undo>,
    pub ctx_pending: Option<(iyakki_agent::contextintent::Detected, Vec<String>)>,
    pub prefs: Prefs,
    pub focus: Pane,
    pub main_tab: MainTab,
    pub bottom_tab: BTab,
    pub compact_pane: Pane,
    /// Held by another session: this one is read-only.
    pub ro: Option<LockInfo>,
    pub ns_warning: Option<String>,
    pub status: Option<(String, bool)>,
    pub unseen_done: bool,
    pub was_running: bool,
    pub features_mtime: Option<std::time::SystemTime>,
    pub board_full: bool,
    /// The cluster this workspace develops against, found in the background (`devcluster::resolve`).
    pub cluster: Slot,
    pub applied_gen: u64,
    /// The cluster chooser was opened once for an ambiguous default (it is not forced again this session).
    pub chooser_shown: bool,
}

impl Ws {
    pub fn activity(&self) -> Activity {
        if self.chat.pending_approval().is_some() {
            Activity::Waiting
        } else if self.chat.is_running() {
            Activity::Running
        } else if self.chat.error_text().is_some() {
            Activity::Error
        } else if self.unseen_done {
            Activity::Done
        } else {
            Activity::Idle
        }
    }

    pub fn root(&self) -> PathBuf {
        self.meta.root_path()
    }

    pub fn features_path(&self) -> PathBuf {
        self.cfg.features_path(&self.root())
    }

    /// Where kubectl runs for this workspace: the resolved cluster profile when there is one, else what project.json says.
    pub fn target(&self) -> Target {
        let mut t = Target { namespace: self.cfg.namespace(&self.meta.name), selector: self.cfg.k8s.selector.clone(), context: self.cfg.k8s.profile.clone(), kubeconfig: self.cfg.k8s.kubeconfig.clone() };
        if let Some(p) = self.chosen_profile() {
            if p.readable {
                t.kubeconfig = p.kubeconfig;
                t.context = p.context;
            }
        }
        t
    }

    /// The profile the resolver picked (reachable or not).
    pub fn chosen_profile(&self) -> Option<ClusterProfile> {
        self.cluster.lock().ok()?.resolution.as_ref()?.chosen().cloned()
    }

    pub fn reachable_profile(&self) -> Option<ClusterProfile> {
        self.cluster.lock().ok()?.resolution.as_ref()?.reachable().cloned()
    }

    pub fn k8s_view(&mut self) -> &mut K8sView {
        let t = self.target();
        let slot = self.cluster.clone();
        self.k8s.get_or_insert_with(|| {
            let mut v = K8sView::new(kube(), t);
            v.slot = Some(slot);
            v
        })
    }

    /// Look for the cluster again on a background thread (`test_all`: test every candidate, for the chooser).
    pub fn start_resolve(&mut self, dir: &Path, user: &str, test_all: bool) {
        let Some((run, env)) = cluster_world() else { return };
        let remembered = iyakki_agent::clusters::remembered_for(dir, user, &self.meta.name);
        let hidden = devcluster::hidden_load(dir, user);
        devcluster::resolve_in_background(&self.cluster, run, env, self.cfg.k8s.clone(), remembered, self.cfg.namespace(&self.meta.name), test_all, hidden);
    }

    /// Why Deploy and Restart are off, if they are.
    pub fn deploy_block(&self, cx: &AppCx) -> Option<String> {
        if self.ro.is_some() {
            return Some("this workspace is open read-only (another session holds it)".into());
        }
        if cx.session.policy.cli != iyakki_sec::policy::CliLevel::Full {
            return Some("your account may not run programs (command-line permission below Full)".into());
        }
        let Some(p) = self.reachable_profile() else { return Some("no cluster is connected (Cluster… chooses one)".into()) };
        if p.looks_like_production() {
            return Some(format!("\"{}\" looks like production, so the assistant and the deploy flow only read from it", p.name));
        }
        if !p.local && !cx.session.policy.allows_host("*") {
            return Some("this cluster is not on this machine and your account may not reach every host".into());
        }
        None
    }

    /// Take the registry choice (`k8s.registry*`) from project.json: the setup flow writes it there, and any later save of the
    /// in-memory settings must not put the old values back.
    pub fn sync_registry(&mut self) {
        if let Ok(c) = ProjectConfig::load(&self.root()) {
            self.cfg.k8s.registry = c.k8s.registry;
            self.cfg.k8s.registry_pull = c.k8s.registry_pull;
            self.cfg.k8s.registry_kind = c.k8s.registry_kind;
        }
    }

    /// The tools of the deploy flow for this workspace's connected cluster.
    pub fn k8s_tools(&self, cx: &AppCx) -> Option<Arc<K8sTools>> {
        if self.deploy_block(cx).is_some() {
            return None;
        }
        let p = self.reachable_profile()?;
        let (run, _) = cluster_world()?;
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let mut t = K8sTools::new(run, p, &self.cfg.namespace(&self.meta.name), &self.meta.name, &self.root(), Arc::new(move |a, d| audit.record(&user, &format!("k8s.{a}"), d)));
        t.cluster_phrase = iyakki_agent::clusters::phrase_for(cx.services.dir(), &cx.session.user, &self.meta.name);
        t.registry_setting = self.cfg.k8s.registry.clone();
        t.registry_pull = self.cfg.k8s.registry_pull.clone();
        t.registry_kind = self.cfg.k8s.registry_kind.clone();
        Some(Arc::new(t))
    }

    pub fn cluster_link(&self) -> Option<super::chat::ClusterLink> {
        let (runner, _) = cluster_world()?;
        Some(super::chat::ClusterLink { slot: self.cluster.clone(), namespace: self.cfg.namespace(&self.meta.name), runner })
    }

    pub fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.status = Some((m.into(), ok));
    }
}

/// The line under the registry form's choice: what the option is good for and whether it needs root.
fn registry_form_hint(f: &mut Form) {
    let c = iyakki_agent::regsetup::CHOICES[f.choice_idx(0).min(2)];
    f.fields[0].hint = format!("{} Root: {}. ← → to change", c.2, c.3);
}

/// The first form's choices: the IYAKKI tile app first (the default), then a service on k3s, then what extends IYAKKI.
pub const NEW_APP_CHOICES: [&str; 6] = ["IYAKKI tile app (a tile in the launcher)", "Web service on k3s", "Skill (text for the assistant)", "MCP server in Python", "MCP server in Node", "WASM tool"];

#[derive(Clone, Debug, PartialEq)]
pub enum FormKind {
    NewWs,
    OpenFolder,
    Clone,
    RenameWs(String),
    Settings,
    NewFile(String),
    NewDir(String),
    RenameFile(String),
    FindFile,
    /// The deploy form of the K8S panel.
    Deploy,
    /// "Set up registry…" of the K8S panel: choose A, B or C.
    Registry,
    /// "Remove all" of the K8S panel: the namespace name must be typed.
    Teardown,
    /// A new parallel task (title, goal, board rows, agent).
    NewTask,
    /// A message for a task that is done or failed: it continues with it.
    TaskFollowup(String),
    /// Guidance for an agent that is working: it pauses after its step and goes on with the message.
    GuideAgent(String),
    /// "New app, MCP server or skill…": kind and name; scaffolds a project and opens the chat with the authoring skill.
    NewApp,
    /// The header chip: what this workspace is for.
    Target,
    /// The Publish dialog: store, version, change log line, signing key.
    Publish,
    /// The publish token, typed once into a masked field and kept in the vault.
    PublishToken,
    /// A form of the git support (commands of a repository, the typed confirmation of a removal, a key's passphrase).
    Git(super::ide_git::GitForm),
}

pub enum Confirm {
    DeleteFile(String),
    RemoveWs(String),
    /// Merge this task (id, what the dry run said).
    MergeTask(String, String),
    DiscardTask(String),
}

pub struct QuickOpen {
    pub query: crate::ui::TextField,
    pub all: Vec<String>,
    pub results: Vec<String>,
    pub sel: usize,
}

pub enum Modal {
    Switcher { sel: usize },
    Form(FormKind, Form),
    Confirm(Confirm),
    Quick(QuickOpen),
    /// The workspace at this index is held by another session.
    Lock(usize),
    Info(String, String),
    /// Choosing the parent folder for a new workspace: the form it came from, the folder shown, and the list.
    Browse(Box<Form>, PathBuf, super::widgets::Picker),
    /// A card with text and buttons (publish review and result, the target plan, checks): see `ide_tile`.
    Card(Box<super::ide_tile::CardModal>),
    /// The Clone dialog and the Repositories panel: see `ide_git`.
    Git(Box<super::ide_git::GitModal>),
}

pub struct IdeApp {
    pub reg: Registry,
    pub ws: Vec<Ws>,
    pub active: Option<usize>,
    pub session: String,
    pub modal: Option<Modal>,
    pub layout: Option<lay::Layout>,
    pub area: R,
    /// The IDE's drawing scale: the window scale times the density's factor.
    pub scale: f32,
    /// The window scale itself, and the scale the embedded chat draws at (its own type is larger than the IDE's).
    pub base_scale: f32,
    pub chat_scale: f32,
    /// How dense the IDE is drawn (kept per person).
    pub density: super::ide_chrome::Density,
    pub mouse: (i32, i32),
    pub drag_div: Option<(Div, f32)>,
    pub redraw: bool,
    pub ticks: u64,
    pub notice: Option<(String, bool)>,
    last_fetches: u64,
    /// Where the registry and the saved layouts live, and whose they are.
    pub(super) dir: PathBuf,
    pub(super) user: String,
    /// The row the context menu was opened on in the tree.
    ctx_row: Option<usize>,
    /// The last context menu was opened over selectable K8S text (flow step or details drawer).
    ctx_k8s: bool,
    /// The last context menu was opened over the Tasks tab's text.
    ctx_tasks: bool,
    /// The top bar's "⋯" menu holds the buttons that did not fit: (label, button number, colour); open or closed.
    pub top_overflow: Vec<(String, usize, [u8; 4])>,
    pub top_overflow_open: bool,
    /// An app the shell should open next (a table file asks for DATA): its id and the item in it.
    pub request: Option<(String, String)>,
    /// A publish in progress (see `ide_tile`).
    pub publish: Option<super::ide_tile::PublishState>,
    /// Where the header drew the target chip, and the "Published: …" line it shows (kept so the disk is not read every frame).
    pub last_target_chip: Option<R>,
    pub last_hint: Option<String>,
    /// Platform services (opened by the K8S panel's Services button): while open it owns every key and click.
    platform: Option<super::platform_services::PlatformPanel>,
    /// Git: the batches of clones, fetches and pulls per workspace, and the repositories' state (see `ide_git`).
    pub git: super::ide_git::GitUi,
}

fn session_id() -> String {
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    format!("ide-{}-{}", std::process::id(), r.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn state_path(dir: &Path, user: &str, ws: &Workspace) -> PathBuf {
    dir.join(format!("ide-state-{}-{}.json", user, ws.id))
}

impl IdeApp {
    pub fn new(cx: &AppCx) -> IdeApp {
        let reg = Registry::load(cx.services.dir(), &cx.session.user);
        super::ide_bottom::set_cli_bin(cx.services.clis.bin_dir());
        iyakki_agent::linuxapps::set_ide_context(cx.services.dir(), &cx.session.user);
        let mut app = IdeApp { reg, ws: vec![], active: None, session: session_id(), modal: None, layout: None, area: R::new(0.0, 0.0, 1280.0, 674.0), scale: 1.0, base_scale: 1.0, chat_scale: 1.0, density: super::ide_chrome::Density::from_index(crate::uisize::load_ide_density(cx.services.dir(), &cx.session.user)), mouse: (-1, -1), drag_div: None, redraw: true, ticks: 0, notice: None, last_fetches: 0, dir: cx.services.dir().to_path_buf(), user: cx.session.user.clone(), ctx_row: None, ctx_k8s: false, ctx_tasks: false, top_overflow: vec![], top_overflow_open: false, request: None, publish: None, last_target_chip: None, last_hint: None, platform: None, git: Default::default() };
        if let Some(id) = app.reg.last.clone() {
            if let Err(e) = app.open_by_id(&id, cx) {
                app.notice = Some((e, false));
            }
        }
        if app.active.is_none() && !app.reg.items.is_empty() {
            app.modal = Some(Modal::Switcher { sel: 0 });
        }
        // git is a separate program: when it is missing (the libre image) the IDE says so once, plainly
        #[cfg(not(test))]
        app.git_announce_missing(cx);
        app
    }

    /// Use another session id (a second IYAKKI session on the same folders, in tests).
    pub fn with_session(mut self, id: &str) -> IdeApp {
        self.session = id.into();
        self
    }

    pub fn active_ws(&self) -> Option<&Ws> {
        self.active.and_then(|i| self.ws.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut Ws> {
        self.active.and_then(|i| self.ws.get_mut(i))
    }

    // ------------------------------------------------------------------------------------------------ workspaces

    fn scope_ok(cx: &AppCx, root: &Path) -> Result<(), String> {
        if cx.session.policy.allows_read(&cx.session.user, &root.to_string_lossy()) {
            Ok(())
        } else {
            let scopes = cx.session.policy.read_scopes(&cx.session.user).into_iter().take(3).collect::<Vec<_>>().join(", ");
            Err(format!("{} is outside the folders your account may read ({scopes}). Choose a folder inside them.", root.display()))
        }
    }

    /// Register a folder as a workspace and open it.
    pub fn add_and_open(&mut self, name: &str, root: &Path, cx: &AppCx) -> Result<(), String> {
        let canon = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
        Self::scope_ok(cx, &canon)?;
        let w = self.reg.add(name, &canon)?;
        cx.audit.record(&cx.session.user, "ide.workspace.add", &w.root);
        self.open_by_id(&w.id, cx)
    }

    /// Where new workspaces go when the person has not chosen: the remembered folder, else `~/Workspaces` when it can be
    /// created there and the account may write in it, else a folder of the account's own under the data folder (window mode,
    /// or any machine where the account has no real home).
    pub fn default_parent(&self, cx: &AppCx) -> PathBuf {
        let user = &cx.session.user;
        let usable = |p: &Path| cx.session.policy.allows_write(user, &p.to_string_lossy()) && workspace::writable_folder(p).is_ok();
        if let Some(p) = self.reg.parent.as_deref().map(PathBuf::from) {
            if usable(&p) {
                return p;
            }
        }
        let home = PathBuf::from(format!("/home/{user}/Workspaces"));
        if usable(&home) {
            return home;
        }
        self.fallback_parent()
    }

    /// The account's own workspace folder under the data folder.
    pub fn fallback_parent(&self) -> PathBuf {
        self.dir.join("workspaces").join(&self.user)
    }

    /// The account's policy, with its own workspace folder under the data folder added to what it may read and write (the
    /// account may already write somewhere, and the window-mode account has no home to keep products in).
    pub fn scoped_cx(&self, cx: &AppCx) -> AppCx {
        let fb = self.fallback_parent().to_string_lossy().into_owned();
        let user = &cx.session.user;
        let mut c = cx.clone();
        if !c.session.policy.fs_write.is_empty() && !c.session.policy.allows_write(user, &fb) {
            c.session.policy.fs_write.push(fb.clone());
        }
        if !c.session.policy.fs_read.is_empty() && !c.session.policy.allows_read(user, &fb) {
            c.session.policy.fs_read.push(fb);
        }
        c
    }

    pub fn create_workspace(&mut self, name: &str, parent: &Path, git: bool, cx: &AppCx) -> Result<(), String> {
        let suggested = self.default_parent(cx);
        if parent.as_os_str().is_empty() {
            return Err(format!("Choose a parent folder, for example {}, or press Ctrl+U to use it.", suggested.display()));
        }
        let target = parent.join(workspace::slug(name));
        if !cx.session.policy.allows_write(&cx.session.user, &target.to_string_lossy()) {
            let scopes = cx.session.policy.write_scopes(&cx.session.user).into_iter().take(3).collect::<Vec<_>>().join(", ");
            return Err(format!("{} is outside the folders your account may write ({scopes}). Choose a parent folder inside them, for example {}, or press Ctrl+U to use it.", target.display(), suggested.display()));
        }
        if let Err(e) = workspace::writable_folder(parent) {
            return Err(workspace::friendly_io(&e, parent, &suggested));
        }
        let parent_real = std::fs::canonicalize(parent).map_err(|e| workspace::friendly_io(&e.to_string(), parent, &suggested))?;
        let w = self.reg.create(name, &parent_real, git && iyakki_agent::wsexec::git_available()).map_err(|e| workspace::friendly_io(&e, &parent_real, &suggested))?;
        self.reg.parent = Some(parent_real.to_string_lossy().into_owned());
        let _ = self.reg.save();
        cx.audit.record(&cx.session.user, "ide.workspace.create", &w.root);
        self.open_by_id(&w.id, cx)
    }

    pub fn open_by_id(&mut self, id: &str, cx: &AppCx) -> Result<(), String> {
        let w = self.reg.get(id).cloned().ok_or("no such workspace")?;
        if let Some(i) = self.ws.iter().position(|x| x.meta.id == w.id) {
            self.activate(i, cx);
            return Ok(());
        }
        let root = w.root_path();
        if !root.is_dir() {
            return Err(format!("the folder {} is missing; remove the workspace from the list or restore the folder", w.root));
        }
        Self::scope_ok(cx, &root)?;
        let ws = self.build_ws(w, cx);
        let held = ws.ro.is_some();
        self.ws.push(ws);
        let i = self.ws.len() - 1;
        self.activate(i, cx);
        if held {
            self.modal = Some(Modal::Lock(i));
        }
        cx.audit.record(&cx.session.user, "ide.workspace.open", &self.ws[i].meta.root);
        self.recheck_namespaces(cx);
        Ok(())
    }

    fn build_ws(&self, w: Workspace, cx: &AppCx) -> Ws {
        let root = w.root_path();
        let user = &cx.session.user;
        let writable = cx.session.policy.allows_write(user, &root.to_string_lossy());
        let mut status = None;
        let cfg = match if writable { ProjectConfig::ensure(&root, &w.name) } else { ProjectConfig::load(&root) } {
            Ok(c) => c,
            Err(e) => {
                let hint = workspace::friendly_io(&e, &root, &root);
                status = Some((if hint == e { format!("project.json: {e} (defaults are used until it is fixed)") } else { format!("IYAKKI cannot write .iyakki/project.json in {}: the folder is read-only for this account; the workspace opens with defaults.", root.display()) }, false));
                ProjectConfig::default()
            }
        };
        // a project file must not point the cluster view at a kubeconfig the account may not read
        let mut cfg = cfg;
        if !cfg.k8s.kubeconfig.is_empty() && !cx.session.policy.allows_read(user, &cfg.k8s.kubeconfig) {
            status = Some((format!("project.json names the kubeconfig {}, outside the folders your account may read: ignored.", cfg.k8s.kubeconfig), false));
            cfg.k8s.kubeconfig.clear();
        }
        let saved: WsSaved = std::fs::read(state_path(&self.dir, &cx.session.user, &w)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let ro = match workspace::acquire_lock(&root, user, &self.session, now_secs()) {
            LockState::Mine => None,
            LockState::Held(l) => Some(l),
        };
        let mut editor = EditApp::new(cx);
        editor.root = Some(root.clone());
        editor.ide = true;
        for f in &saved.files {
            let _ = editor.open_path(cx, &root.join(f).to_string_lossy());
        }
        let features = cfg.features_path(&root);
        let scope = ProjectScope { name: w.name.clone(), root: root.clone(), key: w.chat_key(user), features: features.clone(), cluster: None };
        let mut tasks = TasksView::new(super::ide_tasks_run::manager(cx, &root, &features, &w.name), &cfg.commands.test);
        tasks.user = cx.session.user.clone();
        let mut chat = ChatApp::new().with_agent(cx.session.policy.agent && ro.is_none()).with_project(scope).with_tasks(tasks.mgr.clone()).resumed(cx);
        chat.set_file_context(None);
        let mut tree = Tree::new(&root);
        tree.expanded = saved.expanded.iter().cloned().collect::<BTreeSet<_>>();
        tree.refresh();
        let allowed_cli = cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full && ro.is_none();
        let mut board = BoardView::new(&root, &features);
        board.read_only = ro.is_some();
        board.mode = match saved.board_mode {
            1 => super::board::Mode::Table,
            2 => super::board::Mode::Phases,
            3 => super::board::Mode::Graph,
            _ => super::board::Mode::Kanban,
        };
        let features_mtime = std::fs::metadata(&features).and_then(|m| m.modified()).ok();
        let mut ws = Ws {
            tree,
            editor,
            chat,
            term: Term::new(&root, &saved.term, allowed_cli),
            problems: Problems::new(&root, &cfg.commands.test),
            cov: super::ide_bottom::Runner::new(),
            cov_plan: None,
            search: SearchPanel::new(&root),
            k8s: None,
            browser: None,
            dev_ports: super::ide_browser::DevScan::default(),
            board,
            tasks,
            notes: NotesView::new(&root, &cx.session.user),
            ctx_last: None,
            ctx_undo: None,
            ctx_pending: None,
            prefs: saved.prefs,
            focus: saved.focus.unwrap_or(Pane::Tree),
            main_tab: saved.main_tab.unwrap_or(MainTab::Editor),
            bottom_tab: saved.bottom_tab.unwrap_or(BTab::Terminal),
            compact_pane: Pane::Main,
            ro,
            ns_warning: None,
            status,
            unseen_done: false,
            was_running: false,
            features_mtime,
            board_full: false,
            cluster: devcluster::slot(),
            applied_gen: 0,
            chooser_shown: false,
            meta: w,
            cfg,
        };
        let link = ws.cluster_link();
        if let Some(l) = link {
            let mut sc = ws.chat.project().cloned();
            if let Some(sc) = sc.as_mut() {
                sc.cluster = Some(l);
                ws.chat.set_project(sc.clone());
            }
        }
        ws.start_resolve(&self.dir, user, false);
        ws
    }

    pub fn activate(&mut self, i: usize, cx: &AppCx) {
        if i >= self.ws.len() {
            return;
        }
        self.active = Some(i);
        // Auto approvals end when the person goes to another workspace, unless pinned
        for (j, w) in self.ws.iter_mut().enumerate() {
            if j != i {
                w.chat.end_auto_unless_pinned();
            }
        }
        let id = self.ws[i].meta.id.clone();
        self.ws[i].unseen_done = false;
        self.reg.touch(&id);
        self.redraw = true;
        let _ = cx;
    }

    /// Save a workspace's layout, tabs and terminal history.
    pub fn save_state(&self, i: usize) {
        let Some(w) = self.ws.get(i) else { return };
        let root = w.root();
        let files: Vec<String> = w.editor.open_paths().iter().map(|p| workspace::rel_of(&root, p)).collect();
        let saved = WsSaved {
            prefs: w.prefs,
            focus: Some(w.focus),
            main_tab: Some(w.main_tab),
            bottom_tab: Some(w.bottom_tab),
            files,
            expanded: w.tree.expanded.iter().cloned().collect(),
            term: w.term.saved(),
            board_mode: match w.board.mode {
                super::board::Mode::Kanban => 0,
                super::board::Mode::Table => 1,
                super::board::Mode::Phases => 2,
                super::board::Mode::Graph => 3,
            },
        };
        if let Ok(t) = serde_json::to_vec_pretty(&saved) {
            let _ = std::fs::write(state_path(&self.dir, &self.user, &w.meta), t);
        }
    }

    pub fn save_all(&self) {
        for i in 0..self.ws.len() {
            self.save_state(i);
        }
    }

    /// Close a workspace's panes (its lock is released). Nothing is deleted.
    pub fn close_ws(&mut self, i: usize, _cx: &AppCx) {
        if i >= self.ws.len() {
            return;
        }
        self.save_state(i);
        let w = self.ws.remove(i);
        workspace::release_lock(&w.root(), &self.session);
        self.active = match self.active {
            Some(a) if a == i => (!self.ws.is_empty()).then(|| i.min(self.ws.len() - 1)),
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    /// Warn when two workspaces look at the same namespace of the same cluster.
    pub fn recheck_namespaces(&mut self, _cx: &AppCx) {
        let list: Vec<(String, ProjectConfig)> = self.reg.items.iter().map(|w| (w.name.clone(), ProjectConfig::load(&w.root_path()).unwrap_or_default())).collect();
        let coll = workspace::namespace_collisions(&list);
        for w in self.ws.iter_mut() {
            w.ns_warning = coll.iter().find(|(a, b, _)| *a == w.meta.name || *b == w.meta.name).map(|(a, b, ns)| {
                let other = if *a == w.meta.name { b } else { a };
                format!("Namespace {ns} is also used by the workspace \"{other}\" on the same cluster: set a different namespace in the project settings.")
            });
        }
    }

    /// Reopen a workspace read-write after the person chose to take the lock over.
    pub fn take_over(&mut self, i: usize, cx: &AppCx) {
        let Some(w) = self.ws.get_mut(i) else { return };
        workspace::take_over_lock(&w.root(), &cx.session.user, &self.session, now_secs());
        w.ro = None;
        w.term.set_allowed(cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full);
        w.chat.set_agent_mode(cx.session.policy.agent);
        w.say("You took over this workspace; the other session is now read-only.", true);
        cx.audit.record(&cx.session.user, "ide.workspace.takeover", &w.meta.root);
    }

    // ------------------------------------------------------------------------------------------------ actions

    /// Open a file of the workspace in the editor, at a line.
    pub fn open_file(&mut self, rel: &str, line: Option<usize>, cx: &AppCx) {
        let Some(w) = self.active_mut() else { return };
        let abs = w.root().join(rel);
        // a table is looked at in DATA (the shell keeps the IDE aside and comes back to it), not edited as text
        let low = rel.to_ascii_lowercase();
        if line.is_none() && [".csv", ".tsv"].iter().any(|e| low.ends_with(e)) {
            super::data::request_open(&abs.to_string_lossy(), None);
            self.request = Some(("data".into(), abs.to_string_lossy().into_owned()));
            self.redraw = true;
            return;
        }
        match w.editor.open_path(cx, &abs.to_string_lossy()) {
            Ok(()) => {
                if let Some(l) = line {
                    w.editor.goto_line(l);
                }
                w.main_tab = MainTab::Editor;
                w.set_focus(Pane::Main);
                w.board_full = false;
                w.tree.expand_to(rel);
                w.compact_pane = Pane::Main;
            }
            Err(e) => w.say(e, false),
        }
        self.redraw = true;
    }

    fn ro_check(&mut self, what: &str) -> bool {
        let Some(w) = self.active_mut() else { return false };
        if let Some(l) = &w.ro {
            let m = format!("This workspace is open read-only: {} has it open in another session. Take it over to {what}.", l.user);
            w.say(m, false);
            return true;
        }
        false
    }

    pub fn toggle_pane(&mut self, p: Pane) {
        if let Some(w) = self.active_mut() {
            if w.prefs.is_open(p) {
                w.prefs.toggle(p);
                if w.focus == p {
                    w.set_focus(Pane::Main);
                }
            } else {
                w.prefs.toggle(p);
                w.set_focus(p);
            }
            w.compact_pane = p;
        }
        self.redraw = true;
    }

    pub fn focus_pane(&mut self, p: Pane) {
        if let Some(w) = self.active_mut() {
            if !w.prefs.is_open(p) {
                w.prefs.toggle(p);
            }
            w.set_focus(p);
            w.compact_pane = p;
            if p == Pane::Main {
                w.board_full = false;
            }
        }
        self.redraw = true;
    }

    pub fn show_main(&mut self, t: MainTab) {
        if let Some(w) = self.active_mut() {
            w.main_tab = t;
            w.set_focus(Pane::Main);
            w.compact_pane = Pane::Main;
            if t != MainTab::Board {
                w.board_full = false;
            }
        }
        self.redraw = true;
    }

    pub fn show_bottom(&mut self, t: BTab) {
        if let Some(w) = self.active_mut() {
            w.bottom_tab = t;
            w.prefs.bottom_open = true;
            w.set_focus(Pane::Bottom);
            w.compact_pane = Pane::Bottom;
        }
        self.redraw = true;
    }

    /// Ctrl+P: pick a file of the workspace by name.
    pub fn open_quick(&mut self) {
        let Some(w) = self.active_ws() else { return };
        let all = workspace::all_files(&w.root(), 8000);
        let mut q = QuickOpen { query: crate::ui::TextField::new("file name…", false), results: vec![], all, sel: 0 };
        q.refilter();
        self.modal = Some(Modal::Quick(q));
    }

    /// Ctrl+Shift+F: the search tab, focused.
    pub fn open_search(&mut self) {
        self.show_bottom(BTab::Search);
    }

    /// F5 / the run button: the project's run command, or the editor's own run for scripts and WebAssembly.
    pub fn run_current(&mut self, cx: &AppCx) {
        if self.ro_check("run things") {
            return;
        }
        let Some(w) = self.active_mut() else { return };
        let ext = w.editor.path().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), "py" | "js" | "mjs" | "sh" | "wasm" | "wat") {
            w.editor.handle(&Event::Key(Key::F(5)), cx);
            w.main_tab = MainTab::Editor;
            return;
        }
        // a file inside a cloned repository that has its own run command runs that, inside the repository's folder
        let in_repo = w.editor.path().map(|p| workspace::rel_of(&w.root(), p)).and_then(|rel| w.cfg.repos.iter().filter(|r| !r.run.trim().is_empty() && (rel == r.path || rel.starts_with(&format!("{}/", r.path)))).max_by_key(|r| r.path.len()).cloned());
        if let Some(r) = in_repo {
            w.bottom_tab = BTab::Terminal;
            w.prefs.bottom_open = true;
            w.term.cwd = r.path.clone();
            w.term.run_line(r.run.trim());
            return;
        }
        let cmd = w.cfg.commands.run.trim().to_string();
        if cmd.is_empty() {
            w.say("No run command: add \"commands\": {\"run\": \"…\"} to .iyakki/project.json (Settings ⚙ in the top bar). Press T in the panel to run the tests.", false);
            return;
        }
        w.bottom_tab = BTab::Terminal;
        w.prefs.bottom_open = true;
        w.term.run_line(&cmd);
    }

    /// The Board's Coverage button: the project's coverage command (or the detected tool), run in the workspace off the UI thread. A tool
    /// that is not installed is never run: the board shows what to install.
    pub fn run_coverage(&mut self, cx: &AppCx) {
        if self.ro_check("measure coverage") {
            return;
        }
        let allowed = cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full;
        let Some(w) = self.active_mut() else { return };
        if !allowed {
            w.board.cov_note = Some("Measuring coverage needs the full command-line permission for your account.".into());
            return;
        }
        if w.cov.busy() || w.problems.runner.busy() {
            return;
        }
        match iyakki_agent::coverage::plan_here(&w.root(), &w.cfg.commands.coverage) {
            Ok(plan) => {
                w.board.cov_note = None;
                w.cov.start(&w.root(), plan.argv.clone(), std::time::Duration::from_secs(1200));
                w.cov_plan = Some(plan);
            }
            Err(hint) => {
                w.board.cov_note = Some(hint.clone());
                w.say(hint, false);
            }
        }
        self.redraw = true;
    }

    pub fn run_tests(&mut self, cx: &AppCx) {
        if self.ro_check("run the tests") {
            return;
        }
        let allowed = cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full;
        let Some(w) = self.active_mut() else { return };
        // a file inside a cloned repository that has its own test command: that command, run inside the repository's folder
        let in_repo = w.editor.path().map(|p| workspace::rel_of(&w.root(), p)).and_then(|rel| w.cfg.repos.iter().filter(|r| !r.test.trim().is_empty() && (rel == r.path || rel.starts_with(&format!("{}/", r.path)))).max_by_key(|r| r.path.len()).cloned());
        match &in_repo {
            Some(r) => {
                w.problems.command = r.test.trim().to_string();
                w.problems.root = w.root().join(&r.path);
            }
            None => {
                w.problems.command = w.cfg.commands.test.clone();
                w.problems.root = w.root();
            }
        }
        match w.problems.run(allowed) {
            Ok(()) => {
                w.bottom_tab = BTab::Problems;
                w.prefs.bottom_open = true;
            }
            Err(e) => {
                w.bottom_tab = BTab::Problems;
                w.prefs.bottom_open = true;
                w.say(e, false);
            }
        }
        self.redraw = true;
    }

    /// Send the selection (or the file) to the chat.
    pub fn send_to_chat(&mut self) {
        let Some(w) = self.active_mut() else { return };
        let root = w.root();
        let Some(p) = w.editor.path().map(|p| workspace::rel_of(&root, p)) else {
            w.say("Open a file in the editor first.", false);
            return;
        };
        let text = match w.editor.selection_info() {
            Some((a, b, t)) => format!("About {p}, lines {a}-{b}:\n```\n{t}\n```\n"),
            None => {
                let t: String = w.editor.text().chars().take(6000).collect();
                format!("About {p}:\n```\n{t}\n```\n")
            }
        };
        w.chat.prefill(&text);
        w.prefs.chat_open = true;
        w.set_focus(Pane::Chat);
        w.compact_pane = Pane::Chat;
        w.say("Sent to the chat: add your question and press Enter.", true);
        self.redraw = true;
    }

    fn ask_chat(&mut self, text: &str) {
        let Some(w) = self.active_mut() else { return };
        w.chat.prefill(text);
        w.prefs.chat_open = true;
        w.set_focus(Pane::Chat);
        w.compact_pane = Pane::Chat;
        w.say("The feature is in the chat box: press Enter to ask.", true);
    }

    pub(super) fn handle_board_act(&mut self, act: BoardAct, cx: &AppCx) {
        match act {
            BoardAct::Ask(t) => self.ask_chat(&t),
            BoardAct::Open(f, l) => self.open_file(&f, Some(l), cx),
            BoardAct::Saved(p) => {
                if let Some(w) = self.active_mut() {
                    w.editor.reload_clean(cx, &p);
                    w.features_mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                }
            }
            BoardAct::Close => {
                if let Some(w) = self.active_mut() {
                    w.board_full = false;
                    w.main_tab = MainTab::Editor;
                }
            }
            BoardAct::ToggleFull => {
                if let Some(w) = self.active_mut() {
                    w.board_full = !w.board_full;
                    w.board.full = w.board_full;
                    w.main_tab = MainTab::Board;
                    w.set_focus(Pane::Main);
                }
            }
            BoardAct::RunTests => self.run_tests(cx),
            BoardAct::RunCoverage => self.run_coverage(cx),
            BoardAct::AgentCmd(key, c) => self.agent_cmd(&key, c),
            BoardAct::AssignReady(rows) => self.assign_rows(rows, cx),
            BoardAct::AutoAssign(a) => {
                if let Some(w) = self.active_mut() {
                    w.tasks.mgr.set_auto_assign(a);
                    w.board.ext.auto = a;
                    w.say(
                        match a {
                            iyakki_agent::tasks::AutoAssign::Off => "Auto-assign is off: rows that become ready wait for you.",
                            iyakki_agent::tasks::AutoAssign::Ask => "Auto-assign asks: when a row is done, the rows that are ready are offered in the Agents strip.",
                            iyakki_agent::tasks::AutoAssign::On => "Auto-assign is on: when a row is done, an agent starts on each row that becomes ready (as many as there are free places).",
                        },
                        true,
                    );
                }
            }
            BoardAct::OpenNotes(id) => {
                self.show_main(MainTab::Notes);
                if let (Some(w), Some(id)) = (self.active_mut(), id) {
                    w.notes.select_id(&id);
                }
            }
            BoardAct::None | BoardAct::Used => {}
        }
        self.redraw = true;
    }

    // ------------------------------------------------------------------------------------------------ tree actions

    fn tree_act(&mut self, act: TreeAct, cx: &AppCx) {
        match act {
            TreeAct::None => {}
            TreeAct::Open(rel) => self.open_file(&rel, None, cx),
            TreeAct::NewFile(dir) => {
                if self.ro_check("add files") {
                    return;
                }
                let f = Form::new("New file").text("Name or path", &if dir.is_empty() { String::new() } else { format!("{dir}/") }, "relative to the workspace folder");
                self.modal = Some(Modal::Form(FormKind::NewFile(dir), f));
            }
            TreeAct::NewDir(dir) => {
                if self.ro_check("add folders") {
                    return;
                }
                let f = Form::new("New folder").text("Name or path", &if dir.is_empty() { String::new() } else { format!("{dir}/") }, "relative to the workspace folder");
                self.modal = Some(Modal::Form(FormKind::NewDir(dir), f));
            }
            TreeAct::Rename(rel) => {
                if self.ro_check("rename files") {
                    return;
                }
                let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                let f = Form::new(&format!("Rename {name}")).text("New name", &name, "a name, not a path");
                self.modal = Some(Modal::Form(FormKind::RenameFile(rel), f));
            }
            TreeAct::Delete(rel) => {
                if self.ro_check("delete files") {
                    return;
                }
                self.modal = Some(Modal::Confirm(Confirm::DeleteFile(rel)));
            }
            TreeAct::Filter => {
                let f = Form::new("Find a file by name").text("Name contains", "", "fuzzy: letters in order, e.g. mnrs finds main.rs");
                self.modal = Some(Modal::Form(FormKind::FindFile, f));
            }
        }
        self.redraw = true;
    }

    fn do_file_op(&mut self, kind: FormKind, value: String, cx: &AppCx) -> Result<(), String> {
        let Some(w) = self.active_mut() else { return Ok(()) };
        let root = w.root();
        if !cx.session.policy.allows_write(&cx.session.user, &root.to_string_lossy()) && !matches!(kind, FormKind::FindFile) {
            return Err("your account may not write in this workspace".into());
        }
        let name = value.trim().to_string();
        match kind {
            FormKind::NewFile(_) => {
                workspace::create_file(&root, &name)?;
                w.tree.expand_to(&name);
                cx.audit.record(&cx.session.user, "ide.file.create", &name);
                self.open_file(&name, None, cx);
            }
            FormKind::NewDir(_) => {
                workspace::create_dir(&root, &name)?;
                w.tree.expanded.insert(name.trim_end_matches('/').to_string());
                w.tree.refresh();
                cx.audit.record(&cx.session.user, "ide.dir.create", &name);
            }
            FormKind::RenameFile(rel) => {
                let was_open = w.editor.tab_state(&root.join(&rel)).is_some();
                if was_open && w.editor.tab_state(&root.join(&rel)) == Some(true) {
                    return Err("save or close the open file before renaming it".into());
                }
                let to = workspace::rename(&root, &rel, &name)?;
                if was_open {
                    w.editor.close_path(&root.join(&rel));
                }
                w.tree.refresh();
                cx.audit.record(&cx.session.user, "ide.file.rename", &format!("{rel} -> {name}"));
                if was_open {
                    let r = workspace::rel_of(&root, &to);
                    self.open_file(&r, None, cx);
                }
            }
            FormKind::FindFile => w.tree.set_filter(&name),
            _ => {}
        }
        Ok(())
    }

    fn do_confirm(&mut self, c: Confirm, cx: &AppCx) {
        match c {
            Confirm::DeleteFile(rel) => {
                let Some(w) = self.active_mut() else { return };
                let root = w.root();
                match workspace::delete(&root, &rel) {
                    Ok(()) => {
                        let abs = root.join(&rel);
                        w.editor.close_path(&abs);
                        w.tree.refresh();
                        w.say(format!("Deleted {rel}."), true);
                        cx.audit.record(&cx.session.user, "ide.file.delete", &rel);
                    }
                    Err(e) => w.say(e, false),
                }
            }
            Confirm::MergeTask(id, _) => self.merge_task(&id, cx),
            Confirm::DiscardTask(id) => {
                let Some(w) = self.active_mut() else { return };
                match w.tasks.mgr.discard(&id) {
                    Ok(()) => w.tasks.status = Some(("Discarded: the folder and the branch are removed.".into(), true)),
                    Err(e) => w.tasks.status = Some((e, false)),
                }
                w.tree.refresh();
            }
            Confirm::RemoveWs(id) => {
                if let Some(i) = self.ws.iter().position(|w| w.meta.id == id) {
                    self.close_ws(i, cx);
                }
                match self.reg.remove(&id) {
                    Ok(()) => self.notice = Some(("Removed from the list. The folder and its files were not touched.".into(), true)),
                    Err(e) => self.notice = Some((e, false)),
                }
                cx.audit.record(&cx.session.user, "ide.workspace.remove", &id);
                self.recheck_namespaces(cx);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ forms

    pub fn open_new_workspace_form(&mut self, cx: &AppCx) {
        let parent = self.default_parent(cx).display().to_string();
        let f = Form::new("New workspace")
            .text("Name", "", "the product, e.g. Shop Front")
            .text("Parent folder (Ctrl+O browse · Ctrl+U default)", &parent, "the folder is created inside it")
            .choice("Start a git repository", &["yes, if git is installed", "no", "no: clone repositories into it next"], 0);
        self.modal = Some(Modal::Form(FormKind::NewWs, f));
    }

    /// The folders the parent-folder chooser stays inside (the same helper as the other file pickers).
    fn pick_roots(&self, cx: &AppCx) -> iyakki_agent::scope::FileRoots {
        iyakki_agent::scope::FileRoots::new(&self.scoped_cx(cx).session.policy, &cx.session.user, &self.dir)
    }

    /// A list of the folders inside `dir` (only those the account may read), with "use this folder" on top.
    fn browse_picker(&self, dir: &Path, cx: &AppCx) -> super::widgets::Picker {
        let mut items = vec!["✔ Use this folder".to_string()];
        let roots = self.pick_roots(cx);
        // up only while the parent is still inside the person's home folder, workspaces or narrow scopes
        if dir.parent().is_some_and(|p| roots.contains(p)) {
            items.push(".. (up one level)".to_string());
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.') && cx.session.policy.allows_read(&cx.session.user, &e.path().to_string_lossy())).map(|e| format!("{}/", e.file_name().to_string_lossy())).take(400).collect();
            names.sort();
            items.extend(names);
        }
        super::widgets::Picker::new(&format!("Parent folder: {}", dir.display()), items)
    }

    /// "New app, MCP server or skill…" (top bar ⋯ menu, Ctrl+Shift+N): kind and name, then the chat with the authoring skill.
    pub fn open_new_app_form(&mut self) {
        if self.active.is_none() {
            self.notice = Some(("Open a workspace first: the project is created inside it.".into(), false));
            return;
        }
        let f = Form::new("New app, MCP server or skill").choice("What", &NEW_APP_CHOICES, 0).text("Name", "", "lowercase letters, digits, dashes: my-app");
        self.modal = Some(Modal::Form(FormKind::NewApp, f));
    }

    pub fn open_existing_form(&mut self) {
        let f = Form::new("Open an existing folder").path("Folder", "", "a folder inside your file scope", crate::filepicker::PathKind::Folder, &[]).text("Name", "", "shown in the list (default: the folder's name)");
        self.modal = Some(Modal::Form(FormKind::OpenFolder, f));
    }

    fn open_clone_form(&mut self, cx: &AppCx) {
        self.open_clone_dialog(self.active.is_none(), cx);
    }

    /// Platform services over the IDE: the list, or (G) straight into connecting this workspace to a git service.
    pub(super) fn open_platform(&mut self, i: usize, connect: bool, cx: &AppCx) {
        let (run, env) = cluster_world().unwrap_or_else(|| (Arc::new(iyakki_agent::wsexec::FakeRunner::new(&[])), ClusterEnv::default()));
        let pl = Arc::new(super::platform_services::platform_for(cx, run, env));
        let mut p = super::platform_services::PlatformPanel::new(pl).with_services(cx.services.clone());
        match (connect, self.ws.get(i)) {
            (true, Some(w)) => p.open_connect_workspace(iyakki_agent::platform::connect::WsInfo { name: w.meta.name.clone(), root: w.root() }),
            _ => p.open_list(),
        }
        self.platform = Some(p);
    }

    /// The panel's events; its wants (show an address in the Browser tab) are done here.
    fn platform_event(&mut self, ev: &Event, cx: &AppCx) {
        let Some(p) = self.platform.as_mut() else { return };
        let open = p.handle(ev);
        let wants = p.take_wants();
        if !open {
            self.platform = None;
        }
        for w in wants {
            let super::platform_services::PWant::OpenUrl(u) = w;
            self.open_browser(Some(&u), cx);
            self.platform = None;
        }
    }

    /// What the K8S panel asked for: look again, switch cluster, open the deploy form, restart a workload.
    fn handle_k8s_want(&mut self, i: usize, want: super::ide_k8s::Want, cx: &AppCx) {
        use super::ide_k8s::Want;
        let dir = self.dir.clone();
        let user = cx.session.user.clone();
        match want {
            Want::OpenUrl(u) => self.open_browser(Some(&u), cx),
            Want::Services { connect } => self.open_platform(i, connect, cx),
            Want::Reresolve => {
                if let Some(w) = self.ws.get_mut(i) {
                    w.start_resolve(&dir, &user, true);
                    w.say("Looking for a cluster…", true);
                }
            }
            Want::Choose(n) => {
                let Some(w) = self.ws.get_mut(i) else { return };
                let chosen = w.cluster.lock().ok().and_then(|g| g.resolution.as_ref().and_then(|r| r.profiles.get(n).cloned()));
                let Some(p) = chosen else { return };
                if !p.readable {
                    self.modal = Some(Modal::Info(format!("{} is not readable", p.kubeconfig), p.fix.clone()));
                    return;
                }
                let choice = devcluster::Choice { kubeconfig: p.kubeconfig.clone(), context: p.context.clone() };
                let _ = choice.save(&dir, &user);
                // the workspace remembers it too, when the account may read that file (otherwise the per-user choice carries it)
                if cx.session.policy.allows_read(&user, &p.kubeconfig) && w.ro.is_none() {
                    w.sync_registry();
                    w.cfg.k8s.kubeconfig = p.kubeconfig.clone();
                    w.cfg.k8s.profile = p.context.clone();
                    let _ = w.cfg.save(&w.root());
                } else {
                    w.cfg.k8s.kubeconfig.clear();
                    w.cfg.k8s.profile.clear();
                }
                w.start_resolve(&dir, &user, true);
                w.say(format!("Cluster set to {}: looking at it…", p.chip()), true);
            }
            Want::Hide(path) => {
                if let Err(e) = devcluster::hide(&dir, &user, &path) {
                    self.modal = Some(Modal::Info("Could not hide the cluster".into(), e));
                    return;
                }
                if let Some(w) = self.ws.get_mut(i) {
                    w.start_resolve(&dir, &user, true);
                    w.say("Hidden in IYAKKI (the kubeconfig file is untouched; Cluster… → Show hidden clusters brings it back).", true);
                }
            }
            Want::ShowHidden => {
                let _ = devcluster::hidden_save(&dir, &user, &[]);
                if let Some(w) = self.ws.get_mut(i) {
                    w.start_resolve(&dir, &user, true);
                    w.say("Hidden clusters are shown again.", true);
                }
            }
            Want::Disconnect => {
                let Some(w) = self.ws.get_mut(i) else { return };
                let _ = devcluster::Choice::default().save(&dir, &user);
                if w.ro.is_none() {
                    w.sync_registry();
                    w.cfg.k8s.kubeconfig.clear();
                    w.cfg.k8s.profile.clear();
                    let _ = w.cfg.save(&w.root());
                }
                w.start_resolve(&dir, &user, true);
                w.say("Disconnected: this workspace falls back to this machine's own k3s.", true);
            }
            Want::ClusterSwitched => {
                // the switcher saved the choice (the account's active cluster, or this workspace's own); a pin in project.json would
                // still win over it, so the pin goes (the cluster's kubeconfig stays where it was)
                let Some(w) = self.ws.get_mut(i) else { return };
                if w.ro.is_none() && (!w.cfg.k8s.kubeconfig.is_empty() || !w.cfg.k8s.profile.is_empty()) {
                    w.sync_registry();
                    w.cfg.k8s.kubeconfig.clear();
                    w.cfg.k8s.profile.clear();
                    let _ = w.cfg.save(&w.root());
                }
                w.start_resolve(&dir, &user, true);
                w.say(format!("Working against {}: looking at it\u{2026}", iyakki_agent::clusters::phrase_for(&dir, &user, &w.meta.name)), true);
            }
            Want::ManageClusters => self.request = Some((super::cluster_host::MANAGE_REQUEST.0.into(), super::cluster_host::MANAGE_REQUEST.1.into())),
            Want::Deploy => self.open_deploy_form(i),
            Want::Registry => self.open_registry_form(i),
            Want::Stop | Want::Start | Want::RegistryImages => {
                let Some(w) = self.ws.get_mut(i) else { return };
                let (tool, args, title, done): (&'static str, serde_json::Value, String, &str) = match want {
                    Want::Stop => ("k8s_stop_env", serde_json::json!({"action": "stop"}), format!("Stop {}", w.cfg.namespace(&w.meta.name)), "Stopped the dev environment: it can be started again."),
                    Want::Start => ("k8s_stop_env", serde_json::json!({"action": "start"}), format!("Start {}", w.cfg.namespace(&w.meta.name)), "Started the dev environment."),
                    _ => ("k8s_registry_cleanup", serde_json::json!({}), "Remove the images pushed for this workspace".to_string(), "Removed the workspace's images from the registry."),
                };
                match w.k8s_tools(cx) {
                    Some(t) => {
                        let flow = super::ide_deploy::Flow::single(t, tool, args, title.clone(), &title, done);
                        w.main_tab = MainTab::K8s;
                        w.k8s_view().start_flow(flow);
                    }
                    None => {
                        let why = w.deploy_block(cx).unwrap_or_default();
                        w.say(format!("Tear down is off: {why}."), false);
                    }
                }
            }
            Want::RemoveAll => {
                let Some(w) = self.ws.get(i) else { return };
                let ns = w.cfg.namespace(&w.meta.name);
                let f = Form::new(&format!("Remove all of {ns}"))
                    .text("Type the namespace name to confirm", "", &format!("Deletes the namespace {ns} and EVERYTHING in it, including the data in its volumes. Next you see exactly what goes and approve once more."));
                self.modal = Some(Modal::Form(FormKind::Teardown, f));
            }
            Want::RegistryProbe => {
                let Some(w) = self.ws.get_mut(i) else { return };
                // the setup writes project.json: take the registry fields from it (the rest of the form-edited settings stay)
                w.sync_registry();
                let saved = iyakki_agent::regsetup::RegStatus::saved(&w.cfg.k8s);
                let slot = w.k8s_view().reg.clone();
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(saved);
                if let Some((run, _)) = cluster_world() {
                    let cfg = w.cfg.k8s.clone();
                    std::thread::spawn(move || {
                        let s = iyakki_agent::regsetup::status(&cfg, &*run);
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
                    });
                }
            }
            Want::Restart(kind, name) => {
                let Some(w) = self.ws.get_mut(i) else { return };
                match w.k8s_tools(cx) {
                    Some(t) => {
                        let flow = super::ide_deploy::Flow::restart(t, &kind, &name);
                        w.k8s_view().start_flow(flow);
                    }
                    None => {
                        let why = w.deploy_block(cx).unwrap_or_default();
                        w.say(format!("Restart is off: {why}."), false);
                    }
                }
            }
        }
        self.redraw = true;
    }

    pub fn open_deploy_form(&mut self, i: usize) {
        let Some(w) = self.ws.get(i) else { return };
        let root = w.root();
        let ns = w.cfg.namespace(&w.meta.name);
        let chip = w.chosen_profile().map(|p| p.chip()).unwrap_or_default();
        let on = iyakki_agent::clusters::phrase_for(&self.dir, &self.user, &w.meta.name);
        let chip = if on.is_empty() { chip } else { format!("{chip} \u{b7} {on}") };
        // the folder with a Dockerfile: the root, else the first folder that has one
        let dir = if root.join("Dockerfile").is_file() || root.join("Containerfile").is_file() {
            ".".to_string()
        } else {
            let mut found = String::from(".");
            if let Ok(rd) = std::fs::read_dir(&root) {
                let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir() && (e.path().join("Dockerfile").is_file() || e.path().join("Containerfile").is_file())).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                if let Some(n) = names.first() {
                    found = n.clone();
                }
            }
            found
        };
        let manifest = ["deploy/app.yaml", "deploy/deployment.yaml", "k8s/deployment.yaml", "deployment.yaml"].into_iter().find(|p| root.join(p).is_file()).unwrap_or("");
        let f = Form::new(&format!("Deploy · {} · {chip}", w.meta.name))
            .text("Image name", &super::ide_deploy::app_name(&w.meta.name), "lowercase, no registry or tag; the tag is a hash of the files")
            .text("Build folder (Dockerfile)", &dir, "a folder of this workspace; . is the root")
            .text("Manifest file", manifest, "empty: generate a starter Deployment and Service in deploy/")
            .text("Port", "8080", "the port the app listens on (starter manifest only)")
            .text("Namespace", &ns, "fixed: writes only touch this workspace's namespace (change it in the project settings)")
            .choice("Approval", &["ask for each step", "approve all steps for this deploy"], 0);
        self.modal = Some(Modal::Form(FormKind::Deploy, f));
    }

    /// The registry choices with their one-line pros; Left/Right changes the choice and the line under it.
    pub fn open_registry_form(&mut self, i: usize) {
        let Some(w) = self.ws.get(i) else { return };
        let chip = w.chosen_profile().map(|p| p.chip()).unwrap_or_default();
        let saved = iyakki_agent::regsetup::RegStatus::saved(&w.cfg.k8s);
        let names: Vec<&str> = iyakki_agent::regsetup::CHOICES.iter().map(|c| c.1).collect();
        let pre = iyakki_agent::regsetup::CHOICES.iter().position(|c| c.0 == saved.kind || (c.0 == "in-cluster" && saved.kind == "none")).unwrap_or(0);
        let addr = if saved.kind == "remote" { saved.push.clone() } else { String::new() };
        let mut f = Form::new(&format!("Set up registry · {} · {chip}", w.meta.name))
            .choice(&format!("Where images go (now: {})", saved.chip().replacen("registry: ", "", 1)), &names, pre)
            .text("Existing registry (option C): host:port", &addr, "e.g. 192.168.1.68:30500; http unless the scheme below says https")
            .choice("Scheme (option C)", &["http", "https"], 0)
            .text("NodePort (option A)", "", "empty: 30500, or the next free one");
        registry_form_hint(&mut f);
        self.modal = Some(Modal::Form(FormKind::Registry, f));
    }

    pub fn open_settings_form(&mut self) {
        let Some(w) = self.active_ws() else { return };
        let c = &w.cfg;
        let f = Form::new(&format!("Project settings · {}", w.meta.name))
            .text("Name", &c.name, "shown in the prompts")
            .text("Kubernetes namespace", &c.k8s.namespace, &format!("default {}", c.namespace(&w.meta.name)))
            .text("Label selector", &c.k8s.selector, "e.g. app=web (optional)")
            .text("Cluster profile (kubeconfig context)", &c.k8s.profile, "empty: the current context")
            .path("Kubeconfig file", &c.k8s.kubeconfig, "empty: $KUBECONFIG or the default", crate::filepicker::PathKind::File, &[])
            .text("Features file", &c.features_file, "default FEATURES.md")
            .text("Test command", &c.commands.test, "e.g. cargo test")
            .text("Run command", &c.commands.run, "F5 runs this when the open file is not a script")
            .text("Stack (language)", &c.stack, "empty: detected from the files (Go, TypeScript, Python, Java, Rust, C#, PHP, C, C++)")
            .choice("IDE density (Ctrl+= denser, Ctrl+- looser)", &["Comfortable", "Compact", "Dense"], self.density.index());
        self.modal = Some(Modal::Form(FormKind::Settings, f));
    }

    fn submit_form(&mut self, kind: FormKind, f: Form, cx: &AppCx) {
        let fail = |app: &mut IdeApp, kind: FormKind, mut f: Form, e: String| {
            f.error = Some(e);
            app.modal = Some(Modal::Form(kind, f));
        };
        match kind.clone() {
            FormKind::NewWs => {
                let (name, parent) = (f.value(0), f.value(1));
                match self.create_workspace(&name, Path::new(parent.trim()), f.choice_idx(2) == 0, cx) {
                    Err(e) => fail(self, kind, f, e),
                    Ok(()) if f.choice_idx(2) == 2 => self.open_clone_dialog(false, cx),
                    Ok(()) => {}
                }
            }
            FormKind::OpenFolder => {
                let path = f.value(0);
                let p = PathBuf::from(path.trim());
                let name = if f.value(1).trim().is_empty() { p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "workspace".into()) } else { f.value(1) };
                if let Err(e) = self.add_and_open(&name, &p, cx) {
                    fail(self, kind, f, e);
                }
            }
            FormKind::Clone => self.open_clone_dialog(false, cx),
            FormKind::Git(g) => self.git_submit_form(g, f, cx),
            FormKind::RenameWs(id) => {
                if let Err(e) = self.reg.rename(&id, &f.value(0)) {
                    fail(self, kind, f, e);
                } else if let Some(w) = self.ws.iter_mut().find(|w| w.meta.id == id) {
                    w.meta.name = f.value(0).trim().to_string();
                }
            }
            FormKind::Settings => {
                let dir = self.dir.clone();
                let Some(w) = self.active_mut() else { return };
                w.sync_registry();
                let mut c = w.cfg.clone();
                c.name = f.value(0).trim().to_string();
                c.k8s.namespace = f.value(1).trim().to_string();
                c.k8s.selector = f.value(2).trim().to_string();
                c.k8s.profile = f.value(3).trim().to_string();
                c.k8s.kubeconfig = f.value(4).trim().to_string();
                c.features_file = if f.value(5).trim().is_empty() { "FEATURES.md".into() } else { f.value(5).trim().to_string() };
                c.commands.test = f.value(6).trim().to_string();
                c.commands.run = f.value(7).trim().to_string();
                c.stack = f.value(8).trim().to_string();
                if !iyakki_agent::projectstate::is_known_stack(&c.stack) {
                    return fail(self, kind, f, "Unknown stack. Use one of: Go, JavaScript, TypeScript, Python, Java, Rust, C#, PHP, C, C++ (or leave it empty).".into());
                }
                let density = super::ide_chrome::Density::from_index(f.choice_idx(9));
                if !c.k8s.kubeconfig.is_empty() && !cx.session.policy.allows_read(&cx.session.user, &c.k8s.kubeconfig) {
                    return fail(self, kind, f, "That kubeconfig is outside the folders your account may read.".into());
                }
                if let Err(e) = workspace::resolve_in(&w.root(), &c.features_file) {
                    return fail(self, kind, f, e);
                }
                if let Err(e) = c.save(&w.root()) {
                    return fail(self, kind, f, e);
                }
                w.cfg = c;
                self.set_density(density);
                let w = self.active_mut().unwrap();
                w.problems.command = w.cfg.commands.test.clone();
                let features = w.cfg.features_path(&w.root());
                w.board = {
                    let mut b = BoardView::new(&w.root(), &features);
                    b.mode = w.board.mode;
                    b
                };
                let link = w.cluster_link();
                w.chat.set_project(ProjectScope { name: w.meta.name.clone(), root: w.root(), key: w.meta.chat_key(&cx.session.user), features, cluster: link });
                let t = w.target();
                if let Some(k) = w.k8s.as_mut() {
                    k.set_target(t);
                }
                let user = cx.session.user.clone();
                w.start_resolve(&dir, &user, false);
                w.say("Project settings saved to .iyakki/project.json.", true);
                self.recheck_namespaces(cx);
            }
            FormKind::NewTask => {
                let (title, goal, rows, agent) = (f.value(0), f.value(1), f.value(2), f.choice_idx(3));
                let rows: Vec<String> = rows.split([',', ' ']).map(str::trim).filter(|r| !r.is_empty()).map(str::to_string).collect();
                let agents = self.task_agents(cx);
                let (agent_id, model) = agents.get(agent).cloned().unwrap_or(("builtin".into(), String::new()));
                let num = |i: usize| -> Result<u64, String> {
                    let v = f.value(i);
                    if v.trim().is_empty() { Ok(0) } else { v.trim().replace(',', "").parse::<u64>().map_err(|_| format!("\"{}\" is not a number", v.trim())) }
                };
                let budgets = match (num(4), num(5), num(6)) {
                    (Ok(m), Ok(t), Ok(c)) => iyakki_agent::tasks::Budgets { max_minutes: if m == 0 { 30 } else { m.min(24 * 60) }, max_tokens: t, max_cost_cents: c, ..Default::default() },
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return fail(self, kind, f, e),
                };
                match self.spawn_tasks(vec![iyakki_agent::tasks::NewTask { title, goal, rows, agent: agent_id, model, budgets: Some(budgets), ..Default::default() }], cx) {
                    Ok(_) => {}
                    Err(e) => fail(self, kind, f, e),
                }
            }
            FormKind::NewApp => {
                let (name, choice) = (f.value(1).trim().to_string(), f.choice_idx(0).min(NEW_APP_CHOICES.len() - 1));
                let Some(w) = self.active_mut() else { return };
                let root = w.root();
                if w.ro.is_some() || !cx.session.policy.allows_write(&cx.session.user, &root.to_string_lossy()) {
                    return fail(self, kind, f, "this workspace is read-only for you".into());
                }
                use iyakki_agent::target::Target as AppTarget;
                let (target, sk) = [(AppTarget::IyakkiTile, Some("tile")), (AppTarget::K3sService, None), (AppTarget::Skill, Some("skill")), (AppTarget::McpServer, Some("mcp-python")), (AppTarget::McpServer, Some("mcp-node")), (AppTarget::WasmTool, Some("wasm-tool"))][choice];
                let record = |w: &mut Ws| {
                    let _ = iyakki_agent::target::write(&w.root(), &w.meta.name.clone(), target);
                    if let Ok(c) = ProjectConfig::load(&w.root()) {
                        w.cfg = c;
                    }
                };
                let Some(sk) = sk else {
                    // a service on k3s: the workspace is told so, no files yet; the assistant builds it with the deploy tools
                    record(w);
                    cx.audit.record(&cx.session.user, "ide.target", target.id());
                    w.chat.prefill("This workspace is a web service for k3s. Build it: <say here what it should do>. Use the deploy tools when it is ready.");
                    w.prefs.chat_open = true;
                    w.set_focus(Pane::Chat);
                    w.compact_pane = Pane::Chat;
                    w.say("Target: web service on k3s. Say what it should do in the chat.", true);
                    return;
                };
                match iyakki_agent::store::author::scaffold(sk, &name, &root, &name) {
                    Ok(_) => {
                        w.tree.expanded.insert(name.clone());
                        w.tree.refresh();
                        record(w);
                        cx.audit.record(&cx.session.user, "ide.app.scaffold", &format!("{sk} {name}"));
                        let (skill, note) = if sk == "tile" { ("iyakki-tile-author", " It is an IYAKKI tile app: no server, no Dockerfile, no k3s.") } else { ("iyakki-app-author", "") };
                        w.chat.prefill(&format!("I just created the {sk} project `{name}` in the folder {name}/. Use the {skill} skill.{note} It should: <say here what it should do>. Edit it, run its test, validate it, then {}.", if sk == "tile" { "add it to my launcher with tile_install_dev, and publish it to my private store when I approve" } else { "pack and sign it; publish to my private store (or install it locally for a trial) when I approve" }));
                        w.prefs.chat_open = true;
                        w.set_focus(Pane::Chat);
                        w.compact_pane = Pane::Chat;
                        w.say(format!("Created {name}. The chat has the next step: say what it should do and press Enter."), true);
                    }
                    Err(e) => fail(self, kind, f, e),
                }
            }
            FormKind::Target => self.submit_target_form(f),
            FormKind::Publish => self.submit_publish(f, cx),
            FormKind::PublishToken => self.submit_publish_token(f, cx),
            FormKind::GuideAgent(id) => {
                let msg = f.value(0);
                let Some(w) = self.active_mut() else { return };
                match w.tasks.mgr.guide(&id, &msg) {
                    Ok(()) => w.say("Sent: the agent pauses after its step and continues with your message.", true),
                    Err(e) => w.say(e, false),
                }
            }
            FormKind::TaskFollowup(id) => {
                let msg = f.value(0);
                let Some(w) = self.active_mut() else { return };
                match w.tasks.mgr.resume(&id, &msg) {
                    Ok(()) => w.tasks.status = Some(("Queued: it continues with your message.".into(), true)),
                    Err(e) => w.tasks.status = Some((e, false)),
                }
            }
            FormKind::Teardown => {
                let Some(i) = self.active else { return };
                let w = &mut self.ws[i];
                let ns = w.cfg.namespace(&w.meta.name);
                if f.value(0) != ns {
                    return fail(self, kind, f, format!("Type exactly {ns} to confirm. Nothing was changed."));
                }
                let Some(tools) = w.k8s_tools(cx) else {
                    let why = w.deploy_block(cx).unwrap_or_else(|| "no cluster".into());
                    return fail(self, kind, f, format!("Tear down is off: {why}."));
                };
                w.main_tab = MainTab::K8s;
                w.focus = Pane::Main;
                let flow = super::ide_deploy::Flow::single(tools, "k8s_teardown", serde_json::json!({"confirm": ns}), format!("Remove all of {ns}"), &format!("Remove all of {ns}"), "Removed: the namespace and everything in it is gone (see the output for leftovers).");
                w.k8s_view().start_flow(flow);
            }
            FormKind::Registry => {
                let Some(i) = self.active else { return };
                let w = &mut self.ws[i];
                let option = iyakki_agent::regsetup::CHOICES[f.choice_idx(0).min(2)].0;
                let mut args = serde_json::json!({"option": option});
                match option {
                    "remote" => {
                        let addr = f.value(1);
                        if addr.is_empty() {
                            return fail(self, kind, f, "Option C needs the registry's host:port.".into());
                        }
                        args["registry"] = addr.into();
                        args["scheme"] = ["http", "https"][f.choice_idx(2).min(1)].into();
                    }
                    "in-cluster" if !f.value(3).is_empty() => match f.value(3).parse::<u16>() {
                        Ok(p) if (30500..=32767).contains(&p) => args["node_port"] = p.into(),
                        _ => return fail(self, kind, f, "The NodePort is a number from 30500 to 32767.".into()),
                    },
                    _ => {}
                }
                let Some(tools) = w.k8s_tools(cx) else {
                    let why = w.deploy_block(cx).unwrap_or_else(|| "no cluster".into());
                    return fail(self, kind, f, format!("Registry setup is off: {why}."));
                };
                if option == "system" && !tools.profile.local {
                    return fail(self, kind, f, "B cannot work here: this cluster is on another machine and its nodes cannot reach this machine's localhost:5000. Choose A (a registry inside the cluster) or C (an existing registry).".into());
                }
                w.main_tab = MainTab::K8s;
                w.set_focus(Pane::Main);
                // Save writes the choice at once (project.json k8s.registry_kind is the one source of truth); the approved plan then
                // fills in the rest. Until it is applied the chip says so, and deploy stops at "set it up".
                let (kind_w, push) = match option {
                    "in-cluster" => ("in-cluster", String::new()),
                    "system" => ("system", iyakki_agent::regsetup::LOCAL_REGISTRY.to_string()),
                    _ => ("remote", args["registry"].as_str().unwrap_or("").trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string()),
                };
                w.sync_registry();
                if w.cfg.k8s.registry_kind != kind_w || w.cfg.k8s.registry != push {
                    w.cfg.k8s.registry_kind = kind_w.into();
                    w.cfg.k8s.registry = push;
                    w.cfg.k8s.registry_pull.clear();
                    let _ = w.cfg.save(&w.root());
                }
                w.say(format!("Registry option {} chosen and saved: review the plan and approve it to set it up.", ["A", "B", "C"][f.choice_idx(0).min(2)]), true);
                let flow = super::ide_deploy::Flow::registry(tools, args);
                w.k8s_view().start_flow(flow);
            }
            FormKind::Deploy => {
                let Some(i) = self.active else { return };
                let w = &mut self.ws[i];
                let ns = w.cfg.namespace(&w.meta.name);
                if f.value(4) != ns {
                    return fail(self, kind, f, format!("Writes are limited to this workspace's namespace \"{ns}\"; change it in the project settings (⚙)."));
                }
                let image = f.value(0).trim_start_matches("localhost/").to_string();
                if image.is_empty() || !image.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_./".contains(c)) {
                    return fail(self, kind, f, "The image name uses lowercase letters, digits and - _ . / only.".into());
                }
                let dir = if f.value(1).is_empty() { ".".to_string() } else { f.value(1) };
                match workspace::resolve_in(&w.root(), &dir) {
                    Ok(p) if p.is_dir() && (p.join("Dockerfile").is_file() || p.join("Containerfile").is_file()) => {}
                    Ok(_) => return fail(self, kind, f, format!("{dir} has no Dockerfile or Containerfile.")),
                    Err(e) => return fail(self, kind, f, e),
                }
                let manifest = f.value(2);
                if !manifest.is_empty() {
                    match workspace::resolve_in(&w.root(), &manifest) {
                        Ok(p) if p.is_file() => {}
                        Ok(_) => return fail(self, kind, f, format!("{manifest} is not a file.")),
                        Err(e) => return fail(self, kind, f, e),
                    }
                }
                let Ok(port) = f.value(3).parse::<u16>() else { return fail(self, kind, f, "The port is a number from 1 to 65535.".into()) };
                if port == 0 {
                    return fail(self, kind, f, "The port is a number from 1 to 65535.".into());
                }
                let Some(tools) = w.k8s_tools(cx) else {
                    let why = w.deploy_block(cx).unwrap_or_else(|| "no cluster".into());
                    return fail(self, kind, f, format!("Deploy is off: {why}."));
                };
                let plan = super::ide_deploy::Plan { image, dir, manifest, port, approve_all: f.choice_idx(5) == 1 };
                w.main_tab = MainTab::K8s;
                w.set_focus(Pane::Main);
                let flow = super::ide_deploy::Flow::deploy(tools, plan);
                w.k8s_view().start_flow(flow);
            }
            FormKind::NewFile(_) | FormKind::NewDir(_) | FormKind::RenameFile(_) | FormKind::FindFile => {
                let v = f.value(0);
                if let Err(e) = self.do_file_op(kind.clone(), v, cx) {
                    fail(self, kind, f, e);
                }
            }
        }
        self.redraw = true;
    }

    fn modal_event(&mut self, ev: &Event, cx: &AppCx) {
        self.redraw = true;
        let Some(m) = self.modal.take() else { return };
        match m {
            Modal::Form(kind, mut f) if kind == FormKind::NewWs && matches!(ev, Event::Key(Key::Ctrl('u' | 'o'))) => {
                if matches!(ev, Event::Key(Key::Ctrl('u'))) {
                    f.set_value(1, &self.default_parent(cx).display().to_string());
                    f.error = None;
                    self.modal = Some(Modal::Form(kind, f));
                } else {
                    let start = PathBuf::from(f.value(1).trim());
                    let roots = self.pick_roots(cx);
                    let dir = if start.is_dir() && cx.session.policy.allows_read(&cx.session.user, &start.to_string_lossy()) && std::fs::canonicalize(&start).is_ok_and(|r| roots.contains(&r)) { start } else { self.default_parent(cx) };
                    let picker = self.browse_picker(&dir, cx);
                    self.modal = Some(Modal::Browse(Box::new(f), dir, picker));
                }
            }
            Modal::Browse(mut f, dir, mut picker) => match picker.handle_event(ev) {
                super::widgets::PickEvent::Cancel => self.modal = Some(Modal::Form(FormKind::NewWs, *f)),
                super::widgets::PickEvent::Chosen(label) => {
                    if label.starts_with("✔") {
                        f.set_value(1, &dir.display().to_string());
                        self.modal = Some(Modal::Form(FormKind::NewWs, *f));
                    } else {
                        let next = if label.starts_with("..") { dir.parent().map(Path::to_path_buf).unwrap_or(dir.clone()) } else { dir.join(label.trim_end_matches('/')) };
                        let roots = self.pick_roots(cx);
                        let next = if cx.session.policy.allows_read(&cx.session.user, &next.to_string_lossy()) && std::fs::canonicalize(&next).is_ok_and(|r| roots.contains(&r)) { next } else { dir.clone() };
                        let p = self.browse_picker(&next, cx);
                        self.modal = Some(Modal::Browse(f, next, p));
                    }
                }
                super::widgets::PickEvent::None => self.modal = Some(Modal::Browse(f, dir, picker)),
            },
            Modal::Form(kind, mut f) => {
                let ev2 = match ev {
                    Event::Sub(SUB, k) => Event::Key(k.clone()),
                    o => o.clone(),
                };
                match f.handle_event(&ev2) {
                    FormEvent::Cancel => {}
                    FormEvent::Submit => self.submit_form(kind, f, cx),
                    FormEvent::Changed(0) if kind == FormKind::Registry => {
                        registry_form_hint(&mut f);
                        self.modal = Some(Modal::Form(kind, f));
                    }
                    _ => self.modal = Some(Modal::Form(kind, f)),
                }
            }
            Modal::Card(mut cm) => {
                let ev2 = match ev {
                    Event::Sub(SUB, k) => Event::Key(k.clone()),
                    o => o.clone(),
                };
                match cm.card.handle(&ev2) {
                    Some(k) => self.card_action(cm, k, cx),
                    None => {
                        // Esc with no button for it closes; anything else keeps the card
                        if !matches!(ev2, Event::Key(Key::Esc)) {
                            self.modal = Some(Modal::Card(cm));
                        } else {
                            self.card_action(cm, Key::Esc, cx);
                        }
                    }
                }
            }
            Modal::Git(gm) => self.git_modal_event(*gm, ev, cx),
            Modal::Info(t, m) => {
                if !matches!(ev, Event::Key(Key::Enter | Key::Esc | Key::Char(' ')) | Event::Sub(SUB, _)) {
                    self.modal = Some(Modal::Info(t, m));
                }
            }
            Modal::Confirm(c) => match ev {
                Event::Key(Key::Char('y')) | Event::Sub(SUB, Key::Char('y')) => self.do_confirm(c, cx),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) | Event::Sub(SUB, Key::Esc) => {}
                _ => self.modal = Some(Modal::Confirm(c)),
            },
            Modal::Lock(i) => match ev {
                Event::Key(Key::Char('t')) | Event::Sub(SUB, Key::Char('t')) => self.take_over(i, cx),
                Event::Key(Key::Char('r')) | Event::Sub(SUB, Key::Char('r')) => {
                    if let Some(w) = self.ws.get_mut(i) {
                        w.say("Opened read-only: another session has this workspace. Reading and searching work; changes are blocked.", true);
                    }
                }
                // never a silent read-only: Cancel (Esc) leaves the workspace closed
                Event::Key(Key::Esc) | Event::Sub(SUB, Key::Esc) => self.close_ws(i, cx),
                _ => self.modal = Some(Modal::Lock(i)),
            },
            Modal::Quick(mut q) => match ev {
                Event::Key(Key::Esc) => {}
                Event::Key(Key::Enter) => {
                    if let Some(f) = q.results.get(q.sel).cloned() {
                        self.open_file(&f, None, cx);
                    }
                }
                Event::Row(r) if (ROW_QUICK..ROW_QUICK + 1000).contains(r) => {
                    if let Some(f) = q.results.get(r - ROW_QUICK).cloned() {
                        self.open_file(&f, None, cx);
                    }
                }
                Event::Key(Key::Up) => {
                    q.sel = q.sel.saturating_sub(1);
                    self.modal = Some(Modal::Quick(q));
                }
                Event::Key(Key::Down) => {
                    q.sel = (q.sel + 1).min(q.results.len().saturating_sub(1));
                    self.modal = Some(Modal::Quick(q));
                }
                Event::Key(k) => {
                    q.query.handle(k);
                    q.refilter();
                    self.modal = Some(Modal::Quick(q));
                }
                Event::Paste(t) => {
                    q.query.paste(t);
                    q.refilter();
                    self.modal = Some(Modal::Quick(q));
                }
                Event::FocusAt(_, x) => {
                    q.query.click(*x);
                    self.modal = Some(Modal::Quick(q));
                }
                _ => self.modal = Some(Modal::Quick(q)),
            },
            Modal::Switcher { mut sel } => {
                let order = self.reg.ordered();
                let n = order.len();
                let key = match ev {
                    Event::Key(k) | Event::Sub(SUB, k) => Some(k.clone()),
                    _ => None,
                };
                match (ev, key) {
                    (Event::Row(r), _) if (ROW_SWITCH..ROW_SWITCH + 1000).contains(r) => {
                        if let Some(w) = order.get(r - ROW_SWITCH) {
                            if let Err(e) = self.open_by_id(&w.id, cx) {
                                self.notice = Some((e, false));
                            }
                        }
                    }
                    (_, Some(Key::Esc)) => {}
                    (_, Some(Key::Up)) => {
                        sel = sel.saturating_sub(1);
                        self.modal = Some(Modal::Switcher { sel });
                    }
                    (_, Some(Key::Down)) => {
                        sel = (sel + 1).min(n.saturating_sub(1));
                        self.modal = Some(Modal::Switcher { sel });
                    }
                    (_, Some(Key::Enter)) => {
                        if let Some(w) = order.get(sel) {
                            if let Err(e) = self.open_by_id(&w.id, cx) {
                                self.notice = Some((e, false));
                            }
                        }
                    }
                    (_, Some(Key::Char('n'))) => self.open_new_workspace_form(cx),
                    (_, Some(Key::Char('o'))) => self.open_existing_form(),
                    (_, Some(Key::Char('c'))) => self.open_clone_form(cx),
                    (_, Some(Key::Char('r'))) => {
                        if let Some(w) = order.get(sel) {
                            let f = Form::new("Rename workspace").text("Name", &w.name, "");
                            self.modal = Some(Modal::Form(FormKind::RenameWs(w.id.clone()), f));
                        }
                    }
                    (_, Some(Key::Char('p'))) => {
                        if let Some(w) = order.get(sel) {
                            let _ = self.reg.pin(&w.id, !w.pinned);
                        }
                        self.modal = Some(Modal::Switcher { sel });
                    }
                    (_, Some(Key::Char('a'))) => {
                        if let Some(w) = order.get(sel) {
                            let _ = self.reg.set_accent(&w.id, w.accent + 1);
                        }
                        self.modal = Some(Modal::Switcher { sel });
                    }
                    (_, Some(Key::Delete)) | (_, Some(Key::Char('x'))) => {
                        if let Some(w) = order.get(sel) {
                            self.modal = Some(Modal::Confirm(Confirm::RemoveWs(w.id.clone())));
                        }
                    }
                    _ => self.modal = Some(Modal::Switcher { sel }),
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ heartbeat

    fn on_tick(&mut self, cx: &AppCx) {
        if self.ticks % 30 == 0 {
            self.refresh_target();
        }
        self.ticks += 1;
        self.git_tick(cx);
        self.browser_tick(cx);
        // a link the person clicked in the chat: the Browser tab, a new tab of the BROWSER tile, or the editor
        let outs: Vec<(usize, super::chat::LinkOut)> = self.ws.iter_mut().enumerate().filter_map(|(i, w)| w.chat.take_link_out().map(|o| (i, o))).collect();
        for (i, out) in outs {
            if self.active != Some(i) {
                continue;
            }
            match out {
                super::chat::LinkOut::IdeBrowser(u) => self.open_browser(Some(&u), cx),
                super::chat::LinkOut::Browser(u) => {
                    super::browser::request_open(&u);
                    self.request = Some(("browser".into(), u));
                }
                super::chat::LinkOut::Editor { path, line } => self.open_file(&path, line, cx),
            }
        }
        let now = now_secs();
        let tick = self.ticks;
        let active = self.active;
        let session = self.session.clone();
        let user = cx.session.user.clone();
        let mut acts = Vec::new();
        let mut wants: Vec<(usize, super::ide_k8s::Want)> = Vec::new();
        for (i, w) in self.ws.iter_mut().enumerate() {
            // the chat and the editor of every workspace keep working in the background
            w.chat.handle(&Event::Tick, cx);
            w.editor.handle(&Event::Tick, cx);
            let running = w.chat.is_running() || w.chat.pending_approval().is_some();
            if w.was_running && !running && active != Some(i) {
                w.unseen_done = true;
            }
            w.was_running = running;
            if running {
                self.redraw = true;
            }
            // edits the assistant proposed to the open file become reviewable changes in the editor
            if let Some(reply) = w.chat.take_proposal() {
                match w.editor.propose(&reply) {
                    Ok(Some(_)) => {
                        w.main_tab = MainTab::Editor;
                        if active == Some(i) {
                            w.set_focus(Pane::Main);
                        }
                        w.say("The assistant proposed changes: review them in the editor (y/n per change, Enter applies).", true);
                    }
                    Ok(None) => {}
                    Err(e) => w.say(format!("Could not use the assistant's edit: {e}"), false),
                }
                self.redraw = true;
            }
            // a finished coverage run: read its report, store it, and let the board show it
            if let Some((o, _)) = w.cov.poll() {
                if let Some(plan) = w.cov_plan.take() {
                    match iyakki_agent::coverage::record_run(&w.root(), &plan, &o.stdout, iyakki_agent::clock::now_unix()) {
                        Ok(c) => {
                            w.board.refresh_quality(true);
                            w.say(format!("Coverage {}%", c.overall().map_or("-".to_string(), |p| format!("{p:.0}"))), true);
                        }
                        Err(e) => {
                            let tail: String = o.stderr.lines().last().unwrap_or("").chars().take(160).collect();
                            w.board.cov_note = Some(format!("{e}{}", if o.ok() || tail.is_empty() { String::new() } else { format!(" ({tail})") }));
                        }
                    }
                }
                self.redraw = true;
            }
            w.board.busy = if w.cov.busy() { Some("measuring coverage…".into()) } else if w.problems.runner.busy() { Some("running the tests…".into()) } else { None };
            if w.term.tick() | w.problems.tick() | w.search.tick() {
                self.redraw = true;
            }
            if w.term.runner.busy() || w.problems.runner.busy() || w.search.busy() {
                self.redraw = true;
            }
            // the cluster was (re)found: point the view at it and say whether Deploy works
            let gen = w.cluster.lock().map_or(0, |g| if g.resolving { w.applied_gen } else { g.generation });
            if gen != w.applied_gen {
                w.applied_gen = gen;
                // more than one cluster answers and none was chosen for this workspace: ask once, never pick silently
                let ambiguous = w.cluster.lock().ok().and_then(|g| g.resolution.as_ref().map(|r| r.ambiguous)).unwrap_or(false);
                if ambiguous && !w.chooser_shown {
                    w.chooser_shown = true;
                    w.main_tab = MainTab::K8s;
                    w.k8s_view().open_cluster_picker();
                }
                let t = w.target();
                if let Some(k) = w.k8s.as_mut() {
                    k.set_target(t);
                }
                self.redraw = true;
            }
            let block = w.deploy_block(cx);
            // terminal: kubectl / helm go to the cluster this workspace uses
            w.term.kube = w.chosen_profile().filter(|p| p.readable).map(|p| (p.kubeconfig, p.context, p.server, w.meta.name.clone()));
            if let Some(k) = w.k8s.as_mut() {
                k.deploy_block = block;
                if k.host.is_none() {
                    k.host = Some(super::cluster_host::ClusterHost::new(super::ide_k8s::SWITCH_BASE, cx, &w.meta.name));
                }
                k.tick();
                let f = k.feed.fetches();
                if f != self.last_fetches {
                    self.last_fetches = f;
                    self.redraw = true;
                }
                if k.flow.is_some() || k.has_modal() {
                    self.redraw = true;
                }
                for want in k.take_wants() {
                    wants.push((i, want));
                }
            }
            w.tasks.tick();
            w.board.ext.auto = w.tasks.mgr.auto_assign();
            if let Some(k) = w.board.ext.drill.clone() {
                w.board.ext.log = w.tasks.mgr.log(&k);
            }
            w.notes.read_only = w.ro.is_some();
            match w.notes.tick() {
                NotesAct::Toast(t) => {
                    w.say(t, true);
                    self.redraw = true;
                }
                _ if active == Some(i) && w.main_tab == MainTab::Notes => {}
                _ => {}
            }
            if w.tasks.chip().is_some() || (active == Some(i) && w.main_tab == MainTab::Tasks) {
                self.redraw = true;
            }
            // files the agent or the terminal wrote show up in the tree within about a second
            if tick % 30 == 0 && w.tree.poll() {
                self.redraw = true;
            }
            if active == Some(i) {
                w.board.read_only = w.ro.is_some();
                let act = w.board.tick();
                if act != BoardAct::None {
                    acts.push(act);
                }
                // the features file: the editor tab follows the board and the board follows the editor
                if tick % 30 == 0 {
                    let m = std::fs::metadata(w.features_path()).and_then(|m| m.modified()).ok();
                    if m != w.features_mtime {
                        w.features_mtime = m;
                        let p = w.features_path();
                        w.editor.reload_clean(cx, &p);
                        let a = w.board.poll_disk();
                        if a != BoardAct::None {
                            acts.push(a);
                        }
                        self.redraw = true;
                    }
                }
            }
            // keep the lock fresh; notice a take-over
            if tick % 900 == 0 && w.ro.is_none() && !workspace::refresh_lock(&w.root(), &user, &session, now) {
                w.ro = workspace::read_lock(&w.root());
                w.term.set_allowed(false);
                w.chat.set_agent_mode(false);
                let who = w.ro.as_ref().map_or("another window".to_string(), |l| if l.same_account_here(&user) { format!("another IYAKKI window on this computer (process {})", l.pid) } else { format!("{}", l.user) });
                w.say(format!("Another session took this workspace over ({who}): it is read-only here now. Use Take over to get it back."), false);
                self.redraw = true;
            }
        }
        // a second of quiet: redraw so "updated Ns ago" and the dots move
        if tick % 30 == 0 {
            self.redraw = true;
        }
        for a in acts {
            self.handle_board_act(a, cx);
        }
        for (i, want) in wants {
            self.handle_k8s_want(i, want, cx);
        }
        if tick % 1800 == 0 {
            self.save_all();
        }
    }

    // ------------------------------------------------------------------------------------------------ keys

    /// Change how dense the IDE is drawn and keep it for this person.
    pub fn set_density(&mut self, d: super::ide_chrome::Density) {
        self.density = d;
        crate::uisize::save_ide_density(&self.dir, &self.user, d.index());
        self.redraw = true;
    }

    /// The composer's text is an instruction to change the project context (or the answer to its question): do it, say so in the chat and keep
    /// what is left of the message for the model. True when nothing is left to send.
    pub(super) fn try_context_intent(&mut self, i: usize, cx: &AppCx) -> bool {
        use iyakki_agent::contextintent as ci;
        let w = &mut self.ws[i];
        if w.chat.is_running() || w.chat.pending_approval().is_some() {
            return false;
        }
        let text = w.chat.input_text().trim().to_string();
        if text.is_empty() {
            return false;
        }
        let (root, person, now) = (w.root(), cx.session.user.clone(), iyakki_agent::clock::now_unix() as i64);
        // the numbered answer to "which one?"
        let done = if let Some((det, ids)) = w.ctx_pending.clone() {
            let pick = text.trim_end_matches('.').parse::<usize>().ok();
            if pick.is_some() || text.eq_ignore_ascii_case("cancel") {
                w.ctx_pending = None;
                Some((text.clone(), ci::apply_choice(&root, &person, &det, &ids, pick.unwrap_or(0), now), String::new(), None))
            } else {
                w.ctx_pending = None;
                None
            }
        } else {
            None
        };
        let (shown, done, rest, det) = match done {
            Some((t, d, r, det)) => (t, d, r, det),
            None => {
                let Some(det) = ci::detect(&text) else { return false };
                let d = ci::apply(&root, &person, &det, w.ctx_last.as_deref(), now);
                if d.refused {
                    // a credential: the chat's own handling takes the message (it offers the vault)
                    w.say(d.reply, false);
                    return false;
                }
                (text.clone(), d, det.rest.clone(), Some(det))
            }
        };
        let mut reply = done.reply.clone();
        if done.undo.is_some() {
            reply.push_str("\nUndo: click \"Undo context change\" at the top, or press z in the Context tab.");
        }
        w.chat.local_exchange(cx, &shown, &reply);
        if let Some(t) = done.touched.clone() {
            w.ctx_last = Some(t);
        }
        if done.undo.is_some() {
            w.ctx_undo = done.undo.clone();
        }
        if let (Some(det), false) = (det, done.choices.is_empty()) {
            w.ctx_pending = Some((det, done.choices.clone()));
        }
        w.say(done.reply.lines().next().unwrap_or("").to_string(), !done.reply.starts_with("Not"));
        w.notes.reload();
        self.redraw = true;
        let w = &mut self.ws[i];
        if rest.is_empty() {
            w.chat.prefill("");
            true
        } else {
            // the rest of the message (a task) goes on to the model
            w.chat.prefill(&rest);
            false
        }
    }

    /// Take the last change of the project context back (the top-bar chip, `z` in the Context tab).
    pub(super) fn undo_context(&mut self) {
        let Some(w) = self.active_mut() else { return };
        let Some(u) = w.ctx_undo.take() else { return };
        match iyakki_agent::contextintent::undo(&w.root(), &u) {
            Ok(m) => w.say(m, true),
            Err(e) => w.say(e, false),
        }
        w.notes.reload();
        self.redraw = true;
    }

    /// Shortcuts of the IDE itself. True when the key was used.
    fn global_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        match k {
            Key::Ctrl('p') => self.open_quick(),
            Key::Ctrl('F') => self.open_search(),
            Key::Ctrl('b') => self.toggle_pane(Pane::Tree),
            Key::Ctrl('j') => self.toggle_pane(Pane::Bottom),
            Key::Ctrl('B') => {
                let on = self.active_ws().is_some_and(|w| w.main_tab == MainTab::Board && w.focus == Pane::Main);
                self.show_main(if on { MainTab::Editor } else { MainTab::Board });
            }
            Key::Ctrl('G') => {
                let on = self.active_ws().is_some_and(|w| w.main_tab == MainTab::Tasks && w.focus == Pane::Main);
                self.show_main(if on { MainTab::Editor } else { MainTab::Tasks });
            }
            Key::Ctrl('Y') => {
                let on = self.active_ws().is_some_and(|w| w.main_tab == MainTab::Notes && w.focus == Pane::Main && !w.notes.wants_text());
                self.show_main(if on { MainTab::Editor } else { MainTab::Notes });
            }
            Key::Ctrl('K') => {
                let on = self.active_ws().is_some_and(|w| w.main_tab == MainTab::K8s && w.focus == Pane::Main);
                self.show_main(if on { MainTab::Editor } else { MainTab::K8s });
            }
            Key::Ctrl('O') => {
                let on = self.active_ws().is_some_and(|w| w.main_tab == MainTab::Browser && w.focus == Pane::Main);
                self.show_main(if on { MainTab::Editor } else { MainTab::Browser });
            }
            Key::Ctrl('W') => self.modal = Some(Modal::Switcher { sel: 0 }),
            Key::Ctrl('E') => self.focus_pane(Pane::Tree),
            Key::Ctrl('D') => self.focus_pane(Pane::Main),
            Key::Ctrl('C') => self.focus_pane(Pane::Chat),
            Key::Ctrl('T') => self.show_bottom(BTab::Terminal),
            Key::Ctrl('A') => self.send_to_chat(),
            Key::Ctrl('R') => self.open_settings_form(),
            Key::Ctrl('N') => self.open_new_app_form(),
            // the IDE's own size: denser and looser (separate from Settings → Display)
            Key::Ctrl('=' | '+') => self.set_density(self.density.step(false)),
            Key::Ctrl('-' | '_') => self.set_density(self.density.step(true)),
            Key::F(5) => {
                if !self.run_tile_app(cx) {
                    self.run_current(cx)
                }
            }
            Key::Nav { nav: Nav::Left, shift: true, ctrl: true } => self.nudge(-24.0, true),
            Key::Nav { nav: Nav::Right, shift: true, ctrl: true } => self.nudge(24.0, true),
            Key::Nav { nav: Nav::Up, shift: true, ctrl: true } => self.nudge(24.0, false),
            Key::Nav { nav: Nav::Down, shift: true, ctrl: true } => self.nudge(-24.0, false),
            Key::Nav { nav: Nav::PageDown, ctrl: true, .. } if self.active_ws().is_some_and(|w| w.focus != Pane::Main) => {
                if let Some(w) = self.active_mut() {
                    w.editor.handle(&Event::Key(k.clone()), cx);
                    w.main_tab = MainTab::Editor;
                }
            }
            Key::Nav { nav: Nav::PageUp, ctrl: true, .. } if self.active_ws().is_some_and(|w| w.focus != Pane::Main) => {
                if let Some(w) = self.active_mut() {
                    w.editor.handle(&Event::Key(k.clone()), cx);
                    w.main_tab = MainTab::Editor;
                }
            }
            _ => return false,
        }
        self.redraw = true;
        true
    }

    /// Resize the focused pane from the keyboard (`horizontal`: the width of the side panes, else the bottom panel's height).
    fn nudge(&mut self, d: f32, horizontal: bool) {
        if let Some(w) = self.active_mut() {
            let pane = if horizontal {
                match w.focus {
                    Pane::Chat => Pane::Chat,
                    _ => Pane::Tree,
                }
            } else {
                Pane::Bottom
            };
            // growing the chat means moving its divider left
            let d = if pane == Pane::Chat { -d } else { d };
            w.prefs.nudge(pane, d);
        }
    }

    fn focused_key(&mut self, ev: &Event, k: &Key, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let ro = self.ws[i].ro.is_some();
        // "remember that we use pnpm", "forget the pnpm note": the harness changes the project context before any model is asked
        if matches!(k, Key::Enter) && !ro && self.ws[i].focus == Pane::Chat && self.try_context_intent(i, cx) {
            return;
        }
        let mut act = None;
        let mut tree_act = None;
        let mut open = None;
        let mut show_k8s = false;
        let mut tasks_act: Option<TasksAct> = None;
        {
            let w = &mut self.ws[i];
            match w.focus {
                Pane::Tree => tree_act = Some(w.tree.key(k)),
                Pane::Main => match w.main_tab {
                    MainTab::Editor => {
                        // a read-only workspace: only keys that read
                        let writes = ro && matches!(k, Key::Ctrl('s' | 'x' | 'v' | 'z' | 'y' | 'n' | 'l') | Key::Char(_) | Key::Enter | Key::Backspace | Key::Delete | Key::Tab | Key::F(6));
                        if !writes {
                            if matches!(k, Key::Enter) && false {
                                // placeholder for symmetry with the chat
                            }
                            let flow = w.editor.handle(ev, cx);
                            if matches!(k, Key::Ctrl('s')) {
                                act = Some(w.board.poll_disk());
                            }
                            // Esc with nothing to dismiss (no find bar, no suggestion) leaves the text field for the tree
                            if matches!(k, Key::Esc) && matches!(flow, Flow::Back) && w.prefs.sidebar_open {
                                w.set_focus(Pane::Tree);
                            }
                        } else {
                            w.say("Read-only: another session has this workspace open. Take it over (Ctrl+Shift+W) to edit.", false);
                        }
                    }
                    MainTab::Board => {
                        if ro && matches!(k, Key::Char('e' | 'b' | 'a' | 'x' | ']' | '[' | '+' | '-' | 'p' | 'P' | 'B' | 'T' | 'c') | Key::Ctrl('z') | Key::Delete) && !w.board.wants_text() {
                            w.say("Read-only: another session has this workspace open.", false);
                        } else {
                            act = Some(w.board.handle(ev));
                        }
                    }
                    MainTab::Tasks => {
                        let a = w.tasks.handle(ev);
                        if a == TasksAct::None && matches!(k, Key::Esc) {
                            w.set_focus(Pane::Tree);
                        }
                        tasks_act = Some(a);
                    }
                    MainTab::Notes => {
                        if matches!(k, Key::Char('z')) && !w.notes.wants_text() && w.ctx_undo.is_some() {
                            self.undo_context();
                            return;
                        }
                        let a = w.notes.handle(ev);
                        if a == NotesAct::Back {
                            w.set_focus(Pane::Tree);
                        }
                    }
                    MainTab::Browser => {
                        // the page and its address field own their keys; Esc (when nothing inside takes it) hands the keyboard back
                        let used = w.browser.as_mut().is_some_and(|b| super::component::Component::handle(b, ev, cx));
                        if !used && matches!(k, Key::Esc) {
                            w.set_focus(Pane::Tree);
                        }
                    }
                    MainTab::K8s => {
                        if !w.k8s_view().handle(ev) && matches!(k, Key::Esc) {
                            w.set_focus(Pane::Tree);
                        }
                    }
                },
                Pane::Chat => {
                    if matches!(k, Key::Enter) {
                        // what the editor shows goes with the question
                        let root = w.root();
                        let ctx = w.editor.path().map(|p| {
                            let sel = w.editor.selection_info();
                            iyakki_edit::proposal::edit_context(&workspace::rel_of(&root, p), w.editor.lang().name(), &w.editor.text(), sel.as_ref().map(|(a, b, t)| (*a, *b, t.as_str())))
                        });
                        w.chat.set_file_context(ctx);
                    }
                    // Esc with nothing to dismiss (no recording, no dialog) leaves the composer for the editor
                    if matches!(w.chat.handle(ev, cx), Flow::Back) && matches!(k, Key::Esc) {
                        w.set_focus(Pane::Main);
                    }
                }
                Pane::Bottom => match w.bottom_tab {
                    BTab::Terminal => {
                        if ro && matches!(k, Key::Enter) {
                            w.say("Read-only: another session has this workspace open.", false);
                        } else if !w.term.handle(ev) && matches!(k, Key::Esc) {
                            w.set_focus(Pane::Main);
                        }
                    }
                    // the status strip: Enter shows the K8s view; it has no table, so no letter does anything
                    BTab::K8s => match k {
                        Key::Enter => show_k8s = true,
                        Key::Esc => w.set_focus(Pane::Main),
                        _ => {}
                    },
                    BTab::Problems => {
                        if matches!(k, Key::Char('T')) {
                            // handled below
                        } else if let Some(loc) = w.problems.handle(ev) {
                            open = Some(loc);
                        } else if matches!(k, Key::Esc) {
                            w.set_focus(Pane::Main);
                        }
                    }
                    BTab::Search => {
                        if let Some(loc) = w.search.handle(ev) {
                            open = Some(loc);
                        } else if matches!(k, Key::Esc) && w.search.query.value.is_empty() {
                            w.set_focus(Pane::Main);
                        }
                    }
                },
            }
        }
        if let Some(a) = tree_act {
            self.tree_act(a, cx);
        }
        if let Some(a) = act {
            self.handle_board_act(a, cx);
        }
        if let Some(a) = tasks_act {
            self.tasks_act(a, cx);
        }
        if let Some((f, l)) = open {
            self.open_file(&f, Some(l), cx);
        }
        if show_k8s {
            self.show_main(MainTab::K8s);
        }
        if matches!(k, Key::Char('T')) && self.ws[i].focus == Pane::Bottom && self.ws[i].bottom_tab == BTab::Problems {
            self.run_tests(cx);
        }
        self.redraw = true;
    }

    fn pane_at(&self, x: i32, y: i32) -> Option<Pane> {
        let l = self.layout.as_ref()?;
        let inside = |r: &Option<R>| r.is_some_and(|r| r.contains(x, y));
        if inside(&l.chat) {
            Some(Pane::Chat)
        } else if inside(&l.bottom) {
            Some(Pane::Bottom)
        } else if inside(&l.sidebar) {
            Some(Pane::Tree)
        } else if l.main.contains(x, y) {
            Some(Pane::Main)
        } else {
            None
        }
    }

    /// The IDE's own clickable things.
    fn own_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        if self.top_overflow_open && !matches!(ev, Event::Tab(n) if *n == T_TOP + 9) {
            self.top_overflow_open = false;
        }
        match ev {
            Event::Tab(n) if (T_RAIL..T_RAIL + 500).contains(n) => {
                let order = self.reg.ordered();
                if let Some(w) = order.get(n - T_RAIL) {
                    if let Err(e) = self.open_by_id(&w.id, cx) {
                        self.notice = Some((e, false));
                    }
                }
            }
            Event::Tab(T_RAIL_ADD) => self.modal = Some(Modal::Switcher { sel: 0 }),
            Event::Tab(n) if (T_TOP..T_TOP + 100).contains(n) => match n - T_TOP {
                9 => self.top_overflow_open = !self.top_overflow_open,
                19 => self.open_clone_dialog(self.active.is_none(), cx),
                20 => self.open_repos_panel(cx),
                21 => self.open_connect_flow(None, cx),
                10 => {
                    let d = self.density;
                    self.set_density(if d == super::ide_chrome::Density::Dense { super::ide_chrome::Density::Comfortable } else { d.step(true) });
                }
                0 => self.modal = Some(Modal::Switcher { sel: 0 }),
                1 => self.open_settings_form(),
                2 => self.toggle_pane(Pane::Tree),
                3 => self.toggle_pane(Pane::Bottom),
                4 => self.toggle_pane(Pane::Chat),
                5 => {
                    if !self.run_tile_app(cx) {
                        self.run_current(cx)
                    }
                }
                6 => {
                    if !self.tile_tests_card() {
                        self.run_tests(cx)
                    }
                }
                13 => self.add_tile(cx),
                14 => self.open_publish(cx),
                15 => self.validate_app(),
                16 => self.open_target_form(),
                7 => self.open_quick(),
                8 => self.send_to_chat(),
                11 => self.show_main(MainTab::Tasks),
                17 => self.show_main(MainTab::Notes),
                18 => self.undo_context(),
                12 => self.open_new_app_form(),
                _ => {}
            },
            Event::Tab(n) if (T_MAIN..T_MAIN + 10).contains(n) => self.show_main(MainTab::ALL[(n - T_MAIN).min(MainTab::ALL.len() - 1)]),
            Event::Tab(n) if (T_BOTTOM..T_BOTTOM + 10).contains(n) => self.show_bottom(BTab::ALL[(n - T_BOTTOM).min(3)]),
            Event::Tab(n) if (T_PANE..T_PANE + 10).contains(n) => {
                let p = Pane::ALL[(n - T_PANE).min(3)];
                if let Some(w) = self.active_mut() {
                    w.compact_pane = p;
                    w.set_focus(p);
                }
            }
            Event::Row(r) if *r < 100_000 => {
                // a click on a tree row
                if let Some(w) = self.active_mut() {
                    w.set_focus(Pane::Tree);
                    w.tree.select(*r);
                    w.compact_pane = Pane::Tree;
                }
                if let Some(rel) = self.active_ws().and_then(|w| w.tree.selected().filter(|r| !r.is_dir).map(|r| r.rel.clone())) {
                    // one click on a folder expands it; a file opens on a double click (the Enter that follows)
                    let _ = rel;
                } else if let Some(w) = self.active_mut() {
                    w.tree.toggle(*r);
                }
            }
            Event::Sub(21, k) => {
                if let Some(w) = self.active_mut() {
                    w.set_focus(Pane::Tree);
                    let key = if matches!(k, Key::Char('R')) { None } else { Some(k.clone()) };
                    match key {
                        Some(k) => {
                            let a = w.tree.key(&k);
                            self.tree_act(a, cx);
                        }
                        None => w.tree.refresh(),
                    }
                }
            }
            Event::AreaAt(a, x, y) if (A_DIV..A_DIV + 10).contains(a) => {
                let which = [Div::Side, Div::Chat, Div::Bottom][(a - A_DIV).min(2)];
                let (area, s) = (self.area, self.scale);
                let rect = self.layout.as_ref().and_then(|l| l.dividers.iter().find(|d| d.0 == which).map(|d| d.1));
                if let (Some(r), Some(w)) = (rect, self.active_mut()) {
                    let origin = match &mut w.prefs {
                        _ if crate::ui::dragging() => None,
                        _ => Some(if which == Div::Bottom { r.y } else { r.x }),
                    };
                    let base = match origin {
                        Some(o) => {
                            self.drag_div = Some((which, o));
                            o
                        }
                        None => self.drag_div.map_or(0.0, |d| d.1),
                    };
                    let pos = if which == Div::Bottom { base + y } else { base + x };
                    // the pointer position is where the divider's centre should sit
                    if let Some(w) = self.active_mut() {
                        w.prefs.drag(which, pos, area, s);
                    }
                }
            }
            Event::Sub(SUB, k) => {
                let ev2 = Event::Key(k.clone());
                if let Event::Key(k) = &ev2 {
                    match k {
                        Key::Char('n') => self.open_new_workspace_form(cx),
                        Key::Char('o') => self.open_existing_form(),
                        Key::Char('w') => self.modal = Some(Modal::Switcher { sel: 0 }),
                        Key::Char('t') => {
                            if let Some(i) = self.active {
                                self.take_over(i, cx);
                            }
                        }
                        Key::Char('r') => {
                            if let Some(w) = self.active_mut() {
                                w.ro = workspace::lock_live(&w.root(), now_secs());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => return false,
        }
        self.redraw = true;
        true
    }
}

impl QuickOpen {
    pub fn refilter(&mut self) {
        let q = self.query.value.trim();
        let mut v: Vec<(i32, &String)> = self.all.iter().filter_map(|f| workspace::fuzzy(q, f).map(|s| (s, f))).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
        self.results = v.into_iter().take(60).map(|x| x.1.clone()).collect();
        self.sel = 0;
    }
}

/// Move a child app's click ids into its own range.
pub fn remap(ev: Event, pane: Pane) -> Event {
    let base = pane.code() as usize * STRIDE;
    match ev {
        Event::Tab(n) => Event::Tab(base + n),
        Event::Row(n) => Event::Row(base + n),
        Event::Focus(n) => Event::Focus(base + n),
        Event::Area(n) => Event::Area(base + n),
        Event::Key(k) => Event::Sub(10 + pane.code(), k),
        other => other,
    }
}

/// The child event behind a click id that `remap` moved, and the pane it belongs to.
pub fn decode(ev: &Event) -> Option<(Pane, Event)> {
    let split = |n: usize| -> Option<(Pane, usize)> {
        let p = n / STRIDE;
        if (1..=4).contains(&p) { Pane::from_code(p).map(|pane| (pane, n % STRIDE)) } else { None }
    };
    match ev {
        Event::Tab(n) => split(*n).map(|(p, m)| (p, Event::Tab(m))),
        Event::Row(n) => split(*n).map(|(p, m)| (p, Event::Row(m))),
        Event::FocusAt(n, x) => split(*n).map(|(p, m)| (p, Event::FocusAt(m, *x))),
        Event::AreaAt(n, x, y) => split(*n).map(|(p, m)| (p, Event::AreaAt(m, *x, *y))),
        Event::Sub(c, k) if (11..=14).contains(c) => Pane::from_code((*c - 10) as usize).map(|p| (p, Event::Key(k.clone()))),
        _ => None,
    }
}

/// Run a child's drawing and move the click regions it registered into the child's id range.
pub fn render_child(g: &mut Gfx, pane: Pane, f: impl FnOnce(&mut Gfx)) {
    // a marker tells where the child's regions start, even if the child cleared the list for a dialog of its own
    g.hit(R::new(-10.0, -10.0, 0.0, 0.0), Event::Tick, "ide-mark");
    f(g);
    let start = g.hits.iter().rposition(|h| h.label == "ide-mark").map_or(0, |i| i + 1);
    for h in g.hits[start..].iter_mut() {
        h.ev = remap(std::mem::replace(&mut h.ev, Event::Tick), pane);
    }
}

impl App for IdeApp {
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    /// RUNS sent the person back to a task or a row (`task:<id>`, `row:<id>`).
    fn open_item(&mut self, item: &str, _cx: &AppCx) {
        if let Some(t) = item.strip_prefix("task:") {
            if let Some(w) = self.active_mut() {
                w.tasks.select_id(t);
            }
            self.show_main(MainTab::Tasks);
        } else if let Some(r) = item.strip_prefix("row:") {
            if let Some(w) = self.active_mut() {
                w.board.select_id(r);
            }
            self.show_main(MainTab::Board);
        }
    }
    /// Files dropped on the window (window mode): into the tree folder under the pointer, else the workspace root.
    fn on_drop(&mut self, _paths: &[std::path::PathBuf], cx: &AppCx) -> super::DropAction {
        let (x, y) = self.mouse;
        let Some(w) = self.active_ws() else { return super::DropAction::Ignore("Open a workspace first, then drop files on its file tree.".into()) };
        if w.ro.is_some() {
            return super::DropAction::Ignore("This workspace is open read-only; take it over to add files.".into());
        }
        let rel = match w.tree.row_at(x, y).and_then(|i| w.tree.rows.get(i)) {
            Some(r) if r.is_dir => r.rel.clone(),
            Some(r) => r.rel.rsplit_once('/').map_or(String::new(), |p| p.0.to_string()),
            None => String::new(),
        };
        let dest = w.root().join(&rel);
        if !cx.session.policy.allows_write(&cx.session.user, &dest.to_string_lossy()) {
            return super::DropAction::Ignore("Your account may not write in this workspace folder.".into());
        }
        let label = if rel.is_empty() { "the workspace folder".to_string() } else { format!("the folder {rel}") };
        super::DropAction::Into { dest, label }
    }

    /// What a drop brought in: the first file opens (a table in DATA, anything else in the editor).
    fn dropped_done(&mut self, files: &[std::path::PathBuf], cx: &AppCx) {
        let Some(root) = self.active_ws().map(|w| w.root()) else { return };
        if let Some(rel) = files.first().and_then(|f| f.strip_prefix(&root).ok()).map(|r| r.to_string_lossy().into_owned()) {
            self.open_file(&rel, None, cx);
        }
    }
    fn title(&self) -> &'static str {
        "AI IDE"
    }
    fn icon(&self) -> Icon {
        Icon::Ide
    }
    fn accent(&self) -> [u8; 4] {
        self.active_ws().map_or(ACCENT, |w| w.meta.color())
    }
    fn hint(&self) -> String {
        "Ctrl+P open file · Ctrl+Shift+F search · Ctrl+B tree · Ctrl+J panel · Ctrl+Shift+B board · Ctrl+Shift+W workspaces · F1 all keys".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "Ctrl+Shift+W  workspaces: switch, new, open a folder",
            "Ctrl+P  open a file by name",
            "Ctrl+Shift+F  search the text of the project",
            "Ctrl+B  show / hide the file tree",
            "Ctrl+J  show / hide the bottom panel",
            "Ctrl+Shift+E / D / C / T  focus the tree / editor / chat / terminal",
            "Ctrl+Shift+B  feature board    Ctrl+Shift+O  browser (the app you deployed)    Ctrl+Shift+K  cluster view",
            "Ctrl+Shift+A  send the selection (or file) to the chat",
            "Ctrl+Shift+←→↑↓  resize the focused pane",
            "F5  run the file (scripts) or the project's run command",
            "Ctrl+PageDown / PageUp  next / previous editor tab",
            "Ctrl+Shift+R  project settings (namespace, test command…)",
            "Ctrl+=  Ctrl+-  looser / denser IDE (Comfortable, Compact, Dense)",
            "Tree: n new file · N new folder · F2 rename · Delete delete · / find · h show hidden folders",
            "Board: Tab view · [ ] move · e comment · b blocker · a add · x delete · i ask AI · Ctrl+Z undo",
            "Cluster: Tab kind · Enter details · l logs · r refresh · n namespace · o open the selected Service in the Browser tab",
            "Browser: Ctrl+L address · Esc hands the keyboard back · ← → ⟳ buttons · Reader / Full · ↗ open in the Browser tile; the assistant's clicks show with a banner (Stop, Take over)",
            "A blinking caret means typing goes there: letters are shortcuts only in the tree, the cluster table, the board and the problems list; Esc leaves a text field",
            "Drag the dividers between panes with the mouse",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn wants_redraw(&mut self) -> bool {
        // the chat pane's Jobs strip moves while a job of this workspace runs
        let jobs = self.active_mut().is_some_and(|w| w.chat.jobs_frame_due());
        std::mem::take(&mut self.redraw) || jobs
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let scoped = self.scoped_cx(cx);
        let cx = &scoped;
        match ev {
            Event::Tick => {
                self.on_tick(cx);
                if self.platform.is_some() {
                    self.platform_event(ev, cx);
                }
                return Flow::Stay;
            }
            Event::MouseMove(x, y) if self.platform.is_none() => {
                self.mouse = (*x, *y);
                // the page under the pointer sees it (hover, the cursor shape, the end of a drag)
                if let Some(w) = self.active.and_then(|i| self.ws.get_mut(i)).filter(|w| w.main_tab == MainTab::Browser) {
                    if let Some(b) = w.browser.as_mut().filter(|b| b.page_rect.contains(*x, *y)) {
                        b.pointer(ev, cx);
                    }
                }
                return Flow::Stay;
            }
            _ => {}
        }
        self.redraw = true;
        if self.platform.is_some() {
            self.platform_event(ev, cx);
            return Flow::Stay;
        }
        if self.modal.is_some() {
            self.modal_event(ev, cx);
            return Flow::Stay;
        }
        // focus follows a click: a press on the bare background of a pane (controls and fields send their own events, which
        // move the focus too) gives that pane the keyboard and takes the caret out of every other input
        if let Event::MouseDown(x, y) = ev {
            if let Some(p) = self.pane_at(*x, *y) {
                if let Some(w) = self.active_mut() {
                    w.set_focus(p);
                    w.compact_pane = p;
                }
            }
            return Flow::Stay;
        }
        if let Event::MouseUp(x, y) = ev {
            self.drag_div = None;
            if let Some(w) = self.active.and_then(|i| self.ws.get_mut(i)).filter(|w| w.main_tab == MainTab::Browser) {
                if let Some(b) = w.browser.as_mut() {
                    b.pointer(ev, cx);
                }
            }
            if let Some(w) = self.active_mut() {
                let a = w.board.handle(ev);
                let _ = (x, y);
                if a != BoardAct::None {
                    self.handle_board_act(a, cx);
                }
            }
            return Flow::Stay;
        }
        // keys the IDE owns
        if let Event::Key(k) = ev {
            if self.notice.is_some() {
                self.notice = None;
            }
            if self.active.is_none() {
                match k {
                    Key::Char('n') | Key::Ctrl('n') => self.open_new_workspace_form(cx),
                    Key::Char('o') | Key::Ctrl('o') => self.open_existing_form(),
                    Key::Ctrl('W') | Key::Char('w') | Key::Enter => self.modal = Some(Modal::Switcher { sel: 0 }),
                    _ => {}
                }
                return Flow::Stay;
            }
            let editing = self.active_ws().is_some_and(|w| w.focus == Pane::Main && w.main_tab == MainTab::Board && w.board.has_modal());
            if !editing && self.global_key(k, cx) {
                return Flow::Stay;
            }
            if let Some(w) = self.active_mut() {
                w.status = None;
            }
            self.focused_key(ev, k, cx);
            return Flow::Stay;
        }
        if self.active.is_none() {
            self.own_event(ev, cx);
            return Flow::Stay;
        }
        // a dialog of the board or of the cluster view (their forms and pickers click with small ids) takes its own clicks
        if let Some(i) = self.active {
            let w = &mut self.ws[i];
            let from_dialog = matches!(ev, Event::Tab(_) | Event::Row(_) | Event::FocusAt(..) | Event::Paste(_) | Event::Sub(..) | Event::Scroll(_));
            if from_dialog && w.board.has_modal() && w.main_tab == MainTab::Board {
                let a = w.board.handle(ev);
                self.handle_board_act(a, cx);
                return Flow::Stay;
            }
            if from_dialog && w.k8s.as_ref().is_some_and(|k| k.has_modal()) {
                w.set_focus(Pane::Main);
                w.k8s_view().handle(ev);
                return Flow::Stay;
            }
        }
        if self.own_event(ev, cx) {
            return Flow::Stay;
        }
        // clicks that belong to the embedded browser
        if let Some(inner) = super::component::route::decode_slot(ev, super::component::route::BROWSER) {
            self.browser_route(inner, cx);
            return Flow::Stay;
        }
        // clicks and typing that belong to a child app (the editor or the chat)
        if let Some((pane, inner)) = decode(ev) {
            if let Some(i) = self.active {
                let w = &mut self.ws[i];
                w.set_focus(pane);
                w.compact_pane = pane;
                let ro = w.ro.is_some();
                match pane {
                    Pane::Main => {
                        if !(ro && matches!(inner, Event::Key(_))) {
                            w.editor.handle(&inner, cx);
                        }
                    }
                    Pane::Chat => {
                        w.chat.handle(&inner, cx);
                    }
                    _ => {}
                }
            }
            return Flow::Stay;
        }
        // text fields and areas the IDE draws itself
        let Some(i) = self.active else { return Flow::Stay };
        let mut open = None;
        let mut act = None;
        let mut tasks_act: Option<TasksAct> = None;
        {
            let w = &mut self.ws[i];
            match ev {
                Event::FocusAt(super::ide_bottom::FOCUS_TERM, _) => {
                    w.set_focus(Pane::Bottom);
                    w.bottom_tab = BTab::Terminal;
                    w.term.handle(ev);
                }
                Event::FocusAt(super::ide_bottom::FOCUS_SEARCH, _) => {
                    w.set_focus(Pane::Bottom);
                    w.bottom_tab = BTab::Search;
                    w.search.handle(ev);
                }
                Event::FocusAt(super::board::FOCUS_FILTER, _) | Event::Tab(super::board::TAB_BASE..=9_419_999) | Event::AreaAt(super::board::CARD_BASE..=9_429_999, ..) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Board;
                    act = Some(w.board.handle(ev));
                }
                Event::Row(r) if (super::board::ROW_BASE..super::board::ROW_BASE + 100_000).contains(r) => {
                    w.set_focus(Pane::Main);
                    act = Some(w.board.handle(ev));
                }
                Event::Row(r) if (super::ide_k8s::ROW_BASE..super::ide_k8s::ROW_BASE + 100_000).contains(r) => {
                    w.set_focus(Pane::Main);
                    w.k8s_view().handle(ev);
                }
                Event::Tab(n) if (super::ide_k8s::TAB_BASE..super::ide_k8s::NS_BASE + 1000).contains(n) || (super::ide_k8s::SWITCH_BASE..super::ide_k8s::SWITCH_BASE + 1000).contains(n) => {
                    w.set_focus(Pane::Main);
                    w.k8s_view().handle(ev);
                }
                Event::Sub(super::ide_k8s::SUB, _) | Event::AreaAt(super::ide_k8s::DETAIL_AREA | super::ide_k8s::FLOW_AREA, ..) => {
                    w.set_focus(Pane::Main);
                    w.k8s_view().handle(ev);
                }
                Event::Row(r) if (super::ide_tasks::ROW_BASE..super::ide_tasks::ROW_BASE + 1000).contains(r) || (super::ide_tasks::FILE_BASE..super::ide_tasks::FILE_BASE + 5000).contains(r) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Tasks;
                    tasks_act = Some(w.tasks.handle(ev));
                }
                Event::Tab(n) if (super::ide_tasks::TAB_BASE..super::ide_tasks::TAB_BASE + 10).contains(n) || (super::ide_tasks::APPROVE_BASE..super::ide_tasks::APPROVE_BASE + 100).contains(n) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Tasks;
                    tasks_act = Some(w.tasks.handle(ev));
                }
                Event::Sub(super::ide_tasks::SUB, _) | Event::AreaAt(super::ide_tasks::DETAIL_AREA, ..) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Tasks;
                    tasks_act = Some(w.tasks.handle(ev));
                }
                Event::Row(r) if (super::ide_notes::ROW_BASE..super::ide_notes::ROW_BASE + 100_000).contains(r) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Notes;
                    w.notes.handle(ev);
                }
                Event::FocusAt(super::ide_notes::FOCUS_SEARCH | super::ide_notes::FOCUS_TITLE | super::ide_notes::FOCUS_SOURCE | super::ide_notes::TEXT_AREA, _) | Event::AreaAt(super::ide_notes::TEXT_AREA, ..) | Event::Sub(super::ide_notes::SUB, _) => {
                    w.set_focus(Pane::Main);
                    w.main_tab = MainTab::Notes;
                    w.notes.handle(ev);
                }
                Event::Sub(super::board::SUB, _) => {
                    w.set_focus(Pane::Main);
                    act = Some(w.board.handle(ev));
                }
                Event::Sub(super::ide_bottom::SUB, Key::Char('T')) => {
                    w.set_focus(Pane::Bottom);
                    self.run_tests(cx);
                }
                // the bottom panel's cluster strip: "Open K8s view"
                Event::Sub(super::ide_bottom::SUB, Key::Char('K')) => {
                    self.show_main(MainTab::K8s);
                }
                Event::Row(r) if (super::ide_bottom::PROBLEM_ROW..super::ide_bottom::PROBLEM_ROW + 100_000).contains(r) => {
                    w.set_focus(Pane::Bottom);
                    open = w.problems.handle(ev);
                }
                Event::Row(r) if (super::ide_bottom::SEARCH_ROW..super::ide_bottom::SEARCH_ROW + 100_000).contains(r) => {
                    w.set_focus(Pane::Bottom);
                    open = w.search.handle(ev);
                }
                Event::AreaAt(super::ide_bottom::TERM_AREA, ..) => {
                    w.set_focus(Pane::Bottom);
                    w.term.handle(ev);
                }
                Event::AreaAt(super::board::DRAWER_AREA, ..) => {
                    w.set_focus(Pane::Main);
                    act = Some(w.board.handle(ev));
                }
                Event::Paste(_) => {
                    let ev = ev.clone();
                    match w.focus {
                        Pane::Main => match w.main_tab {
                            MainTab::Editor => {
                                if !w.ro.is_some() {
                                    w.editor.handle(&ev, cx);
                                }
                            }
                            MainTab::Board => act = Some(w.board.handle(&ev)),
                            MainTab::Browser => {
                                // pasted text goes to the address field or to the page's focused input, never to another pane
                                if let (Some(b), Event::Paste(t)) = (w.browser.as_mut(), &ev) {
                                    b.paste(t, cx);
                                }
                            }
                            MainTab::Notes => {
                                w.notes.handle(&ev);
                            }
                            MainTab::K8s | MainTab::Tasks => {}
                        },
                        Pane::Chat => {
                            w.chat.handle(&ev, cx);
                        }
                        Pane::Bottom => match w.bottom_tab {
                            BTab::Terminal => {
                                w.term.handle(&ev);
                            }
                            BTab::Search => {
                                w.search.handle(&ev);
                            }
                            _ => {}
                        },
                        Pane::Tree => {}
                    }
                }
                Event::Scroll(d) => {
                    let (mx, my) = self.mouse;
                    let pane = self.layout.as_ref().and_then(|_| None::<Pane>).or_else(|| {
                        let l = self.layout.as_ref()?;
                        let inside = |r: &Option<R>| r.is_some_and(|r| r.contains(mx, my));
                        if inside(&l.chat) { Some(Pane::Chat) } else if inside(&l.bottom) { Some(Pane::Bottom) } else if inside(&l.sidebar) { Some(Pane::Tree) } else if l.main.contains(mx, my) { Some(Pane::Main) } else { None }
                    });
                    let w = &mut self.ws[i];
                    match pane.unwrap_or(w.focus) {
                        Pane::Tree => w.tree.scroll_by(*d),
                        Pane::Chat => {
                            w.chat.handle(ev, cx);
                        }
                        Pane::Main => match w.main_tab {
                            MainTab::Editor => {
                                w.editor.handle(ev, cx);
                            }
                            MainTab::Board => act = Some(w.board.handle(ev)),
                            MainTab::Browser => {
                                if let (Some(b), Event::Scroll(d)) = (w.browser.as_mut(), ev) {
                                    b.scroll(*d, cx);
                                }
                            }
                            MainTab::K8s => {
                                w.k8s_view().handle(ev);
                            }
                            MainTab::Tasks => tasks_act = Some(w.tasks.handle(ev)),
                            MainTab::Notes => {
                                w.notes.handle(ev);
                            }
                        },
                        Pane::Bottom => match w.bottom_tab {
                            BTab::Terminal => {
                                w.term.handle(ev);
                            }
                            BTab::K8s => {}
                            BTab::Problems => {
                                w.problems.handle(ev);
                            }
                            BTab::Search => {
                                w.search.handle(ev);
                            }
                        },
                    }
                }
                other => {
                    if w.main_tab == MainTab::Board && w.focus == Pane::Main {
                        act = Some(w.board.handle(other));
                    }
                    if w.main_tab == MainTab::Tasks && w.focus == Pane::Main {
                        tasks_act = Some(w.tasks.handle(other));
                    }
                }
            }
        }
        if let Some(a) = act {
            self.handle_board_act(a, cx);
        }
        if let Some(a) = tasks_act {
            self.tasks_act(a, cx);
        }
        if let Some((f, l)) = open {
            self.open_file(&f, Some(l), cx);
        }
        Flow::Stay
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        use crate::ctxmenu::Item;
        let Some(pane) = self.pane_at(x, y) else { return vec![] };
        let Some(w) = self.active.and_then(|i| self.ws.get_mut(i)) else { return vec![] };
        let k8s_items = if matches!(pane, Pane::Main | Pane::Bottom) { w.k8s.as_ref().map(|k| k.context_items(x, y)).unwrap_or_default() } else { Vec::new() };
        self.ctx_k8s = !k8s_items.is_empty();
        let task_items = if pane == Pane::Main && w.main_tab == MainTab::Tasks { w.tasks.context_items(x, y) } else { Vec::new() };
        self.ctx_tasks = !task_items.is_empty();
        if self.ctx_tasks {
            return task_items;
        }
        if pane == Pane::Main && w.main_tab == MainTab::Board {
            let has = w.board.selected().is_some();
            return vec![Item::new("task.row", "Run selected row as a task", "").enabled(has), Item::new("task.rows", "Run all planned rows in parallel", ""), Item::separator(), Item::new("ide.sendchat", "Send selection to the chat", "Ctrl+Shift+A"), Item::new("ide.quick", "Open file…", "Ctrl+P")];
        }
        if self.ctx_k8s {
            let mut v = k8s_items;
            v.push(Item::separator());
            v.push(Item::new("ide.quick", "Open file…", "Ctrl+P"));
            return v;
        }
        match pane {
            Pane::Tree => {
                let row = w.tree.row_at(x, y);
                self.ctx_row = row;
                let has = row.is_some();
                vec![Item::new("tree.new", "New file", "n"), Item::new("tree.newdir", "New folder", "N"), Item::separator(), Item::new("tree.open", "Open", "Enter").enabled(has), Item::new("tree.rename", "Rename", "F2").enabled(has), Item::new("tree.delete", "Delete…", "Del").enabled(has), Item::separator(), Item::new("tree.share", "Share this folder…", ""), Item::separator(), Item::new("tree.refresh", "Refresh", ""), Item::new("tree.heavy", "Show hidden folders", "h")]
            }
            Pane::Main | Pane::Chat | Pane::Bottom => {
                let mut v = vec![Item::new("ide.sendchat", "Send selection to the chat", "Ctrl+Shift+A"), Item::new("ide.quick", "Open file…", "Ctrl+P"), Item::new("ide.search", "Search the project…", "Ctrl+Shift+F")];
                if pane == Pane::Chat {
                    // a link under the pointer: its own entries first
                    let mut l = w.chat.link_menu_items(x, y);
                    l.append(&mut v);
                    return l;
                }
                if pane == Pane::Main {
                    let mut e = w.editor.context_items(x, y);
                    e.push(Item::separator());
                    e.append(&mut v);
                    return e;
                }
                v
            }
        }
    }

    fn context_pick(&mut self, id: &str, cx: &AppCx) -> Flow {
        let Some(i) = self.active else { return Flow::Stay };
        if id.starts_with("link.") && self.ws[i].chat.link_menu_pick(id, cx) {
            return Flow::Stay;
        }
        if let Some(row) = self.ctx_row {
            if id.starts_with("tree.") {
                self.ws[i].tree.select(row);
            }
        }
        if self.ctx_k8s && matches!(id, "copy" | "selall" | "k8s.copyall") {
            if let Some(k) = self.ws[i].k8s.as_mut() {
                k.context_pick(id);
            }
            self.redraw = true;
            return Flow::Stay;
        }
        if self.ctx_tasks && matches!(id, "copy" | "selall" | "tasks.copyall") {
            if let Some(w) = self.ws.get_mut(i) {
                w.tasks.context_pick(id);
            }
            self.redraw = true;
            return Flow::Stay;
        }
        match id {
            "task.row" => self.run_rows_as_tasks(false, cx),
            "task.rows" => self.run_rows_as_tasks(true, cx),
            "tree.new" => self.tree_act(TreeAct::NewFile(self.ws[i].tree.target_dir()), cx),
            "tree.newdir" => self.tree_act(TreeAct::NewDir(self.ws[i].tree.target_dir()), cx),
            "tree.open" => {
                let a = self.ws[i].tree.key(&Key::Enter);
                self.tree_act(a, cx);
            }
            "tree.rename" => {
                let a = self.ws[i].tree.key(&Key::F(2));
                self.tree_act(a, cx);
            }
            "tree.delete" => {
                let a = self.ws[i].tree.key(&Key::Delete);
                self.tree_act(a, cx);
            }
            "tree.share" => {
                // Settings -> Shared folders opens its form with this folder filled in (the person decides there)
                let abs = self.ws[i].root().join(self.ws[i].tree.target_dir());
                self.request = Some(("settings".into(), format!("share:{}", abs.display())));
            }
            "tree.refresh" => self.ws[i].tree.refresh(),
            "tree.heavy" => {
                self.ws[i].tree.key(&Key::Char('h'));
            }
            "ide.sendchat" => self.send_to_chat(),
            "ide.quick" => self.open_quick(),
            "ide.search" => self.open_search(),
            "copy" | "cut" | "paste" | "selall" => {
                let key = Key::Ctrl(match id {
                    "copy" => 'c',
                    "cut" => 'x',
                    "paste" => 'v',
                    _ => 'a',
                });
                self.handle(&Event::Key(key), cx);
            }
            other => {
                let ws = &mut self.ws[i];
                ws.editor.context_pick(other, cx);
            }
        }
        self.redraw = true;
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let scoped = self.scoped_cx(cx);
        self.render_ide(g, area, s, &scoped, tick);
        if let Some(p) = self.platform.as_mut() {
            g.clear_hits();
            crate::fx::glass(g, area.x, area.y, area.w, area.h, 0.0, [120, 170, 255, 255]);
            p.draw(g, R::new(area.x + 8.0 * s, area.y + 8.0 * s, area.w - 16.0 * s, area.h - 16.0 * s), s);
        }
    }
}

impl Drop for IdeApp {
    fn drop(&mut self) {
        // the layouts and tabs are saved and the locks given back when the shell closes the app
        self.save_all();
        for w in &self.ws {
            w.tasks.mgr.shutdown();
            workspace::release_lock(&w.root(), &self.session);
        }
    }
}
