//! Settings -> Network -> Share internet (administrators): give other computers the internet this one has, over an Ethernet, USB or
//! Thunderbolt / USB-C cable. The page picks the connection that has the internet and the cable to share through from what the machine
//! has, starts and stops the service (`iyakki_agent::sharing`, which runs inside the system and does the real work), and shows who is
//! connected. Wi-Fi hotspot appears only when a card can be an access point; on this build it says it is not available.
//!
//! Keys (Network page, `i` opens it): `u`/`d` change list (internet source / share through), Up/Down choose, `s` or Enter start/stop (the first press shows what it
//! means and asks again), `+`/`-` hours until it switches itself off when nobody uses it, `o` devices can reach each other, `l` devices
//! can reach my local network, `b` or Esc back.

use crate::safeclamp::SafeClamp;
use super::*;
use iyakki_agent::sharing::control::{self, Backend, FileBackend};
use iyakki_agent::sharing::plan::{apply_policy, make_plan};
use iyakki_agent::sharing::policy::Who;
use iyakki_agent::sharing::view::{choices, warnings, Choices};
use iyakki_agent::sharing::{human_duration, IfaceInfo, Request, State, Status, DEFAULT_IDLE_HOURS, MAX_IDLE_HOURS};
use std::sync::Arc;
use std::time::Instant;

pub(super) struct SharePanel {
    pub(super) open: bool,
    /// The machine (the real one unless a test puts a fake here).
    pub(super) backend: Option<Arc<dyn Backend>>,
    /// How long a look at the machine stays fresh (0 in tests).
    pub(super) ttl_ms: u128,
    seen: Option<(Instant, Vec<IfaceInfo>, bool, Status)>,
    pub(super) col: usize,
    pub(super) up_sel: usize,
    pub(super) down_sel: usize,
    pub(super) hours: u8,
    pub(super) isolate: bool,
    pub(super) allow_lan: bool,
    /// The first press of Start was made and shown; the next one starts.
    pub(super) armed: bool,
    /// The policy lets this person start (updated every time the page is drawn or opened).
    pub(super) can_start: bool,
    pub(super) scroll: f32,
    pub(super) max: f32,
}

impl Default for SharePanel {
    fn default() -> SharePanel {
        SharePanel { open: false, backend: None, ttl_ms: 2000, seen: None, col: 1, up_sel: 0, down_sel: 0, hours: DEFAULT_IDLE_HOURS, isolate: true, allow_lan: false, armed: false, can_start: true, scroll: 0.0, max: 0.0 }
    }
}

impl SharePanel {
    fn backend(&mut self, cx: &AppCx) -> Arc<dyn Backend> {
        self.backend.get_or_insert_with(|| Arc::new(FileBackend { data: cx.services.dir().to_path_buf() })).clone()
    }

    fn look(&mut self, cx: &AppCx) -> (Vec<IfaceInfo>, bool, Status) {
        let fresh = self.seen.as_ref().is_some_and(|(t, ..)| t.elapsed().as_millis() < self.ttl_ms);
        if !fresh {
            let b = self.backend(cx);
            self.seen = Some((Instant::now(), b.interfaces(), b.wifi_ap(), b.status()));
        }
        let (_, i, w, s) = self.seen.clone().expect("just set");
        (i, w, s)
    }

    pub(super) fn choices(&mut self, cx: &AppCx) -> Choices {
        let (i, w, _) = self.look(cx);
        choices(&i, w)
    }

    pub(super) fn status(&mut self, cx: &AppCx) -> Status {
        self.look(cx).2
    }

    fn forget(&mut self) {
        self.seen = None;
    }

    fn clamp(&mut self, c: &Choices) {
        self.up_sel = self.up_sel.min(c.uplinks.len().saturating_sub(1));
        self.down_sel = self.down_sel.min(c.downs.len().saturating_sub(1));
    }
}

impl SettingsApp {
    /// The form of key n: a new hotspot name and, when typed, a new password. The password is never audited or shown again here.
    pub(super) fn hotspot_wifi_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Err("Only administrators can change the hotspot.".into());
        }
        let b = self.share.backend(cx);
        let (old_ssid, old_pass) = b.hotspot().ok_or("The hotspot's name and password cannot be read on this computer.")?;
        let ssid = form.value(0).trim().to_string();
        let typed = form.raw(1);
        let pass = if typed.is_empty() { old_pass } else { typed };
        b.set_hotspot(&ssid, &pass)?;
        cx.audit.record(&cx.session.user, "settings.share.wifi", &format!("name {old_ssid} -> {ssid}; password {}", if form.raw(1).is_empty() { "kept" } else { "changed" }));
        self.share.forget();
        Ok("Saved. The new Wi-Fi name and password apply the next time sharing starts.".into())
    }

    pub(super) fn share_buttons(&self) -> Vec<Btn> {
        if !self.share.can_start {
            return vec![("Back", TEXT, 'b')];
        }
        vec![("Start / Stop", ACCENT, 's'), ("Hours +", TEXT, '+'), ("Hours -", TEXT, '-'), ("Devices see each other", TEXT, 'o'), ("Reach my network", TEXT, 'l'), ("Back", TEXT, 'b')]
    }

    /// A click or touch on a row of the two lists: the same as moving there with the keys.
    pub(super) fn share_pick(&mut self, row: usize, list: char) {
        if list == 'U' {
            self.share.col = 0;
            self.share.up_sel = row;
        } else {
            self.share.col = 1;
            self.share.down_sel = row;
        }
        self.share.armed = false;
    }

    pub(super) fn share_open(&mut self, cx: &AppCx) {
        self.share.open = true;
        self.share.armed = false;
        self.share.forget();
        let b = self.share.backend(cx);
        self.share.can_start = b.policy().may_start_any(cx.session.policy.manage_settings);
        let req = b.request();
        if req.on {
            self.share.hours = req.idle_hours;
            self.share.isolate = req.isolate;
            self.share.allow_lan = req.allow_lan;
        }
    }

    /// Start or stop. The first key press of Start explains and asks again; a click or tap starts at once.
    pub(super) fn share_toggle(&mut self, cx: &AppCx) {
        let is_admin = cx.session.policy.manage_settings;
        let b = self.share.backend(cx);
        let st = b.status();
        let prev = b.request();
        let policy = b.policy();
        if st.state == State::On || prev.on && st.state != State::Failed && st.request_id != prev.id {
            if !is_admin && prev.by != cx.session.user {
                return self.fail("Only an administrator or the person who started sharing can stop it.");
            }
            let r = control::stop_request(&prev, &cx.session.user);
            match b.submit(&r) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.share", "stop");
                    self.share.forget();
                    self.share.armed = false;
                    self.ok("Sharing is being switched off.");
                }
                Err(e) => self.fail(format!("Could not switch sharing off: {e}")),
            }
            return;
        }
        if !policy.may_start_any(is_admin) {
            return self.fail(policy.may_start(iyakki_agent::sharing::LinkKind::Ethernet, is_admin).err().map(|e| e.line()).unwrap_or_else(|| iyakki_agent::sharing::policy::BLOCKED_LINE.into()));
        }
        let c = self.share.choices(cx);
        self.share.clamp(&c);
        if !c.uplinks.iter().any(|u| u.usable) {
            return self.fail(iyakki_agent::sharing::ShareError::NoDefaultRoute.line());
        }
        let Some(up) = c.uplinks.get(self.share.up_sel) else {
            return self.fail(iyakki_agent::sharing::ShareError::NoDefaultRoute.line());
        };
        if !up.usable {
            return self.fail(format!("{} cannot be used: {}", up.name, up.why));
        }
        let Some(down) = c.downs.get(self.share.down_sel) else {
            return self.fail(iyakki_agent::sharing::ShareError::NoDownstream.line());
        };
        if !down.usable {
            return self.fail(format!("{} cannot be used: {}.", down.name, down.why));
        }
        let r = control::start_request(&prev, Some(up.name.clone()), Some(down.name.clone()), self.share.hours, self.share.isolate, self.share.allow_lan, &cx.session.user, is_admin);
        let (ifs, ap, _) = self.share.look(cx);
        if let Err(e) = make_plan(&ifs, &r, ap, &[]).and_then(|mut p| apply_policy(&mut p, &policy, &r)) {
            self.share.armed = false;
            return self.fail(e.line());
        }
        // A click or tap on the Start button is already a deliberate act (the page shows the choices): one is enough. Keys keep the two-step.
        if !self.share.armed && !crate::ui::from_click() {
            self.share.armed = true;
            let w = warnings(self.share.isolate, self.share.hours);
            return self.ok(format!("{} Press s again to start sharing {} through {}.", w.join(" "), up.name, down.name));
        }
        self.share.armed = false;
        match b.submit(&r) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.share", &format!("start {} -> {}, isolation {}, {} h", up.name, down.name, if r.isolate { "on" } else { "off" }, r.idle_hours));
                self.share.forget();
                self.ok(format!("Starting to share {} through {}. The list of devices fills in as they connect.", up.name, down.name));
            }
            Err(e) => self.fail(format!("Could not start sharing: {e}")),
        }
    }

    pub(super) fn handle_share(&mut self, k: &Key, cx: &AppCx) {
        let c = self.share.choices(cx);
        self.share.clamp(&c);
        let was_armed = self.share.armed;
        match k {
            Key::Esc | Key::Char('b') => {
                self.share.open = false;
                self.share.armed = false;
                return;
            }
            Key::Char('u') => self.share.col = 0,
            Key::Char('d') => self.share.col = 1,
            Key::Up | Key::Down => {
                let d = if matches!(k, Key::Up) { -1i32 } else { 1 };
                let (sel, len) = if self.share.col == 0 { (&mut self.share.up_sel, c.uplinks.len()) } else { (&mut self.share.down_sel, c.downs.len()) };
                *sel = (*sel as i32 + d).clamp_safe(0, len.saturating_sub(1) as i32) as usize;
            }
            Key::PageUp => self.share.scroll = (self.share.scroll - 300.0).max(0.0),
            Key::PageDown => self.share.scroll = (self.share.scroll + 300.0).min(self.share.max),
            Key::Char('s') | Key::Enter => {
                self.share_toggle(cx);
                return;
            }
            Key::Char('w') => {
                if self.admin(cx) {
                    let b = self.share.backend(cx);
                    let next = match b.hotspot_band().as_str() {
                        "auto" => "2.4",
                        "2.4" => "5",
                        _ => "auto",
                    };
                    match b.set_hotspot_band(next) {
                        Ok(()) => self.ok(format!("Hotspot band: {next}. It applies the next time sharing starts.")),
                        Err(e) => self.fail(e),
                    }
                }
            }
            Key::Char('n') => {
                if self.admin(cx) {
                    let (ssid, _) = self.share.backend(cx).hotspot().unwrap_or_default();
                    let f = Form::new("Wi-Fi hotspot name and password")
                        .with_note("Phones and laptops join this network. The change applies the next time sharing starts; a running hotspot keeps its old name and password until then.")
                        .text("Network name", &ssid, "1 to 32 characters")
                        .secret("Password", "8 to 63 characters; leave empty to keep the current one");
                    self.open_form(FormKind::HotspotWifi, f);
                }
            }
            Key::Char('p') => {
                if self.admin(cx) {
                    let b = self.share.backend(cx);
                    let mut p = b.policy();
                    p.wired = match p.wired {
                        Who::Admins => Who::Users,
                        Who::Users => Who::Nobody,
                        Who::Nobody => Who::Admins,
                    };
                    let w = p.wired.word();
                    match b.set_policy(&p, &cx.session.user, true) {
                        Ok(line) => {
                            cx.audit.record(&cx.session.user, "settings.share.policy", &line);
                            self.share.forget();
                            self.share.can_start = true;
                            self.ok(format!("Wired and USB sharing may now be started by: {w}. A running session that this no longer allows stops within seconds."));
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            Key::Char('+') | Key::Char('=') => {
                if self.admin(cx) {
                    self.share.hours = (self.share.hours + 1).min(MAX_IDLE_HOURS);
                }
            }
            Key::Char('-') => {
                if self.admin(cx) {
                    self.share.hours = self.share.hours.saturating_sub(1).max(1);
                }
            }
            Key::Char('o') => {
                if self.admin(cx) {
                    self.share.isolate = !self.share.isolate;
                    let m = if self.share.isolate { "Devices on the cable cannot reach each other." } else { "Devices on the cable can now reach each other (takes effect at the next start)." };
                    self.ok(m);
                }
            }
            Key::Char('l') => {
                if self.admin(cx) {
                    self.share.allow_lan = !self.share.allow_lan;
                    let m = if self.share.allow_lan { "Devices may reach this computer's local network (takes effect at the next start)." } else { "Devices cannot reach this computer's local network." };
                    self.ok(m);
                }
            }
            _ => {}
        }
        if was_armed {
            self.share.armed = false;
        }
    }

    pub(super) fn render_share(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let c = self.share.choices(cx);
        self.share.clamp(&c);
        let st = self.share.status(cx);
        let on = st.state == State::On;
        let admin = cx.session.policy.manage_settings;
        let policy = self.share.backend(cx).policy();
        self.share.can_start = policy.may_start_any(admin);
        let mut lines: Vec<(String, f32, [u8; 4], Face)> = Vec::new();
        // lines that can be clicked or touched: the choices of the two lists (the keys u/d and Up/Down do the same)
        let mut line_hits: std::collections::HashMap<usize, Event> = std::collections::HashMap::new();
        lines.push(("Share internet".into(), 15.0, TEXT, Face::Bold));
        let col = match st.state {
            State::On => rgb(255, 196, 94),
            State::Failed => ERR,
            State::Off => OK,
        };
        lines.push((st.line(), 14.0, col, Face::Bold));
        if !self.share.can_start {
            lines.push((policy.may_start(iyakki_agent::sharing::LinkKind::Ethernet, admin).err().map(|e| e.line()).unwrap_or_default(), 13.0, ERR, Face::Sans));
        }
        lines.push((policy.changed_line(), 12.0, DIM, Face::Sans));
        if admin {
            line_hits.insert(lines.len(), Event::Key(Key::Char('p')));
            lines.push((format!("Who may start wired and USB sharing: {} (p changes it; or click here)", policy.wired.word()), 12.0, DIM, Face::Sans));
        }
        lines.push(("Internet comes from".into(), 14.0, if self.share.col == 0 { ACCENT } else { TEXT }, Face::Bold));
        if !c.uplinks.iter().any(|u| u.usable) {
            lines.push(("None: no connection on this computer has a route to the internet, so there is nothing to share yet.".into(), 13.0, ERR, Face::Sans));
        }
        for (i, u) in c.uplinks.iter().enumerate() {
            line_hits.insert(lines.len(), Event::Sub(i as u8, Key::Char('U')));
            let mark = if i == self.share.up_sel { "> " } else { "  " };
            lines.push((format!("{mark}{}", u.label), 13.0, if i == self.share.up_sel { ACCENT } else { MUTED }, Face::Sans));
        }
        lines.push(("Share it through".into(), 14.0, if self.share.col == 1 { ACCENT } else { TEXT }, Face::Bold));
        if c.downs.iter().all(|d| !d.usable) {
            lines.push(("No free cable device yet: plug in an Ethernet cable, a USB network link or a Thunderbolt / USB-C cable to the other computer.".into(), 13.0, MUTED, Face::Sans));
        }
        for (i, d) in c.downs.iter().enumerate() {
            line_hits.insert(lines.len(), Event::Sub(i as u8, Key::Char('D')));
            let mark = if i == self.share.down_sel { "> " } else { "  " };
            let tail = if d.usable { String::new() } else { format!(" (cannot be used: {})", d.why) };
            lines.push((format!("{mark}{}{tail}", d.label), 13.0, if i == self.share.down_sel { ACCENT } else { MUTED }, Face::Sans));
        }
        if let Some(w) = &c.wifi_line {
            lines.push((format!("  {w}"), 12.0, DIM, Face::Sans));
        }
        let mut qr_at: Option<(usize, iyakki_agent::qr::Qr)> = None;
        if admin && c.downs.get(self.share.down_sel).is_some_and(|d| d.name == iyakki_agent::wifi_ap::AP_IFACE) {
            if let Some((ssid, pass)) = self.share.backend(cx).hotspot() {
                if let Some(q) = iyakki_agent::qr::encode(iyakki_agent::qr::wifi_payload(&ssid, &pass).as_bytes()) {
                    qr_at = Some((lines.len() + 3, q));
                }
                lines.push(("Wi-Fi hotspot".into(), 14.0, TEXT, Face::Bold));
                line_hits.insert(lines.len(), Event::Key(Key::Char('n')));
                lines.push((format!("Network name: {ssid}   (n or a click changes the name and password)"), 13.0, TEXT, Face::Mono));
                lines.push((format!("Password: {pass}"), 13.0, TEXT, Face::Mono));
                let band = match self.share.backend(cx).hotspot_band().as_str() {
                    "2.4" => "2.4 GHz (longer range)",
                    "5" => "5 GHz (faster)",
                    _ => "Auto (the band of this computer's own Wi-Fi)",
                };
                if qr_at.is_some() {
                    lines.push(("Scan with the phone camera to join".into(), 12.0, MUTED, Face::Sans));
                }
                line_hits.insert(lines.len(), Event::Key(Key::Char('w')));
                lines.push((format!("Band: {band}  (w or a click changes it; takes effect when sharing starts)"), 13.0, TEXT, Face::Sans));
                lines.push(("It uses this computer's own Wi-Fi card next to its connection to your router. The internet comes from the source above; devices that join get the same protection as on a cable (isolated from each other unless you turn that off). Change the name or password with n (or: iyakki share wifi-set NAME PASSWORD).".into(), 12.0, DIM, Face::Sans));
            }
        }
        let (h, iso, lan) = if on { (st.idle_hours, st.isolate, self.share.allow_lan) } else { (self.share.hours, self.share.isolate, self.share.allow_lan) };
        lines.push(("Options".into(), 14.0, TEXT, Face::Bold));
        line_hits.insert(lines.len(), Event::Key(Key::Char('+')));
        lines.push((format!("Switch off by itself after {h} h with nobody using it  (+ and -, or click for +1; 1 to {MAX_IDLE_HOURS})"), 13.0, MUTED, Face::Sans));
        line_hits.insert(lines.len(), Event::Key(Key::Char('o')));
        lines.push((format!("Devices can reach each other: {}  (o or click)", if iso { "no" } else { "yes" }), 13.0, if iso { MUTED } else { ERR }, Face::Sans));
        line_hits.insert(lines.len(), Event::Key(Key::Char('l')));
        lines.push((format!("Devices can reach my local network: {}  (l or click)", if lan { "yes" } else { "no" }), 13.0, if lan { ERR } else { MUTED }, Face::Sans));
        lines.push(("What this means".into(), 14.0, TEXT, Face::Bold));
        for w in warnings(iso, h) {
            lines.push((w, 12.0, rgb(255, 196, 94), Face::Sans));
        }
        if on {
            lines.push(("Devices that were given an address".into(), 14.0, TEXT, Face::Bold));
            lines.push(("A device that has left stays in this list until its address runs out.".into(), 12.0, DIM, Face::Sans));
            if st.clients.is_empty() {
                lines.push(("No device has asked for an address yet.".into(), 13.0, MUTED, Face::Sans));
            }
            for cl in &st.clients {
                let name = if cl.name.is_empty() { "unknown device" } else { &cl.name };
                lines.push((format!("{name}   {}   address kept for {}", cl.ip, human_duration(cl.expires_in)), 13.0, TEXT, Face::Mono));
            }
            lines.push((format!("Since it started: {} bytes from the devices, {} bytes to them.", st.rx_bytes, st.tx_bytes), 12.0, DIM, Face::Sans));
        }
        if !admin {
            lines.push(("Only administrators can change the options.".into(), 13.0, MUTED, Face::Sans));
        }
        lines.push(("u and d change list, Up and Down choose, s starts or stops, b goes back.".into(), 12.0, DIM, Face::Sans));
        let mut y = a.y - self.share.scroll;
        let mut used = 0.0f32;
        g.clipped(a, |g| {
            for (li, (t, size, c, face)) in lines.iter().enumerate() {
                let sz = size * s;
                if *size >= 14.0 && face == &Face::Bold {
                    y += 6.0 * s;
                    used += 6.0 * s;
                }
                if let Some((_, q)) = qr_at.as_ref().filter(|(i, _)| *i == li) {
                    // black modules on a white square with the 4-module quiet zone, whole pixels so every edge stays sharp
                    let m = (4.0 * s).round().max(2.0);
                    let side = (q.size + 8) as f32 * m;
                    let (x0, y0) = (a.x.round(), (y + 4.0 * s).round());
                    g.rect(x0, y0, side, side, [255, 255, 255, 255]);
                    for qy in 0..q.size {
                        let mut qx = 0;
                        while qx < q.size {
                            if q.get(qx, qy) {
                                let start = qx;
                                while qx < q.size && q.get(qx, qy) {
                                    qx += 1;
                                }
                                g.rect(x0 + (start + 4) as f32 * m, y0 + (qy + 4) as f32 * m, (qx - start) as f32 * m, m, [0, 0, 0, 255]);
                            } else {
                                qx += 1;
                            }
                        }
                    }
                    y += side + 8.0 * s;
                    used += side + 8.0 * s;
                }
                let top = y;
                for w in g.wrap(sz, t, *face, a.w) {
                    g.text(a.x, y, sz, *c, &w, *face);
                    y += sz * 1.45;
                    used += sz * 1.45;
                }
                if let Some(ev) = line_hits.get(&li) {
                    // the whole row is the target, tall enough for a finger
                    let h = (y - top).max(30.0 * s);
                    g.hit(R::new(a.x, top - 2.0 * s, a.w, h), ev.clone(), t);
                    if h > y - top {
                        y = top + h;
                        used += h - (y - top);
                    }
                }
            }
        });
        self.share.max = (used + 12.0 * s - a.h).max(0.0);
        self.share.scroll = self.share.scroll.clamp_safe(0.0, self.share.max);
    }
}

#[allow(dead_code)]
fn _types(_: Request) {}
