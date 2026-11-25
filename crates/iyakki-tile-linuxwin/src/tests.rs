//! The LINUX WINDOW tile on a fake port, a fake rig, a fake screen and a fake display process: no sandbox, no socket, no `/proc`. The clock is a
//! number the test sets; a start (which runs on a thread of its own) is awaited by blocking on its channel, never by a sleep.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use iyakki_tile_api::linuxwin::{Live, Program, SandboxInfo, Screen, ScreenUpdate, Usage, WinRig, Window};
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{clipboard, Event, Key, Nav, R};

use super::*;

// ---- the fakes

#[derive(Default)]
struct ScreenState {
    updates: Mutex<VecDeque<ScreenUpdate>>,
    pointer: Mutex<Vec<(i32, i32, u8)>>,
    keys: Mutex<Vec<(bool, u32)>>,
    cut: Mutex<Vec<String>>,
    resizes: Mutex<Vec<(u32, u32)>>,
    resize_ok: Mutex<bool>,
    ended: Mutex<Option<String>>,
    closed: Mutex<bool>,
}

struct FakeScreen(Arc<ScreenState>);

impl Screen for FakeScreen {
    fn ready(&self) -> bool {
        true
    }
    fn ended(&self) -> Option<String> {
        self.0.ended.lock().unwrap().clone()
    }
    fn take_updates(&self) -> Vec<ScreenUpdate> {
        self.0.updates.lock().unwrap().drain(..).collect()
    }
    fn pointer(&self, x: i32, y: i32, buttons: u8) {
        self.0.pointer.lock().unwrap().push((x, y, buttons));
    }
    fn key(&self, down: bool, keysym: u32) {
        self.0.keys.lock().unwrap().push((down, keysym));
    }
    fn cut_text(&self, text: &str) {
        self.0.cut.lock().unwrap().push(text.to_string());
    }
    fn request_resize(&self, w: u32, h: u32) -> bool {
        self.0.resizes.lock().unwrap().push((w, h));
        *self.0.resize_ok.lock().unwrap()
    }
    fn close(&mut self) {
        *self.0.closed.lock().unwrap() = true;
    }
}

#[derive(Default)]
struct WinState {
    alive: Mutex<bool>,
    stopped: Mutex<bool>,
}

struct FakeWindow(u32, Arc<WinState>);

impl Window for FakeWindow {
    fn alive(&mut self) -> bool {
        *self.1.alive.lock().unwrap()
    }
    fn pid(&self) -> Option<u32> {
        Some(self.0)
    }
    fn stop(&mut self) {
        *self.1.stopped.lock().unwrap() = true;
    }
}

/// What one start made.
struct Made {
    program: String,
    size: (u32, u32),
    slot: u32,
    screen: Arc<ScreenState>,
    win: Arc<WinState>,
}

struct Rig {
    sandbox: Mutex<Result<SandboxInfo, String>>,
    network: Mutex<Result<bool, String>>,
    start_error: Mutex<Option<String>>,
    made: Mutex<Vec<Made>>,
    usage: Mutex<Usage>,
    usage_asked: Mutex<Vec<u32>>,
    sandbox_reads: Mutex<usize>,
    installed: Mutex<Vec<String>>,
    install_result: Mutex<Result<(), String>>,
}

fn program(id: &str, name: &str) -> Program {
    Program { id: id.into(), name: name.into(), exec: vec![id.into()] }
}

impl Rig {
    fn new() -> Arc<Rig> {
        Arc::new(Rig {
            sandbox: Mutex::new(Ok(SandboxInfo { ready: true, programs: vec![program("xterm", "XTerm"), program("mousepad", "Mousepad")], network: false })),
            network: Mutex::new(Ok(false)),
            start_error: Mutex::new(None),
            made: Mutex::new(vec![]),
            usage: Mutex::new(Usage { procs: 2, rss_kb: 2048, ticks: 0 }),
            usage_asked: Mutex::new(vec![]),
            sandbox_reads: Mutex::new(0),
            installed: Mutex::new(vec![]),
            install_result: Mutex::new(Ok(())),
        })
    }
    fn screen(&self, i: usize) -> Arc<ScreenState> {
        self.made.lock().unwrap()[i].screen.clone()
    }
    fn win(&self, i: usize) -> Arc<WinState> {
        self.made.lock().unwrap()[i].win.clone()
    }
}

impl WinRig for Rig {
    fn sandbox(&self) -> Result<SandboxInfo, String> {
        *self.sandbox_reads.lock().unwrap() += 1;
        self.sandbox.lock().unwrap().clone()
    }
    fn network(&self) -> Result<bool, String> {
        self.network.lock().unwrap().clone()
    }
    fn start(&self, p: &Program, w: u32, h: u32, slot: u32, _cancel: &AtomicBool) -> Result<Live, String> {
        if let Some(e) = self.start_error.lock().unwrap().clone() {
            return Err(e);
        }
        let screen = Arc::new(ScreenState { resize_ok: Mutex::new(true), ..Default::default() });
        let win = Arc::new(WinState { alive: Mutex::new(true), ..Default::default() });
        screen.updates.lock().unwrap().push_back(ScreenUpdate::Full { w: 8, h: 6, rgba: red(8 * 6) });
        self.made.lock().unwrap().push(Made { program: p.name.clone(), size: (w, h), slot, screen: screen.clone(), win: win.clone() });
        Ok((Box::new(FakeWindow(4242 + slot, win)), Box::new(FakeScreen(screen))))
    }
    fn usage(&self, pid: u32) -> Usage {
        self.usage_asked.lock().unwrap().push(pid);
        *self.usage.lock().unwrap()
    }
    fn install_program(&self, package: &str) -> Result<(), String> {
        self.installed.lock().unwrap().push(package.to_string());
        self.install_result.lock().unwrap().clone()
    }
}

fn red(n: usize) -> Vec<u8> {
    (0..n).flat_map(|_| [255, 0, 0, 255]).collect()
}

struct Port {
    rig: Arc<Rig>,
    can_install: bool,
    full_cli: bool,
    all_hosts: bool,
    job: Mutex<bool>,
    install_answer: Result<(), String>,
    installs: Mutex<usize>,
    command: Cell<bool>,
    clock: Cell<u64>,
}

impl Port {
    fn new() -> Port {
        Port { rig: Rig::new(), can_install: true, full_cli: true, all_hosts: true, job: Mutex::new(false), install_answer: Ok(()), installs: Mutex::new(0), command: Cell::new(false), clock: Cell::new(1_000) }
    }
    fn advance(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
    }
}

impl LinuxWinPort for Port {
    fn rig(&self) -> Arc<dyn WinRig> {
        self.rig.clone()
    }
    fn can_install(&self) -> bool {
        self.can_install
    }
    fn full_cli(&self) -> bool {
        self.full_cli
    }
    fn all_hosts(&self) -> bool {
        self.all_hosts
    }
    fn base_install_running(&self) -> bool {
        *self.job.lock().unwrap()
    }
    fn start_base_install(&self) -> Result<(), String> {
        *self.installs.lock().unwrap() += 1;
        self.install_answer.clone()
    }
    fn last_press_was_command(&self) -> bool {
        self.command.get()
    }
    fn now_ms(&self) -> u64 {
        self.clock.get()
    }
}

// ---- helpers

const AREA: R = R { x: 0.0, y: 46.0, w: 1000.0, h: 600.0 };

fn draw(t: &mut LinuxWinTile, p: &Port, w: u32, h: u32, area: R) -> Gfx {
    let mut g = Gfx::new(w, h);
    t.render(&mut g, area, 1.0, p, 0);
    g
}

/// Wait for the start's answer (on its channel, no clock) and let the tile take it on its next tick.
fn finish_start(t: &mut LinuxWinTile, p: &Port) {
    let rx = t.started.take().expect("a start is running");
    let r = rx.recv().expect("the start answers");
    let (tx, rx) = channel();
    tx.send(r).unwrap();
    t.started = Some(rx);
    t.handle(&Event::Tick, p);
}

/// A tile that has drawn once (so it knows its area) and runs the first program.
fn running(p: &Port) -> LinuxWinTile {
    let mut t = LinuxWinTile::new(p);
    draw(&mut t, p, 1000, 700, AREA);
    t.handle(&Event::Tab(T_START), p);
    assert_eq!(t.phase, Phase::Starting);
    finish_start(&mut t, p);
    assert!(t.running(), "{:?}", t.phase);
    t
}

fn pix(g: &Gfx, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * g.width() + x) * 4) as usize;
    let d = g.rgba();
    [d[i], d[i + 1], d[i + 2]]
}

// ---- pure parts (moved with the code)

#[test]
fn keys_become_keysym_sequences_with_modifiers_around_the_key() {
    assert_eq!(key_syms(&Key::Char('a')), [(true, 'a' as u32), (false, 'a' as u32)]);
    assert_eq!(key_syms(&Key::Ctrl('c')), [(true, ks::CONTROL_L), (true, 'c' as u32), (false, 'c' as u32), (false, ks::CONTROL_L)]);
    assert_eq!(key_syms(&Key::Ctrl('C')), [(true, ks::CONTROL_L), (true, ks::SHIFT_L), (true, 'c' as u32), (false, 'c' as u32), (false, ks::SHIFT_L), (false, ks::CONTROL_L)]);
    assert_eq!(key_syms(&Key::Alt('f'))[0], (true, ks::ALT_L));
    assert_eq!(key_syms(&Key::BackTab), [(true, ks::SHIFT_L), (true, ks::ISO_LEFT_TAB), (false, ks::ISO_LEFT_TAB), (false, ks::SHIFT_L)]);
    assert_eq!(key_syms(&Key::F(5))[0], (true, ks::function_key(5)));
    assert_eq!(key_syms(&Key::Enter)[0], (true, ks::RETURN));
    assert_eq!(key_syms(&Key::Nav { nav: Nav::Left, shift: true, ctrl: true }), [(true, ks::CONTROL_L), (true, ks::SHIFT_L), (true, ks::LEFT), (false, ks::LEFT), (false, ks::SHIFT_L), (false, ks::CONTROL_L)]);
    for k in [Key::Esc, Key::Tab, Key::Backspace, Key::Delete, Key::Up, Key::Down, Key::Left, Key::Right, Key::Home, Key::End, Key::PageUp, Key::PageDown] {
        let s = key_syms(&k);
        assert_eq!((s.len(), s[0].0, s[1].0, s[0].1 == s[1].1), (2, true, false, true), "{k:?}");
    }
    assert_eq!(key_syms(&Key::Char('€'))[0].1, 0x0100_0000 + '€' as u32, "other Unicode is carried as a Unicode keysym");
    assert_eq!(paste_keys(), key_syms(&Key::Ctrl('v')));
}

#[test]
fn the_layout_never_goes_negative_and_the_wanted_size_stays_in_bounds() {
    for (w, h, s) in [(0.0, 0.0, 1.0), (10.0, 5.0, 3.0), (1280.0, 720.0, 1.0), (-4.0, -9.0, 2.0)] {
        let (strip, view) = layout(R::new(0.0, 0.0, w, h), s);
        assert!(strip.w >= 0.0 && strip.h >= 0.0 && view.w >= 0.0 && view.h >= 0.0, "{w}x{h}");
    }
    assert_eq!(wanted_size(R::new(0.0, 0.0, 0.0, 0.0)), (lw::MIN_SIZE, lw::MIN_SIZE));
    assert_eq!(wanted_size(R::new(0.0, 0.0, 100_000.0, 100_000.0)), (lw::MAX_SIZE, lw::MAX_SIZE));
    assert_eq!(wanted_size(R::new(0.0, 0.0, 800.0, 600.0)), (800, 600));
}

#[test]
fn a_press_is_a_hit_only_inside_the_area_also_in_a_pane() {
    let view = R::new(100.0, 80.0, 400.0, 300.0);
    assert_eq!(to_remote(view, 100, 80), Some((0, 0)));
    assert_eq!(to_remote(view, 499, 379), Some((399, 299)));
    assert_eq!(to_remote(view, 500, 200), None);
    assert_eq!(to_remote(view, 99, 200), None);
    assert_eq!(to_remote(view, 200, 380), None);
}

#[test]
fn memory_is_said_in_megabytes_or_gigabytes_and_terminals_are_told_by_name() {
    assert_eq!(mem_text(2048), "2 MB");
    assert_eq!(mem_text(1024 * 1024), "1.0 GB");
    assert_eq!(mem_text(1536 * 1024), "1.5 GB");
    for t in ["xterm", "XTerm", "GNOME Terminal", "alacritty", "st", "lxterminal"] {
        assert!(terminal_like(t), "{t}");
    }
    for t in ["mousepad", "gedit", "stellarium", "xclock"] {
        assert!(!terminal_like(t), "{t}");
    }
    assert_eq!(key_for_program(&Key::Ctrl('c'), "xterm", true), Key::Ctrl('C'));
    assert_eq!(key_for_program(&Key::Ctrl('c'), "xterm", false), Key::Ctrl('c'), "Control+C stays the interrupt");
    assert_eq!(key_for_program(&Key::Ctrl('v'), "mousepad", true), Key::Ctrl('v'));
}

// ---- phases and the list

#[test]
fn without_the_full_command_line_permission_the_tile_says_so_and_asks_the_rig_for_nothing() {
    let mut p = Port::new();
    p.full_cli = false;
    let mut t = LinuxWinTile::new(&p);
    assert!(matches!(t.phase, Phase::NoAccess(_)));
    t.handle(&Event::Tab(T_START), &p);
    assert_eq!((*p.rig.sandbox_reads.lock().unwrap(), p.rig.made.lock().unwrap().len()), (0, 0));
    draw(&mut t, &p, 800, 600, R::new(0.0, 46.0, 800.0, 554.0));
}

#[test]
fn nothing_installed_offers_the_install_and_an_administrator_starts_it_once_through_the_port() {
    let p = Port::new();
    *p.rig.sandbox.lock().unwrap() = Ok(SandboxInfo { ready: false, programs: vec![], network: false });
    let mut t = LinuxWinTile::new(&p);
    assert_eq!(t.phase, Phase::NotInstalled);
    let g = draw(&mut t, &p, 1000, 700, AREA);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_INSTALL_BASE)));
    t.handle(&Event::Tab(T_INSTALL_BASE), &p);
    assert_eq!(*p.installs.lock().unwrap(), 1);
    assert!(t.job_running && t.note.as_ref().is_some_and(|n| n.1));
    // the job is read from the port on every refresh
    *p.job.lock().unwrap() = false;
    t.refresh(&p);
    assert!(!t.job_running);
}

#[test]
fn an_ordinary_account_cannot_install_and_a_refused_install_says_why() {
    let mut p = Port::new();
    p.can_install = false;
    *p.rig.sandbox.lock().unwrap() = Ok(SandboxInfo { ready: false, programs: vec![], network: false });
    let mut t = LinuxWinTile::new(&p);
    t.handle(&Event::Tab(T_INSTALL_BASE), &p);
    assert_eq!(*p.installs.lock().unwrap(), 0);
    assert_eq!(t.note.as_ref().unwrap().0, "Only an administrator can install it.");
    let mut q = Port::new();
    q.install_answer = Err("another install is running".into());
    *q.rig.sandbox.lock().unwrap() = Ok(SandboxInfo { ready: false, programs: vec![], network: false });
    let mut u = LinuxWinTile::new(&q);
    u.handle(&Event::Tab(T_INSTALL_BASE), &q);
    assert!(!u.job_running);
    assert_eq!(u.note.as_ref().unwrap(), &("another install is running".to_string(), false));
}

#[test]
fn a_rig_that_cannot_be_made_ends_the_tile_with_the_reason_and_the_list_is_read_again_after_three_seconds() {
    let p = Port::new();
    *p.rig.sandbox.lock().unwrap() = Err("no sandbox manager".into());
    let mut t = LinuxWinTile::new(&p);
    assert_eq!(t.phase, Phase::Ended("no sandbox manager".into()));
    t.handle(&Event::Tab(T_REFRESH), &p);
    assert_eq!(t.phase, Phase::Ended("no sandbox manager".into()), "still the same reason");
    *p.rig.sandbox.lock().unwrap() = Ok(SandboxInfo { ready: true, programs: vec![program("a", "A")], network: true });
    t.handle(&Event::Tab(T_REFRESH), &p);
    assert_eq!((t.phase.clone(), t.programs.len()), (Phase::Pick, 1));
    // while nothing runs the list is read again every three seconds, not before
    let reads = *p.rig.sandbox_reads.lock().unwrap();
    p.advance(2_999);
    t.handle(&Event::Tick, &p);
    assert_eq!(*p.rig.sandbox_reads.lock().unwrap(), reads);
    p.advance(2);
    t.handle(&Event::Tick, &p);
    assert_eq!(*p.rig.sandbox_reads.lock().unwrap(), reads + 1);
}

#[test]
fn the_list_is_chosen_with_the_keys_and_the_rows_and_start_without_a_program_says_so() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    assert_eq!((t.phase.clone(), t.sel), (Phase::Pick, 0));
    t.handle(&Event::Key(Key::Down), &p);
    t.handle(&Event::Key(Key::Down), &p);
    assert_eq!(t.sel, 1, "stops at the last program");
    t.handle(&Event::Key(Key::Up), &p);
    assert_eq!(t.sel, 0);
    t.handle(&Event::Row(9), &p);
    assert_eq!(t.sel, 1);
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
    let mut e = Port::new();
    *e.rig.sandbox.lock().unwrap() = Ok(SandboxInfo { ready: true, programs: vec![], network: false });
    e.can_install = false;
    let mut u = LinuxWinTile::new(&e);
    u.handle(&Event::Key(Key::Enter), &e);
    assert_eq!(u.note.as_ref().unwrap().0, "Choose a program first (or install a test program).");
    assert!(e.rig.made.lock().unwrap().is_empty());
}

#[test]
fn a_test_program_is_installed_on_a_thread_once_at_a_time_by_an_administrator_and_the_list_refreshes() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    t.handle(&Event::Tab(T_PROGRAM_INSTALL + 1), &p);
    assert!(t.installing && t.note.as_ref().unwrap().0.starts_with("Installing x11-apps"));
    t.handle(&Event::Tab(T_PROGRAM_INSTALL), &p);
    assert_eq!(t.note.as_ref().unwrap().0, "An install is running already.");
    let rx = t.install_rx.take().unwrap();
    let r = rx.recv().unwrap();
    let (tx, rx) = channel();
    tx.send(r).unwrap();
    t.install_rx = Some(rx);
    let reads = *p.rig.sandbox_reads.lock().unwrap();
    t.handle(&Event::Tick, &p);
    assert_eq!(p.rig.installed.lock().unwrap().as_slice(), ["x11-apps"]);
    assert!(!t.installing && t.note.as_ref().unwrap().1 && t.note.as_ref().unwrap().0.starts_with("Installed"));
    assert_eq!(*p.rig.sandbox_reads.lock().unwrap(), reads + 1);
    // a failed install says why
    *p.rig.install_result.lock().unwrap() = Err("apt failed".into());
    t.handle(&Event::Tab(T_PROGRAM_INSTALL), &p);
    let rx = t.install_rx.take().unwrap();
    let r = rx.recv().unwrap();
    let (tx, rx) = channel();
    tx.send(r).unwrap();
    t.install_rx = Some(rx);
    t.handle(&Event::Tick, &p);
    assert_eq!(t.note.as_ref().unwrap(), &("apt failed".to_string(), false));
    // an ordinary account cannot
    let mut q = Port::new();
    q.can_install = false;
    let mut u = LinuxWinTile::new(&q);
    u.handle(&Event::Tab(T_PROGRAM_INSTALL), &q);
    assert!(q.rig.installed.lock().unwrap().is_empty() && !u.installing);
    assert!(u.note.as_ref().unwrap().0.contains("Only an administrator"));
}

// ---- starting and running

#[test]
fn a_start_asks_for_the_area_size_the_slot_and_the_chosen_program_and_the_picture_is_drawn_in_the_area() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    let g0 = draw(&mut t, &p, 1000, 700, AREA);
    assert!(g0.hits.iter().any(|h| h.ev == Event::Tab(T_START)));
    t.handle(&Event::Key(Key::Down), &p);
    t.handle(&Event::Key(Key::Enter), &p);
    assert_eq!(t.phase, Phase::Starting);
    assert_eq!(t.windows(), 1, "a window that is starting counts");
    finish_start(&mut t, &p);
    let made = p.rig.made.lock().unwrap();
    let want = wanted_size(layout(AREA, 1.0).1);
    assert_eq!((made[0].program.as_str(), made[0].size, made[0].slot), ("Mousepad", want, 0));
    drop(made);
    assert!(t.running() && t.wants_alt());
    assert_eq!(t.frame_size(), (8, 6));
    let g = draw(&mut t, &p, 1000, 700, AREA);
    let v = t.view;
    assert_eq!(pix(&g, v.x as u32 + 3, v.y as u32 + 3), [255, 0, 0], "the picture's pixels are in the area");
    assert_eq!(pix(&g, v.x as u32 + 20, v.y as u32 + 20), [14, 16, 24], "outside the picture the area is its dark background");
    assert!(g.hits.iter().any(|h| h.ev == Event::Area(AREA_WIN)));
    for ev in [Event::Tab(T_STOP), Event::Tab(T_NEW), Event::Tab(T_PASTE), Event::Tab(T_COPY)] {
        assert!(g.hits.iter().any(|h| h.ev == ev), "{ev:?}");
    }
}

#[test]
fn a_start_without_an_area_yet_uses_the_default_size() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    t.handle(&Event::Tab(T_START), &p);
    finish_start(&mut t, &p);
    assert_eq!(p.rig.made.lock().unwrap()[0].size, (1024, 640));
}

#[test]
fn a_sandbox_with_its_network_on_runs_only_for_an_account_allowed_every_host() {
    let mut p = Port::new();
    p.all_hosts = false;
    *p.rig.network.lock().unwrap() = Ok(true);
    let mut t = LinuxWinTile::new(&p);
    t.handle(&Event::Tab(T_START), &p);
    assert_eq!(t.phase, Phase::Pick);
    assert!(t.note.as_ref().unwrap().0.contains("network on") && p.rig.made.lock().unwrap().is_empty());
    let q = Port::new();
    *q.rig.network.lock().unwrap() = Ok(true);
    let mut u = LinuxWinTile::new(&q);
    u.handle(&Event::Tab(T_START), &q);
    finish_start(&mut u, &q);
    assert!(u.running());
    assert!(u.strip_text().ends_with("network on"), "{}", u.strip_text());
    *q.rig.network.lock().unwrap() = Err("no such sandbox".into());
    let mut v = LinuxWinTile::new(&q);
    v.handle(&Event::Tab(T_START), &q);
    assert_eq!(v.note.as_ref().unwrap().0, "no such sandbox");
}

#[test]
fn a_display_that_cannot_start_ends_with_the_reason_and_back_to_the_list_goes_on() {
    let p = Port::new();
    *p.rig.start_error.lock().unwrap() = Some("the display did not answer".into());
    let mut t = LinuxWinTile::new(&p);
    t.handle(&Event::Tab(T_START), &p);
    finish_start(&mut t, &p);
    assert_eq!(t.phase, Phase::Ended("the display did not answer".into()));
    let g = draw(&mut t, &p, 800, 600, AREA);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_REFRESH)));
    t.handle(&Event::Tab(T_REFRESH), &p);
    assert_eq!(t.phase, Phase::Pick);
    assert_eq!(t.windows(), 0);
}

#[test]
fn a_start_whose_thread_vanished_ends_the_tile_instead_of_waiting_for_ever() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    t.phase = Phase::Starting;
    let (tx, rx) = channel::<StartResult>();
    drop(tx);
    t.started = Some(rx);
    t.handle(&Event::Tick, &p);
    assert_eq!(t.phase, Phase::Ended("the start was interrupted".into()));
}

#[test]
fn the_picture_follows_the_screen_whole_then_by_parts_and_a_part_outside_it_is_cut() {
    let p = Port::new();
    let mut t = running(&p);
    let s = p.rig.screen(0);
    s.updates.lock().unwrap().push_back(ScreenUpdate::Rect { x: 6, y: 4, w: 4, h: 4, rgba: [0, 255, 0, 255].repeat(16) });
    t.handle(&Event::Tick, &p);
    assert!(t.wants_redraw());
    let pm = t.pm.as_ref().unwrap();
    let at = |x: usize, y: usize| pm.data()[(y * 8 + x) * 4..(y * 8 + x) * 4 + 3].to_vec();
    assert_eq!((at(5, 4), at(6, 4), at(7, 5)), (vec![255, 0, 0], vec![0, 255, 0], vec![0, 255, 0]), "only the part inside the picture changed");
    s.updates.lock().unwrap().push_back(ScreenUpdate::Full { w: 4, h: 3, rgba: [0, 0, 255, 255].repeat(12) });
    t.handle(&Event::Tick, &p);
    assert_eq!(t.frame_size(), (4, 3), "a new size is a new picture");
    s.updates.lock().unwrap().push_back(ScreenUpdate::Rect { x: 100, y: 100, w: 2, h: 2, rgba: vec![0; 16] });
    t.handle(&Event::Tick, &p);
    assert_eq!(t.frame_size(), (4, 3));
}

#[test]
fn keys_go_to_the_program_with_the_command_chord_mapped_for_terminals_and_nothing_goes_when_it_is_not_running() {
    let p = Port::new();
    let mut idle = LinuxWinTile::new(&p);
    idle.handle(&Event::Key(Key::Char('x')), &p);
    assert!(p.rig.made.lock().unwrap().is_empty());
    let mut t = running(&p);
    let s = p.rig.screen(0);
    t.handle(&Event::Key(Key::Char('a')), &p);
    assert_eq!(s.keys.lock().unwrap().as_slice(), [(true, 'a' as u32), (false, 'a' as u32)]);
    s.keys.lock().unwrap().clear();
    t.program_name = "xterm".into();
    p.command.set(true);
    t.handle(&Event::Key(Key::Ctrl('c')), &p);
    assert_eq!(s.keys.lock().unwrap().iter().filter(|k| k.0).map(|k| k.1).collect::<Vec<_>>(), [ks::CONTROL_L, ks::SHIFT_L, 'c' as u32], "Command+C in a terminal is its copy");
    s.keys.lock().unwrap().clear();
    p.command.set(false);
    t.handle(&Event::Key(Key::Ctrl('c')), &p);
    assert_eq!(s.keys.lock().unwrap().iter().filter(|k| k.0).map(|k| k.1).collect::<Vec<_>>(), [ks::CONTROL_L, 'c' as u32], "Control+C stays the interrupt");
}

#[test]
fn the_pointer_moves_presses_drags_releases_right_clicks_and_scrolls_inside_the_area_only() {
    let p = Port::new();
    let mut t = running(&p);
    draw(&mut t, &p, 1000, 700, AREA);
    let s = p.rig.screen(0);
    let v = t.view;
    let (vx, vy) = (v.x as i32, v.y as i32);
    t.handle(&Event::MouseMove(vx + 10, vy + 20), &p);
    assert_eq!(s.pointer.lock().unwrap().as_slice(), [(10, 20, 0)]);
    t.handle(&Event::MouseMove(vx + 10, vy + 20), &p);
    assert_eq!(s.pointer.lock().unwrap().len(), 1, "the same place is not sent again");
    t.handle(&Event::MouseMove(vx - 5, vy + 20), &p);
    assert_eq!(s.pointer.lock().unwrap().len(), 1, "outside the area nothing is sent");
    s.pointer.lock().unwrap().clear();
    t.handle(&Event::AreaAt(AREA_WIN, 30.0, 40.0), &p);
    assert_eq!(s.pointer.lock().unwrap().as_slice(), [(30, 40, 0), (30, 40, 1)], "a press moves then presses the left button");
    assert!(t.pressed);
    t.handle(&Event::MouseMove(vx + 50, vy + 60), &p);
    assert_eq!(s.pointer.lock().unwrap().len(), 2, "while pressed a move is not sent as a hover");
    t.handle(&Event::MouseUp(vx + 31, vy + 41), &p);
    assert_eq!(*s.pointer.lock().unwrap().last().unwrap(), (31, 41, 0));
    assert!(!t.pressed);
    s.pointer.lock().unwrap().clear();
    t.handle(&Event::MouseRight(vx + 7, vy + 8), &p);
    assert_eq!(s.pointer.lock().unwrap().iter().map(|e| e.2).collect::<Vec<_>>(), [0, 4, 0]);
    s.pointer.lock().unwrap().clear();
    t.handle(&Event::Scroll(3), &p);
    assert_eq!(s.pointer.lock().unwrap().iter().map(|e| e.2).collect::<Vec<_>>(), [8, 0, 8, 0, 8, 0], "three notches up");
    s.pointer.lock().unwrap().clear();
    t.handle(&Event::Scroll(-1000), &p);
    assert_eq!(s.pointer.lock().unwrap().len(), 2 * MAX_NOTCHES as usize, "a huge wheel turn is capped");
    assert!(s.pointer.lock().unwrap().iter().all(|e| e.2 == 16 || e.2 == 0));
}

#[test]
fn the_clipboard_is_bridged_only_by_the_two_buttons_or_a_paste_and_as_typed_text_of_at_most_64_kb() {
    let p = Port::new();
    let mut t = running(&p);
    let s = p.rig.screen(0);
    assert!(s.cut.lock().unwrap().is_empty(), "nothing is sent on its own");
    t.handle(&Event::Tab(T_COPY), &p);
    assert_eq!(t.note.as_ref().unwrap(), &("The program has not copied any text yet.".to_string(), false));
    s.updates.lock().unwrap().push_back(ScreenUpdate::Clip("from the program".into()));
    t.handle(&Event::Tick, &p);
    t.handle(&Event::Tab(T_COPY), &p);
    assert_eq!(clipboard::latest().as_deref(), Some("from the program"));
    assert!(t.note.as_ref().unwrap().1);
    clipboard::copy("hello");
    t.handle(&Event::Tab(T_PASTE), &p);
    assert_eq!(s.cut.lock().unwrap().as_slice(), ["hello"]);
    assert_eq!(s.keys.lock().unwrap().iter().filter(|k| k.0).map(|k| k.1).collect::<Vec<_>>(), [ks::CONTROL_L, 'v' as u32], "then Ctrl+V");
    t.handle(&Event::Paste("x".repeat(100_000)), &p);
    assert_eq!(s.cut.lock().unwrap()[1].chars().count(), 64 * 1024);
}

#[test]
fn the_display_is_asked_for_the_area_size_only_after_the_area_has_stopped_changing_and_a_refusal_keeps_the_size() {
    let p = Port::new();
    let mut t = running(&p);
    let s = p.rig.screen(0);
    let big = R::new(0.0, 46.0, 1200.0, 700.0);
    draw(&mut t, &p, 1300, 800, big);
    t.handle(&Event::Tick, &p);
    assert!(s.resizes.lock().unwrap().is_empty(), "the change has only just been seen");
    p.advance(299);
    t.handle(&Event::Tick, &p);
    assert!(s.resizes.lock().unwrap().is_empty());
    p.advance(2);
    t.handle(&Event::Tick, &p);
    assert_eq!(s.resizes.lock().unwrap().as_slice(), [wanted_size(layout(big, 1.0).1)]);
    p.advance(1_000);
    t.handle(&Event::Tick, &p);
    assert_eq!(s.resizes.lock().unwrap().len(), 1, "asked once for one size");
    // a server that cannot resize keeps its size and the tile does not ask again
    *s.resize_ok.lock().unwrap() = false;
    let other = R::new(0.0, 46.0, 900.0, 500.0);
    draw(&mut t, &p, 1000, 600, other);
    t.handle(&Event::Tick, &p);
    p.advance(301);
    t.handle(&Event::Tick, &p);
    assert_eq!(s.resizes.lock().unwrap().len(), 2);
    assert_eq!(t.sent, t.frame_size());
    p.advance(1_000);
    t.handle(&Event::Tick, &p);
    assert_eq!(s.resizes.lock().unwrap().len(), 3, "it asks again for the size it still wants, once per settle");
}

#[test]
fn the_status_strip_samples_the_process_table_once_a_second_with_cpu_and_the_peak() {
    let p = Port::new();
    let mut t = running(&p);
    assert_eq!(p.rig.usage_asked.lock().unwrap().as_slice(), [4242], "the first sample is taken at once");
    assert_eq!((t.usage.procs, t.usage.rss_kb, t.peak_kb), (2, 2048, 2048));
    p.advance(999);
    t.handle(&Event::Tick, &p);
    assert_eq!(p.rig.usage_asked.lock().unwrap().len(), 1, "not before a second");
    *p.rig.usage.lock().unwrap() = Usage { procs: 3, rss_kb: 51_200, ticks: 50 };
    p.advance(1);
    t.handle(&Event::Tick, &p);
    assert_eq!(p.rig.usage_asked.lock().unwrap().len(), 2);
    assert!((t.cpu - 50.0).abs() < 0.01, "50 ticks in one second is half a core: {}", t.cpu);
    *p.rig.usage.lock().unwrap() = Usage { procs: 1, rss_kb: 1024, ticks: 60 };
    p.advance(1_000);
    t.handle(&Event::Tick, &p);
    assert_eq!((t.usage.rss_kb, t.peak_kb), (1024, 51_200), "the peak stays");
    let text = t.strip_text();
    assert!(text.contains("CPU 10%") && text.contains("memory 1 MB (peak 50 MB)") && text.contains("1 process(es)"), "{text}");
}

// ---- stopping, several windows

#[test]
fn stop_closes_the_screen_and_the_window_and_goes_back_to_the_list_and_closing_the_tile_stops_every_window() {
    let p = Port::new();
    let mut t = running(&p);
    t.handle(&Event::Tab(T_STOP), &p);
    assert!(*p.rig.screen(0).closed.lock().unwrap() && *p.rig.win(0).stopped.lock().unwrap());
    assert_eq!((t.phase.clone(), t.windows(), t.frame_size()), (Phase::Pick, 0, (0, 0)));
    let mut u = running(&p);
    u.handle(&Event::Tab(T_NEW), &p);
    assert_eq!(u.windows(), 1);
    u.handle(&Event::Tab(T_START), &p);
    finish_start(&mut u, &p);
    assert_eq!(u.windows(), 2);
    u.stop_all();
    for i in [1, 2] {
        assert!(*p.rig.win(i).stopped.lock().unwrap() && *p.rig.screen(i).closed.lock().unwrap(), "window {i}");
    }
    assert_eq!(u.windows(), 0);
}

#[test]
fn several_windows_run_side_by_side_each_with_its_own_slot_and_the_keys_go_to_the_one_on_screen() {
    let p = Port::new();
    let mut t = running(&p);
    assert_eq!((t.windows(), t.slot()), (1, 0));
    t.handle(&Event::Tab(T_NEW), &p);
    assert_eq!((t.windows(), t.parked_names(), t.phase.clone()), (1, vec!["XTerm".to_string()], Phase::Pick));
    t.handle(&Event::Key(Key::Down), &p);
    t.handle(&Event::Key(Key::Enter), &p);
    finish_start(&mut t, &p);
    assert_eq!((t.windows(), t.slot()), (2, 1), "the lowest free slot");
    assert_eq!(p.rig.made.lock().unwrap()[1].slot, 1);
    let g = draw(&mut t, &p, 1000, 700, AREA);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_SWITCH)), "a tab for the other window");
    t.handle(&Event::Key(Key::Char('q')), &p);
    assert_eq!(p.rig.screen(1).keys.lock().unwrap().len(), 2);
    assert!(p.rig.screen(0).keys.lock().unwrap().is_empty());
    t.handle(&Event::Tab(T_SWITCH), &p);
    assert_eq!((t.slot(), t.parked_names(), t.program_name.as_str()), (0, vec!["Mousepad".to_string()], "XTerm"));
    t.handle(&Event::Key(Key::Char('z')), &p);
    assert_eq!(p.rig.screen(0).keys.lock().unwrap().len(), 2);
    assert_eq!(p.rig.screen(1).keys.lock().unwrap().len(), 2, "unchanged");
}

#[test]
fn no_more_windows_than_slots_and_a_window_that_ended_in_front_hands_over_to_another() {
    let p = Port::new();
    let mut t = running(&p);
    for _ in 1..lw::MAX_SLOTS {
        t.handle(&Event::Tab(T_NEW), &p);
        t.handle(&Event::Tab(T_START), &p);
        finish_start(&mut t, &p);
    }
    assert_eq!(t.windows(), lw::MAX_SLOTS as usize);
    t.handle(&Event::Tab(T_NEW), &p);
    assert!(t.note.as_ref().unwrap().0.contains("At most 4 windows") && t.running(), "the new window is refused and the running one stays");
    // the program in front ends: the tile says so and shows another one
    *p.rig.win(lw::MAX_SLOTS as usize - 1).alive.lock().unwrap() = false;
    t.handle(&Event::Tick, &p);
    assert!(t.running());
    assert_eq!(t.windows(), lw::MAX_SLOTS as usize - 1);
    assert!(t.note.as_ref().unwrap().0.contains("ended"), "{:?}", t.note);
    // a window out of sight that ends is dropped from the tabs
    let parked_before = t.parked_names().len();
    *p.rig.win(0).alive.lock().unwrap() = false;
    t.handle(&Event::Tick, &p);
    assert_eq!(t.parked_names().len(), parked_before - 1);
    assert!(t.note.as_ref().unwrap().0.starts_with("The window "));
}

#[test]
fn the_only_window_ending_puts_the_reason_on_the_tile_and_a_lost_connection_does_the_same() {
    let p = Port::new();
    let mut t = running(&p);
    *p.rig.win(0).alive.lock().unwrap() = false;
    t.handle(&Event::Tick, &p);
    assert_eq!(t.phase, Phase::Ended("the program ended".into()));
    assert!(*p.rig.win(0).stopped.lock().unwrap(), "what is left of it is stopped");
    let mut u = running(&p);
    *p.rig.screen(1).ended.lock().unwrap() = Some("the connection to the display server was lost".into());
    u.handle(&Event::Tick, &p);
    assert_eq!(u.phase, Phase::Ended("the connection to the display server was lost".into()));
}

#[test]
fn the_panel_and_the_running_view_draw_in_every_phase_at_the_usual_sizes_without_panicking() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    for (w, h) in [(1280u32, 720u32), (800, 600), (320, 240), (1920, 1080), (40, 30)] {
        let area = R::new(0.0, 0.0, w as f32, h as f32);
        for phase in [Phase::NotInstalled, Phase::NoAccess("no".into()), Phase::Pick, Phase::Starting, Phase::Ended("x".into())] {
            t.phase = phase;
            t.note = Some(("a note".into(), true));
            draw(&mut t, &p, w, h, area);
        }
    }
    let mut r = running(&p);
    r.handle(&Event::Tab(T_NEW), &p);
    r.handle(&Event::Tab(T_START), &p);
    finish_start(&mut r, &p);
    for (w, h) in [(1280u32, 720u32), (800, 600), (320, 240), (60, 40)] {
        draw(&mut r, &p, w, h, R::new(-20.0, 0.0, w as f32, h as f32));
    }
    assert!(r.hint().contains("New window") && t.hint().contains("Enter start"));
    assert_eq!((r.title(), r.accent()), ("LINUX WINDOW", ACCENT));
}

#[test]
fn an_attached_screen_is_the_running_window_for_a_test() {
    let p = Port::new();
    let mut t = LinuxWinTile::new(&p);
    let s = Arc::new(ScreenState::default());
    t.attach_for_test(Box::new(FakeScreen(s.clone())));
    assert!(t.running());
    t.handle(&Event::Key(Key::Esc), &p);
    assert_eq!(s.keys.lock().unwrap().len(), 2, "Esc goes to the program while one runs");
}
