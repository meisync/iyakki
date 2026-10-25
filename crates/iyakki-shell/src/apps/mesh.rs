//! MESH: look at and change a Consul cluster from the shell: the services registered and how healthy each instance is,
//! the machines in the cluster, and the key/value store. The address is set by an administrator in Setup (and the ACL
//! token goes into the system vault, never into a file); every request is checked against the signed-in account's host
//! policy, and changes to the key/value store are written to the audit log.

use crate::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use iyakki_agent::consul::{self, ClusterInfo, Consul, Health, Instance, NodeInfo, ServiceInfo};

use super::widgets::{Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(86, 204, 196);
const WARN: [u8; 4] = rgb(255, 196, 94);
const TOKEN_NAME: &str = "consul-token";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Services,
    Nodes,
    Kv,
    Setup,
}

const TABS: [(Tab, &str); 4] = [(Tab::Services, "Services"), (Tab::Nodes, "Nodes"), (Tab::Kv, "Key/Value"), (Tab::Setup, "Setup")];

enum Mode {
    Browse,
    /// The instances of one service.
    Instances(String),
    /// One key's value.
    Value(String, String),
    Form(Form, FormKind),
    ConfirmDelete(String),
}

#[derive(Clone, PartialEq, Eq)]
enum FormKind {
    Config,
    NewKey,
    EditKey(String),
}

enum Loaded {
    Cluster(Result<ClusterInfo, String>),
    Services(Result<Vec<ServiceInfo>, String>),
    Instances(String, Result<Vec<Instance>, String>),
    Nodes(Result<Vec<NodeInfo>, String>),
    Keys(String, Result<Vec<String>, String>),
    Value(String, Result<Option<String>, String>),
    Changed(String, Result<(), String>),
}

struct Row {
    title: String,
    sub: String,
    color: [u8; 4],
}

pub struct MeshApp {
    tab: Tab,
    mode: Mode,
    sel: usize,
    cluster: Option<ClusterInfo>,
    services: Vec<ServiceInfo>,
    nodes: Vec<NodeInfo>,
    instances: Vec<Instance>,
    prefix: String,
    keys: Vec<String>,
    address: String,
    busy: usize,
    status: Option<(String, bool)>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

fn config_path(cx: &AppCx) -> std::path::PathBuf {
    cx.services.dir().join("mesh.json")
}

pub fn load_address(cx: &AppCx) -> String {
    std::fs::read_to_string(config_path(cx)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["address"].as_str().map(str::to_string)).unwrap_or_default()
}

fn save_address(cx: &AppCx, address: &str) -> Result<(), String> {
    let p = config_path(cx);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::json!({ "address": address }).to_string()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| e.to_string())
}

impl MeshApp {
    pub fn new(cx: &AppCx) -> MeshApp {
        let (tx, rx) = channel();
        let address = load_address(cx);
        let mut app = MeshApp { tab: if address.is_empty() { Tab::Setup } else { Tab::Services }, mode: Mode::Browse, sel: 0, cluster: None, services: vec![], nodes: vec![], instances: vec![], prefix: String::new(), keys: vec![], address, busy: 0, status: None, tx, rx };
        if app.tab != Tab::Setup {
            app.refresh(cx);
        }
        app
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn service_names(&self) -> Vec<&str> {
        self.services.iter().map(|s| s.name.as_str()).collect()
    }

    pub fn key_names(&self) -> &[String] {
        &self.keys
    }

    fn can_configure(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings
    }

    /// The client for the configured cluster, after the account's host policy has said yes.
    fn client(&self, cx: &AppCx) -> Result<Arc<Consul>, String> {
        if self.address.is_empty() {
            return Err("No Consul address yet. An administrator sets it in Setup.".into());
        }
        let host = consul::host_of(&self.address);
        if !cx.session.policy.allows_host(&host) {
            return Err(format!("Your account may not reach {host}."));
        }
        let token = cx.services.secrets.resolve(&format!("vault:{TOKEN_NAME}"), None).ok().map(|t| t.to_string());
        Consul::new(&self.address, token, cx.services.quick_upstream()).map(Arc::new)
    }

    fn spawn(&mut self, cx: &AppCx, f: impl FnOnce(&Consul) -> Loaded + Send + 'static) {
        match self.client(cx) {
            Ok(c) => {
                let tx = self.tx.clone();
                self.busy += 1;
                std::thread::spawn(move || {
                    let _ = tx.send(f(&c));
                });
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Load what the current tab shows.
    fn refresh(&mut self, cx: &AppCx) {
        self.status = None;
        match (&self.tab, &self.mode) {
            (Tab::Services, Mode::Instances(name)) => {
                let name = name.clone();
                self.spawn(cx, move |c| Loaded::Instances(name.clone(), c.instances(&name)));
            }
            (Tab::Services, _) => {
                self.spawn(cx, |c| Loaded::Cluster(c.cluster()));
                self.spawn(cx, |c| Loaded::Services(c.services()));
            }
            (Tab::Nodes, _) => {
                self.spawn(cx, |c| Loaded::Cluster(c.cluster()));
                self.spawn(cx, |c| Loaded::Nodes(c.nodes()));
            }
            (Tab::Kv, _) => {
                let p = self.prefix.clone();
                self.spawn(cx, move |c| Loaded::Keys(p.clone(), c.kv_list(&p)));
            }
            (Tab::Setup, _) => {
                if !self.address.is_empty() {
                    self.spawn(cx, |c| Loaded::Cluster(c.cluster()));
                }
            }
        }
    }

    fn poll(&mut self, cx: &AppCx) {
        while let Ok(l) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match l {
                Loaded::Cluster(r) => match r {
                    Ok(c) => self.cluster = Some(c),
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Services(r) => match r {
                    Ok(s) => {
                        self.services = s;
                        self.sel = self.sel.min(self.services.len().saturating_sub(1));
                    }
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Instances(name, r) => match r {
                    Ok(i) => {
                        if matches!(&self.mode, Mode::Instances(n) if *n == name) {
                            self.instances = i;
                            self.sel = 0;
                        }
                    }
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Nodes(r) => match r {
                    Ok(n) => {
                        self.nodes = n;
                        self.sel = self.sel.min(self.nodes.len().saturating_sub(1));
                    }
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Keys(prefix, r) => match r {
                    Ok(k) => {
                        if prefix == self.prefix {
                            self.keys = k;
                            self.sel = self.sel.min(self.keys.len().saturating_sub(1));
                        }
                    }
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Value(key, r) => match r {
                    Ok(Some(v)) => self.mode = Mode::Value(key, v),
                    Ok(None) => self.status = Some((format!("{key} has no value (it may have been deleted)."), false)),
                    Err(e) => self.status = Some((e, false)),
                },
                Loaded::Changed(what, r) => match r {
                    Ok(()) => {
                        self.mode = Mode::Browse;
                        self.refresh(cx);
                        self.status = Some((what, true));
                    }
                    Err(e) => self.status = Some((e, false)),
                },
            }
        }
    }

    fn rows(&self) -> Vec<Row> {
        match (&self.tab, &self.mode) {
            (Tab::Services, Mode::Instances(_)) => self
                .instances
                .iter()
                .map(|i| {
                    let bad: Vec<String> = i.checks.iter().filter(|c| c.status != Health::Passing).map(|c| format!("{}: {}", c.name, if c.output.is_empty() { c.status.label() } else { c.output.trim() })).collect();
                    Row { title: format!("{}   {}:{}", i.node, i.address, i.port), sub: if bad.is_empty() { format!("{} · {} check(s) ok", i.status.label(), i.checks.len()) } else { format!("{} · {}", i.status.label(), bad.join(" · ")) }, color: health_color(i.status) }
                })
                .collect(),
            (Tab::Services, _) => self.services.iter().map(|s| Row { title: s.name.clone(), sub: if s.tags.is_empty() { "no tags".into() } else { s.tags.join(", ") }, color: ACCENT }).collect(),
            (Tab::Nodes, _) => self.nodes.iter().map(|n| Row { title: n.name.clone(), sub: n.address.clone(), color: ACCENT }).collect(),
            (Tab::Kv, _) => self
                .keys
                .iter()
                .map(|k| {
                    let name = k.strip_prefix(self.prefix.as_str()).unwrap_or(k);
                    let folder = name.ends_with('/');
                    Row { title: name.to_string(), sub: if folder { "folder · Enter opens it".into() } else { "value · Enter shows it".into() }, color: if folder { WARN } else { ACCENT } }
                })
                .collect(),
            (Tab::Setup, _) => vec![],
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &AppCx) {
        if self.tab != tab || !matches!(self.mode, Mode::Browse) {
            self.tab = tab;
            self.mode = Mode::Browse;
            self.sel = 0;
            self.refresh(cx);
        }
    }

    fn open_selected(&mut self, cx: &AppCx) {
        match self.tab {
            Tab::Services if matches!(self.mode, Mode::Browse) => {
                if let Some(s) = self.services.get(self.sel) {
                    self.mode = Mode::Instances(s.name.clone());
                    self.instances.clear();
                    self.refresh(cx);
                }
            }
            Tab::Kv => {
                let Some(k) = self.keys.get(self.sel).cloned() else { return };
                if k.ends_with('/') {
                    self.prefix = k;
                    self.sel = 0;
                    self.keys.clear();
                    self.refresh(cx);
                } else {
                    cx.audit.record(&cx.session.user, "mesh.kv.get", &k);
                    self.spawn(cx, move |c| Loaded::Value(k.clone(), c.kv_get(&k)));
                }
            }
            _ => {}
        }
    }

    fn up_a_folder(&mut self, cx: &AppCx) {
        let trimmed = self.prefix.trim_end_matches('/');
        self.prefix = trimmed.rfind('/').map_or(String::new(), |i| trimmed[..=i].to_string());
        self.sel = 0;
        self.refresh(cx);
    }

    fn config_form(&self) -> Form {
        Form::new("Consul connection").text("Address", &self.address, "http://consul.local:8500").secret("ACL token (optional)", "stored in the system vault; leave empty to keep the current one")
    }

    fn new_key_form(&self) -> Form {
        Form::new("New key").text("Key", &self.prefix, "app/color").text("Value", "", "text")
    }

    fn submit(&mut self, form: &Form, kind: FormKind, cx: &AppCx) {
        match kind {
            FormKind::Config => {
                if !Self::can_configure(cx) {
                    self.status = Some(("Only an administrator can change the connection.".into(), false));
                    self.mode = Mode::Browse;
                    return;
                }
                let addr = match consul::valid_address(&form.value(0)) {
                    Ok(a) => a,
                    Err(e) => {
                        self.mode = Mode::Form(self.config_form(), FormKind::Config);
                        if let Mode::Form(f, _) = &mut self.mode {
                            f.error = Some(e);
                            f.set_value(0, &form.value(0));
                        }
                        return;
                    }
                };
                let token = form.raw(1);
                if !token.is_empty() {
                    if let Err(e) = cx.services.system_set(TOKEN_NAME, &token) {
                        self.status = Some((format!("The token was not saved: {e}"), false));
                        self.mode = Mode::Browse;
                        return;
                    }
                }
                match save_address(cx, &addr) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "mesh.config", &consul::host_of(&addr));
                        self.address = addr;
                        self.cluster = None;
                        self.mode = Mode::Browse;
                        self.refresh(cx);
                        self.status = Some(("Saved. Checking the connection…".into(), true));
                    }
                    Err(e) => {
                        self.status = Some((format!("Could not save: {e}"), false));
                        self.mode = Mode::Browse;
                    }
                }
            }
            FormKind::NewKey | FormKind::EditKey(_) => {
                let (key, value) = match &kind {
                    FormKind::EditKey(k) => (k.clone(), form.raw(0)),
                    _ => (form.value(0), form.raw(1)),
                };
                cx.audit.record(&cx.session.user, "mesh.kv.put", &key);
                self.mode = Mode::Browse;
                self.spawn(cx, move |c| Loaded::Changed(format!("Saved {key}"), c.kv_put(&key, &value)));
            }
        }
    }
}

fn health_color(h: Health) -> [u8; 4] {
    match h {
        Health::Passing => OK,
        Health::Warning => WARN,
        Health::Critical => ERR,
    }
}

impl App for MeshApp {
    fn title(&self) -> &'static str {
        "Mesh"
    }
    fn icon(&self) -> Icon {
        Icon::Mesh
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match &self.mode {
            Mode::Form(..) => "Tab next field · Enter next/save · Esc cancel".into(),
            Mode::ConfirmDelete(_) => "y delete · Esc cancel".into(),
            _ => "← → tabs · ↑↓ choose · Enter open · r refresh".into(),
        }
    }

    fn help(&self) -> Vec<String> {
        [
            "Left / Right or 1-4 · Services, Nodes, Key/Value, Setup",
            "Up / Down · choose a row      Enter · open it",
            "Esc or Backspace · back out of a service, a value or a folder",
            "r · load again",
            "n · new key (Key/Value)      e · change the value shown",
            "d · delete the key shown (asks first)",
            "c · change the connection (administrators)      t · test it",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.poll(cx);
            return Flow::Stay;
        }
        // a form takes everything
        if let Mode::Form(form, kind) = &mut self.mode {
            let kind = kind.clone();
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let f = std::mem::replace(form, Form::new(""));
                    self.submit(&f, kind, cx);
                }
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            Event::Tab(i) => {
                if let Some((t, _)) = TABS.get(*i) {
                    self.set_tab(*t, cx);
                }
                return Flow::Stay;
            }
            Event::Row(i) => {
                self.sel = *i;
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                let n = self.rows().len();
                self.sel = (self.sel as i32 - d).clamp_safe(0, n.saturating_sub(1) as i32) as usize;
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        if let Mode::ConfirmDelete(key) = &self.mode {
            let key = key.clone();
            if *k == Key::Char('y') {
                cx.audit.record(&cx.session.user, "mesh.kv.delete", &key);
                self.mode = Mode::Browse;
                self.spawn(cx, move |c| Loaded::Changed(format!("Deleted {key}"), c.kv_delete(&key)));
            } else {
                self.mode = Mode::Value(key.clone(), String::new());
                self.mode = Mode::Browse;
            }
            return Flow::Stay;
        }
        let n = self.rows().len();
        match k {
            Key::Esc | Key::Backspace => match &self.mode {
                Mode::Browse if self.tab == Tab::Kv && !self.prefix.is_empty() && *k == Key::Backspace => self.up_a_folder(cx),
                Mode::Browse if *k == Key::Esc => return Flow::Back,
                Mode::Browse => {}
                _ => {
                    self.mode = Mode::Browse;
                    if self.tab == Tab::Services {
                        self.sel = 0;
                    }
                }
            },
            Key::Left => {
                let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
                self.set_tab(TABS[(i + TABS.len() - 1) % TABS.len()].0, cx);
            }
            Key::Right => {
                let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
                self.set_tab(TABS[(i + 1) % TABS.len()].0, cx);
            }
            Key::Char(c @ '1'..='4') if matches!(self.mode, Mode::Browse) => self.set_tab(TABS[(*c as u8 - b'1') as usize].0, cx),
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Enter => self.open_selected(cx),
            Key::Char('r') => self.refresh(cx),
            Key::Char('t') if self.tab == Tab::Setup => {
                cx.audit.record(&cx.session.user, "mesh.test", &consul::host_of(&self.address));
                self.cluster = None;
                self.refresh(cx);
                if self.address.is_empty() {
                    self.status = Some(("No Consul address yet.".into(), false));
                }
            }
            Key::Char('c') if self.tab == Tab::Setup => {
                if Self::can_configure(cx) {
                    self.mode = Mode::Form(self.config_form(), FormKind::Config);
                } else {
                    self.status = Some(("Only an administrator can change the connection.".into(), false));
                }
            }
            Key::Char('n') if self.tab == Tab::Kv => self.mode = Mode::Form(self.new_key_form(), FormKind::NewKey),
            Key::Char('e') => {
                if let Mode::Value(key, v) = &self.mode {
                    let (key, v) = (key.clone(), v.clone());
                    let mut f = Form::new(&format!("Change {key}")).text("Value", &v, "text");
                    f.focus = 0;
                    self.mode = Mode::Form(f, FormKind::EditKey(key));
                }
            }
            Key::Char('d') => {
                if let Mode::Value(key, _) = &self.mode {
                    self.mode = Mode::ConfirmDelete(key.clone());
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // tabs
        let mut tx = x;
        for (i, (t, label)) in TABS.iter().enumerate() {
            let tw = g.text_width(15.0 * s, label, Face::Bold) + 30.0 * s;
            let r = R::new(tx, y, tw, 34.0 * s);
            let on = *t == self.tab;
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 17.0 * s, on, g.hovered(r), ACCENT);
            g.text(r.x + 15.0 * s, r.y + 8.0 * s, 15.0 * s, if on { TEXT } else { MUTED }, label, Face::Bold);
            g.hit(r, Event::Tab(i), label);
            tx += tw + 8.0 * s;
        }
        if self.busy > 0 {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            crate::fx::chip(g, tx + 8.0 * s, y + 4.0 * s, s, &format!("loading{dots}"), crate::fx::CYAN, false);
        }
        y += 48.0 * s;
        // the cluster in one line
        if let Some(c) = &self.cluster {
            let line = format!("{} · Consul {} · leader {} · {} server(s)", c.datacenter, c.version, if c.leader.is_empty() { "none" } else { &c.leader }, c.peers.len());
            crate::fx::status_line(g, x, y, w, s, &line, true);
            y += 26.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            let lines = g.wrap(14.0 * s, m, Face::Sans, w);
            for l in lines.iter().take(3) {
                g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, l, Face::Sans);
                y += 20.0 * s;
            }
            y += 6.0 * s;
        }
        match (&self.tab, &self.mode) {
            (Tab::Setup, _) => {
                let token = if cx.services.secrets.resolve(&format!("vault:{TOKEN_NAME}"), None).is_ok() { "stored in the system vault" } else { "none (fine when ACLs are off)" };
                for (title, sub) in [("Address", if self.address.is_empty() { "not set".to_string() } else { self.address.clone() }), ("ACL token", token.to_string())] {
                    crate::fx::glass(g, x, y, w, 62.0 * s, 12.0 * s, ACCENT);
                    g.text(x + 18.0 * s, y + 9.0 * s, 18.0 * s, TEXT, title, Face::Bold);
                    let sub = g.fit(14.0 * s, &sub, Face::Sans, w - 36.0 * s);
                    g.text(x + 18.0 * s, y + 36.0 * s, 14.0 * s, MUTED, &sub, Face::Sans);
                    y += 72.0 * s;
                }
                let (bx, by) = (x, y + 4.0 * s);
                let b1 = if Self::can_configure(cx) { g.button(bx, by, 15.0 * s, "Change (c)", ACCENT, Event::Key(Key::Char('c'))) } else { 0.0 };
                g.button(bx + b1 + 10.0 * s, by, 15.0 * s, "Test (t)", TEXT, Event::Key(Key::Char('t')));
                if !Self::can_configure(cx) {
                    g.text(x, by + 48.0 * s, 13.0 * s, DIM, "Only an administrator can change the connection.", Face::Sans);
                }
            }
            (_, Mode::Value(key, value)) => {
                g.text(x, y, 18.0 * s, TEXT, key, Face::Bold);
                y += 34.0 * s;
                let lines = g.wrap(15.0 * s, if value.is_empty() { "(empty)" } else { value }, Face::Mono, w - 32.0 * s);
                let h = (lines.len().min(18) as f32 * 22.0 * s + 24.0 * s).max(48.0 * s);
                crate::fx::panel(g, R::new(x, y, w, h), s, ACCENT);
                for (i, l) in lines.iter().take(18).enumerate() {
                    g.text(x + 16.0 * s, y + 12.0 * s + i as f32 * 22.0 * s, 15.0 * s, rgb(200, 215, 240), l, Face::Mono);
                }
                y += h + 14.0 * s;
                let b1 = g.button(x, y, 15.0 * s, "Change (e)", ACCENT, Event::Key(Key::Char('e')));
                g.button(x + b1 + 10.0 * s, y, 15.0 * s, "Delete (d)", ERR, Event::Key(Key::Char('d')));
            }
            _ => {
                if self.tab == Tab::Kv {
                    let at = if self.prefix.is_empty() { "/".to_string() } else { format!("/{}", self.prefix) };
                    g.text(x, y, 15.0 * s, MUTED, &format!("Folder {at}   ·   n new key   ·   Backspace up a folder"), Face::Sans);
                    y += 28.0 * s;
                } else if let Mode::Instances(name) = &self.mode {
                    g.text(x, y, 15.0 * s, MUTED, &format!("Instances of {name}   ·   Esc back"), Face::Sans);
                    y += 28.0 * s;
                }
                let rows = self.rows();
                if rows.is_empty() && self.busy == 0 {
                    let msg = match (&self.tab, &self.mode) {
                        (Tab::Kv, _) => "Nothing under this folder.",
                        (_, Mode::Instances(_)) => "No instances are registered for this service.",
                        _ => "Nothing registered.",
                    };
                    crate::fx::empty_state(g, R::new(x, y, w, 240.0 * s), s, Icon::Mesh, msg, "Press n to add one, or pick another tab.", ACCENT);
                }
                let rh = 62.0 * s;
                let fit = (((area.y + area.h - y) / (rh + 8.0 * s)).floor() as usize).max(1);
                let start = self.sel.saturating_sub(fit - 1);
                for (i, r) in rows.iter().enumerate().skip(start).take(fit) {
                    let sel = i == self.sel;
                    let rect = R::new(x, y, w, rh);
                    crate::fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 12.0 * s, sel, g.hovered(rect), ACCENT);
                    g.hit(rect, Event::Row(i), &r.title);
                    g.rect(x, y + 10.0 * s, 4.0 * s, 42.0 * s, r.color);
                    let title = g.fit(18.0 * s, &r.title, Face::Bold, w - 36.0 * s);
                    let sub = g.fit(14.0 * s, &r.sub, Face::Sans, w - 36.0 * s);
                    g.text(x + 18.0 * s, y + 9.0 * s, 18.0 * s, TEXT, &title, Face::Bold);
                    g.text(x + 18.0 * s, y + 36.0 * s, 14.0 * s, MUTED, &sub, Face::Sans);
                    y += rh + 8.0 * s;
                }
            }
        }
        match &self.mode {
            Mode::Form(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::ConfirmDelete(key) => super::widgets::render_confirm(g, area, s, &format!("Delete {key} from Consul?"), "Delete"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
    use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
    use iyakki_sec::policy::Role;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<String>>,
        kv: Mutex<BTreeMap<String, String>>,
    }

    impl Upstream for Fake {
        fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            self.asked.lock().unwrap().push(format!("{method} {url}"));
            let path = url.strip_prefix("http://consul.local:8500").ok_or("unreachable")?;
            let ok = |s: &str| Ok(UpstreamResponse { status: 200, body: s.as_bytes().to_vec() });
            let base = path.split('?').next().unwrap_or("");
            match (method, base) {
                ("GET", "/v1/agent/self") => ok(r#"{"Config":{"Version":"1.19.0","Datacenter":"dc1"}}"#),
                ("GET", "/v1/status/leader") => ok("\"10.0.0.1:8300\""),
                ("GET", "/v1/status/peers") => ok(r#"["10.0.0.1:8300"]"#),
                ("GET", "/v1/catalog/services") => ok(r#"{"web":["v1"],"api":[]}"#),
                ("GET", "/v1/health/service/web") => ok(r#"[{"Node":{"Node":"n1","Address":"10.0.0.5"},"Service":{"Address":"","Port":80},"Checks":[{"Name":"http","Status":"critical","Output":"connection refused"}]},{"Node":{"Node":"n2","Address":"10.0.0.6"},"Service":{"Address":"","Port":80},"Checks":[{"Name":"http","Status":"passing","Output":"ok"}]}]"#),
                ("GET", "/v1/catalog/nodes") => ok(r#"[{"Node":"n1","Address":"10.0.0.5"}]"#),
                ("GET", "/v1/kv/") => ok(r#"["app/","motd"]"#),
                ("GET", "/v1/kv/app/") => ok(r#"["app/","app/color"]"#),
                ("GET", p) if p.starts_with("/v1/kv/") => match self.kv.lock().unwrap().get(&p[7..]) {
                    Some(v) => Ok(UpstreamResponse { status: 200, body: v.clone().into_bytes() }),
                    None => Ok(UpstreamResponse { status: 404, body: vec![] }),
                },
                ("PUT", p) => {
                    self.kv.lock().unwrap().insert(p[7..].to_string(), String::from_utf8_lossy(body).into_owned());
                    ok("true")
                }
                ("DELETE", p) => {
                    self.kv.lock().unwrap().remove(&p[7..]);
                    ok("true")
                }
                _ => Ok(UpstreamResponse { status: 404, body: vec![] }),
            }
        }
    }

    fn cx_with_consul(tag: &str, address: Option<&str>) -> (AppCx, Arc<Fake>) {
        let mut cx = cx_with_admin_store(tag);
        let fake = Arc::new(Fake::default());
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, fake.clone());
        if let Some(a) = address {
            save_address(&cx, a).unwrap();
        }
        (cx, fake)
    }

    fn settle(a: &mut MeshApp, cx: &AppCx) {
        for _ in 0..4000 {
            a.handle(&Event::Tick, cx);
            if a.busy == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("still loading");
    }

    fn key(a: &mut MeshApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }

    fn typ(a: &mut MeshApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            key(a, cx, Key::Char(c));
        }
    }

    fn draw(a: &mut MeshApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), cx, 0);
        g
    }

    #[test]
    fn with_no_address_the_app_opens_on_setup_and_an_admin_saves_one() {
        let (cx, fake) = cx_with_consul("mesh1", None);
        let mut a = MeshApp::new(&cx);
        assert_eq!(a.tab(), Tab::Setup);
        key(&mut a, &cx, Key::Char('c'));
        assert!(matches!(a.mode, Mode::Form(..)));
        // a bad address is explained and nothing is saved
        typ(&mut a, &cx, "consul.local");
        key(&mut a, &cx, Key::Ctrl('s'));
        let Mode::Form(f, _) = &a.mode else { panic!("the form stays open") };
        assert!(f.error.as_deref().unwrap().contains("http://"), "{:?}", f.error);
        assert!(load_address(&cx).is_empty());
        // fix it
        let Mode::Form(f, _) = &mut a.mode else { unreachable!() };
        f.set_value(0, "http://consul.local:8500");
        key(&mut a, &cx, Key::Ctrl('s'));
        settle(&mut a, &cx);
        assert_eq!(load_address(&cx), "http://consul.local:8500");
        assert_eq!(a.cluster.as_ref().unwrap().datacenter, "dc1", "the connection was checked");
        assert!(fake.asked.lock().unwrap().iter().any(|r| r.ends_with("/v1/agent/self")));
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("mesh.config"));
    }

    #[test]
    fn services_show_their_instances_with_health() {
        let (cx, _) = cx_with_consul("mesh2", Some("http://consul.local:8500"));
        let mut a = MeshApp::new(&cx);
        assert_eq!(a.tab(), Tab::Services);
        settle(&mut a, &cx);
        assert_eq!(a.service_names(), vec!["api", "web"]);
        assert!(a.cluster.as_ref().unwrap().leader.contains("8300"));
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        let rows = a.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].title.contains("n1") && rows[0].sub.contains("critical") && rows[0].sub.contains("connection refused"), "{}", rows[0].sub);
        assert_eq!((rows[0].color, rows[1].color), (ERR, OK));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label.contains("n1")));
        key(&mut a, &cx, Key::Esc);
        assert!(matches!(a.mode, Mode::Browse) && a.tab() == Tab::Services);
        a.handle(&Event::Tab(1), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.rows()[0].title, "n1");
    }

    #[test]
    fn the_key_value_store_is_browsed_changed_and_audited() {
        let (cx, fake) = cx_with_consul("mesh3", Some("http://consul.local:8500"));
        fake.kv.lock().unwrap().insert("app/color".into(), "blue".into());
        let mut a = MeshApp::new(&cx);
        settle(&mut a, &cx);
        a.handle(&Event::Tab(2), &cx);
        settle(&mut a, &cx);
        assert_eq!(a.key_names(), ["app/", "motd"]);
        key(&mut a, &cx, Key::Enter); // into app/
        settle(&mut a, &cx);
        assert_eq!(a.key_names(), ["app/color"]);
        key(&mut a, &cx, Key::Enter); // open the value
        settle(&mut a, &cx);
        assert!(matches!(&a.mode, Mode::Value(k, v) if k == "app/color" && v == "blue"));
        // change it
        key(&mut a, &cx, Key::Char('e'));
        for _ in 0..4 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "green");
        key(&mut a, &cx, Key::Ctrl('s'));
        settle(&mut a, &cx);
        assert_eq!(fake.kv.lock().unwrap().get("app/color").map(String::as_str), Some("green"));
        assert!(a.status_text().unwrap().contains("Saved app/color"));
        // a new key
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "size");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "big");
        key(&mut a, &cx, Key::Ctrl('s'));
        settle(&mut a, &cx);
        assert_eq!(fake.kv.lock().unwrap().get("app/size").map(String::as_str), Some("big"), "{:?}", fake.kv.lock().unwrap());
        // delete asks first
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        assert!(matches!(&a.mode, Mode::Value(..)));
        key(&mut a, &cx, Key::Char('d'));
        assert!(matches!(a.mode, Mode::ConfirmDelete(_)));
        key(&mut a, &cx, Key::Char('x'));
        assert!(!fake.kv.lock().unwrap().is_empty() && matches!(a.mode, Mode::Browse), "anything but y cancels");
        key(&mut a, &cx, Key::Enter);
        settle(&mut a, &cx);
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        settle(&mut a, &cx);
        assert_eq!(fake.kv.lock().unwrap().len(), 1, "one key deleted");
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        for what in ["mesh.kv.get", "mesh.kv.put", "mesh.kv.delete"] {
            assert!(log.contains(what), "{what} in {log}");
        }
        // up a folder with Backspace
        a.mode = Mode::Browse;
        key(&mut a, &cx, Key::Backspace);
        assert_eq!(a.prefix, "");
    }

    #[test]
    fn the_hosts_an_account_may_reach_decide_and_only_admins_configure() {
        let (mut cx, fake) = cx_with_consul("mesh4", Some("http://consul.local:8500"));
        cx.session.role = Role::Operator;
        cx.session.policy = Role::Operator.template();
        cx.session.policy.net_hosts = vec!["other.example".into()];
        let mut a = MeshApp::new(&cx);
        settle(&mut a, &cx);
        assert!(a.status_text().unwrap().contains("may not reach consul.local"), "{:?}", a.status_text());
        assert!(fake.asked.lock().unwrap().is_empty(), "nothing was sent");
        a.handle(&Event::Tab(3), &cx);
        key(&mut a, &cx, Key::Char('c'));
        assert!(matches!(a.mode, Mode::Browse), "operators cannot open the connection form");
        assert!(a.status_text().unwrap().contains("administrator"));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.iter().any(|h| h.label.starts_with("Change")), "no Change button for them");
    }

    #[test]
    fn every_tab_and_mode_draws_at_several_sizes() {
        let (cx, fake) = cx_with_consul("mesh5", Some("http://consul.local:8500"));
        fake.kv.lock().unwrap().insert("app/color".into(), "blue\nline two".into());
        let mut a = MeshApp::new(&cx);
        settle(&mut a, &cx);
        for (w, h) in [(1280, 720), (800, 600), (1920, 1080)] {
            for i in 0..4 {
                a.handle(&Event::Tab(i), &cx);
                settle(&mut a, &cx);
                let _ = draw(&mut a, &cx, w, h);
            }
            a.mode = Mode::Value("app/color".into(), "blue\nline two".into());
            let _ = draw(&mut a, &cx, w, h);
            a.mode = Mode::ConfirmDelete("app/color".into());
            let _ = draw(&mut a, &cx, w, h);
            a.mode = Mode::Form(a.config_form(), FormKind::Config);
            let _ = draw(&mut a, &cx, w, h);
            a.mode = Mode::Browse;
        }
        assert!(!a.help().is_empty());
    }
}
