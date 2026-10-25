//! Settings -> Clusters: the management page of the cluster registry (docs/K3S-DEV.md, "Clusters").
//!
//! One list of every cluster the account can work against: the built-in local one ("This machine"), extra local clusters and remote
//! profiles. Per cluster: status (running, stopped, unreachable, stale; version, nodes ready, last error), kind, GPU badge, active
//! mark, autostart. Actions: set active (Enter), add a remote cluster (kubeconfig path with the file picker, or a vault entry), add
//! a local one (with the cost warning), rename, test connection, start and stop (local only), set up an extra local cluster,
//! remove (local: asks and offers to keep the data; remote: only forgets the profile, its vault entry only on request), autostart
//! for a local cluster, visibility to users, Advanced (raises the local maximum).
//!
//! Administrators and operators manage; users switch among the clusters they see; guests see nothing. The page is a panel that owns
//! every key and click of its host while open (the same shape as Platform services). Nothing here blocks the screen: tests of a remote
//! cluster and the set-up of a local one run on short threads whose answers are picked up on the heartbeat.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_agent::clusters::{self as cl, ClusterEntry, Dot, Fleet, Kind, Registry, Status};
use iyakki_agent::k3sdev::{self, Backend, Svc};
use iyakki_sec::policy::Role;

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

use super::cluster_switch::dot_color;
use super::widgets::{Form, FormEvent};

const ACCENT: [u8; 4] = rgb(120, 170, 255);
const AMBER: [u8; 4] = rgb(255, 184, 77);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormKind {
    AddRemote,
    AddLocal,
    Rename(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    Start(String),
    Stop(String),
    /// Remove a local cluster: keep (y) or delete (D) its data.
    RemoveLocal(String),
    /// Forget a remote profile: keep (y) or also delete its vault entry (D).
    ForgetRemote(String),
    SetUp(String, String),
}

enum Mode {
    List,
    Form(FormKind, Box<Form>),
    Confirm(Pending, String),
}

/// Deletes the vault entry a reference names.
pub type VaultDelete = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

type Job = Arc<Mutex<Option<(String, Result<String, String>)>>>;

pub struct ClustersPanel {
    fleet: Arc<Fleet>,
    dir: PathBuf,
    user: String,
    role: Role,
    workspace: Option<String>,
    streamer: Option<Arc<dyn iyakki_agent::iyakki_proxy::Streamer>>,
    /// Deletes the vault entry a reference (`uvault:name` / `vault:name`) names; only called when the person asks for it.
    vault_delete: Option<VaultDelete>,
    reg: Registry,
    status: Arc<Mutex<HashMap<String, Status>>>,
    sel: usize,
    mode: Mode,
    msg: Option<(String, bool)>,
    job: Job,
    busy: Option<String>,
    open: bool,
    tick: u64,
    /// Tests: do not look at remote clusters on open.
    quiet: bool,
    probed: bool,
}

impl ClustersPanel {
    pub fn new(fleet: Arc<Fleet>, dir: PathBuf, user: &str, role: Role) -> ClustersPanel {
        let reg = Registry::load(&dir, user);
        if !Registry::saved(&dir, user) {
            let _ = reg.save(&dir, user);
        }
        let sel = reg.clusters.iter().position(|c| c.id == reg.active).unwrap_or(0);
        ClustersPanel { fleet, dir, user: user.into(), role, workspace: None, streamer: None, vault_delete: None, reg, status: Arc::new(Mutex::new(HashMap::new())), sel, mode: Mode::List, msg: None, job: Arc::new(Mutex::new(None)), busy: None, open: true, tick: 0, quiet: false, probed: false }
    }

    pub fn with_workspace(mut self, ws: &str) -> ClustersPanel {
        self.workspace = Some(ws.to_string()).filter(|w| !w.is_empty());
        self
    }

    pub fn with_streamer(mut self, st: Option<Arc<dyn iyakki_agent::iyakki_proxy::Streamer>>) -> ClustersPanel {
        self.streamer = st;
        self
    }

    pub fn with_vault_delete(mut self, f: VaultDelete) -> ClustersPanel {
        self.vault_delete = Some(f);
        self
    }

    /// Tests: no automatic look at remote clusters.
    pub fn quiet(mut self) -> ClustersPanel {
        self.quiet = true;
        self
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn registry(&self) -> &Registry {
        &self.reg
    }

    pub fn message(&self) -> Option<&(String, bool)> {
        self.msg.as_ref()
    }

    fn manage(&self) -> bool {
        cl::can_manage(self.role)
    }

    fn rows(&self) -> Vec<ClusterEntry> {
        self.reg.visible_to(self.role).into_iter().cloned().collect()
    }

    fn selected(&self) -> Option<ClusterEntry> {
        self.rows().get(self.sel).cloned()
    }

    fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.msg = Some((m.into(), ok));
    }

    fn save(&mut self) {
        if let Err(e) = self.reg.save(&self.dir, &self.user) {
            self.say(format!("Could not save the list: {e}"), false);
        }
    }

    fn status_of(&self, e: &ClusterEntry) -> Status {
        let now = iyakki_agent::clock::now_unix();
        if e.kind.is_local() {
            return self.fleet.status(e, false).aged(now);
        }
        let mut st = self.status.lock().unwrap_or_else(|x| x.into_inner()).get(&e.id).cloned().unwrap_or_else(Status::unknown).aged(now);
        // the GPU badge follows the AI profile's cache of this cluster's nodes (fresh, or marked 'last known'), whatever the last test saw
        st.gpu = iyakki_agent::clusters::gpu_summary(&e.id).or(st.gpu);
        st
    }

    /// Test one remote cluster (read-only `kubectl get nodes`) on a short thread.
    pub fn test(&mut self, id: &str) {
        let Some(e) = self.reg.get(id).cloned() else { return };
        if e.kind.is_local() {
            self.say(format!("{} is on this machine: its status is read from its own supervisor.", e.phrase()), true);
            return;
        }
        let (fleet, map) = (self.fleet.clone(), self.status.clone());
        let phrase = e.phrase();
        std::thread::spawn(move || {
            let st = fleet.status(&e, true);
            map.lock().unwrap_or_else(|x| x.into_inner()).insert(e.id.clone(), st);
        });
        self.say(format!("Testing {phrase} ..."), true);
    }

    fn test_all(&mut self) {
        let ids: Vec<String> = self.reg.clusters.iter().filter(|c| !c.kind.is_local()).map(|c| c.id.clone()).collect();
        for id in ids {
            self.test(&id);
        }
    }

    fn open_form(&mut self, k: FormKind) {
        if let Some(r) = cl::manage_refusal(self.role) {
            return self.say(r, false);
        }
        let form = match &k {
            FormKind::AddRemote => Form::new("Add a remote cluster")
                .text("Name", "", "letters, digits, spaces and - _ . (for example gpu-box)")
                .choice("Kind", &["remote", "tunnel", "cloud-cli"], 0)
                .path("Kubeconfig file", "", "a path, or a vault entry such as uvault:gpu-kubeconfig", crate::filepicker::PathKind::File, &[])
                .text("Context (optional)", "", "the context name inside the kubeconfig")
                .text("Namespace (optional)", "", "where the assistant works by default"),
            FormKind::AddLocal => Form::new("Add a local cluster").text("Name", "", &format!("{} Its program is downloaded and checked in the next step.", cl::COST_WARNING)),
            FormKind::Rename(id) => {
                let cur = self.reg.get(id).map(|c| c.name.clone()).unwrap_or_default();
                Form::new("Rename the cluster").text("New name", &cur, "the id and the data stay the same")
            }
        };
        self.mode = Mode::Form(k, Box::new(form));
    }

    fn submit(&mut self, k: FormKind, form: &Form) -> Result<String, String> {
        match k {
            FormKind::AddRemote => {
                let kind = ["remote", "tunnel", "cloud-cli"].get(form.choice_idx(1)).and_then(|w| Kind::from_word(w)).unwrap_or(Kind::Remote);
                let e = self.reg.add_remote(&form.value(0), kind, &form.value(2), &form.value(3), &form.value(4))?;
                self.save();
                self.test(&e.id);
                Ok(format!("Added {}. Testing the connection.", e.phrase()))
            }
            FormKind::AddLocal => {
                let e = self.reg.add_local_avoiding(&form.value(0), &cl::taken_by_others(&self.dir, &self.user))?;
                self.save();
                Ok(format!("Added {} (not set up yet: press s to download and install its k3s). {}", e.phrase(), cl::COST_WARNING))
            }
            FormKind::Rename(id) => {
                self.reg.rename(&id, &form.value(0))?;
                self.save();
                Ok("Renamed.".into())
            }
        }
    }

    fn confirm(&mut self, p: Pending) {
        if let Some(r) = cl::manage_refusal(self.role) {
            return self.say(r, false);
        }
        let text = match &p {
            Pending::Start(id) | Pending::Stop(id) | Pending::RemoveLocal(id) | Pending::ForgetRemote(id) | Pending::SetUp(id, _) => {
                let Some(e) = self.reg.get(id) else { return };
                match &p {
                    Pending::Start(_) => format!("{}?\n{}", cl::confirm_text_on("Start", e), cl::COST_WARNING),
                    Pending::Stop(_) => format!("{}?", cl::confirm_text_on("Stop", e)),
                    Pending::RemoveLocal(_) => format!("{}?\ny: remove it and keep its data folder   D: remove it and delete its data   Esc: cancel", cl::confirm_text_on("Remove", e)),
                    Pending::ForgetRemote(_) => format!("{}?\nOnly the profile is forgotten; the cluster itself is not touched.\ny: forget the profile   {}D: also delete its vault entry   Esc: cancel", cl::confirm_text_on("Forget", e), if e.in_vault() { "" } else { "(no vault entry)  " }),
                    Pending::SetUp(_, plan) => format!("{}?\n{plan}", cl::confirm_text_on("Download and set up k3s for", e)),
                }
            }
        };
        self.mode = Mode::Confirm(p, text);
    }

    fn run_job(&mut self, label: String, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.busy = Some(label.clone());
        let slot = self.job.clone();
        std::thread::spawn(move || {
            let r = f();
            *slot.lock().unwrap_or_else(|x| x.into_inner()) = Some((label, r));
        });
    }

    fn do_pending(&mut self, p: Pending, delete: bool) {
        let reg = self.reg.clone();
        let (fleet, user, role) = (self.fleet.clone(), self.user.clone(), self.role);
        match p {
            Pending::Start(id) => {
                let label = reg.get(&id).map(|e| cl::confirm_text_on("Starting", e)).unwrap_or_default();
                let phrase = reg.get(&id).map(|e| e.phrase()).unwrap_or_default();
                self.run_job(label, move || fleet.start(&reg, &id, &user, role).map(|_| format!("Starting {phrase}.")));
            }
            Pending::Stop(id) => {
                let label = reg.get(&id).map(|e| cl::confirm_text_on("Stopping", e)).unwrap_or_default();
                let phrase = reg.get(&id).map(|e| e.phrase()).unwrap_or_default();
                self.run_job(label, move || fleet.stop(&reg, &id, &user, role).map(|_| format!("Stopped {phrase}.")));
            }
            Pending::SetUp(id, _) => {
                let Some(e) = reg.get(&id).cloned() else { return };
                let (Some(inst), Some(d)) = (e.instance(), fleet.get(&e)) else { return };
                let Some(st) = self.streamer.clone() else { return self.say("No network connection is set up for downloads.", false) };
                let clis = fleet.clis();
                let label = cl::confirm_text_on("Setting up", &e);
                self.run_job(label, move || {
                    let plan = k3sdev::lookup_for(&clis, &*st, d.layout(), Backend::Native, "none", &inst)?;
                    d.install(&plan, &*st, &user).map(|_| format!("{} is set up. Press b to start it.", e.phrase()))
                });
            }
            Pending::RemoveLocal(id) => {
                let Some(e) = reg.get(&id).cloned() else { return };
                let mut reg = reg;
                let dir = self.dir.clone();
                let label = cl::confirm_text_on("Removing", &e);
                let mut after = self.reg.clone();
                let r = after.remove(&id);
                if let Err(err) = r {
                    return self.say(err, false);
                }
                self.run_job(label, move || {
                    if let Some(d) = fleet.get(&e) {
                        if !matches!(d.state(), Svc::NotInstalled | Svc::Stopped) {
                            let _ = d.stop(&user);
                        }
                        if delete && d.installed().is_some() {
                            d.uninstall(&e.instance().map(|i| i.node_name).unwrap_or_default(), &user)?;
                        }
                    }
                    reg.remove(&id)?;
                    reg.save(&dir, &user)?;
                    fleet.drop_instance(&id);
                    Ok(format!("Removed {}. {}", e.phrase(), if delete { "Its data was deleted." } else { "Its data folder was kept." }))
                });
                self.reg = after;
                self.save();
            }
            Pending::ForgetRemote(id) => {
                let Some(e) = self.reg.get(&id).cloned() else { return };
                let (dir, user) = (self.dir.clone(), self.user.clone());
                let mut reg = self.reg.clone();
                let del = self.vault_delete.clone();
                let r = cl::forget_remote(&dir, &user, &mut reg, &id, delete, &mut |reference| match &del {
                    Some(f) => f(reference),
                    None => Err("the vault is not available here; the vault entry was not deleted".to_string()),
                });
                match r {
                    Ok(_) => {
                        self.reg = reg;
                        self.save();
                        self.say(format!("Forgot {}. {}", e.phrase(), if delete && e.in_vault() { "Its vault entry was deleted." } else { "The cluster and any vault entry were not touched." }), true);
                    }
                    Err(err) => self.say(format!("Not forgotten: {err}"), false),
                }
            }
        }
    }

    /// The heartbeat: picks up a finished background job.
    fn poll(&mut self) {
        self.tick += 1;
        if !self.probed && !self.quiet {
            self.probed = true;
            self.test_all();
        }
        let done = self.job.lock().unwrap_or_else(|x| x.into_inner()).take();
        if let Some((_label, r)) = done {
            self.busy = None;
            self.reg = Registry::load(&self.dir, &self.user);
            match r {
                Ok(m) => self.say(m, true),
                Err(e) => self.say(e, false),
            }
            let n = self.rows().len();
            self.sel = self.sel.min(n.saturating_sub(1));
        }
    }

    /// Every event of the host while the page is open. Returns whether it stays open.
    pub fn handle(&mut self, ev: &Event) -> bool {
        if matches!(ev, Event::Tick) {
            self.poll();
            return self.open;
        }
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::Form(k, mut f) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => {}
                    FormEvent::Submit => match self.submit(k.clone(), &f) {
                        Ok(m) => self.say(m, true),
                        Err(e) => {
                            f.error = Some(e);
                            self.mode = Mode::Form(k, f);
                        }
                    },
                    _ => self.mode = Mode::Form(k, f),
                }
                return self.open;
            }
            Mode::Confirm(p, text) => {
                match ev {
                    Event::Key(Key::Char('y')) => self.do_pending(p, false),
                    Event::Key(Key::Char('D')) if matches!(p, Pending::RemoveLocal(_) | Pending::ForgetRemote(_)) => self.do_pending(p, true),
                    Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => {}
                    Event::Key(Key::Char('Y')) | Event::Key(Key::Enter) if matches!(p, Pending::Start(_) | Pending::Stop(_) | Pending::SetUp(..)) => self.do_pending(p, false),
                    _ => self.mode = Mode::Confirm(p, text),
                }
                return self.open;
            }
            Mode::List => {}
        }
        let n = self.rows().len();
        match ev {
            Event::Key(Key::Esc) => self.open = false,
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Row(i) => self.sel = (*i).min(n.saturating_sub(1)),
            Event::Key(Key::Enter) | Event::Key(Key::Char('d')) => self.activate(),
            Event::Key(Key::Char(c)) => self.key(*c),
            _ => {}
        }
        self.open
    }

    fn activate(&mut self) {
        let Some(e) = self.selected() else {
            let m = if cl::can_switch(self.role) { "No cluster is available to your account." } else { "guests cannot switch clusters" };
            return self.say(m, false);
        };
        match cl::switch(&self.dir, self.fleet.data_dir(), &self.user, self.role, &e.id) {
            Ok(a) => {
                self.reg = Registry::load(&self.dir, &self.user);
                self.say(format!("Active cluster: {}. Nothing was moved or deleted.", a.phrase()), true);
            }
            Err(err) => self.say(err, false),
        }
    }

    fn key(&mut self, c: char) {
        let Some(e) = self.selected() else { return };
        match c {
            'a' => self.open_form(FormKind::AddRemote),
            'l' => self.open_form(FormKind::AddLocal),
            'r' => self.open_form(FormKind::Rename(e.id.clone())),
            't' => self.test(&e.id),
            'T' => self.test_all(),
            'w' | 'W' => match &self.workspace {
                Some(ws) => {
                    let r = cl::switch_workspace(&self.dir, &self.user, self.role, ws, if c == 'w' { Some(&e.id) } else { None });
                    self.reg = Registry::load(&self.dir, &self.user);
                    match r {
                        Ok(a) => self.say(if c == 'w' { format!("Workspace {ws} uses {}.", a.phrase()) } else { format!("Workspace {ws} follows the active cluster ({}).", a.phrase()) }, true),
                        Err(err) => self.say(err, false),
                    }
                }
                None => self.say("Open this page from a workspace (AI IDE) to set a per-workspace cluster.", false),
            },
            'b' | 'o' | 's' | 'x' | 'u' | 'v' | 'A' if !self.manage() => {
                let m = cl::manage_refusal(self.role).unwrap_or_default();
                self.say(m, false)
            }
            'b' if e.kind.is_local() => self.confirm(Pending::Start(e.id.clone())),
            'o' if e.kind.is_local() => self.confirm(Pending::Stop(e.id.clone())),
            'b' | 'o' => self.say(format!("{} is not on this machine: IYAKKI cannot start or stop it.", e.phrase()), false),
            's' if e.kind.is_local() && !e.is_primary() => {
                let Some(d) = self.fleet.get(&e) else { return };
                if d.installed().is_some() {
                    return self.say(format!("{} is set up already.", e.phrase()), false);
                }
                let Some(inst) = e.instance() else { return };
                let plan = format!("A new k3s in {} with API port {}, node {}, ranges {} and {}. It is downloaded from the vendor and checked against the vendor's checksum.\n{}", d.layout().root.display(), inst.api_port, inst.node_name, inst.cluster_cidr, inst.service_cidr, cl::COST_WARNING);
                self.confirm(Pending::SetUp(e.id.clone(), plan));
            }
            's' => self.say("The built-in local cluster is set up from its own card (Settings -> Clusters -> This machine).", false),
            'x' if e.is_primary() => self.say("The built-in local cluster cannot be removed from the list; use Uninstall on its own page to remove the cluster itself.", false),
            'x' if e.kind.is_local() => self.confirm(Pending::RemoveLocal(e.id.clone())),
            'x' => self.confirm(Pending::ForgetRemote(e.id.clone())),
            'u' => {
                let on = !e.autostart;
                match self.reg.set_autostart(&e.id, on) {
                    Ok(()) => {
                        self.save();
                        self.say(format!("{} {}.", e.phrase(), if on { "starts with IYAKKI" } else { "no longer starts with IYAKKI" }), true)
                    }
                    Err(err) => self.say(err, false),
                }
            }
            'v' => {
                let on = !e.visible_to_users;
                match self.reg.set_visible_to_users(&e.id, on) {
                    Ok(()) => {
                        self.save();
                        self.say(format!("{} is {} to users.", e.phrase(), if on { "visible" } else { "hidden" }), true)
                    }
                    Err(err) => self.say(err, false),
                }
            }
            'A' => {
                self.reg.advanced = !self.reg.advanced;
                self.save();
                self.say(if self.reg.advanced { format!("Advanced: up to {} local clusters. {}", self.reg.max_local(), cl::COST_WARNING) } else { format!("Up to {} local clusters.", self.reg.max_local()) }, true);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ drawing

    fn buttons(&self) -> Vec<(String, [u8; 4], char)> {
        let sel = self.selected();
        let mut b: Vec<(String, [u8; 4], char)> = vec![("Set active".into(), ACCENT, 'd')];
        if self.manage() {
            b.push(("Add remote".into(), TEXT, 'a'));
            b.push(("Add local".into(), TEXT, 'l'));
            b.push(("Rename".into(), TEXT, 'r'));
        }
        b.push(("Test".into(), TEXT, 't'));
        if let Some(e) = &sel {
            if self.manage() && e.kind.is_local() {
                b.push(("Start".into(), OK, 'b'));
                b.push(("Stop".into(), MUTED, 'o'));
                if !e.is_primary() {
                    b.push(("Set up".into(), TEXT, 's'));
                    b.push(("Autostart".into(), TEXT, 'u'));
                }
            }
            if self.manage() && !e.is_primary() {
                b.push(("Remove".into(), ERR, 'x'));
            }
        }
        if self.manage() {
            b.push(("Visible to users".into(), TEXT, 'v'));
            b.push((if self.reg.advanced { "Advanced: on" } else { "Advanced" }.into(), TEXT, 'A'));
        }
        b
    }

    pub fn draw(&mut self, g: &mut Gfx, a: R, s: f32) {
        crate::fx::glass(g, a.x, a.y, a.w, a.h, 12.0 * s, ACCENT);
        let tx = a.x + 16.0 * s;
        g.text(tx, a.y + 10.0 * s, 19.0 * s, TEXT, "Clusters", Face::Bold);
        let sub = if self.role == Role::Guest {
            "Guests do not see clusters.".to_string()
        } else if self.manage() {
            format!("Every cluster you can work against. Enter makes the selected one active. {} of {} local clusters.", self.reg.local_count(), self.reg.max_local())
        } else {
            "Every cluster you can see. Enter makes the selected one active; administrators and operators change the list.".to_string()
        };
        let sub = g.fit(12.5 * s, &sub, Face::Sans, a.w - 32.0 * s);
        g.text(tx, a.y + 36.0 * s, 12.5 * s, MUTED, &sub, Face::Sans);
        let top = a.y + 58.0 * s;
        // buttons (wrapped), laid out first so the list gets what is left
        let items = self.buttons();
        let bs = 13.0 * s;
        let (mut bx, mut by) = (tx, 0.0f32);
        let mut place: Vec<(f32, f32, usize)> = Vec::new();
        for (i, b) in items.iter().enumerate() {
            let w = g.text_width(bs, &b.0, Face::Bold) + bs * 1.7;
            if bx + w > a.x + a.w - 16.0 * s && bx > tx {
                bx = tx;
                by += bs * 2.1 + 6.0 * s;
            }
            place.push((bx, by, i));
            bx += w + 8.0 * s;
        }
        let strip_h = by + bs * 2.1 + 10.0 * s;
        let msg_h = 34.0 * s;
        let list = R::new(a.x + 16.0 * s, top, a.w - 32.0 * s, (a.h - (top - a.y) - strip_h - msg_h - 8.0 * s).max(40.0 * s));
        self.draw_rows(g, list, s);
        let by0 = a.y + a.h - strip_h - msg_h + 2.0 * s;
        for (x, y, i) in place {
            let b = &items[i];
            g.button(x, by0 + y, bs, &b.0, b.1, Event::Key(Key::Char(b.2)));
        }
        let my = a.y + a.h - msg_h + 4.0 * s;
        let line = match (&self.busy, &self.msg) {
            (Some(b), _) => (format!("{b} ..."), MUTED),
            (None, Some((m, ok))) => (m.clone(), if *ok { OK } else { ERR }),
            _ => (String::new(), MUTED),
        };
        let t = g.fit(12.5 * s, line.0.lines().next().unwrap_or(""), Face::Sans, a.w - 32.0 * s);
        g.text(tx, my, 12.5 * s, line.1, &t, Face::Sans);
        match &self.mode {
            Mode::Form(_, f) => f.render(g, a, s, ACCENT, self.tick / 15 % 2 == 0),
            Mode::Confirm(_, text) => self.draw_confirm(g, a, s, text),
            Mode::List => {}
        }
    }

    fn draw_confirm(&self, g: &mut Gfx, area: R, s: f32, text: &str) {
        let p = crate::fx::modal(g, area, s, 560.0 * s, 230.0 * s, ERR, true);
        let mut y = p.y + 22.0 * s;
        for (i, line) in text.lines().enumerate() {
            let sz = if i == 0 { 17.0 * s } else { 13.0 * s };
            let t = g.fit(sz, line, if i == 0 { Face::Bold } else { Face::Sans }, p.w - 48.0 * s);
            g.text(p.x + 24.0 * s, y, sz, if i == 0 { TEXT } else { MUTED }, &t, if i == 0 { Face::Bold } else { Face::Sans });
            y += sz * 1.7;
        }
        let (yes, danger) = match &self.mode {
            Mode::Confirm(Pending::RemoveLocal(_), _) => ("Remove, keep data", true),
            Mode::Confirm(Pending::ForgetRemote(_), _) => ("Forget profile", true),
            Mode::Confirm(Pending::Stop(_), _) => ("Stop", false),
            Mode::Confirm(Pending::SetUp(..), _) => ("Set up", false),
            _ => ("Start", false),
        };
        let mut btns: Vec<(&str, [u8; 4], char)> = vec![(yes, if danger { ERR } else { OK }, 'y')];
        if matches!(&self.mode, Mode::Confirm(Pending::RemoveLocal(_) | Pending::ForgetRemote(_), _)) {
            btns.push(("Also delete data", ERR, 'D'));
        }
        btns.push(("Cancel", MUTED, '\u{1b}'));
        // the buttons wrap to a second row when the dialog is narrow
        let bs = 14.0 * s;
        let widths: Vec<f32> = btns.iter().map(|b| g.text_width(bs, b.0, Face::Bold) + bs * 1.7).collect();
        let (mut x, mut row) = (p.x + 16.0 * s, 0usize);
        let mut place: Vec<(f32, usize)> = Vec::new();
        for w in &widths {
            if x + w > p.x + p.w - 12.0 * s && x > p.x + 16.0 * s {
                x = p.x + 16.0 * s;
                row += 1;
            }
            place.push((x, row));
            x += w + 8.0 * s;
        }
        let rows_n = row + 1;
        for (b, (bx, r)) in btns.iter().zip(place) {
            let by = p.y + p.h - 12.0 * s - (rows_n - r) as f32 * (bs * 2.1 + 6.0 * s);
            g.button(bx, by, bs, b.0, b.1, if b.2 == '\u{1b}' { Event::Key(Key::Esc) } else { Event::Key(Key::Char(b.2)) });
        }
    }

    fn draw_rows(&self, g: &mut Gfx, body: R, s: f32) {
        let rows = self.rows();
        if rows.is_empty() {
            g.text(body.x, body.y + 6.0 * s, 14.0 * s, MUTED, "No cluster is available to your account.", Face::Sans);
            return;
        }
        let (rh, gap) = (56.0 * s, 6.0 * s);
        let visible = (((body.h + gap) / (rh + gap)).floor().max(1.0)) as usize;
        let first = self.sel.saturating_sub(visible - 1).min(rows.len().saturating_sub(visible));
        let eff = self.reg.effective(self.workspace.as_deref().unwrap_or("")).id;
        let mut y = body.y;
        for (i, e) in rows.iter().enumerate().skip(first).take(visible) {
            let st = self.status_of(e);
            let rr = R::new(body.x, y, body.w, rh);
            let hot = g.hovered(rr);
            crate::fx::row_at(g, body.x, y, body.w, rh, 12.0 * s, i == self.sel, hot, ACCENT);
            g.hit(rr, Event::Row(i), &format!("{} ({}, {})", e.name, e.kind.word(), st.dot.word()));
            g.circle(body.x + 16.0 * s, y + 16.0 * s, (4.5 * s).max(2.0), dot_color(st.dot));
            let mut chips: Vec<(String, [u8; 4])> = vec![(e.kind.word().to_string(), if e.kind.is_local() { MUTED } else { AMBER })];
            if e.id == eff {
                chips.push(("active".into(), OK));
            }
            if e.autostart || (e.is_primary() && self.fleet.get(e).is_some_and(|d| d.health().autostart)) {
                chips.push(("autostart".into(), MUTED));
            }
            if let Some(gpu) = &st.gpu {
                chips.push((format!("GPU {gpu}"), ACCENT));
            }
            if e.read_only_reason().is_some() {
                chips.push(("read-only".into(), AMBER));
            }
            let chips_w: f32 = chips.iter().take(4).map(|(t, _)| g.text_width(12.0 * s, t, Face::Bold) + 24.0 * s).sum();
            let t = g.fit(16.0 * s, &e.name, Face::Bold, (body.w - 44.0 * s - chips_w).max(80.0 * s));
            g.text(body.x + 30.0 * s, y + 7.0 * s, 16.0 * s, if st.dot == Dot::Unknown { DIM } else { TEXT }, &t, Face::Bold);
            let sub = g.fit(12.5 * s, &st.line(), Face::Mono, body.w - 44.0 * s);
            g.text(body.x + 30.0 * s, y + 32.0 * s, 12.5 * s, MUTED, &sub, Face::Mono);
            let mut cx = body.x + body.w - 14.0 * s;
            for (label, col) in chips.iter().take(4).rev() {
                let cw = g.text_width(12.0 * s, label, Face::Bold) + 18.0 * s;
                cx -= cw;
                crate::fx::chip(g, cx.max(body.x), y + 8.0 * s, s, label, *col, false);
                cx -= 6.0 * s;
            }
            y += rh + gap;
        }
    }
}

#[cfg(test)]
#[path = "clusters_panel_tests.rs"]
mod tests;
