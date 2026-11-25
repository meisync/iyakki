//! MESH: look at and change a Consul cluster from the shell: the services registered and how healthy each instance is,
//! the machines in the cluster, and the key/value store. The address is set by an administrator in Setup (and the ACL
//! token goes into the system vault, never into a file); every request is checked against the signed-in account's host
//! policy, and changes to the key/value store are written to the audit log.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use iyakki_tile_api::mesh::{ClusterInfo, Health, Instance, MeshClient, NodeInfo, ServiceInfo};
pub use iyakki_tile_api::mesh::MeshPort;
use iyakki_uikit::blink::blink;
use iyakki_uikit::fx;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};

#[cfg(test)]
mod tests;

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(86, 204, 196);
const WARN: [u8; 4] = rgb(255, 196, 94);
pub const TOKEN_NAME: &str = "consul-token";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Services,
    Nodes,
    Kv,
    Setup,
}

const TABS: [(Tab, &str); 4] = [(Tab::Services, "Services"), (Tab::Nodes, "Nodes"), (Tab::Kv, "Key/Value"), (Tab::Setup, "Setup")];

pub enum Mode {
    Browse,
    /// The instances of one service.
    Instances(String),
    /// One key's value.
    Value(String, String),
    Form(Form, FormKind),
    ConfirmDelete(String),
}

#[derive(Clone, PartialEq, Eq)]
pub enum FormKind {
    Config,
    NewKey,
    EditKey(String),
}

pub enum Loaded {
    Cluster(Result<ClusterInfo, String>),
    Services(Result<Vec<ServiceInfo>, String>),
    Instances(String, Result<Vec<Instance>, String>),
    Nodes(Result<Vec<NodeInfo>, String>),
    Keys(String, Result<Vec<String>, String>),
    Value(String, Result<Option<String>, String>),
    Changed(String, Result<(), String>),
}

pub struct Row {
    pub title: String,
    pub sub: String,
    pub color: [u8; 4],
}

pub struct MeshTile {
    pub tab: Tab,
    pub mode: Mode,
    pub sel: usize,
    pub cluster: Option<ClusterInfo>,
    pub services: Vec<ServiceInfo>,
    pub nodes: Vec<NodeInfo>,
    pub instances: Vec<Instance>,
    pub prefix: String,
    pub keys: Vec<String>,
    pub address: String,
    pub busy: usize,
    pub status: Option<(String, bool)>,
    pub tx: Sender<Loaded>,
    pub rx: Receiver<Loaded>,
}

impl MeshTile {
    /// The tile over a port (a fake in tests).
    pub fn new(port: &dyn MeshPort) -> MeshTile {
        let (tx, rx) = channel();
        let address = port.load_address();
        let mut app = MeshTile { tab: if address.is_empty() { Tab::Setup } else { Tab::Services }, mode: Mode::Browse, sel: 0, cluster: None, services: vec![], nodes: vec![], instances: vec![], prefix: String::new(), keys: vec![], address, busy: 0, status: None, tx, rx };
        if app.tab != Tab::Setup {
            app.refresh(port);
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

    /// The client for the configured cluster, after the account's host policy has said yes.
    fn client(&self, port: &dyn MeshPort) -> Result<Arc<dyn MeshClient>, String> {
        if self.address.is_empty() {
            return Err("No Consul address yet. An administrator sets it in Setup.".into());
        }
        let host = port.host_of(&self.address);
        if !port.allows_host(&host) {
            return Err(format!("Your account may not reach {host}."));
        }
        port.client(&self.address, TOKEN_NAME)
    }

    fn spawn(&mut self, port: &dyn MeshPort, f: impl FnOnce(&dyn MeshClient) -> Loaded + Send + 'static) {
        match self.client(port) {
            Ok(c) => {
                let tx = self.tx.clone();
                self.busy += 1;
                std::thread::spawn(move || {
                    let _ = tx.send(f(&*c));
                });
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Load what the current tab shows.
    fn refresh(&mut self, port: &dyn MeshPort) {
        self.status = None;
        match (&self.tab, &self.mode) {
            (Tab::Services, Mode::Instances(name)) => {
                let name = name.clone();
                self.spawn(port, move |c| Loaded::Instances(name.clone(), c.instances(&name)));
            }
            (Tab::Services, _) => {
                self.spawn(port, |c| Loaded::Cluster(c.cluster()));
                self.spawn(port, |c| Loaded::Services(c.services()));
            }
            (Tab::Nodes, _) => {
                self.spawn(port, |c| Loaded::Cluster(c.cluster()));
                self.spawn(port, |c| Loaded::Nodes(c.nodes()));
            }
            (Tab::Kv, _) => {
                let p = self.prefix.clone();
                self.spawn(port, move |c| Loaded::Keys(p.clone(), c.kv_list(&p)));
            }
            (Tab::Setup, _) => {
                if !self.address.is_empty() {
                    self.spawn(port, |c| Loaded::Cluster(c.cluster()));
                }
            }
        }
    }

    pub fn poll(&mut self, port: &dyn MeshPort) {
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
                        self.refresh(port);
                        self.status = Some((what, true));
                    }
                    Err(e) => self.status = Some((e, false)),
                },
            }
        }
    }

    pub fn rows(&self) -> Vec<Row> {
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

    pub fn set_tab(&mut self, tab: Tab, port: &dyn MeshPort) {
        if self.tab != tab || !matches!(self.mode, Mode::Browse) {
            self.tab = tab;
            self.mode = Mode::Browse;
            self.sel = 0;
            self.refresh(port);
        }
    }

    fn open_selected(&mut self, port: &dyn MeshPort) {
        match self.tab {
            Tab::Services if matches!(self.mode, Mode::Browse) => {
                if let Some(s) = self.services.get(self.sel) {
                    self.mode = Mode::Instances(s.name.clone());
                    self.instances.clear();
                    self.refresh(port);
                }
            }
            Tab::Kv => {
                let Some(k) = self.keys.get(self.sel).cloned() else { return };
                if k.ends_with('/') {
                    self.prefix = k;
                    self.sel = 0;
                    self.keys.clear();
                    self.refresh(port);
                } else {
                    port.audit("mesh.kv.get", &k);
                    self.spawn(port, move |c| Loaded::Value(k.clone(), c.kv_get(&k)));
                }
            }
            _ => {}
        }
    }

    fn up_a_folder(&mut self, port: &dyn MeshPort) {
        let trimmed = self.prefix.trim_end_matches('/');
        self.prefix = trimmed.rfind('/').map_or(String::new(), |i| trimmed[..=i].to_string());
        self.sel = 0;
        self.refresh(port);
    }

    pub fn config_form(&self) -> Form {
        Form::new("Consul connection").text("Address", &self.address, "http://consul.local:8500").secret("ACL token (optional)", "stored in the system vault; leave empty to keep the current one")
    }

    pub fn new_key_form(&self) -> Form {
        Form::new("New key").text("Key", &self.prefix, "app/color").text("Value", "", "text")
    }

    pub fn submit(&mut self, form: &Form, kind: FormKind, port: &dyn MeshPort) {
        match kind {
            FormKind::Config => {
                if !port.can_configure() {
                    self.status = Some(("Only an administrator can change the connection.".into(), false));
                    self.mode = Mode::Browse;
                    return;
                }
                let addr = match port.valid_address(&form.value(0)) {
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
                    if let Err(e) = port.store_token(TOKEN_NAME, &token) {
                        self.status = Some((format!("The token was not saved: {e}"), false));
                        self.mode = Mode::Browse;
                        return;
                    }
                }
                match port.save_address(&addr) {
                    Ok(()) => {
                        port.audit("mesh.config", &port.host_of(&addr));
                        self.address = addr;
                        self.cluster = None;
                        self.mode = Mode::Browse;
                        self.refresh(port);
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
                port.audit("mesh.kv.put", &key);
                self.mode = Mode::Browse;
                self.spawn(port, move |c| Loaded::Changed(format!("Saved {key}"), c.kv_put(&key, &value)));
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

impl MeshTile {
    pub fn title(&self) -> &'static str {
        "Mesh"
    }
    pub fn icon(&self) -> Icon {
        Icon::Mesh
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match &self.mode {
            Mode::Form(..) => "Tab next field · Enter next/save · Esc cancel".into(),
            Mode::ConfirmDelete(_) => "y delete · Esc cancel".into(),
            _ => "← → tabs · ↑↓ choose · Enter open · r refresh".into(),
        }
    }

    pub fn help(&self) -> Vec<String> {
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

    pub fn handle(&mut self, ev: &Event, port: &dyn MeshPort) -> Answer {
        if matches!(ev, Event::Tick) {
            self.poll(port);
            return Answer::Stay;
        }
        // a form takes everything
        if let Mode::Form(form, kind) = &mut self.mode {
            let kind = kind.clone();
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    let f = std::mem::replace(form, Form::new(""));
                    self.submit(&f, kind,  port);
                }
                _ => {}
            }
            return Answer::Stay;
        }
        match ev {
            Event::Tab(i) => {
                if let Some((t, _)) = TABS.get(*i) {
                    self.set_tab(*t,  port);
                }
                return Answer::Stay;
            }
            Event::Row(i) => {
                self.sel = *i;
                return Answer::Stay;
            }
            Event::Scroll(d) => {
                let n = self.rows().len();
                self.sel = (self.sel as i32 - d).clamp_safe(0, n.saturating_sub(1) as i32) as usize;
                return Answer::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Answer::Stay };
        if let Mode::ConfirmDelete(key) = &self.mode {
            let key = key.clone();
            if *k == Key::Char('y') {
                port.audit("mesh.kv.delete", &key);
                self.mode = Mode::Browse;
                self.spawn(port, move |c| Loaded::Changed(format!("Deleted {key}"), c.kv_delete(&key)));
            } else {
                self.mode = Mode::Value(key.clone(), String::new());
                self.mode = Mode::Browse;
            }
            return Answer::Stay;
        }
        let n = self.rows().len();
        match k {
            Key::Esc | Key::Backspace => match &self.mode {
                Mode::Browse if self.tab == Tab::Kv && !self.prefix.is_empty() && *k == Key::Backspace => self.up_a_folder(port),
                Mode::Browse if *k == Key::Esc => return Answer::Back,
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
                self.set_tab(TABS[(i + TABS.len() - 1) % TABS.len()].0,  port);
            }
            Key::Right => {
                let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
                self.set_tab(TABS[(i + 1) % TABS.len()].0,  port);
            }
            Key::Char(c @ '1'..='4') if matches!(self.mode, Mode::Browse) => self.set_tab(TABS[(*c as u8 - b'1') as usize].0,  port),
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Enter => self.open_selected(port),
            Key::Char('r') => self.refresh(port),
            Key::Char('t') if self.tab == Tab::Setup => {
                port.audit("mesh.test", &port.host_of(&self.address));
                self.cluster = None;
                self.refresh(port);
                if self.address.is_empty() {
                    self.status = Some(("No Consul address yet.".into(), false));
                }
            }
            Key::Char('c') if self.tab == Tab::Setup => {
                if port.can_configure() {
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
        Answer::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn MeshPort, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // tabs
        let mut tx = x;
        for (i, (t, label)) in TABS.iter().enumerate() {
            let tw = g.text_width(15.0 * s, label, Face::Bold) + 30.0 * s;
            // the tabs that do not fit one row continue on the next
            if tx > x && tx + tw > x + w {
                tx = x;
                y += 40.0 * s;
            }
            let r = R::new(tx, y, tw, 34.0 * s);
            let on = *t == self.tab;
            fx::row_at(g, r.x, r.y, r.w, r.h, 17.0 * s, on, g.hovered(r), ACCENT);
            g.text(r.x + 15.0 * s, r.y + 8.0 * s, 15.0 * s, if on { TEXT } else { MUTED }, label, Face::Bold);
            g.hit(r, Event::Tab(i), label);
            tx += tw + 8.0 * s;
        }
        if self.busy > 0 {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            fx::chip(g, tx + 8.0 * s, y + 4.0 * s, s, &format!("loading{dots}"), fx::CYAN, false);
        }
        y += 48.0 * s;
        // the cluster in one line
        if let Some(c) = &self.cluster {
            let line = format!("{} · Consul {} · leader {} · {} server(s)", c.datacenter, c.version, if c.leader.is_empty() { "none" } else { &c.leader }, c.peers.len());
            fx::status_line(g, x, y, w, s, &line, true);
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
                let token = if port.has_token(TOKEN_NAME) { "stored in the system vault" } else { "none (fine when ACLs are off)" };
                for (title, sub) in [("Address", if self.address.is_empty() { "not set".to_string() } else { self.address.clone() }), ("ACL token", token.to_string())] {
                    fx::glass(g, x, y, w, 62.0 * s, 12.0 * s, ACCENT);
                    g.text(x + 18.0 * s, y + 9.0 * s, 18.0 * s, TEXT, title, Face::Bold);
                    let sub = g.fit(14.0 * s, &sub, Face::Sans, w - 36.0 * s);
                    g.text(x + 18.0 * s, y + 36.0 * s, 14.0 * s, MUTED, &sub, Face::Sans);
                    y += 72.0 * s;
                }
                let (bx, by) = (x, y + 4.0 * s);
                let b1 = if port.can_configure() { g.button(bx, by, 15.0 * s, "Change (c)", ACCENT, Event::Key(Key::Char('c'))) } else { 0.0 };
                g.button(bx + b1 + 10.0 * s, by, 15.0 * s, "Test (t)", TEXT, Event::Key(Key::Char('t')));
                if !port.can_configure() {
                    g.text(x, by + 48.0 * s, 13.0 * s, DIM, "Only an administrator can change the connection.", Face::Sans);
                }
            }
            (_, Mode::Value(key, value)) => {
                g.text(x, y, 18.0 * s, TEXT, key, Face::Bold);
                y += 34.0 * s;
                let lines = g.wrap(15.0 * s, if value.is_empty() { "(empty)" } else { value }, Face::Mono, w - 32.0 * s);
                let h = (lines.len().min(18) as f32 * 22.0 * s + 24.0 * s).max(48.0 * s);
                fx::panel(g, R::new(x, y, w, h), s, ACCENT);
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
                    fx::empty_state(g, R::new(x, y, w, 240.0 * s), s, Icon::Mesh, msg, "Press n to add one, or pick another tab.", ACCENT);
                }
                let rh = 62.0 * s;
                let fit = (((area.y + area.h - y) / (rh + 8.0 * s)).floor() as usize).max(1);
                let start = self.sel.saturating_sub(fit - 1);
                for (i, r) in rows.iter().enumerate().skip(start).take(fit) {
                    let sel = i == self.sel;
                    let rect = R::new(x, y, w, rh);
                    fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 12.0 * s, sel, g.hovered(rect), ACCENT);
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
            Mode::ConfirmDelete(key) => render_confirm(g, area, s, &format!("Delete {key} from Consul?"), "Delete"),
            _ => {}
        }
    }
}

