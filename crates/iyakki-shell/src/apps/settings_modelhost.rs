//! Settings -> Models -> "Model host on your network" (also Store -> Setup -> Use a Mac's GPU): the guided page for
//! using the GPU of another computer on the network, a Mac with Apple Silicon first. It shows what the person installs ON that
//! machine (IYAKKI never installs there), takes its address, tests it (model list, then a tiny chat with tokens per second),
//! adds it to Models as `mac-<nickname>`, can look for model servers on the local /24 when the person asks, and can publish
//! a Service in the cluster so pods call it too. The rules and the network code are in `iyakki_agent::modelhost`.

use crate::safeclamp::SafeClamp;
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;

use iyakki_agent::modelhost::{self as mh, Found, Host, Hosts, Kube, Limits, NewHost, Preset, ScanOpts, ServerApp, TestReport};

use super::*;

/// What a finished background step hands back.
pub(super) enum MhOut {
    /// A test (and maybe an add): the form fingerprint it was for, the report or the plain error, and what to say instead of the summary.
    Tested(String, Result<TestReport, String>, Option<String>),
    Scan(Result<Vec<Found>, String>),
    Msg(Result<String, String>),
}

pub(super) struct MhPanel {
    pub(super) open: bool,
    mac: bool,
    server: usize,
    nickname: String,
    address: String,
    key: String,
    confirm: String,
    scroll: f32,
    max_scroll: f32,
    /// The last successful test and the fingerprint (address + key) it was for.
    report: Option<(String, TestReport)>,
    found: Vec<Found>,
    scanned: bool,
    hosts: Vec<Host>,
    /// Tests give their own limits, scan targets and cluster.
    pub(super) limits: Option<(Limits, Limits)>,
    pub(super) scan_override: Option<(Vec<IpAddr>, ScanOpts)>,
    pub(super) kube: Option<Arc<dyn Kube + Send + Sync>>,
}

impl Default for MhPanel {
    fn default() -> MhPanel {
        MhPanel { open: false, mac: true, server: 0, nickname: String::new(), address: String::new(), key: String::new(), confirm: String::new(), scroll: 0.0, max_scroll: 0.0, report: None, found: Vec::new(), scanned: false, hosts: Vec::new(), limits: None, scan_override: None, kube: None }
    }
}

const USE_BASE: usize = 7000;
const HOST_BASE: usize = 7100;
const ACT_TEST: usize = 0;
const ACT_PUBLISH: usize = 1;
const ACT_UNPUBLISH: usize = 2;
const ACT_CLUSTER_TEST: usize = 3;
const ACT_REMOVE: usize = 4;

impl MhPanel {
    #[cfg(test)]
    pub(super) fn scroll_to_top_for_tests(&mut self) {
        self.scroll = 0.0;
    }
    fn preset(&self) -> Preset {
        if self.mac { Preset::Mac } else { Preset::Pc }
    }
    fn app(&self) -> ServerApp {
        ServerApp::ALL[self.server.min(ServerApp::ALL.len() - 1)]
    }
    fn new_host(&self) -> NewHost {
        NewHost { nickname: self.nickname.clone(), preset: self.preset(), address: self.address.clone(), default_port: self.app().default_port(), key: self.key.clone(), confirm: self.confirm.clone(), model: None }
    }
    fn fingerprint(&self, t: &mh::Target) -> String {
        format!("{}|{}", t.authority(), self.key)
    }
}

// ------------------------------------------------------------------ drawing helpers

fn para(g: &mut Gfx, x: f32, y: &mut f32, w: f32, size: f32, col: [u8; 4], text: &str, face: Face, indent: f32) {
    let lines = g.wrap(size, text, face, (w - indent).max(40.0));
    let lh = g.line_height(size, face) + 3.0;
    for l in lines {
        g.text(x + indent, *y, size, col, &l, face);
        *y += lh;
    }
}

fn heading(g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, text: &str) {
    *y += 10.0 * s;
    let t = g.fit(15.5 * s, text, Face::Bold, w);
    g.text(x, *y, 15.5 * s, ACCENT, &t, Face::Bold);
    *y += 24.0 * s;
}

/// Buttons that wrap onto the next row when the row is full.
fn flow(g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, items: &[(String, [u8; 4], Event)]) {
    let size = 13.0 * s;
    let (mut bx, mut by) = (x, *y);
    for (label, col, ev) in items {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if bx > x && bx + bw > x + w {
            bx = x;
            by += size * 2.1 + 8.0 * s;
        }
        bx += g.button(bx, by, size, label, *col, ev.clone()) + 8.0 * s;
    }
    *y = by + size * 2.1 + 10.0 * s;
}

impl SettingsApp {
    pub(super) fn mh_is_open(&self) -> bool {
        self.mh.open
    }

    /// Open the page (administrators). `mac` chooses the preset.
    pub(super) fn mh_open(&mut self, cx: &AppCx, mac: bool) {
        if !cx.session.policy.manage_settings {
            self.fail("Only an administrator can add a model host.");
            return;
        }
        self.mh.open = true;
        self.mh.mac = mac;
        self.mh.scroll = 0.0;
        self.mh_refresh(cx);
    }

    fn mh_close(&mut self) {
        // the typed key and confirmation do not outlive the page
        self.mh.open = false;
        self.mh.key.clear();
        self.mh.confirm.clear();
        self.mh.report = None;
    }

    fn mh_refresh(&mut self, cx: &AppCx) {
        self.mh.hosts = Hosts::new(cx.services.dir()).list();
    }

    fn mh_form(&self) -> Form {
        let port = self.mh.app().default_port();
        Form::new(&format!("Model host: {}", self.mh.preset().title()))
            .text("Nickname", &self.mh.nickname, "for example studio: the provider is named mac-studio (letters and digits)")
            .text("Address", &self.mh.address, &format!("the computer's address on your network, for example 192.168.1.20 or studio.local; the port is {port} unless you type host:port"))
            .secret("API key (optional)", "only if the server needs one; stored in the system vault when you add the host")
            .text("Outside your network?", &self.mh.confirm, &format!("type {} only for an address that is not on your local network (your prompts then cross the internet unencrypted)", mh::PUBLIC_WORD))
    }

    pub(super) fn mh_submit(&mut self, form: &mut Form) -> Result<String, String> {
        let (nick, addr, key, confirm) = (form.value(0), form.value(1), form.raw(2), form.value(3));
        if !nick.is_empty() {
            mh::provider_id(self.mh.preset(), &nick)?;
        }
        if !addr.is_empty() {
            mh::parse_target(&addr, self.mh.app().default_port(), confirm == mh::PUBLIC_WORD)?;
        }
        self.mh.nickname = nick;
        self.mh.address = addr;
        if !key.is_empty() {
            self.mh.key = key;
        }
        self.mh.confirm = confirm;
        self.mh.report = None;
        Ok("Saved. Press Test to check the connection.".into())
    }

    // ------------------------------------------------------------------ events

    /// Everything goes to the page while it is open. Returns true when the event was taken.
    pub(super) fn mh_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        if !self.mh.open {
            return false;
        }
        match ev {
            Event::Key(Key::Esc) => self.mh_close(),
            Event::Key(Key::Up) => self.mh_scroll(-48.0),
            Event::Key(Key::Down) => self.mh_scroll(48.0),
            Event::Key(Key::PageUp) => self.mh_scroll(-320.0),
            Event::Key(Key::PageDown) => self.mh_scroll(320.0),
            Event::Key(Key::Home) => self.mh.scroll = 0.0,
            Event::Key(Key::End) => self.mh.scroll = self.mh.max_scroll,
            Event::Scroll(n) => self.mh_scroll(-(*n as f32) * 48.0),
            Event::Key(Key::Char(c)) => self.mh_key(*c, cx),
            Event::Tab(n) => self.mh_tab(*n, cx),
            _ => {}
        }
        true
    }

    fn mh_scroll(&mut self, d: f32) {
        self.mh.scroll = (self.mh.scroll + d).clamp_safe(0.0, self.mh.max_scroll);
    }

    fn mh_key(&mut self, c: char, cx: &AppCx) {
        match c {
            'M' => self.mh.mac = true,
            'P' => self.mh.mac = false,
            '1'..='4' => self.mh.server = (c as usize) - ('1' as usize),
            'e' => {
                let f = self.mh_form();
                self.open_form(FormKind::ModelHost, f);
            }
            't' => self.mh_test(cx, false),
            'a' => self.mh_test(cx, true),
            's' => self.mh_scan(cx),
            _ => {}
        }
    }

    fn mh_tab(&mut self, n: usize, cx: &AppCx) {
        if (USE_BASE..USE_BASE + 64).contains(&n) {
            if let Some(f) = self.mh.found.get(n - USE_BASE).cloned() {
                self.mh.address = format!("{}:{}", f.ip, f.port);
                self.mh.server = match f.port {
                    11434 => 0,
                    1234 => 1,
                    _ => self.mh.server,
                };
                self.mh.report = None;
                if self.mh.nickname.is_empty() {
                    self.ok("Address filled in. Give it a nickname (Edit details), then Test and Add to Models.");
                } else {
                    self.ok("Address filled in. Press Test.");
                }
            }
        } else if n >= HOST_BASE {
            let (i, act) = ((n - HOST_BASE) / 8, (n - HOST_BASE) % 8);
            if let Some(h) = self.mh.hosts.get(i).cloned() {
                self.mh_host_action(cx, h, act);
            }
        }
    }

    // ------------------------------------------------------------------ actions

    fn mh_limits(&self) -> (Limits, Limits) {
        self.mh.limits.clone().unwrap_or_else(|| (Limits::models(), Limits::chat()))
    }

    /// Test the typed host; with `add`, register it in Models too (testing first when the last test is not for this address).
    fn mh_test(&mut self, cx: &AppCx, add: bool) {
        if !self.admin(cx) {
            return;
        }
        let n = self.mh.new_host();
        let t = match n.target() {
            Ok(t) => t,
            Err(e) => return self.fail(e),
        };
        if add {
            if let Err(e) = mh::provider_id(n.preset, &n.nickname) {
                return self.fail(e);
            }
        }
        let fp = self.mh.fingerprint(&t);
        let known = self.mh.report.clone().filter(|(f, _)| *f == fp).map(|r| r.1);
        let (lim_models, lim_chat) = self.mh_limits();
        let svc = Self::svc(cx).clone();
        cx.audit.record(&cx.session.user, if add { "settings.modelhost.add" } else { "settings.modelhost.test" }, &format!("{} {}", mh::provider_id(n.preset, &n.nickname).unwrap_or_default(), t.authority()));
        self.start(if add { "testing, then adding the host to Models" } else { "testing the model host" }, move || {
            let key = (!n.key.is_empty()).then_some(n.key.as_str());
            let report = match known {
                Some(r) => r,
                None => match mh::test(&t, key, None, n.confirm_public(), &lim_models, &lim_chat) {
                    Ok(r) => r,
                    Err(e) => return JobOut::Mh(MhOut::Tested(fp, Err(e.message()), None)),
                },
            };
            if !add {
                return JobOut::Mh(MhOut::Tested(fp, Ok(report), None));
            }
            match mh::add_to_models(&svc, &n, &report) {
                Ok(h) => {
                    let msg = format!("Added to Models as {} (default model {}). {} It is {}; pick it as the chat model with m on its row.", h.id, report.default_model.as_deref().unwrap_or("none yet"), report.summary(), h.label());
                    JobOut::Mh(MhOut::Tested(fp, Ok(report), Some(msg)))
                }
                Err(e) => JobOut::Mh(MhOut::Tested(fp, Ok(report), Some(format!("ERR {e}")))),
            }
        });
    }

    /// Look for model servers on this machine's own /24, on the four usual ports. Only here, only on the click; nothing is added.
    fn mh_scan(&mut self, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        cx.audit.record(&cx.session.user, "settings.modelhost.scan", "local /24");
        let over = self.mh.scan_override.clone();
        self.start("scanning your local network for model servers", move || {
            let (targets, opts) = match over {
                Some(o) => o,
                None => {
                    let lan = mh::lan_interfaces();
                    let Some((ip, prefix, _)) = lan.first() else {
                        return JobOut::Mh(MhOut::Scan(Err("This machine has no connection to a local network (private address) to scan.".into())));
                    };
                    (mh::scan_targets(*ip, *prefix).into_iter().map(IpAddr::V4).collect(), ScanOpts::default())
                }
            };
            JobOut::Mh(MhOut::Scan(Ok(mh::scan(&targets, &opts, &AtomicBool::new(false)))))
        });
    }

    fn mh_kube(&self, cx: &AppCx) -> Arc<dyn Kube + Send + Sync> {
        match &self.mh.kube {
            Some(k) => k.clone(),
            None => {
                let real: Arc<dyn Kube + Send + Sync> = cx.services.k3sdev.clone();
                real
            }
        }
    }

    fn mh_host_action(&mut self, cx: &AppCx, h: Host, act: usize) {
        if !self.admin(cx) {
            return;
        }
        let svc = Self::svc(cx).clone();
        let (lim_models, lim_chat) = self.mh_limits();
        cx.audit.record(&cx.session.user, "settings.modelhost.action", &format!("{} {}", h.id, ["test", "publish", "unpublish", "cluster-test", "remove"].get(act).unwrap_or(&"?")));
        match act {
            ACT_TEST => {
                let key = svc.secrets.resolve(&format!("vault:{}-key", h.id), None).ok().map(|k| k.to_string());
                let fp = format!("{}|", h.target().authority());
                self.start("testing the model host", move || {
                    JobOut::Mh(MhOut::Tested(fp, mh::test(&h.target(), key.as_deref(), None, h.public_ok, &lim_models, &lim_chat).map_err(|e| e.message()), None))
                });
            }
            ACT_PUBLISH | ACT_UNPUBLISH => {
                let kube = self.mh_kube(cx);
                let publish = act == ACT_PUBLISH;
                self.start(if publish { "publishing the host to the cluster" } else { "removing the host from the cluster" }, move || {
                    let hosts = Hosts::new(svc.dir());
                    let r = if publish { mh::publish(&*kube, svc.dir(), &h).map(|_| format!("Published: pods in the cluster call {} (namespace {}). {}", h.cluster_url(), mh::NAMESPACE, mh::NETWORK_POLICY_NOTE)) } else { mh::unpublish(&*kube, &h.id).map(|_| format!("Removed {} from the cluster.", h.id)) };
                    if r.is_ok() {
                        let _ = hosts.upsert(Host { in_cluster: publish, ..h });
                    }
                    JobOut::Mh(MhOut::Msg(r))
                });
            }
            ACT_CLUSTER_TEST => {
                let kube = self.mh_kube(cx);
                let (timeout, poll) = if self.mh.kube.is_some() { (Duration::from_secs(20), Duration::from_millis(5)) } else { (Duration::from_secs(150), Duration::from_secs(2)) };
                self.start("running a pod in the cluster that calls the host", move || {
                    let r = mh::test_from_cluster(&*kube, svc.dir(), &h, timeout, poll, &|d| std::thread::sleep(d));
                    JobOut::Mh(MhOut::Msg(r.and_then(|c| if c.dns && c.http { Ok(c.message(&h)) } else { Err(c.message(&h)) })))
                });
            }
            ACT_REMOVE => {
                let kube = self.mh_kube(cx);
                self.start("removing the model host", move || {
                    let mut note = String::new();
                    if h.in_cluster {
                        note = match mh::unpublish(&*kube, &h.id) {
                            Ok(()) => " Its cluster Service was removed too.".into(),
                            Err(e) => format!(" Its cluster Service could not be removed ({e})."),
                        };
                    }
                    JobOut::Mh(MhOut::Msg(mh::remove_from_models(&svc, &h.id).map(|_| format!("Removed {} from Models.{note} Its key (if any) stays in the vault until you delete it there.", h.id))))
                });
            }
            _ => {}
        }
    }

    pub(super) fn mh_job_done(&mut self, cx: &AppCx, out: MhOut) {
        self.mh_refresh(cx);
        match out {
            MhOut::Tested(fp, Ok(r), msg) => {
                let text = msg.unwrap_or_else(|| r.summary());
                self.mh.report = Some((fp, r));
                match text.strip_prefix("ERR ") {
                    Some(e) => self.fail(e.to_string()),
                    None => self.ok(text),
                }
            }
            MhOut::Tested(_, Err(e), _) => {
                self.mh.report = None;
                self.fail(e);
            }
            MhOut::Scan(Ok(f)) => {
                self.mh.scanned = true;
                let n = f.len();
                self.mh.found = f;
                self.ok(if n == 0 { "No model server answered on your local network (ports 11434, 8080, 1234, 8000). Is the server running and listening on the network?".to_string() } else { format!("{n} model server(s) found. Nothing was added: press Use on one, then Test and Add to Models.") });
            }
            MhOut::Scan(Err(e)) => self.fail(e),
            MhOut::Msg(Ok(m)) => self.ok(m),
            MhOut::Msg(Err(e)) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------ drawing

    pub(super) fn mh_render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // the page covers the settings behind it: what was drawn there (below the top bar) can no longer be clicked
        g.hits.retain(|h| h.rect.y + h.rect.h <= area.y + 0.5);
        g.rect(area.x, area.y, area.w, area.h, rgb(14, 18, 32));
        let pad = if area.w < 600.0 * s { 14.0 * s } else { 24.0 * s };
        let (x, w) = (area.x + pad, (area.w - 2.0 * pad).max(120.0 * s));
        // header: title, Close
        let title = g.fit(24.0 * s, "Model host on your network", Face::Bold, w);
        g.text(x, area.y + 12.0 * s, 24.0 * s, TEXT, &title, Face::Bold);
        let hy = area.y + 12.0 * s + 34.0 * s;
        let sub = g.fit(13.0 * s, &format!("Use the GPU of another computer: {}", mh::METAL_FACTS[0]), Face::Sans, w);
        g.text(x, hy, 13.0 * s, MUTED, &sub, Face::Sans);
        let body = R::new(area.x, hy + 24.0 * s, area.w, (area.h - (hy - area.y) - 24.0 * s - 44.0 * s).max(60.0 * s));
        let admin = cx.session.policy.manage_settings;
        let (preset, app, key_set) = (self.mh.preset(), self.mh.app(), !self.mh.key.is_empty());
        let (nick, addr, confirm_set) = (self.mh.nickname.clone(), self.mh.address.clone(), !self.mh.confirm.is_empty());
        let (report, found, scanned, hosts) = (self.mh.report.clone(), self.mh.found.clone(), self.mh.scanned, self.mh.hosts.clone());
        let providers = Self::providers(cx);
        let scroll = self.mh.scroll;
        let end = g.clipped(body, |g| {
            let mut y = body.y - scroll;
            let tsize = 13.5 * s;
            para(g, x, &mut y, w, tsize, TEXT, "The model runs on that computer's GPU. IYAKKI only sends it chat requests over HTTP on your network and shows the answers. IYAKKI never installs anything on that computer: you do, with the steps below, and you can stop it any time.", Face::Sans, 0.0);

            heading(g, x, &mut y, w, s, "1. Which computer");
            let items: Vec<(String, [u8; 4], Event)> = Preset::ALL.iter().map(|p| (p.title().to_string(), if *p == preset { ACCENT } else { MUTED }, Event::Key(Key::Char(if *p == Preset::Mac { 'M' } else { 'P' })))).collect();
            flow(g, x, &mut y, w, s, &items);

            heading(g, x, &mut y, w, s, &format!("2. On the {}, install one of these (you do this)", preset.machine()));
            let items: Vec<(String, [u8; 4], Event)> = ServerApp::ALL.iter().enumerate().map(|(i, a)| (format!("{} {}", i + 1, a.title()), if *a == app { ACCENT } else { MUTED }, Event::Key(Key::Char(char::from(b'1' + i as u8))))).collect();
            flow(g, x, &mut y, w, s, &items);
            para(g, x, &mut y, w, 12.5 * s, MUTED, &format!("Licence: {}", app.licence()), Face::Sans, 0.0);
            y += 4.0 * s;
            for step in app.steps(preset) {
                para(g, x, &mut y, w, tsize, TEXT, &step, Face::Sans, 12.0 * s);
                y += 2.0 * s;
            }
            para(g, x, &mut y, w, 12.5 * s, MUTED, app.bind_tip(), Face::Sans, 12.0 * s);
            y += 6.0 * s;
            para(g, x, &mut y, w, 12.5 * s, crate::fx::AMBER, &format!("Security: {}", mh::SECURITY), Face::Sans, 0.0);

            heading(g, x, &mut y, w, s, "3. Tell IYAKKI where it is");
            let row = |g: &mut Gfx, y: &mut f32, label: &str, value: &str, col: [u8; 4]| {
                g.text(x, *y, 13.0 * s, MUTED, label, Face::Sans);
                let lw = 120.0 * s;
                let v = g.fit(13.0 * s, value, Face::Bold, (w - lw).max(60.0 * s));
                g.text(x + lw, *y, 13.0 * s, col, &v, Face::Bold);
                *y += 22.0 * s;
            };
            row(g, &mut y, "Nickname", &if nick.is_empty() { "not set".to_string() } else { format!("{nick}  (provider {})", mh::provider_id(preset, &nick).unwrap_or_else(|_| "invalid".into())) }, if nick.is_empty() { DIM } else { TEXT });
            row(g, &mut y, "Address", &if addr.is_empty() { format!("not set (port {} unless you type host:port)", app.default_port()) } else { addr.clone() }, if addr.is_empty() { DIM } else { TEXT });
            row(g, &mut y, "API key", if key_set { "typed (stored in the system vault when you add the host)" } else { "none" }, if key_set { OK } else { DIM });
            if confirm_set {
                row(g, &mut y, "Outside network", "confirmation typed", crate::fx::AMBER);
            }
            y += 4.0 * s;
            let mut acts: Vec<(String, [u8; 4], Event)> = vec![("Edit details".into(), TEXT, Event::Key(Key::Char('e'))), ("Test".into(), ACCENT, Event::Key(Key::Char('t'))), ("Add to Models".into(), OK, Event::Key(Key::Char('a'))), ("Scan this network".into(), TEXT, Event::Key(Key::Char('s')))];
            if !admin {
                acts.truncate(0);
            }
            flow(g, x, &mut y, w, s, &acts);
            if let Some((_, r)) = &report {
                para(g, x, &mut y, w, tsize, OK, &r.summary(), Face::Bold, 0.0);
                if let Some(c) = &r.chat {
                    para(g, x, &mut y, w, 12.5 * s, MUTED, &format!("The model said: {}", c.text), Face::Sans, 0.0);
                }
                let list: Vec<String> = r.models.iter().take(8).cloned().collect();
                para(g, x, &mut y, w, 12.5 * s, MUTED, &format!("Models: {}{}", list.join(", "), if r.models.len() > 8 { ", ..." } else { "" }), Face::Sans, 0.0);
            }
            para(g, x, &mut y, w, 12.0 * s, DIM, "Scan looks only at your own local network (the /24 of this machine's address, at most 254 addresses) on ports 11434, 8080, 1234 and 8000, only when you press it, and never adds anything by itself.", Face::Sans, 0.0);
            if scanned {
                y += 4.0 * s;
                if found.is_empty() {
                    para(g, x, &mut y, w, tsize, MUTED, "The last scan found no model server.", Face::Sans, 0.0);
                }
                for (i, f) in found.iter().enumerate() {
                    let line = g.fit(13.0 * s, &f.line(), Face::Sans, (w - 90.0 * s).max(60.0 * s));
                    g.text(x, y + 4.0 * s, 13.0 * s, TEXT, &line, Face::Sans);
                    g.button(x + (w - 80.0 * s).max(0.0), y - 2.0 * s, 13.0 * s, "Use", ACCENT, Event::Tab(USE_BASE + i));
                    y += 30.0 * s;
                }
            }

            heading(g, x, &mut y, w, s, &format!("4. Your model hosts ({})", hosts.len()));
            if hosts.is_empty() {
                para(g, x, &mut y, w, tsize, MUTED, "None yet. Several computers are fine: each gets a nickname.", Face::Sans, 0.0);
            }
            for (i, h) in hosts.iter().enumerate() {
                let in_models = providers.iter().any(|p| p.id == h.id);
                para(g, x, &mut y, w, 14.0 * s, TEXT, &format!("{}  {}:{}", h.id, h.address, h.port), Face::Bold, 0.0);
                para(g, x, &mut y, w, 12.0 * s, MUTED, &format!("{} · {} · {}", h.label(), if in_models { "in Models" } else { "not in Models" }, if h.in_cluster { format!("pods call {}", h.cluster_url()) } else { "not published to the cluster".to_string() }), Face::Sans, 0.0);
                let b = HOST_BASE + i * 8;
                let mut items: Vec<(String, [u8; 4], Event)> = vec![("Test".into(), ACCENT, Event::Tab(b + ACT_TEST))];
                if admin {
                    items.push(if h.in_cluster { ("Remove from cluster".into(), TEXT, Event::Tab(b + ACT_UNPUBLISH)) } else { ("Publish to cluster".into(), TEXT, Event::Tab(b + ACT_PUBLISH)) });
                    items.push(("Test from the cluster".into(), TEXT, Event::Tab(b + ACT_CLUSTER_TEST)));
                    items.push(("Remove".into(), ERR, Event::Tab(b + ACT_REMOVE)));
                }
                flow(g, x, &mut y, w, s, &items);
            }
            para(g, x, &mut y, w, 12.0 * s, DIM, mh::NETWORK_POLICY_NOTE, Face::Sans, 0.0);

            heading(g, x, &mut y, w, s, "What to know");
            for f in mh::METAL_FACTS {
                para(g, x, &mut y, w, 12.5 * s, MUTED, f, Face::Sans, 12.0 * s);
            }
            y += 6.0 * s;
            g.text(x, y, 13.5 * s, TEXT, "Model size by memory (4-bit models; a guide)", Face::Bold);
            y += 24.0 * s;
            for l in mh::ram_table_lines() {
                para(g, x, &mut y, w, 12.5 * s, MUTED, &l, Face::Sans, 12.0 * s);
            }
            y + 20.0 * s - (body.y - scroll)
        });
        self.mh.max_scroll = (end - body.h).max(0.0);
        self.mh.scroll = self.mh.scroll.min(self.mh.max_scroll);
        crate::fx::scrollbar(g, R::new(body.x + body.w - 8.0 * s, body.y, 4.0 * s, body.h), s, self.mh.scroll as usize, body.h as usize, end as usize, ACCENT);
        // footer: Close, hints, and the job or status line
        let fy = area.y + area.h - 40.0 * s;
        let bw = g.button(x, fy - 4.0 * s, 13.0 * s, "Close", TEXT, Event::Key(Key::Esc));
        let hint = g.fit(11.5 * s, "e edit · t test · a add · s scan · 1-4 server · M / P computer · Up/Down scroll · Esc close", Face::Sans, (w - bw - 12.0 * s).max(40.0 * s));
        g.text(x + bw + 12.0 * s, fy - 2.0 * s, 11.5 * s, DIM, &hint, Face::Sans);
        let sy = fy + 20.0 * s;
        if let Some(j) = &self.job {
            g.text(x, sy, 13.5 * s, MUTED, &format!("{}{}", j.label, ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
        } else if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, sy, w, s, m, *ok);
        }
    }
}
