//! LINUX WINDOW: a graphical program of the Linux apps sandbox shown in a tile (`docs/LINUX-WINDOWS.md`).
//!
//! The engine is `iyakki_agent::linuxwin` (the sandbox command, the launcher, the install) and `iyakki_rfb` (IYAKKI's own RFB client:
//! the sandbox runs TigerVNC's X server on a Unix socket, this tile is its client). This file is the state, the keys and the drawing:
//! the picture of the remote screen is blitted into the area below a status strip; keys, pointer and wheel are sent to the program; the
//! screen is resized to the area; the clipboard is bridged only by the two buttons (or an explicit paste), as typed text.
//! Closing the tile (or Stop) kills the sandbox's processes. Nothing here panics on odd geometry (`clamp_safe`, no unwrap).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::linuxapps::LinuxApps;
use iyakki_agent::linuxwin::{self as lw, Program, Usage, Window};
use iyakki_agent::quickinstall as qi;
use iyakki_rfb::{keysyms as ks, Event as RfbEvent, Link};
use iyakki_sec::policy::CliLevel;
use tiny_skia::Pixmap;

use super::{App, AppCx, Flow};
use crate::gfx::{Face, Gfx, Icon};
use crate::safeclamp::SafeClamp;
use crate::ui::{clipboard, Event, Key, Nav, DIM, ERR, MUTED, OK, R, TEXT};

pub const ACCENT: [u8; 4] = [120, 200, 150, 255];

/// The window area as a hit region: `Event::AreaAt(AREA_WIN, x, y)` is a press at (x, y) inside it.
pub const AREA_WIN: usize = 62_050;
const T_START: usize = 1;
const T_REFRESH: usize = 2;
const T_STOP: usize = 3;
const T_COPY: usize = 4;
const T_PASTE: usize = 5;
const T_INSTALL_BASE: usize = 6;
const T_PROGRAM_INSTALL: usize = 10; // 10..: the offered test programs
const START_WAIT: Duration = Duration::from_secs(40);
const RESIZE_SETTLE: Duration = Duration::from_millis(300);
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// Wheel notches sent for one scroll event at most.
const MAX_NOTCHES: i32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Linux windows are not installed (or still being installed).
    NotInstalled,
    NoAccess(String),
    Pick,
    Starting,
    Running,
    Ended(String),
}

struct Started {
    win: Window,
    link: Link,
}

type StartResult = Result<Started, String>;

/// What the tile uses that tests replace: the sandbox manager of the person, the spawner and the process table.
pub struct Rig {
    pub la: Arc<dyn Fn() -> Result<LinuxApps, String> + Send + Sync>,
    pub spawn: Arc<dyn lw::Spawn>,
    pub proc_root: std::path::PathBuf,
}

pub struct LinuxWinApp {
    pub phase: Phase,
    pub programs: Vec<Program>,
    pub sel: usize,
    pub note: Option<(String, bool)>,
    rig: Rig,
    started: Option<Receiver<StartResult>>,
    cancel: Arc<AtomicBool>,
    win: Option<Window>,
    link: Option<Link>,
    pm: Option<Pixmap>,
    /// Where the remote screen was drawn last (screen pixels).
    pub view: R,
    pressed: bool,
    last_pos: Option<(i32, i32)>,
    want: (u32, u32),
    want_since: Instant,
    sent: (u32, u32),
    pub usage: Usage,
    pub cpu: f32,
    pub peak_kb: u64,
    last_sample: Option<(Instant, Usage)>,
    remote_clip: Option<String>,
    redraw: bool,
    pub program_name: String,
    network: bool,
    checked: Instant,
    can_install: bool,
    user: String,
    installing: bool,
    started_at: Instant,
    full_cli: bool,
    all_hosts: bool,
    job_running: bool,
    install_rx: Option<Receiver<Result<(), String>>>,
}

// ------------------------------------------------------------------------------------------------ keys

/// A shell key as RFB key events `(down, keysym)`: modifiers go down before the key and up after it; a repeat is the same sequence again.
pub fn key_syms(k: &Key) -> Vec<(bool, u32)> {
    let (mut mods, sym): (Vec<u32>, u32) = match k {
        Key::Char(c) => (vec![], ks::from_char(*c)),
        Key::Ctrl(c) => {
            let mut m = vec![ks::CONTROL_L];
            if c.is_ascii_uppercase() {
                m.push(ks::SHIFT_L);
            }
            (m, ks::from_char(c.to_ascii_lowercase()))
        }
        Key::Enter => (vec![], ks::RETURN),
        Key::Backspace => (vec![], ks::BACKSPACE),
        Key::Delete => (vec![], ks::DELETE),
        Key::Tab => (vec![], ks::TAB),
        Key::BackTab => (vec![ks::SHIFT_L], ks::ISO_LEFT_TAB),
        Key::Esc => (vec![], ks::ESCAPE),
        Key::Up => (vec![], ks::UP),
        Key::Down => (vec![], ks::DOWN),
        Key::Left => (vec![], ks::LEFT),
        Key::Right => (vec![], ks::RIGHT),
        Key::Home => (vec![], ks::HOME),
        Key::End => (vec![], ks::END),
        Key::PageUp => (vec![], ks::PAGE_UP),
        Key::PageDown => (vec![], ks::PAGE_DOWN),
        Key::F(n) => (vec![], ks::function_key(*n)),
        Key::Nav { nav, shift, ctrl } => {
            let mut m = Vec::new();
            if *ctrl {
                m.push(ks::CONTROL_L);
            }
            if *shift {
                m.push(ks::SHIFT_L);
            }
            let s = match nav {
                Nav::Left => ks::LEFT,
                Nav::Right => ks::RIGHT,
                Nav::Up => ks::UP,
                Nav::Down => ks::DOWN,
                Nav::Home => ks::HOME,
                Nav::End => ks::END,
                Nav::PageUp => ks::PAGE_UP,
                Nav::PageDown => ks::PAGE_DOWN,
            };
            (m, s)
        }
    };
    let mut v: Vec<(bool, u32)> = mods.iter().map(|m| (true, *m)).collect();
    v.push((true, sym));
    v.push((false, sym));
    mods.reverse();
    v.extend(mods.into_iter().map(|m| (false, m)));
    v
}

/// Ctrl+V as key events (what pasting into the program does after the text is on its clipboard).
pub fn paste_keys() -> Vec<(bool, u32)> {
    key_syms(&Key::Ctrl('v'))
}

// ------------------------------------------------------------------------------------------------ geometry

/// The status strip and the window area inside `area`; never negative sizes.
pub fn layout(area: R, s: f32) -> (R, R) {
    let sh = (28.0 * s).max(0.0).min(area.h.max(0.0));
    let strip = R::new(area.x, area.y, area.w.max(0.0), sh);
    let view = R::new(area.x, area.y + sh, area.w.max(0.0), (area.h - sh).max(0.0));
    (strip, view)
}

/// The size to ask the remote screen for: the area in pixels, kept within what the sandbox's script and the protocol take.
pub fn wanted_size(view: R) -> (u32, u32) {
    let w = (view.w.max(0.0) as u32).clamp_safe(lw::MIN_SIZE, lw::MAX_SIZE);
    let h = (view.h.max(0.0) as u32).clamp_safe(lw::MIN_SIZE, lw::MAX_SIZE);
    (w, h)
}

/// A screen position as a position in the remote screen, when inside the area.
pub fn to_remote(view: R, x: i32, y: i32) -> Option<(i32, i32)> {
    let (px, py) = (x - view.x.round() as i32, y - view.y.round() as i32);
    (px >= 0 && py >= 0 && (px as f32) < view.w && (py as f32) < view.h).then_some((px, py))
}

/// "12%", "85 MB": short figures for the strip.
pub fn mem_text(kb: u64) -> String {
    if kb >= 1024 * 1024 { format!("{:.1} GB", kb as f64 / 1048576.0) } else { format!("{} MB", kb / 1024) }
}

// ------------------------------------------------------------------------------------------------ the app

impl LinuxWinApp {
    pub fn new(cx: &AppCx) -> LinuxWinApp {
        let data = cx.services.dir().to_path_buf();
        let user = cx.session.user.clone();
        let rig = Rig {
            la: Arc::new(move || {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
                Ok(LinuxApps::new(&data, &user, &exe, Arc::new(iyakki_agent::wsexec::System)))
            }),
            spawn: Arc::new(lw::System),
            proc_root: "/proc".into(),
        };
        LinuxWinApp::with_rig(cx, rig)
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> LinuxWinApp {
        let mut a = LinuxWinApp {
            phase: Phase::NotInstalled,
            programs: Vec::new(),
            sel: 0,
            note: None,
            rig,
            started: None,
            cancel: Arc::new(AtomicBool::new(false)),
            win: None,
            link: None,
            pm: None,
            view: R::new(0.0, 0.0, 0.0, 0.0),
            pressed: false,
            last_pos: None,
            want: (0, 0),
            want_since: Instant::now(),
            sent: (0, 0),
            usage: Usage::default(),
            cpu: 0.0,
            peak_kb: 0,
            last_sample: None,
            remote_clip: None,
            redraw: true,
            program_name: String::new(),
            network: false,
            checked: Instant::now(),
            can_install: cx.session.policy.manage_settings,
            user: cx.session.user.clone(),
            installing: false,
            started_at: Instant::now(),
            full_cli: cx.session.policy.cli == CliLevel::Full,
            all_hosts: cx.session.policy.net_hosts.iter().any(|h| h == "*"),
            job_running: false,
            install_rx: None,
        };
        a.refresh(cx);
        a
    }

    /// Read what is installed (files only) and set the phase when nothing is running.
    pub fn refresh(&mut self, cx: &AppCx) {
        self.checked = Instant::now();
        self.job_running = cx.services.jobs.visible(&self.user, qi::WORKSPACE).iter().any(|j| j.running() && qi::item_of(j) == Some(qi::Item::LinuxWindows));
        if matches!(self.phase, Phase::Starting | Phase::Running) {
            return;
        }
        if !self.full_cli {
            self.phase = Phase::NoAccess("Linux windows run programs of your choosing, so they need the full command-line permission (administrators and operators have it).".into());
            return;
        }
        match (self.rig.la)() {
            Ok(la) if lw::ready(&la) => {
                self.programs = lw::programs(&la.rootfs_of(lw::SANDBOX));
                self.sel = self.sel.min(self.programs.len().saturating_sub(1));
                self.network = la.get(lw::SANDBOX).map(|s| s.network).unwrap_or(false);
                if !matches!(self.phase, Phase::Ended(_)) {
                    self.phase = Phase::Pick;
                }
            }
            Ok(_) => {
                self.programs.clear();
                self.phase = Phase::NotInstalled;
            }
            Err(e) => self.phase = Phase::Ended(e),
        }
        self.redraw = true;
    }

    fn say(&mut self, t: impl Into<String>, good: bool) {
        self.note = Some((t.into(), good));
        self.redraw = true;
    }

    /// Start the selected program: the sandbox, the display and the connection come up on a thread of their own.
    pub fn start(&mut self, cx: &AppCx) {
        let _ = cx;
        let Some(p) = self.programs.get(self.sel).cloned() else { return self.say("Choose a program first (or install a test program).", false) };
        if !self.full_cli {
            return;
        }
        let la = match (self.rig.la)() {
            Ok(l) => l,
            Err(e) => return self.say(e, false),
        };
        // a sandbox with its network on can reach any server: only an account allowed every host runs it (as the CLI does)
        match la.get(lw::SANDBOX) {
            Ok(s) if s.network && !self.all_hosts => return self.say("The sandbox has its network on, so it can reach any server; your account may not run it. Turn the network off on the Linux apps page.", false),
            Ok(s) => self.network = s.network,
            Err(e) => return self.say(e, false),
        }
        let (w, h) = wanted_size(self.view);
        let (w, h) = if self.view.w < 1.0 { (1024, 640) } else { (w, h) };
        let spawn = self.rig.spawn.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (tx, rx) = channel::<StartResult>();
        self.started = Some(rx);
        self.program_name = p.name.clone();
        self.phase = Phase::Starting;
        self.started_at = Instant::now();
        self.want = (w, h);
        self.sent = (w, h);
        self.note = None;
        std::thread::spawn(move || {
            let r = (|| -> StartResult {
                let mut win = Window::start(&la, &*spawn, &p, w, h)?;
                let c2 = cancel.clone();
                win.wait_socket_or(START_WAIT, &std::thread::sleep, &move || c2.load(Ordering::SeqCst))?;
                let link = Link::connect_unix(&win.socket(), Duration::from_secs(10))?;
                let t0 = Instant::now();
                loop {
                    if link.with(|s| s.ready()) {
                        break;
                    }
                    if let Some(e) = link.ended() {
                        return Err(format!("the display ended while connecting: {e}"));
                    }
                    if cancel.load(Ordering::SeqCst) || t0.elapsed() > Duration::from_secs(15) {
                        return Err("the display did not answer".into());
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                Ok(Started { win, link })
            })();
            // a receiver that is gone (the tile was closed) drops the window here, which stops the sandbox
            let _ = tx.send(r);
        });
        self.redraw = true;
    }

    /// Stop the program and everything of the sandbox that runs.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.started = None;
        if let Some(mut l) = self.link.take() {
            l.close();
        }
        if let Some(mut w) = self.win.take() {
            w.stop();
        }
        self.pm = None;
        self.pressed = false;
        self.last_pos = None;
        self.last_sample = None;
        self.usage = Usage::default();
    }

    pub fn running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// The picture's size, when there is one.
    pub fn frame_size(&self) -> (u32, u32) {
        self.pm.as_ref().map_or((0, 0), |p| (p.width(), p.height()))
    }

    fn send(&mut self, f: impl FnOnce(&mut iyakki_rfb::Session)) {
        if let Some(l) = &self.link {
            l.with(f);
        }
    }

    fn pointer(&mut self, x: i32, y: i32, buttons: u8) {
        self.last_pos = Some((x, y));
        self.send(|s| s.pointer_event(x, y, buttons));
    }

    // ---- the heartbeat

    fn tick(&mut self, cx: &AppCx) {
        if let Some(rx) = &self.install_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.install_rx = None;
                    self.installing = false;
                    match r {
                        Ok(()) => self.say("Installed. It is in the list now.", true),
                        Err(e) => self.say(e, false),
                    }
                    self.refresh(cx);
                }
                Err(TryRecvError::Disconnected) => {
                    self.install_rx = None;
                    self.installing = false;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.started {
            match rx.try_recv() {
                Ok(Ok(st)) => {
                    self.started = None;
                    self.win = Some(st.win);
                    self.link = Some(st.link);
                    self.phase = Phase::Running;
                    self.pm = None;
                    self.sync_frame();
                    self.redraw = true;
                }
                Ok(Err(e)) => {
                    self.started = None;
                    self.phase = Phase::Ended(e);
                    self.redraw = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.started = None;
                    self.phase = Phase::Ended("the start was interrupted".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.phase == Phase::Running {
            self.sync_frame();
            let dead = self.win.as_mut().is_some_and(|w| !w.alive());
            let ended = self.link.as_ref().and_then(|l| l.ended()).or_else(|| dead.then(|| "the program ended".to_string()));
            if let Some(why) = ended {
                self.stop();
                self.phase = Phase::Ended(why);
                self.redraw = true;
                return;
            }
            self.maybe_resize();
            self.sample();
        } else if matches!(self.phase, Phase::NotInstalled | Phase::Pick) && self.checked.elapsed() > Duration::from_secs(3) {
            self.refresh(cx);
        }
    }

    /// Take what the display sent: copy the changed parts of its picture into ours.
    fn sync_frame(&mut self) {
        let Some(link) = &self.link else { return };
        let pm = &mut self.pm;
        let mut clip = None;
        let mut changed = false;
        link.with(|s| {
            let events = s.take_events();
            let fb = s.fb();
            for e in events {
                match e {
                    RfbEvent::Ready { .. } | RfbEvent::Resized { .. } => {
                        *pm = Pixmap::new(fb.w.max(1), fb.h.max(1));
                        copy_rect(pm, fb, 0, 0, fb.w, fb.h);
                        changed = true;
                    }
                    RfbEvent::Updated(r) => {
                        if pm.as_ref().is_none_or(|p| (p.width(), p.height()) != (fb.w, fb.h)) {
                            *pm = Pixmap::new(fb.w.max(1), fb.h.max(1));
                            copy_rect(pm, fb, 0, 0, fb.w, fb.h);
                        } else {
                            copy_rect(pm, fb, r.x, r.y, r.w, r.h);
                        }
                        changed = true;
                    }
                    RfbEvent::CutText(t) => clip = Some(t),
                    RfbEvent::Bell | RfbEvent::ResizeRefused(_) => {}
                }
            }
        });
        if clip.is_some() {
            self.remote_clip = clip;
        }
        if changed {
            self.redraw = true;
        }
    }

    /// Ask the display for the size of the area once the area has stopped changing.
    fn maybe_resize(&mut self) {
        let want = wanted_size(self.view);
        if self.view.w < 1.0 || self.view.h < 1.0 {
            return;
        }
        if want != self.want {
            self.want = want;
            self.want_since = Instant::now();
            return;
        }
        if want != self.sent && want != self.frame_size() && self.want_since.elapsed() >= RESIZE_SETTLE {
            self.sent = want;
            let mut ok = false;
            self.send(|s| ok = s.request_resize(want.0, want.1));
            if !ok {
                // a server that cannot resize keeps its size; the picture is drawn as it is
                self.sent = self.frame_size();
            }
        }
    }

    fn sample(&mut self) {
        let Some(pid) = self.win.as_ref().and_then(|w| w.pid()) else { return };
        let now = Instant::now();
        if self.last_sample.is_some_and(|(t, _)| now.duration_since(t) < SAMPLE_EVERY) {
            return;
        }
        let u = lw::usage_of(&self.rig.proc_root, pid);
        if let Some((t, before)) = self.last_sample {
            self.cpu = lw::cpu_percent(before, u, now.duration_since(t).as_secs_f32());
        }
        self.peak_kb = self.peak_kb.max(u.rss_kb);
        self.usage = u;
        self.last_sample = Some((now, u));
        self.redraw = true;
    }

    // ---- drawing

    fn strip_text(&self) -> String {
        format!("{} · {}x{} · CPU {:.0}% · memory {} (peak {}) · {} process(es) · network {}", self.program_name, self.frame_size().0, self.frame_size().1, self.cpu, mem_text(self.usage.rss_kb), mem_text(self.peak_kb), self.usage.procs, if self.network { "on" } else { "off" })
    }

    fn draw_running(&mut self, g: &mut Gfx, area: R, s: f32) {
        let (strip, view) = layout(area, s);
        self.view = view;
        g.rect(strip.x, strip.y, strip.w, strip.h, crate::ui::CARD);
        let size = 11.0 * s;
        // buttons from the right: Stop, Paste into window, Copy from window, while the text keeps some room
        let mut x = strip.x + strip.w - 6.0 * s;
        let by = strip.y + (strip.h - size * 2.1) / 2.0;
        let min_text = 110.0 * s;
        for (label, ev, accent) in [("Stop", T_STOP, ERR), ("Paste into window", T_PASTE, ACCENT), ("Copy from window", T_COPY, ACCENT)] {
            let w = g.text_width(size, label, Face::Bold) + size * 1.7;
            if x - w < strip.x + min_text {
                break;
            }
            x -= w;
            g.button(x, by, size, label, accent, Event::Tab(ev));
            x -= 6.0 * s;
        }
        let text = g.fit(size, &self.strip_text(), Face::Sans, (x - strip.x - 10.0 * s).max(10.0));
        g.text(strip.x + 8.0 * s, strip.y + (strip.h - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, &text, Face::Sans);
        // the remote screen
        g.rect(view.x, view.y, view.w, view.h, [14, 16, 24, 255]);
        g.hit(view, Event::Area(AREA_WIN), "linux window");
        let Some(pm) = &self.pm else { return };
        let (vx, vy) = (view.x.round() as i32, view.y.round() as i32);
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        let (sx0, dx0) = if vx < 0 { (-vx, 0) } else { (0, vx) };
        let cols = (pw - sx0).min(view.w as i32 - sx0.min(0)).min(gw - dx0).max(0);
        let rows = ph.min(view.h as i32);
        if cols <= 0 {
            return;
        }
        let src = pm.data();
        let dst = g.pm.data_mut();
        for y in 0..rows {
            let dy = vy + y;
            if dy < 0 || dy >= gh {
                continue;
            }
            let a = ((y * pw + sx0) * 4) as usize;
            let b = ((dy * gw + dx0) * 4) as usize;
            let n = (cols * 4) as usize;
            if a + n <= src.len() && b + n <= dst.len() {
                dst[b..b + n].copy_from_slice(&src[a..a + n]);
            }
        }
    }

    fn panel(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
        let size = 13.0 * s;
        let lh = g.line_height(size, Face::Sans);
        let pad = 16.0 * s;
        let (x, mut y, w) = (area.x + pad, area.y + pad, (area.w - 2.0 * pad).max(40.0));
        g.text(x, y, size * 1.2, TEXT, "Linux windows", Face::Bold);
        y += lh * 1.8;
        let para = |g: &mut Gfx, y: &mut f32, t: &str, c: [u8; 4]| {
            for l in g.wrap(size, t, Face::Sans, w) {
                if *y + lh < area.y + area.h {
                    g.text(x, *y, size, c, &l, Face::Sans);
                }
                *y += lh;
            }
            *y += lh * 0.4;
        };
        match self.phase.clone() {
            Phase::NoAccess(m) => para(g, &mut y, &m, ERR),
            Phase::NotInstalled => {
                para(g, &mut y, lw::NOTE, MUTED);
                if self.job_running {
                    para(g, &mut y, "Installing: progress is beside the clock in the top bar.", OK);
                } else if self.can_install {
                    para(g, &mut y, "Install makes a Debian 12 sandbox named gui and installs the display server with apt (about 250 MB, needs the network). Nothing is shipped in IYAKKI.", DIM);
                    g.button(x, y, size, "Install Linux windows", ACCENT, Event::Tab(T_INSTALL_BASE));
                    y += size * 3.0;
                } else {
                    para(g, &mut y, "Ask an administrator to install it (Store, Setup, Linux windows).", DIM);
                }
            }
            Phase::Starting => para(g, &mut y, &format!("Starting {} ... the sandbox, the display and the connection come up in a few seconds.", self.program_name), MUTED),
            Phase::Ended(m) => {
                para(g, &mut y, &format!("Stopped: {m}"), ERR);
                g.button(x, y, size, "Back to the list", ACCENT, Event::Tab(T_REFRESH));
                y += size * 3.0;
            }
            Phase::Pick => {
                para(g, &mut y, "Choose a graphical program of the sandbox. It opens in this window; closing the tile stops it.", MUTED);
                let row_h = lh * 1.7;
                let room = ((area.y + area.h - y - size * 6.0) / row_h).floor().max(1.0) as usize;
                let first = self.sel.saturating_sub(room.saturating_sub(1));
                if self.programs.is_empty() {
                    para(g, &mut y, "No graphical program is installed yet. Install a test program below, or any package with apt on the Linux apps page.", DIM);
                }
                for (i, p) in self.programs.iter().enumerate().skip(first).take(room) {
                    let r = R::new(x, y, w, row_h);
                    if i == self.sel {
                        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, crate::ui::CARD_SEL);
                    } else if g.hovered(r) {
                        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, crate::ui::CARD_HOVER);
                    }
                    let t = g.fit(size, &format!("{}  ({})", p.name, p.exec.join(" ")), Face::Sans, w - 12.0 * s);
                    g.text(r.x + 6.0 * s, r.y + (row_h - lh) / 2.0, size, TEXT, &t, Face::Sans);
                    g.hit(r, Event::Row(i), "program");
                    y += row_h;
                }
                y += lh * 0.6;
                let mut bx = x;
                if !self.programs.is_empty() {
                    bx += g.button(bx, y, size, "Start", ACCENT, Event::Tab(T_START)) + 8.0 * s;
                }
                bx += g.button(bx, y, size, "Refresh", MUTED, Event::Tab(T_REFRESH)) + 8.0 * s;
                y += size * 3.0;
                if self.can_install {
                    for (i, (pkg, _)) in lw::TEST_PROGRAMS.iter().enumerate() {
                        if bx + 140.0 * s > x + w {
                            bx = x;
                            y += size * 3.0;
                        }
                        bx += g.button(bx, y, size, &format!("Install {pkg}"), DIM, Event::Tab(T_PROGRAM_INSTALL + i)) + 8.0 * s;
                    }
                    y += size * 3.0;
                }
                let _ = cx;
            }
            Phase::Running => {}
        }
        if let Some((t, good)) = self.note.clone() {
            para(g, &mut y, &t, if good { OK } else { ERR });
        }
    }

    /// Install one of the test programs on a thread (apt takes a while); the list refreshes after.
    fn install_program(&mut self, i: usize, cx: &AppCx) {
        let Some((pkg, _)) = lw::TEST_PROGRAMS.get(i) else { return };
        if !self.can_install {
            return self.say("Only an administrator can install programs for the sandbox.", false);
        }
        if self.installing {
            return self.say("An install is running already.", false);
        }
        let la = match (self.rig.la)() {
            Ok(l) => l,
            Err(e) => return self.say(e, false),
        };
        self.installing = true;
        self.say(format!("Installing {pkg} with apt: this takes a minute (the network of the sandbox is on only while it runs) ..."), true);
        let pkg = pkg.to_string();
        let (tx, rx) = channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let r = lw::install_program(&la, &pkg, None);
            let _ = tx.send(r);
        });
        self.install_rx = Some(rx);
        let _ = cx;
    }
}

fn copy_rect(pm: &mut Option<Pixmap>, fb: &iyakki_rfb::Framebuffer, x: u32, y: u32, w: u32, h: u32) {
    let Some(pm) = pm.as_mut() else { return };
    let (pw, ph) = (pm.width(), pm.height());
    if (pw, ph) != (fb.w, fb.h) {
        return;
    }
    let x1 = x.saturating_add(w).min(pw);
    let y1 = y.saturating_add(h).min(ph);
    let data = pm.data_mut();
    for yy in y.min(ph)..y1 {
        for xx in x.min(pw)..x1 {
            let i = (yy * pw + xx) as usize;
            let (Some(p), Some(d)) = (fb.px.get(i), data.get_mut(i * 4..i * 4 + 4)) else { continue };
            d.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
        }
    }
}

impl Drop for LinuxWinApp {
    fn drop(&mut self) {
        self.stop();
    }
}

impl App for LinuxWinApp {
    fn title(&self) -> &'static str {
        "LINUX WINDOW"
    }

    fn icon(&self) -> Icon {
        Icon::Cli
    }

    fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    fn hint(&self) -> String {
        if self.running() {
            "Keys, mouse and wheel go to the program · Stop closes it".into()
        } else {
            "Up/Down choose · Enter start · R refresh · Esc back".into()
        }
    }

    fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Tick => self.tick(cx),
            Event::Tab(T_START) => self.start(cx),
            Event::Tab(T_REFRESH) => {
                if matches!(self.phase, Phase::Ended(_)) {
                    self.phase = Phase::Pick;
                }
                self.note = None;
                self.refresh(cx);
            }
            Event::Tab(T_STOP) => {
                self.stop();
                self.phase = Phase::Pick;
                self.refresh(cx);
            }
            Event::Tab(T_COPY) => match self.remote_clip.clone().filter(|t| !t.is_empty()) {
                Some(t) => {
                    clipboard::copy(&t);
                    self.say("Copied the program's clipboard text.", true);
                }
                None => self.say("The program has not copied any text yet.", false),
            },
            Event::Tab(T_PASTE) => match clipboard::latest().filter(|t| !t.is_empty()) {
                Some(t) => self.paste_text(&t),
                None => self.say("Your clipboard has no text.", false),
            },
            Event::Tab(T_INSTALL_BASE) => {
                if !self.can_install {
                    self.say("Only an administrator can install it.", false);
                } else {
                    match qi::Launch::this_program(cx.services.dir()).and_then(|l| qi::start(&cx.services.jobs, &cx.session.user, qi::Item::LinuxWindows, &|_| false, &l)) {
                        Ok(_) => {
                            cx.audit.record(&cx.session.user, "quickinstall.start", "linux-windows");
                            self.job_running = true;
                            self.say("Started: Install Linux windows. Progress is beside the clock.", true);
                        }
                        Err(e) => self.say(e, false),
                    }
                }
            }
            Event::Tab(i) if (T_PROGRAM_INSTALL..T_PROGRAM_INSTALL + lw::TEST_PROGRAMS.len()).contains(i) => self.install_program(i - T_PROGRAM_INSTALL, cx),
            Event::Row(i) if self.phase == Phase::Pick => {
                self.sel = (*i).min(self.programs.len().saturating_sub(1));
                self.redraw = true;
            }
            Event::Key(k) if self.running() => {
                let seq = key_syms(k);
                self.send(|s| {
                    for (down, sym) in seq {
                        s.key_event(down, sym);
                    }
                });
            }
            Event::Key(k) => match k {
                Key::Up if self.sel > 0 => {
                    self.sel -= 1;
                    self.redraw = true;
                }
                Key::Down if self.sel + 1 < self.programs.len() => {
                    self.sel += 1;
                    self.redraw = true;
                }
                Key::Enter if self.phase == Phase::Pick => self.start(cx),
                Key::Char('r') | Key::Char('R') => {
                    self.refresh(cx);
                }
                Key::Esc => return Flow::Back,
                _ => {}
            },
            Event::Paste(t) if self.running() => self.paste_text(t),
            Event::AreaAt(AREA_WIN, x, y) if self.running() => {
                let (px, py) = (*x as i32, *y as i32);
                if crate::ui::dragging() && self.pressed {
                    self.pointer(px, py, 1);
                } else {
                    self.pressed = true;
                    self.pointer(px, py, 0);
                    self.pointer(px, py, 1);
                }
            }
            Event::MouseMove(x, y) if self.running() && !self.pressed => {
                if let Some((px, py)) = to_remote(self.view, *x, *y) {
                    if self.last_pos != Some((px, py)) {
                        self.pointer(px, py, 0);
                    }
                } else {
                    self.last_pos = None;
                }
            }
            Event::MouseUp(x, y) if self.running() && self.pressed => {
                self.pressed = false;
                let (px, py) = (x - self.view.x.round() as i32, y - self.view.y.round() as i32);
                self.pointer(px, py, 0);
            }
            Event::MouseRight(x, y) if self.running() => {
                if let Some((px, py)) = to_remote(self.view, *x, *y) {
                    self.pointer(px, py, 0);
                    self.pointer(px, py, 4);
                    self.pointer(px, py, 0);
                }
            }
            Event::Scroll(d) if self.running() => {
                if let Some((px, py)) = self.last_pos {
                    let mask = if *d > 0 { 8u8 } else { 16u8 };
                    for _ in 0..d.unsigned_abs().min(MAX_NOTCHES as u32) {
                        self.pointer(px, py, mask);
                        self.pointer(px, py, 0);
                    }
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        if self.running() {
            self.draw_running(g, area, s);
        } else {
            // the size a start asks the display for: what the window area will be below its strip
            self.view = layout(area, s).1;
            self.panel(g, area, s, cx);
        }
    }
}

impl LinuxWinApp {
    /// Put typed text on the program's clipboard and press Ctrl+V there. Only ever called for an explicit paste (the button or a paste).
    pub fn paste_text(&mut self, t: &str) {
        let t: String = t.chars().take(64 * 1024).collect();
        let seq = paste_keys();
        self.send(|s| {
            s.cut_text(&t);
            for (down, sym) in seq {
                s.key_event(down, sym);
            }
        });
        self.say("Pasted your clipboard text into the program (as text only).", true);
    }

    /// For tests: connect an already made link as the running window.
    #[cfg(test)]
    pub fn attach_for_test(&mut self, link: Link) {
        self.link = Some(link);
        self.phase = Phase::Running;
        self.program_name = "test".into();
    }
}
