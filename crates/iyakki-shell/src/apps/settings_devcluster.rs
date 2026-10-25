//! Settings → Dev cluster (docs/K3S-DEV.md, phases 4 and 5).
//!
//! * **Profiles**: every cluster IYAKKI can find (this machine's k3s, the cluster IYAKKI installed, `$KUBECONFIG`, `~/.kube/config`,
//!   this pod's service account, kubeconfigs held in a vault) with server, node name, version, whether it answers and why the assistant
//!   may only read from it. Test connection, make it the default for you or for one workspace, hide/show (IYAKKI-side only, the file is
//!   never touched), disconnect, and the registry choice (A in-cluster, B system, C existing) with the same plan-and-approve logic as the
//!   K8S panel.
//! * **The machine's own cluster**: on bare metal with no cluster, "Set up a local cluster" shows a plan (what is downloaded and checked,
//!   where it goes, what starts, the ports) and only an approval runs it; afterwards Start, Stop, Restart, Logs, Upgrade, Reset and
//!   Uninstall. With a cluster that is not IYAKKI's, or in a desktop window, the page only manages profiles and registries.
//!
//! Administrators change the machine; everyone with the page chooses their own profiles. Every change is audited by name, never by value.

use super::*;
use crate::apps::ScrollLog;
use iyakki_agent::devcluster::{self, Choice, ClusterProfile, Env as ClusterEnv, Resolution, Source};
use iyakki_agent::k3sdev::{self, Backend, Situation, Svc};
use iyakki_agent::kubeprofiles;
use iyakki_agent::regsetup;
use iyakki_agent::workspace::{K8sConfig, ProjectConfig, Registry as WsRegistry};
use iyakki_agent::wsexec::{Runner, System};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const AMBER: [u8; 4] = rgb(255, 184, 77);

/// What a confirmation or the plan screen's Approve does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DcAct {
    Start,
    Stop,
    Restart,
    Disconnect(String),
    RemoveVault(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DcForm {
    Setup,
    Reset,
    Uninstall,
    Vault,
    Registry,
}

/// A plan found by a background job, to show for approval.
pub(super) enum DcOut {
    SetupPlan(Result<k3sdev::Plan, String>),
    UpgradePlan(Result<Option<k3sdev::Plan>, String>),
    RegistryPlan(Result<(regsetup::SetupPlan, String), String>),
}

enum OverlayAct {
    Setup(k3sdev::Plan),
    Upgrade(k3sdev::Plan),
    Registry(Box<regsetup::SetupPlan>, String),
    Run(DcAct),
    Close,
}

struct Overlay {
    title: String,
    log: ScrollLog,
    act: OverlayAct,
    /// Why Approve is not offered (empty: it is).
    blocked: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RowKind {
    Cluster,
    Vault(String),
    Hidden,
}

#[derive(Clone, Debug)]
struct Row {
    kind: RowKind,
    /// The kubeconfig path, or the vault reference.
    id: String,
    context: String,
    source: Source,
    title: String,
    sub: String,
    chips: Vec<(String, [u8; 4])>,
    profile: Option<ClusterProfile>,
}

#[derive(Default)]
pub(super) struct Dc {
    world: Option<(Arc<dyn Runner>, ClusterEnv)>,
    res: Arc<Mutex<Option<Resolution>>>,
    nodes: Arc<Mutex<HashMap<String, String>>>,
    tests: Arc<Mutex<HashMap<String, String>>>,
    scanning: Arc<AtomicBool>,
    /// A look was asked for while another was running: look again when it ends.
    pending: Arc<AtomicBool>,
    scans: Arc<AtomicUsize>,
    asked: bool,
    rows: Vec<Row>,
    rows_scans: usize,
    rows_at: Option<Instant>,
    rows_dirty: bool,
    ws: Vec<(String, PathBuf, ProjectConfig)>,
    ws_idx: usize,
    overlay: Option<Overlay>,
    sit: Option<(Instant, Situation)>,
    reg: Arc<Mutex<Option<(String, regsetup::RegStatus)>>>,
    health_busy: Arc<AtomicBool>,
    /// Platform services (Gitea, the private store): while it is open it owns every key and click of this page.
    pub(super) platform: Option<crate::apps::platform_services::PlatformPanel>,
    /// Settings -> Clusters (the cluster registry): owns every key and click of this page while open.
    pub(super) clusters: Option<crate::apps::clusters_panel::ClustersPanel>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lines_of(text: &str) -> ScrollLog {
    let mut l = ScrollLog { nowrap: false, ..ScrollLog::default() };
    for line in text.lines() {
        let col = if line.starts_with(|c: char| c.is_ascii_uppercase()) && line.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii_uppercase() || c == ' ') && line.len() > 3 { ACCENT } else { [0, 0, 0, 0] };
        l.push(line, col);
    }
    l.scroll = 1_000_000; // clamped on draw: starts at the top
    l
}

impl SettingsApp {
    /// Give the page its own idea of where clusters are (tests); `None` by default in tests means "look nowhere".
    pub fn with_cluster(mut self, run: Arc<dyn Runner>, env: ClusterEnv) -> SettingsApp {
        self.dc.world = Some((run, env));
        self
    }

    fn dc_world(&self) -> Option<(Arc<dyn Runner>, ClusterEnv)> {
        if let Some(w) = &self.dc.world {
            return Some(w.clone());
        }
        if cfg!(test) {
            return None;
        }
        Some((Arc::new(System), ClusterEnv::real()))
    }

    fn dc_ids(cx: &AppCx) -> (PathBuf, String) {
        (cx.services.dir().to_path_buf(), cx.session.user.clone())
    }

    pub(super) fn dc_visible(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings
    }

    pub(super) fn dc_len(&self) -> usize {
        self.dc.rows.len()
    }

    pub(super) fn dc_has_overlay(&self) -> bool {
        self.dc.overlay.is_some()
    }

    /// Open Platform services on this page, optionally straight into the deploy wizard of one service.
    pub fn dc_open_platform(&mut self, cx: &AppCx, service: Option<&str>) {
        let (run, env) = self.dc_world().unwrap_or_else(|| (Arc::new(iyakki_agent::wsexec::FakeRunner::new(&[])), ClusterEnv::default()));
        let pl = Arc::new(crate::apps::platform_services::platform_for(cx, run, env));
        let mut p = crate::apps::platform_services::PlatformPanel::new(pl).with_services(cx.services.clone());
        match service {
            Some("private-store:manage") => p.open_store_manage(),
            Some(id) => p.open_wizard(id),
            None => p.open_list(),
        }
        self.dc.platform = Some(p);
    }

    pub(super) fn dc_platform_open(&self) -> bool {
        self.dc.platform.as_ref().is_some_and(|p| p.is_open()) || self.dc.clusters.as_ref().is_some_and(|p| p.is_open())
    }

    /// Open the Clusters page (the registry of local and remote clusters).
    pub fn dc_open_clusters(&mut self, cx: &AppCx) {
        let (dir, user) = Self::dc_ids(cx);
        let svc = cx.services.clone();
        let (v_svc, v_user) = (svc.clone(), user.clone());
        let panel = crate::apps::clusters_panel::ClustersPanel::new(svc.fleet(), dir, &user, cx.session.role)
            .with_streamer(svc.streamer())
            .with_vault_delete(Arc::new(move |reference: &str| {
                let result = match reference.split_once(':') {
                    Some(("uvault", name)) => v_svc.user_delete(&v_user, name),
                    Some(("vault", name)) => v_svc.system_delete(name),
                    _ => return Ok(()),
                };
                result.map_err(|e| format!("the vault entry was not deleted: {e}"))
            }));
        self.dc.clusters = Some(panel);
    }

    /// Every event while Platform services is open (the panel is the only keyboard owner). Closing drops it, secrets included.
    pub(super) fn dc_platform_event(&mut self, ev: &Event) {
        if let Some(c) = self.dc.clusters.as_mut() {
            if !c.handle(ev) {
                self.dc.clusters = None;
            }
            return;
        }
        let Some(p) = self.dc.platform.as_mut() else { return };
        let open = p.handle(ev);
        let wants = p.take_wants();
        let msg = p.message();
        if !open {
            self.dc.platform = None;
        }
        for w in wants {
            let crate::apps::platform_services::PWant::OpenUrl(u) = w;
            self.ok(format!("{u} is copied: paste it in the BROWSER app."));
        }
        let _ = msg;
    }

    pub(super) fn dc_button_rows(&self) -> usize {
        2
    }

    // ------------------------------------------------------------------ looking

    /// Look through the places clusters can be, in the background (never blocks the screen).
    pub(super) fn dc_scan(&mut self, cx: &AppCx) {
        self.dc.asked = true;
        self.dc.rows_dirty = true;
        let Some((run, env)) = self.dc_world() else {
            *lock(&self.dc.res) = Some(Resolution::default());
            return;
        };
        if self.dc.scanning.swap(true, Ordering::SeqCst) {
            self.dc.pending.store(true, Ordering::SeqCst);
            return;
        }
        let (dir, user) = Self::dc_ids(cx);
        let ns = self.dc_namespace();
        let (res, nodes, scanning, scans) = (self.dc.res.clone(), self.dc.nodes.clone(), self.dc.scanning.clone(), self.dc.scans.clone());
        std::thread::spawn(move || {
            let remembered = Choice::load(&dir, &user);
            let hidden = devcluster::hidden_load(&dir, &user);
            let r = devcluster::resolve_hiding(&run, &env, &K8sConfig::default(), remembered.as_ref(), &ns, true, &hidden);
            let mut n = HashMap::new();
            for p in r.profiles.iter().filter(|p| p.reachable) {
                let kc = devcluster::Kc::new(run.clone(), &p.kubeconfig, &p.context);
                let o = kc.read(&["get", "nodes", "-o", "jsonpath={.items[*].metadata.name}"]);
                if o.ok() && !o.stdout.trim().is_empty() {
                    n.insert(p.kubeconfig.clone(), o.stdout.trim().to_string());
                }
            }
            *lock(&nodes) = n;
            *lock(&res) = Some(r);
            scans.fetch_add(1, Ordering::SeqCst);
            scanning.store(false, Ordering::SeqCst);
        });
    }

    fn dc_namespace(&self) -> String {
        self.dc.ws.get(self.dc.ws_idx).map(|(name, _, cfg)| cfg.namespace(name)).unwrap_or_else(|| "default".into())
    }

    fn dc_situation(&mut self, cx: &AppCx) -> Situation {
        if let Some((at, s)) = &self.dc.sit {
            if at.elapsed() < Duration::from_secs(2) {
                return s.clone();
            }
        }
        let s = cx.services.k3sdev.situation();
        self.dc.sit = Some((Instant::now(), s.clone()));
        s
    }

    /// Keep the rows, the situation and the workspaces up to date; called by the heartbeat and before drawing.
    pub(super) fn dc_prepare(&mut self, cx: &AppCx) {
        // what the cluster supervisor does in the background (restarts, exits) and what this page starts is audited the same way
        let audit = cx.audit.clone();
        cx.services.k3sdev.set_audit(Arc::new(move |u, a, d| audit.record(u, a, d)));
        if !self.dc.scanning.load(Ordering::SeqCst) && self.dc.pending.swap(false, Ordering::SeqCst) {
            self.dc.asked = false;
        }
        if !self.dc.asked {
            self.dc_scan(cx);
        }
        let scans = self.dc.scans.load(Ordering::SeqCst);
        if self.dc.rows_dirty || scans != self.dc.rows_scans || self.dc.rows_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.dc_build_rows(cx);
            self.dc.rows_scans = scans;
        }
    }

    fn dc_build_rows(&mut self, cx: &AppCx) {
        let (dir, user) = Self::dc_ids(cx);
        self.dc.rows_dirty = false;
        self.dc.rows_at = Some(Instant::now());
        // the person's workspaces and what each one has chosen
        let reg = WsRegistry::load(&dir, &user);
        let keep = self.dc.ws.get(self.dc.ws_idx).map(|w| w.1.clone());
        self.dc.ws = reg.ordered().into_iter().map(|w| (w.name.clone(), w.root_path(), ProjectConfig::load(&w.root_path()).unwrap_or_default())).collect();
        self.dc.ws_idx = keep.and_then(|k| self.dc.ws.iter().position(|w| w.1 == k)).unwrap_or(0);
        let choice = Choice::load(&dir, &user);
        let managed = cx.services.k3sdev.layout().kubeconfig().display().to_string();
        let res = lock(&self.dc.res).clone().unwrap_or_default();
        let nodes = lock(&self.dc.nodes).clone();
        let tests = lock(&self.dc.tests).clone();
        let defaults_of = |id: &str| -> Vec<(String, [u8; 4])> {
            let mut v = Vec::new();
            if choice.as_ref().is_some_and(|c| c.kubeconfig == id) {
                v.push(("your default".to_string(), ACCENT));
            }
            let named: Vec<&str> = self.dc.ws.iter().filter(|(_, _, c)| c.k8s.kubeconfig == id).map(|w| w.0.as_str()).collect();
            if let Some(first) = named.first() {
                v.push((if named.len() == 1 { format!("default for {first}") } else { format!("default for {first} +{}", named.len() - 1) }, ACCENT));
            }
            v
        };
        let mut rows: Vec<Row> = Vec::new();
        for p in &res.profiles {
            let mut chips: Vec<(String, [u8; 4])> = Vec::new();
            chips.push((p.state().to_string(), if p.reachable { OK } else if p.tested || !p.readable { ERR } else { DIM }));
            if p.kubeconfig == managed {
                chips.push(("managed by IYAKKI".into(), ACCENT));
            }
            if p.k3s {
                chips.push(("k3s".into(), MUTED));
            }
            if !p.local {
                chips.push(("remote".into(), AMBER));
            }
            if p.source == Source::InCluster {
                chips.push(("in-cluster".into(), AMBER));
            }
            chips.extend(defaults_of(&p.kubeconfig));
            if p.looks_like_production() {
                chips.push(("production-looking".into(), ERR));
            }
            if devcluster::read_only_reason(p).is_some() {
                chips.push(("read-only".into(), AMBER));
            }
            let node = nodes.get(&p.kubeconfig).map(|n| format!("node {n}")).unwrap_or_default();
            let sub = [if p.server.is_empty() { "server unknown".to_string() } else { p.server.clone() }, node, p.version.clone(), format!("from {}", p.source.label())].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let title = if p.context.is_empty() || p.context == p.name { p.name.clone() } else { format!("{} · {}", p.name, p.context) };
            rows.push(Row { kind: RowKind::Cluster, id: p.kubeconfig.clone(), context: p.context.clone(), source: p.source, title, sub, chips, profile: Some(p.clone()) });
        }
        for v in kubeprofiles::vault_load(&dir, &user) {
            let mut chips: Vec<(String, [u8; 4])> = vec![("vault".into(), MUTED)];
            if let Some(t) = tests.get(&v.reference) {
                chips.insert(0, if t.contains("reachable") && !t.contains("unreachable") { ("reachable".into(), OK) } else { ("unreachable".into(), ERR) });
            } else {
                chips.insert(0, ("not tested".into(), DIM));
            }
            chips.extend(defaults_of(&v.reference));
            rows.push(Row { kind: RowKind::Vault(v.name.clone()), id: v.reference.clone(), context: String::new(), source: Source::Vault, title: v.name.clone(), sub: format!("{} · the kubeconfig stays in the vault; a 0600 temp file exists only while a command runs", v.reference), chips, profile: None });
        }
        for h in &res.hidden {
            rows.push(Row { kind: RowKind::Hidden, id: h.clone(), context: String::new(), source: Source::UserKube, title: h.clone(), sub: "hidden in IYAKKI: never probed, listed in tools or used; the file itself is untouched".into(), chips: vec![("hidden".into(), DIM)], profile: None });
        }
        // the selection follows the row, not its place: a look that finishes meanwhile may add or move rows
        let chosen = self.dc.rows.get(self.sel[self.tab]).map(|r| r.id.clone());
        self.dc.rows = rows;
        let n = self.dc.rows.len();
        let i = &mut self.sel[self.tab];
        if let Some(at) = chosen.and_then(|id| self.dc.rows.iter().position(|r| r.id == id)) {
            *i = at;
        }
        *i = (*i).min(n.saturating_sub(1));
        // the saved registry choice of the workspace in view (and, in the background, whether it answers)
        self.dc_registry_probe();
    }

    fn dc_registry_probe(&mut self) {
        let Some((name, _, cfg)) = self.dc.ws.get(self.dc.ws_idx).cloned() else {
            *lock(&self.dc.reg) = None;
            return;
        };
        let saved = regsetup::RegStatus::saved(&cfg.k8s);
        let slot = self.dc.reg.clone();
        let current = lock(&slot).as_ref().is_some_and(|(n, s)| *n == name && s.kind == saved.kind && s.push == saved.push);
        if !current {
            *lock(&slot) = Some((name.clone(), saved));
            if let Some((run, _)) = self.dc_world() {
                let k8s = cfg.k8s.clone();
                std::thread::spawn(move || {
                    let s = regsetup::status(&k8s, &*run);
                    *lock(&slot) = Some((name, s));
                });
            }
        }
    }

    fn dc_row(&self) -> Option<Row> {
        self.dc.rows.get(self.sel()).cloned()
    }

    // ------------------------------------------------------------------ keys

    pub(super) fn handle_devcluster(&mut self, k: &Key, cx: &AppCx) {
        self.dc_prepare(cx);
        let len = self.dc.rows.len();
        match k {
            Key::Up => return self.move_sel(-1, len),
            Key::Down => return self.move_sel(1, len),
            Key::Char('r') => {
                self.dc.asked = false;
                self.dc.sit = None;
                self.dc_scan(cx);
                return self.ok("Looking for clusters again…");
            }
            Key::Char('t') => return self.dc_test(cx),
            Key::Char('d') => return self.dc_default_me(cx),
            Key::Char('w') => return self.dc_default_workspace(cx),
            Key::Char('k') => {
                if self.dc.ws.is_empty() {
                    return self.fail("You have no workspace yet (the AI IDE makes them).");
                }
                self.dc.ws_idx = (self.dc.ws_idx + 1) % self.dc.ws.len();
                self.dc_registry_probe();
                let n = self.dc.ws[self.dc.ws_idx].0.clone();
                return self.ok(format!("Workspace choices now apply to {n}."));
            }
            Key::Char('h') => return self.dc_hide(cx),
            Key::Char('x') => return self.dc_disconnect(cx),
            Key::Char('v') => return self.dc_vault_form(cx),
            Key::Char('V') => return self.dc_vault_remove(),
            Key::Char('g') => return self.dc_registry_form(cx),
            Key::Char('P') => return self.dc_open_platform(cx, None),
            Key::Char('C') => return self.dc_open_clusters(cx),
            _ => {}
        }
        // the machine's own cluster: administrators only
        let sit = self.dc_situation(cx);
        match k {
            Key::Char('s') => self.dc_setup_form(cx, &sit),
            Key::Char('b') => self.dc_act_overlay(cx, DcAct::Start),
            Key::Char('o') => self.dc_act_overlay(cx, DcAct::Stop),
            Key::Char('e') => self.dc_act_overlay(cx, DcAct::Restart),
            Key::Char('l') => self.dc_logs(cx),
            Key::Char('p') => self.dc_upgrade(cx),
            Key::Char('a') => self.dc_autostart(cx),
            Key::Char('z') => self.dc_typed_form(cx, DcForm::Reset),
            Key::Char('U') => self.dc_typed_form(cx, DcForm::Uninstall),
            _ => {}
        }
    }

    /// Events while the plan screen is up: scroll and select the text, Approve (y) or Cancel (Esc).
    pub(super) fn dc_overlay_event(&mut self, ev: &Event, cx: &AppCx) {
        let Some(o) = self.dc.overlay.as_mut() else { return };
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => self.dc.overlay = None,
            Event::Key(Key::Char('y')) | Event::Key(Key::Enter) if !matches!(o.act, OverlayAct::Close) => {
                if !o.blocked.is_empty() {
                    let why = o.blocked.clone();
                    return self.fail(why);
                }
                let o = self.dc.overlay.take().expect("overlay");
                self.dc_approved(cx, o.act);
            }
            Event::Key(Key::Enter) => self.dc.overlay = None,
            Event::Key(Key::Up) => o.log.scroll_by(1),
            Event::Key(Key::Down) => o.log.scroll_by(-1),
            Event::Key(Key::PageUp) => o.log.scroll_by(10),
            Event::Key(Key::PageDown) => o.log.scroll_by(-10),
            Event::Scroll(d) => o.log.scroll_by(*d * 3),
            Event::Tick => o.log.on_tick(),
            other => {
                o.log.handle(other);
            }
        }
    }

    fn dc_show(&mut self, title: &str, text: &str, act: OverlayAct, blocked: String) {
        self.dc.overlay = Some(Overlay { title: title.into(), log: lines_of(text), act, blocked });
    }

    fn dc_approved(&mut self, cx: &AppCx, act: OverlayAct) {
        let (svc, user) = (cx.services.k3sdev.clone(), cx.session.user.clone());
        match act {
            OverlayAct::Close => {}
            OverlayAct::Run(a) => self.dc_run(cx, a),
            OverlayAct::Setup(plan) => {
                if !self.admin(cx) {
                    return;
                }
                let Some(st) = cx.services.streamer() else { return self.fail("No network connection is set up for downloads.") };
                self.start("setting up the local cluster", move || {
                    JobOut::Msg(svc.install(&plan, &*st, &user).and_then(|_| svc.start(&user)).map(|_| format!("Installed {} {} in {} and started it. The first start takes a minute or two; SYSTEM and this page show when the node is Ready. The registry ({}) is the next step: press g.", plan.backend.label(), plan.version, svc.layout().root.display(), plan.registry)))
                });
            }
            OverlayAct::Upgrade(plan) => {
                if !self.admin(cx) {
                    return;
                }
                let Some(st) = cx.services.streamer() else { return self.fail("No network connection is set up for downloads.") };
                self.start("upgrading the cluster", move || JobOut::Msg(svc.upgrade(&plan, &*st, &user).map(|_| format!("Upgraded to {}. The old program is kept as k3s.prev next to it.", plan.version))));
            }
            OverlayAct::Registry(plan, ws) => {
                let Some((run, _)) = self.dc_world() else { return self.fail("No cluster tools are available here.") };
                cx.audit.record(&user, "devcluster.registry.apply", &format!("{} {ws}", plan.option.kind()));
                self.start("setting up the registry", move || {
                    JobOut::Msg(regsetup::apply(&plan, &*run, true).map(|a| {
                        let mut t = format!("Registry option {} for {ws}: {}.", plan.option.kind(), if a.done.is_empty() { "nothing to do".to_string() } else { a.done.join("; ") });
                        if !a.to_run.is_empty() {
                            t.push_str(&format!(" IYAKKI does not use sudo: run as root on the node: {}", a.to_run.join(" ")));
                        }
                        t
                    }))
                });
                self.dc.rows_dirty = true;
            }
        }
    }

    fn dc_run(&mut self, cx: &AppCx, a: DcAct) {
        let (svc, user) = (cx.services.k3sdev.clone(), cx.session.user.clone());
        match a {
            DcAct::Start => {
                if self.admin(cx) {
                    self.start("starting the cluster", move || JobOut::Msg(svc.start(&user).map(|_| "Starting. The first start takes a minute or two.".to_string())));
                }
            }
            DcAct::Stop => {
                if self.admin(cx) {
                    self.start("stopping the cluster", move || JobOut::Msg(svc.stop(&user).map(|_| "Stopped. It starts again with Start (or at the next start of IYAKKI when Autostart is on).".to_string())));
                }
            }
            DcAct::Restart => {
                if self.admin(cx) {
                    self.start("restarting the cluster", move || JobOut::Msg(svc.restart(&user).map(|_| "Restarting.".to_string())));
                }
            }
            DcAct::Disconnect(id) => self.dc_do_disconnect(cx, &id),
            DcAct::RemoveVault(name) => {
                let (dir, user) = Self::dc_ids(cx);
                match kubeprofiles::vault_remove(&dir, &user, &name) {
                    Ok(()) => {
                        cx.audit.record(&user, "devcluster.vault_profile.remove", &name);
                        self.dc.rows_dirty = true;
                        self.ok(format!("Removed the profile {name} (the vault entry itself is kept)."));
                    }
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    pub(super) fn dc_confirm(&mut self, cx: &AppCx, a: DcAct) {
        self.dc_run(cx, a);
    }

    /// A job finished with a plan to show.
    pub(super) fn dc_job_done(&mut self, cx: &AppCx, out: DcOut) {
        match out {
            DcOut::SetupPlan(Err(e)) | DcOut::RegistryPlan(Err(e)) | DcOut::UpgradePlan(Err(e)) => self.fail(e),
            DcOut::SetupPlan(Ok(plan)) => {
                let mut text = cx.services.k3sdev.describe(&plan);
                if plan.backend == Backend::Native && !iyakki_agent::wsexec::on_path("podman") {
                    text.push_str("\n\nThe Podman variant is not offered here: podman is not installed. It would need the podman packages in the image (docs/K3S-DEV.md, \"The Podman variant\").");
                }
                let blocked = match cx.services.k3sdev.situation() {
                    Situation::Free { problems } if problems.is_empty() => String::new(),
                    Situation::Free { problems } => format!("Cannot set up here: {}.", problems.join("; ")),
                    Situation::Managed => "IYAKKI already manages a cluster here.".into(),
                    Situation::Foreign(f) | Situation::Window { found: f } if !f.is_empty() => k3sdev::refusal_text(&f),
                    Situation::Window { .. } => "In a desktop window IYAKKI never installs a cluster.".into(),
                    Situation::Foreign(_) => "Another cluster exists here.".into(),
                };
                self.dc_show("Set up a local cluster: review, then approve", &text, OverlayAct::Setup(plan), blocked);
            }
            DcOut::UpgradePlan(Ok(None)) => self.ok("The cluster already runs the latest k3s release."),
            DcOut::UpgradePlan(Ok(Some(plan))) => {
                let cur = cx.services.k3sdev.installed().map(|s| s.version).unwrap_or_default();
                let running = matches!(cx.services.k3sdev.state(), Svc::Starting | Svc::Running);
                let text = format!("Upgrade k3s {cur} -> {}\n\n{}\n\n{}", plan.version, k3sdev::plan_text(&plan, true, false).split("\nWHERE IT IS INSTALLED").next().unwrap_or_default().lines().skip(1).collect::<Vec<_>>().join("\n"), if running { "The cluster is stopped while the program is replaced and started again. The download is verified first; if it does not match, nothing changes. The old program is kept as k3s.prev." } else { "The cluster is not running; the program is replaced and stays stopped. The download is verified first. The old program is kept as k3s.prev." });
                self.dc_show("Upgrade the cluster: review, then approve", &text, OverlayAct::Upgrade(plan), String::new());
            }
            DcOut::RegistryPlan(Ok((plan, ws))) => {
                let text = format!("Workspace {ws}\n\n{}", regsetup::preview(&plan));
                self.dc_show("Set up the registry: review, then approve", &text, OverlayAct::Registry(Box::new(plan), ws), String::new());
            }
        }
    }

    // ------------------------------------------------------------------ profile actions

    fn dc_test(&mut self, cx: &AppCx) {
        let Some(row) = self.dc_row() else { return self.fail("No cluster is selected.") };
        if row.kind == RowKind::Hidden {
            return self.fail("A hidden cluster is never probed. Show it first (h), then test it.");
        }
        let Some((run, _)) = self.dc_world() else { return self.fail("No cluster tools are available here.") };
        cx.audit.record(&cx.session.user, "devcluster.test", &row.id);
        let ns = self.dc_namespace();
        let tests = self.dc.tests.clone();
        let (id, ctx, source) = (row.id.clone(), row.context.clone(), row.source);
        self.start("testing the connection", move || {
            let p = devcluster::inspect(&run, Path::new(&id), source, &ctx, true);
            let mut parts = vec![p.chip()];
            if !p.server.is_empty() {
                parts.push(p.server.clone());
            }
            if p.reachable {
                if !p.version.is_empty() {
                    parts.push(p.version.clone());
                }
                let kc = devcluster::Kc::new(run.clone(), &p.kubeconfig, &p.context);
                let nodes = kc.read(&["get", "nodes", "-o", "jsonpath={.items[*].metadata.name}"]);
                if nodes.ok() && !nodes.stdout.trim().is_empty() {
                    parts.push(format!("node {}", nodes.stdout.trim()));
                }
                parts.push(devcluster::can_i_summary(&run, &p, &ns));
                if let Some(why) = devcluster::read_only_reason(&p) {
                    parts.push(format!("assistant: read-only ({why})"));
                }
            } else {
                parts.push(if p.problem.is_empty() { "did not answer".to_string() } else { p.problem.clone() });
                if !p.fix.is_empty() {
                    parts.push(p.fix.clone());
                }
            }
            let text = parts.join(" · ");
            lock(&tests).insert(id, text.clone());
            JobOut::Msg(if p.reachable { Ok(text) } else { Err(text) })
        });
        self.dc.rows_dirty = true;
    }

    fn dc_default_me(&mut self, cx: &AppCx) {
        let Some(row) = self.dc_row() else { return self.fail("No cluster is selected.") };
        if row.kind == RowKind::Hidden {
            return self.fail("That cluster is hidden. Show it first (h).");
        }
        if let Some(p) = &row.profile {
            if !p.readable {
                return self.fail(if p.fix.is_empty() { p.problem.clone() } else { p.fix.clone() });
            }
        }
        let (dir, user) = Self::dc_ids(cx);
        match (Choice { kubeconfig: row.id.clone(), context: row.context.clone() }).save(&dir, &user) {
            Ok(()) => {
                cx.audit.record(&user, "devcluster.default.user", &row.id);
                self.dc.rows_dirty = true;
                self.dc.asked = false;
                self.ok(format!("{} is now your default cluster (every workspace of yours without its own choice uses it).", row.title));
            }
            Err(e) => self.fail(e),
        }
    }

    fn dc_default_workspace(&mut self, cx: &AppCx) {
        let Some(row) = self.dc_row() else { return self.fail("No cluster is selected.") };
        if row.kind == RowKind::Hidden {
            return self.fail("That cluster is hidden. Show it first (h).");
        }
        let Some((name, root, _)) = self.dc.ws.get(self.dc.ws_idx).cloned() else { return self.fail("You have no workspace yet (the AI IDE makes them).") };
        let (dir, user) = Self::dc_ids(cx);
        // the workspace file is shared with other people of the project: a file this account may not read, or a vault entry, stays
        // in the person's own choice, as in the K8S panel
        let shareable = !kubeprofiles::is_vault_ref(&row.id) && cx.session.policy.allows_read(&user, &row.id);
        let _ = (Choice { kubeconfig: row.id.clone(), context: row.context.clone() }).save(&dir, &user);
        if !shareable {
            cx.audit.record(&user, "devcluster.default.user", &row.id);
            self.dc.rows_dirty = true;
            return self.ok(format!("{} is kept as your own choice: it is a vault entry or a file outside your file scope, so it is not written into {name}'s project.json.", row.title));
        }
        match ProjectConfig::load(&root).and_then(|mut c| {
            c.k8s.kubeconfig = row.id.clone();
            c.k8s.profile = row.context.clone();
            c.save(&root)
        }) {
            Ok(()) => {
                cx.audit.record(&user, "devcluster.default.workspace", &format!("{name} {}", row.id));
                self.dc.rows_dirty = true;
                self.ok(format!("{} is now the cluster of workspace {name} (saved in its .iyakki/project.json).", row.title));
            }
            Err(e) => self.fail(e),
        }
    }

    fn dc_hide(&mut self, cx: &AppCx) {
        let Some(row) = self.dc_row() else { return self.fail("No cluster is selected.") };
        let (dir, user) = Self::dc_ids(cx);
        if row.kind == RowKind::Hidden {
            let l: Vec<String> = devcluster::hidden_load(&dir, &user).into_iter().filter(|h| *h != row.id).collect();
            return match devcluster::hidden_save(&dir, &user, &l) {
                Ok(()) => {
                    cx.audit.record(&user, "devcluster.show", &row.id);
                    self.dc.asked = false;
                    self.ok("Shown again (the next look probes it).");
                }
                Err(e) => self.fail(e),
            };
        }
        if row.kind != RowKind::Cluster {
            return self.fail("A vault profile is removed with V, not hidden.");
        }
        if Choice::load(&dir, &user).is_some_and(|c| c.kubeconfig == row.id) {
            return self.fail("That is your default cluster. Choose another default, or Disconnect, before hiding it.");
        }
        match devcluster::hide(&dir, &user, &row.id) {
            Ok(()) => {
                cx.audit.record(&user, "devcluster.hide", &row.id);
                self.dc.asked = false;
                self.ok("Hidden in IYAKKI: it is not probed, listed or used by any tool. The kubeconfig file is untouched; select the hidden row and press h to show it again.");
            }
            Err(e) => self.fail(e),
        }
    }

    fn dc_disconnect(&mut self, cx: &AppCx) {
        let Some(row) = self.dc_row() else { return self.fail("No cluster is selected.") };
        let (dir, user) = Self::dc_ids(cx);
        let mine = Choice::load(&dir, &user).is_some_and(|c| c.kubeconfig == row.id);
        let ws: Vec<String> = self.dc.ws.iter().filter(|(_, _, c)| c.k8s.kubeconfig == row.id).map(|w| w.0.clone()).collect();
        if !mine && ws.is_empty() {
            return self.fail("Nothing of yours points at this cluster, so there is nothing to disconnect.");
        }
        let text = format!("Disconnect from {}\n\nThis forgets the choices that point at this cluster:\n{}{}\nNothing in the cluster and no kubeconfig file is changed. Workspaces fall back to this machine's own k3s, or to the next default.", row.title, if mine { "  - your default cluster\n" } else { "" }, ws.iter().map(|w| format!("  - the cluster of workspace {w}\n")).collect::<String>());
        self.dc_show("Disconnect: review, then approve", &text, OverlayAct::Run(DcAct::Disconnect(row.id.clone())), String::new());
    }

    fn dc_do_disconnect(&mut self, cx: &AppCx, id: &str) {
        let (dir, user) = Self::dc_ids(cx);
        if Choice::load(&dir, &user).is_some_and(|c| c.kubeconfig == id) {
            let _ = Choice::default().save(&dir, &user);
        }
        let mut cleared = Vec::new();
        for (name, root, cfg) in &self.dc.ws {
            if cfg.k8s.kubeconfig == id {
                let r = ProjectConfig::load(root).and_then(|mut c| {
                    c.k8s.kubeconfig.clear();
                    c.k8s.profile.clear();
                    c.save(root)
                });
                if r.is_ok() {
                    cleared.push(name.clone());
                }
            }
        }
        cx.audit.record(&user, "devcluster.disconnect", &format!("{id} {}", cleared.join(",")));
        self.dc.rows_dirty = true;
        self.dc.asked = false;
        self.ok(format!("Disconnected{}.", if cleared.is_empty() { String::new() } else { format!(" (also workspace {})", cleared.join(", ")) }));
    }

    fn dc_vault_form(&mut self, cx: &AppCx) {
        let _ = cx;
        let f = Form::new("Add a kubeconfig held in a vault")
            .text("Profile name", "", "e.g. staging eks")
            .text("Vault entry", "", "the entry that holds the kubeconfig text (Settings \u{2192} Vault); the secret never leaves the vault")
            .choice("Which vault", &["my personal vault (uvault:)", "the system vault (vault:, administrators)"], 0);
        self.open_form(FormKind::Dc(DcForm::Vault), f);
    }

    fn dc_vault_remove(&mut self) {
        match self.dc_row() {
            Some(Row { kind: RowKind::Vault(name), .. }) => self.modal = Some(Modal::Confirm(ConfirmKind::Dc(DcAct::RemoveVault(name.clone())), format!("Remove the profile {name}?"))),
            _ => self.fail("Select a vault profile to remove it (cluster files are hidden with h)."),
        }
    }

    // ------------------------------------------------------------------ registry

    fn dc_registry_form(&mut self, cx: &AppCx) {
        let _ = cx;
        let Some((name, _, cfg)) = self.dc.ws.get(self.dc.ws_idx).cloned() else { return self.fail("The registry is chosen per workspace and you have none yet (the AI IDE makes them).") };
        let saved = regsetup::RegStatus::saved(&cfg.k8s);
        let names: Vec<&str> = regsetup::CHOICES.iter().map(|c| c.1).collect();
        let pre = regsetup::CHOICES.iter().position(|c| c.0 == saved.kind || (c.0 == "in-cluster" && saved.kind == "none")).unwrap_or(0);
        let addr = if saved.kind == "remote" { saved.push.clone() } else { String::new() };
        let f = Form::new(&format!("Registry for {name}"))
            .choice(&format!("Where images go (now: {})", saved.chip().replacen("registry: ", "", 1)), &names, pre)
            .text("Existing registry (option C): host:port", &addr, "e.g. 192.168.1.68:30500; http unless the scheme below says https")
            .choice("Scheme (option C)", &["http", "https"], 0)
            .text("NodePort (option A)", "", "empty: 30500, or the next free one");
        self.open_form(FormKind::Dc(DcForm::Registry), f);
    }

    /// The cluster this workspace uses: its own choice, else yours, else the first one that answers.
    fn dc_effective(&self, cfg: &ProjectConfig, cx: &AppCx) -> Option<ClusterProfile> {
        let (dir, user) = Self::dc_ids(cx);
        let key = if !cfg.k8s.kubeconfig.is_empty() { cfg.k8s.kubeconfig.clone() } else { Choice::load(&dir, &user).map(|c| c.kubeconfig).unwrap_or_default() };
        let res = lock(&self.dc.res).clone()?;
        if key.is_empty() { res.reachable().cloned() } else { res.profiles.iter().find(|p| p.kubeconfig == key).cloned() }
    }

    // ------------------------------------------------------------------ the machine's cluster

    fn dc_setup_form(&mut self, cx: &AppCx, sit: &Situation) {
        if !self.admin(cx) {
            return;
        }
        match sit {
            Situation::Free { problems } if problems.is_empty() => {}
            Situation::Free { problems } => return self.fail(format!("The cluster cannot be set up here: {}.", problems.join("; "))),
            Situation::Managed => return self.fail("IYAKKI already manages a cluster here. Use Start, Stop, Upgrade, Reset or Uninstall."),
            Situation::Foreign(f) | Situation::Window { found: f } if !f.is_empty() => return self.fail(k3sdev::refusal_text(f)),
            _ => return self.fail("In a desktop window IYAKKI never installs a cluster: install k3s yourself, or connect a remote cluster."),
        }
        let podman = iyakki_agent::wsexec::on_path("podman");
        let engine = podman || iyakki_agent::wsexec::on_path("docker");
        let f = Form::new("Set up a local cluster (you see the plan next)")
            .choice(if podman { "How to run k3s (podman found)" } else { "How to run k3s (podman not installed)" }, &["Native k3s (recommended)", "k3s in Podman"], 0)
            .choice(if engine { "Registry for images" } else { "Registry for images (B needs podman or docker)" }, &["A  Registry pod inside the cluster (recommended)", "B  System registry (registry:2 container)", "None for now"], 0);
        self.open_form(FormKind::Dc(DcForm::Setup), f);
    }

    fn dc_typed_form(&mut self, cx: &AppCx, kind: DcForm) {
        if !self.admin(cx) {
            return;
        }
        if cx.services.k3sdev.installed().is_none() {
            return self.fail("IYAKKI has not installed a cluster here.");
        }
        let root = cx.services.k3sdev.layout().root.display().to_string();
        let f = if kind == DcForm::Reset {
            Form::new("Reset the dev cluster").text(&format!("Type {} to confirm", k3sdev::NODE_NAME), "", &format!("Deletes only {root}/data: every object, volume and image in the cluster. The program, the kubeconfig and the log stay."))
        } else {
            Form::new("Uninstall the dev cluster").text(&format!("Type {} to confirm", k3sdev::NODE_NAME), "", &format!("Stops the cluster and deletes {root} with all its data. Nothing outside it is touched."))
        };
        self.open_form(FormKind::Dc(kind), f);
    }

    fn dc_act_overlay(&mut self, cx: &AppCx, act: DcAct) {
        if !self.admin(cx) {
            return;
        }
        let d = &cx.services.k3sdev;
        let Some(st) = d.installed() else { return self.fail("IYAKKI has not installed a cluster here (Set up a local cluster first).") };
        let state = d.state();
        let (title, text) = match &act {
            DcAct::Start => {
                if matches!(state, Svc::Starting | Svc::Running | Svc::Stopping) {
                    return self.fail("The cluster is already running.");
                }
                let cmd = if st.backend == Backend::Native { format!("{} {}", d.layout().bin().display(), k3sdev::server_args(d.layout()).join(" ")) } else { format!("podman start {}", k3sdev::POD_NAME) };
                ("Start the dev cluster", format!("Start the dev cluster\n\nThis runs:\n  {cmd}\n\nIt is refused if another cluster exists on this machine. The first start takes a minute or two."))
            }
            DcAct::Stop => {
                if !matches!(state, Svc::Starting | Svc::Running | Svc::Failed(_)) {
                    return self.fail("The cluster is not running.");
                }
                ("Stop the dev cluster", "Stop the dev cluster\n\nSIGTERM goes to the k3s process group (podman stop for the container); after 30 seconds a process that is still there is killed. Everything the cluster stores stays; Start brings it back.".to_string())
            }
            DcAct::Restart => ("Restart the dev cluster", "Restart the dev cluster\n\nStops it as described under Stop, then starts it again. Workloads are interrupted for a minute or two.".to_string()),
            _ => return,
        };
        self.dc_show(title, &text, OverlayAct::Run(act), String::new());
    }

    fn dc_logs(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        if cx.services.k3sdev.installed().is_none() {
            return self.fail("IYAKKI has not installed a cluster here, so there is no log of its own.");
        }
        let t = cx.services.k3sdev.log_tail(400);
        let text = if t.trim().is_empty() { "The log is empty.".to_string() } else { t };
        let mut o = Overlay { title: "The cluster's log (select text with the mouse; Ctrl+C copies)".into(), log: ScrollLog::default(), act: OverlayAct::Close, blocked: String::new() };
        for l in text.lines() {
            o.log.push(l, if l.contains("level=error") || l.contains("Failed") || l.contains("error") { ERR } else { [0, 0, 0, 0] });
        }
        self.dc.overlay = Some(o);
    }

    fn dc_upgrade(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        if cx.services.k3sdev.installed().is_none() {
            return self.fail("IYAKKI has not installed a cluster here.");
        }
        let Some(st) = cx.services.streamer() else { return self.fail("No network connection is set up for downloads.") };
        let svc = cx.services.k3sdev.clone();
        cx.audit.record(&cx.session.user, "devcluster.upgrade.lookup", "");
        self.start("looking for a newer k3s release", move || JobOut::Dc(DcOut::UpgradePlan(svc.upgrade_plan(&*st))));
    }

    fn dc_autostart(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        let Some(s) = cx.services.k3sdev.installed() else { return self.fail("IYAKKI has not installed a cluster here.") };
        match cx.services.k3sdev.set_autostart(!s.autostart, &cx.session.user) {
            Ok(()) => self.ok(if s.autostart { "Autostart is off: the cluster only starts when you press Start." } else { "Autostart is on: the cluster starts when IYAKKI starts." }),
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------ forms

    pub(super) fn dc_submit(&mut self, cx: &AppCx, kind: &DcForm, form: &Form) -> Result<String, String> {
        let (dir, user) = Self::dc_ids(cx);
        match kind {
            DcForm::Vault => {
                let system = form.choice_idx(2) == 1;
                if system && !cx.session.policy.manage_settings {
                    return Err("Only administrators use the system vault.".into());
                }
                let entry = form.value(1);
                let names = if system { cx.services.system_names() } else { cx.services.user_names(&user) };
                match names {
                    Ok(n) if !n.iter().any(|x| *x == entry) => return Err(format!("There is no entry named `{entry}` in {}. Add the kubeconfig text there first (Settings \u{2192} Vault).", if system { "the system vault" } else { "your vault" })),
                    Err(_) => return Err("The vault is locked, so the entry cannot be checked.".into()),
                    _ => {}
                }
                kubeprofiles::vault_add(&dir, &user, &form.value(0), &format!("{}{entry}", if system { "vault:" } else { "uvault:" }))?;
                cx.audit.record(&user, "devcluster.vault_profile.add", &format!("{} {}{entry}", form.value(0), if system { "vault:" } else { "uvault:" }));
                self.dc.rows_dirty = true;
                Ok(format!("Added the profile {}. The kubeconfig stays in the vault; select it and press t to test it.", form.value(0)))
            }
            DcForm::Reset | DcForm::Uninstall => {
                if !self.admin(cx) {
                    return Ok(String::new());
                }
                let typed = form.value(0);
                if typed != k3sdev::NODE_NAME {
                    return Err(format!("Type {} exactly to confirm; nothing was changed.", k3sdev::NODE_NAME));
                }
                let (svc, u) = (cx.services.k3sdev.clone(), user.clone());
                let reset = *kind == DcForm::Reset;
                self.start(if reset { "resetting the cluster" } else { "uninstalling the cluster" }, move || {
                    JobOut::Msg(if reset { svc.reset(&typed, &u).map(|_| "Reset: the cluster's data folder is empty again; Start brings up a fresh cluster.".to_string()) } else { svc.uninstall(&typed, &u).map(|_| "Uninstalled: the cluster and its folder are gone.".to_string()) })
                });
                self.dc.asked = false;
                Ok(String::new())
            }
            DcForm::Setup => {
                if !self.admin(cx) {
                    return Ok(String::new());
                }
                let podman_wanted = form.choice_idx(0) == 1;
                if podman_wanted && !iyakki_agent::wsexec::on_path("podman") {
                    return Err("podman is not installed on this machine, so the Podman variant is not available. The image needs the podman packages (docs/K3S-DEV.md); use the native variant.".into());
                }
                let registry = ["in-cluster", "system", "none"][form.choice_idx(1).min(2)];
                if registry == "system" && !(iyakki_agent::wsexec::on_path("podman") || iyakki_agent::wsexec::on_path("docker")) {
                    return Err("The system registry (B) needs podman or docker, and neither is installed. Choose A or None.".into());
                }
                let Some(st) = cx.services.streamer() else { return Err("No network connection is set up for downloads.".into()) };
                let (svc, backend) = (cx.services.k3sdev.clone(), if podman_wanted { Backend::Podman } else { Backend::Native });
                let clis = cx.services.clis.clone();
                cx.audit.record(&user, "devcluster.setup.lookup", backend.label());
                self.start("looking up the latest k3s release", move || JobOut::Dc(DcOut::SetupPlan(k3sdev::lookup(&clis, &*st, svc.layout(), backend, registry))));
                Ok(String::new())
            }
            DcForm::Registry => {
                let Some((name, root, cfg)) = self.dc.ws.get(self.dc.ws_idx).cloned() else { return Err("You have no workspace.".into()) };
                let (opt, hostport) = (["in-cluster", "system", "remote"][form.choice_idx(0).min(2)], form.value(1));
                let node_port = form.value(3);
                let option = regsetup::RegistryOption::parse(opt, if opt == "in-cluster" { &node_port } else { &hostport }, if form.choice_idx(2) == 1 { "https" } else { "http" }, false)?;
                let profile = self.dc_effective(&cfg, cx).ok_or("No cluster is chosen for this workspace yet: make one the default first (d or w).")?;
                if !profile.reachable {
                    return Err(format!("The cluster {} does not answer right now; test it first (t).", profile.chip()));
                }
                if option == regsetup::RegistryOption::Local && !profile.local {
                    return Err("The system registry (B) only fits a cluster on this machine: its node pulls localhost:5000. Use A or C.".into());
                }
                if matches!(option, regsetup::RegistryOption::InCluster { .. }) {
                    if let Some(why) = devcluster::read_only_reason(&profile) {
                        return Err(why);
                    }
                }
                let Some((run, _)) = self.dc_world() else { return Err("No cluster tools are available here.".into()) };
                let engine = ["podman", "docker"].into_iter().find(|e| run.has(e));
                let mut env = regsetup::PlanEnv::real(engine, Some(&root));
                env.api_host = if profile.local { String::new() } else { profile.server.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or("").to_string() };
                let kube = regsetup::KubeCtx { kubeconfig: profile.kubeconfig.clone(), context: profile.context.clone() };
                cx.audit.record(&user, "devcluster.registry.plan", &format!("{} {name}", option.kind()));
                self.start("planning the registry", move || {
                    if matches!(option, regsetup::RegistryOption::InCluster { .. }) {
                        env.facts = regsetup::probe(&*run, &kube);
                    }
                    env.kube = Some(kube);
                    JobOut::Dc(DcOut::RegistryPlan(regsetup::plan(&option, &env).map(|p| (p, name))))
                });
                Ok(String::new())
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    pub(super) fn render_devcluster(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        self.dc_prepare(cx);
        if let Some(p) = self.dc.clusters.as_mut() {
            p.draw(g, a, s);
            return;
        }
        if let Some(p) = self.dc.platform.as_mut() {
            p.draw(g, a, s);
            return;
        }
        let admin = cx.session.policy.manage_settings;
        let sit = self.dc_situation(cx);
        let health = cx.services.k3sdev.health();
        // keep the health snapshot fresh while the cluster runs (a few kubectl calls, off the screen's thread)
        if health.installed && !matches!(health.state.as_str(), "stopped" | "not installed") && health.checked_at < iyakki_agent::clock::now_unix() - 15 && !self.dc.health_busy.swap(true, Ordering::SeqCst) {
            let (svc, busy) = (cx.services.k3sdev.clone(), self.dc.health_busy.clone());
            std::thread::spawn(move || {
                svc.refresh_health();
                busy.store(false, Ordering::SeqCst);
            });
        }
        // ---- the machine's cluster
        let card_h = self.dc_card(g, a, s, admin, &sit, &health);
        let mut y = a.y + card_h + 8.0 * s;
        // ---- the profiles
        let strip_h = 66.0 * s;
        let rows_h = (a.y + a.h - y - strip_h).max(54.0 * s);
        let (rh, gap) = (52.0 * s, 6.0 * s);
        let visible = ((rows_h + gap) / (rh + gap)).floor().max(1.0) as usize;
        let n = self.dc.rows.len();
        let sel = self.sel().min(n.saturating_sub(1));
        if n == 0 {
            let msg = if lock(&self.dc.res).is_none() || self.dc.scanning.load(Ordering::SeqCst) { "Looking for clusters…" } else { "No kubeconfig was found (looked in $KUBECONFIG, ~/.kube/config, /etc/rancher/k3s/k3s.yaml). Add one held in a vault with v, or install a cluster." };
            let t = g.fit(14.0 * s, msg, Face::Sans, a.w);
            g.text(a.x, y + 6.0 * s, 14.0 * s, MUTED, &t, Face::Sans);
        }
        let first = sel.saturating_sub(visible - 1).min(n.saturating_sub(visible));
        for (i, r) in self.dc.rows.iter().enumerate().skip(first).take(visible) {
            let chips: Vec<(&str, [u8; 4])> = r.chips.iter().take(4).map(|(t, c)| (t.as_str(), *c)).collect();
            let chips_w: f32 = chips.iter().map(|(t, _)| g.text_width(12.0 * s, t, Face::Bold) + 24.0 * s).sum();
            let title = g.fit(17.0 * s, &r.title, Face::Bold, (a.w - 36.0 * s - chips_w).max(80.0 * s));
            row(g, &mut self.tip, i, a.x, y, a.w, rh, s, i == sel, &title, &r.sub, &chips);
            y += rh + gap;
        }
        if n > visible {
            g.text(a.x + a.w - 90.0 * s, a.y + card_h + 8.0 * s - 18.0 * s, 11.0 * s, DIM, &format!("{} of {n}", sel + 1), Face::Sans);
        }
        // ---- details of the selected profile, the namespace rule, the registry
        let sy = a.y + a.h - strip_h + 4.0 * s;
        let ws_name = self.dc.ws.get(self.dc.ws_idx).map(|w| w.0.clone());
        let ns = self.dc_namespace();
        let detail = self.dc_row().map(|r| {
            let tests = lock(&self.dc.tests).clone();
            if let Some(t) = tests.get(&r.id) {
                (t.clone(), MUTED)
            } else if let Some(p) = &r.profile {
                match devcluster::read_only_reason(p) {
                    Some(why) => (format!("The assistant only reads here: {why}"), AMBER),
                    None if !p.can_i.is_empty() => (format!("You may: {} (press t to test again)", p.can_i), MUTED),
                    None => (if p.problem.is_empty() { "Press t to test the connection.".to_string() } else { p.problem.clone() }, if p.problem.is_empty() { MUTED } else { ERR }),
                }
            } else {
                ("Press t to test the connection.".to_string(), MUTED)
            }
        });
        let reg = lock(&self.dc.reg).clone();
        let lines: Vec<(String, [u8; 4])> = vec![
            detail.unwrap_or_else(|| ("".to_string(), MUTED)),
            (format!("Writes go only to the namespace {ns}{} (created on the first approved apply; default, kube-* and iyakki-registry are refused). Every write asks first.", ws_name.as_ref().map(|n| format!(" of workspace {n} (k switches workspace)")).unwrap_or_default()), MUTED),
            (match reg {
                Some((n, st)) => format!("Registry for {n}: {} (g changes it). A cluster whose name, context or server says \u{201c}prod\u{201d} is read-only for the assistant.", st.chip().replacen("registry: ", "", 1)),
                None => "A cluster whose name, context or server says \u{201c}prod\u{201d} is read-only for the assistant. The registry is chosen per workspace (g).".to_string(),
            }, MUTED),
        ];
        for (i, (t, c)) in lines.iter().enumerate() {
            let t = g.fit(13.0 * s, t, Face::Sans, a.w);
            g.text(a.x, sy + i as f32 * 19.0 * s, 13.0 * s, *c, &t, Face::Sans);
        }
        self.dc_render_overlay(g, a, s);
    }

    /// The card about this machine's own cluster; returns its height.
    fn dc_card(&mut self, g: &mut Gfx, a: R, s: f32, admin: bool, sit: &Situation, h: &k3sdev::Health) -> f32 {
        let mut chips: Vec<(String, [u8; 4])> = Vec::new();
        let (title, detail, mut buttons): (String, String, Vec<(&str, [u8; 4], char)>) = match sit {
            Situation::Managed => {
                let col = match h.state.as_str() {
                    "running" => OK,
                    "failed" => ERR,
                    "stopped" => DIM,
                    _ => AMBER,
                };
                chips.push((h.state.clone(), col));
                chips.push((h.backend.label().to_string(), MUTED));
                chips.push((if h.autostart { "autostart on".to_string() } else { "autostart off".to_string() }, DIM));
                let mut d = h.line();
                if let Some(b) = h.data_bytes {
                    d.push_str(&format!(" · data {}", iyakki_storage::human(b)));
                }
                if let Some(p) = h.disk_pct {
                    d.push_str(&format!(" · disk {p}%"));
                }
                if !h.failed_reason.is_empty() {
                    d = h.failed_reason.clone();
                } else if h.restarts > 0 {
                    d.push_str(&format!(" · restarted {}x: {}", h.restarts, h.last_restart_reason));
                }
                let running = matches!(h.state.as_str(), "running" | "starting");
                let mut b: Vec<(&str, [u8; 4], char)> = Vec::new();
                b.push(if running { ("Stop", ERR, 'o') } else { ("Start", ACCENT, 'b') });
                b.extend([("Restart", TEXT, 'e'), ("Logs", TEXT, 'l'), ("Upgrade", TEXT, 'p'), ("Autostart", TEXT, 'a'), ("Reset", ERR, 'z'), ("Uninstall", ERR, 'U')]);
                ("Local cluster managed by IYAKKI".into(), d, b)
            }
            Situation::Foreign(f) | Situation::Window { found: f } if !f.is_empty() => {
                chips.push(("managed elsewhere".into(), AMBER));
                ("This machine already manages its own cluster".into(), format!("{}. IYAKKI never installs, starts or stops another one here; use the clusters below.", f.join("; ")), vec![])
            }
            Situation::Window { .. } => {
                chips.push(("desktop window".into(), DIM));
                ("No cluster found on this machine".into(), "In a desktop window IYAKKI never installs a cluster. Install k3s yourself, or connect a remote one (a kubeconfig, or a vault entry with v).".into(), vec![])
            }
            Situation::Foreign(_) => ("Another cluster exists here".into(), "IYAKKI will not set up a second one.".into(), vec![]),
            Situation::Free { problems } => {
                chips.push(("none".into(), DIM));
                let d = if problems.is_empty() { "IYAKKI can install k3s in its own data folder. You first see exactly what is downloaded, checked and started, and approve.".to_string() } else { format!("Cannot be set up yet: {}.", problems.join("; ")) };
                ("No cluster on this machine".into(), d, if problems.is_empty() { vec![("Set up a local cluster", ACCENT, 's')] } else { vec![] })
            }
        };
        if !admin {
            buttons.clear();
        }
        let bs = 13.0 * s;
        // lay the buttons out first so the card is tall enough for the rows they need
        let widths: Vec<f32> = buttons.iter().map(|b| g.text_width(bs, b.0, Face::Bold) + bs * 1.7 + 8.0 * s).collect();
        let (mut rows_needed, mut used) = (usize::from(!buttons.is_empty()), 0.0f32);
        for w in &widths {
            if used + w > a.w - 28.0 * s && used > 0.0 {
                rows_needed += 1;
                used = 0.0;
            }
            used += w;
        }
        let ch = 64.0 * s + rows_needed as f32 * (bs * 2.1 + 6.0 * s);
        crate::fx::glass(g, a.x, a.y, a.w, ch, 12.0 * s, ACCENT);
        let mut cxp = a.x + a.w - 14.0 * s;
        for (t, c) in chips.iter().rev() {
            let w = g.text_width(12.0 * s, t, Face::Bold) + 18.0 * s;
            cxp -= w;
            crate::fx::chip(g, cxp, a.y + 10.0 * s, s, t, *c, false);
            cxp -= 6.0 * s;
        }
        let t = g.fit(16.0 * s, &title, Face::Bold, (cxp - a.x - 24.0 * s).max(100.0 * s));
        g.text(a.x + 14.0 * s, a.y + 10.0 * s, 16.0 * s, TEXT, &t, Face::Bold);
        let d = g.fit(12.5 * s, &detail, Face::Sans, a.w - 28.0 * s);
        g.text(a.x + 14.0 * s, a.y + 36.0 * s, 12.5 * s, MUTED, &d, Face::Sans);
        let (mut bx, mut by) = (a.x + 14.0 * s, a.y + 60.0 * s);
        for ((label, col, key), w) in buttons.iter().zip(&widths) {
            if bx + w > a.x + a.w - 14.0 * s && bx > a.x + 14.0 * s {
                bx = a.x + 14.0 * s;
                by += bs * 2.1 + 6.0 * s;
            }
            g.button(bx, by, bs, label, *col, Event::Key(Key::Char(*key)));
            bx += w;
        }
        ch
    }

    fn dc_render_overlay(&mut self, g: &mut Gfx, a: R, s: f32) {
        let Some(o) = self.dc.overlay.as_mut() else { return };
        let danger = matches!(o.act, OverlayAct::Run(DcAct::Stop | DcAct::Restart | DcAct::Disconnect(_)));
        let p = crate::fx::modal(g, a, s, 860.0 * s, a.h - 24.0 * s, ACCENT, danger);
        let y0 = crate::fx::modal_title(g, p, s, &o.title, ACCENT);
        let bh = 13.0 * s * 2.1;
        let text = R::new(p.x + 24.0 * s, y0 + 4.0 * s, p.w - 48.0 * s, (p.y + p.h - y0 - bh - 40.0 * s).max(40.0 * s));
        o.log.render(g, text, 13.0 * s, Face::Mono);
        let by = p.y + p.h - bh - 16.0 * s;
        let mut bx = p.x + 24.0 * s;
        if matches!(o.act, OverlayAct::Close) {
            g.button(bx, by, 13.0 * s, "Close", MUTED, Event::Key(Key::Esc));
        } else {
            if o.blocked.is_empty() {
                bx += g.button(bx, by, 13.0 * s, "Approve", if danger { ERR } else { ACCENT }, Event::Key(Key::Char('y'))) + 10.0 * s;
            } else {
                let t = g.fit(12.5 * s, &o.blocked, Face::Sans, p.w - 48.0 * s - 120.0 * s);
                g.text(bx + 100.0 * s, by + 6.0 * s, 12.5 * s, ERR, &t, Face::Sans);
            }
            g.button(bx, by, 13.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
        }
    }
}

#[cfg(test)]
#[path = "settings_devcluster_tests.rs"]
mod tests;
