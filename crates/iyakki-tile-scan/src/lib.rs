//! NET SCAN: find what runs on the local network and use it. Model servers (Ollama and others) can be added to
//! Settings → Models in one step, with an API key going into the system vault when the server needs one; cameras and
//! other services show their address to copy. Scans stay on private addresses, ask only public questions and can be
//! stopped; every scan is written to the audit log. The tile asks a `ScanPort` for everything outside its own drawing (the scanner, the
//! subnets, the host policy, the device watch, the audit line, the settings writes); it never names the scanner crate.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use iyakki_tile_api::scan::ScanPort;
use iyakki_tile_api::scan::{Event as ScanEvent, HostInfo, Kind, Neighbor, ScanFeed, ScanOpts, Service, Subnet};
use iyakki_uikit::blink::blink;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::tooltip::{fit_keep_ip, Tooltip};
use iyakki_uikit::ui::{clipboard, Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{Form, FormEvent};
use iyakki_uikit::fx;

#[cfg(test)]
mod tests;

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(120, 220, 140);
const WARN: [u8; 4] = rgb(255, 196, 94);

pub struct Network {
    pub label: String,
    pub targets: Vec<Ipv4Addr>,
}

pub enum Mode {
    Browse,
    Form(Form, FormKind),
}

pub enum FormKind {
    Target,
    RenameDevice(String),
    AddModel { ip: Ipv4Addr, svc: Box<Service>, models: Vec<String> },
}

const FILTERS: [&str; 6] = ["All", "Model servers", "Cameras", "Media", "Devices", "Risks"];

fn kind_color(k: Kind) -> [u8; 4] {
    match k {
        Kind::Ollama | Kind::LlmServer => OK,
        Kind::Camera => rgb(255, 110, 165),
        Kind::Media => rgb(120, 170, 255),
        Kind::Printer => WARN,
        Kind::SmartHome => rgb(86, 204, 196),
        Kind::Data => rgb(180, 140, 255),
        Kind::Device | Kind::Platform => rgb(150, 170, 210),
        Kind::Access => MUTED,
        Kind::Web | Kind::Other => DIM,
    }
}

fn matches(f: usize, h: &HostInfo) -> bool {
    match f {
        1 => h.services.iter().any(|s| s.kind.is_model_server()),
        2 => h.has(Kind::Camera),
        3 => h.has(Kind::Media),
        4 => h.services.iter().any(|s| matches!(s.kind, Kind::Device | Kind::Printer | Kind::SmartHome | Kind::Platform)),
        5 => h.risky(),
        _ => true,
    }
}

pub struct ScanTile {
    pub nets: Vec<Network>,
    pub net_sel: usize,
    pub hosts: BTreeMap<Ipv4Addr, HostInfo>,
    pub filter: usize,
    pub sel: usize,
    pub svc_sel: usize,
    pub running: bool,
    pub cancel: Arc<AtomicBool>,
    pub feed: Option<Box<dyn ScanFeed>>,
    pub progress: (usize, usize),
    pub summary: Option<(usize, usize, bool)>,
    pub mode: Mode,
    pub status: Option<(String, bool)>,
    /// Ports to try instead of the default list (tests).
    pub ports: Option<Vec<u16>>,
    /// The list of devices seen on the network (new-device alerts) instead of the scan's machines.
    pub devices: bool,
    pub dev_sel: usize,
    /// The whole text of a row whose lines were cut to the width.
    pub tip: Tooltip,
}

impl ScanTile {
    pub fn new(port: &dyn ScanPort) -> ScanTile {
        let mut nets: Vec<Network> = port.subnets().into_iter().map(|s: Subnet| Network { label: format!("{}  {}  ({} addresses)", s.iface, s.cidr(), port.hosts_of(&s).len()), targets: port.hosts_of(&s) }).collect();
        nets.push(Network { label: "This machine (127.0.0.1)".into(), targets: vec![Ipv4Addr::LOCALHOST] });
        ScanTile { nets, net_sel: 0, hosts: BTreeMap::new(), filter: 0, sel: 0, svc_sel: 0, running: false, cancel: Arc::new(AtomicBool::new(false)), feed: None, progress: (0, 0), summary: None, mode: Mode::Browse, status: None, ports: None, devices: false, dev_sel: 0, tip: Tooltip::new() }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn hosts(&self) -> Vec<&HostInfo> {
        self.hosts.values().collect()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn shown(&self) -> Vec<&HostInfo> {
        self.hosts.values().filter(|h| matches(self.filter, h)).collect()
    }

    pub fn current(&self) -> Option<&HostInfo> {
        self.shown().get(self.sel).copied()
    }

    pub fn start(&mut self, port: &dyn ScanPort) {
        if self.running {
            return;
        }
        let Some(net) = self.nets.get(self.net_sel) else { return };
        // the account's host policy decides which addresses may be touched
        let blocked = net.targets.iter().find(|ip| !port.allows_host(&ip.to_string()));
        if let Some(ip) = blocked {
            self.status = Some((format!("Your account may not scan {ip}."), false));
            return;
        }
        let label = net.label.clone();
        let (targets, cancel) = (net.targets.clone(), Arc::new(AtomicBool::new(false)));
        let opts = ScanOpts { ports: self.ports.clone() };
        self.cancel = cancel.clone();
        let feed = match port.start_scan(targets, opts, cancel) {
            Ok(f) => f,
            Err(e) => {
                self.status = Some((format!("The scan did not start: {e}"), false));
                return;
            }
        };
        port.audit("netscan.start", &label);
        self.feed = Some(feed);
        self.running = true;
        self.hosts.clear();
        self.sel = 0;
        self.svc_sel = 0;
        self.progress = (0, 1);
        self.summary = None;
        self.status = None;
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&mut self, port: &dyn ScanPort) {
        let Some(feed) = &mut self.feed else { return };
        let mut finished = None;
        while let Some(ev) = feed.next() {
            match ev {
                ScanEvent::Started { hosts, ports } => self.progress = (0, (hosts * ports).max(1)),
                ScanEvent::Progress { done, total } => self.progress = (done, total.max(1)),
                ScanEvent::Host(h) => {
                    self.hosts.insert(h.ip, h);
                }
                ScanEvent::Finished { hosts, services, cancelled } => finished = Some((hosts, services, cancelled)),
            }
        }
        if let Some((hosts, services, cancelled)) = finished {
            self.running = false;
            self.feed = None;
            self.summary = Some((hosts, services, cancelled));
            // what the scan found also tells the new-device watch who is on the network
            let seen: Vec<Neighbor> = self.hosts.values().filter_map(|h| Some(Neighbor { ip: h.ip.to_string(), mac: h.mac.clone()?.to_lowercase(), iface: "scan".into() })).collect();
            if port.admin() {
                port.devwatch().observe_scan(&seen, port.now());
            }
            port.audit("netscan.done", &format!("{hosts} hosts, {services} services{}", if cancelled { ", stopped" } else { "" }));
        }
    }

    pub fn handle_devices(&mut self, ev: &Event, port: &dyn ScanPort) -> Answer {
        let admin = port.admin();
        let list = port.devwatch().list();
        let cur = list.get(self.dev_sel.min(list.len().saturating_sub(1))).cloned();
        let deny = |this: &mut Self| this.status = Some(("Only administrators can change the device list.".into(), false));
        match ev {
            Event::Tab(111) | Event::Key(Key::Esc) | Event::Key(Key::Char('d')) => self.devices = false,
            Event::Row(i) => self.dev_sel = (*i).min(list.len().saturating_sub(1)),
            Event::Key(Key::Up) => self.dev_sel = self.dev_sel.saturating_sub(1),
            Event::Key(Key::Down) => self.dev_sel = (self.dev_sel + 1).min(list.len().saturating_sub(1)),
            Event::Tab(112) | Event::Key(Key::Char('t')) => match (admin, cur) {
                (false, _) => deny(self),
                (true, Some(d)) => {
                    port.devwatch().trust(&d.mac);
                    port.audit("netsec.trust", &d.mac);
                    self.status = Some((format!("{} is trusted now.", d.mac), true));
                }
                _ => {}
            },
            Event::Tab(113) | Event::Key(Key::Char('T')) => {
                if !admin {
                    deny(self);
                } else {
                    let n = port.devwatch().trust_all();
                    port.audit("netsec.trust_all", &n.to_string());
                    self.status = Some((format!("{n} device(s) trusted."), true));
                }
            }
            Event::Key(Key::Char('r')) => match (admin, cur) {
                (false, _) => deny(self),
                (true, Some(d)) => self.mode = Mode::Form(Form::new("Name this device").text("Name", &d.name, "Printer, Anna's phone…"), FormKind::RenameDevice(d.mac)),
                _ => {}
            },
            Event::Tab(114) | Event::Key(Key::Char('x')) => match (admin, cur) {
                (false, _) => deny(self),
                (true, Some(d)) => {
                    port.devwatch().forget(&d.mac);
                    self.status = Some((format!("Forgot {}. If it comes back it is new again.", d.mac), true));
                }
                _ => {}
            },
            Event::Tab(115) | Event::Key(Key::Char('w')) => {
                if !admin {
                    deny(self);
                } else {
                    let on = !port.devwatch().enabled();
                    port.devwatch().set_enabled(on);
                    port.audit("netsec.watch", if on { "on" } else { "off" });
                    self.status = Some((if on { "Watching for new devices.".into() } else { "Not watching for new devices.".into() }, true));
                }
            }
            _ => {}
        }
        Answer::Stay
    }

    fn render_devices(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn ScanPort, y0: f32) {
        let (x, w) = (area.x + 24.0 * s, area.w - 48.0 * s);
        let list = port.devwatch().list();
        let on = port.devwatch().enabled();
        let mut y = y0;
        let untrusted = list.iter().filter(|d| !d.trusted).count();
        let head = if !on { "Watching is off (w turns it on).".to_string() } else if untrusted > 0 { format!("{untrusted} device(s) you have not seen before. Trust the ones you know (t), forget the rest (x).") } else { "Every device seen is trusted. A new one raises an alert.".to_string() };
        let head = g.fit(14.0 * s, &head, Face::Sans, w);
        g.text(x, y, 14.0 * s, if on && untrusted > 0 { WARN } else { MUTED }, &head, Face::Sans);
        y += 26.0 * s;
        let now = port.now();
        let rh = 52.0 * s;
        let fit = (((area.y + area.h - y - 50.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
        let sel = self.dev_sel.min(list.len().saturating_sub(1));
        let first = sel.saturating_sub(fit - 1);
        if list.is_empty() {
            g.text(x, y + 6.0 * s, 15.0 * s, DIM, "No devices seen yet. The list fills from this machine's own network neighbours, and from a scan.", Face::Sans);
        }
        let mut cut: Option<String> = None;
        for (i, d) in list.iter().enumerate().skip(first).take(fit) {
            let r = R::new(x, y, w, rh);
            let on_row = i == sel;
            fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, on_row, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), "device");
            g.rect(x, y + 10.0 * s, 4.0 * s, rh - 20.0 * s, if d.trusted { OK } else { WARN });
            let title = if d.name.is_empty() { d.mac.clone() } else { format!("{} · {}", d.name, d.mac) };
            let shown = g.fit(16.0 * s, &title, Face::Bold, w - 190.0 * s);
            g.text(x + 16.0 * s, y + 6.0 * s, 16.0 * s, TEXT, &shown, Face::Bold);
            let ago = |t: i64| human_duration((now - t).max(0) as u64);
            let line = format!("{} on {} · first seen {} ago · last seen {} ago", d.ip, d.iface, ago(d.first_seen), ago(d.last_seen));
            let line_shown = fit_keep_ip(g, 12.0 * s, &line, Face::Sans, w - 32.0 * s);
            g.text(x + 16.0 * s, y + 30.0 * s, 12.0 * s, MUTED, &line_shown, Face::Sans);
            if shown != title || line_shown != line {
                let full = format!("{title}\n{line}");
                self.tip.offer(g, r, on_row, &full);
                if on_row {
                    cut = Some(format!("{title} · {line}"));
                }
            }
            let (chip, col) = if d.trusted { ("trusted", OK) } else { ("NEW", WARN) };
            let cw = g.text_width(12.0 * s, chip, Face::Bold) + 17.0 * s;
            fx::chip(g, r.x + r.w - cw - 12.0 * s, y + 14.0 * s, s, chip, col, !d.trusted);
            y += rh + 6.0 * s;
        }
        let by = area.y + area.h - 40.0 * s;
        // the keyboard's way to the tooltip: the whole text of the selected device above the buttons when its lines were cut
        if let Some(full) = cut {
            let t = fit_keep_ip(g, 12.0 * s, &full, Face::Sans, w);
            g.text(x, by - 22.0 * s, 12.0 * s, TEXT, &t, Face::Sans);
        }
        let mut bx = x;
        for (label, col, ev) in [("Trust (t)", OK, Event::Tab(112)), ("Trust all (T)", MUTED, Event::Tab(113)), ("Name (r)", MUTED, Event::Key(Key::Char('r'))), ("Forget (x)", ERR, Event::Tab(114)), (if on { "Stop watching (w)" } else { "Watch (w)" }, MUTED, Event::Tab(115)), ("Back (d)", ACCENT, Event::Tab(111))] {
            bx += g.button(bx, by, 13.0 * s, label, col, ev) + 8.0 * s;
        }
    }

    fn selected_service(&self) -> Option<(Ipv4Addr, Service)> {
        let h = self.current()?;
        h.services.get(self.svc_sel).map(|s| (h.ip, s.clone()))
    }

    pub fn target_form() -> Form {
        Form::new("Scan this").text("Address, range or network", "", "192.168.1.0/24   ·   192.168.1.20   ·   192.168.1.10-50")
    }

    fn add_form(ip: Ipv4Addr, svc: &Service) -> Form {
        let id = provider_id(ip, svc);
        let base = base_url(ip, svc);
        let mut f = Form::new("Add to models").text("Provider name", &id, "lowercase letters, digits, - and _").text("Address", &base, "http://host:11434").secret("API key (only if the server asks)", "stored in the system vault, never in settings");
        let options: Vec<&str> = svc.models.iter().map(String::as_str).take(40).collect();
        if !options.is_empty() {
            f = f.choice("Model to use", &options, 0);
        } else {
            f = f.text("Model to use", "", "pick one later in Settings → Models");
        }
        f
    }

    pub fn open_add(&mut self, port: &dyn ScanPort) {
        let Some((ip, svc)) = self.selected_service() else { return };
        if !svc.kind.is_model_server() {
            self.status = Some(("Only model servers (Ollama and OpenAI-compatible ones) can be added to Models.".into(), false));
            return;
        }
        if !port.admin() {
            self.status = Some(("Only an administrator can add models. Copy the address with c and ask one.".into(), false));
            return;
        }
        let models = svc.models.clone();
        self.mode = Mode::Form(Self::add_form(ip, &svc), FormKind::AddModel { ip, svc: Box::new(svc), models });
    }

    fn copy_address(&mut self) {
        let Some((ip, svc)) = self.selected_service() else { return };
        let url = svc.url.clone().unwrap_or_else(|| format!("{ip}:{}", svc.port));
        clipboard::copy(&url);
        self.status = Some((format!("Copied {url}"), true));
    }
}

/// A Lemonade server found by the scan (it names itself in its health answer).
pub fn is_lemonade(svc: &Service) -> bool {
    svc.kind == Kind::LlmServer && svc.name.starts_with("Lemonade")
}

pub fn provider_id(ip: Ipv4Addr, svc: &Service) -> String {
    let base = if svc.kind == Kind::Ollama { "ollama" } else if is_lemonade(svc) { "lemonade" } else { "llm" };
    let dashed = ip.to_string().replace('.', "-");
    if svc.port == 11434 || svc.port == 1234 { format!("{base}-{dashed}") } else { format!("{base}-{dashed}-{}", svc.port) }
}

pub fn base_url(ip: Ipv4Addr, svc: &Service) -> String {
    let scheme = if svc.url.as_deref().is_some_and(|u| u.starts_with("https")) { "https" } else { "http" };
    match svc.kind {
        // OpenAI-compatible servers are reached through their /v1 address; Ollama's own address has no path
        Kind::LlmServer => svc.url.clone().unwrap_or_else(|| format!("{scheme}://{ip}:{}/v1", svc.port)),
        _ => format!("{scheme}://{ip}:{}", svc.port),
    }
}

impl ScanTile {
    pub fn title(&self) -> &'static str {
        "Net scan"
    }
    pub fn icon(&self) -> Icon {
        Icon::Scan
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        "s scan · x stop · n network · t type a target · ↑↓ machine · ← → service · a add model · c copy".into()
    }

    pub fn help(&self) -> Vec<String> {
        [
            "s or Enter · scan the chosen network      x · stop it",
            "n · the next network (this machine's networks, then this machine)",
            "t · scan an address, a range (192.168.1.10-50) or a network (192.168.1.0/24)",
            "Up / Down · choose a machine      Left / Right · choose one of its services",
            "f or 1-6 · filter: all, model servers, cameras, media, devices, risks",
            "a · add the chosen model server (Ollama or OpenAI-compatible) to Models; an API key goes into the vault (administrators)",
            "c · copy the chosen service's address (for a camera: its RTSP address)",
            "Scans stay on private addresses and ask only public questions; nothing is tried or changed. Every scan is audited.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn wants_redraw(&mut self) -> bool {
        self.tip.take_changed()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn ScanPort) -> Answer {
        self.tip.event(ev);
        if matches!(ev, Event::Tick) {
            self.poll(port);
            return Answer::Stay;
        }
        if let Mode::Form(form, kind) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    match kind {
                        FormKind::Target => {
                            let text = form.value(0);
                            match port.parse_targets(&text) {
                                Ok(t) => {
                                    self.nets.insert(0, Network { label: format!("typed: {text}  ({} addresses)", t.len()), targets: t });
                                    self.net_sel = 0;
                                    self.mode = Mode::Browse;
                                    self.start(port);
                                }
                                Err(e) => form.error = Some(e),
                            }
                        }
                        FormKind::RenameDevice(mac) => {
                            let ok = port.devwatch().rename(mac, &form.value(0));
                            self.status = Some((if ok { "Renamed.".into() } else { "That device is no longer listed.".into() }, ok));
                            self.mode = Mode::Browse;
                        }
                        FormKind::AddModel { ip, svc, models } => {
                            let (id, address, key) = (form.value(0), form.value(1), form.raw(2));
                            let model = if models.is_empty() { form.value(3) } else { models.get(form.choice_idx(3)).cloned().unwrap_or_default() };
                            match port.add_model_server(*ip, svc, &id, &address, &model, &key) {
                                Ok(msg) => {
                                    self.status = Some((msg, true));
                                    self.mode = Mode::Browse;
                                }
                                Err(e) => form.error = Some(e),
                            }
                        }
                    }
                }
                _ => {}
            }
            return Answer::Stay;
        }
        if self.devices {
            return self.handle_devices(ev, port);
        }
        match ev {
            Event::Tab(110) => {
                self.devices = true;
                return Answer::Stay;
            }
            Event::Row(i) => {
                self.sel = (*i).min(self.shown().len().saturating_sub(1));
                self.svc_sel = 0;
                return Answer::Stay;
            }
            Event::Tab(n) if *n < FILTERS.len() => {
                self.filter = *n;
                self.sel = 0;
                self.svc_sel = 0;
                return Answer::Stay;
            }
            Event::Tab(100) => {
                self.start(port);
                return Answer::Stay;
            }
            Event::Tab(101) => {
                self.stop();
                return Answer::Stay;
            }
            Event::Tab(102) => {
                self.open_add(port);
                return Answer::Stay;
            }
            Event::Scroll(d) => {
                self.sel = (self.sel as i32 - d).clamp_safe(0, self.shown().len().saturating_sub(1) as i32) as usize;
                self.svc_sel = 0;
                return Answer::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Answer::Stay };
        let n = self.shown().len();
        match k {
            Key::Esc => {
                if self.running {
                    self.stop();
                }
                return Answer::Back;
            }
            Key::Char('s') | Key::Enter => self.start(port),
            Key::Char('x') => self.stop(),
            Key::Char('n') if !self.running => {
                self.net_sel = (self.net_sel + 1) % self.nets.len().max(1);
            }
            Key::Char('t') if !self.running => self.mode = Mode::Form(Self::target_form(), FormKind::Target),
            Key::Up => {
                self.sel = self.sel.saturating_sub(1);
                self.svc_sel = 0;
            }
            Key::Down => {
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
                self.svc_sel = 0;
            }
            Key::Left => self.svc_sel = self.svc_sel.saturating_sub(1),
            Key::Right => {
                let m = self.current().map_or(0, |h| h.services.len());
                self.svc_sel = (self.svc_sel + 1).min(m.saturating_sub(1));
            }
            Key::Char('f') => {
                self.filter = (self.filter + 1) % FILTERS.len();
                self.sel = 0;
            }
            Key::Char(c @ '1'..='6') => {
                self.filter = (*c as u8 - b'1') as usize;
                self.sel = 0;
            }
            Key::Char('a') => self.open_add(port),
            Key::Char('d') => self.devices = true,
            Key::Char('c') => self.copy_address(),
            _ => {}
        }
        Answer::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn ScanPort, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // network and controls
        let label = self.nets.get(self.net_sel).map_or(String::new(), |n| n.label.clone());
        // a window too narrow for the buttons beside the network name gives them a row of their own under it
        let own_row = w < 500.0 * s;
        let bar_h = if own_row { 84.0 } else { 46.0 } * s;
        fx::glass(g, x, y, w, bar_h, 12.0 * s, ACCENT);
        // a narrow window has no room for the key hints beside the buttons: the word alone
        let caption = if !own_row && g.text_width(11.0 * s, "NETWORK  (n changes it, t types one)", Face::Bold) + 32.0 * s <= w - 330.0 * s { "NETWORK  (n changes it, t types one)" } else { "NETWORK" };
        g.text(x + 16.0 * s, y + 6.0 * s, 11.0 * s, DIM, caption, Face::Bold);
        let label = g.fit(16.0 * s, &label, Face::Bold, if own_row { w - 32.0 * s } else { w - 330.0 * s });
        g.text(x + 16.0 * s, y + 22.0 * s, 16.0 * s, TEXT, &label, Face::Bold);
        let new_devices = port.devwatch().untrusted_count();
        let dlabel = if new_devices > 0 { format!("Devices ({new_devices} new)") } else { "Devices (d)".to_string() };
        let dw = g.text_width(14.0 * s, &dlabel, Face::Bold) + 24.0 * s;
        let bx = x + w - 300.0 * s;
        let (by, dev_x, scan_x) = if own_row { (y + 46.0 * s, x + 16.0 * s, x + 16.0 * s + dw + 8.0 * s) } else { (y + 8.0 * s, bx - 10.0 * s - dw, bx + 120.0 * s) };
        if self.running {
            g.button(scan_x, by, 14.0 * s, "Stop (x)", ERR, Event::Tab(101));
        } else {
            g.button(scan_x, by, 14.0 * s, if self.summary.is_some() { "Scan again (s)" } else { "Scan (s)" }, ACCENT, Event::Tab(100));
        }
        g.button(dev_x, by, 14.0 * s, &dlabel, if new_devices > 0 { WARN } else { MUTED }, if self.devices { Event::Tab(111) } else { Event::Tab(110) });
        y += bar_h + 10.0 * s;
        // progress or the summary
        if self.running {
            let f = (self.progress.0 as f32 / self.progress.1.max(1) as f32).clamp(0.0, 1.0);
            fx::progress_bar(g, R::new(x, y, w, 8.0 * s), f, ACCENT, s);
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            g.text(x, y + 14.0 * s, 13.0 * s, MUTED, &format!("scanning{dots} {} of {} probes · {} machine(s) so far", self.progress.0, self.progress.1, self.hosts.len()), Face::Sans);
            y += 38.0 * s;
        } else if let Some((hosts, services, cancelled)) = self.summary {
            g.text(x, y, 14.0 * s, if cancelled { WARN } else { OK }, &format!("{} {hosts} machine(s) answered, {services} service(s) found", if cancelled { "stopped:" } else { "done:" }), Face::Sans);
            y += 26.0 * s;
        } else {
            // the hint wraps in a narrow window instead of running past its edge
            let hint = g.wrap(14.0 * s, "Press s to scan. Only the local network is touched, and only public questions are asked.", Face::Sans, w);
            for (i, l) in hint.iter().take(3).enumerate() {
                g.text(x, y + i as f32 * 19.0 * s, 14.0 * s, MUTED, l, Face::Sans);
            }
            y += 7.0 * s + hint.len().clamp(1, 3) as f32 * 19.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        }
        if self.devices {
            self.render_devices(g, area, s, port, y);
            if let Mode::Form(f, _) = &self.mode {
                f.render(g, area, s, ACCENT, blink(tick));
            }
            self.tip.draw(g, s);
            return;
        }
        // filters
        let mut fx = x;
        for (i, name) in FILTERS.iter().enumerate() {
            let count = self.hosts.values().filter(|h| matches(i, h)).count();
            let text = format!("{name} {count}");
            let fw = g.text_width(13.0 * s, &text, Face::Bold) + 26.0 * s;
            // the filters that do not fit one row continue on the next
            if fx > x && fx + fw > x + w {
                fx = x;
                y += 34.0 * s;
            }
            let r = R::new(fx, y, fw, 28.0 * s);
            let on = i == self.filter;
            let hot = g.hovered(r);
            fx::row_at(g, r.x, r.y, r.w, r.h, 14.0 * s, on, hot, ACCENT);
            g.text(r.x + 13.0 * s, r.y + 6.0 * s, 13.0 * s, if on { TEXT } else { MUTED }, &text, Face::Bold);
            g.hit(r, Event::Tab(i), name);
            fx += fw + 8.0 * s;
        }
        y += 40.0 * s;
        // machines (left) and the chosen one (right)
        let list_w = (w * 0.42).min(460.0 * s);
        let shown: Vec<HostInfo> = self.shown().into_iter().cloned().collect();
        let rh = 54.0 * s;
        let fit = (((area.y + area.h - y - 6.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
        let start = self.sel.saturating_sub(fit - 1);
        if shown.is_empty() {
            let msg = if self.running { "Looking…" } else if self.hosts.is_empty() && self.summary.is_some() { "Nothing answered on that network." } else if self.hosts.is_empty() { "Nothing scanned yet." } else { "Nothing in this filter." };
            fx::empty_state(g, R::new(x, y, w, (area.y + area.h - y - 20.0 * s).max(200.0 * s)), s, Icon::Scan, msg, "Press s to scan the local network.", ACCENT);
        }
        let mut ry = y;
        for (i, h) in shown.iter().enumerate().skip(start).take(fit) {
            let r = R::new(x, ry, list_w, rh);
            let sel = i == self.sel;
            fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &h.ip.to_string());
            let k = h.main_kind();
            g.rect(x, ry + 10.0 * s, 4.0 * s, rh - 20.0 * s, kind_color(k));
            let head = format!("{}{}", h.ip, h.name.as_ref().map(|n| format!("  {n}")).unwrap_or_default());
            let head_shown = fit_keep_ip(g, 16.0 * s, &head, Face::Bold, list_w - 60.0 * s);
            g.text(x + 16.0 * s, ry + 6.0 * s, 16.0 * s, TEXT, &head_shown, Face::Bold);
            let summary: Vec<String> = h.services.iter().take(4).map(|s| if s.kind == Kind::Web || s.kind == Kind::Other { format!(":{}", s.port) } else { s.kind.label().to_string() }).collect();
            let sub = g.fit(12.0 * s, &summary.join(" · "), Face::Sans, list_w - 60.0 * s);
            g.text(x + 16.0 * s, ry + 30.0 * s, 12.0 * s, kind_color(k), &sub, Face::Sans);
            if head_shown != head || sub != summary.join(" · ") {
                // the list is narrow; the right-hand panel has the same facts for the selected machine, and the tooltip for any other
                let all: Vec<String> = h.services.iter().map(|s| format!("{}/{} {}", s.port, s.proto, s.name)).collect();
                self.tip.offer(g, r, sel, &format!("{head}\n{}{}", h.mac.as_deref().map(|m| format!("hardware address {m}\n")).unwrap_or_default(), all.join(", ")));
            }
            if h.risky() {
                g.text(r.x + r.w - 26.0 * s, ry + 14.0 * s, 18.0 * s, WARN, "⚠", Face::Bold);
            }
            ry += rh + 6.0 * s;
        }
        if let Some(h) = shown.get(self.sel) {
            let dx = x + list_w + 16.0 * s;
            let dw = w - list_w - 16.0 * s;
            fx::panel(g, R::new(dx, y, dw, area.y + area.h - y - 6.0 * s), s, ACCENT);
            g.text(dx + 18.0 * s, y + 12.0 * s, 20.0 * s, TEXT, &h.ip.to_string(), Face::Bold);
            let sub = format!("{}{}", h.mac.as_deref().map(|m| format!("hardware address {m}")).unwrap_or_default(), h.name.as_ref().map(|n| format!("   name {n}")).unwrap_or_default());
            if !sub.trim().is_empty() {
                g.text(dx + 18.0 * s, y + 40.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
            }
            let mut sy = y + 64.0 * s;
            for (i, sv) in h.services.iter().enumerate() {
                let sel = i == self.svc_sel;
                let lines = g.wrap(12.0 * s, &sv.detail, Face::Sans, dw - 60.0 * s);
                let risk_lines = sv.risk.as_ref().map(|r| g.wrap(12.0 * s, &format!("⚠ {r}"), Face::Sans, dw - 60.0 * s)).unwrap_or_default();
                let models = if sv.models.is_empty() { None } else { Some(format!("models: {}", sv.models.iter().take(8).cloned().collect::<Vec<_>>().join(", "))) };
                let model_lines = models.map(|m| g.wrap(12.0 * s, &m, Face::Sans, dw - 60.0 * s)).unwrap_or_default();
                let nlines = lines.len().min(2) + risk_lines.len().min(2) + model_lines.len().min(2) + usize::from(sv.url.is_some());
                let rhh = 40.0 * s + nlines as f32 * 16.0 * s;
                if sy + rhh > area.y + area.h {
                    break;
                }
                let r = R::new(dx + 10.0 * s, sy, dw - 20.0 * s, rhh);
                fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, sel, false, ACCENT);
                g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, kind_color(sv.kind));
                let nm = g.fit(15.0 * s, &sv.name, Face::Bold, r.w - 150.0 * s);
                g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &nm, Face::Bold);
                g.text(r.x + r.w - 118.0 * s, r.y + 8.0 * s, 12.0 * s, kind_color(sv.kind), &format!("{}/{}", sv.port, sv.proto), Face::Mono);
                let mut ly = r.y + 28.0 * s;
                for l in lines.iter().take(2) {
                    g.text(r.x + 16.0 * s, ly, 12.0 * s, MUTED, l, Face::Sans);
                    ly += 16.0 * s;
                }
                for l in model_lines.iter().take(2) {
                    g.text(r.x + 16.0 * s, ly, 12.0 * s, OK, l, Face::Sans);
                    ly += 16.0 * s;
                }
                for l in risk_lines.iter().take(2) {
                    g.text(r.x + 16.0 * s, ly, 12.0 * s, WARN, l, Face::Sans);
                    ly += 16.0 * s;
                }
                if let Some(u) = &sv.url {
                    let uu = g.fit(12.0 * s, u, Face::Mono, r.w - 32.0 * s);
                    g.text(r.x + 16.0 * s, ly, 12.0 * s, rgb(120, 175, 255), &uu, Face::Mono);
                }
                if sel && sv.kind.is_model_server() {
                    g.button(r.x + r.w - 190.0 * s, r.y + r.h - 34.0 * s, 13.0 * s, "Add to models (a)", ACCENT, Event::Tab(102));
                }
                g.hit(R::new(r.x, r.y, r.w, r.h - 36.0 * s), Event::Row(self.sel), "service");
                sy += rhh + 8.0 * s;
            }
        }
        match &self.mode {
            Mode::Form(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Browse => {}
        }
        self.tip.draw(g, s);
    }
}

/// "1h 5m", "3m 4s": an age for a row (the same words the System page uses).
fn human_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else if m > 0 { format!("{m}m {}s", secs % 60) } else { format!("{secs}s") }
}
