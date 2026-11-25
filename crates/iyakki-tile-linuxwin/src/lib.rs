//! LINUX WINDOW: a graphical program of the Linux apps sandbox shown in a tile (`docs/LINUX-WINDOWS.md`).
//!
//! The engine is `iyakki_shared::linuxwin` (the sandbox command, the launcher, the install) and `iyakki_rfb` (IYAKKI's own RFB client:
//! the sandbox runs TigerVNC's X server on a Unix socket, this tile is its client); the tile reaches both only through `LinuxWinPort`, its `WinRig`
//! and the `Screen` and `Window` traits of `iyakki-tile-api::linuxwin`. This crate is the state, the keys and the drawing:
//! the picture of the remote screen is blitted into the area below a status strip; keys, pointer and wheel are sent to the program; the
//! screen is resized to the area; the clipboard is bridged only by the two buttons (or an explicit paste), as typed text.
//! Closing the tile (or Stop) kills the sandbox's processes. Nothing here panics on odd geometry (`clamp_safe`, no unwrap).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use iyakki_tile_api::linuxwin::{self as lw, keysyms as ks, Live, Program, Screen, ScreenUpdate, Usage, Window, WinRig};
pub use iyakki_tile_api::linuxwin::LinuxWinPort;
use iyakki_uikit::gfx::{Face, Gfx, Icon};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::ui::{clipboard, Event, Key, Nav, DIM, ERR, MUTED, OK, R, TEXT};

#[cfg(test)]
mod tests;

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = [120, 200, 150, 255];

/// The window area as a hit region: `Event::AreaAt(AREA_WIN, x, y)` is a press at (x, y) inside it.
pub const AREA_WIN: usize = 62_050;
const T_START: usize = 1;
const T_REFRESH: usize = 2;
const T_STOP: usize = 3;
const T_COPY: usize = 4;
const T_PASTE: usize = 5;
const T_INSTALL_BASE: usize = 6;
const T_NEW: usize = 7;
const T_SWITCH: usize = 30; // 30..: the other windows of the tile, by their place in the list
const T_PROGRAM_INSTALL: usize = 10; // 10..: the offered test programs
/// The area must stop changing this long before the display is asked for its size (milliseconds).
const RESIZE_SETTLE_MS: u64 = 300;
const SAMPLE_EVERY_MS: u64 = 1000;
/// The list of programs is read again this often while nothing runs (milliseconds).
const RECHECK_MS: u64 = 3000;
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

/// The picture of a remote screen: RGBA bytes.
pub struct Frame {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl Frame {
    fn new(w: u32, h: u32) -> Option<Frame> {
        let n = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
        (w > 0 && h > 0).then(|| Frame { w, h, data: vec![0; n] })
    }
    fn width(&self) -> u32 {
        self.w
    }
    fn height(&self) -> u32 {
        self.h
    }
    fn data(&self) -> &[u8] {
        &self.data
    }
    /// Copy a part of the picture in (cut to the picture's edge).
    fn put(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) {
        let x1 = x.saturating_add(w).min(self.w);
        let y1 = y.saturating_add(h).min(self.h);
        for yy in y.min(self.h)..y1 {
            for xx in x.min(self.w)..x1 {
                let from = ((yy - y) as usize * w as usize + (xx - x) as usize) * 4;
                let to = (yy as usize * self.w as usize + xx as usize) * 4;
                if let (Some(s), Some(d)) = (rgba.get(from..from + 4), self.data.get_mut(to..to + 4)) {
                    d.copy_from_slice(s);
                }
            }
        }
    }
}

/// Everything that belongs to one window of the tile. The tile holds one of these in its own fields (the one on screen) and the others here: they are swapped, so the
/// code that draws and serves the window on screen is the same for each.
pub struct Sess {
    win: Option<Box<dyn Window>>,
    screen: Option<Box<dyn Screen>>,
    pm: Option<Frame>,
    pressed: bool,
    last_pos: Option<(i32, i32)>,
    want: (u32, u32),
    want_since: u64,
    sent: (u32, u32),
    usage: Usage,
    cpu: f32,
    peak_kb: u64,
    last_sample: Option<(u64, Usage)>,
    remote_clip: Option<String>,
    program_name: String,
    slot: u32,
}

impl Sess {
    fn empty() -> Sess {
        Sess { win: None, screen: None, pm: None, pressed: false, last_pos: None, want: (0, 0), want_since: 0, sent: (0, 0), usage: Usage::default(), cpu: 0.0, peak_kb: 0, last_sample: None, remote_clip: None, program_name: String::new(), slot: 0 }
    }

    fn close(&mut self) {
        if let Some(mut l) = self.screen.take() {
            l.close();
        }
        if let Some(mut w) = self.win.take() {
            w.stop();
        }
    }
}

type StartResult = Result<Live, String>;

pub struct LinuxWinTile {
    pub phase: Phase,
    pub programs: Vec<Program>,
    pub sel: usize,
    pub note: Option<(String, bool)>,
    rig: Arc<dyn WinRig>,
    started: Option<Receiver<StartResult>>,
    cancel: Arc<AtomicBool>,
    win: Option<Box<dyn Window>>,
    screen: Option<Box<dyn Screen>>,
    pm: Option<Frame>,
    /// The slot of the window on screen (or being started): its display and socket.
    slot: u32,
    /// The other windows of the tile, out of sight but alive.
    parked: Vec<Sess>,
    /// Where the remote screen was drawn last (screen pixels).
    pub view: R,
    pressed: bool,
    last_pos: Option<(i32, i32)>,
    want: (u32, u32),
    want_since: u64,
    sent: (u32, u32),
    pub usage: Usage,
    pub cpu: f32,
    pub peak_kb: u64,
    last_sample: Option<(u64, Usage)>,
    remote_clip: Option<String>,
    redraw: bool,
    pub program_name: String,
    network: bool,
    checked: u64,
    /// The port's clock at the last call (milliseconds).
    now: u64,
    can_install: bool,
    installing: bool,
    full_cli: bool,
    all_hosts: bool,
    pub job_running: bool,
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
        Key::Alt(c) => {
            let mut m = vec![ks::ALT_L];
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

/// Program names (words of them) that are terminal emulators: there Ctrl+C is the interrupt and the copy is Ctrl+Shift+C.
const TERMINAL_WORDS: [&str; 18] = ["xterm", "terminal", "konsole", "alacritty", "kitty", "foot", "wezterm", "urxvt", "rxvt", "tilix", "terminator", "tmux", "sakura", "guake", "st", "lxterminal", "qterminal", "console"];

/// Is this program a terminal emulator (by its name)?
pub fn terminal_like(program: &str) -> bool {
    program.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).any(|w| TERMINAL_WORDS.contains(&w))
}

/// The key to send to the program. With the Mac profile Command+C reaches the window as Ctrl+C and Command+V as Ctrl+V; a terminal program would take
/// the first as an interrupt, so for a terminal they become Ctrl+Shift+C and Ctrl+Shift+V (its copy and paste). Control+C always stays the interrupt;
/// every other program (an editor, an IDE) gets Ctrl+C and Ctrl+V, which are copy and paste there.
pub fn key_for_program(k: &Key, program: &str, from_command: bool) -> Key {
    match k {
        Key::Ctrl(c @ ('c' | 'v')) if from_command && terminal_like(program) => Key::Ctrl(c.to_ascii_uppercase()),
        other => other.clone(),
    }
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

impl LinuxWinTile {
    /// The tile over any port (a fake in tests), with the port's own rig.
    pub fn new(port: &dyn LinuxWinPort) -> LinuxWinTile {
        LinuxWinTile::with_rig(port, port.rig())
    }

    /// The tile over a port and a rig of the caller's.
    pub fn with_rig(port: &dyn LinuxWinPort, rig: Arc<dyn WinRig>) -> LinuxWinTile {
        let mut a = LinuxWinTile {
            phase: Phase::NotInstalled,
            programs: Vec::new(),
            sel: 0,
            note: None,
            rig,
            started: None,
            cancel: Arc::new(AtomicBool::new(false)),
            win: None,
            screen: None,
            pm: None,
            slot: 0,
            parked: Vec::new(),
            view: R::new(0.0, 0.0, 0.0, 0.0),
            pressed: false,
            last_pos: None,
            want: (0, 0),
            want_since: 0,
            sent: (0, 0),
            usage: Usage::default(),
            cpu: 0.0,
            peak_kb: 0,
            last_sample: None,
            remote_clip: None,
            redraw: true,
            program_name: String::new(),
            network: false,
            checked: port.now_ms(),
            now: port.now_ms(),
            can_install: port.can_install(),
            installing: false,
            full_cli: port.full_cli(),
            all_hosts: port.all_hosts(),
            job_running: false,
            install_rx: None,
        };
        a.refresh(port);
        a
    }

    /// Read what is installed (files only) and set the phase when nothing is running.
    pub fn refresh(&mut self, port: &dyn LinuxWinPort) {
        self.now = port.now_ms();
        self.checked = self.now;
        self.job_running = port.base_install_running();
        if matches!(self.phase, Phase::Starting | Phase::Running) {
            return;
        }
        if !self.full_cli {
            self.phase = Phase::NoAccess("Linux windows run programs of your choosing, so they need the full command-line permission (administrators and operators have it).".into());
            return;
        }
        match self.rig.sandbox() {
            Ok(sb) if sb.ready => {
                self.programs = sb.programs;
                self.sel = self.sel.min(self.programs.len().saturating_sub(1));
                self.network = sb.network;
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
    pub fn start(&mut self) {
        let Some(p) = self.programs.get(self.sel).cloned() else { return self.say("Choose a program first (or install a test program).", false) };
        if !self.full_cli {
            return;
        }
        // a sandbox with its network on can reach any server: only an account allowed every host runs it (as the CLI does)
        match self.rig.network() {
            Ok(on) if on && !self.all_hosts => return self.say("The sandbox has its network on, so it can reach any server; your account may not run it. Turn the network off on the Linux apps page.", false),
            Ok(on) => self.network = on,
            Err(e) => return self.say(e, false),
        }
        let Some(slot) = self.free_slot() else { return self.say(format!("At most {} windows run at once in a tile; close one first.", lw::MAX_SLOTS), false) };
        self.slot = slot;
        let (w, h) = wanted_size(self.view);
        let (w, h) = if self.view.w < 1.0 { (1024, 640) } else { (w, h) };
        let rig = self.rig.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (tx, rx) = channel::<StartResult>();
        self.started = Some(rx);
        self.program_name = p.name.clone();
        self.phase = Phase::Starting;
        self.want = (w, h);
        self.sent = (w, h);
        self.note = None;
        std::thread::spawn(move || {
            let r = rig.start(&p, w, h, slot, &cancel);
            // a receiver that is gone (the tile was closed) drops the window here, which stops the sandbox
            let _ = tx.send(r);
        });
        self.redraw = true;
    }

    /// Stop the program and everything of the sandbox that runs.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.started = None;
        if let Some(mut l) = self.screen.take() {
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

    /// Stop every window of the tile (the tile is closing).
    pub fn stop_all(&mut self) {
        self.stop();
        for mut p in std::mem::take(&mut self.parked) {
            p.close();
        }
    }

    /// The slot of the window on screen (for tests and the tab row).
    pub fn slot(&self) -> u32 {
        self.slot
    }

    pub fn running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// How many windows the tile has: the one on screen (also while it starts) and the others.
    pub fn windows(&self) -> usize {
        self.parked.len() + usize::from(self.win.is_some() || self.phase == Phase::Starting)
    }

    /// The names of the other windows, in the order of the tabs.
    pub fn parked_names(&self) -> Vec<String> {
        self.parked.iter().map(|p| p.program_name.clone()).collect()
    }

    /// The lowest slot no window of the tile uses.
    fn free_slot(&self) -> Option<u32> {
        let used = |n: u32| self.parked.iter().any(|p| p.slot == n) || ((self.win.is_some() || self.phase == Phase::Starting) && self.slot == n);
        (0..lw::MAX_SLOTS).find(|n| !used(*n))
    }

    fn swap_sess(&mut self, o: &mut Sess) {
        std::mem::swap(&mut self.win, &mut o.win);
        std::mem::swap(&mut self.screen, &mut o.screen);
        std::mem::swap(&mut self.pm, &mut o.pm);
        std::mem::swap(&mut self.pressed, &mut o.pressed);
        std::mem::swap(&mut self.last_pos, &mut o.last_pos);
        std::mem::swap(&mut self.want, &mut o.want);
        std::mem::swap(&mut self.want_since, &mut o.want_since);
        std::mem::swap(&mut self.sent, &mut o.sent);
        std::mem::swap(&mut self.usage, &mut o.usage);
        std::mem::swap(&mut self.cpu, &mut o.cpu);
        std::mem::swap(&mut self.peak_kb, &mut o.peak_kb);
        std::mem::swap(&mut self.last_sample, &mut o.last_sample);
        std::mem::swap(&mut self.remote_clip, &mut o.remote_clip);
        std::mem::swap(&mut self.program_name, &mut o.program_name);
        std::mem::swap(&mut self.slot, &mut o.slot);
    }

    /// Put the window on screen aside (it keeps running) and show the list, to start another one beside it.
    pub fn new_window(&mut self, port: &dyn LinuxWinPort) {
        if !self.running() {
            return;
        }
        if self.windows() >= lw::MAX_SLOTS as usize {
            return self.say(format!("At most {} windows run at once in a tile; close one first.", lw::MAX_SLOTS), false);
        }
        let mut s = Sess::empty();
        self.swap_sess(&mut s);
        self.parked.push(s);
        self.phase = Phase::Pick;
        self.note = None;
        self.refresh(port);
        self.redraw = true;
    }

    /// Bring the `i`-th other window onto the screen. The one that was on screen goes aside, unless nothing runs there.
    pub fn switch_to(&mut self, i: usize) {
        if i >= self.parked.len() || self.phase == Phase::Starting {
            return;
        }
        if self.running() {
            let mut p = self.parked.remove(i);
            self.swap_sess(&mut p);
            self.parked.insert(i, p);
        } else {
            let mut p = self.parked.remove(i);
            self.swap_sess(&mut p);
            p.close();
        }
        self.phase = Phase::Running;
        self.note = None;
        self.sync_frame();
        self.redraw = true;
    }

    /// The other windows keep their picture and clipboard up to date; one whose program ended is dropped from the list.
    fn serve_parked(&mut self) {
        let mut i = 0;
        while i < self.parked.len() {
            let mut p = self.parked.remove(i);
            self.swap_sess(&mut p);
            self.sync_frame();
            let dead = self.win.as_mut().is_some_and(|w| !w.alive()) || self.screen.as_ref().is_some_and(|l| l.ended().is_some());
            self.swap_sess(&mut p);
            if dead {
                let name = p.program_name.clone();
                p.close();
                self.say(format!("The window {name} ended."), false);
            } else {
                self.parked.insert(i, p);
                i += 1;
            }
        }
    }

    /// The picture's size, when there is one.
    pub fn frame_size(&self) -> (u32, u32) {
        self.pm.as_ref().map_or((0, 0), |p| (p.width(), p.height()))
    }

    fn keys(&self, seq: &[(bool, u32)]) {
        if let Some(s) = &self.screen {
            for (down, sym) in seq {
                s.key(*down, *sym);
            }
        }
    }

    fn pointer(&mut self, x: i32, y: i32, buttons: u8) {
        self.last_pos = Some((x, y));
        if let Some(s) = &self.screen {
            s.pointer(x, y, buttons);
        }
    }

    // ---- the heartbeat

    fn tick(&mut self, port: &dyn LinuxWinPort) {
        self.now = port.now_ms();
        if let Some(rx) = &self.install_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.install_rx = None;
                    self.installing = false;
                    match r {
                        Ok(()) => self.say("Installed. It is in the list now.", true),
                        Err(e) => self.say(e, false),
                    }
                    self.refresh(port);
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
                Ok(Ok((win, screen))) => {
                    self.started = None;
                    self.win = Some(win);
                    self.screen = Some(screen);
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
        if !self.parked.is_empty() {
            self.serve_parked();
        }
        if self.phase == Phase::Running {
            self.sync_frame();
            let dead = self.win.as_mut().is_some_and(|w| !w.alive());
            let ended = self.screen.as_ref().and_then(|l| l.ended()).or_else(|| dead.then(|| "the program ended".to_string()));
            if let Some(why) = ended {
                self.stop();
                if !self.parked.is_empty() {
                    let name = self.program_name.clone();
                    self.phase = Phase::Pick;
                    self.switch_to(self.parked.len() - 1);
                    self.say(format!("The window {name} ended: {why}"), false);
                    return;
                }
                self.phase = Phase::Ended(why);
                self.redraw = true;
                return;
            }
            self.maybe_resize();
            self.sample();
        } else if matches!(self.phase, Phase::NotInstalled | Phase::Pick) && self.now.saturating_sub(self.checked) > RECHECK_MS {
            self.refresh(port);
        }
    }

    /// Take what the display sent: copy the changed parts of its picture into ours.
    fn sync_frame(&mut self) {
        let Some(screen) = &self.screen else { return };
        let mut clip = None;
        let mut changed = false;
        for u in screen.take_updates() {
            match u {
                ScreenUpdate::Full { w, h, rgba } => {
                    self.pm = Frame::new(w.max(1), h.max(1));
                    if let Some(pm) = &mut self.pm {
                        pm.put(0, 0, w, h, &rgba);
                    }
                    changed = true;
                }
                ScreenUpdate::Rect { x, y, w, h, rgba } => {
                    if let Some(pm) = &mut self.pm {
                        pm.put(x, y, w, h, &rgba);
                    }
                    changed = true;
                }
                ScreenUpdate::Clip(t) => clip = Some(t),
            }
        }
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
            self.want_since = self.now;
            return;
        }
        if want != self.sent && want != self.frame_size() && self.now.saturating_sub(self.want_since) >= RESIZE_SETTLE_MS {
            self.sent = want;
            let ok = self.screen.as_ref().is_some_and(|s| s.request_resize(want.0, want.1));
            if !ok {
                // a server that cannot resize keeps its size; the picture is drawn as it is
                self.sent = self.frame_size();
            }
        }
    }

    fn sample(&mut self) {
        let Some(pid) = self.win.as_ref().and_then(|w| w.pid()) else { return };
        let now = self.now;
        if self.last_sample.is_some_and(|(t, _)| now.saturating_sub(t) < SAMPLE_EVERY_MS) {
            return;
        }
        let u = self.rig.usage(pid);
        if let Some((t, before)) = self.last_sample {
            self.cpu = lw::cpu_percent(before, u, now.saturating_sub(t) as f32 / 1000.0);
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
        let (strip, mut view) = layout(area, s);
        if !self.parked.is_empty() {
            // one row of tabs: the window on screen is marked, a click on another brings it forward
            let size = 11.0 * s;
            let row_h = size * 2.4;
            let row = R::new(view.x, view.y, view.w, row_h.min(view.h));
            g.rect(row.x, row.y, row.w, row.h, iyakki_uikit::ui::CARD);
            let mut x = row.x + 6.0 * s;
            let by = row.y + (row.h - size * 2.1) / 2.0;
            let mut tabs: Vec<(String, Option<usize>)> = self.parked.iter().enumerate().map(|(i, p)| (format!("{}: {}", p.slot + 1, p.program_name), Some(i))).collect();
            tabs.push((format!("{}: {}", self.slot + 1, self.program_name), None));
            tabs.sort_by_key(|(t, _)| t.clone());
            for (label, which) in tabs {
                let label = g.fit(size, &label, Face::Bold, 150.0 * s);
                match which {
                    Some(i) => x += g.button(x, by, size, &label, MUTED, Event::Tab(T_SWITCH + i)) + 6.0 * s,
                    None => {
                        let w = g.text_width(size, &label, Face::Bold) + size * 1.7;
                        g.rrect(x, by, w, size * 2.1, 6.0 * s, iyakki_uikit::ui::CARD_SEL);
                        g.text(x + size * 0.85, by + (size * 2.1 - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, &label, Face::Bold);
                        x += w + 6.0 * s;
                    }
                }
            }
            view = R::new(view.x, view.y + row.h, view.w, (view.h - row.h).max(0.0));
        }
        self.view = view;
        g.rect(strip.x, strip.y, strip.w, strip.h, iyakki_uikit::ui::CARD);
        let size = 11.0 * s;
        // buttons from the right: Stop, Paste into window, Copy from window, while the text keeps some room
        let mut x = strip.x + strip.w - 6.0 * s;
        let by = strip.y + (strip.h - size * 2.1) / 2.0;
        let min_text = 110.0 * s;
        for (label, ev, accent) in [("Stop", T_STOP, ERR), ("New window", T_NEW, ACCENT), ("Paste into window", T_PASTE, ACCENT), ("Copy from window", T_COPY, ACCENT)] {
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
        let dst = g.pm_mut().data_mut();
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

    fn panel(&mut self, g: &mut Gfx, area: R, s: f32) {
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
        if !self.parked.is_empty() && self.phase != Phase::Starting {
            let mut bx = x;
            g.text(bx, y, size, MUTED, "Running:", Face::Sans);
            bx += g.text_width(size, "Running:", Face::Sans) + 10.0 * size * 0.5;
            for (i, p) in self.parked.iter().enumerate() {
                bx += g.button(bx, y - size * 0.4, size, &format!("{}: {}", p.slot + 1, p.program_name), ACCENT, Event::Tab(T_SWITCH + i)) + 8.0 * s;
            }
            y += size * 3.0;
        }
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
                        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, iyakki_uikit::ui::CARD_SEL);
                    } else if g.hovered(r) {
                        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, iyakki_uikit::ui::CARD_HOVER);
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
            }
            Phase::Running => {}
        }
        if let Some((t, good)) = self.note.clone() {
            para(g, &mut y, &t, if good { OK } else { ERR });
        }
    }

    /// Install the base system of the Linux windows (a detached job; administrators).
    pub fn install_base(&mut self, port: &dyn LinuxWinPort) {
        if !self.can_install {
            self.say("Only an administrator can install it.", false);
            return;
        }
        match port.start_base_install() {
            Ok(()) => {
                self.job_running = true;
                self.say("Started: Install Linux windows. Progress is beside the clock.", true);
            }
            Err(e) => self.say(e, false),
        }
    }

    /// Install one of the test programs on a thread (apt takes a while); the list refreshes after.
    fn install_program(&mut self, i: usize) {
        let Some((pkg, _)) = lw::TEST_PROGRAMS.get(i) else { return };
        if !self.can_install {
            return self.say("Only an administrator can install programs for the sandbox.", false);
        }
        if self.installing {
            return self.say("An install is running already.", false);
        }
        self.installing = true;
        self.say(format!("Installing {pkg} with apt: this takes a minute (the network of the sandbox is on only while it runs) ..."), true);
        let pkg = pkg.to_string();
        let rig = self.rig.clone();
        let (tx, rx) = channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let r = rig.install_program(&pkg);
            let _ = tx.send(r);
        });
        self.install_rx = Some(rx);
    }
}

impl LinuxWinTile {
    pub fn title(&self) -> &'static str {
        "LINUX WINDOW"
    }

    pub fn icon(&self) -> Icon {
        Icon::Cli
    }

    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    pub fn hint(&self) -> String {
        if self.running() {
            "Keys (Alt too), mouse and wheel go to the program · New window opens another beside it · Stop closes this one".into()
        } else {
            "Up/Down choose · Enter start · R refresh · Esc back".into()
        }
    }

    pub fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    pub fn wants_alt(&self) -> bool {
        self.running()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn LinuxWinPort) -> Answer {
        self.now = port.now_ms();
        match ev {
            Event::Tick => self.tick(port),
            Event::Tab(T_START) => self.start(),
            Event::Tab(T_REFRESH) => {
                if matches!(self.phase, Phase::Ended(_)) {
                    self.phase = Phase::Pick;
                }
                self.note = None;
                self.refresh(port);
            }
            Event::Tab(T_STOP) => {
                self.stop();
                if self.parked.is_empty() {
                    self.phase = Phase::Pick;
                    self.refresh(port);
                } else {
                    self.phase = Phase::Pick;
                    self.switch_to(self.parked.len() - 1);
                }
            }
            Event::Tab(T_NEW) => self.new_window(port),
            Event::Tab(i) if (T_SWITCH..T_SWITCH + lw::MAX_SLOTS as usize).contains(i) => self.switch_to(i - T_SWITCH),
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
            Event::Tab(T_INSTALL_BASE) => self.install_base(port),
            Event::Tab(i) if (T_PROGRAM_INSTALL..T_PROGRAM_INSTALL + lw::TEST_PROGRAMS.len()).contains(i) => self.install_program(i - T_PROGRAM_INSTALL),
            Event::Row(i) if self.phase == Phase::Pick => {
                self.sel = (*i).min(self.programs.len().saturating_sub(1));
                self.redraw = true;
            }
            Event::Key(k) if self.running() => {
                let seq = key_syms(&key_for_program(k, &self.program_name, port.last_press_was_command()));
                self.keys(&seq);
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
                Key::Enter if self.phase == Phase::Pick => self.start(),
                Key::Char('r') | Key::Char('R') => {
                    self.refresh(port);
                }
                Key::Esc => return Answer::Back,
                _ => {}
            },
            Event::Paste(t) if self.running() => self.paste_text(t),
            Event::AreaAt(AREA_WIN, x, y) if self.running() => {
                let (px, py) = (*x as i32, *y as i32);
                if iyakki_uikit::ui::dragging() && self.pressed {
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
        Answer::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, _port: &dyn LinuxWinPort, _tick: u64) {
        if self.running() {
            self.draw_running(g, area, s);
        } else {
            // the size a start asks the display for: what the window area will be below its strip
            self.view = layout(area, s).1;
            self.panel(g, area, s);
        }
    }
}

impl LinuxWinTile {
    /// Put typed text on the program's clipboard and press Ctrl+V there. Only ever called for an explicit paste (the button or a paste).
    pub fn paste_text(&mut self, t: &str) {
        let t: String = t.chars().take(64 * 1024).collect();
        let seq = paste_keys();
        if let Some(s) = &self.screen {
            s.cut_text(&t);
        }
        self.keys(&seq);
        self.say("Pasted your clipboard text into the program (as text only).", true);
    }

    /// For tests: use an already made screen as the running window.
    pub fn attach_for_test(&mut self, screen: Box<dyn Screen>) {
        self.screen = Some(screen);
        self.phase = Phase::Running;
        self.program_name = "test".into();
    }
}
