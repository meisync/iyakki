//! A deterministic monkey: seeded pseudo-random mouse, wheel, keyboard and resize events thrown at the launcher, every app, the
//! panes and the settings pages while the screen is drawn between them. A panic anywhere (the shell catches an app's panic and
//! shows "stopped because of an internal error", so the panic hook is what sees it) fails the test with the app, the size, the
//! seed and the index of the event, so it can be replayed:
//!
//!   IYAKKI_MONKEY_APP=ide IYAKKI_MONKEY_SEED=12345 cargo test -p iyakki-shell --lib monkey_replay -- --ignored --nocapture
//!
//! The default run is short. `IYAKKI_MONKEY_ITERS=N` multiplies the work (use with `--ignored monkey_long`).

use crate::shell::tests::{login, shell_with_users};
use crate::shell::Shell;
use crate::gfx::Gfx;
use crate::ui::{Event, Key, Nav, R};
use iyakki_sec::policy::App as PApp;
use std::cell::RefCell;

/// The sizes the real devices and the odd ones have.
pub const SIZES: [(u32, u32); 10] = [(2560, 1600), (1366, 768), (800, 480), (360, 740), (3840, 2160), (200, 150), (1920, 1080), (1280, 720), (2880, 1800), (1, 1)];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1).max(1) as u64) as i32
    }
}

thread_local! {
    /// Settings only: the page (index in its list) to open before the random events start.
    static SETTINGS_TAB: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// Settings only: keys typed right after the page is opened (for example `C` opens the Clusters page).
    static SETTINGS_KEYS: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
    /// The Store only: open on the Setup tab (tiles and popup) with made-up facts, instead of on Installed.
    static STORE_SETUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// While set, panics on this thread are recorded here instead of printed.
    static CAUGHT: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

fn install_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mine = CAUGHT.with(|c| {
                if let Some(v) = c.borrow_mut().as_mut() {
                    let bt = std::backtrace::Backtrace::force_capture().to_string();
                    let frames = iyakki_core_frames(&bt);
                    v.push(format!("{info}\n    {}", frames.join("\n    ")));
                    true
                } else {
                    false
                }
            });
            if !mine {
                prev(info);
            }
        }));
    });
}

/// The frames of our own crates, first ten.
fn iyakki_core_frames(bt: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = bt.lines().peekable();
    while let Some(l) = it.next() {
        let t = l.trim();
        if t.contains("iyakki_shell") && !t.contains("monkey_tests") {
            let at = it.peek().map(|n| n.trim()).filter(|n| n.starts_with("at ")).unwrap_or("");
            out.push(format!("{t} {at}"));
            if out.len() >= 10 {
                break;
            }
        }
    }
    out
}

fn pick_key(r: &mut Rng) -> Event {
    // no path or shell characters: a typed command cannot do anything outside the test's own folders
    const CHARS: &str = "abcdefghijklmnopqrstuvwxyz ABCXYZ0123456789?!,:";
    let cs: Vec<char> = CHARS.chars().collect();
    match r.below(14) {
        0..=3 => Event::Key(Key::Char(cs[r.below(cs.len() as u64) as usize])),
        4 => Event::Key(Key::Ctrl((b'a' + r.below(26) as u8) as char)),
        5 => Event::Key(Key::Ctrl((b'A' + r.below(26) as u8) as char)),
        6 => Event::Key([Key::Enter, Key::Backspace, Key::Delete, Key::Tab, Key::BackTab, Key::Esc][r.below(6) as usize].clone()),
        7 => Event::Key([Key::Up, Key::Down, Key::Left, Key::Right, Key::Home, Key::End, Key::PageUp, Key::PageDown][r.below(8) as usize].clone()),
        8 => Event::Key(Key::F(1 + r.below(12) as u8)),
        9 => {
            let nav = [Nav::Left, Nav::Right, Nav::Up, Nav::Down, Nav::Home, Nav::End, Nav::PageUp, Nav::PageDown][r.below(8) as usize];
            Event::Key(Key::Nav { nav, shift: r.below(2) == 0, ctrl: r.below(2) == 0 })
        }
        10 => Event::Paste(["hello", "line one\nline two", "é世界", "", "x".repeat(300).as_str()][r.below(5) as usize].to_string()),
        11 => {
            use crate::panes::{LayoutKey, Side};
            let side = [Side::Left, Side::Right, Side::Up, Side::Down][r.below(4) as usize];
            Event::Layout(match r.below(6) {
                0 => LayoutKey::Apply(1 + r.below(9) as u8),
                1 => LayoutKey::Snap(side),
                2 => LayoutKey::Nudge(side),
                3 => LayoutKey::Cycle { back: r.below(2) == 0 },
                4 => LayoutKey::NextDisplay,
                _ => LayoutKey::Picker,
            })
        }
        _ => Event::Tick,
    }
}

/// One replayable run: `events` random events against `app` (or the launcher) at `size`, with a resize now and then.
/// Returns the first panic with where it happened.
pub fn run_case(sh: &mut Shell, app: Option<PApp>, size: (u32, u32), seed: u64, events: usize, render_every: usize) -> Result<(), String> {
    install_hook();
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut size = size;
    let mut g = Gfx::new(size.0, size.1);
    let name = app.map_or("launcher".to_string(), |a| a.id().to_string());
    match app {
        Some(PApp::Ide) => open_ide_with_workspace(sh),
        Some(PApp::Store) if STORE_SETUP.with(|c| c.get()) => open_store_setup(sh),
        Some(a) => sh.open_for_test(a),
        None => sh.launcher_for_test(),
    }
    if let (Some(PApp::Settings), Some(t)) = (app, SETTINGS_TAB.with(|c| c.get())) {
        sh.handle(&Event::Tab(t));
        for c in SETTINGS_KEYS.with(|k| k.get()).chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
    }
    CAUGHT.with(|c| *c.borrow_mut() = Some(Vec::new()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut log: Vec<String> = Vec::new();
        let mut render = |sh: &mut Shell, g: &mut Gfx, idx: usize, log: &mut Vec<String>| -> Option<String> {
            let t0 = std::time::Instant::now();
            sh.render(g);
            if t0.elapsed().as_millis() > 400 && std::env::var("IYAKKI_MONKEY_VERBOSE").is_ok() {
                eprintln!("slow render at {}x{} event#{idx}: {:?}", g.width(), g.height(), t0.elapsed());
            }
            let got = CAUGHT.with(|c| c.borrow().as_ref().map_or(0, |v| v.len()));
            if got > 0 {
                let msg = CAUGHT.with(|c| c.borrow().as_ref().map(|v| v.join("\n")).unwrap_or_default());
                return Some(format!("PANIC app={name} size={}x{} seed={seed} event#{idx} (last events: {})\n{msg}", g.width(), g.height(), log.iter().rev().take(6).rev().cloned().collect::<Vec<_>>().join(" | ")));
            }
            None
        };
        if let Some(e) = render(sh, &mut g, 0, &mut log) {
            return Err(e);
        }
        let mut i = 0;
        let mut next_render = 0;
        while i < events {
            let (w, h) = size;
            let hits: Vec<R> = sh.hits().iter().map(|x| x.rect).collect();
            // where to point: a control (most of the time) or anywhere, edges included
            let mut point = |r: &mut Rng| -> (i32, i32) {
                if !hits.is_empty() && r.below(10) < 7 {
                    let b = hits[r.below(hits.len() as u64) as usize];
                    (b.x as i32 + r.range(0, b.w.max(1.0) as i32), b.y as i32 + r.range(0, b.h.max(1.0) as i32))
                } else {
                    (r.range(-4, w as i32 + 4), r.range(-4, h as i32 + 4))
                }
            };
            let mut batch: Vec<Event> = Vec::new();
            match r.below(20) {
                0..=3 => {
                    let p = point(&mut r);
                    batch.push(Event::MouseMove(p.0, p.1));
                }
                4..=6 => {
                    let p = point(&mut r);
                    batch.extend([Event::MouseMove(p.0, p.1), Event::MouseDown(p.0, p.1), Event::MouseUp(p.0, p.1)]);
                }
                7..=9 => {
                    // a drag: down, many moves, up (the selection drag of the editor, a divider, a scrollbar)
                    let p = point(&mut r);
                    batch.extend([Event::MouseMove(p.0, p.1), Event::MouseDown(p.0, p.1)]);
                    let mut q = p;
                    for _ in 0..r.range(3, 14) {
                        q = (q.0 + r.range(-90, 90), q.1 + r.range(-60, 60));
                        batch.push(Event::MouseMove(q.0, q.1));
                    }
                    if r.below(3) == 0 {
                        let e = point(&mut r);
                        batch.push(Event::MouseMove(e.0, e.1));
                    }
                    batch.push(Event::MouseUp(q.0, q.1));
                }
                10 => {
                    let p = point(&mut r);
                    batch.extend([Event::MouseMove(p.0, p.1), Event::MouseRight(p.0, p.1)]);
                }
                11..=12 => {
                    let p = point(&mut r);
                    batch.push(Event::MouseMove(p.0, p.1));
                    // wheel notches and the 1-pixel steps of a trackpad
                    let n = if r.below(2) == 0 { r.range(-5, 5) } else { r.range(-1, 1) };
                    batch.extend(std::iter::repeat(Event::Scroll(n)).take(r.range(1, 6) as usize));
                }
                13 => {
                    // resize between odd sizes (also in the middle of a drag: down first, then the new size)
                    let p = point(&mut r);
                    if r.below(2) == 0 {
                        batch.extend([Event::MouseMove(p.0, p.1), Event::MouseDown(p.0, p.1), Event::MouseMove(p.0 + 20, p.1 + 10)]);
                    }
                    // the default run leaves out the two sizes that take seconds to draw in a debug build
                    let pool: Vec<(u32, u32)> = SIZES.iter().copied().filter(|z| multiplier() > 1 || z.0 as usize * z.1 as usize <= 2_100_000).collect();
                    let n = pool[r.below(pool.len() as u64) as usize];
                    let n = if r.below(4) == 0 { (r.range(120, 1400) as u32, r.range(90, 900) as u32) } else { n };
                    size = n;
                    sh.resize_for_test(n.0, n.1);
                    g = Gfx::new(n.0, n.1);
                    batch.push(Event::MouseMove(p.0.min(n.0 as i32), p.1.min(n.1 as i32)));
                    batch.push(Event::MouseUp(p.0.min(n.0 as i32), p.1.min(n.1 as i32)));
                }
                _ => batch.push(pick_key(&mut r)),
            }
            for ev in batch {
                log.push(format!("{ev:?}"));
                let t0 = std::time::Instant::now();
                sh.handle(&ev);
                if t0.elapsed().as_millis() > 150 && std::env::var("IYAKKI_MONKEY_VERBOSE").is_ok() {
                    eprintln!("slow event {ev:?}: {:?}", t0.elapsed());
                }
                i += 1;
                let got = CAUGHT.with(|c| c.borrow().as_ref().map_or(0, |v| v.len()));
                if got > 0 {
                    break;
                }
            }
            if i >= next_render || CAUGHT.with(|c| c.borrow().as_ref().is_some_and(|v| !v.is_empty())) {
                next_render = i + render_every.max(1);
                if let Some(e) = render(sh, &mut g, i, &mut log) {
                    return Err(e);
                }
            }
        }
        Ok(())
    }));
    let left = CAUGHT.with(|c| c.borrow_mut().take()).unwrap_or_default();
    match result {
        Ok(Err(e)) => Err(e),
        Ok(Ok(())) if left.is_empty() => Ok(()),
        Ok(Ok(())) => Err(format!("PANIC app={name} size={}x{} seed={seed}\n{}", size.0, size.1, left.join("\n"))),
        Err(_) => Err(format!("PANIC escaped the shell: app={name} size={}x{} seed={seed}\n{}", size.0, size.1, left.join("\n"))),
    }
}

/// The AI IDE with a workspace open (without one it only shows the welcome card): a source file with long lines to select in,
/// a features file for the board, and a chat to type into.
pub(crate) fn open_ide_with_workspace(sh: &mut Shell) {
    use crate::apps::App;
    let Some(cx) = sh.cx_for_test() else { return };
    let dir = std::env::temp_dir().join(format!("iyakki-monkey-ws-{}", std::process::id()));
    let _ = std::fs::create_dir_all(dir.join("src"));
    let long = "let value = compute(alpha, beta, gamma) + other_function(delta, epsilon) * 12345.6789; // a long line to select in\n".repeat(60);
    let _ = std::fs::write(dir.join("src/main.rs"), format!("fn main() {{\n{long}}}\n"));
    let _ = std::fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n".repeat(200));
    let _ = std::fs::write(dir.join("FEATURES.md"), "| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | Alpha | P1 | done | P0 | 100% | `t::a` |  |\n| F-A-02 | Beta | P2 | planned | P1 | 0% | - |  |\n");
    let mut ide = crate::apps::ide::IdeApp::new(&cx);
    if ide.add_and_open("monkey", &dir, &cx).is_ok() {
        // open the file in the editor and give the chat something to show
        for k in [Key::Ctrl('p')] {
            ide.handle(&Event::Key(k), &cx);
        }
        ide.handle(&Event::Key(Key::Esc), &cx);
    }
    sh.show_app_for_test(Box::new(ide), PApp::Ide);
}

/// The Store on its Setup tab with a made-up machine (a MacBook with a GPU and its camera) and a mix of states: installed, running,
/// stopped, an update, a failed install, one that is installing, found by hand. The jobs run a stand-in program, never the real installer.
pub(crate) fn open_store_setup(sh: &mut Shell) {
    use crate::apps::store::setup::Machine;
    use iyakki_agent::quickinstall::{Facts, Item, Latest, Presence};
    let Some(cx) = sh.cx_for_test() else { return };
    let mut store = crate::apps::store::StoreApp::new(&cx);
    let mut f = Facts::default();
    for it in Item::ALL {
        let p = match it {
            Item::K3s => Presence::Installed("k3s v1.31.4+k3s1 \u{b7} running".into()),
            Item::Node => Presence::Installed("version 26.1.0".into()),
            Item::Python => Presence::Elsewhere("/usr/bin/python3".into()),
            Item::Lemonade => Presence::Installed("answering on 127.0.0.1:13305 (2 model(s))".into()),
            Item::ClaudeCode => Presence::Installed("version 2.1.7".into()),
            Item::DuckDb => Presence::Earlier("cluster down".into()),
            _ => Presence::Missing,
        };
        f.presence.push((it, p));
    }
    store.setup.set_machine(Machine { model: "MacBookPro14,1".into(), gpus: 1, nvidia: true, ram_mb: 16_000, camera: true, ..Default::default() });
    store.setup.set_facts(f);
    store.setup.latest = vec![(Item::ClaudeCode, Latest::Newer("v2.2.0".into()))];
    store.setup.failed = vec![Item::Registry];
    store.setup.launch = Some(iyakki_agent::quickinstall::Launch { exe: "/usr/bin/true".into(), data: std::env::temp_dir().join(format!("iyakki-monkey-qi-{}", std::process::id())) });
    crate::apps::App::handle(&mut store, &Event::Tab(4), &cx);
    sh.show_app_for_test(Box::new(store), PApp::Store);
}

fn multiplier() -> usize {
    std::env::var("IYAKKI_MONKEY_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1)
}

fn signed(tag: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    sh
}

/// Events per size: the big screens cost a lot to draw, so they get fewer.
fn budget(size: (u32, u32), mult: usize) -> (usize, usize) {
    let px = size.0 as usize * size.1 as usize;
    let (events, every) = if px > 6_000_000 { (6, 6) } else if px > 3_000_000 { (10, 10) } else if px > 1_000_000 { (30, 6) } else { (60, 5) };
    (events * mult, every)
}

/// `all_sizes`: every size for each app (the AI IDE, the launcher and the long run); otherwise three sizes per app, rotating, so
/// the default run stays short while the whole set is still covered across the apps. The default run also stops after
/// `DEFAULT_SECONDS` (sizes go in rounds across the apps, so a stop leaves every app covered at the first sizes).
fn run_apps(tag: &str, apps: &[Option<PApp>], all_sizes: bool) {
    const DEFAULT_SECONDS: u64 = 8;
    let mut sh = signed(tag);
    let mult = multiplier();
    let started = std::time::Instant::now();
    let mut fails = Vec::new();
    let mut ran = 0;
    // the small and medium sizes first, the heavy ones last
    let mut order: Vec<usize> = (0..SIZES.len()).collect();
    order.sort_by_key(|i| SIZES[*i].0 as usize * SIZES[*i].1 as usize);
    for si in order {
        let size = SIZES[si];
        for (ai, app) in apps.iter().enumerate() {
            if !all_sizes && mult == 1 && !(0..3).any(|j| (ai * 3 + j) % SIZES.len() == si) {
                continue;
            }
            if mult == 1 && started.elapsed().as_secs() >= DEFAULT_SECONDS {
                continue;
            }
            let (events, every) = budget(size, mult);
            for k in 0..mult {
                let seed = 1000 * (ai as u64 + 1) + 10 * si as u64 + k as u64 + 1;
                ran += 1;
                let t0 = std::time::Instant::now();
                let r = run_case(&mut sh, *app, size, seed, events, every);
                if std::env::var("IYAKKI_MONKEY_VERBOSE").is_ok() {
                    eprintln!("monkey {tag}: {app:?} {}x{} seed {seed}: {:?}", size.0, size.1, t0.elapsed());
                }
                if let Err(e) = r {
                    fails.push(e);
                }
            }
        }
    }
    eprintln!("monkey {tag}: {ran} cases in {:?}", started.elapsed());
    assert!(fails.is_empty(), "{} monkey failures:\n{}", fails.len(), fails.join("\n---\n"));
}

#[test]
fn monkey_ai_ide() {
    run_apps("monkey-ide", &[Some(PApp::Ide)], true);
}

#[test]
fn monkey_launcher_and_panes() {
    run_apps("monkey-launcher", &[None, Some(PApp::Settings)], true);
}

/// Every page of Settings (by its place in the list) gets its own run at three sizes, so no page is only reached by luck.
#[test]
fn monkey_settings_every_page() {
    let mut sh = signed("monkey-settings-pages");
    let mut fails = Vec::new();
    for tab in 0..31usize {
        SETTINGS_TAB.with(|c| c.set(Some(tab)));
        for (si, size) in [(360u32, 740u32), (800, 480), (1280, 720)].into_iter().enumerate() {
            let seed = 7000 + 10 * tab as u64 + si as u64;
            if let Err(e) = run_case(&mut sh, Some(PApp::Settings), size, seed, 15, 5) {
                fails.push(format!("page #{tab}: {e}"));
            }
        }
    }
    SETTINGS_TAB.with(|c| c.set(None));
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}

/// A laptop battery on screen (the chip, its popup on F4, the plug-in card and the countdown card) while random events fly, in
/// the launcher and in Settings (whose Battery page is then in the list), at every size.
#[test]
fn monkey_battery_laptop() {
    let d = std::env::temp_dir().join(format!("iyakki-monkey-batt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (k, v) in [("type", "Battery"), ("status", "Discharging"), ("capacity", "4"), ("energy_now", "2000000"), ("energy_full", "50000000"), ("power_now", "9000000")] {
        let p = d.join("class/power_supply/BAT0").join(k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }
    let mut sh = signed("monkey-battery");
    sh.battery_set_env_for_test(&d);
    let mut fails = Vec::new();
    for (si, size) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2880, 1800)].into_iter().enumerate() {
        for (ai, app) in [None, Some(PApp::Settings)].into_iter().enumerate() {
            let seed = 9100 + 10 * si as u64 + ai as u64;
            if let Err(e) = run_case(&mut sh, app, size, seed, 40, 5) {
                fails.push(e);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}

/// The screen saver and the black screen while random events fly and a fake clock jumps about: it never panics, any input that comes
/// while the screen is dark or showing the saver wakes it (and only that: the screen is awake after every event), and whenever the
/// screen is awake the backlight is exactly what it was.
#[test]
fn monkey_screen_saver_and_off() {
    use crate::saver::Stage;
    use iyakki_agent::screensaver::{First, SaverConfig, Style};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    for (si, size) in [(360u32, 740u32), (800, 480), (1280, 720), (200, 150), (1, 1), (1920, 1080)].into_iter().enumerate() {
        let mut sh = signed("monkey-saver");
        let d = std::env::temp_dir().join(format!("iyakki-monkey-saver-{}-{si}", std::process::id()));
        let bl = d.join("class/backlight/intel_backlight");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&bl).unwrap();
        std::fs::write(bl.join("brightness"), "612").unwrap();
        std::fs::write(bl.join("max_brightness"), "1000").unwrap();
        let level = || std::fs::read_to_string(bl.join("brightness")).unwrap().trim().parse::<u32>().unwrap();
        sh.saver_panel_for_test(&d, &d.join("restore"));
        let clock = Arc::new(AtomicU64::new(1000));
        sh.saver_clock_for_test(clock.clone());
        let first = [First::Saver, First::Off, First::Saver][si % 3];
        sh.saver_config_for_test(SaverConfig { first, after_min: 1, then_off_min: 1, style: Style::ALL[si % 4], dim_pct: 25, ..SaverConfig::default() });
        let mut r = Rng(0xC0FFEE + si as u64);
        let mut g = Gfx::new(size.0, size.1);
        let mut woke = 0;
        for i in 0..600 {
            let ev = match r.below(10) {
                0..=4 => {
                    clock.fetch_add(r.below(40_000) + 1, Ordering::SeqCst);
                    Event::Tick
                }
                5 => Event::MouseMove(r.range(-5, size.0 as i32 + 5), r.range(-5, size.1 as i32 + 5)),
                6 => Event::MouseDown(r.range(0, size.0 as i32), r.range(0, size.1 as i32)),
                7 => Event::MouseUp(r.range(0, size.0 as i32), r.range(0, size.1 as i32)),
                8 => Event::Scroll(r.range(-3, 3)),
                _ => pick_key(&mut r),
            };
            let covered = sh.screen_stage().covers();
            sh.handle(&ev);
            if !matches!(ev, Event::Tick) {
                // input never leaves the screen dark or showing the saver
                assert_eq!(sh.screen_stage().covers(), false, "event #{i} {ev:?} at {size:?}: input must wake");
                woke += covered as u32;
            }
            if sh.take_stale() || r.below(4) == 0 {
                sh.render(&mut g);
            }
            for _ in sh.take_blank_requests() {}
            if sh.screen_stage() == Stage::Awake {
                assert_eq!(level(), 612, "event #{i}: awake means the brightness is back");
            } else if sh.screen_stage() == Stage::Off {
                assert_eq!(level(), 0);
            }
        }
        assert!(woke > 0, "the run really went through the saver ({size:?})");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The Clusters page (opened with `C` on the old Dev cluster page; every page gets the key, only that one reacts) at five sizes.
#[test]
fn monkey_clusters_page() {
    let mut sh = signed("monkey-clusters-page");
    let mut fails = Vec::new();
    SETTINGS_KEYS.with(|k| k.set("C"));
    for tab in 0..31usize {
        for (si, size) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)].into_iter().enumerate() {
            SETTINGS_TAB.with(|c| c.set(Some(tab)));
            let seed = 9000 + 10 * tab as u64 + si as u64;
            if let Err(e) = run_case(&mut sh, Some(PApp::Settings), size, seed, 20, 4) {
                fails.push(format!("page #{tab}: {e}"));
            }
        }
    }
    SETTINGS_KEYS.with(|k| k.set(""));
    SETTINGS_TAB.with(|c| c.set(None));
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}

/// Developer mode -> Connect over USB-C (opened with `u` on the Developer page; every page gets the key, only that one reacts) at three sizes,
/// with random keys, clicks and resizes while its worker thread runs the checks against the real (empty, unprivileged) machine.
#[test]
fn monkey_developer_usb_page() {
    let mut sh = signed("monkey-developer-usb-page");
    let mut fails = Vec::new();
    SETTINGS_KEYS.with(|k| k.set("u"));
    for tab in 0..31usize {
        for (si, size) in [(360u32, 740u32), (800, 480), (2560, 1600)].into_iter().enumerate() {
            SETTINGS_TAB.with(|c| c.set(Some(tab)));
            let seed = 11000 + 10 * tab as u64 + si as u64;
            if let Err(e) = run_case(&mut sh, Some(PApp::Settings), size, seed, 12, 3) {
                fails.push(format!("page #{tab}: {e}"));
            }
        }
    }
    SETTINGS_KEYS.with(|k| k.set(""));
    SETTINGS_TAB.with(|c| c.set(None));
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}

#[test]
fn monkey_disks() {
    // the tile gets the made-up machine in tests: the keys m, u, e and the partition manager's buttons act on fakes only
    run_apps("monkey-disks", &[Some(PApp::Disks)], true);
}

#[test]
fn monkey_chat_tales_voice_edit() {
    run_apps("monkey-chat", &[Some(PApp::Chat), Some(PApp::Tales), Some(PApp::Voice), Some(PApp::Edit), Some(PApp::Rag), Some(PApp::Mcp)], false);
}

#[test]
fn monkey_data_dash_flows_store() {
    run_apps("monkey-data", &[Some(PApp::Data), Some(PApp::Dash), Some(PApp::Flows), Some(PApp::Store), Some(PApp::K8s), Some(PApp::Browser)], false);
}

/// The cluster switchers (the K8S tile's header, the AI IDE's Kubernetes panel, the Store -> Setup popups) with several clusters in the
/// registry, at every size, with random clicks, keys and resizes; GPU badges (fresh and last known) come from the fake cache.
#[test]
fn monkey_cluster_switchers() {
    let mut sh = signed("monkey-cluster-switchers");
    let svc = sh.services_for_demo().clone();
    let (dir, user) = (svc.dir().to_path_buf(), "root".to_string());
    let mut reg = iyakki_agent::clusters::Registry::load(&dir, &user);
    reg.add_remote("monkey-gpu-box", iyakki_agent::clusters::Kind::Remote, "/fake/gpu.yaml", "", "").unwrap();
    reg.add_remote("monkey-prod-eu", iyakki_agent::clusters::Kind::Tunnel, "/fake/prod.yaml", "", "").unwrap();
    reg.save(&dir, &user).unwrap();
    let cache = iyakki_agent::aiprofile_remote::clusters_cache();
    let json = r#"{"items":[{"metadata":{"name":"g1","labels":{"nvidia.com/gpu.product":"NVIDIA-GeForce-RTX-4090","nvidia.com/gpu.memory":"24576"}},"status":{"allocatable":{"nvidia.com/gpu":"2"}}}]}"#;
    iyakki_agent::aiprofile_remote::note_nodes(&cache, "monkey-gpu-box", "monkey-gpu-box", json, 100);
    iyakki_agent::aiprofile_remote::note_failure(&cache, "monkey-gpu-box", "monkey-gpu-box", "down", 200);
    let mut fails = Vec::new();
    STORE_SETUP.with(|c| c.set(true));
    for (i, (app, size)) in [(PApp::K8s, (360u32, 740u32)), (PApp::K8s, (800, 480)), (PApp::K8s, (1280, 720)), (PApp::Ide, (800, 480)), (PApp::Ide, (1280, 720)), (PApp::Store, (360, 740)), (PApp::Store, (1280, 720)), (PApp::K8s, (1920, 1080)), (PApp::Store, (1920, 1080))].into_iter().enumerate() {
        let (events, every) = budget(size, 1);
        if let Err(e) = run_case(&mut sh, Some(app), size, 7000 + i as u64, events, every) {
            fails.push(e);
        }
    }
    STORE_SETUP.with(|c| c.set(false));
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n---\n"));
}

/// The Store's Setup tab (tiles, category bar, filters, the popup, the review) at every size, with the keys, the wheel, drags and resizes.
#[test]
fn monkey_store_setup_tab() {
    STORE_SETUP.with(|c| c.set(true));
    run_apps("monkey-store-setup", &[Some(PApp::Store)], true);
    STORE_SETUP.with(|c| c.set(false));
}

#[test]
fn monkey_the_rest() {
    run_apps(
        "monkey-rest",
        &[Some(PApp::Cli), Some(PApp::Users), Some(PApp::Runs), Some(PApp::Mesh), Some(PApp::Radio), Some(PApp::Scan), Some(PApp::Skills), Some(PApp::Usage), Some(PApp::Calendar), Some(PApp::System), Some(PApp::Media), Some(PApp::SrePilot), Some(PApp::Inbox), Some(PApp::Camera)],
        false,
    );
}

/// The long run: `IYAKKI_MONKEY_ITERS=20 cargo test -p iyakki-shell --lib monkey_long -- --ignored --nocapture`.
#[test]
#[ignore]
fn monkey_long() {
    let all: Vec<Option<PApp>> = std::iter::once(None).chain(PApp::ALL.iter().map(|a| Some(*a))).collect();
    run_apps("monkey-long", &all, true);
}

/// Replays one case: `IYAKKI_MONKEY_APP=ide IYAKKI_MONKEY_SEED=1011 IYAKKI_MONKEY_SIZE=2560x1600 IYAKKI_MONKEY_EVENTS=60`.
#[test]
#[ignore]
fn monkey_replay() {
    let app = std::env::var("IYAKKI_MONKEY_APP").unwrap_or_else(|_| "ide".into());
    let app = PApp::ALL.iter().copied().find(|a| a.id() == app);
    let seed: u64 = std::env::var("IYAKKI_MONKEY_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let size = std::env::var("IYAKKI_MONKEY_SIZE").ok().and_then(|v| v.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280), b.parse().unwrap_or(720)))).unwrap_or((1280, 720));
    let events: usize = std::env::var("IYAKKI_MONKEY_EVENTS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let mut sh = signed("monkey-replay");
    if let Err(e) = run_case(&mut sh, app, size, seed, events, 5) {
        panic!("{e}");
    }
}
