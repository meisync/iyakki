//! K8S: a tile to pick a cluster and a namespace and explore it (docs/K8S-TILE.md). The managed dev cluster and the person's saved
//! kubeconfig profiles are the clusters; namespaces, resource tables, details (summary, YAML, events, related objects, pod logs) and,
//! behind an administrator's "Allow changes" switch, a few careful actions.
//!
//! The engine is `iyakki_agent::k8sexp` (one `kubectl get ... -o json` per list, bounded rows and logs, Secret values masked, every
//! action an argument array for the managed kubectl); the cluster views of the AI IDE share its `Kube` runner, its failure texts
//! (`ide_k8s::banner_for`) and its parsing helpers. This file is the state and the keys; `k8s_tile_ui.rs` draws it.
//!
//! Nothing here calls `kubectl` while the tile is not on screen, and nothing blocks the screen: lists come from a feed thread, details,
//! logs and actions run on short threads whose answers are picked up on the heartbeat.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use iyakki_agent::k3sdev::Svc;
use iyakki_agent::k8s::{EventRow, Kube, Target};
use iyakki_agent::k8sexp::{self as kx, Action, Confirm, ExpFeed, Kind, LogBuf, LogLine, LogReq, Prefs, Rel, Scope, Want};
use iyakki_sec::policy::Role;

use super::cluster_host::{ClusterHost, HostOut};
use super::{App, AppCx, Flow};
use crate::safeclamp::SafeClamp;
use crate::gfx::Icon;
use crate::ui::{clipboard, Event, Key, Nav, TextField, R};

pub const ACCENT: [u8; 4] = [64, 196, 232, 255];

// click codes (Event::Tab)
pub(super) const T_CLUSTER: usize = 1;
pub(super) const T_NS_BTN: usize = 2;
pub(super) const T_KIND_BTN: usize = 3;
pub(super) const T_REFRESH: usize = 4;
pub(super) const T_ALLOW: usize = 5;
pub(super) const T_BACK: usize = 6;
pub(super) const T_START: usize = 7;
pub(super) const T_SETTINGS: usize = 8;
pub(super) const T_CLOSE: usize = 9;
pub(super) const T_TAB: usize = 10; // 10..15: Summary, YAML, Events, Related, Logs
pub(super) const T_COPY: usize = 20;
pub(super) const T_REVEAL: usize = 21;
pub(super) const T_EXEC: usize = 22;
pub(super) const T_SAVE: usize = 23;
pub(super) const T_WRAP: usize = 24;
pub(super) const T_FOLLOW: usize = 25;
pub(super) const T_PAUSE: usize = 26;
pub(super) const T_PREV: usize = 27;
pub(super) const T_TAIL: usize = 28;
pub(super) const T_IDE: usize = 29;
pub(super) const T_LOG: usize = 30; // Show log / Hide log
pub(super) const T_LOG_COPY: usize = 31;
pub(super) const T_ACT: usize = 40; // 40..: restart, scale, delete pod, cordon, uncordon, delete
pub(super) const T_YES: usize = 60;
pub(super) const T_NO: usize = 61;
pub(super) const T_SCALE_DN: usize = 62;
pub(super) const T_SCALE_UP: usize = 63;
pub(super) const T_PANEL_CLOSE: usize = 64;
pub(super) const T_CLEAR_FILTER: usize = 65;
pub(super) const T_REVEAL_YES: usize = 66;
pub(super) const T_CONT: usize = 100; // 100..199: container chips
pub(super) const T_CLUSTER_ROW: usize = 1_000;
/// The cluster switcher of the header owns the codes from here (its list rows follow).
pub(super) const T_SWITCH: usize = 900_000;
pub(super) const T_SORT: usize = 2_000;
pub(super) const T_REL_ROW: usize = 3_000;
pub(super) const T_NS_ROW: usize = 100_000;
pub(super) const T_KIND_ROW: usize = 200_000;
pub(super) const T_RES_ROW: usize = 300_000;

// text fields
pub(super) const F_FILTER: usize = 63_001;
pub(super) const F_NS: usize = 63_002;
pub(super) const F_LOGS: usize = 63_003;
pub(super) const F_NAME: usize = 63_004;

/// Rows a wheel notch moves.
const WHEEL_ROWS: i32 = 3;
const SETTLE: Duration = Duration::from_millis(200);
const FOLLOW_EVERY: Duration = Duration::from_secs(2);

// ------------------------------------------------------------------------------------------------ the rig

/// The managed dev cluster as the tile sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Managed {
    /// Nothing was set up on this computer (Settings -> Dev cluster does that).
    NotSet,
    Stopped,
    Starting { waiting_net: bool },
    Running,
    Stopping,
    Failed(String),
}

impl Managed {
    pub fn chip(&self) -> &'static str {
        match self {
            Managed::NotSet => "Not set up",
            Managed::Stopped => "Stopped",
            Managed::Starting { waiting_net: true } => "Waiting for the network",
            Managed::Starting { .. } => "Starting",
            Managed::Running => "Running",
            Managed::Stopping => "Stopping",
            Managed::Failed(_) => "Failed",
        }
    }
}

/// One cluster in the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterEntry {
    /// The key the person's choices are kept under (`managed`, `profile:<name>`).
    pub key: String,
    pub label: String,
    pub managed: bool,
    pub target: Target,
    /// The registry entry this row stands for (`clusters::Registry`): its name and kind go into every confirmation and audit line.
    pub reg: iyakki_agent::clusters::ClusterEntry,
}

/// What the tile uses that tests replace: the cluster client of an entry, the managed cluster's state and its Start.
#[derive(Clone)]
pub struct Rig {
    pub kube_for: Arc<dyn Fn(&ClusterEntry) -> Arc<dyn Kube> + Send + Sync>,
    /// The managed cluster's state and the kubeconfig file it uses (none for the Podman backend).
    pub managed: Arc<dyn Fn() -> (Managed, Option<PathBuf>) + Send + Sync>,
    pub start: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
}

impl Rig {
    pub fn system(cx: &AppCx) -> Rig {
        if cfg!(test) {
            return Rig::offline();
        }
        let svc = cx.services.k3sdev.clone();
        let (s1, s2) = (svc.clone(), svc.clone());
        Rig {
            kube_for: Arc::new(move |e: &ClusterEntry| -> Arc<dyn Kube> {
                let run: Arc<dyn iyakki_agent::wsexec::Runner> = Arc::new(iyakki_agent::wsexec::System);
                if e.managed && s1.installed().is_some_and(|s| s.backend == iyakki_agent::k3sdev::Backend::Podman) {
                    Arc::new(iyakki_agent::devcluster::PodKube { run, container: iyakki_agent::k3sdev::POD_NAME.into() })
                } else {
                    Arc::new(iyakki_agent::devcluster::RunnerKube { run })
                }
            }),
            managed: Arc::new(move || {
                let h = svc.health();
                let m = match svc.state() {
                    Svc::NotInstalled => Managed::NotSet,
                    Svc::Stopped => Managed::Stopped,
                    Svc::Starting => Managed::Starting { waiting_net: !h.note.is_empty() },
                    Svc::Running => Managed::Running,
                    Svc::Stopping => Managed::Stopping,
                    Svc::Failed(r) => Managed::Failed(r),
                };
                let native = svc.installed().is_some_and(|s| s.backend == iyakki_agent::k3sdev::Backend::Native);
                (m, native.then(|| svc.layout().kubeconfig()))
            }),
            start: Arc::new(move |user: &str| s2.start(user)),
        }
    }

    /// No cluster at all: kubectl "is missing" and the managed cluster is not set up. What the shell's own tests get.
    pub fn offline() -> Rig {
        Rig {
            kube_for: Arc::new(|_| {
                let k = iyakki_agent::k8s::FakeKube::new();
                k.missing();
                Arc::new(k)
            }),
            managed: Arc::new(|| (Managed::NotSet, None)),
            start: Arc::new(|_| Err("no dev cluster is set up here".into())),
        }
    }
}

/// The cluster client the feed and the detail jobs use; the person's choice of cluster swaps what is inside.
pub(super) struct SwitchKube {
    cur: Mutex<Arc<dyn Kube>>,
}

impl SwitchKube {
    fn get(&self) -> Arc<dyn Kube> {
        self.cur.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn set(&self, k: Arc<dyn Kube>) {
        *self.cur.lock().unwrap_or_else(|e| e.into_inner()) = k;
    }
}

impl Kube for SwitchKube {
    fn run(&self, args: &[String]) -> iyakki_agent::wsexec::Out {
        self.get().run(args)
    }
    fn run_with(&self, args: &[String], secs: u64) -> iyakki_agent::wsexec::Out {
        self.get().run_with(args, secs)
    }
}

// ------------------------------------------------------------------------------------------------ jobs

pub(super) struct Job<T>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Job<T> {
    pub fn spawn(f: impl FnOnce() -> T + Send + 'static) -> Job<T> {
        let slot = Arc::new(Mutex::new(None));
        let s2 = slot.clone();
        std::thread::spawn(move || {
            let v = f();
            *s2.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        });
        Job(slot)
    }
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

// ------------------------------------------------------------------------------------------------ text view

/// One line of a scrolling text: the text, the colour of its value part, and how many characters at the start are a key.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Line {
    pub text: String,
    pub color: [u8; 4],
    pub key_len: usize,
}

impl Line {
    pub fn plain(text: impl Into<String>, color: [u8; 4]) -> Line {
        Line { text: text.into(), color, key_len: 0 }
    }
}

#[derive(Default)]
pub(super) struct TextView {
    pub lines: Vec<Line>,
    /// The first line in view.
    pub top: usize,
    pub wrap: bool,
    /// Keep the end in view (follow mode).
    pub stick: bool,
    /// Lines that fit, as drawn last.
    pub fit: usize,
    /// The area as drawn last.
    pub rect: R,
}

impl TextView {
    pub fn set(&mut self, lines: Vec<Line>, keep_top: bool) {
        if !keep_top {
            self.top = 0;
        }
        self.lines = lines;
        self.top = self.top.min(self.lines.len().saturating_sub(1));
    }
    pub fn scroll(&mut self, rows: i32) {
        let max = self.lines.len().saturating_sub(self.fit.max(1)) as i32;
        let cur = if self.stick { max } else { self.top as i32 };
        let now = (cur + rows).clamp_safe(0, max.max(0));
        self.stick = false;
        self.top = now as usize;
        if now >= max && max > 0 && rows > 0 {
            // scrolling down to the end re-attaches follow mode (the log view uses this)
            self.stick = true;
        }
    }
    pub fn home(&mut self) {
        self.stick = false;
        self.top = 0;
    }
    pub fn end(&mut self) {
        self.stick = true;
        self.top = self.lines.len().saturating_sub(self.fit.max(1));
    }
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

// ------------------------------------------------------------------------------------------------ lists

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ListState {
    pub top: usize,
    pub sel: usize,
    /// The selection moved by key: show it at the next draw.
    pub reveal: bool,
    /// Rows that fit, as drawn last.
    pub fit: usize,
    pub rect: R,
}

impl ListState {
    pub fn up(&mut self, n: usize, by: usize) {
        self.sel = self.sel.saturating_sub(by).min(n.saturating_sub(1));
        self.reveal = true;
    }
    pub fn down(&mut self, n: usize, by: usize) {
        self.sel = (self.sel + by).min(n.saturating_sub(1));
        self.reveal = true;
    }
    pub fn page(&self) -> usize {
        self.fit.saturating_sub(1).max(1)
    }
    pub fn scroll(&mut self, n: usize, rows: i32) {
        let max = n.saturating_sub(self.fit.max(1)) as i32;
        self.top = (self.top as i32 + rows).clamp_safe(0, max.max(0)) as usize;
        self.reveal = false;
    }
    pub fn reset(&mut self) {
        self.top = 0;
        self.sel = 0;
        self.reveal = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    Ns,
    Kinds,
    Res,
    Detail,
    Filter,
    NsFilter,
    LogSearch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
    Clusters,
    Namespaces,
    Kinds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DTab {
    Summary,
    Yaml,
    Events,
    Related,
    Logs,
}

impl DTab {
    pub const ALL: [DTab; 5] = [DTab::Summary, DTab::Yaml, DTab::Events, DTab::Related, DTab::Logs];
    pub fn label(self) -> &'static str {
        match self {
            DTab::Summary => "Summary",
            DTab::Yaml => "YAML",
            DTab::Events => "Events",
            DTab::Related => "Related",
            DTab::Logs => "Logs",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Wide,
    Medium,
    Phone,
}

impl Mode {
    pub fn of(width: f32, s: f32) -> Mode {
        let w = width / s.max(0.5);
        if w >= 1000.0 {
            Mode::Wide
        } else if w >= 620.0 {
            Mode::Medium
        } else {
            Mode::Phone
        }
    }
}

pub(super) enum DState<T> {
    None,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> DState<T> {
    pub fn ready(&self) -> Option<&T> {
        if let DState::Ready(v) = self { Some(v) } else { None }
    }
}

/// The object the details pane shows: kind, namespace, name.
pub(super) type DKey = (Kind, String, String);

pub(super) struct LogState {
    pub buf: LogBuf,
    pub container: String,
    pub previous: bool,
    pub tail_i: usize,
    pub follow: bool,
    pub paused: bool,
    pub search: TextField,
    pub err: Option<String>,
    pub loaded: bool,
    pub job: Option<Job<Result<Vec<LogLine>, String>>>,
    /// The request the running job answers: a fresh load replaces the buffer, a follow poll is merged.
    pub job_fresh: bool,
    pub polled: Option<Instant>,
    pub dirty: bool,
}

impl LogState {
    fn new() -> LogState {
        LogState { buf: LogBuf::new(kx::LOG_CAP), container: String::new(), previous: false, tail_i: 1, follow: true, paused: false, search: TextField::new("search the log", false), err: None, loaded: false, job: None, job_fresh: true, polled: None, dirty: true }
    }
}

pub(super) struct Detail {
    pub key: Option<DKey>,
    pub tab: DTab,
    pub obj: DState<Value>,
    pub events: DState<Vec<EventRow>>,
    pub related: DState<Vec<Rel>>,
    pub reveal: bool,
    pub view: TextView,
    pub logs: LogState,
    pub containers: Vec<String>,
    /// Bumped when the selection changes: answers of older jobs are dropped.
    pub gen: u64,
    pub obj_job: Option<Job<Result<Value, String>>>,
    pub ev_job: Option<Job<Result<Vec<EventRow>, String>>>,
    pub rel_job: Option<Job<Vec<Rel>>>,
    /// The selection settled on this key at this time; the load starts after [`SETTLE`].
    pub due: Option<Instant>,
    /// Pod owner for the confirmation text ("recreated by its controller").
    pub owned: bool,
    pub cordoned: bool,
}

impl Detail {
    fn new() -> Detail {
        Detail { key: None, tab: DTab::Summary, obj: DState::None, events: DState::None, related: DState::None, reveal: false, view: TextView::default(), logs: LogState::new(), containers: vec![], gen: 0, obj_job: None, ev_job: None, rel_job: None, due: None, owned: false, cordoned: false }
    }
}

/// A change waiting for the person's yes.
pub(super) struct Pending {
    pub action: Action,
    pub owned: bool,
    pub typed: TextField,
    /// Scale: the number being chosen.
    pub replicas: u32,
    pub scaling: Option<(Kind, String, String)>,
}

pub(super) struct Panel {
    pub title: String,
    pub action: Action,
    pub cluster: String,
    /// `cluster "gpu-box" (remote)` as it was when the change was confirmed.
    pub phrase: String,
    pub lines: Arc<Mutex<Vec<String>>>,
    pub done: Arc<Mutex<Option<Result<String, String>>>>,
    pub finished_at: Option<Instant>,
}

// ------------------------------------------------------------------------------------------------ the app

pub struct K8sApp {
    pub(super) rig: Rig,
    pub(super) user: String,
    pub(super) admin: bool,
    pub(super) role: Role,
    pub(super) dir: PathBuf,
    /// The cluster switcher of the header (hidden with one cluster) and the registry it reads.
    pub(super) host: ClusterHost,
    pub(super) prefs: Prefs,
    pub(super) entries: Vec<ClusterEntry>,
    pub(super) cur: usize,
    pub(super) managed: Managed,
    managed_at: Option<Instant>,
    switch: Arc<SwitchKube>,
    pub(super) feed: ExpFeed,
    pub(super) scope: Scope,
    pub(super) kind: Kind,
    /// A sort the person chose (column in the full column list, descending); None: the kind's default order.
    pub(super) sort: Option<(usize, bool)>,
    pub(super) filter: TextField,
    pub(super) ns_filter: TextField,
    pub(super) ns: ListState,
    pub(super) kinds: ListState,
    pub(super) res: ListState,
    pub(super) clusters: ListState,
    pub(super) focus: Focus,
    pub(super) overlay: Option<Overlay>,
    /// Phone width: showing the details page (Back returns to the lists).
    pub(super) page_detail: bool,
    pub(super) det: Detail,
    pub(super) mouse: (i32, i32),
    pub(super) mode: Mode,
    pub(super) last_draw: Option<Instant>,
    pub(super) pending: Option<Pending>,
    pub(super) reveal_ask: bool,
    pub(super) panel: Option<Panel>,
    pub(super) status: Option<(String, bool)>,
    /// The selected row, kept across refreshes (namespace, name).
    pub(super) sel_key: Option<(String, String)>,
    /// The order of the rows in view: indexes into the feed's list. Rebuilt when the list, the filter or the sort changes.
    order: Vec<usize>,
    order_key: Option<(usize, String, Option<(usize, bool)>)>,
    pub(super) host_save: Option<(String, Vec<u8>)>,
    request: Option<(String, String)>,
    start_job: Option<Job<Result<(), String>>>,
    pub(super) ide_ok: bool,
    /// The cluster log (the last kubectl problems) is on screen instead of the lists.
    pub(super) show_log: bool,
    /// The Launch cloud mode card (`k8s_cloud.rs`).
    pub(super) cloud: Option<super::k8s_cloud::CloudForm>,
    /// Tests: no waiting before a selection loads its details.
    pub(super) instant: bool,
}

impl K8sApp {
    pub fn new(cx: &AppCx) -> K8sApp {
        Self::with_rig(cx, Rig::system(cx))
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> K8sApp {
        let dir = cx.services.dir().to_path_buf();
        let user = cx.session.user.clone();
        let prefs = Prefs::load(&dir, &user);
        let (managed, kc) = (rig.managed)();
        let first = rig.kube_for.clone();
        let placeholder = first(&ClusterEntry { key: "managed".into(), label: String::new(), managed: true, target: Target::default(), reg: iyakki_agent::clusters::ClusterEntry::local_primary() });
        let switch = Arc::new(SwitchKube { cur: Mutex::new(placeholder) });
        let feed = ExpFeed::start(switch.clone(), Duration::from_secs(5), Duration::from_secs(10));
        let mut a = K8sApp {
            rig,
            user: user.clone(),
            admin: cx.session.role == Role::Admin,
            role: cx.session.role,
            host: ClusterHost::new(T_SWITCH, cx, ""),
            dir,
            prefs,
            entries: vec![],
            cur: 0,
            managed,
            managed_at: Some(Instant::now()),
            switch,
            feed,
            scope: Scope::All,
            kind: Kind::Pods,
            sort: None,
            filter: TextField::new("/ to filter", false),
            ns_filter: TextField::new("filter namespaces", false),
            ns: ListState::default(),
            kinds: ListState::default(),
            res: ListState::default(),
            clusters: ListState::default(),
            focus: Focus::Res,
            overlay: None,
            page_detail: false,
            det: Detail::new(),
            mouse: (-1, -1),
            mode: Mode::Wide,
            last_draw: None,
            pending: None,
            reveal_ask: false,
            panel: None,
            status: None,
            sel_key: None,
            order: vec![],
            order_key: None,
            host_save: None,
            request: None,
            start_job: None,
            ide_ok: false,
            show_log: false,
            cloud: None,
            instant: cfg!(test),
        };
        a.entries = a.build_entries(kc);
        // with several clusters the registry's active one decides (Settings -> Clusters, the switcher, the AI IDE); with one, the tile's own memory as before
        let want = match a.host.effective().filter(|_| a.host.visible()) {
            Some(eff) => a.entries.iter().find(|e| e.reg.id == eff.id).map(|e| e.key.clone()).unwrap_or_else(|| a.prefs.cluster.clone()),
            None => a.prefs.cluster.clone(),
        };
        let i = a.entries.iter().position(|e| e.key == want).or_else(|| (a.managed == Managed::Running).then_some(0)).or_else(|| (a.entries.len() > 1).then_some(1)).unwrap_or(0);
        a.select_cluster(i, false);
        a.ide_ok = !iyakki_agent::workspace::Registry::load(a.dir.as_path(), &user).items.is_empty();
        a
    }

    // ---------------------------------------------------------------------------------------- clusters

    /// The rows of the cluster list: the registry's clusters this account may use (the managed k3s first, as before); the registry
    /// takes in the saved kubeconfig profiles, so nothing the old list had is lost.
    fn build_entries(&self, kc: Option<PathBuf>) -> Vec<ClusterEntry> {
        let reg = iyakki_agent::clusters::Registry::load_synced(&self.dir, &self.user);
        let mut v = Vec::new();
        for e in reg.visible_to(self.role) {
            if e.is_primary() {
                v.push(ClusterEntry { key: "managed".into(), label: "Dev cluster (managed k3s)".into(), managed: true, target: Target { kubeconfig: kc.clone().map(|p| p.display().to_string()).unwrap_or_default(), context: String::new(), ..Default::default() }, reg: e.clone() });
            } else if e.kind.is_local() {
                v.push(ClusterEntry { key: format!("local:{}", e.id), label: e.name.clone(), managed: false, target: Target { kubeconfig: e.kubeconfig_for(&self.dir), ..Default::default() }, reg: e.clone() });
            } else {
                v.push(ClusterEntry { key: format!("profile:{}", e.name), label: e.name.clone(), managed: false, target: Target { kubeconfig: e.kubeconfig.clone(), context: e.context.clone(), ..Default::default() }, reg: e.clone() });
            }
        }
        if v.is_empty() {
            // a guest sees no cluster in the registry; the tile keeps its old single row (changes are off for them anyway)
            v.push(ClusterEntry { key: "managed".into(), label: "Dev cluster (managed k3s)".into(), managed: true, target: Target { kubeconfig: kc.map(|p| p.display().to_string()).unwrap_or_default(), ..Default::default() }, reg: iyakki_agent::clusters::ClusterEntry::local_primary() });
        }
        v
    }

    /// `cluster "gpu-box" (remote)` for the current row (every confirmation and audit line names it).
    pub(super) fn phrase(&self) -> String {
        self.entry().reg.phrase()
    }

    /// The state dot the switcher shows for a registry entry: the managed k3s knows its own state here.
    fn dot_of(managed: &Managed, e: &iyakki_agent::clusters::ClusterEntry) -> Option<iyakki_agent::clusters::Dot> {
        use iyakki_agent::clusters::Dot;
        if !e.is_primary() {
            return None;
        }
        Some(match managed {
            Managed::Running => Dot::Running,
            Managed::Failed(_) => Dot::Unreachable,
            Managed::Starting { .. } => Dot::Unknown,
            _ => Dot::Stopped,
        })
    }

    pub(super) fn entry(&self) -> &ClusterEntry {
        &self.entries[self.cur.min(self.entries.len() - 1)]
    }

    pub(super) fn cluster_key(&self) -> String {
        self.entry().key.clone()
    }

    /// Whether the cluster can be asked at all right now (the managed one has to be running).
    pub(super) fn usable(&self) -> bool {
        !self.entry().managed || self.managed == Managed::Running
    }

    pub(super) fn select_cluster(&mut self, i: usize, save: bool) {
        if i >= self.entries.len() {
            return;
        }
        self.cur = i;
        let key = self.entries[i].key.clone();
        if save {
            self.prefs.cluster = key.clone();
            self.save_prefs();
            self.remember_in_registry(i);
        }
        let p = self.prefs.of(&key);
        self.scope = Scope::from_id(&p.namespace);
        self.kind = Kind::from_resource(&p.kind).unwrap_or(Kind::Pods);
        self.switch.set((self.rig.kube_for)(&self.entries[i]));
        self.sort = None;
        self.filter.clear();
        self.ns.reset();
        self.res.reset();
        self.kinds.sel = Kind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
        self.kinds.reveal = true;
        self.sel_key = None;
        self.order_key = None;
        self.clear_detail();
        self.page_detail = false;
        self.update_want();
        self.status = None;
    }

    /// The tile's own choice also becomes the account's active cluster (role-checked; a guest's choice stays the tile's own).
    fn remember_in_registry(&mut self, i: usize) {
        let e = self.entries[i].reg.clone();
        if self.host.effective().is_some_and(|c| c.id == e.id) {
            return;
        }
        if iyakki_agent::clusters::switch(&self.dir, &self.dir, &self.user, self.role, &e.id).is_ok() {
            self.reload_host();
        }
    }

    /// Read the registry again for the switcher (after a switch, and now and then).
    fn reload_host(&mut self) {
        let managed = self.managed.clone();
        let dot = move |c: &iyakki_agent::clusters::ClusterEntry| Self::dot_of(&managed, c);
        self.host.reload(&dot);
    }

    fn save_prefs(&self) {
        let _ = self.prefs.save(&self.dir, &self.user);
    }

    pub(super) fn update_want(&mut self) {
        let t = self.entry().target.clone();
        self.feed.set_want(Want { target: t, scope: self.scope.clone(), kind: self.kind });
        self.order_key = None;
    }

    /// Why changes are off for this person and cluster (None: they can be turned on).
    pub(super) fn change_block(&self) -> Option<String> {
        if !self.admin {
            return Some("Only administrators can change things in a cluster".into());
        }
        let e = self.entry();
        let l = e.label.to_lowercase();
        if !e.managed && l.contains("prod") && !l.contains("nonprod") && !l.contains("non-prod") {
            return Some(format!("\"{}\" looks like production, so the tile only reads from it", e.label));
        }
        None
    }

    pub(super) fn allow_changes(&self) -> bool {
        self.change_block().is_none() && self.prefs.of(&self.cluster_key()).allow_changes
    }

    fn set_allow(&mut self, on: bool, cx: &AppCx) {
        if on {
            if let Some(why) = self.change_block() {
                self.say(why, false);
                return;
            }
        }
        let key = self.cluster_key();
        self.prefs.edit(&key, |c| c.allow_changes = on);
        self.save_prefs();
        let phrase = self.phrase();
        cx.audit.record(&self.user, if on { "k8s.allow_changes.on" } else { "k8s.allow_changes.off" }, &format!("cluster={key} on {phrase}"));
        let said = if on { format!("{}. Every change asks first and is recorded.", iyakki_agent::clusters::confirm_text_on("Changes are allowed on", &self.entry().reg)) } else { iyakki_agent::clusters::confirm_text_on("Read-only again on", &self.entry().reg) + "." };
        self.say(said, true);
    }

    pub(super) fn say(&mut self, m: impl Into<String>, ok: bool) {
        let m = m.into();
        clipboard::toast(&m);
        self.status = Some((m, ok));
    }

    // ---------------------------------------------------------------------------------------- namespace, kind, rows

    pub(super) fn choose_scope(&mut self, s: Scope) {
        if s == self.scope {
            return;
        }
        self.scope = s;
        let key = self.cluster_key();
        let id = self.scope.id();
        self.prefs.edit(&key, |c| c.namespace = id);
        self.save_prefs();
        self.res.reset();
        self.sel_key = None;
        self.filter.clear();
        self.clear_detail();
        self.update_want();
    }

    pub(super) fn choose_kind(&mut self, k: Kind) {
        self.kinds.sel = Kind::ALL.iter().position(|x| *x == k).unwrap_or(0);
        self.kinds.reveal = true;
        if k == self.kind {
            return;
        }
        self.kind = k;
        let key = self.cluster_key();
        let r = k.resource().to_string();
        self.prefs.edit(&key, |c| c.kind = r);
        self.save_prefs();
        self.sort = None;
        self.res.reset();
        self.sel_key = None;
        self.filter.clear();
        self.clear_detail();
        self.update_want();
    }

    /// The namespaces as the picker lists them: (scope, label, pods, failing), "All namespaces" first, filtered by the field.
    pub(super) fn ns_rows(&self) -> Vec<(Scope, String, Option<usize>, usize)> {
        let f = self.ns_filter.value.to_lowercase();
        let mut v = Vec::new();
        let list = self.feed.ns();
        let total: Option<usize> = list.as_ref().filter(|l| l.pods_known).map(|l| l.items.iter().map(|i| i.pods).sum());
        let failing: usize = list.as_ref().map_or(0, |l| l.items.iter().map(|i| i.failing).sum());
        if f.is_empty() || "all namespaces".contains(&f) {
            v.push((Scope::All, "All namespaces".to_string(), total, failing));
        }
        if let Some(l) = &list {
            for i in &l.items {
                if f.is_empty() || i.name.to_lowercase().contains(&f) {
                    v.push((Scope::Ns(i.name.clone()), i.name.clone(), l.pods_known.then_some(i.pods), i.failing));
                }
            }
        }
        // a namespace chosen earlier that the list does not hold (yet) is still shown, so the choice is visible
        if let Scope::Ns(n) = &self.scope {
            if !v.iter().any(|(s, ..)| s == &self.scope) && (f.is_empty() || n.to_lowercase().contains(&f)) {
                v.push((self.scope.clone(), n.clone(), None, 0));
            }
        }
        v
    }

    pub(super) fn listing(&self) -> Option<Arc<kx::Listing>> {
        self.feed.listing().filter(|l| l.kind == self.kind && l.scope == self.scope)
    }

    /// Rebuild the order of the rows when the list, the filter or the sort changed. Returns the order (indexes into `listing().rows`).
    pub(super) fn order(&mut self) -> &[usize] {
        let l = self.listing();
        let key = l.as_ref().map(|l| (Arc::as_ptr(l) as usize, self.filter.value.clone(), self.sort));
        if key != self.order_key || (key.is_none() && !self.order.is_empty()) {
            self.order = match &l {
                Some(l) => {
                    let mut idx = kx::filter_rows(&l.rows, &self.filter.value);
                    if let Some((c, d)) = self.sort {
                        idx.sort_by(|&a, &b| kx::cmp_rows(&l.rows[a], &l.rows[b], c, d));
                    }
                    idx
                }
                None => vec![],
            };
            self.order_key = key;
            // keep the same object selected across a refresh
            if let (Some(l), Some(k)) = (&l, &self.sel_key) {
                if let Some(p) = self.order.iter().position(|&i| l.rows[i].namespace == k.0 && l.rows[i].name == k.1) {
                    self.res.sel = p;
                    self.res.reveal = false;
                }
            }
        }
        &self.order
    }

    pub(super) fn shown_rows(&mut self) -> usize {
        self.order().len().min(kx::MAX_SHOWN)
    }

    /// The row under the selection.
    pub(super) fn selected_row(&mut self) -> Option<kx::Row> {
        let l = self.listing()?;
        let n = self.shown_rows();
        let sel = self.res.sel.min(n.saturating_sub(1));
        let i = *self.order().get(sel)?;
        l.rows.get(i).cloned()
    }

    fn sync_selection(&mut self) {
        let row = self.selected_row();
        let key = row.as_ref().map(|r| (r.namespace.clone(), r.name.clone()));
        if key != self.sel_key {
            self.sel_key = key;
            self.on_selection_changed();
        }
    }

    // ---------------------------------------------------------------------------------------- details

    pub(super) fn clear_detail(&mut self) {
        self.det.gen += 1;
        self.det.key = None;
        self.det.obj = DState::None;
        self.det.events = DState::None;
        self.det.related = DState::None;
        self.det.reveal = false;
        self.det.obj_job = None;
        self.det.ev_job = None;
        self.det.rel_job = None;
        self.det.logs = LogState::new();
        self.det.logs.follow = true;
        self.det.containers.clear();
        self.det.due = None;
        self.det.view.set(vec![], false);
        self.pending = None;
        self.reveal_ask = false;
    }

    fn on_selection_changed(&mut self) {
        let Some((ns, name)) = self.sel_key.clone() else {
            self.clear_detail();
            return;
        };
        let key: DKey = (self.kind, ns, name);
        if self.det.key.as_ref() == Some(&key) {
            return;
        }
        self.clear_detail();
        self.det.key = Some(key);
        if self.det.tab == DTab::Logs && self.kind != Kind::Pods {
            self.det.tab = DTab::Summary;
        }
        self.det.due = Some(if self.instant { Instant::now().checked_sub(SETTLE).unwrap_or_else(Instant::now) } else { Instant::now() });
    }

    pub(super) fn kube(&self) -> Arc<dyn Kube> {
        self.switch.clone()
    }

    /// Start reading the selected object (and what the open tab needs).
    fn load_detail(&mut self) {
        let Some((kind, ns, name)) = self.det.key.clone() else { return };
        self.det.due = None;
        self.det.obj = DState::Loading;
        let (kube, t, reveal) = (self.kube(), self.entry().target.clone(), self.det.reveal);
        self.det.obj_job = Some(Job::spawn(move || {
            let mut v = kx::get_object(&*kube, &t, kind, &ns, &name)?;
            if kind == Kind::Secrets && !reveal {
                kx::scrub_secret(&mut v);
            }
            Ok(v)
        }));
        self.load_tab_data();
        self.rebuild_view();
    }

    fn load_tab_data(&mut self) {
        let Some((kind, ns, name)) = self.det.key.clone() else { return };
        let (kube, t) = (self.kube(), self.entry().target.clone());
        match self.det.tab {
            DTab::Events if matches!(self.det.events, DState::None) => {
                self.det.events = DState::Loading;
                let now = clock();
                self.det.ev_job = Some(Job::spawn(move || kx::object_events(&*kube, &t, kind, &ns, &name, now)));
            }
            DTab::Related if matches!(self.det.related, DState::None) => {
                if let Some(obj) = self.det.obj.ready().cloned() {
                    self.det.related = DState::Loading;
                    self.det.rel_job = Some(Job::spawn(move || kx::related(&*kube, &t, kind, &ns, &obj)));
                }
            }
            DTab::Logs => self.start_logs(true),
            _ => {}
        }
    }

    pub(super) fn set_tab(&mut self, tab: DTab) {
        if tab == DTab::Logs && self.kind != Kind::Pods {
            self.say("Logs are for pods: select a pod.", false);
            return;
        }
        self.det.tab = tab;
        self.load_tab_data();
        self.rebuild_view();
    }

    fn start_logs(&mut self, fresh: bool) {
        let Some((Kind::Pods, ns, name)) = self.det.key.clone() else { return };
        if self.det.logs.job.is_some() {
            return;
        }
        let l = &mut self.det.logs;
        let since = if fresh || l.previous { None } else { l.buf.last_ts() };
        let req = LogReq { container: l.container.clone(), previous: l.previous, tail: LogReq::TAILS[l.tail_i.min(2)], since_time: since };
        l.job_fresh = fresh || req.since_time.is_none();
        l.polled = Some(Instant::now());
        let (kube, t) = (self.kube(), self.entry().target.clone());
        self.det.logs.job = Some(Job::spawn(move || kx::fetch_logs(&*kube, &t, &ns, &name, &req)));
    }

    /// Pick up the answers of the detail, event, related and log jobs.
    fn poll_jobs(&mut self) {
        let mut changed = false;
        if let Some(r) = self.det.obj_job.as_ref().and_then(|j| j.take()) {
            self.det.obj_job = None;
            match r {
                Ok(v) => {
                    self.det.containers = if self.kind == Kind::Pods { kx::pod_containers(&v) } else { vec![] };
                    self.det.owned = v["metadata"]["ownerReferences"].as_array().is_some_and(|a| !a.is_empty());
                    self.det.cordoned = v["spec"]["unschedulable"].as_bool() == Some(true);
                    self.det.obj = DState::Ready(v);
                    if self.det.tab == DTab::Related && matches!(self.det.related, DState::None) {
                        self.load_tab_data();
                    }
                }
                Err(e) => self.det.obj = DState::Failed(e),
            }
            changed = true;
        }
        if let Some(r) = self.det.ev_job.as_ref().and_then(|j| j.take()) {
            self.det.ev_job = None;
            self.det.events = match r {
                Ok(v) => DState::Ready(v),
                Err(e) => DState::Failed(e),
            };
            changed = true;
        }
        if let Some(r) = self.det.rel_job.as_ref().and_then(|j| j.take()) {
            self.det.rel_job = None;
            self.det.related = DState::Ready(r);
            changed = true;
        }
        if let Some(r) = self.det.logs.job.as_ref().and_then(|j| j.take()) {
            let l = &mut self.det.logs;
            l.job = None;
            match r {
                Ok(lines) => {
                    l.err = None;
                    if l.job_fresh {
                        l.buf.set(lines);
                    } else {
                        l.buf.merge(lines);
                    }
                    l.loaded = true;
                }
                Err(e) => {
                    l.err = Some(e);
                    l.loaded = true;
                }
            }
            l.dirty = true;
            changed = true;
        }
        if changed {
            self.rebuild_view();
        }
    }

    /// Build the lines of the open tab.
    pub(super) fn rebuild_view(&mut self) {
        use crate::fx::{AMBER, CYAN};
        use crate::ui::{DIM, ERR, MUTED, OK, TEXT};
        let keep = self.det.tab == DTab::Logs;
        let mut lines: Vec<Line> = Vec::new();
        let Some((kind, _, _)) = self.det.key.clone() else {
            self.det.view.set(lines, false);
            return;
        };
        match self.det.tab {
            DTab::Summary | DTab::Yaml => match &self.det.obj {
                DState::None | DState::Loading => lines.push(Line::plain("Reading it from the cluster\u{2026}", DIM)),
                DState::Failed(e) => lines.push(Line::plain(format!("Could not read it: {e}"), ERR)),
                DState::Ready(v) => {
                    if self.det.tab == DTab::Yaml {
                        for l in kx::object_yaml(kind, v, self.det.reveal).lines() {
                            lines.push(yaml_line(l));
                        }
                    } else {
                        for (k, val) in kx::summary(kind, v, clock()) {
                            let color = if k == "Status" && (val.starts_with("Running") || val.starts_with("Ready")) { OK } else if k == "What it means" { AMBER } else if k == "Status" && !val.starts_with("Finished") { ERR } else { TEXT };
                            lines.push(Line { text: format!("{k}: {val}"), color, key_len: k.chars().count() + 1 });
                        }
                    }
                }
            },
            DTab::Events => match &self.det.events {
                DState::None | DState::Loading => lines.push(Line::plain("Reading the events\u{2026}", DIM)),
                DState::Failed(e) => lines.push(Line::plain(format!("Could not read the events: {e}"), ERR)),
                DState::Ready(v) if v.is_empty() => lines.push(Line::plain("No events for this object (the cluster forgets them after about an hour).", DIM)),
                DState::Ready(v) => {
                    for e in v {
                        let times = if e.count > 1 { format!(" (x{})", e.count) } else { String::new() };
                        lines.push(Line { text: format!("{} {} \u{b7} {} ago{}: {}", e.kind, e.reason, iyakki_agent::k8s::age_text(e.age), times, e.message), color: if e.kind == "Warning" { AMBER } else { TEXT }, key_len: 0 });
                    }
                }
            },
            DTab::Related => match &self.det.related {
                DState::None | DState::Loading => lines.push(Line::plain("Looking for related objects\u{2026}", DIM)),
                DState::Failed(e) => lines.push(Line::plain(e.clone(), ERR)),
                DState::Ready(v) if v.is_empty() => lines.push(Line::plain("Nothing else is tied to this object.", DIM)),
                DState::Ready(v) => {
                    for r in v {
                        let note = if r.note.is_empty() { String::new() } else { format!(" ({})", r.note) };
                        lines.push(Line { text: format!("{}: {} {}{}", r.relation, r.kind.api_kind(), r.name, note), color: TEXT, key_len: r.relation.chars().count() + 1 });
                    }
                }
            },
            DTab::Logs => {
                let l = &self.det.logs;
                if let Some(e) = &l.err {
                    lines.push(Line::plain(format!("Could not read the log: {e}"), ERR));
                } else if !l.loaded {
                    lines.push(Line::plain("Reading the log\u{2026}", DIM));
                } else if l.buf.lines.is_empty() {
                    lines.push(Line::plain(if l.previous { "There is no previous log: the container has not restarted." } else { "The log is empty." }, DIM));
                }
                for x in &l.buf.lines {
                    let up = x.text.to_ascii_uppercase();
                    let c = if up.contains("ERROR") || up.contains("FATAL") || up.contains("PANIC") { ERR } else if up.contains("WARN") { AMBER } else { TEXT };
                    lines.push(Line::plain(x.text.clone(), c));
                }
                let _ = (CYAN, MUTED);
            }
        }
        let stick = self.det.tab == DTab::Logs && self.det.logs.follow && !self.det.logs.paused;
        self.det.view.set(lines, keep);
        if keep {
            self.det.view.stick = stick || self.det.view.stick && self.det.logs.follow;
        }
    }

    // ---------------------------------------------------------------------------------------- actions

    /// The change buttons the selected object offers: (code offset, label).
    pub(super) fn action_buttons(&self) -> Vec<(usize, &'static str)> {
        let Some((kind, _, _)) = &self.det.key else { return vec![] };
        match kind {
            Kind::Deployments | Kind::StatefulSets => vec![(0, "Restart"), (1, "Scale")],
            Kind::DaemonSets => vec![(0, "Restart")],
            Kind::ReplicaSets => vec![(1, "Scale"), (5, "Delete")],
            Kind::Pods => vec![(2, "Delete pod")],
            Kind::Nodes => vec![if self.det.cordoned { (4, "Uncordon") } else { (3, "Cordon") }],
            Kind::Namespaces | Kind::Events | Kind::Crds => vec![],
            _ => vec![(5, "Delete")],
        }
    }

    pub(super) fn begin_action(&mut self, which: usize) {
        if !self.allow_changes() {
            self.say(self.change_block().unwrap_or_else(|| "Turn on Allow changes first (the switch in the header).".into()), false);
            return;
        }
        let Some((kind, ns, name)) = self.det.key.clone() else { return };
        let mk = |action: Action, owned: bool, scaling| Pending { action, owned, typed: TextField::new("type the name", false), replicas: 1, scaling };
        let p = match which {
            0 => mk(Action::Restart { kind, ns, name }, true, None),
            1 => {
                let n = self.det.obj.ready().and_then(|v| v["spec"]["replicas"].as_u64()).unwrap_or(1) as u32;
                let mut p = mk(Action::Scale { kind, ns: ns.clone(), name: name.clone(), replicas: n }, true, Some((kind, ns, name)));
                p.replicas = n;
                p
            }
            2 => mk(Action::DeletePod { ns, name }, self.det.owned, None),
            3 => mk(Action::Cordon { node: name }, true, None),
            4 => mk(Action::Uncordon { node: name }, true, None),
            _ => mk(Action::Delete { kind, ns, name }, true, None),
        };
        if let Err(e) = p.action.validate() {
            self.say(e, false);
            return;
        }
        self.pending = Some(p);
        self.focus = if matches!(self.pending.as_ref().map(|p| p.action.confirm()), Some(Confirm::TypeName(_))) { Focus::Detail } else { self.focus };
    }

    /// The person said yes to the card.
    pub(super) fn confirm_action(&mut self, cx: &AppCx) {
        let Some(p) = self.pending.take() else { return };
        let mut action = p.action.clone();
        if let Action::Scale { replicas, .. } = &mut action {
            *replicas = p.replicas;
        }
        if let Confirm::TypeName(n) = action.confirm() {
            if p.typed.value.trim() != n {
                self.pending = Some(p);
                self.say("Type the name exactly as shown to confirm.", false);
                return;
            }
        }
        // checked again here: the switch may have been turned off while the card was open
        if !self.allow_changes() {
            self.say("Changes are not allowed on this cluster.", false);
            return;
        }
        if let Err(e) = action.validate() {
            self.say(e, false);
            return;
        }
        let key = self.cluster_key();
        let phrase = self.phrase();
        cx.audit.record(&self.user, action.audit_name(), &format!("{} on {phrase}", action.audit_detail(&key)));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(Mutex::new(None));
        self.panel = Some(Panel { title: action.title(), action: action.clone(), cluster: key, phrase: phrase.clone(), lines: lines.clone(), done: done.clone(), finished_at: None });
        let (kube, t) = (self.kube(), self.entry().target.clone());
        let a2 = action.clone();
        std::thread::spawn(move || {
            let l2 = lines.clone();
            let r = kx::run_action(&*kube, &t, &a2, &mut |l: &str| l2.lock().unwrap_or_else(|e| e.into_inner()).push(l.to_string()));
            *done.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        });
    }
}

fn clock() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A YAML line with the length of its key part (indent, dash and key up to the colon) for colouring.
fn yaml_line(l: &str) -> Line {
    use crate::fx::{AMBER, MAGENTA};
    use crate::ui::{DIM, TEXT};
    let trimmed = l.trim_start();
    let indent = l.len() - trimmed.len();
    let body = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let dash = trimmed.len() - body.len();
    let key_end = if body.starts_with('"') { None } else { body.find(": ").map(|i| i + 1).or_else(|| body.strip_suffix(':').map(|b| b.len() + 1)) };
    match key_end {
        Some(k) => {
            let value = body[k..].trim();
            let color = if value.is_empty() || value == "|" { TEXT } else if value == kx::MASK { DIM } else if value == "true" || value == "false" || value == "null" { MAGENTA } else if value.parse::<f64>().is_ok() { AMBER } else { TEXT };
            Line { text: l.to_string(), color, key_len: indent + dash + k }
        }
        None => Line { text: l.to_string(), color: TEXT, key_len: 0 },
    }
}

// ------------------------------------------------------------------------------------------------ heartbeat

impl K8sApp {
    /// The heartbeat: the managed cluster's state, the feed's visibility, the answers of the jobs, the follow poll.
    pub(super) fn tick(&mut self, cx: &AppCx) {
        if self.managed_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            self.managed_at = Some(Instant::now());
            self.refresh_entries();
        }
        let vis = self.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) && self.usable();
        self.feed.set_visible(vis);
        // a refused connection while the managed k3s starts is expected: the feed retries quietly
        self.feed.set_starting(self.entry().managed && matches!(self.managed, Managed::Starting { .. }));
        self.feed.set_local(self.entry().managed);
        if let Some(r) = self.start_job.as_ref().and_then(|j| j.take()) {
            self.start_job = None;
            match r {
                Ok(()) => self.say("Starting the dev cluster. The first start takes a minute or two.", true),
                Err(e) => self.say(e, false),
            }
        }
        self.poll_jobs();
        self.sync_selection();
        if self.det.due.is_some_and(|t| t.elapsed() >= SETTLE) {
            self.load_detail();
        }
        let l = &self.det.logs;
        if vis && self.det.tab == DTab::Logs && l.follow && !l.paused && !l.previous && l.loaded && l.job.is_none() && l.polled.is_none_or(|t| t.elapsed() >= FOLLOW_EVERY) {
            self.start_logs(false);
        }
        self.poll_panel(cx);
        if self.res.fit > 0 {
            let n = self.shown_rows();
            self.res.sel = self.res.sel.min(n.saturating_sub(1));
        }
    }

    fn refresh_entries(&mut self) {
        let (m, kc) = (self.rig.managed)();
        let was = std::mem::replace(&mut self.managed, m);
        let kc_s = kc.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        let mut fresh = self.build_entries(kc);
        fresh[0].target.kubeconfig = kc_s;
        let key = self.cluster_key();
        if fresh != self.entries {
            self.entries = fresh;
            self.cur = self.entries.iter().position(|e| e.key == key).unwrap_or(0);
            self.update_want();
            self.reload_host();
        }
        if was != self.managed {
            self.reload_host();
        } else {
            let managed = self.managed.clone();
            self.host.tick(&move |c| Self::dot_of(&managed, c));
        }
        // the registry's active cluster changed elsewhere (Settings -> Clusters, the AI IDE): follow it
        if self.host.visible() {
            let eff = self.host.effective().map(|e| e.id.clone());
            if let Some(i) = eff.and_then(|id| self.entries.iter().position(|e| e.reg.id == id)) {
                if i != self.cur {
                    self.select_cluster(i, false);
                }
            }
        }
        if was != Managed::Running && self.managed == Managed::Running && self.entry().managed {
            self.feed.refresh();
        }
    }

    fn poll_panel(&mut self, cx: &AppCx) {
        let Some(p) = &mut self.panel else { return };
        let title = p.title.clone();
        if p.finished_at.is_none() {
            let r = p.done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(r) = r {
                p.finished_at = Some(Instant::now());
                let (name, detail) = (p.action.audit_name(), format!("{} on {}", p.action.audit_detail(&p.cluster), p.phrase));
                match r {
                    Ok(m) => {
                        cx.audit.record(&self.user, &format!("{name}.ok"), &detail);
                        self.say(m, true);
                    }
                    Err(e) => {
                        cx.audit.record(&self.user, &format!("{name}.failed"), &detail);
                        self.say(format!("{title}: {e}"), false);
                    }
                }
                self.feed.refresh();
                // the object changed (or went away): read it again
                if self.det.key.is_some() {
                    self.det.due = Some(Instant::now());
                }
            }
        } else if p.finished_at.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
            self.panel = None;
        }
    }

    fn start_managed(&mut self) {
        if self.start_job.is_some() {
            return;
        }
        let (f, u) = (self.rig.start.clone(), self.user.clone());
        self.start_job = Some(Job::spawn(move || f(&u)));
        self.say("Starting the dev cluster\u{2026}", true);
    }

    pub(super) fn load_now(&mut self) {
        if self.det.key.is_some() {
            self.det.due = Some(Instant::now().checked_sub(SETTLE).unwrap_or_else(Instant::now));
        }
    }

    // ---------------------------------------------------------------------------------------- input

    fn open_ns(&mut self) {
        if self.mode == Mode::Wide {
            self.focus = Focus::Ns;
        } else {
            self.overlay = Some(Overlay::Namespaces);
            self.focus = Focus::NsFilter;
        }
        let i = self.ns_rows().iter().position(|r| r.0 == self.scope).unwrap_or(0);
        self.ns.sel = i;
        self.ns.reveal = true;
    }

    fn open_kinds(&mut self) {
        if self.mode == Mode::Wide {
            self.focus = Focus::Kinds;
        } else {
            self.overlay = Some(Overlay::Kinds);
        }
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
        if matches!(self.focus, Focus::NsFilter) {
            self.ns_filter.clear();
            self.focus = Focus::Res;
        }
    }

    fn pick_ns(&mut self, i: usize) {
        let rows = self.ns_rows();
        if let Some(r) = rows.get(i) {
            self.choose_scope(r.0.clone());
        }
        self.ns_filter.clear();
        self.overlay = None;
        self.focus = Focus::Res;
    }

    fn pick_kind(&mut self, i: usize) {
        if let Some(k) = Kind::ALL.get(i) {
            self.choose_kind(*k);
        }
        self.overlay = None;
        self.focus = Focus::Res;
    }

    fn open_row(&mut self) {
        self.sync_selection();
        self.load_now();
        if self.mode == Mode::Phone {
            self.page_detail = true;
        }
        self.focus = Focus::Detail;
    }

    fn go_related(&mut self, i: usize) {
        let Some(r) = self.det.related.ready().and_then(|v| v.get(i)).cloned() else { return };
        if r.kind.namespaced() && !r.namespace.is_empty() {
            self.choose_scope(Scope::Ns(r.namespace.clone()));
        }
        self.choose_kind(r.kind);
        self.filter.set(&r.name);
        self.res.reset();
        self.sel_key = None;
        self.page_detail = false;
        self.focus = Focus::Res;
        self.sync_selection();
        self.load_now();
    }

    fn sort_by(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, d)) if c == col => Some((c, !d)),
            _ => Some((col, false)),
        };
        self.res.reset();
    }

    fn copy_view(&mut self) {
        let t = self.det.view.text();
        if t.is_empty() {
            self.say("Nothing to copy yet.", false);
        } else {
            clipboard::copy(&t);
            self.status = Some(("Copied to the clipboard.".into(), true));
        }
    }

    /// The cluster log (what kubectl said, when, the verdict and the retry) to the clipboard.
    pub(super) fn copy_cluster_log(&mut self) {
        clipboard::copy(&self.feed.cluster_log().text());
        self.status = Some(("Copied the cluster log to the clipboard.".into(), true));
    }

    /// Use another retry policy (tests give it a fake clock): the feed is started again for the same cluster.
    #[cfg(test)]
    pub(super) fn use_retry(&mut self, r: iyakki_agent::k8sretry::Retry) {
        self.feed = ExpFeed::start_with(self.switch.clone(), Duration::from_secs(5), Duration::from_secs(10), r);
        self.update_want();
    }

    /// The dim line over stale data: the cluster did not answer, the last list stays, a retry is under way.
    pub(super) fn stale_text(&self) -> Option<String> {
        let st = self.feed.status();
        (st.failing && !st.banner).then(|| iyakki_agent::k8sretry::stale_line(&st))
    }

    fn copy_exec(&mut self) {
        if let Some((Kind::Pods, ns, name)) = self.det.key.clone() {
            let c = if self.det.logs.container.is_empty() { self.det.containers.first().cloned().filter(|_| self.det.containers.len() > 1).unwrap_or_default() } else { self.det.logs.container.clone() };
            clipboard::copy(&kx::exec_command(&self.entry().target, &ns, &name, &c));
            self.status = Some(("Copied the kubectl exec command: paste it in the CLI tile.".into(), true));
        } else {
            self.say("Select a pod first.", false);
        }
    }

    fn save_logs(&mut self, cx: &AppCx) {
        let Some((Kind::Pods, ns, name)) = self.det.key.clone() else { return };
        let text: String = self.det.logs.buf.lines.iter().map(|l| format!("{}\n", l.text)).collect();
        if text.is_empty() {
            self.say("The log is empty: nothing to save.", false);
            return;
        }
        let file = format!("{ns}-{name}.log");
        if iyakki_agent::hostfiles::enabled() {
            self.host_save = Some((file, text.into_bytes()));
            return;
        }
        let home = iyakki_agent::scope::home_root(&self.user, cx.services.dir());
        let path = home.join(&file);
        if !cx.session.policy.allows_write(&self.user, &path.to_string_lossy()) {
            self.say("Your account may not write there.", false);
            return;
        }
        match std::fs::write(&path, text) {
            Ok(()) => self.say(format!("Saved {}", path.display()), true),
            Err(e) => self.say(format!("Could not save: {e}"), false),
        }
    }

    fn wheel(&mut self, d: i32) {
        let rows = -d * WHEEL_ROWS;
        let (x, y) = self.mouse;
        if let Some(o) = self.overlay {
            match o {
                Overlay::Clusters => {
                    let n = self.entries.len();
                    self.clusters.scroll(n, rows);
                }
                Overlay::Namespaces => {
                    let n = self.ns_rows().len();
                    self.ns.scroll(n, rows);
                }
                Overlay::Kinds => self.kinds.scroll(Kind::ALL.len(), rows),
            }
            return;
        }
        if self.ns.rect.contains(x, y) && self.mode == Mode::Wide {
            let n = self.ns_rows().len();
            self.ns.scroll(n, rows);
        } else if self.kinds.rect.contains(x, y) && self.mode == Mode::Wide {
            self.kinds.scroll(Kind::ALL.len(), rows);
        } else if self.det.view.rect.contains(x, y) {
            self.det.view.scroll(rows);
        } else if self.res.rect.contains(x, y) {
            let n = self.shown_rows();
            self.res.scroll(n, rows);
        }
    }

    pub(super) fn handle_event(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Tick => {
                self.tick(cx);
                return Flow::Stay;
            }
            Event::MouseMove(x, y) | Event::MouseDown(x, y) => {
                self.mouse = (*x, *y);
                if matches!(ev, Event::MouseMove(..)) {
                    return Flow::Stay;
                }
            }
            Event::Scroll(d) => {
                self.wheel(*d);
                return Flow::Stay;
            }
            Event::Paste(t) => {
                match self.focus {
                    Focus::Filter => {
                        self.filter.paste(t);
                    }
                    Focus::NsFilter => {
                        self.ns_filter.paste(t);
                    }
                    Focus::LogSearch => {
                        self.det.logs.search.paste(t);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Event::FocusAt(id, x) => {
                match *id {
                    F_FILTER => {
                        self.focus = Focus::Filter;
                        self.filter.click(*x);
                    }
                    F_NS => {
                        self.focus = Focus::NsFilter;
                        self.ns_filter.click(*x);
                    }
                    F_LOGS => {
                        self.focus = Focus::LogSearch;
                        self.det.logs.search.click(*x);
                    }
                    F_NAME => {
                        if let Some(p) = &mut self.pending {
                            p.typed.click(*x);
                        }
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Event::Tab(n) if *n >= T_SWITCH && *n < T_SWITCH + 1_000 => self.switcher(ev),
            Event::Tab(n) => {
                if let Some(f) = self.click(*n, cx) {
                    return f;
                }
            }
            Event::Key(_) if self.host.owns_keys() => self.switcher(ev),
            Event::Key(k) => return self.key(k, cx),
            _ => {}
        }
        Flow::Stay
    }

    /// A click or key of the cluster switcher: switch the registry's active cluster, or open Settings -> Clusters.
    fn switcher(&mut self, ev: &Event) {
        let managed = self.managed.clone();
        let out = self.host.handle(ev, &move |c| Self::dot_of(&managed, c));
        match out {
            HostOut::None => {}
            HostOut::Manage => self.request = Some((super::cluster_host::MANAGE_REQUEST.0.into(), super::cluster_host::MANAGE_REQUEST.1.into())),
            HostOut::Refused(why) => self.say(why, false),
            HostOut::Switched(e) => {
                if let Some(i) = self.entries.iter().position(|x| x.reg.id == e.id) {
                    if i != self.cur {
                        self.select_cluster(i, false);
                    }
                }
                self.say(format!("Working against {}.", e.phrase()), true);
            }
        }
    }

    fn click(&mut self, n: usize, cx: &AppCx) -> Option<Flow> {
        match n {
            n if (super::k8s_cloud::T_CLOUD..=super::k8s_cloud::T_CLOUD_LAST).contains(&n) => self.cloud_click(n, cx),
            T_CLUSTER => {
                self.overlay = Some(Overlay::Clusters);
                self.clusters.sel = self.cur;
                self.clusters.reveal = true;
            }
            T_NS_BTN => self.open_ns(),
            T_KIND_BTN => self.open_kinds(),
            T_REFRESH => {
                self.feed.refresh();
                self.load_now();
                self.status = Some(("Refreshing\u{2026}".into(), true));
            }
            T_ALLOW => {
                let on = !self.allow_changes();
                self.set_allow(on, cx);
            }
            T_BACK => {
                if self.page_detail {
                    self.page_detail = false;
                    self.focus = Focus::Res;
                } else {
                    return Some(Flow::Back);
                }
            }
            T_START => self.start_managed(),
            T_SETTINGS => self.request = Some(("settings".into(), "tab:devcluster".into())),
            T_IDE => self.request = Some(("ide".into(), String::new())),
            T_CLOSE => self.close_overlay(),
            T_LOG => self.show_log = !self.show_log,
            T_LOG_COPY => self.copy_cluster_log(),
            T_COPY => self.copy_view(),
            T_EXEC => self.copy_exec(),
            T_SAVE => self.save_logs(cx),
            T_REVEAL => {
                if self.det.reveal {
                    self.det.reveal = false;
                    self.load_now();
                } else {
                    self.reveal_ask = true;
                }
            }
            T_REVEAL_YES => self.reveal(cx),
            T_WRAP => self.det.view.wrap = !self.det.view.wrap,
            T_FOLLOW => {
                self.det.logs.follow = !self.det.logs.follow;
                if self.det.logs.follow {
                    self.det.view.end();
                }
            }
            T_PAUSE => self.det.logs.paused = !self.det.logs.paused,
            T_PREV => {
                self.det.logs.previous = !self.det.logs.previous;
                self.reload_logs();
            }
            T_TAIL => {
                self.det.logs.tail_i = (self.det.logs.tail_i + 1) % 3;
                self.reload_logs();
            }
            T_YES => self.confirm_action(cx),
            T_NO => {
                self.pending = None;
                self.reveal_ask = false;
            }
            T_SCALE_DN => {
                if let Some(p) = &mut self.pending {
                    p.replicas = p.replicas.saturating_sub(1);
                }
            }
            T_SCALE_UP => {
                if let Some(p) = &mut self.pending {
                    p.replicas = (p.replicas + 1).min(kx::MAX_REPLICAS);
                }
            }
            T_PANEL_CLOSE => self.panel = None,
            T_CLEAR_FILTER => {
                self.filter.clear();
                self.res.reset();
            }
            n if (T_TAB..T_TAB + 5).contains(&n) => self.set_tab(DTab::ALL[n - T_TAB]),
            n if (T_ACT..T_ACT + 6).contains(&n) => self.begin_action(n - T_ACT),
            n if (T_CONT..T_CONT + 100).contains(&n) => {
                let c = self.det.containers.get(n - T_CONT).cloned().unwrap_or_default();
                self.det.logs.container = c;
                self.reload_logs();
            }
            n if (T_CLUSTER_ROW..T_CLUSTER_ROW + 1000).contains(&n) => {
                self.overlay = None;
                self.select_cluster(n - T_CLUSTER_ROW, true);
            }
            n if (T_SORT..T_SORT + 100).contains(&n) => self.sort_by(n - T_SORT),
            n if (T_REL_ROW..T_REL_ROW + 1000).contains(&n) => self.go_related(n - T_REL_ROW),
            n if (T_NS_ROW..T_NS_ROW + 100_000).contains(&n) => self.pick_ns(n - T_NS_ROW),
            n if (T_KIND_ROW..T_KIND_ROW + 100).contains(&n) => self.pick_kind(n - T_KIND_ROW),
            n if n >= T_RES_ROW => {
                self.res.sel = n - T_RES_ROW;
                self.focus = Focus::Res;
                self.sync_selection();
                self.open_row();
                if self.mode != Mode::Phone {
                    self.focus = Focus::Res;
                }
            }
            _ => {}
        }
        None
    }

    fn reload_logs(&mut self) {
        let l = &mut self.det.logs;
        l.buf.clear();
        l.loaded = false;
        l.err = None;
        l.job = None;
        self.start_logs(true);
        self.rebuild_view();
    }

    fn reveal(&mut self, cx: &AppCx) {
        self.reveal_ask = false;
        let Some((Kind::Secrets, ns, name)) = self.det.key.clone() else { return };
        // recorded by name only, never the values
        cx.audit.record(&self.user, "k8s.secret.reveal", &format!("cluster={} ns={ns} Secret/{name}", self.cluster_key()));
        self.det.reveal = true;
        self.det.tab = DTab::Yaml;
        self.load_now();
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        // the Launch cloud mode card owns every key while it is open
        if self.cloud.is_some() {
            return self.cloud_key(k, cx);
        }
        // a card for a change: it owns every key
        if self.pending.is_some() {
            return self.card_key(k, cx);
        }
        if self.reveal_ask {
            match k {
                Key::Char('y') | Key::Enter => self.reveal(cx),
                Key::Char('n') | Key::Esc => self.reveal_ask = false,
                _ => {}
            }
            return Flow::Stay;
        }
        if matches!(self.focus, Focus::Filter | Focus::NsFilter | Focus::LogSearch) {
            return self.field_key(k);
        }
        if let Some(o) = self.overlay {
            return self.overlay_key(o, k);
        }
        let n_res = self.shown_rows();
        match k {
            Key::Esc => {
                if self.show_log {
                    self.show_log = false;
                } else if self.panel.is_some() {
                    self.panel = None;
                } else if self.mode == Mode::Phone && self.page_detail {
                    self.page_detail = false;
                    self.focus = Focus::Res;
                } else if self.focus == Focus::Detail {
                    self.focus = Focus::Res;
                } else if !self.filter.value.is_empty() {
                    self.filter.clear();
                    self.res.reset();
                } else {
                    return Flow::Back;
                }
            }
            Key::Char('r') => {
                self.feed.refresh();
                self.load_now();
                self.status = Some(("Refreshing\u{2026}".into(), true));
            }
            Key::Char('/') => {
                self.focus = match self.focus {
                    Focus::Ns if self.mode == Mode::Wide => Focus::NsFilter,
                    Focus::Detail if self.det.tab == DTab::Logs => Focus::LogSearch,
                    _ => Focus::Filter,
                };
                if self.mode == Mode::Phone {
                    self.page_detail = self.focus == Focus::LogSearch;
                }
            }
            Key::Char('n') => self.open_ns(),
            Key::Char('k') => self.open_kinds(),
            Key::Char('l') => {
                self.set_tab(DTab::Logs);
                if self.det.tab == DTab::Logs {
                    self.focus = Focus::Detail;
                    self.page_detail = self.mode == Mode::Phone || self.page_detail;
                }
            }
            Key::Char('y') => self.set_tab(DTab::Yaml),
            Key::Char('s') => self.set_tab(DTab::Summary),
            Key::Char('e') => self.set_tab(DTab::Events),
            Key::Char('o') => self.set_tab(DTab::Related),
            Key::Char('L') => self.show_log = !self.show_log,
            Key::Char('c') if self.show_log => self.copy_cluster_log(),
            Key::Char('c') => self.copy_view(),
            Key::Char('x') => self.copy_exec(),
            Key::Char('w') => self.det.view.wrap = !self.det.view.wrap,
            Key::Char('f') if self.det.tab == DTab::Logs => {
                self.det.logs.follow = !self.det.logs.follow;
                if self.det.logs.follow {
                    self.det.view.end();
                }
            }
            Key::Char('p') if self.det.tab == DTab::Logs => self.det.logs.paused = !self.det.logs.paused,
            Key::Tab => self.cycle_focus(1),
            Key::BackTab => self.cycle_focus(-1),
            Key::Left => match self.focus {
                Focus::Detail => self.focus = Focus::Res,
                Focus::Res if self.mode == Mode::Wide => self.focus = Focus::Ns,
                _ => {}
            },
            Key::Right => match self.focus {
                Focus::Res if self.mode != Mode::Phone => self.focus = Focus::Detail,
                Focus::Ns | Focus::Kinds => self.focus = Focus::Res,
                _ => {}
            },
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Nav { .. } => self.move_key(k, n_res),
            Key::Enter => match self.focus {
                Focus::Ns => {
                    let i = self.ns.sel;
                    self.pick_ns(i);
                }
                Focus::Kinds => {
                    let i = self.kinds.sel;
                    self.pick_kind(i);
                }
                Focus::Res => self.open_row(),
                _ => {}
            },
            _ => {}
        }
        self.sync_selection();
        Flow::Stay
    }

    fn cycle_focus(&mut self, d: i32) {
        let order: Vec<Focus> = if self.mode == Mode::Wide { vec![Focus::Ns, Focus::Kinds, Focus::Res, Focus::Detail] } else { vec![Focus::Res, Focus::Detail] };
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        self.focus = order[(i + d).rem_euclid(order.len() as i32) as usize];
    }

    fn move_key(&mut self, k: &Key, n_res: usize) {
        let (up, down, pgup, pgdn, home, end) = match k {
            Key::Up => (true, false, false, false, false, false),
            Key::Down => (false, true, false, false, false, false),
            Key::PageUp => (false, false, true, false, false, false),
            Key::PageDown => (false, false, false, true, false, false),
            Key::Home => (false, false, false, false, true, false),
            Key::End => (false, false, false, false, false, true),
            Key::Nav { nav, .. } => match nav {
                Nav::Up => (true, false, false, false, false, false),
                Nav::Down => (false, true, false, false, false, false),
                Nav::PageUp => (false, false, true, false, false, false),
                Nav::PageDown => (false, false, false, true, false, false),
                Nav::Home => (false, false, false, false, true, false),
                Nav::End => (false, false, false, false, false, true),
                _ => return,
            },
            _ => return,
        };
        match self.focus {
            Focus::Res => {
                let by = if pgup || pgdn { self.res.page() } else { 1 };
                if up || pgup {
                    self.res.up(n_res, by);
                } else if down || pgdn {
                    self.res.down(n_res, by);
                } else if home {
                    self.res.sel = 0;
                    self.res.reveal = true;
                } else if end {
                    self.res.sel = n_res.saturating_sub(1);
                    self.res.reveal = true;
                }
            }
            Focus::Ns => {
                let n = self.ns_rows().len();
                list_key(&mut self.ns, n, (up, down, pgup, pgdn, home, end));
            }
            Focus::Kinds => list_key(&mut self.kinds, Kind::ALL.len(), (up, down, pgup, pgdn, home, end)),
            Focus::Detail => {
                let page = self.det.view.fit.saturating_sub(1).max(1) as i32;
                if up {
                    self.det.view.scroll(-1);
                } else if down {
                    self.det.view.scroll(1);
                } else if pgup {
                    self.det.view.scroll(-page);
                } else if pgdn {
                    self.det.view.scroll(page);
                } else if home {
                    self.det.view.home();
                } else if end {
                    self.det.view.end();
                }
            }
            _ => {}
        }
    }

    fn field_key(&mut self, k: &Key) -> Flow {
        let which = self.focus;
        let f = match which {
            Focus::Filter => &mut self.filter,
            Focus::NsFilter => &mut self.ns_filter,
            _ => &mut self.det.logs.search,
        };
        match k {
            Key::Esc => {
                f.clear();
                self.focus = if which == Focus::NsFilter && self.overlay.is_none() { Focus::Ns } else { Focus::Res };
                if which == Focus::NsFilter && self.overlay.is_some() {
                    self.overlay = None;
                }
                self.res.reset();
            }
            Key::Enter => {
                match which {
                    Focus::NsFilter => {
                        let i = self.ns.sel.min(self.ns_rows().len().saturating_sub(1));
                        self.pick_ns(i);
                    }
                    Focus::LogSearch => self.focus = Focus::Detail,
                    _ => self.focus = Focus::Res,
                }
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown if which == Focus::NsFilter => {
                let n = self.ns_rows().len();
                list_key(&mut self.ns, n, (*k == Key::Up, *k == Key::Down, *k == Key::PageUp, *k == Key::PageDown, false, false));
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown if which == Focus::Filter => {
                // the list keeps moving while the filter is being typed
                let n = self.shown_rows();
                list_key(&mut self.res, n, (*k == Key::Up, *k == Key::Down, *k == Key::PageUp, *k == Key::PageDown, false, false));
                self.sync_selection();
            }
            other => {
                f.handle(other);
                if which != Focus::LogSearch {
                    self.res.reset();
                    if which == Focus::NsFilter {
                        self.ns.reset();
                    }
                }
            }
        }
        Flow::Stay
    }

    fn overlay_key(&mut self, o: Overlay, k: &Key) -> Flow {
        match (o, k) {
            (_, Key::Esc) => self.close_overlay(),
            (Overlay::Clusters, Key::Up) => list_key(&mut self.clusters, self.entries.len(), (true, false, false, false, false, false)),
            (Overlay::Clusters, Key::Down) => list_key(&mut self.clusters, self.entries.len(), (false, true, false, false, false, false)),
            (Overlay::Clusters, Key::Enter) => {
                let i = self.clusters.sel;
                self.overlay = None;
                self.select_cluster(i, true);
            }
            (Overlay::Kinds, Key::Up) => list_key(&mut self.kinds, Kind::ALL.len(), (true, false, false, false, false, false)),
            (Overlay::Kinds, Key::Down) => list_key(&mut self.kinds, Kind::ALL.len(), (false, true, false, false, false, false)),
            (Overlay::Kinds, Key::PageUp) => list_key(&mut self.kinds, Kind::ALL.len(), (false, false, true, false, false, false)),
            (Overlay::Kinds, Key::PageDown) => list_key(&mut self.kinds, Kind::ALL.len(), (false, false, false, true, false, false)),
            (Overlay::Kinds, Key::Enter) => {
                let i = self.kinds.sel;
                self.pick_kind(i);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn card_key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        let Some(p) = &mut self.pending else { return Flow::Stay };
        let typed = matches!(p.action.confirm(), Confirm::TypeName(_));
        match k {
            Key::Esc => self.pending = None,
            Key::Enter => self.confirm_action(cx),
            Key::Char('y') if !typed && p.scaling.is_none() => self.confirm_action(cx),
            Key::Char('n') if !typed => self.pending = None,
            Key::Char('+') | Key::Up if p.scaling.is_some() => p.replicas = (p.replicas + 1).min(kx::MAX_REPLICAS),
            Key::Char('-') | Key::Down if p.scaling.is_some() => p.replicas = p.replicas.saturating_sub(1),
            Key::PageUp if p.scaling.is_some() => p.replicas = (p.replicas + 10).min(kx::MAX_REPLICAS),
            Key::PageDown if p.scaling.is_some() => p.replicas = p.replicas.saturating_sub(10),
            other if typed => {
                p.typed.handle(other);
            }
            _ => {}
        }
        Flow::Stay
    }
}

fn list_key(l: &mut ListState, n: usize, (up, down, pgup, pgdn, home, end): (bool, bool, bool, bool, bool, bool)) {
    if up {
        l.up(n, 1);
    } else if down {
        l.down(n, 1);
    } else if pgup {
        let by = l.page();
        l.up(n, by);
    } else if pgdn {
        let by = l.page();
        l.down(n, by);
    } else if home {
        l.sel = 0;
        l.reveal = true;
    } else if end {
        l.sel = n.saturating_sub(1);
        l.reveal = true;
    }
}

impl App for K8sApp {
    fn title(&self) -> &'static str {
        "K8S"
    }
    fn icon(&self) -> Icon {
        Icon::Mesh
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "\u{2191}\u{2193} move \u{b7} Enter open \u{b7} / filter \u{b7} n namespaces \u{b7} k kinds \u{b7} l logs \u{b7} y YAML \u{b7} r refresh \u{b7} Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        vec![
            "Up, Down, PageUp, PageDown, Home, End: move in the list that has the focus (Tab changes it)".into(),
            "Enter: open the selected object; in the namespace or kind list: choose it".into(),
            "/: filter the list (Esc clears the filter)".into(),
            "n: namespaces \u{b7} k: kinds of resources".into(),
            "s Summary \u{b7} y YAML \u{b7} e Events \u{b7} o Related \u{b7} l Logs of a pod".into(),
            "Logs: f follow \u{b7} p pause \u{b7} w wrap \u{b7} / search".into(),
            "c: copy what the details show \u{b7} x: copy the kubectl exec command of a pod".into(),
            "r: refresh (also ends a wait between retries) \u{b7} L: cluster log of kubectl problems (c copies it) \u{b7} Esc: back".into(),
            "Changes (restart, scale, delete, cordon) need the Allow changes switch, which only administrators have".into(),
        ]
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.handle_event(ev, cx)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        vec![crate::ctxmenu::Item::new("k8s.copy", "Copy what the details show", "").enabled(!self.det.view.lines.is_empty())]
    }
    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> Flow {
        if id == "k8s.copy" {
            self.copy_view();
        }
        Flow::Stay
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.draw(g, area, s, cx, tick);
    }
}

#[cfg(test)]
#[path = "k8s_tile_tests.rs"]
mod tests;
