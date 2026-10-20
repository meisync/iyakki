//! The AI IDE's K8S view: the workspace's namespace as tables (workloads, services, ingresses, pods, config names, storage,
//! nodes), a details drawer for a pod with a log tail, and plain-English states for "kubectl missing" and "cluster
//! unreachable". The header shows the connected cluster profile (`k3s · default · reachable`) with a Change cluster chooser; Deploy and
//! Restart run the approved steps of `ide_deploy`.
//! The cluster is polled on a background thread (`iyakki_agent::k8s::Feed`) only while this view is drawn.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::devcluster::Slot;
use iyakki_agent::regsetup::RegStatus;
use iyakki_agent::k8s::{self, age_text, Cluster, Feed, Health, Kube, Snapshot, Target};

use super::ide_deploy::{Answer, Flow, St};

use super::widgets::{PickEvent, Picker};
use super::ScrollLog;
use crate::fx::{self, with_alpha, Glyph, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::textsel;
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

pub const TAB_BASE: usize = 9_300_000;
pub const NS_BASE: usize = 9_310_000;
pub const ROW_BASE: usize = 200_000;
/// `Event::Sub` code of this view's buttons and keys.
pub const SUB: u8 = 31;
pub const DETAIL_AREA: usize = 62_002;
/// The text area of a deploy / registry flow step (selectable like the chat transcript).
pub const FLOW_AREA: usize = 62_020;
/// What the view asks its owner (the IDE) to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// Look for the cluster again.
    Reresolve,
    /// The person chose profile number n in the chooser.
    Choose(usize),
    /// Open the deploy form.
    Deploy,
    /// Restart this workload (after the person pressed the button on its row).
    Restart(String, String),
    /// Open the "Set up registry" dialog.
    Registry,
    /// Tear down: scale the workspace namespace to 0 / back up, delete it (the IDE asks for the typed name first), or delete the
    /// images pushed for it from the registry.
    Stop,
    Start,
    RemoveAll,
    RegistryImages,
    /// Hide this kubeconfig file in IYAKKI (never probed, listed or used; the file itself is untouched).
    Hide(String),
    /// Bring the hidden clusters back.
    ShowHidden,
    /// Forget this workspace's cluster choice (project.json and the per-user choice); the machine's own k3s is found again.
    Disconnect,
    /// Read the saved registry choice and ask whether it answers (fills [`K8sView::reg`]).
    RegistryProbe,
    /// Show this address (a Service's NodePort or LoadBalancer address) in the IDE's Browser tab.
    OpenUrl(String),
    /// Open Platform services (Gitea, the private store); `connect` goes straight to connecting this workspace to a git service.
    Services { connect: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Workloads,
    Services,
    Ingresses,
    Pods,
    Config,
    Storage,
    Nodes,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Workloads, Kind::Services, Kind::Ingresses, Kind::Pods, Kind::Config, Kind::Storage, Kind::Nodes];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Workloads => "Workloads",
            Kind::Services => "Services",
            Kind::Ingresses => "Ingresses",
            Kind::Pods => "Pods",
            Kind::Config => "Config",
            Kind::Storage => "Storage",
            Kind::Nodes => "Nodes",
        }
    }
    pub fn glyph(self) -> Glyph {
        match self {
            Kind::Workloads => Glyph::Workloads,
            Kind::Services => Glyph::Services,
            Kind::Ingresses => Glyph::Ingresses,
            Kind::Pods => Glyph::Pods,
            Kind::Config => Glyph::Config,
            Kind::Storage => Glyph::Storage,
            Kind::Nodes => Glyph::Nodes,
        }
    }
    fn next(self, d: i32) -> Kind {
        let i = Kind::ALL.iter().position(|k| *k == self).unwrap_or(0) as i32;
        Kind::ALL[(i + d).rem_euclid(Kind::ALL.len() as i32) as usize]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TRow {
    pub cells: Vec<String>,
    pub health: Option<Health>,
    /// The name (pods and workloads can be opened).
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Column title and its share of the width.
    pub cols: Vec<(&'static str, f32)>,
    pub rows: Vec<TRow>,
}

pub fn health_color(h: Health) -> [u8; 4] {
    match h {
        Health::Good => OK,
        Health::Warn => AMBER,
        Health::Bad => ERR,
        Health::Idle => rgb(140, 150, 185),
    }
}

/// The table for one kind of resource.
pub fn table(snap: &Snapshot, kind: Kind) -> Table {
    let row = |cells: Vec<String>, health: Option<Health>, key: &str| TRow { cells, health, key: key.to_string() };
    match kind {
        Kind::Workloads => Table {
            cols: vec![("Name", 0.24), ("Kind", 0.13), ("Ready", 0.09), ("Image", 0.30), ("Age", 0.08), ("Status", 0.16)],
            rows: snap.workloads.iter().map(|w| row(vec![w.name.clone(), w.kind.clone(), format!("{}/{}", w.ready, w.desired), w.images.join(", "), age_text(w.age), w.status.clone()], Some(w.health), &w.name)).collect(),
        },
        Kind::Services => Table {
            cols: vec![("Name", 0.20), ("Type", 0.14), ("Cluster IP", 0.16), ("Ports", 0.22), ("External", 0.20), ("Age", 0.08)],
            rows: snap.services.iter().map(|x| row(vec![x.name.clone(), x.kind.clone(), x.cluster_ip.clone(), x.ports.clone(), if x.external.is_empty() { "-".into() } else { x.external.clone() }, age_text(x.age)], None, &x.name)).collect(),
        },
        Kind::Ingresses => Table {
            cols: vec![("Name", 0.22), ("Class", 0.14), ("Hosts", 0.38), ("Address", 0.18), ("Age", 0.08)],
            rows: snap.ingresses.iter().map(|x| row(vec![x.name.clone(), if x.class.is_empty() { "-".into() } else { x.class.clone() }, x.hosts.clone(), if x.address.is_empty() { "-".into() } else { x.address.clone() }, age_text(x.age)], None, &x.name)).collect(),
        },
        Kind::Pods => Table {
            cols: vec![("Name", 0.28), ("Ready", 0.08), ("Status", 0.19), ("Restarts", 0.11), ("Node", 0.19), ("Age", 0.08)],
            rows: snap.pods.iter().map(|p| row(vec![p.name.clone(), p.ready.clone(), if p.reason.is_empty() { p.phase.clone() } else { p.reason.clone() }, p.restarts.to_string(), p.node.clone(), age_text(p.age)], Some(p.health), &p.name)).collect(),
        },
        Kind::Config => Table {
            cols: vec![("Name", 0.5), ("Kind", 0.2), ("Value", 0.3)],
            rows: snap.configmaps.iter().map(|n| row(vec![n.clone(), "ConfigMap".into(), "names only".into()], None, n)).chain(snap.secrets.iter().map(|n| row(vec![n.clone(), "Secret".into(), "value hidden".into()], None, n))).collect(),
        },
        Kind::Storage => Table {
            cols: vec![("Name", 0.4), ("Status", 0.2), ("Size", 0.2), ("Class", 0.2)],
            rows: snap.pvcs.iter().map(|p| row(vec![p.name.clone(), p.status.clone(), p.size.clone(), p.class.clone()], Some(if p.status == "Bound" { Health::Good } else { Health::Warn }), &p.name)).collect(),
        },
        Kind::Nodes => Table {
            cols: vec![("Name", 0.24), ("Status", 0.12), ("Roles", 0.24), ("Version", 0.18), ("CPU", 0.08), ("Memory", 0.14)],
            rows: snap.nodes.iter().map(|n| row(vec![n.name.clone(), if n.ready { "Ready".into() } else { "NotReady".into() }, n.roles.clone(), n.version.clone(), n.cpu.clone(), n.memory.clone()], Some(if n.ready { Health::Good } else { Health::Bad }), &n.name)).collect(),
        },
    }
}

/// What the view says when it has no cluster data, in plain English.
pub fn banner(snap: &Snapshot) -> Option<(String, String)> {
    match &snap.cluster {
        Cluster::Ok => None,
        Cluster::KubectlMissing => Some(("kubectl was not found".into(), "Install kubectl (or the k3s bundle: CLI → CLIs → k3s), then press r. Cluster profiles live in Settings → Dev cluster; the plan is in docs/K3S-DEV.md.".into())),
        Cluster::Unreachable(m) => Some(("The cluster is not reachable".into(), format!("{m}. Check that the cluster is running (k3s status), that the VPN or SSH tunnel is up, and that the kubeconfig points at it (the profile in .iyakki/project.json or $KUBECONFIG). Settings → Dev cluster and docs/K3S-DEV.md explain the profiles. Press r to try again."))),
        Cluster::Forbidden(m) => Some(("This account may not read the cluster".into(), format!("{m}. Ask for a read role in this namespace."))),
        Cluster::Failed(m) => Some(("kubectl failed".into(), format!("{m}. Press r to try again."))),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Drawer {
    None,
    Detail(String),
    Logs(String),
}

pub struct K8sView {
    pub feed: Feed,
    pub kind: Kind,
    pub sel: usize,
    scroll: usize,
    drawer: Drawer,
    pub detail: ScrollLog,
    picker: Option<Picker>,
    last_draw: Option<Instant>,
    visible_rows: usize,
    pub status: Option<String>,
    detail_fresh: bool,
    /// The workspace's cluster resolution (shared with the chat and the tools).
    pub slot: Option<Slot>,
    /// A running deploy or restart.
    pub flow: Option<Flow>,
    pub wants: Vec<Want>,
    cluster_picker: Option<Picker>,
    teardown_picker: Option<Picker>,
    /// Why Deploy and Restart are off (no cluster, read-only workspace, no terminal rights); `None`: they work.
    pub deploy_block: Option<String>,
    /// The Registry strip: the saved choice and whether it answers (filled by the owner on `Want::RegistryProbe`).
    pub reg: Arc<Mutex<Option<RegStatus>>>,
    /// Selection over the text of the flow step in view; cleared when the step in view changes.
    pub flow_pane: textsel::Pane,
    flow_key: Option<(usize, St)>,
}

impl K8sView {
    pub fn new(kube: Arc<dyn Kube>, target: Target) -> K8sView {
        let feed = Feed::start(kube, target, Duration::from_secs(5), Duration::from_secs(2));
        let mut detail = ScrollLog::default();
        detail.pane = textsel::Pane::new(DETAIL_AREA);
        let mut v = K8sView { feed, kind: Kind::Workloads, sel: 0, scroll: 0, drawer: Drawer::None, detail, picker: None, last_draw: None, visible_rows: 10, status: None, detail_fresh: false, slot: None, flow: None, wants: vec![], cluster_picker: None, teardown_picker: None, deploy_block: Some("no cluster is connected yet".into()), reg: Arc::new(Mutex::new(None)), flow_pane: textsel::Pane::new(FLOW_AREA), flow_key: None };
        v.wants.push(Want::RegistryProbe);
        v
    }

    pub fn set_target(&mut self, t: Target) {
        self.feed.set_target(t);
        self.sel = 0;
        self.scroll = 0;
        self.drawer = Drawer::None;
        self.feed.follow(None);
    }

    /// Called every heartbeat: the cluster is only polled while the view was drawn a moment ago.
    pub fn tick(&mut self) {
        let vis = self.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(500));
        self.feed.set_visible(vis);
        if let Drawer::Logs(p) = &self.drawer {
            if let Some((name, res)) = self.feed.logs() {
                if &name == p {
                    self.detail.lines = match res {
                        Ok(l) => l.into_iter().map(|x| (x, [0, 0, 0, 0])).collect(),
                        Err(e) => vec![(format!("could not read the log: {e}"), ERR)],
                    };
                    self.detail.scroll = 0;
                }
            }
        }
        if let Drawer::Detail(p) = &self.drawer {
            if let Some((name, res)) = self.feed.detail() {
                if &name == p {
                    self.detail.lines = detail_lines(&res);
                    if self.detail_fresh {
                        // a pod's details read from the top; a log from its end
                        self.detail.scroll = 1_000_000;
                        self.detail_fresh = false;
                    }
                }
            }
        }
        self.detail.on_tick();
    }

    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.feed.latest()
    }

    fn table(&self) -> Option<Table> {
        self.snapshot().map(|s| table(&s, self.kind))
    }

    fn selected_key(&self) -> Option<String> {
        self.table().and_then(|t| t.rows.get(self.sel).map(|r| r.key.clone()))
    }

    /// The pod to show for the selected row: a pod row itself, or the first pod of a workload.
    fn pod_for_selection(&self) -> Option<String> {
        let snap = self.snapshot()?;
        let key = self.selected_key()?;
        match self.kind {
            Kind::Pods => Some(key),
            Kind::Workloads => snap.pods.iter().find(|p| p.name.starts_with(&format!("{key}-"))).map(|p| p.name.clone()),
            _ => None,
        }
    }

    pub fn open_logs(&mut self) {
        match self.pod_for_selection() {
            Some(p) => {
                let container = String::new();
                self.feed.follow(Some((p.clone(), container)));
                self.detail.clear();
                self.detail.push("reading the log…", DIM);
                self.drawer = Drawer::Logs(p);
            }
            None => self.status = Some("Select a pod (or a workload with pods) to read its log.".into()),
        }
    }

    pub fn open_detail(&mut self) {
        match self.pod_for_selection() {
            Some(p) => {
                self.feed.follow(None);
                self.feed.request_detail(&p);
                self.detail.clear();
                self.detail.push("loading…", DIM);
                self.detail_fresh = true;
                self.drawer = Drawer::Detail(p);
            }
            None => self.status = Some("Details are shown for pods and workloads.".into()),
        }
    }

    pub fn close_drawer(&mut self) -> bool {
        let was = self.drawer != Drawer::None;
        self.drawer = Drawer::None;
        self.feed.follow(None);
        was
    }

    pub fn has_drawer(&self) -> bool {
        self.drawer != Drawer::None
    }

    pub fn has_modal(&self) -> bool {
        self.picker.is_some() || self.cluster_picker.is_some() || self.teardown_picker.is_some()
    }

    /// "Open in browser" for the selected Service: its first address that works from outside (`reachable at:`).
    fn open_in_browser(&mut self) {
        if self.kind != Kind::Services {
            self.status = Some("Select a Service (Services tab) to open it in the Browser tab.".into());
            return;
        }
        let (Some(snap), Some(key)) = (self.snapshot(), self.selected_key()) else { return };
        let apps = iyakki_agent::idebrowser::reachable_apps(&k8s::reachable_lines(&snap));
        match apps.into_iter().find(|(name, _)| *name == key) {
            Some((_, url)) => {
                self.status = Some(format!("Opening {url} in the Browser tab…"));
                self.wants.push(Want::OpenUrl(url));
            }
            None => self.status = Some(format!("{key} has no address outside the cluster (make it a NodePort or LoadBalancer Service).")),
        }
    }

    pub fn take_wants(&mut self) -> Vec<Want> {
        std::mem::take(&mut self.wants)
    }

    /// Ctrl+A over the flow text or the details drawer.
    fn select_all_key(&mut self, ev: &Event) -> bool {
        if !matches!(ev, Event::Key(Key::Ctrl('a'))) {
            return false;
        }
        if self.flow.is_some() && self.flow_pane.has_text() {
            self.flow_pane.select_all();
            return true;
        }
        if self.has_drawer() && self.detail.pane.has_text() {
            self.detail.pane.select_all();
            return true;
        }
        false
    }

    fn active_pane(&self) -> Option<&textsel::Pane> {
        if self.flow.is_some() {
            Some(&self.flow_pane)
        } else if self.has_drawer() {
            Some(&self.detail.pane)
        } else {
            None
        }
    }

    /// Copy, Select all and Copy all for the right-click menu when the pointer is over the flow text or a details drawer.
    pub fn context_items(&self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        let Some(p) = self.active_pane() else { return Vec::new() };
        let mut v = p.context_items(x, y);
        if !v.is_empty() {
            v.push(crate::ctxmenu::Item::new("k8s.copyall", "Copy all", "").enabled(p.has_text()));
        }
        v
    }

    /// A context menu choice made on [`K8sView::context_items`]; true when it was one of ours.
    pub fn context_pick(&mut self, id: &str) -> bool {
        let flow = self.flow.is_some();
        let pane = if flow { &mut self.flow_pane } else { &mut self.detail.pane };
        match id {
            "copy" => {
                if let Some(t) = pane.selected_text() {
                    crate::ui::clipboard::copy(&t);
                }
                true
            }
            "selall" => {
                pane.select_all();
                true
            }
            "k8s.copyall" => {
                if let Some(t) = pane.all_text() {
                    crate::ui::clipboard::copy(&t);
                }
                true
            }
            _ => false,
        }
    }

    fn close_flow(&mut self) {
        self.flow = None;
        // a setup flow changes the saved registry: read it again
        self.wants.push(Want::RegistryProbe);
    }

    pub fn start_flow(&mut self, f: Flow) {
        self.flow = Some(f);
        self.drawer = Drawer::None;
    }

    pub fn open_cluster_picker(&mut self) {
        let items: Vec<String> = self
            .slot
            .as_ref()
            .and_then(|s| {
                s.lock().ok().and_then(|g| {
                    g.resolution.as_ref().map(|r| {
                        let mut v: Vec<String> = r
                            .profiles
                            .iter()
                            .enumerate()
                            .map(|(i, p)| {
                                let tag = if i == 0 && p.k3s && p.local { " (recommended: this machine's k3s)" } else { "" };
                                format!("{}. {}{} — {} — {}{}{}", i + 1, p.chip(), if p.local { "" } else { " [remote]" }, if p.server.is_empty() { "server unknown" } else { &p.server }, p.kubeconfig, tag, if p.fix.is_empty() { String::new() } else { " — needs a fix (see below after choosing)".to_string() })
                            })
                            .collect();
                        for (i, p) in r.profiles.iter().enumerate() {
                            if r.chosen != Some(i) {
                                v.push(format!("Hide {}. {} — {} (only in IYAKKI; the file is not touched)", i + 1, p.chip(), p.server));
                            }
                        }
                        if r.chosen().is_some_and(|p| matches!(p.source, iyakki_agent::devcluster::Source::Project | iyakki_agent::devcluster::Source::Remembered)) {
                            v.push("Disconnect this workspace from its chosen cluster (back to this machine's k3s)".to_string());
                        }
                        if !r.hidden.is_empty() {
                            v.push(format!("Show hidden clusters ({})", r.hidden.len()));
                        }
                        v
                    })
                })
            })
            .unwrap_or_default();
        if items.is_empty() {
            self.status = Some("No kubeconfig was found. Point the workspace at one in the project settings (⚙), or install a cluster yourself.".into());
            self.wants.push(Want::Reresolve);
            return;
        }
        self.cluster_picker = Some(Picker::new("Change cluster", items));
    }

    /// The text under the header about the chosen cluster when it cannot be used.
    pub fn cluster_problem(&self) -> Option<String> {
        let g = self.slot.as_ref()?.lock().ok()?;
        let r = g.resolution.as_ref()?;
        if r.reachable().is_some() {
            return None;
        }
        Some(r.explain())
    }

    fn profile_chip(&self) -> Option<(String, bool)> {
        let g = self.slot.as_ref()?.lock().ok()?;
        if g.resolving && g.resolution.is_none() {
            return Some(("looking for a cluster…".into(), false));
        }
        let r = g.resolution.as_ref()?;
        Some(match r.chosen() {
            Some(p) => (p.chip(), p.reachable),
            None => ("no cluster found".into(), false),
        })
    }

    /// (API server, is it on this machine) of the selected cluster.
    pub fn profile_server(&self) -> Option<(String, bool)> {
        let g = self.slot.as_ref()?.lock().ok()?;
        let p = g.resolution.as_ref()?.chosen()?;
        Some((p.server.clone(), p.local))
    }

    fn open_namespaces(&mut self) {
        let Some(s) = self.snapshot() else { return };
        let cur = self.feed.target().namespace;
        let items: Vec<String> = s.namespaces.iter().map(|n| if *n == cur { format!("{n}  (current)") } else { n.clone() }).collect();
        if items.is_empty() {
            self.status = Some("The cluster did not list its namespaces.".into());
            return;
        }
        self.picker = Some(Picker::new("Namespace", items));
    }

    pub fn choose_namespace(&mut self, ns: &str) {
        let mut t = self.feed.target();
        t.namespace = ns.to_string();
        self.set_target(t);
    }

    /// Keys for the view. Returns true when the key was used.
    pub fn handle(&mut self, ev: &Event) -> bool {
        if let Some(p) = self.teardown_picker.as_mut() {
            match p.handle_event(ev) {
                PickEvent::Cancel => self.teardown_picker = None,
                PickEvent::Chosen(label) => {
                    self.teardown_picker = None;
                    self.wants.push(if label.starts_with("Stop") { Want::Stop } else if label.starts_with("Start") { Want::Start } else if label.starts_with("Remove all") { Want::RemoveAll } else { Want::RegistryImages });
                }
                PickEvent::None => {}
            }
            return true;
        }
        if let Some(p) = self.cluster_picker.as_mut() {
            match p.handle_event(ev) {
                PickEvent::Cancel => self.cluster_picker = None,
                PickEvent::Chosen(label) => {
                    self.cluster_picker = None;
                    if label.starts_with("Disconnect") {
                        self.wants.push(Want::Disconnect);
                        return true;
                    }
                    if label.starts_with("Show hidden") {
                        self.wants.push(Want::ShowHidden);
                        return true;
                    }
                    if let Some(rest) = label.strip_prefix("Hide ") {
                        let n = rest.split('.').next().and_then(|n| n.trim().parse::<usize>().ok()).unwrap_or(0);
                        let path = self.slot.as_ref().and_then(|s| s.lock().ok().and_then(|g| g.resolution.as_ref().and_then(|r| r.profiles.get(n.saturating_sub(1)).map(|p| p.kubeconfig.clone()))));
                        if let Some(p) = path {
                            self.wants.push(Want::Hide(p));
                        }
                        return true;
                    }
                    if let Some(n) = label.split('.').next().and_then(|n| n.trim().parse::<usize>().ok()) {
                        self.wants.push(Want::Choose(n.saturating_sub(1)));
                    }
                }
                PickEvent::None => {}
            }
            return true;
        }
        if self.flow.is_some() && (self.flow_pane.handle(ev) || self.select_all_key(ev)) {
            return true;
        }
        if let Some(f) = &self.flow {
            let key = match ev {
                Event::Key(k) | Event::Sub(SUB, k) => Some(k.clone()),
                _ => None,
            };
            match key {
                Some(Key::Char('y')) | Some(Key::Enter) if f.asking().is_some() => f.answer(Answer::Approve),
                Some(Key::Char('a')) if f.asking().is_some() => f.answer(Answer::ApproveAll),
                Some(Key::Esc) => {
                    if f.finished() {
                        self.close_flow();
                    } else {
                        f.answer(Answer::Cancel);
                    }
                }
                Some(Key::Enter) | Some(Key::Char('q')) if f.finished() => self.close_flow(),
                Some(_) | None => return matches!(ev, Event::Key(_) | Event::Sub(SUB, _)),
            }
            return true;
        }
        if let Some(p) = self.picker.as_mut() {
            match p.handle_event(ev) {
                PickEvent::Cancel => self.picker = None,
                PickEvent::Chosen(label) => {
                    self.picker = None;
                    let ns = label.split_whitespace().next().unwrap_or("").to_string();
                    if !ns.is_empty() {
                        self.choose_namespace(&ns);
                    }
                }
                PickEvent::None => {}
            }
            return true;
        }
        if self.detail.handle(ev) {
            return true;
        }
        if self.select_all_key(ev) {
            return true;
        }
        let n = self.table().map_or(0, |t| t.rows.len());
        let key = match ev {
            Event::Key(k) | Event::Sub(SUB, k) => k,
            Event::Tab(t) if (TAB_BASE..TAB_BASE + 20).contains(t) => {
                self.kind = Kind::ALL[(t - TAB_BASE).min(Kind::ALL.len() - 1)];
                self.sel = 0;
                self.scroll = 0;
                self.close_drawer();
                return true;
            }
            Event::Tab(t) if *t >= NS_BASE && *t < NS_BASE + 500 => {
                let i = t - NS_BASE;
                if let Some(ns) = self.snapshot().and_then(|s| s.namespaces.get(i).cloned()) {
                    self.choose_namespace(&ns);
                }
                return true;
            }
            Event::Row(r) if *r >= ROW_BASE && *r < ROW_BASE + 100_000 => {
                self.sel = (r - ROW_BASE).min(n.saturating_sub(1));
                return true;
            }
            Event::Scroll(d) => {
                if self.has_drawer() {
                    self.detail.scroll_by(*d * 3);
                } else {
                    self.scroll = (self.scroll as i32 - d * 2).clamp(0, n.saturating_sub(self.visible_rows) as i32) as usize;
                }
                return true;
            }
            _ => return false,
        };
        match key {
            Key::Tab => {
                self.kind = self.kind.next(1);
                self.sel = 0;
                self.scroll = 0;
                self.close_drawer();
            }
            Key::BackTab => {
                self.kind = self.kind.next(-1);
                self.sel = 0;
                self.scroll = 0;
                self.close_drawer();
            }
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::PageUp => self.sel = self.sel.saturating_sub(self.visible_rows),
            Key::PageDown => self.sel = (self.sel + self.visible_rows).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::Enter => self.open_detail(),
            Key::Char('l') => self.open_logs(),
            Key::Char('w') if matches!(self.drawer, Drawer::Logs(_)) => self.detail.nowrap = !self.detail.nowrap,
            Key::Char('r') => {
                self.feed.refresh();
                self.status = Some("Refreshing…".into());
                self.wants.push(Want::RegistryProbe);
                if self.cluster_problem().is_some() {
                    self.wants.push(Want::Reresolve);
                }
            }
            Key::Char('c') => self.open_cluster_picker(),
            Key::Char('d') => match &self.deploy_block {
                None => self.wants.push(Want::Deploy),
                Some(why) => self.status = Some(format!("Deploy is off: {why}.")),
            },
            Key::Char('T') => match &self.deploy_block {
                None => {
                    let ns = self.feed.target().namespace;
                    self.teardown_picker = Some(Picker::new("Tear down the dev environment", vec![format!("Stop everything in {ns} (scale to 0; keeps data, can start again)"), "Start it again (restore the replica counts)".to_string(), format!("Remove all: delete namespace {ns} and everything in it (asks you to type the name)"), format!("Remove the images pushed for {ns} from the registry (optional)")]));
                }
                Some(why) => self.status = Some(format!("Tear down is off: {why}.")),
            },
            Key::Char('S') => self.wants.push(Want::Services { connect: false }),
            Key::Char('G') => self.wants.push(Want::Services { connect: true }),
            Key::Char('g') => match &self.deploy_block {
                None => self.wants.push(Want::Registry),
                Some(why) => self.status = Some(format!("Registry setup is off: {why}.")),
            },
            Key::Char('R') => match (&self.deploy_block, self.snapshot().zip(self.selected_key())) {
                (Some(why), _) => self.status = Some(format!("Restart is off: {why}.")),
                (None, Some((snap, key))) if self.kind == Kind::Workloads => match snap.workloads.iter().find(|w| w.name == key) {
                    Some(w) => self.wants.push(Want::Restart(w.kind.to_lowercase(), w.name.clone())),
                    None => {}
                },
                _ => self.status = Some("Select a workload (Workloads tab) to restart it.".into()),
            },
            Key::Char('n') => self.open_namespaces(),
            Key::Char('o') => self.open_in_browser(),
            Key::Esc => {
                if !self.close_drawer() {
                    return false;
                }
            }
            _ => return false,
        }
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible_rows {
            self.scroll = self.sel + 1 - self.visible_rows;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], tick: u64, ws_name: &str, extra_warning: Option<&str>) {
        self.draw_body(g, r, s, focused, accent, tick, ws_name, extra_warning);
        // the choosers are drawn last, over whatever the body showed
        if let Some(p) = &self.picker {
            p.render(g, r, s, accent);
        }
        if let Some(p) = &self.cluster_picker {
            p.render(g, r, s, accent);
        }
        if let Some(p) = &self.teardown_picker {
            p.render(g, r, s, accent);
        }
    }

    /// Pods that are running and ready, and all pods.
    pub fn pods_ready(snap: &Snapshot) -> (usize, usize) {
        let ready = snap
            .pods
            .iter()
            .filter(|p| {
                let ok = p.ready.split_once('/').is_some_and(|(a, b)| a == b && a != "0");
                ok && (p.phase == "Running" || p.phase == "Succeeded")
            })
            .count();
        (ready, snap.pods.len())
    }

    /// The one-line cluster status the bottom panel shows in place of a second copy of the cluster view: the cluster, the
    /// namespace, how many pods are ready and the latest note, with a button that opens the cluster view. Returns the room
    /// it used. The view itself is drawn only once, as the K8s tab beside the editor and the board.
    pub fn strip(&mut self, g: &mut Gfx, r: R, s: f32, center_shows_k8s: bool) -> f32 {
        // the strip counts as "on screen": the cluster keeps being polled while it is shown
        self.last_draw = Some(Instant::now());
        let m = super::ide_chrome::cur();
        let pad = 10.0 * s;
        let rowh = super::ide_chrome::tool(s);
        let y = r.y + 4.0 * s;
        let cs = s * 0.85;
        let chip_h = fx::size::CAPTION * cs * 1.9;
        let cy = y + (rowh - chip_h) / 2.0;
        let snap = self.snapshot();
        let target = self.feed.target();
        let bounds = R::new(r.x, r.y, r.w, r.h + 60.0 * s);
        // the button on the right: it goes to the K8s tab
        let label = if center_shows_k8s { "Show K8s view" } else { "Open K8s view" };
        let show_text = r.w > 520.0 * s;
        let bw = super::ide_chrome::icon_btn_width(g, s, label, show_text);
        let bx = r.x + r.w - pad - bw;
        super::ide_chrome::icon_btn_glyph(g, bx, y, s, Glyph::K8s, label, show_text, CYAN, false, true, Event::Sub(super::ide_bottom::SUB, Key::Char('K')), "Open K8s view", Some("Show the cluster view (Ctrl+Shift+K)"), bounds);
        let limit = bx - 8.0 * s;
        let mut x = r.x + pad;
        let put = |g: &mut Gfx, text: &str, color: [u8; 4], filled: bool, x: &mut f32| {
            let w = g.text_width(fx::size::CAPTION * cs, text, Face::Bold) + fx::size::CAPTION * cs * 1.4;
            if *x + w <= limit {
                fx::chip(g, *x, cy, cs, text, color, filled);
                *x += w + 5.0 * s;
            }
        };
        let (cluster, ok) = self.profile_chip().unwrap_or_else(|| {
            let ctx = snap.as_ref().map(|s| s.context.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| if target.context.is_empty() { "default context".into() } else { target.context.clone() });
            (format!("ctx {ctx}"), true)
        });
        let shown = g.fit(fx::size::CAPTION * cs, &cluster, Face::Bold, (r.w * 0.34).max(90.0 * s));
        put(g, &shown, if ok { OK } else { ERR }, ok, &mut x);
        put(g, &format!("ns {}", target.namespace), MAGENTA, true, &mut x);
        match &snap {
            Some(sn) => {
                let (up, all) = Self::pods_ready(sn);
                put(g, &format!("pods {up}/{all} ready"), if all == 0 || up == all { OK } else { AMBER }, up != all, &mut x);
            }
            None => put(g, "pods …", MUTED, false, &mut x),
        }
        // the latest note: why something is failing, a status of the view, or that nothing is
        let note = self.status.clone().or_else(|| snap.as_ref().and_then(|sn| sn.notes.first().cloned())).unwrap_or_else(|| "no problems reported".to_string());
        let nsz = m.font_small * s;
        if limit - x > 60.0 * s {
            let t = g.fit(nsz, &note, Face::Sans, limit - x);
            g.text(x, y + (rowh - g.line_height(nsz, Face::Sans)) / 2.0, nsz, super::ide_chrome::dim(), &t, Face::Sans);
        }
        let hint = if center_shows_k8s { "The cluster view is open in the K8s tab above." } else { "The tables, logs and deploy buttons are in the K8s tab beside Editor and Board." };
        let hy = y + rowh + 8.0 * s;
        let t = g.fit(nsz, hint, Face::Sans, r.w - 2.0 * pad);
        g.text(r.x + pad, hy, nsz, super::ide_chrome::dim(), &t, Face::Sans);
        rowh + 8.0 * s
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_body(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], tick: u64, ws_name: &str, extra_warning: Option<&str>) {
        self.last_draw = Some(Instant::now());
        let m = super::ide_chrome::cur();
        let pad = 8.0 * s;
        let snap = self.snapshot();
        let target = self.feed.target();
        // row 1: context, namespace and selector chips on the left; plain text buttons on the right (or on a second row when
        // the pane is narrow). The keys are in the hover hint, not in the labels.
        let narrow = r.w < 760.0 * s;
        let mut x = r.x + pad;
        let rowh = super::ide_chrome::tool(s);
        let y = r.y + 3.0 * s;
        let cs = s * 0.85;
        if let Some((chip, ok)) = self.profile_chip() {
            let shown = g.fit(11.0 * cs, &chip, Face::Bold, (r.w * 0.4).max(100.0 * s));
            x += fx::chip(g, x, y + 1.0 * s, cs, &shown, if ok { OK } else { ERR }, ok) + 5.0 * s;
        } else {
            let ctx = snap.as_ref().map(|s| s.context.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| if target.context.is_empty() { "default context".into() } else { target.context.clone() });
            x += fx::chip(g, x, y + 1.0 * s, cs, &format!("ctx {ctx}"), CYAN, false) + 5.0 * s;
        }
        x += fx::chip(g, x, y + 1.0 * s, cs, &format!("ns {}", target.namespace), MAGENTA, true) + 5.0 * s;
        // the server of the SELECTED cluster, and an amber badge when it is not on this machine
        if let Some((server, local)) = self.profile_server() {
            if !server.is_empty() {
                let shown = g.fit(11.0 * cs, &server.replace("https://", ""), Face::Bold, (r.w * 0.2).max(90.0 * s));
                x += fx::chip(g, x, y + 1.0 * s, cs, &shown, CYAN, false) + 5.0 * s;
            }
            if !local {
                x += fx::chip(g, x, y + 1.0 * s, cs, "remote", AMBER, true) + 5.0 * s;
            }
        }
        // the Registry strip: what images are pushed to / pulled from, and whether it answers
        {
            let st = self.reg.lock().ok().and_then(|g| g.clone());
            let (text, ok) = st.map_or(("registry: …".to_string(), true), |r| (r.chip(), r.ok()));
            let shown = g.fit(11.0 * cs, &text, Face::Bold, (r.w * 0.34).max(110.0 * s));
            x += fx::chip(g, x, y + 1.0 * s, cs, &shown, if ok { CYAN } else { ERR }, false) + 5.0 * s;
        }
        if !target.selector.is_empty() {
            x += fx::chip(g, x, y + 1.0 * s, cs, &format!("-l {}", target.selector), AMBER, false) + 5.0 * s;
        }
        let by = if narrow { y + rowh + 1.0 * s } else { y };
        let right = r.x + r.w - pad;
        let left_edge = if narrow { r.x + pad } else { x + 40.0 * s };
        let off = self.deploy_block.clone().map(|w| format!("off: {w}"));
        let mut rx = right;
        // (picture, shown label, hit label, key, tip, needs the write permission)
        let buttons: [(Glyph, &str, &str, Key, &str, bool); 10] = [
            (Glyph::Open, "Git", "Connect workspace to Gitea  G", Key::Char('G'), "Create the repository on your Gitea, set origin and push after you approve (G)", false),
            (Glyph::Services, "Services", "Services…  S", Key::Char('S'), "Platform services: deploy Gitea or a private store, connect this workspace to Gitea (S)", false),
            (Glyph::Globe, "Open", "Open in browser  o", Key::Char('o'), "Open the selected Service in the Browser tab (o)", false),
            (Glyph::Restart, "Restart", "Restart…  R", Key::Char('R'), "Restart a deployment (R)", true),
            (Glyph::Deploy, "Deploy", "Deploy…  d", Key::Char('d'), "Build, load and apply (d)", true),
            (Glyph::Storage, "Tear down", "Tear down…  T", Key::Char('T'), "Stop or remove this workspace's dev environment (T)", true),
            (Glyph::Registry, "Registry", "Registry…  g", Key::Char('g'), "Set up the registry images go through (g)", true),
            (Glyph::Cluster, "Cluster", "Cluster…  c", Key::Char('c'), "Choose the cluster (c)", false),
            (Glyph::Namespace, "Namespace", "Namespace…  n", Key::Char('n'), "Choose the namespace (n)", false),
            (Glyph::Refresh, "Refresh", "Refresh  r", Key::Char('r'), "Refresh now (r)", false),
        ];
        let bounds = R::new(r.x, r.y, r.w, r.h + 60.0 * s);
        // the text goes first when the whole row fits with it; otherwise the buttons are pictures with a hover hint
        let with_text: f32 = buttons.iter().map(|b| super::ide_chrome::icon_btn_width(g, s, b.1, true) + 2.0 * s).sum();
        let show_text = with_text <= right - left_edge;
        for (gl, label, hit, key, tip, write) in buttons {
            let on = !write || off.is_none();
            let w = super::ide_chrome::icon_btn_width(g, s, label, show_text);
            // a button that would run into the chips or the edge is left out, never drawn over them
            if rx - w < left_edge {
                continue;
            }
            let t = if on { tip.to_string() } else { off.clone().unwrap_or_default() };
            let color = if write { AMBER } else { TEXT };
            super::ide_chrome::icon_btn_glyph(g, rx - w, by, s, gl, label, show_text, color, false, on, Event::Sub(SUB, key), hit, Some(&t), bounds);
            rx -= w + 2.0 * s;
        }
        let age = snap.as_ref().map(|s| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            format!("updated {} ago", age_text(now.saturating_sub(s.taken_at)))
        });
        if let Some(a) = age {
            let asz = m.font_small * s;
            let aw = g.text_width(asz, &a, Face::Sans);
            let ay = y + (rowh - g.line_height(asz, Face::Sans)) / 2.0;
            let ax = if narrow { r.x + pad } else { x };
            let limit = if narrow { rx } else { rx };
            let ay = if narrow { by + (rowh - g.line_height(asz, Face::Sans)) / 2.0 } else { ay };
            if limit - aw > ax + 8.0 * s {
                g.text(if narrow { ax } else { rx - aw - 4.0 * s }, ay, asz, super::ide_chrome::dim(), &a, Face::Sans);
            }
        }
        // kind tabs: flat text with an underline, with counts, then without, then short names
        let ty = r.y + 3.0 * s + rowh + if narrow { rowh + 1.0 * s } else { 0.0 } + 2.0 * s;
        let sel_i = Kind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
        let avail = r.w - 2.0 * pad;
        let counted: Vec<String> = Kind::ALL.iter().map(|k| format!("{}{}", k.label(), snap.as_ref().map_or(String::new(), |sn| format!(" {}", table(sn, *k).rows.len())))).collect();
        let plain: Vec<String> = Kind::ALL.iter().map(|k| k.label().to_string()).collect();
        let short: Vec<String> = ["Work", "Svc", "Ing", "Pods", "Cfg", "PVC", "Nodes"].iter().map(|x| x.to_string()).collect();
        let width = |g: &mut Gfx, l: &[String], text: bool| l.iter().enumerate().map(|(i, t)| super::ide_chrome::icon_tab_width(g, s, t, text, i == sel_i, None)).sum::<f32>();
        // the whole names with counts, then without, then short names, then only the pictures (the names show on hover)
        let (labels, show_text) = if width(g, &counted, true) <= avail {
            (counted, true)
        } else if width(g, &plain, true) <= avail {
            (plain, true)
        } else if width(g, &short, true) <= avail {
            (short, true)
        } else {
            (counted, false)
        };
        let mut tx = r.x + pad - 4.0 * s;
        for (i, l) in labels.iter().enumerate() {
            tx += super::ide_chrome::icon_tab(g, tx, ty, s, Kind::ALL[i].glyph(), l, show_text, None, i == sel_i, accent, Event::Tab(TAB_BASE + i), Kind::ALL[i].label(), bounds);
        }
        super::ide_chrome::strip_line(g, r.x + pad, ty + m.tab_h * s, r.w - 2.0 * pad);
        let tab_h = m.tab_h * s + 3.0 * s;
        let footer = 20.0 * s;
        let body = R::new(r.x + pad, ty + tab_h, r.w - 2.0 * pad, (r.y + r.h - footer - ty - tab_h).max(0.0));
        if self.flow.is_some() {
            self.draw_flow(g, body, s, accent);
            return;
        }
        let Some(snap) = snap else {
            let msg = if self.feed.fetches() == 0 { "Asking the cluster…" } else { "No answer yet" };
            let t = format!("{msg} (kubectl, read-only)");
            g.text(body.x + 8.0 * s, body.y + 10.0 * s, 14.0 * s, MUTED, &t, if tick % 60 < 40 { Face::Sans } else { Face::Bold });
            return;
        };
        let problem = self.cluster_problem();
        if let Some((title, text)) = problem.map(|t| ("No cluster is connected".to_string(), t)).filter(|_| !matches!(snap.cluster, Cluster::Ok)).or_else(|| banner(&snap)) {
            let w = body.w.min(720.0 * s);
            let lines = g.wrap(13.0 * s, &text, Face::Sans, w - 40.0 * s);
            let h = (64.0 + lines.len() as f32 * 19.0) * s;
            let br = R::new(body.x, body.y + 6.0 * s, w, h);
            g.rrect(br.x, br.y, br.w, br.h, 12.0 * s, rgb(34, 18, 28));
            g.rrect_outline(br.x, br.y, br.w, br.h, 12.0 * s, 1.3 * s, with_alpha(ERR, 200));
            g.circle(br.x + 22.0 * s, br.y + 24.0 * s, 6.0 * s, ERR);
            g.text(br.x + 38.0 * s, br.y + 14.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
            for (i, l) in lines.iter().enumerate() {
                g.text(br.x + 20.0 * s, br.y + 44.0 * s + i as f32 * 19.0 * s, 13.0 * s, MUTED, l, Face::Sans);
            }
            return;
        }
        if !snap.namespace_found {
            let note = snap.notes.first().cloned().unwrap_or_default();
            let lines = g.wrap(14.0 * s, &note, Face::Sans, body.w.min(700.0 * s) - 30.0 * s);
            for (i, l) in lines.iter().enumerate() {
                g.text(body.x + 12.0 * s, body.y + 10.0 * s + i as f32 * 20.0 * s, 14.0 * s, AMBER, l, Face::Sans);
            }
            self.draw_namespaces(g, R::new(body.x, body.y + 20.0 * s + lines.len() as f32 * 20.0 * s, body.w, 60.0 * s), s, &snap, &target.namespace, accent);
            return;
        }
        // namespaces strip, table (left) and drawer (right)
        let strip_h = super::ide_chrome::tool(s) + 1.0 * s;
        self.draw_namespaces(g, R::new(body.x, body.y, body.w, strip_h), s, &snap, &target.namespace, accent);
        let area = R::new(body.x, body.y + strip_h + 4.0 * s, body.w, (body.h - strip_h - 4.0 * s).max(0.0));
        let stacked = self.drawer != Drawer::None && area.w <= 760.0 * s;
        if stacked {
            // a narrow pane shows the details (or the log) instead of the table; Esc brings the table back
            self.draw_drawer(g, area, s, accent);
        } else {
            let (table_r, drawer_r) = if self.drawer != Drawer::None {
                let dw = (area.w * 0.46).max(300.0 * s);
                (R::new(area.x, area.y, area.w - dw - 8.0 * s, area.h), Some(R::new(area.x + area.w - dw, area.y, dw, area.h)))
            } else {
                (area, None)
            };
            // the notes (why pods are failing) are shown under the table when there is room
            let t = table(&snap, self.kind);
            let notes_h = if drawer_r.is_none() && !snap.notes.is_empty() && self.kind != Kind::Config { (snap.notes.len().min(2) as f32 * 20.0 + 14.0) * s } else { 0.0 };
            let table_r = R::new(table_r.x, table_r.y, table_r.w, (table_r.h - notes_h).max(40.0 * s));
            self.draw_table(g, table_r, s, &t, focused, accent);
            if notes_h > 0.0 && table_r.h + notes_h <= area.h + 1.0 {
                let ny = table_r.y + table_r.h + 4.0 * s;
                for (i, n) in snap.notes.iter().take(2).enumerate() {
                    g.circle(table_r.x + 8.0 * s, ny + 9.0 * s + i as f32 * 20.0 * s, 3.5 * s, AMBER);
                    let line = g.fit(12.0 * s, n, Face::Sans, table_r.w - 26.0 * s);
                    g.text(table_r.x + 18.0 * s, ny + 2.0 * s + i as f32 * 20.0 * s, 12.0 * s, rgb(230, 200, 150), &line, Face::Sans);
                }
            }
            if let Some(d) = drawer_r {
                self.draw_drawer(g, d, s, accent);
            }
        }
        if let Some(w) = extra_warning {
            let line = g.fit(12.0 * s, w, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, r.y + r.h - 19.0 * s, 12.0 * s, AMBER, &line, Face::Sans);
        } else if let Some(m) = &self.status {
            let line = g.fit(12.0 * s, m, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, r.y + r.h - 19.0 * s, 12.0 * s, MUTED, &line, Face::Sans);
        } else {
            let line = format!("{ws_name} · Tab kind · ↑↓ select · Enter details · l logs · r refresh · n namespace · read-only");
            let line = g.fit(11.0 * s, &line, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, r.y + r.h - 19.0 * s, 11.0 * s, DIM, &line, Face::Sans);
        }
    }

    fn draw_namespaces(&self, g: &mut Gfx, r: R, s: f32, snap: &Snapshot, current: &str, accent: [u8; 4]) {
        let m = super::ide_chrome::cur();
        let sz = m.font_small * s;
        let ly = r.y + (r.h - g.line_height(10.5 * s, Face::Bold)) / 2.0;
        g.text(r.x + 2.0 * s, ly, 10.5 * s, super::ide_chrome::dim(), "NAMESPACES", Face::Bold);
        let mut x = r.x + g.text_width(10.5 * s, "NAMESPACES", Face::Bold) + 12.0 * s;
        for (i, n) in snap.namespaces.iter().enumerate() {
            let on = n == current;
            let w = g.text_width(sz, n, Face::Bold) + 14.0 * s;
            if x + w > r.x + r.w {
                g.text(x, ly, sz, super::ide_chrome::dim(), "…", Face::Bold);
                break;
            }
            let b = R::new(x, r.y + 1.0 * s, w, (r.h - 2.0 * s).max(20.0));
            let hot = g.hovered(b);
            if on {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, fx::over(fx::WELL, with_alpha(accent, 60)));
            } else if hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, rgb(34, 44, 78));
            }
            g.text(b.x + 7.0 * s, b.y + (b.h - g.line_height(sz, Face::Bold)) / 2.0, sz, if on { TEXT } else { super::ide_chrome::muted() }, n, Face::Bold);
            g.hit(b, Event::Tab(NS_BASE + i), n);
            x += w + 2.0 * s;
        }
    }

    fn draw_table(&mut self, g: &mut Gfx, r: R, s: f32, t: &Table, focused: bool, accent: [u8; 4]) {
        let total_w: f32 = t.cols.iter().map(|c| c.1).sum();
        let m = super::ide_chrome::cur();
        let head_h = 20.0 * s;
        let mut cx = r.x;
        let col_x: Vec<f32> = t.cols.iter().map(|c| {
            let x = cx;
            cx += r.w * c.1 / total_w;
            x
        }).collect();
        let cols: Vec<(&str, f32)> = t.cols.iter().zip(&col_x).enumerate().map(|(i, ((n, _), x))| (*n, (*x - r.x) / s + if i == 0 { 22.0 } else { 4.0 })).collect();
        fx::table_header(g, r.x, r.y + 2.0 * s, r.w, s, &cols, accent);
        let list = R::new(r.x, r.y + head_h + 4.0 * s, r.w, (r.h - head_h - 4.0 * s).max(0.0));
        let rh = m.row_h * s;
        self.visible_rows = ((list.h / rh) as usize).max(1);
        if t.rows.is_empty() {
            g.text(list.x + 10.0 * s, list.y + 10.0 * s, 13.0 * s, DIM, &format!("No {} in this namespace{}.", self.kind.label().to_lowercase(), if self.feed.target().selector.is_empty() { "" } else { " for this selector" }), Face::Sans);
            return;
        }
        self.sel = self.sel.min(t.rows.len() - 1);
        self.scroll = self.scroll.min(t.rows.len().saturating_sub(self.visible_rows));
        for (i, row) in t.rows.iter().enumerate().skip(self.scroll).take(self.visible_rows) {
            let y = list.y + (i - self.scroll) as f32 * rh;
            let rect = R::new(list.x, y, list.w - 8.0 * s, rh);
            let selected = i == self.sel;
            let hot = g.hovered(rect);
            if selected {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 3.0 * s, fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })));
                g.rect(rect.x, rect.y + 2.0 * s, 2.0 * s.max(1.0), rect.h - 4.0 * s, with_alpha(accent, if focused { 230 } else { 120 }));
            } else if hot {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 3.0 * s, rgb(20, 27, 50));
            }
            if let Some(h) = row.health {
                let c = health_color(h);
                g.circle(rect.x + 10.0 * s, rect.y + rect.h / 2.0, 3.0 * s, c);
                g.circle(rect.x + 10.0 * s, rect.y + rect.h / 2.0, 5.5 * s, with_alpha(c, 36));
            }
            for (ci, cell) in row.cells.iter().enumerate() {
                let x = col_x[ci] + if ci == 0 { 22.0 * s } else { 4.0 * s };
                let w = r.w * t.cols[ci].1 / total_w - if ci == 0 { 26.0 * s } else { 8.0 * s };
                let is_status = t.cols[ci].0 == "Status" && row.health.is_some();
                let color = if is_status { health_color(row.health.unwrap()) } else if ci == 0 { TEXT } else { rgb(190, 198, 222) };
                // numbers (3/3, 9d, 120m) are monospace so the columns line up
                let numeric = ci > 0 && cell.chars().next().is_some_and(|c| c.is_ascii_digit()) && !cell.contains(' ') && cell.len() <= 12;
                let face = if numeric { Face::Mono } else if ci == 0 || is_status { Face::Bold } else { Face::Sans };
                let fsz = m.font_ui * s;
                let txt = g.fit(fsz, cell, face, w.max(10.0));
                g.text(x, rect.y + (rect.h - g.line_height(fsz, face)) / 2.0, fsz, color, &txt, face);
            }
            g.hit(rect, Event::Row(ROW_BASE + i), &format!("k8s:{}", row.key));
        }
        fx::scrollbar(g, R::new(r.x + r.w - 6.0 * s, list.y, 6.0 * s, list.h), s, self.scroll, self.visible_rows, t.rows.len(), accent);
    }

    /// The deploy or restart steps: what each will run, its answer buttons, and the output as it arrives.
    fn draw_flow(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
        let Some(f) = &self.flow else { return };
        fx::panel(g, r, s, accent);
        let steps = f.steps();
        let (finished, asking) = (f.finished(), f.asking());
        let title = g.fit(15.0 * s, &f.title, Face::Bold, r.w - 40.0 * s);
        g.text(r.x + 14.0 * s, r.y + 10.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
        let mut y = r.y + 38.0 * s;
        for (i, st) in steps.iter().enumerate() {
            let c = match st.state {
                St::Done => OK,
                St::Failed => ERR,
                St::Asking => AMBER,
                St::Running => CYAN,
                St::Pending | St::Skipped => rgb(110, 120, 150),
            };
            g.circle(r.x + 24.0 * s, y + 9.0 * s, 5.0 * s, c);
            if matches!(st.state, St::Asking | St::Running) {
                g.ring(r.x + 24.0 * s, y + 9.0 * s, 8.0 * s, 1.2 * s, with_alpha(c, 120));
            }
            let label = format!("{}  {}", i + 1, st.title);
            let line = g.fit(13.0 * s, &label, Face::Bold, r.w - 70.0 * s);
            g.text(r.x + 40.0 * s, y + 1.0 * s, 13.0 * s, if matches!(st.state, St::Pending | St::Skipped) { MUTED } else { TEXT }, &line, Face::Bold);
            let tag = match st.state {
                St::Pending => "waiting",
                St::Asking => "needs your approval",
                St::Running => "running",
                St::Done => "done",
                St::Failed => "failed",
                St::Skipped => "skipped",
            };
            let tw = g.text_width(11.0 * s, tag, Face::Sans);
            g.text(r.x + r.w - 14.0 * s - tw, y + 3.0 * s, 11.0 * s, c, tag, Face::Sans);
            y += 24.0 * s;
        }
        // the step in focus: the one asking, running, or the last that did something
        let focus = asking.or_else(|| steps.iter().position(|x| x.state == St::Running)).or_else(|| steps.iter().rposition(|x| matches!(x.state, St::Done | St::Failed))).unwrap_or(0);
        let btn_h = 32.0 * s;
        let area = R::new(r.x + 14.0 * s, y + 6.0 * s, r.w - 28.0 * s, (r.y + r.h - y - btn_h - 18.0 * s).max(20.0));
        // the selection belongs to the step in view: another step (or state) starts clean
        let key = steps.get(focus).map(|st| (focus, st.state));
        if key != self.flow_key {
            self.flow_pane.clear();
            self.flow_key = key;
        }
        let mut lines: Vec<(String, [u8; 4], Face)> = Vec::new();
        if let Some(st) = steps.get(focus) {
            for l in g.wrap(12.0 * s, &st.summary, Face::Sans, area.w) {
                lines.push((l, TEXT, Face::Sans));
            }
            if !st.detail.is_empty() {
                lines.push((String::new(), TEXT, Face::Mono));
                for raw in st.detail.lines() {
                    let colour = if raw.starts_with('+') { OK } else if raw.starts_with('-') && !raw.starts_with("--") { ERR } else { rgb(190, 198, 222) };
                    for l in g.wrap(11.5 * s, raw, Face::Mono, area.w) {
                        lines.push((l, colour, Face::Mono));
                    }
                }
            }
            let out = if st.state == St::Running { f.live_text() } else { st.output.clone() };
            if !out.trim().is_empty() {
                lines.push((String::new(), TEXT, Face::Mono));
                lines.push(("OUTPUT".into(), DIM, Face::Bold));
                for raw in out.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
                    for l in g.wrap(11.5 * s, raw, Face::Mono, area.w) {
                        lines.push((l, if st.state == St::Failed { ERR } else { rgb(200, 208, 230) }, Face::Mono));
                    }
                }
            }
        }
        let lh = 16.0 * s;
        let fit = ((area.h / lh) as usize).max(1);
        // when it does not all fit, the end (the newest output) is the part shown
        let skip = lines.len().saturating_sub(fit);
        for (i, (l, c, face)) in lines.iter().skip(skip).enumerate() {
            g.text(area.x, area.y + i as f32 * lh, if *face == Face::Mono { 11.5 * s } else { 12.0 * s }, *c, l, *face);
        }
        // the same text as selectable lines (those scrolled out of view too, so Copy all has everything)
        self.flow_pane.begin(area);
        let mut para = 0u32;
        for (i, (l, _, face)) in lines.iter().enumerate() {
            if l.is_empty() {
                para += 1;
            }
            let size = if *face == Face::Mono { 11.5 * s } else { 12.0 * s };
            self.flow_pane.line(g, area.x, area.y + (i as f32 - skip as f32) * lh, lh, size, *face, l, 0, para);
        }
        self.flow_pane.end(g, accent);
        // the buttons
        let by = r.y + r.h - btn_h - 8.0 * s;
        let mut bx = r.x + 14.0 * s;
        let mut put = |g: &mut Gfx, label: &str, colour: [u8; 4], key: Key| {
            let w = g.text_width(12.0 * s, label, Face::Bold) + 22.0 * s;
            let b = R::new(bx, by, w, 26.0 * s);
            let hot = g.hovered(b);
            g.rrect(b.x, b.y, b.w, b.h, 13.0 * s, if hot { rgb(40, 50, 86) } else { fx::WELL });
            g.rrect_outline(b.x, b.y, b.w, b.h, 13.0 * s, 1.2 * s, colour);
            g.text(b.x + 11.0 * s, b.y + 6.0 * s, 12.0 * s, TEXT, label, Face::Bold);
            g.hit(b, Event::Sub(SUB, key), label);
            bx += w + 8.0 * s;
        };
        if asking.is_some() {
            put(g, "Approve  y", OK, Key::Char('y'));
            put(g, "Approve all steps  a", AMBER, Key::Char('a'));
            put(g, "Cancel  Esc", ERR, Key::Esc);
        } else if finished {
            put(g, "Close  Esc", accent, Key::Esc);
            let sm = f.summary();
            let line = g.fit(12.0 * s, &sm, Face::Sans, (r.x + r.w - bx - 14.0 * s).max(40.0));
            g.text(bx + 4.0 * s, by + 6.0 * s, 12.0 * s, if sm.starts_with("Deployed") || sm.starts_with("Restart started") || sm.starts_with("Registry set up") || sm.starts_with("Removed") || sm.starts_with("Stopped the") || sm.starts_with("Started the") { OK } else { AMBER }, &line, Face::Sans);
        } else {
            put(g, "Cancel  Esc", ERR, Key::Esc);
        }
    }

    fn draw_drawer(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
        let m = super::ide_chrome::cur();
        fx::panel(g, r, s, accent);
        let (title, logs) = match &self.drawer {
            Drawer::Detail(p) => (format!("Pod {p}"), false),
            Drawer::Logs(p) => (format!("Log tail · {p}"), true),
            Drawer::None => return,
        };
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 300.0 * s, r.h + 100.0 * s);
        let hh = super::ide_chrome::tool(s);
        // the buttons of the header from the right: close, and for the log wrap on/off
        let mut bx = r.x + r.w - 6.0 * s;
        let close = g.text_width(m.font_small * s + 1.0 * s, "Close", Face::Bold) + 16.0 * s;
        bx -= close;
        super::ide_chrome::text_btn(g, bx, r.y + 4.0 * s, s, "Close", MUTED, true, Event::Sub(SUB, Key::Esc), "close drawer", Some("Close (Esc)"), bounds);
        if logs {
            let label = if self.detail.nowrap { "Wrap: off" } else { "Wrap: on" };
            let w = g.text_width(m.font_small * s + 1.0 * s, label, Face::Bold) + 16.0 * s;
            bx -= w + 2.0 * s;
            super::ide_chrome::text_btn(g, bx, r.y + 4.0 * s, s, label, CYAN, true, Event::Sub(SUB, Key::Char('w')), "Wrap logs", Some("Wrap long log lines (w)"), bounds);
        }
        let tsz = m.font_ui * s;
        let t = g.fit(tsz, &title, Face::Bold, (bx - r.x - 20.0 * s).max(40.0 * s));
        g.text(r.x + 10.0 * s, r.y + 4.0 * s + (hh - g.line_height(tsz, Face::Bold)) / 2.0, tsz, TEXT, &t, Face::Bold);
        let mut top = 8.0 * s + hh;
        if logs {
            let live = if (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()) / 500) % 2 == 0 { "● following every 2 s" } else { "○ following every 2 s" };
            g.text(r.x + 10.0 * s, r.y + top - 2.0 * s, 10.5 * s, fx::readable(OK, fx::PANEL, 4.5), live, Face::Sans);
            top += 14.0 * s;
        }
        let body = R::new(r.x + 10.0 * s, r.y + top, r.w - 20.0 * s, (r.h - top - 8.0 * s).max(0.0));
        // logs and details in small type; logs in mono
        self.detail.render(g, body, if logs { 13.0 * s } else { m.font_small * s + 0.5 * s }, if logs { Face::Mono } else { Face::Sans });
    }
}

/// The details drawer's text for a pod.
pub fn detail_lines(res: &Result<k8s::PodDetail, String>) -> Vec<(String, [u8; 4])> {
    let d = match res {
        Ok(d) => d,
        Err(e) => return vec![(format!("could not read the pod: {e}"), ERR)],
    };
    let mut v: Vec<(String, [u8; 4])> = Vec::new();
    v.push((format!("phase {} · node {} · ip {} · owner {}", d.phase, if d.node.is_empty() { "-" } else { &d.node }, if d.ip.is_empty() { "-" } else { &d.ip }, if d.owner.is_empty() { "-" } else { &d.owner }), MUTED));
    for e in &d.explanation {
        v.push((format!("What this means: {e}"), AMBER));
    }
    v.push((String::new(), [0, 0, 0, 0]));
    v.push(("CONTAINERS".into(), CYAN));
    for c in &d.containers {
        let col = if c.ready { OK } else if c.state == "waiting" || c.state == "terminated" { ERR } else { AMBER };
        v.push((format!("{} · {}{}", c.name, c.state, if c.reason.is_empty() { String::new() } else { format!(" ({})", c.reason) }), col));
        v.push((format!("  image {} · restarts {} · ready {}", c.image, c.restarts, c.ready), MUTED));
        if !c.requests.is_empty() {
            v.push((format!("  requests {}", c.requests), MUTED));
        }
        if !c.limits.is_empty() {
            v.push((format!("  limits   {}", c.limits), MUTED));
        }
        if !c.message.is_empty() {
            v.push((format!("  {}", c.message), rgb(230, 200, 150)));
        }
    }
    v.push((String::new(), [0, 0, 0, 0]));
    v.push(("RECENT EVENTS".into(), CYAN));
    if d.events.is_empty() {
        v.push(("none".into(), DIM));
    }
    for e in &d.events {
        v.push((format!("{} {} ({}×) {} ago: {}", if e.kind == "Warning" { "!" } else { "·" }, e.reason, e.count, age_text(e.age), e.message), if e.kind == "Warning" { AMBER } else { MUTED }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::k8s::{fetch, sample_cluster, FakeKube};

    const NOW: u64 = 1_790_000_000;

    fn snap() -> Snapshot {
        fetch(&sample_cluster(NOW), &Target { namespace: "shop-dev".into(), ..Default::default() }, NOW)
    }

    #[test]
    fn tables_for_every_kind_and_secret_values_never_appear() {
        let s = snap();
        let w = table(&s, Kind::Workloads);
        assert_eq!(w.rows.len(), 6);
        assert_eq!(w.rows[0].cells[2], "3/3");
        assert!(w.rows.iter().any(|r| r.cells[3].contains("web:1.4.2")));
        let p = table(&s, Kind::Pods);
        let bad = p.rows.iter().find(|r| r.cells[2] == "CrashLoopBackOff").unwrap();
        assert_eq!((bad.health, bad.cells[3].as_str()), (Some(Health::Bad), "14"));
        let c = table(&s, Kind::Config);
        assert!(c.rows.iter().any(|r| r.cells[1] == "Secret" && r.cells[2] == "value hidden"));
        assert!(c.rows.iter().all(|r| r.cells.len() == 3));
        for k in Kind::ALL {
            let t = table(&s, k);
            assert!(t.rows.iter().all(|r| r.cells.len() == t.cols.len()), "{k:?}");
        }
        assert_eq!(table(&s, Kind::Nodes).rows.len(), 2);
        assert_eq!(table(&s, Kind::Services).rows[0].cells[4], "192.168.1.50");
        let all = format!("{:?}", Kind::ALL.iter().map(|k| table(&s, *k)).collect::<Vec<_>>());
        assert!(!all.contains("password") && !all.contains("c3VwZXI"));
    }

    #[test]
    fn banners_explain_the_state_and_the_fix() {
        let t = Target { namespace: "x".into(), ..Default::default() };
        let k = FakeKube::new();
        k.missing();
        let (title, text) = banner(&fetch(&k, &t, NOW)).unwrap();
        assert!(title.contains("kubectl was not found") && text.contains("Settings → Dev cluster") && text.contains("docs/K3S-DEV.md"));
        let k = FakeKube::new();
        k.fail("get namespaces", "Unable to connect to the server: dial tcp 127.0.0.1:6443: connect: connection refused");
        let (title, text) = banner(&fetch(&k, &t, NOW)).unwrap();
        assert!(title.contains("not reachable") && text.contains("refused") && text.contains("Settings → Dev cluster"));
        assert!(banner(&snap()).is_none());
    }

    #[test]
    fn keys_switch_kinds_select_open_details_and_logs_and_pick_namespaces() {
        let mut v = K8sView::new(Arc::new(sample_cluster(NOW)), Target { namespace: "shop-dev".into(), ..Default::default() });
        v.tick();
        v.last_draw = Some(Instant::now());
        v.tick();
        let wait = |v: &mut K8sView, f: &dyn Fn(&K8sView) -> bool| {
            for _ in 0..300 {
                v.last_draw = Some(Instant::now());
                v.tick();
                if f(v) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(wait(&mut v, &|v| v.snapshot().is_some()));
        assert!(v.handle(&Event::Key(Key::Tab)) && v.kind == Kind::Services);
        v.handle(&Event::Key(Key::Tab));
        v.handle(&Event::Key(Key::Tab));
        assert_eq!(v.kind, Kind::Pods);
        v.handle(&Event::Key(Key::Down));
        v.handle(&Event::Key(Key::Down));
        v.handle(&Event::Key(Key::Down));
        v.handle(&Event::Key(Key::Down));
        assert_eq!(v.selected_key().as_deref(), Some("api-5b8d44-uvwxy"));
        v.handle(&Event::Key(Key::Enter));
        assert!(wait(&mut v, &|v| v.detail.lines.iter().any(|l| l.0.contains("exits again and again"))));
        assert!(v.detail.lines.iter().any(|l| l.0.contains("requests cpu 100m")));
        v.handle(&Event::Key(Key::Char('l')));
        assert!(wait(&mut v, &|v| v.detail.lines.iter().any(|l| l.0.contains("password authentication failed"))));
        assert!(v.handle(&Event::Key(Key::Esc)) && !v.has_drawer());
        // a workload's log is its first pod's
        v.handle(&Event::Key(Key::BackTab));
        v.handle(&Event::Key(Key::BackTab));
        v.handle(&Event::Key(Key::BackTab));
        assert_eq!(v.kind, Kind::Workloads);
        v.sel = 0;
        assert_eq!(v.pod_for_selection().as_deref(), Some("web-6d9f7c-abcde"));
        // the namespace chooser
        v.handle(&Event::Key(Key::Char('n')));
        assert!(v.has_modal());
        for c in "kube".chars() {
            v.handle(&Event::Key(Key::Char(c)));
        }
        v.handle(&Event::Key(Key::Enter));
        assert!(!v.has_modal());
        assert_eq!(v.feed.target().namespace, "kube-system");
        assert!(!v.handle(&Event::Key(Key::Char('z'))), "unknown keys are left to the host");
    }

    #[test]
    fn renders_every_state_without_panicking_and_shows_the_disabled_buttons() {
        for (w, h) in [(900.0, 520.0), (1280.0, 700.0), (560.0, 420.0)] {
            let mut v = K8sView::new(Arc::new(sample_cluster(NOW)), Target { namespace: "shop-dev".into(), ..Default::default() });
            let mut g = Gfx::new(w as u32, h as u32);
            let mut drew = false;
            for _ in 0..300 {
                g.clear(rgb(8, 11, 26));
                g.hits.clear();
                v.draw(&mut g, R::new(0.0, 0.0, w, h), 1.0, true, CYAN, 1, "Shop", None);
                v.tick();
                if v.snapshot().is_some() {
                    drew = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(drew);
            for k in Kind::ALL {
                v.kind = k;
                g.hits.clear();
                v.draw(&mut g, R::new(0.0, 0.0, w, h), 1.0, true, CYAN, 1, "Shop", Some("another workspace uses the same namespace"));
            }
            assert!(g.hits.iter().any(|h| h.label.starts_with("Deploy…")) && g.hits.iter().any(|h| h.label.starts_with("Restart…")));
            v.kind = Kind::Pods;
            v.sel = 4;
            v.open_detail();
            for _ in 0..100 {
                v.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
            g.hits.clear();
            v.draw(&mut g, R::new(0.0, 0.0, w, h), 1.0, true, CYAN, 1, "Shop", None);
        }
        // kubectl missing
        let k = FakeKube::new();
        k.missing();
        let mut v = K8sView::new(Arc::new(k), Target { namespace: "x".into(), ..Default::default() });
        let mut g = Gfx::new(900, 500);
        for _ in 0..200 {
            g.clear(rgb(8, 11, 26));
            v.draw(&mut g, R::new(0.0, 0.0, 900.0, 500.0), 1.0, true, CYAN, 1, "Shop", None);
            v.tick();
            if v.snapshot().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        g.clear(rgb(8, 11, 26));
        v.draw(&mut g, R::new(0.0, 0.0, 900.0, 500.0), 1.0, true, CYAN, 1, "Shop", None);
        assert!(v.snapshot().is_some_and(|s| s.cluster == Cluster::KubectlMissing));
    }
}
