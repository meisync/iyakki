//! Settings → Time (zone, 12/24-hour clock, seconds, network time) and Settings → Host name.
//!
//! **Time.** The zone and the network time are the machine's (administrators); the 12/24-hour choice and seconds are each
//! person's own. The zone list comes from the machine's zone files when it has them, else from a built-in table of the common
//! zones, so an image without a time zone package still works. The network time is a small SNTP client (`iyakki-agent`'s `ntp`):
//! it sets the clock once the network is up and every few hours, and never puts it back by more than a few seconds
//! without asking: that is the "Set the clock to the network time" button.
//!
//! **Host name.** Shows the name, lets an administrator change it (the installer's rules), applies it to the running system and
//! saves it in `system.conf` for the next boots. A desktop window shows the computer's name and changes nothing; the boot option
//! `iyakki.hostname=` locks the page.

use super::*;
use iyakki_agent::clock::{self, Zone};

/// Changes the clock for a sync (the real one needs root; tests record).
pub(super) type ClockSetter = Arc<dyn Fn(f64) -> Result<(), String> + Send + Sync>;
/// Looks the time zone up (the real one asks an internet service in the background; tests answer at once).
pub(super) type ZoneDetector = Arc<dyn Fn() -> Result<(String, &'static str), String> + Send + Sync>;
/// Changes the host name (the real one needs root; tests record).
pub(super) type NameSetter = Arc<dyn Fn(&str, &std::path::Path) -> Result<(), String> + Send + Sync>;

pub(super) struct TimePanel {
    /// May this page change the machine (bare metal) or only look (a window on someone's computer)?
    pub(super) writable: bool,
    pub(super) set_clock: ClockSetter,
    /// Servers to ask instead of the usual ones (tests use a fake one on this computer).
    pub(super) servers: Option<Vec<String>>,
    /// Tell the rest of the process (the assistant, the calendar) about a new zone. Off in tests, which share one process.
    pub(super) global_zone: bool,
    /// "Detect automatically": `None` asks the internet service on a thread of its own ([`iyakki_agent::tzdetect`]); tests answer.
    pub(super) detect: Option<ZoneDetector>,
}

impl Default for TimePanel {
    fn default() -> TimePanel {
        TimePanel { writable: !hf_on(), set_clock: Arc::new(iyakki_agent::ntp::set_clock), servers: None, global_zone: true, detect: None }
    }
}

pub(super) struct NamePanel {
    pub(super) conf: std::path::PathBuf,
    pub(super) etc: std::path::PathBuf,
    pub(super) cmdline: String,
    pub(super) writable: bool,
    pub(super) apply: NameSetter,
    /// The name being typed, not yet applied or saved.
    pub(super) draft: Option<String>,
}

impl Default for NamePanel {
    fn default() -> NamePanel {
        NamePanel { conf: iyakki_zram::CONF_PATH.into(), etc: "/etc/hostname".into(), cmdline: std::fs::read_to_string("/proc/cmdline").unwrap_or_default(), writable: !hf_on(), apply: Arc::new(|n, p| iyakki_devctl::apply_hostname(n, p)), draft: None }
    }
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 90 => "a moment ago".into(),
        s if s < 5400 => format!("{} minutes ago", s / 60),
        s => format!("{} hours ago", s / 3600),
    }
}

impl SettingsApp {
    /// Use other seams for the Time page (tests).
    pub fn with_time(mut self, writable: bool, set_clock: ClockSetter, servers: Option<Vec<String>>) -> SettingsApp {
        self.time = TimePanel { writable, set_clock, servers, global_zone: false, detect: None };
        self
    }

    /// Answer "Detect automatically" lookups with `detect` instead of the internet service (tests).
    pub fn with_zone_detector(mut self, detect: ZoneDetector) -> SettingsApp {
        self.time.detect = Some(detect);
        self
    }

    /// Use other files and seams for the Host name page (tests).
    pub fn with_host_name(mut self, conf: std::path::PathBuf, etc: std::path::PathBuf, cmdline: &str, writable: bool, apply: NameSetter) -> SettingsApp {
        self.hn = NamePanel { conf, etc, cmdline: cmdline.into(), writable, apply, draft: None };
        self
    }

    // ------------------------------------------------------------------------------------------------ Time

    fn time_settings(cx: &AppCx) -> iyakki_agent::settings::TimeSettings {
        Self::svc(cx).settings.lock().map(|s| s.get().time.clone()).unwrap_or_default()
    }

    fn time_update(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut iyakki_agent::settings::TimeSettings)) -> bool {
        let r = Self::svc(cx).settings.lock().unwrap().update(|s| f(&mut s.time));
        match r {
            Ok(()) => {
                if self.time.global_zone {
                    clock::set_chosen_zone(&Self::time_settings(cx).zone);
                }
                cx.audit.record(&cx.session.user, "settings.time", what);
                true
            }
            Err(e) => {
                self.fail(e.to_string());
                false
            }
        }
    }

    pub(super) fn handle_time(&mut self, k: &Key, cx: &AppCx) {
        let (dir, user) = (Self::svc(cx).dir().to_path_buf(), cx.session.user.clone());
        match k {
            // your own clock face
            Key::Char('h') => {
                let (h12, secs) = crate::uisize::load_clock(&dir, &user);
                crate::uisize::save_clock(&dir, &user, !h12, secs);
                self.ok(if h12 { "24-hour clock." } else { "12-hour clock with AM and PM." });
            }
            Key::Char('s') => {
                let (h12, secs) = crate::uisize::load_clock(&dir, &user);
                crate::uisize::save_clock(&dir, &user, h12, !secs);
                self.ok(if secs { "The clock shows hours and minutes." } else { "The clock shows seconds too." });
            }
            // the machine's time: administrators
            Key::Char('z' | 'u' | 'n' | 'c' | 'y' | 'b' | 'a' | 'd') => {
                if !self.admin(cx) {
                    return;
                }
                if !self.time.writable {
                    return self.fail("This window follows the clock and time zone of the computer it runs on. They are changed on the IYAKKI system itself.");
                }
                match k {
                    Key::Char('z') => {
                        let items = clock::zone_names();
                        self.modal = Some(Modal::Pick(PickKind::Zone, Picker::new("Time zone (type to search)", items)));
                    }
                    Key::Char('u') => {
                        if self.time_update(cx, "zone=default", |t| {
                            t.zone.clear();
                            t.zone_auto = false;
                        }) {
                            self.ok("Using the time zone the machine itself has (UTC when it has none).");
                        }
                    }
                    Key::Char('a') => {
                        let on = !Self::time_settings(cx).zone_auto;
                        if self.time_update(cx, if on { "zone=auto" } else { "zone=fixed" }, |t| t.zone_auto = on) {
                            if on {
                                self.zone_detect(cx);
                            } else {
                                self.ok("The time zone stays as it is now; choose another one with Time zone.");
                            }
                        }
                    }
                    Key::Char('d') => {
                        if !Self::time_settings(cx).zone_auto {
                            return self.fail("Switch on Detect automatically first: it asks an internet service which zone this network's public IP address is in.");
                        }
                        self.zone_detect(cx);
                    }
                    Key::Char('n') => {
                        let off = !Self::time_settings(cx).no_ntp;
                        if self.time_update(cx, if off { "ntp=off" } else { "ntp=on" }, |t| t.no_ntp = off) {
                            self.ok(if off { "The clock is no longer set from the network." } else { "The clock is set from the network when it is online." });
                        }
                    }
                    Key::Char('c') => {
                        let t = Self::time_settings(cx);
                        self.open_form(FormKind::TimeServer, Form::new("Time server").text("Host name (empty = time.cloudflare.com, then pool.ntp.org)", &t.ntp_server, "for example ntp.example.org"));
                    }
                    _ => self.time_sync(cx, matches!(k, Key::Char('b'))),
                }
            }
            Key::Char('r') => self.ok("Read again."),
            _ => {}
        }
    }

    fn time_sync(&mut self, cx: &AppCx, allow_back: bool) {
        let t = Self::time_settings(cx);
        let servers = self.time.servers.clone().unwrap_or_else(|| iyakki_agent::ntp::servers_for(&t.ntp_server));
        let setter = self.time.set_clock.clone();
        cx.audit.record(&cx.session.user, "settings.time.sync", if allow_back { "allow back" } else { "" });
        self.start("Asking the time server", move || {
            let st = iyakki_agent::ntp::sync_once(&servers, allow_back, Duration::from_secs(4), &*setter);
            if st.message.starts_with("Could not") || st.message.contains("could not be set") { JobOut::Msg(Err(st.message)) } else { JobOut::Msg(Ok(st.message)) }
        });
    }

    pub(super) fn time_pick(&mut self, cx: &AppCx, item: &str) {
        // a zone chosen by hand is fixed: automatic detection stops
        if self.time_update(cx, &format!("zone={item}"), |t| {
            t.zone = item.to_string();
            t.zone_auto = false;
        }) {
            self.ok(format!("Time zone: {item} (fixed)."));
        }
    }

    /// Look the zone up now ("Detect automatically" is on): through the test's detector at once, else in the background.
    fn zone_detect(&mut self, cx: &AppCx) {
        cx.audit.record(&cx.session.user, "settings.time.detect", "");
        match self.time.detect.clone() {
            Some(f) => {
                let r = f();
                let mut changed = None;
                let saved = Self::svc(cx).settings.lock().unwrap().update(|s| changed = iyakki_agent::tzdetect::apply(&mut s.time, &r, clock::now_unix()));
                if let Err(e) = saved {
                    return self.fail(e.to_string());
                }
                if let (Some(z), true) = (&changed, self.time.global_zone) {
                    clock::set_chosen_zone(z);
                }
                match r {
                    Ok((z, src)) => self.ok(format!("Time zone detected: {z} (from {src}).")),
                    Err(e) => self.fail(e),
                }
            }
            None => {
                let global = self.time.global_zone;
                let started = iyakki_agent::tzdetect::refresh_in_background(Self::svc(cx), true, Arc::new(move |z: &str| {
                    if global {
                        clock::set_chosen_zone(z);
                    }
                }));
                if started {
                    self.ok("Looking up the time zone (asks ipapi.co, then worldtimeapi.org)...");
                } else {
                    self.ok("A lookup is already running.");
                }
            }
        }
    }

    pub(super) fn time_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Ok(String::new());
        }
        let server = form.value(0).trim().to_string();
        if !server.is_empty() && !iyakki_agent::ntp::valid_server(&server) {
            return Err("Use a host name or address: letters, digits, '.' and '-'.".into());
        }
        let ok = self.time_update(cx, "ntp server", |t| t.ntp_server = server.clone());
        if ok { Ok(if server.is_empty() { "Using the usual time servers.".to_string() } else { format!("Time server: {server}.") }) } else { Err("Could not save.".into()) }
    }

    /// The rows the page shows: (label, value, is it a warning).
    pub(super) fn time_rows(&self, cx: &AppCx) -> Vec<(&'static str, String, bool)> {
        let t = Self::time_settings(cx);
        let z = Zone::system();
        let now = clock::now_unix();
        let (h12, secs) = crate::uisize::load_clock(Self::svc(cx).dir(), &cx.session.user);
        let mut rows = vec![("Now", clock::format(&z, now, "%A %d %B %Y, %H:%M:%S %Z (UTC%z)"), clock::looks_wrong(now))];
        let list = clock::zone_names();
        let source = if clock::has_zone_files() { "from the machine's zone files" } else { "from the built-in list (this image has no time zone package)" };
        rows.push(("Time zone", if t.zone.is_empty() { format!("the machine's own ({})", z.local(now).abbrev) } else { t.zone.clone() }, false));
        rows.push(("Zones known", format!("{} {source}", list.len()), false));
        let auto = if t.zone_auto {
            let last = if t.zone_checked_at > 0 { format!(" · last lookup {}: {}", ago(now - t.zone_checked_at), t.zone_note) } else { " · not looked up yet".to_string() };
            format!("on (from the public IP address){last}")
        } else {
            "off: the zone is fixed (Detect automatically switches it on)".to_string()
        };
        rows.push(("Automatic zone", auto, t.zone_auto && t.zone_note.starts_with("Could not")));
        rows.push(("", iyakki_agent::tzdetect::EXPLAIN.to_string(), false));
        rows.push(("Your clock", format!("{}-hour{}", if h12 { 12 } else { 24 }, if secs { ", with seconds" } else { "" }), false));
        let st = iyakki_agent::ntp::status();
        let server = if t.ntp_server.is_empty() { "time.cloudflare.com, then pool.ntp.org".to_string() } else { t.ntp_server.clone() };
        rows.push(("Network time", if t.no_ntp { "off".to_string() } else { format!("on · {server}") }, false));
        if st.last_sync > 0 {
            rows.push(("Last answer", format!("{} from {}", ago(now - st.last_sync), st.server), false));
        }
        if !st.message.is_empty() {
            rows.push(("", st.message, st.pending_back.is_some()));
        }
        if clock::looks_wrong(now) {
            rows.push(("", "The clock looks wrong: this computer has not set its time (no clock battery, or the network was not up yet). Press y to ask the time server now.".to_string(), true));
        }
        rows
    }

    pub(super) fn render_time(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mut y = a.y;
        if !self.time.writable {
            for l in g.wrap(13.0 * s, "A desktop window follows the clock and time zone of the computer it runs on. Only your own clock face (12/24-hour, seconds) is changed here.", Face::Sans, a.w).iter().take(2) {
                g.text(a.x, y, 13.0 * s, ACCENT, l, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        let label_w = (150.0 * s).min(a.w * 0.35);
        for (label, value, warn) in self.time_rows(cx) {
            if label.is_empty() {
                for l in g.wrap(13.0 * s, &value, Face::Sans, a.w - label_w) {
                    g.text(a.x + label_w, y, 13.0 * s, if warn { rgb(255, 196, 94) } else { MUTED }, &l, Face::Sans);
                    y += 18.0 * s;
                }
                continue;
            }
            g.text(a.x, y, 14.0 * s, MUTED, label, Face::Sans);
            let t = g.fit(14.0 * s, &value, Face::Bold, a.w - label_w);
            g.text(a.x + label_w, y, 14.0 * s, if warn { rgb(255, 196, 94) } else { TEXT }, &t, Face::Bold);
            y += 24.0 * s;
        }
    }

    pub(super) fn time_buttons(&self) -> Vec<Btn> {
        let (acc, mut_) = (ACCENT, TEXT);
        vec![("Time zone", acc, 'z'), ("Detect automatically on/off", mut_, 'a'), ("Detect now", mut_, 'd'), ("Machine's own zone", mut_, 'u'), ("12/24 hour", acc, 'h'), ("Seconds", mut_, 's'), ("Network time on/off", mut_, 'n'), ("Time server", mut_, 'c'), ("Ask server now", acc, 'y'), ("Set the clock to the network time", mut_, 'b')]
    }

    // ------------------------------------------------------------------------------------------------ Host name

    fn hn_locked(&self) -> Option<String> {
        crate::hostname::locked_by(&self.hn.cmdline)
    }

    pub(super) fn handle_host_name(&mut self, k: &Key, cx: &AppCx) {
        if let Key::Char('r') = k {
            self.hn.draft = None;
            return self.ok("Read again.");
        }
        if !self.admin(cx) {
            return;
        }
        if !self.hn.writable {
            return self.fail("This window shows the name of the computer it runs on and changes nothing there. The host name is changed on the IYAKKI system itself.");
        }
        if let Some(n) = self.hn_locked() {
            return self.fail(format!("The name is fixed by the boot option iyakki.hostname={n}. Edit the boot entry to change it."));
        }
        match k {
            Key::Char('e') => {
                let start = self.hn.draft.clone().unwrap_or_else(|| crate::hostname::current("iyakki"));
                self.open_form(FormKind::HostName, Form::new("Host name").text(crate::hostname::HINT, &start, "for example kitchen-mac"));
            }
            Key::Char('y') | Key::Char('v') => {
                let Some(name) = self.hn.draft.clone().or_else(|| (matches!(k, Key::Char('v'))).then(|| crate::hostname::current("iyakki"))) else {
                    return self.fail("Press e and type a new name first.");
                };
                let apply_now = matches!(k, Key::Char('y'));
                let saved = crate::hostname::save(&self.hn.conf, &name);
                let applied = if apply_now { Some((self.hn.apply)(&name, &self.hn.etc)) } else { None };
                cx.audit.record(&cx.session.user, if apply_now { "hostname.apply" } else { "hostname.save" }, &name);
                match (saved, applied) {
                    (Err(e), _) => self.fail(format!("Could not save: {e}")),
                    (Ok(()), Some(Err(e))) => self.fail(format!("Saved for the next boots, but not applied now: {e}")),
                    (Ok(()), Some(Ok(()))) => {
                        self.hn.draft = None;
                        self.ok(format!("The host name is now {name}, and it is kept for the next boots."));
                    }
                    (Ok(()), None) => self.ok(format!("Saved: {name} is used from the next boot.")),
                }
            }
            _ => {}
        }
    }

    pub(super) fn host_name_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Ok(String::new());
        }
        let name = crate::hostname::check(&form.value(0))?;
        self.hn.draft = Some(name.clone());
        Ok(format!("{name} is ready. Press Apply now to use it, or Save for next boots."))
    }

    pub(super) fn render_host_name(&mut self, g: &mut Gfx, a: R, s: f32, _cx: &AppCx) {
        let mut y = a.y;
        if !self.hn.writable {
            for l in g.wrap(13.0 * s, "A desktop window shows the name of the computer it runs on, read-only. The host name is changed on the IYAKKI system itself.", Face::Sans, a.w).iter().take(2) {
                g.text(a.x, y, 13.0 * s, ACCENT, l, Face::Sans);
                y += 18.0 * s;
            }
            y += 4.0 * s;
        }
        let label_w = (170.0 * s).min(a.w * 0.4);
        let row = |g: &mut Gfx, y: &mut f32, label: &str, value: &str, col: [u8; 4]| {
            g.text(a.x, *y, 14.0 * s, MUTED, label, Face::Sans);
            let t = g.fit(14.0 * s, value, Face::Bold, a.w - label_w);
            g.text(a.x + label_w, *y, 14.0 * s, col, &t, Face::Bold);
            *y += 24.0 * s;
        };
        row(g, &mut y, "Host name now", &crate::hostname::current("iyakki"), TEXT);
        let saved = crate::hostname::saved(&self.hn.conf);
        row(g, &mut y, "Kept for next boots", saved.as_deref().unwrap_or("not set (the image's own name)"), TEXT);
        if let Some(d) = &self.hn.draft {
            row(g, &mut y, "Typed, not applied", d, rgb(255, 196, 94));
        }
        if let Some(n) = self.hn_locked() {
            row(g, &mut y, "Locked", &format!("by the boot option iyakki.hostname={n}"), rgb(255, 196, 94));
        }
        y += 6.0 * s;
        for l in g.wrap(13.0 * s, crate::hostname::HINT, Face::Sans, a.w) {
            g.text(a.x, y, 13.0 * s, DIM, &l, Face::Sans);
            y += 18.0 * s;
        }
        y += 6.0 * s;
        for l in g.wrap(13.0 * s, "The name is how this computer shows up on your network and in a terminal prompt. Applying it changes the running system at once; programs already running may keep the old name until restarted.", Face::Sans, a.w) {
            g.text(a.x, y, 13.0 * s, DIM, &l, Face::Sans);
            y += 18.0 * s;
        }
    }

    pub(super) fn host_name_buttons(&self) -> Vec<Btn> {
        vec![("Change name", ACCENT, 'e'), ("Apply now", ACCENT, 'y'), ("Save for next boots", TEXT, 'v')]
    }
}
