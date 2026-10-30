//! Settings -> Developer mode -> Connect over USB-C: the seven-step check list for two computers joined by ONE cable (a Thunderbolt 3/4
//! or USB4 cable between Thunderbolt/USB4 ports, or a host-to-host USB bridge cable; a plain USB-C cable between two PCs cannot carry a
//! network, and the page says so). Each step turns green or says in plain words what to do. The work is in
//! `iyakki_agent::directlink` (pure engine, `Env` seam); this file draws it and runs the slow parts (a one-second scan, the TLS hello,
//! the 64 MiB speed check) on a worker thread so the screen never waits.
//!
//! Keys (administrators): `r` check again, `z` authorize the waiting Thunderbolt device, `p` pair (then type the code shown on the other
//! computer, Enter), `s` speed check, `c` copy the diagnostic text, `f` fixed private pair on/off, `m` allow more on direct links
//! on/off, `u` or Esc back to developer mode, Up/Down scroll the steps.

use super::*;
use iyakki_agent::directlink::checks::{self, Action, Speed, State};
use iyakki_agent::directlink::{self as dl, Env};
use std::sync::{Arc, Mutex};

pub(super) const CODE_ID: usize = 931;
/// How often the check list looks again by itself while the page is open.
const REFRESH_SECS: u64 = 3;

#[derive(Default)]
struct Shared {
    report: Option<checks::Report>,
    speed: Option<Speed>,
    /// What the worker is doing right now, in words.
    running: Option<&'static str>,
    /// The answer of the last button: (it worked, text).
    note: Option<(bool, String)>,
}

#[derive(Clone, Copy)]
enum Job {
    Check,
    Authorize,
    Pair,
    Speed,
}

pub(super) struct UsbPanel {
    /// The machine (the real one unless a test puts a fake here).
    pub(super) env: Option<Arc<dyn Env>>,
    /// Run jobs on the calling thread (tests).
    pub(super) sync: bool,
    shared: Arc<Mutex<Shared>>,
    started: Option<std::time::Instant>,
    top: usize,
    pub(super) code: TextField,
    pub(super) code_focus: bool,
}

impl Default for UsbPanel {
    fn default() -> UsbPanel {
        UsbPanel { env: None, sync: false, shared: Arc::default(), started: None, top: 0, code: TextField::new("Pairing code from the other computer", false), code_focus: false }
    }
}

impl UsbPanel {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The last report (tests and the diagnostic button).
    pub(super) fn report(&self) -> Option<checks::Report> {
        self.lock().report.clone()
    }
    #[cfg(test)]
    pub(super) fn note(&self) -> Option<(bool, String)> {
        self.lock().note.clone()
    }
}

fn run_job(env: &dyn Env, sh: &Mutex<Shared>, job: Job, code: &str) {
    let lock = || sh.lock().unwrap_or_else(|e| e.into_inner());
    let (speed, report) = {
        let g = lock();
        (g.speed, g.report.clone())
    };
    let words = |g: &mut Shared, r: Result<String, String>| g.note = Some(match r {
        Ok(m) => (true, m),
        Err(e) => (false, e),
    });
    match job {
        Job::Check => {}
        Job::Authorize => {
            let r = report.map(|r| checks::authorize_pending(env, &r)).unwrap_or(Err("check first".into()));
            words(&mut lock(), r);
        }
        Job::Pair => {
            let r = report.map(|r| checks::pair(env, &r, code)).unwrap_or(Err("check first".into()));
            words(&mut lock(), r);
        }
        Job::Speed => {
            let r = report.map(|r| checks::speed_test(env, &r)).unwrap_or(Err("check first".into()));
            let mut g = lock();
            match r {
                Ok(s) => {
                    g.speed = Some(s);
                    g.note = Some((true, format!("{:.2} Gbit/s", s.sample.gbit_per_s())));
                }
                Err(e) => g.note = Some((false, e)),
            }
        }
    }
    let speed = lock().speed.or(speed);
    let fresh = checks::run(env, speed.as_ref());
    let mut g = lock();
    g.report = Some(fresh);
    g.running = None;
}

impl SettingsApp {
    fn usb_env(&mut self, cx: &AppCx) -> Arc<dyn Env> {
        if self.devel.usb.env.is_none() {
            let home = cx.services.dir().join("devmode").join("client").join(&cx.session.user);
            self.devel.usb.env = Some(Arc::new(dl::real::RealEnv::new(cx.services.dir(), &home)));
        }
        self.devel.usb.env.clone().unwrap_or_else(|| Arc::new(dl::fake::FakeEnv::new()))
    }

    fn usb_job(&mut self, cx: &AppCx, job: Job, what: &'static str) {
        if self.devel.usb.lock().running.is_some() {
            return;
        }
        let env = self.usb_env(cx);
        self.devel.usb.lock().running = Some(what);
        self.devel.usb.started = Some(std::time::Instant::now());
        let shared = self.devel.usb.shared.clone();
        let code = self.devel.usb.code.value.trim().to_string();
        if self.devel.usb.sync {
            run_job(env.as_ref(), &shared, job, &code);
        } else if std::thread::Builder::new().name("iyakki-usbcheck".into()).spawn({
            let (env, shared) = (env, shared.clone());
            move || run_job(env.as_ref(), &shared, job, &code)
        }).is_err() {
            shared.lock().unwrap_or_else(|e| e.into_inner()).running = None;
        }
    }

    /// While the page is open the list looks again every few seconds (on a worker; never two at once).
    fn usb_refresh(&mut self, cx: &AppCx) {
        let due = self.devel.usb.started.is_none_or(|t| t.elapsed().as_secs() >= REFRESH_SECS);
        if due && self.devel.usb.lock().running.is_none() {
            self.usb_job(cx, Job::Check, "Looking at the cable...");
        }
    }

    pub(super) fn usb_buttons(&self) -> Vec<Btn> {
        let mut v: Vec<Btn> = vec![("Check again", ACCENT, 'r')];
        match self.devel.usb.report().and_then(|r| r.checks.iter().find_map(|c| c.action.clone())) {
            Some(Action::Authorize(_)) => v.push(("Authorize", ERR, 'z')),
            Some(Action::Pair) => v.push(("Pair", ACCENT, 'p')),
            Some(Action::Speed) => v.push(("Speed", ACCENT, 's')),
            None => {}
        }
        v.push(("Copy diagnostics", TEXT, 'c'));
        v.push(("Fixed addresses", TEXT, 'f'));
        v.push(("Allow more", TEXT, 'm'));
        v.push(("Back", TEXT, 'u'));
        v
    }

    /// Events of the page that must not reach the rest of Settings: typing the pairing code, Esc, Up/Down. True when consumed.
    pub(super) fn usb_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        if !self.devel.usb_open {
            return false;
        }
        match ev {
            Event::FocusAt(id, x) if *id == CODE_ID => {
                self.devel.usb.code_focus = true;
                self.devel.usb.code.click(*x);
                true
            }
            Event::Paste(t) if self.devel.usb.code_focus => {
                self.devel.usb.code.paste(t);
                true
            }
            Event::Key(k) if self.devel.usb.code_focus => {
                match k {
                    Key::Esc => {
                        self.devel.usb.code_focus = false;
                        self.devel.usb.code.clear();
                    }
                    Key::Enter => {
                        self.devel.usb.code_focus = false;
                        if self.admin(cx) {
                            cx.audit.record(&cx.session.user, "directlink.pair", "pairing with the computer on the cable");
                            self.usb_job(cx, Job::Pair, "Pairing...");
                        }
                        self.devel.usb.code.clear();
                    }
                    other => {
                        self.devel.usb.code.handle(other);
                    }
                }
                true
            }
            Event::Key(Key::Esc) => {
                self.devel.usb_open = false;
                true
            }
            Event::Key(Key::Down) => {
                self.devel.usb.top = (self.devel.usb.top + 1).min(6);
                true
            }
            Event::Key(Key::Up) => {
                self.devel.usb.top = self.devel.usb.top.saturating_sub(1);
                true
            }
            _ => false,
        }
    }

    pub(super) fn handle_usb(&mut self, k: &Key, cx: &AppCx) {
        match k {
            Key::Char('u') => self.devel.usb_open = false,
            Key::Char('r') => self.usb_job(cx, Job::Check, "Looking at the cable..."),
            Key::Char('c') => match self.devel.usb.report() {
                Some(r) => {
                    let env = self.usb_env(cx);
                    crate::ui::clipboard::copy(&r.diagnostic(env.as_ref()));
                    self.ok("The diagnostic text is copied (no keys, no pairing codes in it).");
                }
                None => self.fail("Nothing to copy yet: the first check is still running."),
            },
            Key::Char('z') | Key::Char('p') | Key::Char('s') | Key::Char('f') | Key::Char('m') if !self.admin(cx) => {}
            Key::Char('z') => {
                cx.audit.record(&cx.session.user, "directlink.authorize", "approve the Thunderbolt device on the cable");
                self.usb_job(cx, Job::Authorize, "Approving...");
            }
            Key::Char('p') => {
                self.devel.usb.code_focus = true;
                self.ok("Type the code shown on the other computer, then Enter. Esc cancels.");
            }
            Key::Char('s') => self.usb_job(cx, Job::Speed, "Sending 64 MiB..."),
            Key::Char('f') | Key::Char('m') => {
                let env = self.usb_env(cx);
                let mut c = env.config();
                if matches!(k, Key::Char('f')) {
                    c.fixed_pair = !c.fixed_pair;
                } else {
                    c.allow_more = !c.allow_more;
                }
                match env.save_config(&c) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "directlink.config", &format!("fixed pair {}, allow more {}", c.fixed_pair, c.allow_more));
                        self.ok(if matches!(k, Key::Char('f')) {
                            format!("Fixed private addresses 10.77.0.1 / 10.77.0.2 on the cable: {}.", if c.fixed_pair { "on" } else { "off" })
                        } else if c.allow_more {
                            "Everything is now let in on direct links (the cable). Switch it off again when you are done.".to_string()
                        } else {
                            "Direct links let in developer mode, the peer beacon and ping only.".to_string()
                        });
                    }
                    Err(e) => self.fail(e),
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_usb(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        self.usb_refresh(cx);
        let (report, running, note, speed) = {
            let sh = self.devel.usb.lock();
            (sh.report.clone(), sh.running, sh.note.clone(), sh.speed)
        };
        let bottom = a.y + a.h;
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Connect two computers with one cable");
        let intro = "Two IYAKKI computers joined by one cable, no Wi-Fi and no Ethernet. This works over a Thunderbolt 3, Thunderbolt 4 or USB4 cable between two Thunderbolt/USB4 ports (10 to 20 Gbit/s), or over a host-to-host USB bridge cable. A plain USB-C cable between two PCs cannot work: both ports are USB hosts.";
        for w in g.wrap(13.0 * s, intro, Face::Sans, a.w) {
            if y + 18.0 * s > bottom {
                return;
            }
            g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
            y += 18.0 * s;
        }
        y += 6.0 * s;
        // Last result and what is happening now
        let last = report.as_ref().map(|r| r.last.clone()).unwrap_or_else(|| "Looking at the cable...".into());
        let all_green = report.as_ref().is_some_and(|r| r.checks.iter().all(|c| c.state == State::Pass));
        for w in g.wrap(14.0 * s, &format!("Last result: {last}"), Face::Bold, a.w) {
            if y + 20.0 * s > bottom {
                return;
            }
            g.text(a.x, y, 14.0 * s, if all_green { OK } else { TEXT }, &w, Face::Bold);
            y += 20.0 * s;
        }
        if let Some(r) = running {
            g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), r, Face::Sans);
            y += 18.0 * s;
        }
        if let Some((ok, t)) = note {
            for w in g.wrap(13.0 * s, &t, Face::Sans, a.w) {
                if y + 18.0 * s > bottom {
                    return;
                }
                g.text(a.x, y, 13.0 * s, if ok { OK } else { ERR }, &w, Face::Sans);
                y += 18.0 * s;
            }
        }
        y += 6.0 * s;
        if self.devel.usb.code_focus {
            let r = R::new(a.x, y, a.w.min(420.0 * s), 30.0 * s);
            self.devel.usb.code.render(g, r, true, s, (self.ticks / 4) % 2 == 0, Face::Mono, CODE_ID);
            y += 38.0 * s;
        }
        let Some(report) = report else { return };
        let indent = 22.0 * s;
        for c in report.checks.iter().skip(self.devel.usb.top) {
            if y + 40.0 * s > bottom {
                break;
            }
            let col = match c.state {
                State::Pass => OK,
                State::Fail => ERR,
                State::Wait => DIM,
            };
            g.circle(a.x + 7.0 * s, y + 9.0 * s, 6.0 * s, col);
            let title = g.fit(14.0 * s, c.title, Face::Bold, (a.w - indent).max(40.0 * s));
            g.text(a.x + indent, y, 14.0 * s, TEXT, &title, Face::Bold);
            y += 20.0 * s;
            for w in g.wrap(12.5 * s, &c.text, Face::Sans, (a.w - indent).max(40.0 * s)) {
                if y + 17.0 * s > bottom {
                    break;
                }
                g.text(a.x + indent, y, 12.5 * s, if c.state == State::Fail { rgb(255, 196, 94) } else { MUTED }, &w, Face::Sans);
                y += 17.0 * s;
            }
            y += 8.0 * s;
        }
        if y + 60.0 * s > bottom {
            return;
        }
        // the link, as the network page would show it
        y += controls::section_title(g, a.x, y, s, "The link");
        let cfg = self.usb_env(cx).config();
        let line = match (&report.link, &report.peer) {
            (Some(l), p) => format!(
                "{} · {} · {} · other computer: {}",
                l.name,
                l.ipv4.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "),
                l.speed_mbps.map_or("speed not reported".to_string(), |m| if m >= 1000 { format!("{} Gbit/s", m / 1000) } else { format!("{m} Mbit/s") }),
                p.as_ref().map_or("not found yet".to_string(), |p| format!("{} at {}", p.beacon.host, p.addr))
            ),
            (None, _) => "No interface for the cable yet.".to_string(),
        };
        for w in g.wrap(13.0 * s, &line, Face::Mono, a.w) {
            if y + 18.0 * s > bottom {
                return;
            }
            g.text(a.x, y, 13.0 * s, TEXT, &w, Face::Mono);
            y += 18.0 * s;
        }
        let sets = format!("Fixed private addresses (f): {} · Allow more than developer mode and ping on the cable (m): {}{}", if cfg.fixed_pair { "on" } else { "off" }, if cfg.allow_more { "ON" } else { "off" }, speed.map_or(String::new(), |s| format!(" · last speed {:.2} Gbit/s", s.sample.gbit_per_s())));
        for w in g.wrap(12.5 * s, &sets, Face::Sans, a.w) {
            if y + 17.0 * s > bottom {
                return;
            }
            g.text(a.x, y, 12.5 * s, if cfg.allow_more { ERR } else { MUTED }, &w, Face::Sans);
            y += 17.0 * s;
        }
    }
}
