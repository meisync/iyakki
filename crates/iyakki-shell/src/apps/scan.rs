//! NET SCAN: find what runs on the local network and use it. Model servers (Ollama and others) can be added to
//! Settings → Models in one step, with an API key going into the system vault when the server needs one; cameras and
//! other services show their address to copy. Scans stay on private addresses, ask only public questions and can be
//! stopped; every scan is written to the audit log.

use crate::safeclamp::SafeClamp;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_agent::providers::{valid_provider_id, Kind as ProviderKind, ProviderCfg};
use iyakki_agent::settings::ModelChoice;
use iyakki_netscan::{net, Event as ScanEvent, HostInfo, Kind, Options, Service};

use super::widgets::{Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::tooltip::{fit_keep_ip, Tooltip};
use crate::ui::{clipboard, Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(120, 220, 140);
const WARN: [u8; 4] = rgb(255, 196, 94);

struct Network {
    label: String,
    targets: Vec<Ipv4Addr>,
}

enum Mode {
    Browse,
    Form(Form, FormKind),
}

enum FormKind {
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

pub struct ScanApp {
    nets: Vec<Network>,
    net_sel: usize,
    hosts: BTreeMap<Ipv4Addr, HostInfo>,
    filter: usize,
    sel: usize,
    svc_sel: usize,
    running: bool,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<ScanEvent>>,
    progress: (usize, usize),
    summary: Option<(usize, usize, bool)>,
    mode: Mode,
    status: Option<(String, bool)>,
    /// Ports to try instead of the default list (tests).
    ports: Option<Vec<u16>>,
    /// The list of devices seen on the network (new-device alerts) instead of the scan's machines.
    devices: bool,
    dev_sel: usize,
    /// The whole text of a row whose lines were cut to the width.
    tip: Tooltip,
}

impl ScanApp {
    pub fn new(_cx: &AppCx) -> ScanApp {
        let mut nets: Vec<Network> = net::local_subnets().into_iter().map(|s| Network { label: format!("{}  {}  ({} addresses)", s.iface, s.cidr(), net::hosts_of(&s).len()), targets: net::hosts_of(&s) }).collect();
        nets.push(Network { label: "This machine (127.0.0.1)".into(), targets: vec![Ipv4Addr::LOCALHOST] });
        ScanApp { nets, net_sel: 0, hosts: BTreeMap::new(), filter: 0, sel: 0, svc_sel: 0, running: false, cancel: Arc::new(AtomicBool::new(false)), rx: None, progress: (0, 0), summary: None, mode: Mode::Browse, status: None, ports: None, devices: false, dev_sel: 0, tip: Tooltip::new() }
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

    fn shown(&self) -> Vec<&HostInfo> {
        self.hosts.values().filter(|h| matches(self.filter, h)).collect()
    }

    fn current(&self) -> Option<&HostInfo> {
        self.shown().get(self.sel).copied()
    }

    fn start(&mut self, cx: &AppCx) {
        if self.running {
            return;
        }
        let Some(net) = self.nets.get(self.net_sel) else { return };
        // the account's host policy decides which addresses may be touched
        let blocked = net.targets.iter().find(|ip| !cx.session.policy.allows_host(&ip.to_string()));
        if let Some(ip) = blocked {
            self.status = Some((format!("Your account may not scan {ip}."), false));
            return;
        }
        cx.audit.record(&cx.session.user, "netscan.start", &net.label);
        let (tx, rx) = channel();
        let (targets, cancel) = (net.targets.clone(), Arc::new(AtomicBool::new(false)));
        let mut opts = Options::default();
        if let Some(p) = &self.ports {
            opts.ports = p.clone();
            opts.announce_wait = None;
        }
        self.cancel = cancel.clone();
        std::thread::Builder::new().name("iyakki-netscan".into()).spawn(move || iyakki_netscan::scan(targets, opts, cancel, tx)).ok();
        self.rx = Some(rx);
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

    fn poll(&mut self, cx: &AppCx) {
        let Some(rx) = &self.rx else { return };
        let mut finished = None;
        while let Ok(ev) = rx.try_recv() {
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
            self.rx = None;
            self.summary = Some((hosts, services, cancelled));
            // what the scan found also tells the new-device watch who is on the network
            let seen: Vec<iyakki_agent::devwatch::Neighbor> = self.hosts.values().filter_map(|h| Some(iyakki_agent::devwatch::Neighbor { ip: h.ip.to_string(), mac: h.mac.clone()?.to_lowercase(), iface: "scan".into() })).collect();
            if cx.session.policy.manage_settings {
                cx.services.devwatch.observe(&seen, iyakki_agent::clock::now_unix());
            }
            cx.audit.record(&cx.session.user, "netscan.done", &format!("{hosts} hosts, {services} services{}", if cancelled { ", stopped" } else { "" }));
        }
    }

    fn handle_devices(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let admin = cx.session.policy.manage_settings;
        let list = cx.services.devwatch.list();
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
                    cx.services.devwatch.trust(&d.mac);
                    cx.audit.record(&cx.session.user, "netsec.trust", &d.mac);
                    self.status = Some((format!("{} is trusted now.", d.mac), true));
                }
                _ => {}
            },
            Event::Tab(113) | Event::Key(Key::Char('T')) => {
                if !admin {
                    deny(self);
                } else {
                    let n = cx.services.devwatch.trust_all();
                    cx.audit.record(&cx.session.user, "netsec.trust_all", &n.to_string());
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
                    cx.services.devwatch.forget(&d.mac);
                    self.status = Some((format!("Forgot {}. If it comes back it is new again.", d.mac), true));
                }
                _ => {}
            },
            Event::Tab(115) | Event::Key(Key::Char('w')) => {
                if !admin {
                    deny(self);
                } else {
                    let on = !cx.services.devwatch.enabled();
                    cx.services.devwatch.set_enabled(on);
                    cx.audit.record(&cx.session.user, "netsec.watch", if on { "on" } else { "off" });
                    self.status = Some((if on { "Watching for new devices.".into() } else { "Not watching for new devices.".into() }, true));
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render_devices(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, y0: f32) {
        let (x, w) = (area.x + 24.0 * s, area.w - 48.0 * s);
        let list = cx.services.devwatch.list();
        let on = cx.services.devwatch.enabled();
        let mut y = y0;
        let untrusted = list.iter().filter(|d| !d.trusted).count();
        let head = if !on { "Watching is off (w turns it on).".to_string() } else if untrusted > 0 { format!("{untrusted} device(s) you have not seen before. Trust the ones you know (t), forget the rest (x).") } else { "Every device seen is trusted. A new one raises an alert.".to_string() };
        let head = g.fit(14.0 * s, &head, Face::Sans, w);
        g.text(x, y, 14.0 * s, if on && untrusted > 0 { WARN } else { MUTED }, &head, Face::Sans);
        y += 26.0 * s;
        let now = iyakki_agent::clock::now_unix();
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
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, on_row, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), "device");
            g.rect(x, y + 10.0 * s, 4.0 * s, rh - 20.0 * s, if d.trusted { OK } else { WARN });
            let title = if d.name.is_empty() { d.mac.clone() } else { format!("{} · {}", d.name, d.mac) };
            let shown = g.fit(16.0 * s, &title, Face::Bold, w - 190.0 * s);
            g.text(x + 16.0 * s, y + 6.0 * s, 16.0 * s, TEXT, &shown, Face::Bold);
            let ago = |t: i64| iyakki_agent::sysinfo::human_duration((now - t).max(0) as u64);
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
            crate::fx::chip(g, r.x + r.w - cw - 12.0 * s, y + 14.0 * s, s, chip, col, !d.trusted);
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

    fn target_form() -> Form {
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

    fn open_add(&mut self, cx: &AppCx) {
        let Some((ip, svc)) = self.selected_service() else { return };
        if !svc.kind.is_model_server() {
            self.status = Some(("Only model servers (Ollama and OpenAI-compatible ones) can be added to Models.".into(), false));
            return;
        }
        if !cx.session.policy.manage_settings {
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
fn is_lemonade(svc: &Service) -> bool {
    svc.kind == Kind::LlmServer && svc.name.starts_with("Lemonade")
}

/// Add a Lemonade server from the scan: its typed list is read at once so the chat list holds its chat models only, and the
/// model chosen in the form becomes the chat model when none is set and it is one of those.
fn add_lemonade(cx: &AppCx, ip: Ipv4Addr, mut cfg: ProviderCfg, model: &str, key: &str) -> Result<String, String> {
    let id = cfg.id.clone();
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?;
    if !key.is_empty() {
        cx.services.system_set(&key_name, key).map_err(|e| e.to_string())?;
        cx.audit.record(&cx.session.user, "vault.system.set", &key_name);
    }
    cx.services.settings.lock().unwrap().upsert_provider(cfg).map_err(|e| e.to_string())?;
    cx.audit.record(&cx.session.user, "settings.provider.save", &format!("{id} (Lemonade) from net scan {ip}"));
    let list = cx.services.refresh_lemonade(&id).unwrap_or_default();
    let chats: Vec<String> = list.iter().filter(|m| m.downloaded && m.kind.chats()).map(|m| m.id.clone()).collect();
    let mut note = String::new();
    let mut st = cx.services.settings.lock().unwrap();
    let pick = if chats.iter().any(|m| m == model) { Some(model.to_string()) } else { chats.first().cloned() };
    if let (None, Some(m)) = (st.get().chat.clone(), pick) {
        st.set_chat(Some(ModelChoice { provider: id.clone(), model: m.clone() })).map_err(|e| e.to_string())?;
        note = format!(" It is now the chat model ({m}).");
    }
    let others = list.len() - chats.len();
    Ok(format!("Added {id} (Lemonade) to Models with {} chat model{} and {others} other{} (pictures, speech, embeddings: see Settings → Models).{note}", chats.len(), if chats.len() == 1 { "" } else { "s" }, if others == 1 { "" } else { "s" }))
}

fn provider_id(ip: Ipv4Addr, svc: &Service) -> String {
    let base = if svc.kind == Kind::Ollama { "ollama" } else if is_lemonade(svc) { "lemonade" } else { "llm" };
    let dashed = ip.to_string().replace('.', "-");
    if svc.port == 11434 || svc.port == 1234 { format!("{base}-{dashed}") } else { format!("{base}-{dashed}-{}", svc.port) }
}

fn base_url(ip: Ipv4Addr, svc: &Service) -> String {
    let scheme = if svc.url.as_deref().is_some_and(|u| u.starts_with("https")) { "https" } else { "http" };
    match svc.kind {
        // OpenAI-compatible servers are reached through their /v1 address; Ollama's own address has no path
        Kind::LlmServer => svc.url.clone().unwrap_or_else(|| format!("{scheme}://{ip}:{}/v1", svc.port)),
        _ => format!("{scheme}://{ip}:{}", svc.port),
    }
}

/// Add a found model server to Settings → Models. The API key, if there is one, goes into the system vault and the provider
/// keeps only a reference to it. If no chat model is chosen yet, this one becomes it. Returns what to tell the person.
pub fn add_model_server(cx: &AppCx, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
    if !cx.session.policy.manage_settings {
        cx.audit.record(&cx.session.user, "settings.denied", "netscan add model");
        return Err("Only an administrator can add models.".into());
    }
    if !valid_provider_id(id) {
        return Err("Provider name: lowercase letters, digits, - and _; it starts with a letter.".into());
    }
    let kind = if svc.kind == Kind::Ollama { ProviderKind::Ollama } else if is_lemonade(svc) { ProviderKind::Lemonade } else { ProviderKind::Compat };
    let mut cfg = ProviderCfg::new(id, kind);
    cfg.base_url = Some(address.trim().to_string());
    if kind == ProviderKind::Lemonade {
        // Lemonade's list says what each model does: only its chat models go to the chat list (read after it is added)
        return add_lemonade(cx, ip, cfg, model, key);
    }
    cfg.models = svc.models.clone();
    cfg.default_model = if model.is_empty() { svc.models.first().cloned() } else { Some(model.to_string()) };
    let key_name = format!("{id}-key");
    if !key.is_empty() {
        cfg.key_ref = Some(format!("vault:{key_name}"));
    }
    cfg.validate()?; // before touching the vault: a rejected form stores nothing
    if !key.is_empty() {
        cx.services.system_set(&key_name, key).map_err(|e| match e {
            iyakki_agent::vault::VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })?;
        cx.audit.record(&cx.session.user, "vault.system.set", &key_name);
    }
    let chosen = cfg.default_model.clone();
    let mut settings = cx.services.settings.lock().unwrap();
    settings.upsert_provider(cfg).map_err(|e| e.to_string())?;
    let mut note = String::new();
    if settings.get().chat.is_none() {
        if let Some(m) = &chosen {
            settings.set_chat(Some(ModelChoice { provider: id.to_string(), model: m.clone() })).map_err(|e| e.to_string())?;
            note = format!(" It is now the chat model ({m}).");
        }
    }
    drop(settings);
    cx.audit.record(&cx.session.user, "settings.provider.save", &format!("{id} ({}) from net scan {ip}", kind.label()));
    Ok(format!("Added {id} to Models{}.{note}", if key.is_empty() { String::new() } else { " and its key to the vault".to_string() }))
}

impl App for ScanApp {
    fn title(&self) -> &'static str {
        "Net scan"
    }
    fn icon(&self) -> Icon {
        Icon::Scan
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "s scan · x stop · n network · t type a target · ↑↓ machine · ← → service · a add model · c copy".into()
    }

    fn help(&self) -> Vec<String> {
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

    fn wants_redraw(&mut self) -> bool {
        self.tip.take_changed()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.tip.event(ev);
        if matches!(ev, Event::Tick) {
            self.poll(cx);
            return Flow::Stay;
        }
        if let Mode::Form(form, kind) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => {
                    match kind {
                        FormKind::Target => {
                            let text = form.value(0);
                            match net::parse_targets(&text) {
                                Ok(t) => {
                                    self.nets.insert(0, Network { label: format!("typed: {text}  ({} addresses)", t.len()), targets: t });
                                    self.net_sel = 0;
                                    self.mode = Mode::Browse;
                                    self.start(cx);
                                }
                                Err(e) => form.error = Some(e),
                            }
                        }
                        FormKind::RenameDevice(mac) => {
                            let ok = cx.services.devwatch.rename(mac, &form.value(0));
                            self.status = Some((if ok { "Renamed.".into() } else { "That device is no longer listed.".into() }, ok));
                            self.mode = Mode::Browse;
                        }
                        FormKind::AddModel { ip, svc, models } => {
                            let (id, address, key) = (form.value(0), form.value(1), form.raw(2));
                            let model = if models.is_empty() { form.value(3) } else { models.get(form.choice_idx(3)).cloned().unwrap_or_default() };
                            match add_model_server(cx, *ip, svc, &id, &address, &model, &key) {
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
            return Flow::Stay;
        }
        if self.devices {
            return self.handle_devices(ev, cx);
        }
        match ev {
            Event::Tab(110) => {
                self.devices = true;
                return Flow::Stay;
            }
            Event::Row(i) => {
                self.sel = (*i).min(self.shown().len().saturating_sub(1));
                self.svc_sel = 0;
                return Flow::Stay;
            }
            Event::Tab(n) if *n < FILTERS.len() => {
                self.filter = *n;
                self.sel = 0;
                self.svc_sel = 0;
                return Flow::Stay;
            }
            Event::Tab(100) => {
                self.start(cx);
                return Flow::Stay;
            }
            Event::Tab(101) => {
                self.stop();
                return Flow::Stay;
            }
            Event::Tab(102) => {
                self.open_add(cx);
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                self.sel = (self.sel as i32 - d).clamp_safe(0, self.shown().len().saturating_sub(1) as i32) as usize;
                self.svc_sel = 0;
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        let n = self.shown().len();
        match k {
            Key::Esc => {
                if self.running {
                    self.stop();
                }
                return Flow::Back;
            }
            Key::Char('s') | Key::Enter => self.start(cx),
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
            Key::Char('a') => self.open_add(cx),
            Key::Char('d') => self.devices = true,
            Key::Char('c') => self.copy_address(),
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // network and controls
        let label = self.nets.get(self.net_sel).map_or(String::new(), |n| n.label.clone());
        crate::fx::glass(g, x, y, w, 46.0 * s, 12.0 * s, ACCENT);
        g.text(x + 16.0 * s, y + 6.0 * s, 11.0 * s, DIM, "NETWORK  (n changes it, t types one)", Face::Bold);
        let label = g.fit(16.0 * s, &label, Face::Bold, w - 330.0 * s);
        g.text(x + 16.0 * s, y + 22.0 * s, 16.0 * s, TEXT, &label, Face::Bold);
        let bx = x + w - 300.0 * s;
        if self.running {
            g.button(bx + 120.0 * s, y + 8.0 * s, 14.0 * s, "Stop (x)", ERR, Event::Tab(101));
        } else {
            g.button(bx + 120.0 * s, y + 8.0 * s, 14.0 * s, if self.summary.is_some() { "Scan again (s)" } else { "Scan (s)" }, ACCENT, Event::Tab(100));
        }
        let new_devices = _cx.services.devwatch.untrusted_count();
        let dlabel = if new_devices > 0 { format!("Devices ({new_devices} new)") } else { "Devices (d)".to_string() };
        let dw = g.text_width(14.0 * s, &dlabel, Face::Bold) + 24.0 * s;
        g.button(bx - 10.0 * s - dw, y + 8.0 * s, 14.0 * s, &dlabel, if new_devices > 0 { WARN } else { MUTED }, if self.devices { Event::Tab(111) } else { Event::Tab(110) });
        y += 56.0 * s;
        // progress or the summary
        if self.running {
            let f = (self.progress.0 as f32 / self.progress.1.max(1) as f32).clamp(0.0, 1.0);
            crate::fx::progress_bar(g, R::new(x, y, w, 8.0 * s), f, ACCENT, s);
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            g.text(x, y + 14.0 * s, 13.0 * s, MUTED, &format!("scanning{dots} {} of {} probes · {} machine(s) so far", self.progress.0, self.progress.1, self.hosts.len()), Face::Sans);
            y += 38.0 * s;
        } else if let Some((hosts, services, cancelled)) = self.summary {
            g.text(x, y, 14.0 * s, if cancelled { WARN } else { OK }, &format!("{} {hosts} machine(s) answered, {services} service(s) found", if cancelled { "stopped:" } else { "done:" }), Face::Sans);
            y += 26.0 * s;
        } else {
            g.text(x, y, 14.0 * s, MUTED, "Press s to scan. Only the local network is touched, and only public questions are asked.", Face::Sans);
            y += 26.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
            y += 24.0 * s;
        }
        if self.devices {
            self.render_devices(g, area, s, _cx, y);
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
            let r = R::new(fx, y, fw, 28.0 * s);
            let on = i == self.filter;
            let hot = g.hovered(r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 14.0 * s, on, hot, ACCENT);
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
            crate::fx::empty_state(g, R::new(x, y, w, (area.y + area.h - y - 20.0 * s).max(200.0 * s)), s, Icon::Scan, msg, "Press s to scan the local network.", ACCENT);
        }
        let mut ry = y;
        for (i, h) in shown.iter().enumerate().skip(start).take(fit) {
            let r = R::new(x, ry, list_w, rh);
            let sel = i == self.sel;
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, sel, g.hovered(r), ACCENT);
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
            crate::fx::panel(g, R::new(dx, y, dw, area.y + area.h - y - 6.0 * s), s, ACCENT);
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
                crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, sel, false, ACCENT);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use crate::tooltip::POINTER_DELAY;
    use iyakki_sec::policy::Role;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A tiny Ollama look-alike on a free local port.
    fn fake_ollama() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut buf = [0u8; 2048];
                    let mut got = Vec::new();
                    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => got.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&got).lines().next().unwrap_or("").to_string();
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = match path.as_str() {
                        "/" => "Ollama is running".to_string(),
                        "/api/version" => "{\"version\":\"0.5.7\"}".to_string(),
                        "/api/tags" => "{\"models\":[{\"name\":\"llama3.2:3b\"},{\"name\":\"qwen3:32b\"}]}".to_string(),
                        _ => "nope".to_string(),
                    };
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                });
            }
        });
        port
    }

    fn admin_cx(tag: &str) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib scan_picture -- --ignored`: a real scan of this machine, drawn.
    #[test]
    #[ignore]
    fn scan_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let cx = admin_cx("scanpic");
        let mut a = ScanApp::new(&cx);
        a.net_sel = a.nets.len() - 1;
        a.handle(&Event::Key(Key::Char('s')), &cx);
        finish(&mut a, &cx);
        // choose the model server
        let i = a.shown().iter().position(|h| h.has(Kind::Ollama)).unwrap_or(0);
        a.sel = i;
        let svc = a.current().and_then(|h| h.services.iter().position(|s| s.kind == Kind::Ollama)).unwrap_or(0);
        a.svc_sel = svc;
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 5);
        g.save_png(std::path::Path::new(&out)).unwrap();
    }

    fn finish(a: &mut ScanApp, cx: &AppCx) {
        for _ in 0..3000 {
            a.handle(&Event::Tick, cx);
            if !a.is_running() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the scan never finished");
    }

    fn scanned(cx: &AppCx) -> (ScanApp, u16) {
        let port = fake_ollama();
        let mut a = ScanApp::new(cx);
        a.ports = Some(vec![port]);
        a.net_sel = a.nets.len() - 1; // this machine
        a.handle(&Event::Key(Key::Char('s')), cx);
        assert!(a.is_running());
        finish(&mut a, cx);
        (a, port)
    }

    #[test]
    fn a_scan_finds_a_model_server_and_is_written_to_the_audit_log() {
        let cx = admin_cx("scan1");
        let (a, port) = scanned(&cx);
        let hosts = a.hosts();
        assert_eq!(hosts.len(), 1);
        let s = &hosts[0].services[0];
        assert_eq!((s.port, s.kind, s.name.as_str()), (port, Kind::Ollama, "Ollama 0.5.7"));
        assert_eq!(s.models, vec!["llama3.2:3b", "qwen3:32b"]);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("netscan.start") && log.contains("netscan.done") && log.contains("1 hosts"), "{log}");
    }

    #[test]
    fn adding_a_found_ollama_creates_the_provider_picks_the_chat_model_and_keeps_a_key_in_the_vault() {
        let cx = admin_cx("scan2");
        let (mut a, port) = scanned(&cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        let Mode::Form(f, _) = &mut a.mode else { panic!("the add form opens") };
        assert_eq!(f.value(0), format!("ollama-127-0-0-1-{port}"), "named after the machine, with the port when it is not the default");
        assert_eq!(f.value(1), format!("http://127.0.0.1:{port}"));
        f.set_value(2, "tok-123");
        f.fields[3].choice = 1; // qwen3:32b
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
        assert!(a.status_text().unwrap().contains("Added") && a.status_text().unwrap().contains("vault"), "{:?}", a.status_text());
        let settings = cx.services.settings.lock().unwrap();
        let p = &settings.get().providers[0];
        assert_eq!((p.kind, p.base_url.as_deref(), p.default_model.as_deref()), (ProviderKind::Ollama, Some(&format!("http://127.0.0.1:{port}")[..]), Some("qwen3:32b")));
        assert_eq!(p.models, vec!["llama3.2:3b", "qwen3:32b"]);
        assert_eq!(p.key_ref.as_deref(), Some(&format!("vault:ollama-127-0-0-1-{port}-key")[..]), "the provider holds a reference, not the key");
        let chat = settings.get().chat.clone().unwrap();
        assert_eq!((chat.provider.as_str(), chat.model.as_str()), (&format!("ollama-127-0-0-1-{port}")[..], "qwen3:32b"), "the first model server becomes the chat model");
        drop(settings);
        assert!(cx.services.system_names().unwrap().contains(&format!("ollama-127-0-0-1-{port}-key")));
        let json = std::fs::read_to_string(cx.services.dir().join("settings.json")).unwrap_or_default();
        assert!(!json.contains("tok-123"), "the key is not in the settings file");
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("settings.provider.save") && log.contains("vault.system.set"));
    }

    #[test]
    fn a_lemonade_found_on_the_network_is_added_as_lemonade_with_only_its_chat_models_in_the_chat_list() {
        let fake = crate::apps::lemonade_fake::serve();
        let mut cx = cx_with_admin_store("scanlem");
        let d = temp_dir("scanlem-svc");
        cx.services = services_in(&d, Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(10))));
        cx.services.on_login("root", "rootpass12", true);
        let mut a = ScanApp::new(&cx);
        a.ports = Some(vec![fake.port]);
        a.net_sel = a.nets.len() - 1;
        a.handle(&Event::Key(Key::Char('s')), &cx);
        finish(&mut a, &cx);
        let s = a.hosts()[0].services[0].clone();
        assert_eq!((s.kind, s.name.as_str()), (Kind::LlmServer, "Lemonade Server 2026.42.1"));
        a.handle(&Event::Key(Key::Char('a')), &cx);
        let Mode::Form(f, _) = &mut a.mode else { panic!("the add form opens") };
        assert_eq!((f.value(0), f.value(1)), (format!("lemonade-127-0-0-1-{}", fake.port), fake.base()));
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
        assert!(a.status_text().unwrap().contains("(Lemonade)") && a.status_text().unwrap().contains("2 chat models"), "{:?}", a.status_text());
        let st = cx.services.settings.lock().unwrap().get().clone();
        let p = &st.providers[0];
        assert_eq!((p.kind, p.models.clone()), (ProviderKind::Lemonade, vec!["Qwen3-0.6B-GGUF".to_string(), "Phi-4-Multimodal-GGUF".to_string()]));
        assert_eq!(p.model_kinds.get("Whisper-Base").map(String::as_str), Some("stt"));
        assert_eq!(st.chat.as_ref().map(|c| c.model.as_str()), Some("Qwen3-0.6B-GGUF"), "the first chat model, never a picture model");
    }

    #[test]
    fn a_second_model_server_does_not_take_over_the_chat_model_and_bad_names_are_refused() {
        let cx = admin_cx("scan3");
        let (mut a, _) = scanned(&cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        let first = cx.services.settings.lock().unwrap().get().chat.clone().unwrap().provider;
        a.handle(&Event::Key(Key::Char('a')), &cx);
        let Mode::Form(f, _) = &mut a.mode else { panic!() };
        f.set_value(0, "Bad Name!");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        let Mode::Form(f, _) = &mut a.mode else { panic!("the form stays open") };
        assert!(f.error.as_deref().unwrap().contains("Provider name"), "{:?}", f.error);
        f.set_value(0, "second");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        let s = cx.services.settings.lock().unwrap();
        assert_eq!(s.get().providers.len(), 2);
        assert_eq!(s.get().chat.clone().unwrap().provider, first, "the first choice stays");
    }

    #[test]
    fn only_administrators_add_models_and_the_hosts_a_scan_may_touch_follow_the_policy() {
        let mut cx = admin_cx("scan4");
        let (mut a, _) = scanned(&cx);
        cx.session.role = Role::Operator;
        cx.session.policy = Role::Operator.template();
        a.handle(&Event::Key(Key::Char('a')), &cx);
        assert!(matches!(a.mode, Mode::Browse));
        assert!(a.status_text().unwrap().contains("administrator"), "{:?}", a.status_text());
        assert!(cx.services.settings.lock().unwrap().get().providers.is_empty());
        // copying the address needs no permission
        a.handle(&Event::Key(Key::Char('c')), &cx);
        assert!(a.status_text().unwrap().starts_with("Copied http://127.0.0.1:"));
        // an account that may not reach those addresses cannot scan them
        cx.session.policy.net_hosts = vec!["other.example".into()];
        let mut b = ScanApp::new(&cx);
        b.net_sel = b.nets.len() - 1;
        b.handle(&Event::Key(Key::Char('s')), &cx);
        assert!(!b.is_running() && b.status_text().unwrap().contains("may not scan"), "{:?}", b.status_text());
        // the terminal level is checked in the command; here the tile is simply not given to users
        assert!(!Role::User.template().apps.contains(&iyakki_sec::policy::App::Scan) && Role::Operator.template().apps.contains(&iyakki_sec::policy::App::Scan));
    }

    #[test]
    fn typed_targets_are_checked_filters_work_and_every_state_draws() {
        let cx = admin_cx("scan5");
        let (mut a, _) = scanned(&cx);
        a.handle(&Event::Key(Key::Char('t')), &cx);
        for c in "8.8.8.8".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        let Mode::Form(f, _) = &a.mode else { panic!() };
        assert!(f.error.as_deref().unwrap().contains("not on the local network"), "{:?}", f.error);
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(matches!(a.mode, Mode::Browse));
        // filters
        a.handle(&Event::Key(Key::Char('4')), &cx); // media: nothing
        assert!(a.shown().is_empty());
        a.handle(&Event::Key(Key::Char('3')), &cx); // cameras: nothing
        assert!(a.shown().is_empty());
        a.handle(&Event::Key(Key::Char('2')), &cx); // model servers
        assert_eq!(a.shown().len(), 1);
        a.handle(&Event::Key(Key::Char('1')), &cx); // all
        assert_eq!(a.shown().len(), 1);
        let mut g;
        for (w, h) in [(1280, 720), (800, 600), (1920, 1080)] {
            g = Gfx::new(w, h);
            a.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), (w as f32 / 1280.0).clamp(0.6, 3.0), &cx, 3);
        }
        a.mode = Mode::Form(ScanApp::target_form(), FormKind::Target);
        g = Gfx::new(1280, 720);
        a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 3);
        assert!(!a.help().is_empty());
    }

    #[test]
    fn openai_compatible_servers_are_added_as_compatible_providers_through_their_v1_address() {
        let cx = admin_cx("scan6");
        let mut svc = Service::new(1234, Kind::LlmServer, "LM Studio");
        svc.url = Some("http://192.168.1.50:1234/v1".into());
        svc.models = vec!["qwen2-7b".into()];
        let ip: Ipv4Addr = "192.168.1.50".parse().unwrap();
        assert_eq!(provider_id(ip, &svc), "llm-192-168-1-50");
        assert_eq!(base_url(ip, &svc), "http://192.168.1.50:1234/v1");
        let msg = add_model_server(&cx, ip, &svc, "lmstudio", &base_url(ip, &svc), "", "").unwrap();
        assert!(msg.contains("chat model"), "{msg}");
        let s = cx.services.settings.lock().unwrap();
        assert_eq!((s.get().providers[0].kind, s.get().providers[0].default_model.as_deref()), (ProviderKind::Compat, Some("qwen2-7b")));
        assert!(s.get().providers[0].key_ref.is_none());
    }

    #[test]
    fn the_devices_view_lists_strangers_first_and_an_administrator_trusts_names_and_forgets_them() {
        let cx = cx_with_admin_store("scan-devices");
        let mut app = ScanApp::new(&cx);
        let n = |ip: &str, mac: &str| iyakki_agent::devwatch::Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() };
        cx.services.devwatch.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01")], 100);
        cx.services.devwatch.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.9", "bb:bb:bb:bb:bb:09")], 200);
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(110)), "the Devices button is there and says one is new");
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(app.devices);
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        assert_eq!(g.hits.iter().filter(|h| h.label == "device").count(), 2);
        // the stranger is first
        assert_eq!(cx.services.devwatch.list()[0].mac, "bb:bb:bb:bb:bb:09");
        app.handle(&Event::Key(Key::Char('r')), &cx);
        for c in "Guest phone".chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(cx.services.devwatch.list().iter().find(|d| d.mac == "bb:bb:bb:bb:bb:09").unwrap().name, "Guest phone");
        app.handle(&Event::Key(Key::Char('t')), &cx);
        assert_eq!(cx.services.devwatch.untrusted_count(), 0);
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("netsec.trust") && l.contains("bb:bb")));
        // forget the chosen one (the list is trusted-last now, so choose the stranger by row)
        let row = cx.services.devwatch.list().iter().position(|d| d.mac == "bb:bb:bb:bb:bb:09").unwrap();
        app.handle(&Event::Row(row), &cx);
        app.handle(&Event::Key(Key::Char('x')), &cx);
        assert_eq!(cx.services.devwatch.list().len(), 1);
        // switching the watch off
        app.handle(&Event::Key(Key::Char('w')), &cx);
        assert!(!cx.services.devwatch.enabled());
        // Esc leaves the devices view first, then the app
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Stay));
        assert!(!app.devices);
        // an ordinary account can look but not change anything
        let mut cx2 = cx_with_admin_store("scan-devices2");
        cx2.services = cx.services.clone();
        cx2.session.role = Role::User;
        cx2.session.policy = Role::User.template();
        cx2.services.devwatch.set_enabled(true);
        let mut u = ScanApp::new(&cx2);
        u.handle(&Event::Key(Key::Char('d')), &cx2);
        u.handle(&Event::Key(Key::Char('T')), &cx2);
        assert!(u.status_text().unwrap().contains("Only administrators"));
    }

    #[test]
    fn a_narrow_machine_list_keeps_the_address_and_a_tooltip_has_all_of_the_row() {
        let cx = admin_cx("scan-tip");
        let mut a = ScanApp::new(&cx);
        let ip = Ipv4Addr::new(192, 168, 100, 200);
        let mut h = HostInfo::new(ip);
        h.name = Some("living-room-television-with-a-very-long-friendly-name".into());
        h.mac = Some("aa:bb:cc:dd:ee:ff".into());
        a.hosts.insert(ip, h);
        let area = R::new(0.0, 46.0, 520.0, 674.0);
        let mut g = Gfx::new(520, 720);
        let draw = |a: &mut ScanApp, g: &mut Gfx| {
            g.clear(rgb(9, 11, 20));
            a.render(g, area, 1.0, &cx, 5);
        };
        draw(&mut a, &mut g);
        let row = g.hits.iter().find(|h| h.label == "192.168.100.200").expect("the row").rect;
        // the line is cut for the width, yet the address is whole
        let head = format!("{ip}  {}", a.hosts[&ip].name.clone().unwrap());
        let cut = fit_keep_ip(&mut g, 16.0, &head, Face::Bold, row.w - 60.0);
        assert!(cut != head && cut.starts_with("192.168.100.200"), "{cut}");
        let plain = g.rgba().to_vec();
        // the pointer rests on it: nothing for half a second, then the tooltip, which asks for a new picture
        let (mx, my) = ((row.x + 20.0) as i32, (row.y + 10.0) as i32);
        g.mouse = (mx, my);
        a.handle(&Event::MouseMove(mx, my), &cx);
        for _ in 0..POINTER_DELAY - 1 {
            a.handle(&Event::Tick, &cx);
        }
        draw(&mut a, &mut g);
        let hovered = g.rgba().to_vec();
        for _ in 0..2 {
            a.handle(&Event::Tick, &cx);
        }
        assert!(a.wants_redraw(), "the tooltip appears");
        draw(&mut a, &mut g);
        assert_ne!(hovered, g.rgba(), "the tooltip is drawn");
        assert_ne!(plain, g.rgba());
        // a key press puts it away at once
        a.handle(&Event::Key(Key::Down), &cx);
        draw(&mut a, &mut g);
        assert!(a.wants_redraw());
    }
}
