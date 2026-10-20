//! The CLI tile's Connections tab: saved SSH hosts (name, host, port, user, a key kept in the vault, an optional default command),
//! a Connect action, host-key trust with the fingerprint shown first, and key generation into the vault. Nothing here holds a
//! secret; the file of connections lists names only. There is no terminal emulator yet, so Connect runs the default command (or a
//! quick check) without a prompt and streams its output into the Terminal tab (docs/CLI-SSH.md, docs/ALPHA-SCOPE.md).

use std::sync::{Arc, Mutex};

use iyakki_agent::ssh::{self, Connection, Env, HostKey, Mode};
use iyakki_sec::policy::CliLevel;

use super::super::{AppCx, Flow};
use super::ACCENT;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const WARN: [u8; 4] = rgb(255, 196, 94);
/// The click that saves the form.
pub const SAVE: usize = 20;
const FIELDS: [(&str, &str); 6] = [
    ("Name", "web server"),
    ("Host", "box.example.com or 10.0.0.5"),
    ("Port", "22"),
    ("User", "optional"),
    ("Key", "ssh.<name> in your vault (optional)"),
    ("Command", "optional: runs when you connect"),
];

struct TrustAsk {
    host: String,
    port: u16,
    keys: Vec<HostKey>,
    /// The host already has a different key on record.
    changed: bool,
    /// Connect to this saved connection once the key is trusted.
    then: Option<String>,
}

enum CMode {
    List,
    /// The form; `Some(name)` while editing that connection.
    Form(Option<String>),
    Delete(String),
    Trust(TrustAsk),
    GenKey,
    /// A background call is running (`what` is shown).
    Wait,
}

enum Done {
    Probe(Result<Vec<HostKey>, String>, Option<String>),
    Run(Result<String, String>),
}

#[derive(Default)]
struct Shared {
    buf: String,
    streamed: bool,
    done: Option<Done>,
}

pub struct Conns {
    sel: usize,
    mode: CMode,
    fields: Vec<TextField>,
    focus: usize,
    key_name: TextField,
    status: Option<(String, bool)>,
    shared: Arc<Mutex<Shared>>,
    waiting: String,
    /// Lines for the Terminal tab's log.
    pub out: Vec<(String, [u8; 4])>,
    /// A public key just made, shown in the panel until something else is chosen.
    fresh_pub: Option<(String, String)>,
}

impl Conns {
    pub fn new() -> Conns {
        Conns {
            sel: 0,
            mode: CMode::List,
            fields: FIELDS.iter().map(|(_, ph)| TextField::new(ph, false)).collect(),
            focus: 0,
            key_name: TextField::new("ssh.deploy", false),
            status: None,
            shared: Arc::new(Mutex::new(Shared::default())),
            waiting: String::new(),
            out: Vec::new(),
            fresh_pub: None,
        }
    }

    fn env(cx: &AppCx) -> Env {
        Env::for_services(&cx.services, &cx.session.user)
    }

    fn list(cx: &AppCx) -> Vec<Connection> {
        Self::env(cx).connections().list()
    }

    #[cfg(test)]
    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    /// The host-key question on screen: host, port, the fingerprints, whether the key differs from the trusted one.
    #[cfg(test)]
    pub fn trust_ask(&self) -> Option<(String, u16, Vec<String>, bool)> {
        match &self.mode {
            CMode::Trust(a) => Some((a.host.clone(), a.port, ssh::describe_keys(&a.keys), a.changed)),
            _ => None,
        }
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self.mode, CMode::Wait)
    }

    pub fn in_form(&self) -> bool {
        !matches!(self.mode, CMode::List)
    }

    fn can_connect(cx: &AppCx) -> Result<(), String> {
        if cx.session.policy.cli != CliLevel::Full {
            return Err("Connecting needs a full terminal; ask an administrator.".into());
        }
        Ok(())
    }

    fn open_form(&mut self, c: Option<&Connection>) {
        let vals: [String; 6] = match c {
            Some(c) => [c.name.clone(), c.host.clone(), c.port.to_string(), c.user.clone(), c.key.clone(), c.command.clone()],
            None => Default::default(),
        };
        for (f, v) in self.fields.iter_mut().zip(vals.iter()) {
            f.set(v);
        }
        self.focus = 0;
        self.mode = CMode::Form(c.map(|c| c.name.clone()));
    }

    fn form_connection(&self) -> Result<Connection, String> {
        let v = |i: usize| self.fields[i].value.trim().to_string();
        let port = if v(2).is_empty() { 22 } else { v(2).parse::<u16>().map_err(|_| "The port is a number from 1 to 65535.".to_string())? };
        Ok(Connection { name: v(0), host: v(1), port, user: v(3), key: v(4), command: v(5) })
    }

    fn start_wait(&mut self, what: &str) -> Arc<Mutex<Shared>> {
        self.shared = Arc::new(Mutex::new(Shared::default()));
        self.waiting = what.to_string();
        self.mode = CMode::Wait;
        self.shared.clone()
    }

    /// Fetch the host's key (a thread; the answer arrives with [`Conns::tick`]).
    fn start_probe(&mut self, cx: &AppCx, host: &str, port: u16, then: Option<String>) {
        if let Err(e) = Self::can_connect(cx) {
            self.status = Some((e, false));
            return;
        }
        if !cx.session.policy.allows_host(host) {
            self.status = Some((format!("Your account may not reach {host}."), false));
            return;
        }
        let env = Self::env(cx);
        let shared = self.start_wait(&format!("Reading the host key of {host}:{port}…"));
        let host = host.to_string();
        std::thread::spawn(move || {
            let r = env.client().ok_or_else(|| format!("No SSH client was found on this machine. {}", ssh::install_hint(std::env::consts::OS))).and_then(|c| ssh::probe(&c, &host, port));
            shared.lock().unwrap().done = Some(Done::Probe(r, then));
        });
    }

    fn connect(&mut self, cx: &AppCx, c: &Connection) {
        if let Err(e) = Self::can_connect(cx) {
            self.status = Some((e, false));
            return;
        }
        if !cx.session.policy.allows_host(&c.host) {
            self.status = Some((format!("Your account may not reach {}.", c.host), false));
            return;
        }
        let env = Self::env(cx);
        if env.known_hosts().for_host(&c.host, c.port).is_empty() {
            self.start_probe(cx, &c.host, c.port, Some(c.name.clone()));
            return;
        }
        cx.audit.record(&cx.session.user, "ssh.connect", &c.name);
        let args = c.args();
        let shared = self.start_wait(&format!("Connected to {} … running {} (up to 2 minutes)", c.name, if c.command.is_empty() { "a quick check" } else { "the default command" }));
        self.out.push((format!("$ ssh {}", iyakki_agent::exec::command_line("", &args).trim()), OK));
        let sink_shared = shared.clone();
        let sink: iyakki_agent::exec::Sink = Arc::new(move |t: &str| {
            let mut s = sink_shared.lock().unwrap();
            s.streamed = true;
            s.buf.push_str(t);
        });
        let cwd = std::env::temp_dir();
        std::thread::spawn(move || {
            let r = ssh::run(&env, Mode::Person, "ssh", &args, &cwd, 120, &|_, _| Err("this connection names no local files".into()), Some(sink));
            shared.lock().unwrap().done = Some(Done::Run(r));
        });
    }

    /// Moves what the background call produced to the Terminal tab and acts on its end. Called on every tick.
    /// Runs a terminal line (`ssh`, `scp`, `sftp`, `sshtrust`) on a thread; its output streams into the Terminal tab. `false` when
    /// another call is still running.
    pub fn start_call(&mut self, call: iyakki_agent::cli::SshCall) -> bool {
        if self.is_waiting() {
            return false;
        }
        let shared = self.start_wait(&format!("{} is running… (up to 2 minutes)", call.command()));
        let sink_shared = shared.clone();
        let sink: iyakki_agent::exec::Sink = Arc::new(move |t: &str| {
            let mut s = sink_shared.lock().unwrap();
            s.streamed = true;
            s.buf.push_str(t);
        });
        std::thread::spawn(move || {
            let r = call.run(Some(sink));
            shared.lock().unwrap().done = Some(Done::Run(r));
        });
        true
    }

    pub fn tick(&mut self, cx: &AppCx) {
        let (text, done, streamed) = {
            let mut s = self.shared.lock().unwrap();
            // whole lines now, the rest when the call ends
            let cut = if s.done.is_some() { s.buf.len() } else { s.buf.rfind('\n').map_or(0, |i| i + 1) };
            let text: String = s.buf.drain(..cut).collect();
            (text, s.done.take(), s.streamed)
        };
        for l in text.lines() {
            self.out.push((l.to_string(), TEXT));
        }
        let Some(done) = done else { return };
        match done {
            Done::Probe(Err(e), _) => {
                self.mode = CMode::List;
                self.status = Some((e, false));
            }
            Done::Probe(Ok(keys), then) => {
                let host_port = keys.first().map(|k| k.pattern.clone()).unwrap_or_default();
                let (host, port) = parse_pattern(&host_port);
                let known = Self::env(cx).known_hosts().for_host(&host, port);
                let changed = !known.is_empty() && !keys.iter().any(|k| known.contains(k));
                self.mode = CMode::Trust(TrustAsk { host, port, keys, changed, then });
                self.status = None;
            }
            Done::Run(r) => {
                self.mode = CMode::List;
                match r {
                    Ok(t) => {
                        if !streamed {
                            t.lines().for_each(|l| self.out.push((l.to_string(), TEXT)));
                        }
                        self.status = Some(("Finished. The output is in the Terminal tab.".into(), true));
                    }
                    Err(e) => {
                        let first = if streamed && (e.starts_with("exit status") || e.starts_with("stopped after")) { e.lines().next().unwrap_or("").to_string() } else { e.clone() };
                        first.lines().for_each(|l| self.out.push((l.to_string(), ERR)));
                        self.status = Some((first.lines().next().unwrap_or("failed").to_string(), false));
                    }
                }
            }
        }
    }

    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let list = Self::list(cx);
        let n = list.len();
        self.sel = self.sel.min(n.saturating_sub(1));
        match &mut self.mode {
            CMode::Wait => return Flow::Stay,
            CMode::Form(editing) => {
                let editing = editing.clone();
                match ev {
                    Event::Key(Key::Esc) => self.mode = CMode::List,
                    Event::Tab(SAVE) => self.save(cx, editing),
                    Event::Key(Key::Enter) if self.focus == FIELDS.len() - 1 => self.save(cx, editing),
                    Event::Key(Key::Tab) | Event::Key(Key::Down) | Event::Key(Key::Enter) => self.focus = (self.focus + 1) % FIELDS.len(),
                    Event::Key(Key::Up) => self.focus = (self.focus + FIELDS.len() - 1) % FIELDS.len(),
                    Event::Paste(t) => {
                        self.fields[self.focus].paste(t);
                    }
                    Event::FocusAt(id, x) if *id < FIELDS.len() => {
                        self.focus = *id;
                        self.fields[*id].click(*x);
                    }
                    Event::Key(k) => {
                        self.fields[self.focus].handle(k);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            CMode::Delete(name) => {
                let name = name.clone();
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = CMode::List;
                    match Self::env(cx).connections().remove(&name) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "ssh.connection.remove", &name);
                            self.status = Some((format!("Removed {name}. Its host key and vault key were left alone."), true));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = CMode::List;
                }
                return Flow::Stay;
            }
            CMode::Trust(ask) => {
                if matches!(ev, Event::Key(Key::Char('y'))) {
                    let (host, port, keys, then) = (ask.host.clone(), ask.port, ask.keys.clone(), ask.then.clone());
                    self.mode = CMode::List;
                    match Self::env(cx).known_hosts().trust(&host, port, &keys) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "ssh.trust", &format!("{host}:{port}"));
                            self.status = Some((format!("Trusted {host}:{port}."), true));
                            if let Some(c) = then.and_then(|n| list.iter().find(|c| c.name == n).cloned()) {
                                self.connect(cx, &c);
                            }
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = CMode::List;
                    self.status = Some(("Nothing was trusted.".into(), true));
                }
                return Flow::Stay;
            }
            CMode::GenKey => {
                match ev {
                    Event::Key(Key::Esc) => self.mode = CMode::List,
                    Event::Key(Key::Enter) => {
                        let name = self.key_name.value.trim().to_string();
                        self.mode = CMode::List;
                        let env = Self::env(cx);
                        let (svc, user) = (cx.services.clone(), cx.session.user.clone());
                        match ssh::generate_key(&env, &name, &format!("{user}@iyakki"), &move |n, v| svc.user_set(&user, n, v).map_err(|e| format!("could not store the key in your vault: {e}"))) {
                            Ok(public) => {
                                cx.audit.record(&cx.session.user, "ssh.keygen", &name);
                                self.fresh_pub = Some((name.clone(), public));
                                self.status = Some((format!("Made {name} in your vault. Copy the public key below to the server's authorized_keys."), true));
                            }
                            Err(e) => self.status = Some((e, false)),
                        }
                    }
                    Event::Paste(t) => {
                        self.key_name.paste(t);
                    }
                    Event::FocusAt(_, x) => self.key_name.click(*x),
                    Event::Key(k) => {
                        self.key_name.handle(k);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            CMode::List => {}
        }
        let cur = list.get(self.sel).cloned();
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Row(i) => self.sel = (*i).min(n.saturating_sub(1)),
            Event::Key(Key::Char('n')) => {
                self.status = None;
                self.open_form(None);
            }
            Event::Key(Key::Char('e')) => match &cur {
                Some(c) => self.open_form(Some(c)),
                None => self.status = Some(("There is no connection to edit; press N to add one.".into(), false)),
            },
            Event::Key(Key::Char('x')) | Event::Key(Key::Delete) => match &cur {
                Some(c) => self.mode = CMode::Delete(c.name.clone()),
                None => self.status = Some(("There is no connection to remove.".into(), false)),
            },
            Event::Key(Key::Char('c')) | Event::Key(Key::Enter) => match &cur {
                Some(c) => self.connect(cx, c),
                None => self.status = Some(("There is no connection yet; press N to add one.".into(), false)),
            },
            Event::Key(Key::Char('t')) => match &cur {
                Some(c) => self.start_probe(cx, &c.host.clone(), c.port, None),
                None => self.status = Some(("Choose a connection first.".into(), false)),
            },
            Event::Key(Key::Char('f')) => match &cur {
                Some(c) => match Self::env(cx).known_hosts().forget(&c.host, c.port) {
                    Ok(0) => self.status = Some((format!("No host key was trusted for {}.", c.host), false)),
                    Ok(_) => {
                        cx.audit.record(&cx.session.user, "ssh.forget", &format!("{}:{}", c.host, c.port));
                        self.status = Some((format!("Forgot the host key of {}. Trust it again before connecting.", c.host), true));
                    }
                    Err(e) => self.status = Some((e, false)),
                },
                None => self.status = Some(("Choose a connection first.".into(), false)),
            },
            Event::Key(Key::Char('g')) => {
                self.key_name.set(&cur.as_ref().map(|c| format!("ssh.{}", c.name.to_ascii_lowercase().replace(' ', "-"))).unwrap_or_else(|| "ssh.key".into()));
                self.mode = CMode::GenKey;
            }
            _ => {}
        }
        Flow::Stay
    }

    fn save(&mut self, cx: &AppCx, editing: Option<String>) {
        match self.form_connection().and_then(|c| {
            let name = c.name.clone();
            Self::env(cx).connections().save(c, editing.as_deref()).map(|_| name)
        }) {
            Ok(name) => {
                cx.audit.record(&cx.session.user, "ssh.connection.save", &name);
                self.mode = CMode::List;
                self.sel = Self::list(cx).iter().position(|c| c.name == name).unwrap_or(0);
                self.status = Some((format!("Saved {name}."), true));
            }
            Err(e) => self.status = Some((e, false)),
        }
    }

    pub fn hint(&self) -> &'static str {
        match self.mode {
            CMode::Form(_) => "Tab next field · Enter next / save · Esc cancel",
            CMode::Trust(_) => "Y trust this key · any other key cancels",
            CMode::GenKey => "Enter make the key · Esc cancel",
            _ => "↑↓ choose · Enter or C connect · N new · E edit · X remove · T trust host key · F forget it · G make a key · Tab next tab · Esc back",
        }
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 4.0 * s;
        let busy = matches!(self.mode, CMode::Wait);
        let line = if busy { Some((self.waiting.clone(), true)) } else { self.status.clone() };
        if let Some((m, ok)) = &line {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        }
        let env = Self::env(cx);
        let list = env.connections().list();
        self.sel = self.sel.min(list.len().saturating_sub(1));
        let client = cx.services.clis.system_status("ssh");
        let list_w = (w * 0.34).clamp(220.0 * s, 360.0 * s);
        let (rh, gap) = (46.0 * s, 5.0 * s);
        let bottom = area.y + area.h;
        let slots = (((bottom - y) / (rh + gap)).floor() as usize).max(1);
        let top = if self.sel >= slots { self.sel + 1 - slots } else { 0 };
        if list.is_empty() {
            let r = R::new(x, y, list_w, rh * 2.0);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, false, false, ACCENT);
            for (k, l) in g.wrap(13.0 * s, "No saved connections yet. Press N to add one.", Face::Sans, r.w - 24.0 * s).into_iter().take(3).enumerate() {
                g.text(r.x + 12.0 * s, r.y + 8.0 * s + k as f32 * 18.0 * s, 13.0 * s, MUTED, &l, Face::Sans);
            }
        }
        for (k, (i, c)) in list.iter().enumerate().skip(top).take(slots).enumerate() {
            let r = R::new(x, y + k as f32 * (rh + gap), list_w, rh);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel, false, ACCENT);
            g.hit(r, Event::Row(i), &c.name);
            let trusted = !env.known_hosts().for_host(&c.host, c.port).is_empty();
            g.rect(r.x, r.y + 9.0 * s, 4.0 * s, r.h - 18.0 * s, if trusted { OK } else { DIM });
            let nm = g.fit(15.0 * s, &c.name, Face::Bold, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &nm, Face::Bold);
            let sub = g.fit(12.0 * s, &format!("{}:{}", c.target(), c.port), Face::Sans, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 26.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
        }
        let (px, pw) = (x + list_w + 18.0 * s, w - list_w - 18.0 * s);
        if pw >= 220.0 * s {
            let ph = bottom - y;
            crate::fx::panel(g, R::new(px, y, pw, ph), s, ACCENT);
            self.draw_panel(g, R::new(px, y, pw, ph), s, cx, &env, list.get(self.sel), client.as_ref(), busy);
        }
        match &self.mode {
            CMode::Form(editing) => self.draw_form(g, area, s, editing.is_some(), tick),
            CMode::Delete(n) => super::super::widgets::render_confirm(g, area, s, &format!("Remove the connection {n}? Its host key and vault key stay."), "Remove"),
            CMode::Trust(ask) => draw_trust(g, area, s, ask),
            CMode::GenKey => self.draw_genkey(g, area, s, tick),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_panel(&self, g: &mut Gfx, p: R, s: f32, cx: &AppCx, env: &Env, cur: Option<&Connection>, client: Option<&iyakki_agent::clis::SystemStatus>, busy: bool) {
        let mut ty = p.y + 16.0 * s;
        // how many rows of buttons the panel's width needs, so the text above them stops in time
        let size = 15.0 * s;
        let items: [(&str, char, [u8; 4]); 7] = [("Connect (c)", 'c', ACCENT), ("New (n)", 'n', OK), ("Edit (e)", 'e', TEXT), ("Remove (x)", 'x', ERR), ("Trust key (t)", 't', TEXT), ("Forget key (f)", 'f', TEXT), ("Make key (g)", 'g', TEXT)];
        let (mut rows, mut used) = (1.0f32, 20.0 * s);
        for (label, _, _) in items {
            let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
            if used + bw > p.w - 4.0 * s && used > 20.0 * s {
                rows += 1.0;
                used = 20.0 * s;
            }
            used += bw + 10.0 * s;
        }
        let by_rows = rows;
        let limit = p.y + p.h - (by_rows * 38.0 + 22.0) * s;
        g.text(p.x + 20.0 * s, ty, 22.0 * s, TEXT, cur.map_or("Connections", |c| c.name.as_str()), Face::Bold);
        ty += 34.0 * s;
        let line = |g: &mut Gfx, ty: &mut f32, k: &str, v: &str, col: [u8; 4]| {
            if *ty > limit {
                return;
            }
            g.text(p.x + 20.0 * s, *ty, 12.0 * s, DIM, k, Face::Bold);
            for l in g.wrap(13.0 * s, v, Face::Sans, p.w - 190.0 * s).into_iter().take(4) {
                if *ty > limit {
                    break;
                }
                g.text(p.x + 170.0 * s, *ty, 13.0 * s, col, &l, Face::Sans);
                *ty += 18.0 * s;
            }
            *ty += 4.0 * s;
        };
        match client {
            Some(c) if c.found => line(g, &mut ty, "SSH CLIENT", &format!("{} ({})", c.label, c.path), OK),
            Some(c) => line(g, &mut ty, "SSH CLIENT", &format!("none found. {}", c.how_to), WARN),
            None => {}
        }
        if let Some(c) = cur {
            line(g, &mut ty, "HOST", &format!("{} port {}", c.host, c.port), TEXT);
            line(g, &mut ty, "USER", if c.user.is_empty() { "(the client's default)" } else { &c.user }, TEXT);
            let keys = env.known_hosts().for_host(&c.host, c.port);
            if keys.is_empty() {
                line(g, &mut ty, "HOST KEY", "not trusted yet: Connect (or T) shows the fingerprint to confirm first", WARN);
            } else {
                line(g, &mut ty, "HOST KEY", &format!("trusted: {}", ssh::describe_keys(&keys).join("; ")), OK);
            }
            if c.key.is_empty() {
                line(g, &mut ty, "KEY", "none (the client's own default keys are not used here)", MUTED);
            } else {
                let there = cx.services.user_names(&cx.session.user).map(|n| n.contains(&c.key));
                let (t, col) = match there {
                    Ok(true) => (format!("{} (in your vault)", c.key), OK),
                    Ok(false) => (format!("{} is not in your vault: press G to make it, or store it in Settings > Vault", c.key), WARN),
                    Err(_) => (format!("{} (your vault is locked)", c.key), WARN),
                };
                line(g, &mut ty, "KEY", &t, col);
            }
            line(g, &mut ty, "COMMAND", if c.command.is_empty() { "(a quick check: true)" } else { &c.command }, TEXT);
            let pubkey = self.fresh_pub.as_ref().filter(|(n, _)| *n == c.key).map(|(_, k)| k.clone()).or_else(|| env.public_key(&c.key));
            if let Some(k) = pubkey {
                line(g, &mut ty, "PUBLIC KEY", &k, TEXT);
            }
        } else {
            line(g, &mut ty, "ABOUT", "Saved hosts for ssh. Host keys are kept by IYAKKI and checked on every connection; private keys stay in your vault. Press N to add a host.", MUTED);
        }
        // buttons, two rows at most
        let mut bx = p.x + 20.0 * s;
        let mut by = p.y + p.h - (by_rows * 38.0 + 8.0) * s;
        for (label, ch, col) in items {
            let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
            if bx + bw > p.x + p.w - 4.0 * s && bx > p.x + 20.0 * s {
                bx = p.x + 20.0 * s;
                by += 38.0 * s;
            }
            let needs_cur = !matches!(ch, 'n');
            if busy || (needs_cur && cur.is_none()) {
                g.button_off(bx, by, size, label);
            } else {
                g.button(bx, by, size, label, col, Event::Key(Key::Char(ch)));
            }
            bx += bw + 10.0 * s;
        }
    }

    fn draw_form(&mut self, g: &mut Gfx, area: R, s: f32, editing: bool, tick: u64) {
        let p = crate::fx::modal(g, area, s, 660.0 * s, 480.0 * s, ACCENT, false);
        let y0 = crate::fx::modal_title(g, p, s, if editing { "Edit connection" } else { "New connection" }, ACCENT);
        let foot = 64.0 * s;
        let rh = ((p.y + p.h - foot - y0) / FIELDS.len() as f32).clamp(30.0 * s, 46.0 * s);
        let (lw, fx) = (96.0 * s, p.x + 26.0 * s + 100.0 * s);
        let fw = p.x + p.w - 26.0 * s - fx;
        for (i, (label, _)) in FIELDS.iter().enumerate() {
            let y = y0 + i as f32 * rh;
            let fh = (rh - 6.0 * s).max(26.0 * s);
            g.text(p.x + 26.0 * s, y + (fh - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, if i == self.focus { TEXT } else { MUTED }, label, Face::Bold);
            let _ = lw;
            self.fields[i].render(g, R::new(fx, y, fw, fh), i == self.focus, s, super::blink(tick), Face::Sans, i);
        }
        if let Some((m, _)) = &self.status {
            let m = g.fit(13.0 * s, m, Face::Sans, p.w - 52.0 * s);
            g.text(p.x + 26.0 * s, p.y + p.h - 58.0 * s - 18.0 * s, 13.0 * s, ERR, &m, Face::Sans);
        }
        let by = p.y + p.h - 50.0 * s;
        let w1 = g.button(p.x + 26.0 * s, by, 15.0 * s, "Save", ACCENT, Event::Tab(SAVE));
        g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
    }

    fn draw_genkey(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let p = crate::fx::modal(g, area, s, 560.0 * s, 230.0 * s, ACCENT, false);
        let y0 = crate::fx::modal_title(g, p, s, "Make an SSH key", ACCENT);
        for (k, l) in g.wrap(13.0 * s, "A new ed25519 key is made and kept in your vault under this name (ssh.<name>). The private half never goes into a folder; you get the public half to copy to the server.", Face::Sans, p.w - 52.0 * s).into_iter().take(3).enumerate() {
            g.text(p.x + 26.0 * s, y0 + k as f32 * 18.0 * s, 13.0 * s, MUTED, &l, Face::Sans);
        }
        self.key_name.render(g, R::new(p.x + 26.0 * s, p.y + p.h - 112.0 * s, p.w - 52.0 * s, 36.0 * s), true, s, super::blink(tick), Face::Sans, 0);
        let by = p.y + p.h - 58.0 * s;
        let w1 = g.button(p.x + 26.0 * s, by, 15.0 * s, "Make key", ACCENT, Event::Key(Key::Enter));
        g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
    }
}

fn draw_trust(g: &mut Gfx, area: R, s: f32, ask: &TrustAsk) {
    let p = crate::fx::modal(g, area, s, 680.0 * s, 360.0 * s, if ask.changed { ERR } else { ACCENT }, ask.changed);
    let where_ = if ask.port == 22 { ask.host.clone() } else { format!("{}:{}", ask.host, ask.port) };
    let y0 = crate::fx::modal_title(g, p, s, &if ask.changed { format!("{where_} presents a DIFFERENT key") } else { format!("Trust the host key of {where_}?") }, if ask.changed { ERR } else { ACCENT });
    let mut y = y0;
    let intro = if ask.changed { "This is NOT the key you trusted before. That can mean the server was reinstalled, or that someone is intercepting the connection. Trust it only if you know why it changed." } else { "This machine has not been seen before. Compare the fingerprint with the one the server's owner gave you (on the server: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub)." };
    for l in g.wrap(13.0 * s, intro, Face::Sans, p.w - 52.0 * s).into_iter().take(4) {
        g.text(p.x + 26.0 * s, y, 13.0 * s, MUTED, &l, Face::Sans);
        y += 18.0 * s;
    }
    y += 6.0 * s;
    for k in ask.keys.iter().take(3) {
        let t = g.fit(14.0 * s, &format!("{}  {}", k.kind, k.fingerprint()), Face::Mono, p.w - 52.0 * s);
        g.text(p.x + 26.0 * s, y, 14.0 * s, TEXT, &t, Face::Mono);
        y += 22.0 * s;
    }
    g.text(p.x + 26.0 * s, p.y + p.h - 92.0 * s, 13.0 * s, MUTED, "Press y to trust this key, or any other key to cancel.", Face::Sans);
    let by = p.y + p.h - 58.0 * s;
    let w1 = g.button(p.x + 26.0 * s, by, 15.0 * s, "Trust this key", if ask.changed { ERR } else { OK }, Event::Key(Key::Char('y')));
    g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}

/// `host` or `[host]:port` back into its parts.
fn parse_pattern(p: &str) -> (String, u16) {
    if let Some(rest) = p.strip_prefix('[') {
        if let Some((h, port)) = rest.split_once("]:") {
            return (h.to_string(), port.parse().unwrap_or(22));
        }
    }
    (p.to_string(), 22)
}
