//! Platform services (docs/PLATFORM-SERVICES.md): the list of services (Gitea, the private store, the registry) with their state, and the
//! deploy wizard: choose a cluster (with the badges and the reasons a cluster is refused), choose the options (a form made from the
//! service's schema), review the exact plan, approve, watch the progress, read the result. Open, Stop/Start, Upgrade, Backup, Remove,
//! Details, Connect to an existing service and Connect a workspace use the same review-and-approve screens.
//!
//! One panel, three homes: Settings → Dev cluster ("Platform services…"), STORE → Settings (the "Private store" card calls
//! [`PlatformPanel::open_wizard`] with the store's service id) and the AI IDE's K8S panel ("Services…"). While it is open it owns every key
//! and click of its host. Nothing runs without the person's approval on a review screen; no secret is ever drawn except in the masked
//! reveal dialog, which shows it once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_agent::devcluster::{self, ClusterProfile};
use iyakki_agent::platform::connect::{self, ConnectOpts, ConnectPlan, PushPlan, WsInfo};
use iyakki_agent::platform::localbin::{self, Arch, BuildPlan, BuildState, Discovery, LocalBinary, Places};
use iyakki_agent::platform::{self, installed, Zeroizing, Applied, Installed, ListedProfile, OptKind, OptionDef, Options, Plan, Platform, RemovePlan, UpgradePlan};
use iyakki_agent::services::Services;
use iyakki_agent::store::privatemanage as pm;
use iyakki_agent::store::privatestore;
use iyakki_agent::store::Store;
use iyakki_agent::localreg::{self, Dependency, Target};
use iyakki_agent::quickinstall;
use iyakki_agent::workspace::Registry as WsRegistry;


use crate::apps::ScrollLog;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{clipboard, Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(120, 170, 255);
const AMBER: [u8; 4] = rgb(255, 184, 77);

/// What the panel asks its host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PWant {
    /// Show this address in a browser (the IDE's Browser tab; elsewhere the address is copied).
    OpenUrl(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    List,
    Cluster,
    Options,
    Review,
    Progress,
    Result,
    Confirm,
    Existing,
    Workspaces,
    /// Type the path of a prebuilt static program for a service that needs one.
    Choose,
    /// The private store's state, with Repair, Reset and reinstall, and Remove.
    Store,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RowKind {
    Spec(String),
    Inst(String),
    Registry,
}

struct ListRow {
    kind: RowKind,
    title: String,
    sub: String,
    chips: Vec<(String, [u8; 4])>,
}

struct OptField {
    def: OptionDef,
    tf: TextField,
    /// For a choice or a flag: the words and the current one.
    choices: Option<(Vec<String>, usize)>,
}

enum Act {
    Power(Installed, bool),
    Upgrade(Box<UpgradePlan>),
    Remove(RemovePlan),
    Backup(Installed),
    Connect(Box<ConnectPlan>),
    Push(Box<PushPlan>),
    /// Build the program a local image needs, as a detached job.
    Build(Box<BuildPlan>),
    /// Create the local image registry as a background job; `resume`: continue the deploy when it is ready.
    CreateRegistry { target: Target, size_gi: u32, resume: bool },
    /// Delete the registry's namespace (typed name).
    RemoveRegistry(Target),
    /// Repair the private store (no typing: nothing is deleted).
    StoreRepair(Box<pm::Report>),
    /// Reset or remove the private store (the namespace typed); `force` also clears finalizers if the namespace sticks.
    StoreDo { rep: Box<pm::Report>, action: pm::Action, force: bool },
    Close,
}

/// The reset or removal job being followed.
struct StoreRun {
    id: String,
    action: pm::Action,
    rep: Box<pm::Report>,
}

/// The registry job in flight.
struct RegRun {
    id: String,
    target: Target,
    resume: bool,
}

/// Why a deploy stopped for want of a locally built program, and what can be done about it (the buttons of the result card).
struct FailCard {
    lb: LocalBinary,
    disc: Discovery,
    places: Places,
}

/// A build running as a job; the deploy continues when it succeeds.
struct BuildRun {
    id: String,
    plan: BuildPlan,
    lb: LocalBinary,
    progress: Option<f32>,
}

struct Confirm {
    title: String,
    log: ScrollLog,
    act: Act,
    blocked: String,
    typed: Option<TextField>,
    danger: bool,
    busy: bool,
    done: bool,
}

struct RevealView {
    label: String,
    vault: String,
    value: Zeroizing<String>,
    shown: bool,
    copied: bool,
}

enum Done {
    Profiles(Vec<ListedProfile>),
    Plan(Result<Box<Plan>, String>),
    Applied(Result<Applied, String>),
    Status(HashMap<String, String>, Option<bool>),
    UpgradePlan(Result<Box<UpgradePlan>, String>),
    ConnectPlan(Result<Box<ConnectPlan>, String>),
    PushPlan(Result<Box<PushPlan>, String>),
    Action(Result<Vec<String>, String>),
    Existing(Result<Installed, String>),
    /// The registry of the default cluster: where, what exists, and whether it can be created.
    RegState(Result<(Target, localreg::State, Dependency), String>),
    /// What state the private store is in.
    StoreState(Box<pm::Report>),
    /// A reset or removal passed its checks: the job's arguments.
    StoreReady(Result<(Box<pm::Report>, pm::Action, pm::JobArgs, bool), String>),
    /// This device's side of a reset or removal is cleaned (lines), and whether to deploy again.
    StoreLocal(Result<Vec<String>, String>, bool),
}

struct Wizard {
    service: String,
    profiles: Vec<ListedProfile>,
    loading: bool,
    psel: usize,
    fields: Vec<OptField>,
    osel: usize,
    plan: Option<Box<Plan>>,
    chosen: Option<ClusterProfile>,
    /// Opened to look after the private store, not to deploy it.
    manage: bool,
    /// The cluster to preselect (`kubeconfig`, `context`).
    prefer: Option<(String, String)>,
}

pub struct PlatformPanel {
    pl: Arc<Platform>,
    page: Page,
    open: bool,
    rows: Vec<ListRow>,
    sel: usize,
    msg: Option<(String, bool)>,
    inbox: Arc<Mutex<Vec<Done>>>,
    busy: Arc<AtomicUsize>,
    statuses: HashMap<String, String>,
    registry_up: Option<bool>,
    wiz: Option<Wizard>,
    log: ScrollLog,
    progress: Arc<Mutex<Vec<String>>>,
    live: Arc<Mutex<String>>,
    confirm: Option<Confirm>,
    reveal: Vec<RevealView>,
    url: TextField,
    token: TextField,
    existing_focus: usize,
    wss: Vec<(String, std::path::PathBuf)>,
    wsel: usize,
    /// The installed service a workspace is being connected to, and a workspace to connect without asking (the IDE's).
    connect_to: Option<String>,
    fixed_ws: Option<WsInfo>,
    wants: Vec<PWant>,
    last_result: Vec<String>,
    result_urls: Vec<(String, String)>,
    loaded: bool,
    /// The approval in flight is the connect step: when it works, the push is offered as its own approval.
    after_connect: bool,
    /// Rows scrolled off the top of the visible list, per page.
    first: usize,
    tick: u64,
    /// The jobs engine, for "Build now" (the host gives it; without it the button is not offered).
    svc: Option<Arc<Services>>,
    fail: Option<FailCard>,
    build: Option<BuildRun>,
    /// A harmless command in place of the real build (tests).
    build_override: Option<Vec<String>>,
    /// The registry job being followed, and whether a failed one can be retried (with "continue the deploy" or not).
    reg_run: Option<RegRun>,
    reg_retry: Option<(Target, u32, bool)>,
    /// The deploy continues by itself when the plan made after the registry is ready can be applied.
    auto_deploy: bool,
    /// How the registry job is launched (tests give a script).
    reg_launch: Option<quickinstall::Launch>,
    choose: TextField,
    choose_err: String,
    show_docs: bool,
    result_text: String,
    /// The private store's report on the Store page, and the reset or removal job being followed.
    store_rep: Option<pm::Report>,
    store_run: Option<StoreRun>,
    /// After a store action's result is closed, look at the store again.
    store_after: bool,
    /// A reset that stuck in Terminating can be tried again clearing finalizers.
    store_retry: Option<Box<pm::Report>>,
}

/// The review text of a store action: the agent's exact list plus the choices of this screen.
fn store_confirm_text(rep: &pm::Report, action: &pm::Action, force: bool) -> String {
    let mut t = pm::preview(rep, action);
    if matches!(action, pm::Action::Reset) {
        t.push_str(&format!("
Ctrl+F: if the namespace sticks in Terminating, also clear the finalizers of what is left in it (now: {}).", if force { "ON" } else { "off" }));
    }
    t
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lines_of(text: &str) -> ScrollLog {
    let mut l = ScrollLog { nowrap: false, ..ScrollLog::default() };
    for line in text.lines() {
        let col = if line.starts_with("CANNOT") || line.starts_with("WARNING") && line.contains("DELETED") {
            ERR
        } else if line.starts_with("WARNINGS") || line.starts_with("warning") || line.starts_with("- ") && false {
            AMBER
        } else if line.len() > 3 && line.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii_uppercase() || c == ' ') && line.starts_with(|c: char| c.is_ascii_uppercase()) {
            ACCENT
        } else {
            [0, 0, 0, 0]
        };
        l.push(line, col);
    }
    l.scroll = 1_000_000; // clamped on draw: starts at the top
    l
}

impl PlatformPanel {
    pub fn new(pl: Arc<Platform>) -> PlatformPanel {
        PlatformPanel {
            pl,
            page: Page::List,
            open: true,
            rows: vec![],
            sel: 0,
            msg: None,
            inbox: Arc::default(),
            busy: Arc::default(),
            statuses: HashMap::new(),
            registry_up: None,
            wiz: None,
            log: ScrollLog::default(),
            progress: Arc::default(),
            live: Arc::default(),
            confirm: None,
            reveal: vec![],
            url: TextField::new("http://192.168.1.10:3000", false),
            token: TextField::new("an access token with the repository write scope", true),
            existing_focus: 0,
            wss: vec![],
            wsel: 0,
            connect_to: None,
            fixed_ws: None,
            wants: vec![],
            last_result: vec![],
            result_urls: vec![],
            loaded: false,
            after_connect: false,
            first: 0,
            tick: 0,
            svc: None,
            fail: None,
            build: None,
            build_override: None,
            reg_run: None,
            reg_retry: None,
            auto_deploy: false,
            reg_launch: None,
            choose: TextField::new("/path/to/iyakki-store-server-x86_64", false),
            choose_err: String::new(),
            show_docs: false,
            result_text: String::new(),
            store_rep: None,
            store_run: None,
            store_after: false,
            store_retry: None,
        }
    }

    /// Give the panel the jobs engine so "Build now" can start a detached build job.
    pub fn with_services(mut self, svc: Arc<Services>) -> PlatformPanel {
        self.svc = Some(svc);
        self
    }

    /// Launch the registry job with this program (tests give a script).
    pub fn with_registry_launch(mut self, l: quickinstall::Launch) -> PlatformPanel {
        self.reg_launch = Some(l);
        self
    }

    /// Replace the build command (tests give a harmless one).
    pub fn with_build_command(mut self, argv: Vec<String>) -> PlatformPanel {
        self.build_override = Some(argv);
        self
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) > 0
    }

    pub fn take_wants(&mut self) -> Vec<PWant> {
        std::mem::take(&mut self.wants)
    }

    /// The text of the review, result or details screen (tests and accessibility).
    pub fn text(&self) -> String {
        let l = match (&self.page, &self.confirm) {
            (Page::Confirm, Some(c)) => &c.log,
            _ => &self.log,
        };
        l.lines.iter().map(|x| x.0.clone()).collect::<Vec<_>>().join("\n")
    }

    pub fn message(&self) -> String {
        self.msg.as_ref().map(|m| m.0.clone()).unwrap_or_default()
    }

    pub fn reveal_open(&self) -> bool {
        !self.reveal.is_empty()
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.msg = Some((t.into(), ok));
    }

    fn spawn(&self, f: impl FnOnce(&Platform) -> Done + Send + 'static) {
        let (pl, inbox, busy) = (self.pl.clone(), self.inbox.clone(), self.busy.clone());
        busy.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let d = f(&pl);
            lock(&inbox).push(d);
            busy.fetch_sub(1, Ordering::SeqCst);
        });
    }

    // ------------------------------------------------------------------ the list

    /// Re-read the installed services and (in the background) their live state.
    pub fn reload(&mut self) {
        self.loaded = true;
        let inst = installed::load(&self.pl.dir, &self.pl.user);
        let mut rows = Vec::new();
        for d in platform::specs() {
            let mine: Vec<&Installed> = inst.iter().filter(|i| i.service == d.id()).collect();
            let mut chips = vec![(d.kind().to_string(), MUTED)];
            chips.push(if mine.is_empty() { ("not installed".into(), DIM) } else { (format!("{} installed", mine.len()), OK) });
            rows.push(ListRow { kind: RowKind::Spec(d.id().into()), title: d.title(), sub: d.card(), chips });
            for i in mine {
                let live = self.statuses.get(&i.id).cloned().unwrap_or_else(|| "checking…".into());
                let mut chips = vec![(live.clone(), if live.starts_with("running") || live.starts_with("connected") { OK } else if live.starts_with("checking") { DIM } else if live.starts_with("stopped") { MUTED } else { AMBER }), (i.version.clone(), MUTED)];
                if i.connected {
                    chips.push(("existing".into(), AMBER));
                }
                let place = i.cluster.as_ref().map(|c| format!("{} · {}", c.name, i.namespace)).unwrap_or_else(|| "outside the cluster".into());
                rows.push(ListRow { kind: RowKind::Inst(i.id.clone()), title: format!("   {}", i.id), sub: format!("   {place} · {}", i.web_url().unwrap_or("no address")), chips });
            }
        }
        let reg = match self.registry_up {
            Some(true) => ("installed".to_string(), OK),
            Some(false) => ("not installed".to_string(), DIM),
            None => ("checking…".to_string(), DIM),
        };
        rows.push(ListRow { kind: RowKind::Registry, title: "Registry".into(), sub: "where images built on this device go; Enter creates it in one step (or shows it and offers Remove)".into(), chips: vec![("registry".into(), MUTED), reg] });
        self.rows = rows;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        let ids: Vec<Installed> = inst;
        self.spawn(move |pl| {
            let mut m = HashMap::new();
            for i in &ids {
                m.insert(i.id.clone(), platform::status(pl, i, false).line());
            }
            let reg = devcluster::Choice::load(&pl.dir, &pl.user).map(|c| {
                let kc = devcluster::Kc::new(pl.run.clone(), &c.kubeconfig, &c.context);
                kc.read(&["get", "service", iyakki_agent::regsetup::IN_NAME, "-n", iyakki_agent::regsetup::IN_NS, "-o", "name"]).ok()
            });
            Done::Status(m, reg)
        });
    }

    fn current(&self) -> Option<&ListRow> {
        self.rows.get(self.sel)
    }

    fn current_inst(&self) -> Option<Installed> {
        match self.current()?.kind.clone() {
            RowKind::Inst(id) => installed::find(&self.pl.dir, &self.pl.user, &id),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ entry points

    /// Start the deploy wizard for a service (the STORE's "Private store" card and the list's button).
    pub fn open_wizard(&mut self, service: &str) {
        self.open = true;
        if !self.loaded {
            self.reload();
        }
        let Some(def) = platform::spec(service) else {
            self.say(format!("There is no platform service called {service}."), false);
            return;
        };
        self.wiz = Some(Wizard { service: service.into(), profiles: vec![], loading: true, psel: 0, fields: vec![], osel: 0, plan: None, chosen: None, manage: false, prefer: None });
        self.page = Page::Cluster;
        self.msg = None;
        let _ = def;
        self.spawn(|pl| Done::Profiles(platform::list_profiles(pl)));
    }

    /// Look after the private store (the STORE's "Repair, reset or remove" row and the list's button): straight to its state when this
    /// device knows which cluster it is in, otherwise choose the cluster first.
    pub fn open_store_manage(&mut self) {
        self.open = true;
        if !self.loaded {
            self.reload();
        }
        let known = installed::load(&self.pl.dir, &self.pl.user).into_iter().find(|i| i.service == privatestore::ID && !i.connected && i.cluster.is_some());
        match known {
            Some(i) => {
                let c = i.cluster.clone().unwrap_or_else(|| unreachable!());
                self.wiz = Some(Wizard { service: privatestore::ID.into(), profiles: vec![], loading: false, psel: 0, fields: vec![], osel: 0, plan: None, chosen: None, manage: true, prefer: None });
                self.detect_store(c, i.namespace);
            }
            None => {
                self.open_wizard(privatestore::ID);
                if let Some(w) = self.wiz.as_mut() {
                    w.manage = true;
                }
            }
        }
    }

    /// Open on the list, optionally with the connect-a-workspace flow for one workspace (the IDE).
    pub fn open_list(&mut self) {
        self.open = true;
        self.page = Page::List;
        self.reload();
    }

    /// The IDE's "Connect workspace to Gitea": for this workspace and (if several) the git service selected in the list.
    pub fn open_connect_workspace(&mut self, ws: WsInfo) {
        self.open = true;
        self.reload();
        self.fixed_ws = Some(ws);
        let git: Vec<Installed> = installed::load(&self.pl.dir, &self.pl.user).into_iter().filter(|i| i.kind == "git").collect();
        match git.len() {
            0 => {
                self.page = Page::List;
                self.say("No git service is installed yet: deploy Gitea (Deploy to k3s…) or connect an existing one first.", false);
            }
            1 => {
                self.connect_to = Some(git[0].id.clone());
                self.start_connect();
            }
            _ => {
                self.page = Page::List;
                if let Some(i) = self.rows.iter().position(|r| matches!(&r.kind, RowKind::Inst(id) if git.iter().any(|g| g.id == *id))) {
                    self.sel = i;
                }
                self.say("Select the git service and press w to connect this workspace to it.", true);
            }
        }
    }

    // ------------------------------------------------------------------ events

    /// Every key and click while the panel is open. Returns false when the host should close it (it is closed).
    pub fn handle(&mut self, ev: &Event) -> bool {
        if matches!(ev, Event::Tick) {
            self.tick += 1;
            self.poll();
            if let Some(c) = self.confirm.as_mut() {
                c.log.on_tick();
            }
            self.log.on_tick();
            return self.open;
        }
        if !self.reveal.is_empty() {
            self.reveal_event(ev);
            return self.open;
        }
        match self.page.clone() {
            Page::List => self.list_event(ev),
            Page::Cluster => self.cluster_event(ev),
            Page::Options => self.options_event(ev),
            Page::Review => self.review_event(ev),
            Page::Progress => {
                if self.build.is_some() && matches!(ev, Event::Key(Key::Esc)) {
                    self.stop_build();
                } else if self.store_run.is_some() && matches!(ev, Event::Key(Key::Esc)) {
                    // hide: the job keeps going and its result appears when it ends
                    self.page = if self.store_rep.is_some() { Page::Store } else { Page::List };
                } else if self.reg_run.is_some() && matches!(ev, Event::Key(Key::Esc)) {
                    // hide: the job keeps going and the deploy continues when it ends
                    self.page = if self.wiz.is_some() { Page::Review } else { Page::List };
                } else {
                    self.log.handle(ev);
                }
            }
            Page::Result => self.result_event(ev),
            Page::Confirm => self.confirm_event(ev),
            Page::Existing => self.existing_event(ev),
            Page::Workspaces => self.workspaces_event(ev),
            Page::Choose => self.choose_event(ev),
            Page::Store => self.store_event(ev),
        }
        self.open
    }

    fn scroll_log(log: &mut ScrollLog, ev: &Event) -> bool {
        match ev {
            Event::Key(Key::Up) => log.scroll_by(1),
            Event::Key(Key::Down) => log.scroll_by(-1),
            Event::Key(Key::PageUp) => log.scroll_by(10),
            Event::Key(Key::PageDown) => log.scroll_by(-10),
            Event::Scroll(d) => log.scroll_by(*d * 3),
            other => return log.handle(other),
        }
        true
    }

    fn move_sel(sel: &mut usize, d: i32, len: usize) {
        if len > 0 {
            *sel = (*sel as i32 + d).clamp(0, len as i32 - 1) as usize;
        }
    }

    fn list_event(&mut self, ev: &Event) {
        let len = self.rows.len();
        match ev {
            Event::Row(i) if *i < len => {
                if self.sel == *i && matches!(self.current().map(|r| &r.kind), Some(RowKind::Inst(_))) {
                    return self.details();
                }
                self.sel = *i;
            }
            Event::Scroll(d) => Self::move_sel(&mut self.sel, -*d, len),
            Event::Key(k) => match k {
                Key::Up => Self::move_sel(&mut self.sel, -1, len),
                Key::Down => Self::move_sel(&mut self.sel, 1, len),
                Key::Esc => self.open = false,
                Key::Enter => match self.current().map(|r| r.kind.clone()) {
                    Some(RowKind::Inst(_)) => self.details(),
                    Some(RowKind::Spec(id)) => self.open_wizard(&id),
                    Some(RowKind::Registry) => self.registry_row(),
                    _ => {}
                },
                Key::Char('d') => match self.current().map(|r| r.kind.clone()) {
                    Some(RowKind::Spec(id)) => self.open_wizard(&id),
                    Some(RowKind::Inst(_)) => {
                        let svc = self.current_inst().map(|i| i.service).unwrap_or_default();
                        self.open_wizard(&svc)
                    }
                    _ => self.registry_row(),
                },
                Key::Char('g') => self.registry_row(),
                Key::Char('r') => {
                    self.say("Looking again…", true);
                    self.statuses.clear();
                    self.registry_up = None;
                    self.reload();
                }
                Key::Char('c') => self.existing_form(),
                Key::Char('o') => self.open_selected(),
                Key::Char('s') => self.power_selected(),
                Key::Char('u') => self.upgrade_selected(),
                Key::Char('b') => self.backup_selected(),
                Key::Char('x') => self.remove_selected(),
                Key::Char('m') => self.manage_selected(),
                Key::Char('v') => self.reveal_vault(),
                Key::Char('w') => {
                    if let Some(i) = self.current_inst() {
                        self.connect_to = Some(i.id);
                        self.pick_workspace();
                    } else {
                        self.say("Select an installed git service first.", false);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn open_selected(&mut self) {
        let Some(i) = self.current_inst() else { return self.say("Select an installed service to open it.", false) };
        match i.web_url() {
            Some(u) => {
                let u = u.to_string();
                clipboard::copy(&u);
                self.say(format!("Opening {u} (the address is also copied)."), true);
                self.wants.push(PWant::OpenUrl(u));
            }
            None => self.say("This service has no web address.", false),
        }
    }

    fn details(&mut self) {
        let Some(i) = self.current_inst() else { return };
        let mut t = format!("{} · {} {}\n", i.id, i.title, i.version);
        t.push_str(&format!("State: {}\n", self.statuses.get(&i.id).cloned().unwrap_or_else(|| "checking…".into())));
        t.push_str(&match &i.cluster {
            Some(c) => format!("Cluster: {}\nNamespace: {}{}\n", c.line(), i.namespace, if i.created_namespace { " (created by IYAKKI)" } else { "" }),
            None => "Cluster: none (an existing service you connected; IYAKKI runs nothing for it)\n".into(),
        });
        t.push_str("\nADDRESSES\n");
        for u in &i.urls {
            t.push_str(&format!("{}: {}\n", u.label, u.url));
        }
        if !i.vault.is_empty() {
            t.push_str("\nCREDENTIALS (values are only in your vault and the cluster's Secret; press v to reveal)\n");
            for v in &i.vault {
                t.push_str(&format!("uvault:{v}\n"));
            }
        }
        if !i.workspaces.is_empty() {
            t.push_str("\nCONNECTED WORKSPACES\n");
            for w in &i.workspaces {
                t.push_str(&format!("{} → {}\n", w.workspace, w.repo_url));
            }
        }
        if let Some(d) = platform::spec(&i.service) {
            t.push_str(&format!("\nUPGRADES: {}\nBACKUP: {}\n", if i.connected { "n/a".to_string() } else { let u = d.upgrade_path(&i.version); if u.is_empty() { "none newer than this version".to_string() } else { u.join(", ") } }, d.backup_hint()));
            t.push_str(&format!("\nHOW TO USE\n{}\n", d.how_to_use(&i.urls, &i.options)));
        }
        if !i.note.is_empty() {
            t.push_str(&format!("\nNote: {}\n", i.note));
        }
        self.confirm = Some(Confirm { title: format!("Details: {}", i.id), log: lines_of(&t), act: Act::Close, blocked: String::new(), typed: None, danger: false, busy: false, done: false });
        self.page = Page::Confirm;
    }

    fn reveal_vault(&mut self) {
        let Some(i) = self.current_inst().or_else(|| self.connect_to.as_ref().and_then(|id| installed::find(&self.pl.dir, &self.pl.user, id))) else { return self.say("Select an installed service first.", false) };
        let names: Vec<&String> = i.vault.iter().filter(|v| v.ends_with(".admin-password")).collect();
        if names.is_empty() {
            return self.say("This service has no generated password to reveal (a connected service keeps only a token, which is never shown).", false);
        }
        for n in names {
            match self.pl.vault.get(n) {
                Ok(v) => self.reveal.push(RevealView { label: format!("Admin password of {}", i.id), vault: n.clone(), value: v, shown: false, copied: false }),
                Err(e) => self.say(format!("Could not read the vault: {e}"), false),
            }
        }
    }

    fn power_selected(&mut self) {
        let Some(i) = self.current_inst() else { return self.say("Select an installed service first.", false) };
        let stopped = self.statuses.get(&i.id).is_some_and(|s| s.starts_with("stopped"));
        let text = platform::preview_power(&i, stopped);
        self.show_confirm(&format!("{} {}: review, then approve", if stopped { "Start" } else { "Stop" }, i.id), &text, Act::Power(i, stopped), String::new(), None, !stopped);
    }

    fn backup_selected(&mut self) {
        let Some(i) = self.current_inst() else { return self.say("Select an installed service first.", false) };
        let text = platform::preview_backup(&i, true);
        self.show_confirm(&format!("Back up {}: review, then approve", i.id), &text, Act::Backup(i), String::new(), None, false);
    }

    /// The list's "Repair or reset…" on a private store (or its row).
    fn manage_selected(&mut self) {
        let is_store = match self.current().map(|r| r.kind.clone()) {
            Some(RowKind::Spec(id)) => id == privatestore::ID,
            Some(RowKind::Inst(_)) => self.current_inst().is_some_and(|i| i.service == privatestore::ID),
            _ => false,
        };
        if !is_store {
            return self.say("Select the private store (or its row) first.", false);
        }
        match self.current_inst().filter(|i| i.cluster.is_some() && !i.connected) {
            Some(i) => {
                let c = i.cluster.clone().unwrap_or_else(|| unreachable!());
                self.wiz = Some(Wizard { service: privatestore::ID.into(), profiles: vec![], loading: false, psel: 0, fields: vec![], osel: 0, plan: None, chosen: None, manage: true, prefer: None });
                self.detect_store(c, i.namespace);
            }
            None => self.open_store_manage(),
        }
    }

    fn remove_selected(&mut self) {
        let Some(i) = self.current_inst() else { return self.say("Select an installed service first.", false) };
        let r = platform::plan_remove(&i, true);
        let text = format!("{}\n\nTo remove it and KEEP its data, clear the checkbox with Ctrl+K before approving.\nTo approve, type the service id ({}) below.", platform::preview_remove(&r), i.id);
        let mut tf = TextField::new(&i.id, false);
        tf.max = 64;
        self.show_confirm(&format!("Remove {}: review, then approve", i.id), &text, Act::Remove(r), String::new(), Some(tf), true);
    }

    fn upgrade_selected(&mut self) {
        let Some(i) = self.current_inst() else { return self.say("Select an installed service first.", false) };
        self.say("Reading the cluster for the upgrade plan…", true);
        self.spawn(move |pl| Done::UpgradePlan(platform::plan_upgrade(pl, &i, None, true).map(Box::new)));
    }

    fn show_confirm(&mut self, title: &str, text: &str, act: Act, blocked: String, typed: Option<TextField>, danger: bool) {
        self.confirm = Some(Confirm { title: title.into(), log: lines_of(text), act, blocked, typed, danger, busy: false, done: false });
        self.page = Page::Confirm;
        self.msg = None;
    }

    // ------------------------------------------------------------------ wizard: cluster

    fn cluster_event(&mut self, ev: &Event) {
        let Some(w) = self.wiz.as_mut() else { return };
        let len = w.profiles.len();
        match ev {
            Event::Row(i) if *i < len => {
                if w.psel == *i {
                    return self.cluster_next();
                }
                w.psel = *i;
            }
            Event::Scroll(d) => Self::move_sel(&mut w.psel, -*d, len),
            Event::Key(Key::Up) => Self::move_sel(&mut w.psel, -1, len),
            Event::Key(Key::Down) => Self::move_sel(&mut w.psel, 1, len),
            Event::Key(Key::Enter) => self.cluster_next(),
            Event::Key(Key::Char('r')) => {
                w.loading = true;
                w.profiles.clear();
                self.spawn(|pl| Done::Profiles(platform::list_profiles(pl)));
            }
            Event::Key(Key::Esc) => {
                self.wiz = None;
                self.page = Page::List;
            }
            _ => {}
        }
    }

    fn cluster_next(&mut self) {
        let Some(w) = self.wiz.as_mut() else { return };
        let Some(l) = w.profiles.get(w.psel).cloned() else { return };
        if let Some(r) = &l.refusal {
            let r = r.clone();
            return self.say(r, false);
        }
        w.chosen = Some(l.profile.clone());
        if w.service == privatestore::ID {
            // before anything is deployed: is there something half-installed (or broken) to repair, reset or remove first?
            let c = platform::ClusterRef::of(&l.profile);
            return self.detect_store(c, pm::DEFAULT_NAMESPACE.into());
        }
        self.build_options();
    }

    /// The options form of the chosen cluster.
    fn build_options(&mut self) {
        let Some(w) = self.wiz.as_mut() else { return };
        let Some(def) = platform::spec(&w.service) else { return };
        let defs = platform::common_options(&*def);
        w.fields = defs
            .into_iter()
            .map(|d| {
                let mut tf = TextField::new("", false);
                tf.max = 200;
                let choices = match &d.kind {
                    OptKind::Choice(c) => Some((c.clone(), c.iter().position(|x| *x == d.default).unwrap_or(0))),
                    OptKind::Bool => Some((vec!["false".into(), "true".into()], usize::from(d.default == "true"))),
                    _ => None,
                };
                if choices.is_none() {
                    tf.set(&d.default);
                }
                OptField { def: d, tf, choices }
            })
            .collect();
        w.osel = 0;
        self.page = Page::Options;
        self.msg = None;
        self.first = 0;
    }

    // ------------------------------------------------------------------ wizard: options

    fn options_values(w: &Wizard) -> Options {
        let mut o = Options::default();
        for f in &w.fields {
            let v = match &f.choices {
                Some((c, i)) => c[*i].clone(),
                None => f.tf.value.trim().to_string(),
            };
            if !v.is_empty() {
                o.set(&f.def.key, &v);
            }
        }
        o
    }

    fn options_event(&mut self, ev: &Event) {
        let Some(w) = self.wiz.as_mut() else { return };
        let len = w.fields.len();
        match ev {
            Event::Row(i) | Event::FocusAt(i, _) if *i < len => {
                w.osel = *i;
                if let (Event::FocusAt(_, x), Some(f)) = (ev, w.fields.get_mut(*i)) {
                    f.tf.click(*x);
                }
            }
            Event::Scroll(d) => Self::move_sel(&mut w.osel, -*d, len),
            Event::Paste(t) => {
                if let Some(f) = w.fields.get_mut(w.osel) {
                    if f.choices.is_none() {
                        f.tf.paste(t);
                    }
                }
            }
            Event::Key(Key::Esc) => {
                self.page = Page::Cluster;
                self.msg = None;
            }
            Event::Key(Key::Ctrl('s')) => self.options_submit(),
            Event::Key(Key::Enter) => {
                if w.osel + 1 < len {
                    w.osel += 1;
                } else {
                    self.options_submit();
                }
            }
            Event::Key(Key::Up | Key::BackTab) => Self::move_sel(&mut w.osel, -1, len),
            Event::Key(Key::Down | Key::Tab) => Self::move_sel(&mut w.osel, 1, len),
            Event::Key(k @ (Key::Left | Key::Right | Key::Char(' '))) => {
                if let Some(f) = w.fields.get_mut(w.osel) {
                    match &mut f.choices {
                        Some((c, i)) => {
                            let n = c.len();
                            *i = if *k == Key::Left { (*i + n - 1) % n } else { (*i + 1) % n };
                        }
                        None => {
                            f.tf.handle(k);
                        }
                    }
                }
            }
            Event::Key(k) => {
                if let Some(f) = w.fields.get_mut(w.osel) {
                    if f.choices.is_none() {
                        f.tf.handle(k);
                    }
                }
            }
            _ => {}
        }
    }

    fn options_submit(&mut self) {
        let Some(w) = self.wiz.as_mut() else { return };
        let (Some(p), Some(def)) = (w.chosen.clone(), platform::spec(&w.service)) else { return };
        let opts = Self::options_values(w);
        // check the form now, so a bad value is said next to the form
        if let Err(e) = platform::validate_options(&*def, &opts) {
            return self.say(e, false);
        }
        w.plan = None;
        self.page = Page::Review;
        self.log = lines_of("Reading the cluster and building the plan…");
        self.msg = None;
        let progress = self.progress.clone();
        lock(&progress).clear();
        self.spawn(move |pl| Done::Plan(platform::plan_waiting(pl, &*def, &p, &opts, localreg::Wait::CLUSTER_START, &mut |l| lock(&progress).push(l.to_string())).map(Box::new)));
    }

    // ------------------------------------------------------------------ wizard: review, progress, result

    fn review_event(&mut self, ev: &Event) {
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => {
                self.page = Page::Options;
                self.msg = None;
            }
            Event::Key(Key::Char('y')) | Event::Key(Key::Enter) => self.deploy(),
            Event::Key(Key::Char('g')) if self.wiz.as_ref().and_then(|w| w.plan.as_ref()).is_some_and(|p| p.needs_registry) => self.offer_registry(true),
            other => {
                Self::scroll_log(&mut self.log, other);
            }
        }
    }

    // ------------------------------------------------------------------ the local image registry (localreg.rs)

    /// The registry button of the deploy review: what will be created, in plain words, to approve.
    fn offer_registry(&mut self, resume: bool) {
        let Some(t) = self.wiz.as_ref().and_then(|w| w.chosen.as_ref()).map(Target::from_profile) else { return self.say("Choose a cluster first.", false) };
        self.show_registry_create(t, localreg::Opts::new(Default::default()).size_gi, resume, String::new());
    }

    fn show_registry_create(&mut self, t: Target, size_gi: u32, resume: bool, blocked: String) {
        if self.svc.is_none() {
            return self.say("Background jobs are not available in this window, so the registry cannot be created from here.", false);
        }
        let mut text = format!("{}\n\nVolume size: {size_gi} Gi (Ctrl+K changes it).\nIt runs as a background job: the log is in JOBS and the progress is beside the clock.", localreg::describe_plan(size_gi, &t));
        if resume {
            text.push_str("\nWhen the registry is ready the private store deploy continues by itself.");
        }
        self.show_confirm("Create the local image registry: review, then approve", &text, Act::CreateRegistry { target: t, size_gi, resume }, blocked, None, false);
    }

    /// The list's Registry row: look at the default cluster, then show what exists (Remove) or offer to create it.
    fn registry_row(&mut self) {
        self.say("Looking at the cluster…", true);
        self.spawn(|pl| {
            let profiles = platform::list_profiles(pl);
            let choice = devcluster::Choice::load(&pl.dir, &pl.user);
            let pick = profiles.iter().filter(|l| l.refusal.is_none()).find(|l| choice.as_ref().is_some_and(|c| c.kubeconfig == l.profile.kubeconfig)).or_else(|| profiles.iter().find(|l| l.refusal.is_none()));
            let Some(l) = pick else { return Done::RegState(Err("No cluster is reachable. Start the dev cluster (Quick install \u{2192} k3s) or choose one in Settings \u{2192} Dev cluster.".into())) };
            let t = Target::from_profile(&l.profile);
            let st = localreg::inspect(&pl.run, &iyakki_agent::ocipush::TcpHttp::default(), &t);
            let dep = localreg::check_dependency(&pl.run, &t);
            Done::RegState(Ok((t, st, dep)))
        });
    }

    fn start_registry_job(&mut self, target: Target, size_gi: u32, resume: bool) {
        let Some(svc) = self.svc.clone() else { return };
        let launch = match self.reg_launch.clone() {
            Some(l) => l,
            None => match quickinstall::Launch::this_program(svc.dir()) {
                Ok(l) => l,
                Err(e) => return self.say(e, false),
            },
        };
        let managed = svc.k3sdev.layout().kubeconfig().display().to_string() == target.kubeconfig;
        let args = quickinstall::RegistryArgs { target: if managed { None } else { Some(target.clone()) }, size_gi, node_port: 0 };
        match quickinstall::start_registry(&svc.jobs, &self.pl.user, &args, svc.k3sdev.installed().is_some(), &launch) {
            Ok(jobs) => {
                let Some(j) = jobs.last() else { return self.say("Nothing needed to start.", false) };
                (self.pl.audit)("platform.registry", &format!("create job {} size {size_gi}Gi", j.id));
                self.reg_run = Some(RegRun { id: j.id.clone(), target, resume });
                self.reg_retry = Some((self.reg_run.as_ref().map(|r| r.target.clone()).unwrap_or_default(), size_gi, resume));
                self.confirm = None;
                self.page = Page::Progress;
                self.log = ScrollLog::default();
                self.msg = None;
            }
            Err(e) => {
                self.confirm = None;
                self.page = if resume { Page::Review } else { Page::List };
                self.say(e, false);
            }
        }
    }

    /// Follow the registry job: show its log; when it ends continue the deploy (re-plan, then apply) or say why not.
    fn poll_registry(&mut self) {
        let (Some(r), Some(svc)) = (self.reg_run.as_ref(), self.svc.clone()) else { return };
        let user = self.pl.user.clone();
        let (id, resume) = (r.id.clone(), r.resume);
        let Ok(j) = svc.jobs.get(&user, &id) else {
            self.reg_run = None;
            return self.registry_failed("the registry job is gone".into(), String::new());
        };
        if !j.state.finished() {
            if self.page == Page::Progress {
                let tail = svc.jobs.tail(&user, &id, 14).unwrap_or_default();
                let dots = ".".repeat((self.tick / 6 % 4) as usize);
                let t = format!("Creating the local image registry (job {id}){dots}\nRunning for {}. Esc hides this (the job keeps going; the top bar shows it); JOBS has the full log.\n\n{tail}", iyakki_agent::jobs::fmt_duration(j.elapsed_secs(iyakki_agent::jobs::now_ms())));
                let keep = self.log.scroll;
                self.log = lines_of(&t);
                self.log.scroll = keep.min(1_000_000);
            }
            return;
        }
        self.reg_run = None;
        let tail = svc.jobs.tail(&user, &id, 15).unwrap_or_default();
        if j.state != iyakki_agent::jobs::State::Succeeded {
            let why = j.last_line.trim().trim_start_matches("error: ").to_string();
            return self.registry_failed(if why.is_empty() { format!("the registry job {}", j.state.label()) } else { why }, tail);
        }
        (self.pl.audit)("platform.registry", &format!("job {id} succeeded"));
        self.reg_retry = None;
        if resume && self.wiz.as_ref().is_some_and(|w| w.chosen.is_some()) {
            self.auto_deploy = true;
            self.page = Page::Review;
            self.log = lines_of("The registry is ready. Reading the cluster and building the plan again…");
            self.say("Registry ready. Continuing the deploy.", true);
            let Some(w) = self.wiz.as_mut() else { return };
            let (Some(p), Some(def)) = (w.chosen.clone(), platform::spec(&w.service)) else { return };
            let opts = Self::options_values(w);
            w.plan = None;
            self.spawn(move |pl| Done::Plan(platform::plan(pl, &*def, &p, &opts).map(Box::new)));
        } else {
            self.page = Page::List;
            self.say("The local image registry is ready.", true);
            self.reload();
        }
    }

    fn registry_failed(&mut self, reason: String, tail: String) {
        let mut t = format!("THE REGISTRY WAS NOT CREATED: {reason}\n");
        if !tail.trim().is_empty() {
            t.push_str(&format!("\nLAST OUTPUT\n{}\n", tail.trim_end()));
        }
        t.push_str("\nNothing else was changed. Fix the cause above and press Retry (a namespace or volume that was created is reused).\n");
        self.result_text = t.clone();
        self.log = lines_of(&t);
        self.fail = None;
        self.page = Page::Result;
    }

    fn deploy(&mut self) {
        let Some(plan) = self.wiz.as_ref().and_then(|w| w.plan.clone()) else { return self.say("The plan is not ready yet.", false) };
        if !plan.can_apply() {
            return self.say(format!("Cannot deploy yet: {}", plan.blockers.join("; ")), false);
        }
        self.page = Page::Progress;
        self.log = ScrollLog::default();
        lock(&self.progress).clear();
        lock(&self.live).clear();
        let (progress, live) = (self.progress.clone(), self.live.clone());
        self.msg = None;
        self.spawn(move |pl| {
            let pl = pl.with_live(live);
            Done::Applied(platform::apply(&pl, &plan, true, &mut |s| lock(&progress).push(s.to_string())))
        });
    }

    fn result_event(&mut self, ev: &Event) {
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Enter) => {
                self.fail = None;
                self.reg_retry = None;
                self.store_retry = None;
                self.show_docs = false;
                if std::mem::take(&mut self.store_after) {
                    if let Some(r) = self.store_rep.clone() {
                        return self.detect_store(r.cluster, r.namespace);
                    }
                }
                self.page = Page::List;
                self.wiz = None;
                self.reload();
            }
            Event::Key(Key::Char('f')) if self.store_retry.is_some() => {
                if let Some(r) = self.store_retry.take() {
                    self.store_rep = Some(*r);
                    self.store_confirm(pm::Action::Reset, true);
                }
            }
            Event::Key(Key::Char('g')) if self.reg_retry.is_some() => {
                if let Some((t, size, resume)) = self.reg_retry.clone() {
                    self.show_registry_create(t, size, resume, String::new());
                }
            }
            Event::Key(Key::Char('b')) if self.fail.is_some() => self.offer_build(),
            Event::Key(Key::Char('f')) if self.fail.is_some() => {
                self.choose.set("");
                self.choose_err.clear();
                self.page = Page::Choose;
                self.msg = None;
            }
            Event::Key(Key::Char('d')) if self.fail.is_some() => self.toggle_docs(),
            Event::Key(Key::Char('r')) if self.fail.is_some() => {
                self.show_docs = false;
                self.deploy();
            }
            Event::Key(Key::Char('v')) => {
                if self.reveal.is_empty() {
                    self.reveal_vault();
                }
            }
            Event::Key(Key::Char('o')) => {
                if let Some((_, u)) = self.result_urls.first().cloned() {
                    clipboard::copy(&u);
                    self.say(format!("Opening {u} (also copied)."), true);
                    self.wants.push(PWant::OpenUrl(u));
                }
            }
            other => {
                Self::scroll_log(&mut self.log, other);
            }
        }
    }

    // ------------------------------------------------------------------ a program the image is built from: find, build, choose

    /// What stopped the deploy, if it was a missing program a service builds its image from.
    fn fail_card(&self) -> Option<FailCard> {
        let plan = self.wiz.as_ref()?.plan.as_ref()?;
        let lb = platform::spec(&plan.service)?.local_binary()?;
        let want = Arch::parse(&plan.target_arch).unwrap_or_else(Arch::host);
        let places = Places::real(&self.pl.dir, &lb);
        let mut disc = localbin::discover(self.pl.run.as_ref(), &lb, &places, want);
        disc.found.is_none().then_some(())?;
        if let (Some(o), BuildState::Ready(b)) = (&self.build_override, &mut disc.build) {
            b.argv = o.clone();
        }
        Some(FailCard { lb, disc, places })
    }

    fn can_build(&self) -> bool {
        self.svc.is_some() && self.fail.as_ref().is_some_and(|f| matches!(f.disc.build, BuildState::Ready(_)))
    }

    /// "Build now": the exact command, the folder and what it writes, as a review to approve.
    fn offer_build(&mut self) {
        let Some(FailCard { disc, lb, .. }) = self.fail.as_ref() else { return };
        let BuildState::Ready(b) = &disc.build else { return self.say("Building here is not possible yet; see what is missing above.", false) };
        if self.svc.is_none() {
            return self.say("Background jobs are not available in this window, so the build cannot be started from here. Run the command yourself.", false);
        }
        let text = format!(
            "BUILD {}\nIt compiles the small static server for {} as a background job (JOBS shows it; Stop ends it). Nothing is deployed until it has succeeded; then the deploy continues by itself.\n\nCOMMAND\n{}\n\nFOLDER\n{}\n\nWRITES\n{} (inside the source folder's target/ only)\n\nIt needs no network if the crates were fetched before; the first build takes a few minutes.",
            lb.file,
            disc.want.uname(),
            b.argv.join(" "),
            b.cwd.display(),
            b.output.display()
        );
        let plan = b.clone();
        self.show_confirm("Build the store server now: review, then approve", &text, Act::Build(Box::new(plan)), String::new(), None, false);
    }

    fn start_build(&mut self, plan: BuildPlan) {
        let (Some(svc), Some(lb)) = (self.svc.clone(), self.fail.as_ref().map(|f| f.lb)) else { return };
        let spec = iyakki_agent::jobs::Spec { label: format!("Build {}", lb.file), cmd: iyakki_agent::jobs::Cmd::Argv(plan.argv.clone()), cwd: plan.cwd.clone(), workspace: String::new() };
        self.confirm = None;
        match svc.jobs.start(&self.pl.user, &spec, &|_: &std::path::Path| Ok(())) {
            Ok(j) => {
                (self.pl.audit)("platform.build", &format!("{} job {}", lb.file, j.id));
                self.build = Some(BuildRun { id: j.id, plan, lb, progress: None });
                self.page = Page::Progress;
                self.log = ScrollLog::default();
                self.msg = None;
            }
            Err(e) => {
                self.page = Page::Result;
                self.say(format!("Could not start the build: {e}"), false);
            }
        }
    }

    fn stop_build(&mut self) {
        if let (Some(b), Some(svc)) = (self.build.as_ref(), self.svc.clone()) {
            let _ = svc.jobs.stop(&self.pl.user, &b.id, iyakki_agent::jobs::STOP_GRACE);
        }
    }

    /// Follow the build job: show it, and when it ends either continue the deploy or say why not.
    fn poll_build(&mut self) {
        let (Some(b), Some(svc)) = (self.build.as_ref(), self.svc.clone()) else { return };
        let user = self.pl.user.clone();
        let Ok(j) = svc.jobs.get(&user, &b.id) else {
            self.build = None;
            return self.build_failed("the build job is gone".into(), String::new());
        };
        if !j.state.finished() {
            let secs = j.elapsed_secs(iyakki_agent::jobs::now_ms());
            let dots = ".".repeat((self.tick / 6 % 4) as usize);
            let tail = svc.jobs.tail(&user, &b.id, 12).unwrap_or_default();
            let mut t = format!("Building {} for {} (job {}){dots}\nRunning for {}. Esc stops it; JOBS shows the full log.\n\n", b.lb.file, b.plan.arch.uname(), b.id, iyakki_agent::jobs::fmt_duration(secs));
            t.push_str(&tail);
            let keep = self.log.scroll;
            self.log = lines_of(&t);
            self.log.scroll = keep.min(1_000_000);
            if let Some(r) = self.build.as_mut() {
                r.progress = j.progress;
            }
            return;
        }
        let Some(b) = self.build.take() else { return };
        let tail = svc.jobs.tail(&user, &b.id, 15).unwrap_or_default();
        if j.state == iyakki_agent::jobs::State::Succeeded {
            match localbin::judge(&b.plan.output, b.plan.arch) {
                Ok(_) => {
                    (self.pl.audit)("platform.build", &format!("{} job {} succeeded", b.lb.file, b.id));
                    self.fail = None;
                    self.say(format!("Built {}. Continuing the deploy.", b.lb.file), true);
                    return self.deploy();
                }
                Err(e) => return self.build_failed(format!("the build finished but its result cannot be used: {e}"), tail),
            }
        }
        self.build_failed(format!("building {} {}", b.lb.file, j.state.label()), tail);
    }

    fn build_failed(&mut self, reason: String, tail: String) {
        self.fail = self.fail_card();
        let mut t = format!("CANNOT DEPLOY: {reason}\n");
        if !tail.trim().is_empty() {
            t.push_str(&format!("\nLAST OUTPUT OF THE BUILD\n{}\n", tail.trim_end()));
        }
        t.push_str("\nNothing further was done; run the wizard again after fixing it (a namespace or Secret that was created stays and is reused).\n");
        self.result_text = t.clone();
        self.log = lines_of(&t);
        self.show_docs = false;
        self.page = Page::Result;
    }

    fn toggle_docs(&mut self) {
        self.show_docs = !self.show_docs;
        let t = if self.show_docs { server_program_docs() } else { self.result_text.clone() };
        self.log = lines_of(&t);
    }

    fn choose_event(&mut self, ev: &Event) {
        match ev {
            Event::Key(Key::Esc) => {
                self.page = Page::Result;
                self.choose_err.clear();
            }
            Event::Key(Key::Enter) | Event::Key(Key::Ctrl('s')) => self.use_chosen_file(),
            Event::Paste(t) => {
                self.choose.paste(t);
            }
            Event::FocusAt(_, x) => self.choose.click(*x),
            Event::Key(k) => {
                self.choose.handle(k);
            }
            _ => {}
        }
    }

    /// Check the typed file by its ELF headers (it is never run), keep a copy in the data folder and continue the deploy.
    fn use_chosen_file(&mut self) {
        let Some(f) = self.fail.as_ref() else { return };
        if self.choose.value.trim().is_empty() {
            self.choose_err = "Type the path of the file first.".into();
            return;
        }
        let path = localbin::expand_typed(&self.choose.value);
        match localbin::adopt(&path, &f.lb, &f.places, f.disc.want) {
            Ok(found) => {
                (self.pl.audit)("platform.choose-binary", &format!("{} {}", f.lb.file, found.path.display()));
                self.fail = None;
                self.choose_err.clear();
                self.say(format!("Using {} ({}). Continuing the deploy.", path.display(), found.arch.uname()), true);
                self.deploy();
            }
            Err(e) => self.choose_err = e,
        }
    }

    fn reveal_event(&mut self, ev: &Event) {
        let Some(r) = self.reveal.first_mut() else { return };
        match ev {
            Event::Key(Key::Char('s')) | Event::Key(Key::Char('h')) => r.shown = !r.shown,
            Event::Key(Key::Char('c')) => {
                clipboard::copy(&r.value);
                r.copied = true;
            }
            Event::Key(Key::Esc) | Event::Key(Key::Enter) | Event::Key(Key::Char('d')) => {
                if r.copied {
                    clipboard::forget(&r.value);
                }
                self.reveal.remove(0); // dropped: the memory is wiped
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ confirm screens

    fn confirm_event(&mut self, ev: &Event) {
        let Some(c) = self.confirm.as_mut() else { return self.page = Page::List };
        if c.busy {
            Self::scroll_log(&mut c.log, ev);
            return;
        }
        if matches!(c.act, Act::Build(_)) && matches!(ev, Event::Key(Key::Esc) | Event::Key(Key::Char('n'))) {
            self.confirm = None;
            self.page = Page::Result;
            return;
        }
        match ev {
            Event::Key(Key::Esc) => {
                let done = c.done;
                self.leave_confirm(done);
            }
            Event::Key(Key::Char('v')) if matches!(c.act, Act::Close) => self.reveal_vault_from_details(),
            Event::Key(Key::Char('n')) if c.typed.is_none() && !matches!(c.act, Act::Close) => {
                self.leave_confirm(false);
            }
            Event::Key(Key::Char('y')) if c.typed.is_none() && !matches!(c.act, Act::Close) && !c.done => self.approve(),
            Event::Key(Key::Enter) => {
                if matches!(c.act, Act::Close) || c.done {
                    let done = c.done;
                    self.leave_confirm(done);
                } else {
                    self.approve();
                }
            }
            Event::Key(Key::Ctrl(k @ ('k' | 'r' | 'f'))) if matches!(c.act, Act::StoreDo { .. }) => {
                if let Act::StoreDo { rep, action, force } = &mut c.act {
                    match (*k, &mut *action) {
                        ('k', pm::Action::Remove(ch)) => ch.keep_data = !ch.keep_data,
                        ('r', pm::Action::Remove(ch)) => ch.remove_registry = !ch.remove_registry,
                        ('f', pm::Action::Reset) => *force = !*force,
                        _ => {}
                    }
                    c.log = lines_of(&store_confirm_text(rep, action, *force));
                }
            }
            Event::Key(Key::Ctrl('k')) => {
                if let Act::CreateRegistry { target, size_gi, resume } = &c.act {
                    let sizes = localreg::SIZES_GI;
                    let next = sizes[sizes.iter().position(|x| x == size_gi).map_or(0, |i| (i + 1) % sizes.len())];
                    let (t, r, b) = (target.clone(), *resume, c.blocked.clone());
                    return self.show_registry_create(t, next, r, b);
                }
                if let Act::Remove(r) = &mut c.act {
                    r.delete_data = !r.delete_data;
                    let text = format!("{}\n\nTo toggle keeping the data, press Ctrl+K.\nTo approve, type the service id ({}) below and press Enter.", platform::preview_remove(r), r.inst.id);
                    c.log = lines_of(&text);
                }
            }
            Event::Key(k @ (Key::Up | Key::Down | Key::PageUp | Key::PageDown)) if c.typed.is_none() || matches!(k, Key::PageUp | Key::PageDown) => {
                Self::scroll_log(&mut c.log, ev);
            }
            Event::Paste(t) => {
                if let Some(tf) = c.typed.as_mut() {
                    tf.paste(t);
                }
            }
            Event::FocusAt(_, x) => {
                if let Some(tf) = c.typed.as_mut() {
                    tf.click(*x);
                }
            }
            Event::Key(k) if c.typed.is_some() => {
                if let Some(tf) = c.typed.as_mut() {
                    tf.handle(k);
                }
            }
            other => {
                Self::scroll_log(&mut c.log, other);
            }
        }
    }

    fn reveal_vault_from_details(&mut self) {
        let id = self.confirm.as_ref().map(|c| c.title.trim_start_matches("Details: ").to_string()).unwrap_or_default();
        if let Some(i) = installed::find(&self.pl.dir, &self.pl.user, &id) {
            self.connect_to = Some(i.id);
            self.reveal_vault();
        }
    }

    fn approve(&mut self) {
        let Some(c) = self.confirm.as_mut() else { return };
        if !c.blocked.is_empty() {
            let b = c.blocked.clone();
            return self.say(b, false);
        }
        let typed = c.typed.as_ref().map(|t| t.value.trim().to_string());
        let act = std::mem::replace(&mut c.act, Act::Close);
        self.after_connect = matches!(act, Act::Connect(_));
        let Some(c) = self.confirm.as_mut() else { return };
        c.busy = true;
        c.log = lines_of("Working…");
        match act {
            Act::Power(i, start) => self.spawn(move |pl| Done::Action(platform::power(pl, &i, start, true))),
            Act::Backup(i) => self.spawn(move |pl| Done::Action(platform::backup(pl, &i, true, true).map(|b| vec![format!("backup written inside the pod: {}", b.remote), b.local.map(|l| format!("copied to {}", l.display())).unwrap_or_else(|| "not copied to this machine".into())]))),
            Act::Upgrade(u) => {
                let progress = self.progress.clone();
                self.spawn(move |pl| Done::Action(platform::upgrade(pl, &u, true, &mut |s| lock(&progress).push(s.to_string()))))
            }
            Act::Remove(r) => {
                let confirm = typed.unwrap_or_default();
                self.spawn(move |pl| Done::Action(platform::remove(pl, &r, &confirm, true)))
            }
            Act::Connect(p) => self.spawn(move |pl| {
                Done::Action(connect::connect(pl, &p, true).map(|c| vec![format!("repository {} ({})", c.repo_url, if c.created { "created" } else { "already existed" }), format!("origin is {}", c.remote_url), if c.origin_set { "origin was set in the workspace".to_string() } else { "origin was already right".to_string() }]))
            }),
            Act::Push(p) => self.spawn(move |pl| Done::Action(connect::push(pl, &p, true).map(|t| vec![t]))),
            Act::Build(p) => self.start_build(*p),
            Act::CreateRegistry { target, size_gi, resume } => self.start_registry_job(target, size_gi, resume),
            Act::RemoveRegistry(t) => {
                let confirm = typed.unwrap_or_default();
                let progress = self.progress.clone();
                let data = self.pl.dir.clone();
                lock(&progress).clear();
                self.spawn(move |pl| {
                    let r = localreg::remove(&pl.run, &iyakki_agent::ocipush::TcpHttp::default(), &t, &confirm, &mut |l| lock(&progress).push(l.to_string()));
                    if r.is_ok() {
                        quickinstall::unmark(&data, quickinstall::Item::Registry);
                        (pl.audit)("platform.registry", "removed");
                    }
                    Done::Action(r.map(|_| vec!["the local image registry and its images were removed".to_string()]))
                })
            }
            Act::StoreRepair(rep) => {
                let (progress, live) = (self.progress.clone(), self.live.clone());
                lock(&progress).clear();
                lock(&live).clear();
                self.spawn(move |pl| {
                    let pl = pl.with_live(live);
                    let r = platform::guard_write(&pl, &rep.cluster).and_then(|profile| {
                        let opts = rep.record.as_ref().map(|r| r.options.clone()).unwrap_or_default();
                        pm::repair(&pl, &rep, &profile, &opts, &iyakki_agent::ocipush::TcpHttp::default(), &mut |l| lock(&progress).push(l.to_string()))
                    });
                    Done::Action(r)
                })
            }
            Act::StoreDo { rep, action, force } => {
                let typed = typed.unwrap_or_default();
                self.spawn(move |pl| {
                    Done::StoreReady(pm::prepare(pl, &rep, &action, &typed, force).and_then(|a| a.ok_or_else(|| "nothing to run".to_string())).map(|a| (rep, action, a, force)))
                })
            }
            Act::Close => {}
        }
    }

    // ------------------------------------------------------------------ the private store: state, repair, reset, remove

    /// Read what state the private store is in (in the background) and show it on the Store page.
    fn detect_store(&mut self, cluster: platform::ClusterRef, namespace: String) {
        self.store_rep = None;
        self.page = Page::Store;
        self.log = lines_of("Reading the cluster to see what is there. Nothing is changed.");
        self.msg = None;
        let svc = self.svc.clone();
        self.spawn(move |pl| {
            let store = svc.as_ref().map(|s| Store::for_services(s));
            Done::StoreState(Box::new(pm::private_store_state(pl, store.as_ref(), &cluster, &namespace)))
        });
    }

    fn show_store(&mut self, rep: pm::Report) {
        self.log = lines_of(&rep.text());
        self.store_rep = Some(rep);
        self.page = Page::Store;
        self.msg = None;
    }

    fn can_start_cluster(&self) -> bool {
        match (&self.svc, &self.store_rep) {
            (Some(s), Some(r)) => s.k3sdev.installed().is_some() && s.k3sdev.layout().kubeconfig().display().to_string() == r.cluster.kubeconfig,
            _ => false,
        }
    }

    fn store_event(&mut self, ev: &Event) {
        let offers = self.store_rep.as_ref().map(|r| r.offers()).unwrap_or_default();
        match ev {
            Event::Key(Key::Esc) => {
                self.msg = None;
                if self.wiz.as_ref().map_or(true, |w| w.manage) {
                    self.wiz = None;
                    self.store_rep = None;
                    self.page = Page::List;
                    self.reload();
                } else {
                    self.page = Page::Cluster;
                }
            }
            Event::Key(Key::Char('l')) => match self.store_rep.clone() {
                Some(r) => self.detect_store(r.cluster, r.namespace),
                None => {}
            },
            Event::Key(Key::Char('r')) if offers.repair => self.store_confirm(pm::Action::Repair, false),
            Event::Key(Key::Char('x')) if offers.reset => self.store_confirm(pm::Action::Reset, false),
            Event::Key(Key::Char('m')) if offers.remove => self.store_confirm(pm::Action::Remove(pm::RemoveChoice::default()), false),
            Event::Key(Key::Char('d')) if offers.deploy => {
                if self.wiz.as_ref().is_some_and(|w| !w.manage && w.chosen.is_some()) {
                    self.build_options();
                } else {
                    let prefer = self.store_rep.as_ref().map(|r| (r.cluster.kubeconfig.clone(), r.cluster.context.clone()));
                    self.open_wizard(privatestore::ID);
                    if let Some(w) = self.wiz.as_mut() {
                        w.prefer = prefer;
                    }
                }
            }
            Event::Key(Key::Char('s')) if offers.start_cluster => match self.svc.clone() {
                Some(svc) if self.can_start_cluster() => match svc.k3sdev.start(&self.pl.user) {
                    Ok(()) => self.say("Starting the cluster. When it is Ready press Look again.", true),
                    Err(e) => self.say(e, false),
                },
                _ => self.say("Start the cluster in Settings, Dev cluster (or start your own cluster), then press Look again.", false),
            },
            other => {
                Self::scroll_log(&mut self.log, other);
            }
        }
    }

    /// The review-and-approve screen of a Repair, Reset or Remove.
    fn store_confirm(&mut self, action: pm::Action, force: bool) {
        let Some(rep) = self.store_rep.clone() else { return };
        let text = store_confirm_text(&rep, &action, force);
        let (title, typed) = match action {
            pm::Action::Repair => ("Repair the private store: review, then approve", None),
            pm::Action::Reset => ("Reset and reinstall the private store: review, then approve", Some(TextField::new(&rep.namespace, false))),
            pm::Action::Remove(_) => ("Remove the private store: review, then approve", Some(TextField::new(&rep.namespace, false))),
        };
        let danger = action.destructive();
        let act = match action {
            pm::Action::Repair => Act::StoreRepair(Box::new(rep)),
            a => Act::StoreDo { rep: Box::new(rep), action: a, force },
        };
        self.show_confirm(title, &text, act, String::new(), typed, danger);
        self.store_after = true;
    }

    fn confirm_failed(&mut self, why: String) {
        if let Some(c) = self.confirm.as_mut() {
            c.busy = false;
            c.done = true;
            c.title = "Not done".into();
            c.danger = true;
            c.typed = None;
            c.log = lines_of(&format!("NOT DONE: {why}"));
        } else {
            self.say(why, false);
        }
    }

    /// Leave a review screen. After a store action the store is looked at again.
    fn leave_confirm(&mut self, reload: bool) {
        self.confirm = None;
        if std::mem::take(&mut self.store_after) {
            if let Some(r) = self.store_rep.clone() {
                return self.detect_store(r.cluster, r.namespace);
            }
        }
        self.page = Page::List;
        if reload {
            self.reload();
        }
    }

    fn start_store_job(&mut self, rep: Box<pm::Report>, action: pm::Action, args: pm::JobArgs) {
        let Some(svc) = self.svc.clone() else { return self.confirm_failed("Background jobs are not available in this window, so this cannot run from here.".into()) };
        let launch = match self.reg_launch.clone() {
            Some(l) => l,
            None => match quickinstall::Launch::this_program(svc.dir()) {
                Ok(l) => l,
                Err(e) => return self.confirm_failed(e),
            },
        };
        match pm::start_job(&svc.jobs, &self.pl.user, &launch, &args) {
            Ok(j) => {
                (self.pl.audit)("platform.store", &format!("{:?} job {} namespace {}", action, j.id, args.namespace));
                self.store_run = Some(StoreRun { id: j.id, action, rep });
                self.confirm = None;
                self.page = Page::Progress;
                self.log = lines_of("Starting…");
                self.msg = None;
            }
            Err(e) => self.confirm_failed(e),
        }
    }

    /// Follow the reset or removal job: show its log; when it ends clean this device's side, or say why it stopped.
    fn poll_store(&mut self) {
        let (Some(r), Some(svc)) = (self.store_run.as_ref(), self.svc.clone()) else { return };
        let user = self.pl.user.clone();
        let id = r.id.clone();
        let reset = matches!(r.action, pm::Action::Reset);
        let Ok(j) = svc.jobs.get(&user, &id) else {
            self.store_run = None;
            return self.store_failed(reset, None, "the job is gone".into(), String::new());
        };
        if !j.state.finished() {
            if self.page == Page::Progress {
                let tail = svc.jobs.tail(&user, &id, 14).unwrap_or_default();
                let dots = ".".repeat((self.tick / 6 % 4) as usize);
                let t = format!("{} the private store (job {id}){dots}\nRunning for {}. Esc hides this (the job keeps going; the top bar shows it); JOBS has the full log.\n\n{tail}", if reset { "Resetting" } else { "Removing" }, iyakki_agent::jobs::fmt_duration(j.elapsed_secs(iyakki_agent::jobs::now_ms())));
                let keep = self.log.scroll;
                self.log = lines_of(&t);
                self.log.scroll = keep.min(1_000_000);
            }
            return;
        }
        let Some(run) = self.store_run.take() else { return };
        let tail = svc.jobs.tail(&user, &id, 15).unwrap_or_default();
        if j.state != iyakki_agent::jobs::State::Succeeded {
            let why = j.last_line.trim().trim_start_matches("error: ").to_string();
            let why = if why.is_empty() { format!("the job {}", j.state.label()) } else { why };
            return self.store_failed(reset, Some(run), why, tail);
        }
        self.msg = None;
        self.log = lines_of("The cluster side is done. Cleaning this device's entries…");
        let reinstall = matches!(run.action, pm::Action::Reset);
        self.spawn(move |pl| {
            let store = Store::for_services(&svc);
            let http = iyakki_agent::ocipush::TcpHttp::default();
            let want_registry = matches!(run.action, pm::Action::Remove(c) if c.remove_registry);
            let target = if want_registry { platform::guard_write(pl, &run.rep.cluster).ok().map(|p| Target::from_profile(&p)) } else { None };
            let reg = target.as_ref().map(|t| (t, &http as &dyn iyakki_agent::ocipush::RegistryHttp));
            Done::StoreLocal(pm::finish_local(pl, &store, &iyakki_agent::store::install::Actor::system(), &run.rep, &run.action, reg, &mut |_| {}), reinstall)
        });
    }

    fn store_failed(&mut self, reset: bool, run: Option<StoreRun>, why: String, tail: String) {
        let mut t = format!("THE {} STOPPED: {why}\n", if reset { "RESET" } else { "REMOVAL" });
        if !tail.trim().is_empty() {
            t.push_str(&format!("\nLAST OUTPUT\n{}\n", tail.trim_end()));
        }
        t.push_str("\nNothing outside the store's namespace was touched, and this device's entries (vault, STORE, installed services) were not changed yet. Every step can be run again safely: Close, then choose the action again.\n");
        self.store_retry = None;
        if reset && why.contains("still Terminating") {
            if let Some(r) = run {
                t.push_str("\nThe namespace is stuck. Press f (or the button) to reset again and ALSO clear the finalizers of what is left in it; that is what Kubernetes is waiting for. Only objects in this one namespace are touched.\n");
                self.store_retry = Some(r.rep);
            }
        }
        self.result_text = t.clone();
        self.log = lines_of(&t);
        self.fail = None;
        self.reg_retry = None;
        self.page = Page::Result;
    }

    fn store_done(&mut self, r: Result<Vec<String>, String>, reinstall: bool) {
        match r {
            Ok(lines) => {
                let mut t = format!("DONE\n- {}\n", if lines.is_empty() { "nothing more to clean on this device".to_string() } else { lines.join("\n- ") });
                if reinstall {
                    t.push_str("\nThe old store is gone. The deploy wizard continues: choose the cluster, review the plan, approve.\n");
                    let prefer = self.store_rep.as_ref().map(|r| (r.cluster.kubeconfig.clone(), r.cluster.context.clone()));
                    self.store_rep = None;
                    self.store_after = false;
                    self.open_wizard(privatestore::ID);
                    if let Some(w) = self.wiz.as_mut() {
                        w.prefer = prefer;
                    }
                    self.say("Reset done. Choose the cluster to deploy the private store again.", true);
                    self.result_text = t;
                    return;
                }
                self.store_rep = None;
                self.store_after = false;
                self.result_text = t.clone();
                self.log = lines_of(&t);
                self.page = Page::Result;
            }
            Err(e) => {
                let t = format!("THE CLUSTER SIDE WORKED BUT THIS DEVICE'S ENTRIES COULD NOT BE CLEANED: {e}\nLook at the private store again; the remaining entries can be removed from there.\n");
                self.result_text = t.clone();
                self.log = lines_of(&t);
                self.page = Page::Result;
            }
        }
    }

    // ------------------------------------------------------------------ connect existing, connect workspace

    fn existing_form(&mut self) {
        self.url.set("");
        self.token.set("");
        self.existing_focus = 0;
        self.page = Page::Existing;
        self.msg = None;
    }

    fn existing_event(&mut self, ev: &Event) {
        let tf = if self.existing_focus == 0 { &mut self.url } else { &mut self.token };
        match ev {
            Event::Key(Key::Esc) => {
                self.page = Page::List;
                self.token.set("");
            }
            Event::Key(Key::Tab | Key::Down | Key::Up | Key::BackTab) => self.existing_focus = 1 - self.existing_focus,
            Event::Key(Key::Enter) if self.existing_focus == 0 => self.existing_focus = 1,
            Event::Key(Key::Enter) | Event::Key(Key::Ctrl('s')) => {
                let (url, token) = (self.url.value.clone(), self.token.value.clone());
                if let Err(e) = platform::gitea::normalize_url(&url) {
                    return self.say(e, false);
                }
                self.say("Connecting…", true);
                self.spawn(move |pl| Done::Existing(platform::gitea::connect_existing(pl, &url, &token, None, "", true)));
            }
            Event::Paste(t) => {
                tf.paste(t);
            }
            Event::FocusAt(i, x) => {
                self.existing_focus = (*i).min(1);
                if self.existing_focus == 0 { self.url.click(*x) } else { self.token.click(*x) }
            }
            Event::Key(k) => {
                tf.handle(k);
            }
            _ => {}
        }
    }

    fn pick_workspace(&mut self) {
        if let Some(w) = self.fixed_ws.clone() {
            self.wss = vec![(w.name.clone(), w.root)];
        } else {
            let reg = WsRegistry::load(&self.pl.dir, &self.pl.user);
            self.wss = reg.ordered().into_iter().map(|w| (w.name.clone(), w.root_path())).collect();
        }
        self.wsel = 0;
        if self.wss.is_empty() {
            return self.say("You have no workspace yet (the AI IDE makes them).", false);
        }
        if self.wss.len() == 1 {
            return self.start_connect();
        }
        self.page = Page::Workspaces;
    }

    fn workspaces_event(&mut self, ev: &Event) {
        let len = self.wss.len();
        match ev {
            Event::Row(i) if *i < len => {
                if self.wsel == *i {
                    return self.start_connect();
                }
                self.wsel = *i;
            }
            Event::Key(Key::Up) => Self::move_sel(&mut self.wsel, -1, len),
            Event::Key(Key::Down) => Self::move_sel(&mut self.wsel, 1, len),
            Event::Key(Key::Enter) => self.start_connect(),
            Event::Key(Key::Esc) => self.page = Page::List,
            _ => {}
        }
    }

    fn start_connect(&mut self) {
        let Some(inst) = self.connect_to.as_ref().and_then(|id| installed::find(&self.pl.dir, &self.pl.user, id)) else { return self.say("Select an installed git service first.", false) };
        let ws = match self.fixed_ws.clone() {
            Some(w) => w,
            None => match self.wss.get(self.wsel) {
                Some((n, r)) => WsInfo { name: n.clone(), root: r.clone() },
                None => return self.say("Choose a workspace.", false),
            },
        };
        self.say("Reading the workspace's git state…", true);
        self.spawn(move |pl| Done::ConnectPlan(connect::plan_connect(pl, &inst, &ws, &ConnectOpts::default()).map(Box::new)));
    }

    // ------------------------------------------------------------------ background results

    fn poll(&mut self) {
        let done: Vec<Done> = std::mem::take(&mut *lock(&self.inbox));
        for d in done {
            match d {
                Done::Profiles(p) => {
                    if let Some(w) = self.wiz.as_mut() {
                        w.profiles = p;
                        w.loading = false;
                        let preferred = w.prefer.as_ref().and_then(|(k, c)| w.profiles.iter().position(|l| l.refusal.is_none() && l.profile.kubeconfig == *k && l.profile.context == *c));
                        w.psel = preferred.or_else(|| w.profiles.iter().position(|l| l.refusal.is_none())).unwrap_or(0);
                    }
                }
                Done::Plan(r) => {
                    if self.page != Page::Review {
                        continue;
                    }
                    match r {
                        Ok(p) => {
                            self.log = lines_of(&platform::preview(&p));
                            let go = std::mem::take(&mut self.auto_deploy) && p.can_apply();
                            if let Some(w) = self.wiz.as_mut() {
                                w.plan = Some(p);
                            }
                            if go {
                                self.deploy();
                            }
                        }
                        Err(e) => {
                            self.auto_deploy = false;
                            self.log = lines_of(&format!("CANNOT PLAN: {e}"))
                        }
                    }
                }
                Done::Applied(r) => {
                    let mut t = String::new();
                    self.fail = None;
                    self.reg_retry = None;
                    self.show_docs = false;
                    match r {
                        Ok(mut a) => {
                            t.push_str(if a.ready { "DEPLOYED: the service is ready\n" } else { "APPLIED: but it is not ready yet\n" });
                            for l in &a.done {
                                t.push_str(&format!("- {l}\n"));
                            }
                            t.push_str("\nADDRESSES\n");
                            self.result_urls = a.urls.iter().filter(|u| u.kind == "web").map(|u| (u.label.clone(), u.url.clone())).collect();
                            for u in &a.urls {
                                t.push_str(&format!("{}: {}\n", u.label, u.url));
                            }
                            t.push_str(&format!("\nHOW TO USE\n{}\n", a.how_to_use));
                            for r in a.reveal.drain(..) {
                                t.push_str(&format!("\nCREDENTIALS: {} is stored in your vault as uvault:{} (and the cluster's Secret). It is shown once, masked, in the next dialog.\n", r.label, r.vault));
                                self.reveal.push(RevealView { label: r.label.clone(), vault: r.vault.clone(), value: r.value.clone(), shown: false, copied: false });
                            }
                        }
                        Err(e) => {
                            // a missing locally built program is a card with the fixes at hand, not a command to go and type
                            self.fail = self.fail_card();
                            t.push_str(&format!("CANNOT DEPLOY: {e}\nNothing further was done; run the wizard again after fixing it (a namespace or Secret that was created stays and is reused).\n"));
                            if self.fail.is_some() {
                                t.push_str("\nChoose one of the buttons below; the deploy then continues by itself (or press Try again).\n");
                            }
                        }
                    }
                    self.result_text = t.clone();
                    self.log = lines_of(&t);
                    self.page = Page::Result;
                }
                Done::Status(m, reg) => {
                    self.statuses = m;
                    self.registry_up = reg;
                    if self.page == Page::List {
                        self.reload_rows_only();
                    }
                }
                Done::UpgradePlan(r) => match r {
                    Ok(u) => {
                        let blocked = u.blockers.join("; ");
                        let text = platform::preview_upgrade(&u);
                        self.msg = None;
                        self.show_confirm(&format!("Upgrade {} to {}: review, then approve", u.inst.id, u.to), &text, Act::Upgrade(u), blocked, None, false);
                    }
                    Err(e) => self.say(e, false),
                },
                Done::ConnectPlan(r) => match r {
                    Ok(p) => {
                        let blocked = p.blockers.join("; ");
                        let text = connect::preview_connect(&p);
                        self.show_confirm(&format!("Connect {} to {}: review, then approve", p.ws.name, p.inst.id), &text, Act::Connect(p), blocked, None, false);
                    }
                    Err(e) => self.say(e, false),
                },
                Done::PushPlan(r) => match r {
                    Ok(p) => {
                        let blocked = p.blockers.join("; ");
                        let text = connect::preview_push(&p);
                        self.show_confirm(&format!("Push {}: review, then approve", p.branch), &text, Act::Push(p), blocked, None, false);
                    }
                    Err(e) => self.say(e, false),
                },
                Done::Action(r) => {
                    let (title_done, text, ok) = match &r {
                        Ok(lines) => ("Done".to_string(), format!("DONE\n- {}", lines.join("\n- ")), true),
                        Err(e) => ("Not done".to_string(), format!("NOT DONE: {e}"), false),
                    };
                    self.last_result = r.clone().unwrap_or_default();
                    if let Some(c) = self.confirm.as_mut() {
                        c.busy = false;
                        c.done = true;
                        c.title = title_done;
                        c.danger = !ok;
                        c.typed = None;
                        c.log = lines_of(&text);
                    }
                    // after a connect that worked, offer the push as its own approval
                    if ok && std::mem::take(&mut self.after_connect) {
                        if let (Some(id), Some(ws)) = (self.connect_to.clone(), self.pending_ws()) {
                            if let Some(inst) = installed::find(&self.pl.dir, &self.pl.user, &id) {
                                self.spawn(move |pl| Done::PushPlan(connect::plan_push(pl, &inst, &ws).map(Box::new)));
                            }
                        }
                    }
                    self.after_connect = false;
                }
                Done::RegState(r) => match r {
                    Err(e) => self.say(e, false),
                    Ok((t, st, dep)) => {
                        self.msg = None;
                        let line = localreg::status_line(&st);
                        match &st {
                            localreg::State::Absent => {
                                let blocked = match dep {
                                    Dependency::Missing(m) => m,
                                    Dependency::Met { .. } => String::new(),
                                };
                                self.show_registry_create(t, localreg::Opts::new(Default::default()).size_gi, false, blocked);
                            }
                            localreg::State::Unknown(e) => self.say(e.clone(), false),
                            _ => {
                                let text = format!("{line}\n\nPushing: {}\nPods pull: {}\nVolume: {}\n\n{}", st.info().map(|i| i.push.clone()).unwrap_or_default(), st.info().map(|i| i.pull.clone()).unwrap_or_default(), st.info().map(|i| i.storage.clone()).unwrap_or_default(), localreg::describe_remove(&st));
                                let tf = TextField::new("", false);
                                self.show_confirm(&format!("Local image registry: {}", line.trim_start_matches("registry ")), &text, Act::RemoveRegistry(t), String::new(), Some(tf), true);
                            }
                        }
                    }
                },
                Done::StoreState(rep) => {
                    let continue_deploy = self.wiz.as_ref().is_some_and(|w| !w.manage && w.chosen.is_some()) && (matches!(rep.state, pm::StoreState::NotInstalled) || matches!(rep.state, pm::StoreState::Broken { why: pm::Why::ClusterDown | pm::Why::NoKubectl, .. }));
                    if continue_deploy {
                        // nothing there to look after: the deploy goes on (a cluster still starting is waited for when the plan is made)
                        self.build_options();
                    } else {
                        self.show_store(*rep);
                    }
                }
                Done::StoreReady(r) => match r {
                    Ok((rep, action, args, _)) => self.start_store_job(rep, action, args),
                    Err(e) => self.confirm_failed(e),
                },
                Done::StoreLocal(r, reinstall) => self.store_done(r, reinstall),
                Done::Existing(r) => match r {
                    Ok(i) => {
                        self.say(format!("Connected {} ({}).", i.id, i.note), true);
                        self.token.set("");
                        self.url.set("");
                        self.page = Page::List;
                        self.reload();
                    }
                    Err(e) => self.say(e, false),
                },
            }
        }
        self.poll_build();
        self.poll_registry();
        self.poll_store();
        // the wait for a cluster that is still starting
        if self.page == Page::Review && self.wiz.as_ref().is_some_and(|w| w.plan.is_none()) {
            let p = lock(&self.progress).clone();
            if !p.is_empty() && self.busy.load(Ordering::SeqCst) > 0 {
                self.log = lines_of(&format!("Reading the cluster and building the plan…\n\n{}", p.join("\n")));
            }
        }
        // live output of a running step
        if self.page == Page::Progress && self.build.is_none() && self.reg_run.is_none() && self.store_run.is_none() {
            let mut lines: Vec<String> = lock(&self.progress).clone();
            let live = lock(&self.live).clone();
            lines.extend(live.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().map(|l| format!("  {l}")));
            let keep = self.log.scroll;
            self.log.clear();
            for l in lines {
                self.log.push(l, [0, 0, 0, 0]);
            }
            self.log.scroll = keep;
        }
        if let Some(c) = self.confirm.as_mut() {
            if c.busy {
                let p = lock(&self.progress).clone();
                if !p.is_empty() {
                    c.log = lines_of(&format!("Working…\n{}", p.join("\n")));
                }
            }
        }
    }

    fn pending_ws(&self) -> Option<WsInfo> {
        self.fixed_ws.clone().or_else(|| self.wss.get(self.wsel).map(|(n, r)| WsInfo { name: n.clone(), root: r.clone() }))
    }

    fn reload_rows_only(&mut self) {
        let keep = self.sel;
        let before = self.statuses.clone();
        // rebuild the rows without starting another status job
        let inst = installed::load(&self.pl.dir, &self.pl.user);
        let mut rows = Vec::new();
        for d in platform::specs() {
            let mine: Vec<&Installed> = inst.iter().filter(|i| i.service == d.id()).collect();
            let mut chips = vec![(d.kind().to_string(), MUTED)];
            chips.push(if mine.is_empty() { ("not installed".into(), DIM) } else { (format!("{} installed", mine.len()), OK) });
            rows.push(ListRow { kind: RowKind::Spec(d.id().into()), title: d.title(), sub: d.card(), chips });
            for i in mine {
                let live = before.get(&i.id).cloned().unwrap_or_else(|| "checking…".into());
                let mut chips = vec![(live.clone(), if live.starts_with("running") || live.starts_with("connected") { OK } else if live.starts_with("checking") { DIM } else if live.starts_with("stopped") { MUTED } else { AMBER }), (i.version.clone(), MUTED)];
                if i.connected {
                    chips.push(("existing".into(), AMBER));
                }
                let place = i.cluster.as_ref().map(|c| format!("{} · {}", c.name, i.namespace)).unwrap_or_else(|| "outside the cluster".into());
                rows.push(ListRow { kind: RowKind::Inst(i.id.clone()), title: format!("   {}", i.id), sub: format!("   {place} · {}", i.web_url().unwrap_or("no address")), chips });
            }
        }
        let reg = match self.registry_up {
            Some(true) => ("installed".to_string(), OK),
            Some(false) => ("not installed".to_string(), DIM),
            None => ("checking…".to_string(), DIM),
        };
        rows.push(ListRow { kind: RowKind::Registry, title: "Registry".into(), sub: "where images built on this device go; Enter creates it in one step (or shows it and offers Remove)".into(), chips: vec![("registry".into(), MUTED), reg] });
        self.rows = rows;
        self.sel = keep.min(self.rows.len().saturating_sub(1));
    }

    // ------------------------------------------------------------------ drawing

    pub fn draw(&mut self, g: &mut Gfx, a: R, s: f32) {
        if !self.loaded {
            self.reload();
        }
        let titles = match self.page {
            Page::List => ("Platform services", "Deploy Gitea or a private store into a cluster you choose. Every change is a plan you review and approve first."),
            Page::Cluster => ("Choose the cluster", "Only clusters you can see are listed; production-looking, read-only and unreachable ones are refused."),
            Page::Options => ("Options", "Defaults are filled in. Registration and passwords are handled for you."),
            Page::Review => ("Review the plan", "Exactly what will run. Select text with the mouse; Ctrl+C copies."),
            Page::Progress if self.store_run.is_some() => ("Working on the private store", "Deleting in the cluster first, then this device's entries. It keeps going if you hide this."),
            Page::Progress => ("Deploying", "Running the approved plan."),
            Page::Result => ("Result", "Addresses and how to use the service."),
            Page::Confirm => ("Review", ""),
            Page::Existing => ("Connect to an existing Gitea or Forgejo", "Nothing is deployed or changed in any cluster."),
            Page::Workspaces => ("Choose the workspace", "The repository is named after it."),
            Page::Choose => ("Choose a program file", "A static executable for the cluster's CPU. It is read, never run."),
            Page::Store => ("Private store", "What state it is in, and the ways to repair, reset or remove it. Reading changes nothing."),
        };
        let title = match (&self.page, &self.confirm, &self.wiz) {
            (Page::Confirm, Some(c), _) => c.title.clone(),
            (Page::Progress | Page::Result, _, _) if self.store_run.is_some() || self.store_retry.is_some() || (self.store_rep.is_some() && self.store_after) => "Private store".to_string(),
            (Page::Cluster | Page::Options | Page::Review | Page::Progress | Page::Result | Page::Choose, _, Some(w)) => format!("Deploy {}: {}", platform::spec(&w.service).map(|d| d.title()).unwrap_or_default(), titles.0.to_lowercase()),
            _ => titles.0.to_string(),
        };
        crate::fx::glass(g, a.x, a.y, a.w, a.h, 12.0 * s, ACCENT);
        let tx = a.x + 16.0 * s;
        let t = g.fit(19.0 * s, &title, Face::Bold, a.w - 32.0 * s);
        g.text(tx, a.y + 10.0 * s, 19.0 * s, TEXT, &t, Face::Bold);
        let sub = g.fit(12.5 * s, titles.1, Face::Sans, a.w - 32.0 * s);
        g.text(tx, a.y + 36.0 * s, 12.5 * s, MUTED, &sub, Face::Sans);
        let top = a.y + 58.0 * s;
        // the buttons of this page, laid out first so the body gets what is left
        let items = self.buttons();
        let bs = 13.0 * s;
        let bw: Vec<f32> = items.iter().map(|b| g.text_width(bs, &b.0, Face::Bold) + bs * 1.7 + 8.0 * s).collect();
        let (mut rows_n, mut used) = (usize::from(!items.is_empty()), 0.0f32);
        for w in &bw {
            if used + w > a.w - 28.0 * s && used > 0.0 {
                rows_n += 1;
                used = 0.0;
            }
            used += w;
        }
        let bar_h = rows_n as f32 * (bs * 2.1 + 6.0 * s);
        let msg_h = 20.0 * s;
        let body = R::new(a.x + 14.0 * s, top, a.w - 28.0 * s, (a.h - (top - a.y) - bar_h - msg_h - 10.0 * s).max(40.0 * s));
        match self.page.clone() {
            Page::List => self.draw_list(g, body, s),
            Page::Cluster => self.draw_clusters(g, body, s),
            Page::Options => self.draw_options(g, body, s),
            Page::Review => self.log.render(g, R::new(body.x, body.y, body.w, body.h), 13.0 * s, Face::Mono),
            Page::Progress => self.draw_progress(g, body, s),
            Page::Result => self.draw_result(g, body, s),
            Page::Choose => self.draw_choose(g, body, s),
            Page::Store => self.log.render(g, R::new(body.x, body.y, body.w, body.h), 13.0 * s, Face::Mono),
            Page::Confirm => self.draw_confirm(g, body, s),
            Page::Existing => self.draw_existing(g, body, s),
            Page::Workspaces => self.draw_workspaces(g, body, s),
        }
        if let Some((m, ok)) = &self.msg {
            let t = g.fit(13.0 * s, m, Face::Sans, a.w - 28.0 * s);
            g.text(a.x + 14.0 * s, a.y + a.h - bar_h - msg_h - 2.0 * s, 13.0 * s, if *ok { MUTED } else { ERR }, &t, Face::Sans);
        }
        let (mut bx, mut by) = (a.x + 14.0 * s, a.y + a.h - bar_h - 2.0 * s);
        for ((label, col, key), w) in items.iter().zip(&bw) {
            if bx + w > a.x + a.w - 14.0 * s && bx > a.x + 14.0 * s {
                bx = a.x + 14.0 * s;
                by += bs * 2.1 + 6.0 * s;
            }
            g.button(bx, by, bs, label, *col, Event::Key(key.clone()));
            bx += w;
        }
        if !self.reveal.is_empty() {
            self.draw_reveal(g, a, s);
        }
    }

    fn buttons(&self) -> Vec<(String, [u8; 4], Key)> {
        let b = |l: &str, c: [u8; 4], k: Key| (l.to_string(), c, k);
        match &self.page {
            Page::List => {
                let mut v = vec![];
                match self.current().map(|r| &r.kind) {
                    Some(RowKind::Spec(id)) => {
                        v.push(b("Deploy to k3s…", ACCENT, Key::Char('d')));
                        if id == privatestore::ID {
                            v.push(b("Repair or reset…", TEXT, Key::Char('m')));
                        }
                        if platform::spec(id).is_some_and(|d| d.kind() == "git") {
                            v.push(b("Connect to existing", TEXT, Key::Char('c')));
                        }
                    }
                    Some(RowKind::Inst(id)) => {
                        let stopped = self.statuses.get(id).is_some_and(|s| s.starts_with("stopped"));
                        v.push(b("Open", ACCENT, Key::Char('o')));
                        v.push(b("Details", TEXT, Key::Enter));
                        v.push(b(if stopped { "Start" } else { "Stop" }, TEXT, Key::Char('s')));
                        v.push(b("Upgrade", TEXT, Key::Char('u')));
                        v.push(b("Backup", TEXT, Key::Char('b')));
                        if installed::find(&self.pl.dir, &self.pl.user, id).is_some_and(|i| i.kind == "git") {
                            v.push(b("Connect workspace", TEXT, Key::Char('w')));
                        }
                        if installed::find(&self.pl.dir, &self.pl.user, id).is_some_and(|i| i.service == privatestore::ID) {
                            v.push(b("Repair or reset…", TEXT, Key::Char('m')));
                        }
                        v.push(b("Remove", ERR, Key::Char('x')));
                    }
                    _ => {}
                }
                v.push(b("Look again", MUTED, Key::Char('r')));
                v.push(b("Close", MUTED, Key::Esc));
                v
            }
            Page::Cluster => vec![b("Next", ACCENT, Key::Enter), b("Look again", MUTED, Key::Char('r')), b("Back", MUTED, Key::Esc)],
            Page::Options => vec![b("Review the plan", ACCENT, Key::Ctrl('s')), b("Back", MUTED, Key::Esc)],
            Page::Review => {
                let ok = self.wiz.as_ref().and_then(|w| w.plan.as_ref()).is_some_and(|p| p.can_apply());
                let mut v = vec![];
                if ok {
                    v.push(b("Approve and deploy", ACCENT, Key::Char('y')));
                } else if self.svc.is_some() && self.wiz.as_ref().and_then(|w| w.plan.as_ref()).is_some_and(|p| p.needs_registry) {
                    v.push(b("Create registry", ACCENT, Key::Char('g')));
                }
                v.push(b("Back", MUTED, Key::Esc));
                v
            }
            Page::Progress if self.build.is_some() => vec![b("Stop the build", ERR, Key::Esc)],
            Page::Progress if self.reg_run.is_some() || self.store_run.is_some() => vec![b("Hide (keeps running)", MUTED, Key::Esc)],
            Page::Progress => vec![],
            Page::Result if self.fail.is_some() => {
                let mut v = vec![];
                if !self.show_docs {
                    if self.can_build() {
                        v.push(b("Build now", ACCENT, Key::Char('b')));
                    }
                    v.push(b("Choose a binary file…", TEXT, Key::Char('f')));
                }
                v.push(b(if self.show_docs { "Back to the result" } else { "Open docs" }, TEXT, Key::Char('d')));
                v.push(b("Try again", TEXT, Key::Char('r')));
                v.push(b("Close", MUTED, Key::Enter));
                v
            }
            Page::Result if self.store_retry.is_some() => vec![b("Reset again, clearing finalizers", ERR, Key::Char('f')), b("Close", MUTED, Key::Enter)],
            Page::Result if self.reg_retry.is_some() && self.fail.is_none() => vec![b("Retry", ACCENT, Key::Char('g')), b("Close", MUTED, Key::Enter)],
            Page::Result => {
                let mut v = vec![];
                if !self.result_urls.is_empty() {
                    v.push(b("Open", ACCENT, Key::Char('o')));
                }
                v.push(b("Close", MUTED, Key::Enter));
                v
            }
            Page::Confirm => match &self.confirm {
                Some(c) if c.busy => vec![],
                Some(c) if c.done => vec![b("Close", MUTED, Key::Enter)],
                Some(c) if matches!(c.act, Act::Close) => vec![b("Reveal password", TEXT, Key::Char('v')), b("Close", MUTED, Key::Esc)],
                Some(c) => {
                    let mut v = vec![];
                    if c.blocked.is_empty() {
                        v.push(b("Approve", if c.danger { ERR } else { ACCENT }, if c.typed.is_some() { Key::Enter } else { Key::Char('y') }));
                    }
                    if matches!(c.act, Act::Remove(_)) {
                        v.push(b("Keep/delete data", MUTED, Key::Ctrl('k')));
                    }
                    match &c.act {
                        Act::StoreDo { action: pm::Action::Remove(_), .. } => {
                            v.push(b("Keep/delete data", MUTED, Key::Ctrl('k')));
                            v.push(b("Registry too", MUTED, Key::Ctrl('r')));
                        }
                        Act::StoreDo { action: pm::Action::Reset, .. } => v.push(b("Clear finalizers", MUTED, Key::Ctrl('f'))),
                        _ => {}
                    }
                    if matches!(c.act, Act::CreateRegistry { .. }) {
                        v.push(b("Volume size", MUTED, Key::Ctrl('k')));
                    }
                    v.push(b("Cancel", MUTED, Key::Esc));
                    v
                }
                None => vec![b("Close", MUTED, Key::Esc)],
            },
            Page::Existing => vec![b("Approve and connect", ACCENT, Key::Ctrl('s')), b("Cancel", MUTED, Key::Esc)],
            Page::Workspaces => vec![b("Next", ACCENT, Key::Enter), b("Back", MUTED, Key::Esc)],
            Page::Choose => vec![b("Use this file", ACCENT, Key::Enter), b("Back", MUTED, Key::Esc)],
            Page::Store => {
                let mut v = vec![];
                if let Some(r) = &self.store_rep {
                    let o = r.offers();
                    if o.deploy {
                        v.push(b("Deploy…", ACCENT, Key::Char('d')));
                    }
                    if o.repair {
                        v.push(b("Repair", ACCENT, Key::Char('r')));
                    }
                    if o.reset {
                        v.push(b("Reset and reinstall", ERR, Key::Char('x')));
                    }
                    if o.remove {
                        v.push(b("Remove", ERR, Key::Char('m')));
                    }
                    if o.start_cluster && self.can_start_cluster() {
                        v.push(b("Start the cluster", ACCENT, Key::Char('s')));
                    }
                    v.push(b("Look again", MUTED, Key::Char('l')));
                }
                v.push(b("Back", MUTED, Key::Esc));
                v
            }
        }
    }

    fn draw_rows(g: &mut Gfx, body: R, s: f32, rows: &[(String, String, Vec<(String, [u8; 4])>, bool)], sel: usize) {
        let (rh, gap) = (52.0 * s, 6.0 * s);
        let visible = (((body.h + gap) / (rh + gap)).floor().max(1.0)) as usize;
        let first = sel.saturating_sub(visible - 1).min(rows.len().saturating_sub(visible));
        let mut y = body.y;
        for (i, (title, sub, chips, dim)) in rows.iter().enumerate().skip(first).take(visible) {
            let chips_w: f32 = chips.iter().take(4).map(|(t, _)| g.text_width(12.0 * s, t, Face::Bold) + 24.0 * s).sum();
            let hot = g.hovered(R::new(body.x, y, body.w, rh));
            crate::fx::row_at(g, body.x, y, body.w, rh, 12.0 * s, i == sel, hot, ACCENT);
            g.hit(R::new(body.x, y, body.w, rh), Event::Row(i), title);
            if i == sel {
                g.rect(body.x, y + 10.0 * s, 4.0 * s, rh - 20.0 * s, ACCENT);
            }
            let t = g.fit(16.0 * s, title, Face::Bold, (body.w - 36.0 * s - chips_w).max(80.0 * s));
            g.text(body.x + 18.0 * s, y + 7.0 * s, 16.0 * s, if *dim { DIM } else { TEXT }, &t, Face::Bold);
            let sub = g.fit(12.5 * s, sub, Face::Mono, body.w - 36.0 * s);
            g.text(body.x + 18.0 * s, y + 31.0 * s, 12.5 * s, MUTED, &sub, Face::Mono);
            let mut cx = body.x + body.w - 14.0 * s;
            for (label, col) in chips.iter().take(4).rev() {
                let cw = g.text_width(12.0 * s, label, Face::Bold) + 18.0 * s;
                cx -= cw;
                crate::fx::chip(g, cx, y + 8.0 * s, s, label, *col, false);
                cx -= 6.0 * s;
            }
            y += rh + gap;
        }
        if rows.len() > visible {
            g.text(body.x + body.w - 70.0 * s, body.y + body.h - 14.0 * s, 11.0 * s, DIM, &format!("{} of {}", sel + 1, rows.len()), Face::Sans);
        }
    }

    fn draw_list(&mut self, g: &mut Gfx, body: R, s: f32) {
        let rows: Vec<_> = self.rows.iter().map(|r| (r.title.clone(), r.sub.clone(), r.chips.clone(), false)).collect();
        Self::draw_rows(g, body, s, &rows, self.sel);
    }

    fn draw_clusters(&mut self, g: &mut Gfx, body: R, s: f32) {
        let Some(w) = &self.wiz else { return };
        if w.loading {
            g.text(body.x, body.y + 6.0 * s, 14.0 * s, MUTED, "Looking for clusters…", Face::Sans);
            return;
        }
        if w.profiles.is_empty() {
            let t = g.fit(14.0 * s, "No cluster was found. Install k3s, or add a kubeconfig on the Dev cluster page.", Face::Sans, body.w);
            g.text(body.x, body.y + 6.0 * s, 14.0 * s, MUTED, &t, Face::Sans);
            return;
        }
        let rows: Vec<_> = w
            .profiles
            .iter()
            .map(|l| {
                let p = &l.profile;
                let mut chips: Vec<(String, [u8; 4])> = vec![(p.state().to_string(), if p.reachable { OK } else { ERR })];
                if p.k3s {
                    chips.push(("k3s".into(), MUTED));
                }
                chips.push(if p.local { ("this machine".into(), MUTED) } else { ("remote".into(), AMBER) });
                if p.source == devcluster::Source::InCluster {
                    chips.push(("in-cluster".into(), AMBER));
                }
                if p.looks_like_production() {
                    chips.push(("production-looking".into(), ERR));
                }
                if devcluster::read_only_reason(p).is_some() {
                    chips.push(("read-only".into(), AMBER));
                }
                let title = if p.context.is_empty() || p.context == p.name { p.name.clone() } else { format!("{} · {}", p.name, p.context) };
                let sub = match &l.refusal {
                    Some(r) => r.trim_start_matches("refused: ").to_string(),
                    None => format!("{} · {} · from {}", if p.server.is_empty() { "server unknown" } else { &p.server }, p.version, p.source.label()),
                };
                (title, sub, chips, l.refusal.is_some())
            })
            .collect();
        Self::draw_rows(g, body, s, &rows, w.psel);
    }

    fn draw_options(&mut self, g: &mut Gfx, body: R, s: f32) {
        let tick = self.tick;
        let Some(w) = self.wiz.as_mut() else { return };
        let cluster = w.chosen.as_ref().map(|p| format!("{} at {}", p.name, p.server)).unwrap_or_default();
        let help_h = 40.0 * s;
        let head_h = 22.0 * s;
        let t = g.fit(12.5 * s, &format!("Cluster: {cluster}"), Face::Sans, body.w);
        g.text(body.x, body.y, 12.5 * s, MUTED, &t, Face::Sans);
        let list = R::new(body.x, body.y + head_h, body.w, (body.h - head_h - help_h).max(36.0 * s));
        let rh = 34.0 * s;
        let visible = ((list.h / rh).floor().max(1.0)) as usize;
        let sel = w.osel;
        let first = sel.saturating_sub(visible - 1).min(w.fields.len().saturating_sub(visible));
        let label_w = (list.w * 0.30).clamp(110.0 * s, 240.0 * s);
        let mut y = list.y;
        for i in first..(first + visible).min(w.fields.len()) {
            let f = &w.fields[i];
            let focused = i == sel;
            g.hit(R::new(list.x, y, label_w, rh), Event::Row(i), &f.def.label);
            let t = g.fit(13.5 * s, &f.def.label, Face::Bold, label_w - 8.0 * s);
            g.text(list.x, y + 8.0 * s, 13.5 * s, if focused { TEXT } else { MUTED }, &t, Face::Bold);
            let r = R::new(list.x + label_w, y + 1.0 * s, list.w - label_w, rh - 4.0 * s);
            match &f.choices {
                Some((c, idx)) => {
                    crate::fx::field_frame(g, r, s, focused, ACCENT);
                    g.text(r.x + 12.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &format!("◀  {}  ▶", c[*idx]), Face::Sans);
                    g.hit(R::new(r.x, r.y, r.w / 2.0, r.h), Event::Row(i), "prev");
                    g.hit(R::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h), Event::Row(i), "next");
                }
                None => f.tf.render(g, r, focused, s, crate::apps::blink(tick), Face::Sans, i),
            }
            y += rh;
        }
        if w.fields.len() > visible {
            g.text(list.x + list.w - 60.0 * s, body.y, 11.0 * s, DIM, &format!("{} of {}", sel + 1, w.fields.len()), Face::Sans);
        }
        if let Some(f) = w.fields.get(sel) {
            let help = g.wrap(12.5 * s, &f.def.help, Face::Sans, body.w);
            for (i, l) in help.iter().take(2).enumerate() {
                g.text(body.x, body.y + body.h - help_h + 4.0 * s + i as f32 * 17.0 * s, 12.5 * s, DIM, l, Face::Sans);
            }
        }
    }

    fn draw_confirm(&mut self, g: &mut Gfx, body: R, s: f32) {
        let Some(c) = self.confirm.as_mut() else { return };
        let typed_h = if c.typed.is_some() && !c.done && !c.busy { 46.0 * s } else { 0.0 };
        c.log.render(g, R::new(body.x, body.y, body.w, (body.h - typed_h).max(30.0 * s)), 13.0 * s, Face::Mono);
        if let (Some(tf), true) = (c.typed.as_ref(), typed_h > 0.0) {
            let r = R::new(body.x, body.y + body.h - typed_h + 6.0 * s, body.w.min(420.0 * s), 34.0 * s);
            tf.render(g, r, true, s, crate::apps::blink(self.tick), Face::Sans, 0);
        }
    }

    fn draw_existing(&mut self, g: &mut Gfx, body: R, s: f32) {
        let help = "IYAKKI will call GET <address>/api/v1/version and GET <address>/api/v1/user with the token to check them, then keep the token only in your vault. Create the token in the service: Settings, Applications, with the repository write scope. Nothing in any cluster is changed.";
        let lines = g.wrap(12.5 * s, help, Face::Sans, body.w);
        let mut y = body.y;
        for l in lines.iter().take(4) {
            g.text(body.x, y, 12.5 * s, MUTED, l, Face::Sans);
            y += 17.0 * s;
        }
        y += 8.0 * s;
        for (i, (label, tf)) in [("Address of the service", &self.url), ("Access token (typed here, goes to your vault, never shown again)", &self.token)].into_iter().enumerate() {
            g.text(body.x, y, 13.0 * s, if i == self.existing_focus { TEXT } else { MUTED }, label, Face::Bold);
            let r = R::new(body.x, y + 20.0 * s, body.w.min(640.0 * s), 36.0 * s);
            tf.render(g, r, i == self.existing_focus, s, crate::apps::blink(self.tick), Face::Sans, i);
            y += 70.0 * s;
        }
    }

    /// The result: when a program is missing, the log sits in a red card (the reason, then what can be done).
    fn draw_result(&mut self, g: &mut Gfx, body: R, s: f32) {
        if self.fail.is_some() && !self.show_docs {
            crate::fx::glass(g, body.x, body.y, body.w, body.h, 10.0 * s, ERR);
            let inner = R::new(body.x + 10.0 * s, body.y + 8.0 * s, (body.w - 20.0 * s).max(40.0 * s), (body.h - 16.0 * s).max(30.0 * s));
            self.log.render(g, inner, 13.0 * s, Face::Mono);
        } else {
            self.log.render(g, body, 13.0 * s, Face::Mono);
        }
    }

    /// The progress log; while a build runs, a bar below it moves (it fills when the output gives a percentage).
    fn draw_progress(&mut self, g: &mut Gfx, body: R, s: f32) {
        let Some(b) = self.build.as_ref() else { return self.log.render(g, body, 13.0 * s, Face::Mono) };
        let bar_h = 14.0 * s;
        let bar = R::new(body.x, body.y + body.h - bar_h, body.w, bar_h);
        let log = R::new(body.x, body.y, body.w, (body.h - bar_h - 8.0 * s).max(30.0 * s));
        match b.progress {
            Some(p) => crate::fx::progress_bar(g, bar, p / 100.0, ACCENT, s),
            None => {
                g.rrect(bar.x, bar.y, bar.w, bar.h, bar.h / 2.0, rgb(22, 29, 54));
                let seg = bar.w * 0.3;
                let pos = (self.tick as f32 * 2.2 * s.max(0.8)) % (bar.w + seg) - seg;
                g.clipped(bar, |g| g.rrect(bar.x + pos, bar.y, seg, bar.h, bar.h / 2.0, ACCENT));
            }
        }
        self.log.render(g, log, 13.0 * s, Face::Mono);
    }

    fn draw_choose(&mut self, g: &mut Gfx, body: R, s: f32) {
        let want = self.fail.as_ref().map(|f| (f.lb.file, f.disc.want.uname())).unwrap_or(("the program", "x86_64"));
        let help = format!("Type the path of {}-{} (a prebuilt static file, for example the one from the release page). IYAKKI reads its ELF header to check that it is static and built for {}; it is never run. A good file is copied into your IYAKKI data folder and the deploy continues.", want.0, want.1, want.1);
        let mut y = body.y;
        for l in g.wrap(12.5 * s, &help, Face::Sans, body.w).iter().take(5) {
            g.text(body.x, y, 12.5 * s, MUTED, l, Face::Sans);
            y += 17.0 * s;
        }
        y += 10.0 * s;
        g.text(body.x, y, 13.0 * s, TEXT, "Path of the file", Face::Bold);
        let r = R::new(body.x, y + 20.0 * s, body.w.min(700.0 * s), 36.0 * s);
        self.choose.render(g, r, true, s, crate::apps::blink(self.tick), Face::Sans, 0);
        y += 66.0 * s;
        if !self.choose_err.is_empty() {
            for l in g.wrap(13.0 * s, &self.choose_err, Face::Sans, body.w).iter().take(5) {
                g.text(body.x, y, 13.0 * s, ERR, l, Face::Sans);
                y += 18.0 * s;
            }
        }
    }

    fn draw_workspaces(&mut self, g: &mut Gfx, body: R, s: f32) {
        let rows: Vec<_> = self.wss.iter().map(|(n, r)| (n.clone(), r.display().to_string(), vec![("workspace".to_string(), MUTED)], false)).collect();
        Self::draw_rows(g, body, s, &rows, self.wsel);
    }

    fn draw_reveal(&mut self, g: &mut Gfx, a: R, s: f32) {
        let Some(r) = self.reveal.first() else { return };
        let p = crate::fx::modal(g, a, s, 560.0 * s, 250.0 * s, ACCENT, false);
        let y0 = crate::fx::modal_title(g, p, s, "Shown once", ACCENT);
        let t = g.fit(14.0 * s, &r.label, Face::Bold, p.w - 52.0 * s);
        g.text(p.x + 26.0 * s, y0 + 4.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
        let fr = R::new(p.x + 26.0 * s, y0 + 30.0 * s, p.w - 52.0 * s, 40.0 * s);
        crate::fx::field_frame(g, fr, s, false, ACCENT);
        let shown = if r.shown { r.value.to_string() } else { "\u{2022}".repeat(r.value.chars().count().min(32)) };
        g.text(fr.x + 14.0 * s, fr.y + 9.0 * s, 17.0 * s, TEXT, &shown, Face::Mono);
        let note = g.wrap(12.0 * s, &format!("It is also in your vault as uvault:{} and in the cluster's Secret. Copy it into your password manager; Copy puts it in the clipboard history, which is cleared again when you press Done.", r.vault), Face::Sans, p.w - 52.0 * s);
        for (i, l) in note.iter().take(3).enumerate() {
            g.text(p.x + 26.0 * s, fr.y + fr.h + 10.0 * s + i as f32 * 16.0 * s, 12.0 * s, DIM, l, Face::Sans);
        }
        let by = p.y + p.h - 50.0 * s;
        let mut bx = p.x + 26.0 * s;
        bx += g.button(bx, by, 13.0 * s, if r.shown { "Hide" } else { "Show" }, TEXT, Event::Key(Key::Char('s'))) + 10.0 * s;
        bx += g.button(bx, by, 13.0 * s, if r.copied { "Copied" } else { "Copy" }, TEXT, Event::Key(Key::Char('c'))) + 10.0 * s;
        g.button(bx, by, 13.0 * s, "Done", ACCENT, Event::Key(Key::Enter));
    }
}

/// The part of docs/STORE.md that says where the server program comes from (kept in one place; a test checks it is found).
fn server_program_docs() -> String {
    const STORE: &str = include_str!("../../../../docs/STORE.md");
    let start = STORE.find("### 13.1 The server program").unwrap_or(0);
    let rest = &STORE[start..];
    let end = rest[10..].find("\n## ").or_else(|| rest[10..].find("\n### ")).map(|e| e + 10).unwrap_or(rest.len());
    format!("THE SERVER PROGRAM (docs/STORE.md, section 13)\n\n{}\n", rest[..end].trim_end())
}

/// The Platform context of one signed-in person for the shell: the executor and cluster places of the host (a fake in tests).
pub fn platform_for(cx: &super::AppCx, run: Arc<dyn iyakki_agent::wsexec::Runner>, env: devcluster::Env) -> Platform {
    let audit = cx.audit.clone();
    let user = cx.session.user.clone();
    let tmp = iyakki_agent::kubeprofiles::temp_dir().unwrap_or_else(std::env::temp_dir);
    let u2 = user.clone();
    Platform {
        run: run.clone(),
        dir: cx.services.dir().to_path_buf(),
        user: user.clone(),
        vault: Arc::new(platform::secrets::ServicesVault { svc: cx.services.clone(), user }),
        audit: Arc::new(move |a, d| audit.record(&u2, a, d)),
        env,
        tmp: tmp.clone(),
        http: Arc::new(connect::CurlHttp { run, tmp }),
        gen: Arc::new(platform::secrets::random_secret),
        live: None,
    }
}

#[cfg(test)]
#[path = "platform_services_tests.rs"]
mod tests;
