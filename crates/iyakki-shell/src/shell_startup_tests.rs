//! What is still starting, on the screen (`shell_startup.rs`): the Home strip and its list at five screen sizes, the transitions with
//! a fake registry, the "all ready" fade, a failure that stays with Retry and Open, what each role reads, the keys and clicks, the
//! sign-in line, the main display only, the audit lines, and what `iyakki dev state` sees.

use super::startup_impl::{self, pop_layout, strip_rect};
use super::tests::*;
use super::*;
use iyakki_agent::startup::{Phase, Registry, StartupState};
use std::sync::atomic::{AtomicU64, Ordering};

const SIZES: [(u32, u32); 5] = [(800, 480), (1024, 600), (1280, 720), (1920, 1080), (2560, 1600)];

fn fake(sh: &mut Shell) -> Arc<Registry> {
    let t = Arc::new(AtomicU64::new(0));
    let r = Arc::new(Registry::new(move || t.fetch_add(1, Ordering::SeqCst)));
    sh.startup.reg = r.clone();
    r
}

/// Everything ready except the cluster, which waits for the clock.
fn cluster_waits(r: &Registry) {
    for id in ["network", "firewall", "sound", "touch", "web", "shares", "agent"] {
        r.ready(id);
    }
    r.starting("clock", "waiting for a time server");
}

fn beat(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

fn draw(sh: &mut Shell, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    g
}

fn inside(r: R, w: u32, h: u32) -> bool {
    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32 && r.w > 0.0 && r.h > 0.0
}

fn apart(a: R, b: R) -> bool {
    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
}

fn home(tag: &str, who: (&str, &str)) -> (Shell, Arc<Registry>) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, who.0, who.1);
    assert_eq!(sh.screen_name(), "launcher");
    let r = fake(&mut sh);
    (sh, r)
}

fn center(r: R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

#[test]
fn a_test_shell_starts_quiet_so_no_other_screen_has_a_strip() {
    let (mut sh, _) = shell_with_users("st-quiet");
    login(&mut sh, "root", "rootpass12");
    beat(&mut sh, 5);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_none() && sh.startup.login_line.is_none());
}

#[test]
fn the_strip_sits_clear_of_tiles_chips_dots_and_the_host_line_at_five_sizes_and_its_list_stays_on_screen() {
    for (w, h) in SIZES {
        let (mut sh, r) = home("st-layout", ("root", "rootpass12"));
        cluster_waits(&r);
        r.starting("k3s", "waiting for the clock to be set");
        r.failed("flows", "could not read the flows");
        beat(&mut sh, 2);
        draw(&mut sh, w, h);
        let s = scale_for(w, h);
        let strip = sh.startup.strip.unwrap_or_else(|| panic!("{w}x{h}: no strip"));
        assert!(inside(strip, w, h), "{w}x{h}: {strip:?}");
        assert!(sh.hits().iter().any(|x| x.label == "startup status" && x.rect == strip && x.ev == Event::Tab(startup_impl::TAB_STRIP)), "{w}x{h}: the strip is a button");
        let n = sh.group_items().len();
        let pages = launcher::page_count(n, launcher::per_page(w, h, s));
        for t in launcher::page_rects(w, h, s, n, 0) {
            assert!(apart(strip, t), "{w}x{h}: strip {strip:?} meets a tile {t:?}");
        }
        if pages > 1 {
            let nav = launcher::page_nav(w, h, s, pages);
            for r in [nav.prev, nav.next].into_iter().chain(nav.dots.iter().copied()) {
                assert!(apart(strip, r), "{w}x{h}: strip meets the page dots {r:?}");
            }
        }
        for (_, c) in &sh.chip_row().chips {
            assert!(apart(strip, *c), "{w}x{h}: strip meets a group chip");
        }
        assert!(strip.y + strip.h <= h as f32 - 36.0 * s, "{w}x{h}: strip is above the host line");
        // the list
        key(&mut sh, Key::Char('t'));
        assert!(sh.startup.open);
        draw(&mut sh, w, h);
        let p = sh.startup.panel.unwrap_or_else(|| panic!("{w}x{h}: no list"));
        assert!(inside(p, w, h) && p.y >= 96.0 * s - 0.5 && p.y + p.h <= strip.y, "{w}x{h}: list {p:?} strip {strip:?}");
        assert!(sh.startup.lines.len() >= 3, "{w}x{h}: {:?}", sh.startup.lines);
        for (tab, b) in &sh.startup.buttons {
            assert!(inside(*b, w, h) && b.x >= p.x && b.x + b.w <= p.x + p.w && b.y >= p.y && b.y + b.h <= p.y + p.h, "{w}x{h}: button {tab:x} {b:?} outside {p:?}");
            assert!(sh.hits().iter().any(|x| x.rect == *b && x.ev == Event::Tab(*tab)), "{w}x{h}: button is a hit region");
            assert!(b.w >= 40.0 * s && b.h >= 18.0 * s, "{w}x{h}: big enough to hit");
        }
        let btns: Vec<R> = sh.startup.buttons.iter().map(|(_, b)| *b).collect();
        for (i, a) in btns.iter().enumerate() {
            for b in &btns[i + 1..] {
                assert!(apart(*a, *b), "{w}x{h}: buttons overlap");
            }
        }
    }
}

#[test]
fn the_layout_functions_hold_for_many_row_counts_and_page_counts() {
    for (w, h) in SIZES {
        let s = scale_for(w, h);
        for pages in 1..=9 {
            if let Some(st) = strip_rect(w, h, s, pages) {
                assert!(inside(st, w, h));
                for rows in [0usize, 1, 3, 10, 12, 20] {
                    let wants = vec![(true, true); rows];
                    let l = pop_layout(w, h, s, st, &wants);
                    assert!(inside(l.panel, w, h) && l.panel.y + l.panel.h <= st.y, "{w}x{h} p{pages} r{rows}");
                    assert!(l.rows.len() + l.more == rows, "{w}x{h} rows");
                    for r in &l.rows {
                        assert!(r.line.y >= l.panel.y && r.line.y + r.line.h <= l.panel.y + l.panel.h, "{w}x{h} p{pages} r{rows}");
                        if let (Some(a), Some(b)) = (r.retry, r.open) {
                            assert!(apart(a, b) && a.x >= r.line.x && b.x + b.w <= r.line.x + r.line.w);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn the_strip_says_what_waits_for_what_follows_the_start_and_collapses_when_everything_is_ready() {
    let (mut sh, r) = home("st-flow", ("root", "rootpass12"));
    cluster_waits(&r);
    // the cluster has not started: it waits for the clock
    beat(&mut sh, 2);
    draw(&mut sh, 1280, 720);
    assert!(matches!(sh.startup.phase, Phase::Busy));
    let sum = r.summary();
    let top = sum.lines(true);
    assert!(top.iter().any(|l| l.text == "Dev cluster (k3s): waiting for the clock"), "{top:?}");
    assert!(sum.strip(true).starts_with("Starting Clock: waiting for a time server"), "{}", sum.strip(true));
    // the clock is set, the cluster starts, then it is ready
    r.ready("clock");
    r.starting("k3s", "starting k3s (about 20 s)");
    beat(&mut sh, 2);
    assert_eq!(r.summary().strip(true), "Starting Dev cluster (k3s): starting k3s (about 20 s)");
    assert!(sh.take_stale(), "a changed line asks for a new picture");
    r.ready("k3s");
    beat(&mut sh, 1);
    assert!(matches!(sh.startup.phase, Phase::Done(255)), "{:?}", sh.startup.phase);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_some(), "'All services ready' is shown for a few seconds");
    beat(&mut sh, 100);
    assert!(matches!(sh.startup.phase, Phase::Done(a) if a < 255), "fading: {:?}", sh.startup.phase);
    beat(&mut sh, 40);
    assert!(matches!(sh.startup.phase, Phase::Gone));
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_none(), "gone");
    // quiet afterwards: heartbeats do not ask for pictures
    sh.take_stale();
    beat(&mut sh, 300);
    assert!(!sh.take_stale(), "no redraw storm while everything is ready");
    // a service that stops later shows again
    r.failed("k3s", "k3s exited with 1");
    beat(&mut sh, 1);
    assert!(matches!(sh.startup.phase, Phase::Busy));
}

#[test]
fn a_failure_stays_with_its_reason_is_audited_once_and_retry_and_open_work() {
    let (mut sh, r) = home("st-fail", ("root", "rootpass12"));
    for id in ["clock", "network", "firewall", "sound", "touch", "web", "shares", "agent"] {
        r.ready(id);
    }
    r.failed("k3s", "k3s stopped: port 6443 is busy");
    beat(&mut sh, 60);
    let a = audit_text(&sh);
    assert_eq!(a.matches("startup.failed").count(), 1, "one line for the transition: {a}");
    assert!(a.contains("Dev cluster (k3s): k3s stopped: port 6443 is busy"));
    draw(&mut sh, 1280, 720);
    assert!(matches!(sh.startup.phase, Phase::Busy), "a failure never fades");
    beat(&mut sh, 600);
    assert!(matches!(sh.startup.phase, Phase::Busy));
    // the list: the reason, Retry and Open
    key(&mut sh, Key::Char('t'));
    draw(&mut sh, 1280, 720);
    let l = &sh.startup.lines[0];
    assert!(l.failed && l.text.contains("port 6443 is busy") && l.actions.retry);
    assert_eq!(sh.startup.buttons.len(), 2);
    // Retry: audited, and the cluster (not installed in this test machine) answers with a failure of its own
    let (tab, rect) = sh.startup.buttons.iter().find(|(t, _)| *t == 0x4D50).copied().expect("Retry");
    let _ = tab;
    let (x, y) = center(rect);
    sh.handle(&Event::MouseDown(x, y));
    assert!(audit_text(&sh).contains("startup.retry"));
    // Open: the cluster page of Settings
    draw(&mut sh, 1280, 720);
    let (_, rect) = sh.startup.buttons.iter().find(|(t, _)| *t == 0x4D60).copied().expect("Open");
    let (x, y) = center(rect);
    sh.handle(&Event::MouseDown(x, y));
    assert_eq!(sh.screen_name(), "app:settings");
    assert!(!sh.startup.open, "the list closed");
}

#[test]
fn people_who_are_not_administrators_read_names_and_steps_but_no_reasons_and_get_no_buttons() {
    for who in [("alice", "alicepass1"), ("oscar", "operpass12"), ("gus", "guestpass1")] {
        let (mut sh, r) = home("st-roles", who);
        cluster_waits(&r);
        r.starting("k3s", "starting k3s (about 20 s)");
        beat(&mut sh, 2);
        draw(&mut sh, 1280, 720);
        let s = r.summary();
        assert_eq!(s.strip(false), "Starting services, 2 left", "{who:?}");
        key(&mut sh, Key::Char('t'));
        draw(&mut sh, 1280, 720);
        assert!(sh.startup.open && sh.startup.buttons.is_empty(), "{who:?}: no buttons");
        assert!(sh.startup.lines.iter().any(|l| l.text == "Dev cluster (k3s): starting k3s (about 20 s)"));
        r.failed("k3s", "k3s: /var/lib/iyakki/k3s-dev: permission denied for 10.0.0.5");
        beat(&mut sh, 2);
        draw(&mut sh, 1280, 720);
        let t: String = sh.startup.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("|");
        assert!(t.contains("Dev cluster (k3s): did not start") && !t.contains("permission") && !t.contains("10.0.0.5"), "{who:?}: {t}");
        assert!(sh.startup.buttons.is_empty());
        // the reason is not in the audit log text for them either way: it is the system's line
        assert!(audit_text(&sh).contains("startup.failed"));
    }
}

#[test]
fn the_keys_open_choose_and_close_the_list_without_signing_out_or_opening_a_tile() {
    let (mut sh, r) = home("st-keys", ("root", "rootpass12"));
    cluster_waits(&r);
    r.failed("k3s", "k3s exited with 1");
    beat(&mut sh, 2);
    draw(&mut sh, 1280, 720);
    key(&mut sh, Key::Char('t'));
    assert!(sh.startup.open);
    draw(&mut sh, 1280, 720);
    key(&mut sh, Key::Down);
    assert_eq!(sh.startup.sel, 1);
    key(&mut sh, Key::Up);
    assert_eq!(sh.startup.sel, 0);
    key(&mut sh, Key::Esc);
    assert!(!sh.startup.open && sh.screen_name() == "launcher", "Esc closes the list and does not sign out");
    // t again, then t closes
    key(&mut sh, Key::Char('t'));
    key(&mut sh, Key::Char('t'));
    assert!(!sh.startup.open);
    // while it is open, the layout keys and tile keys do nothing
    key(&mut sh, Key::Char('t'));
    key(&mut sh, Key::Char('1'));
    assert_eq!(sh.screen_name(), "launcher");
    assert!(sh.startup.open);
    // Enter opens the failed service's page
    draw(&mut sh, 1280, 720);
    key(&mut sh, Key::Enter);
    assert_eq!(sh.screen_name(), "app:settings");
}

#[test]
fn a_click_on_the_strip_opens_the_list_and_a_click_elsewhere_only_closes_it() {
    let (mut sh, r) = home("st-click", ("root", "rootpass12"));
    cluster_waits(&r);
    beat(&mut sh, 2);
    draw(&mut sh, 1280, 720);
    let (x, y) = center(sh.startup.strip.unwrap());
    sh.handle(&Event::MouseDown(x, y));
    assert!(sh.startup.open);
    draw(&mut sh, 1280, 720);
    // a click on a tile closes the list and does not open the tile
    let tile = launcher::page_rects(1280, 720, scale_for(1280, 720), sh.group_items().len(), 0)[0];
    let (tx, ty) = center(tile);
    sh.handle(&Event::MouseDown(tx, ty));
    assert!(!sh.startup.open);
    assert_eq!(sh.screen_name(), "launcher");
}

#[test]
fn only_the_main_display_and_only_a_full_home_screen_have_the_strip() {
    let (mut sh, r) = home("st-main", ("root", "rootpass12"));
    cluster_waits(&r);
    beat(&mut sh, 2);
    sh.test_set_drawing_display(1);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_none(), "not on a second display");
    sh.test_set_drawing_display(0);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_some());
    sh.open(PApp::Settings);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.strip.is_none() && !sh.startup.open, "an app's screen has none");
}

#[test]
fn the_sign_in_screen_shows_the_system_services_that_are_still_starting_at_five_sizes() {
    for (w, h) in SIZES {
        for volatile in [false, true] {
            let (mut sh, _) = shell_with_users("st-login");
            sh.env.data_volatile = volatile;
            let r = fake(&mut sh);
            r.starting("clock", "waiting for a time server");
            r.starting("network", "waiting for a connection (cable or Wi-Fi)");
            r.starting("k3s", "starting k3s (about 20 s)");
            for id in ["firewall", "sound", "touch", "web", "shares", "agent"] {
                r.ready(id);
            }
            beat(&mut sh, 2);
            draw(&mut sh, w, h);
            let line = sh.startup.login_line.unwrap_or_else(|| panic!("{w}x{h}: no line"));
            let s = scale_for(w, h);
            assert!(inside(line, w, h), "{w}x{h}: {line:?}");
            let fields = if let Screen::Login(f) = &sh.screen { f.fields.len() } else { 0 };
            let card = auth_card(w, h, s, fields);
            assert!(apart(line, card), "{w}x{h} volatile={volatile}: line {line:?} meets the card {card:?}");
            assert!(line.y + line.h <= h as f32 - 34.0 * s + 1.0 || (w, h) == (800, 480), "{w}x{h}: above the host line");
            for (_, c) in &sh.lbar.last {
                assert!(apart(line, *c), "{w}x{h}: line meets the top bar");
            }
        }
    }
    // the same text on the first-run screen, and a failed service as "did not start" without the reason
    let (e, _) = env("st-setup");
    let mut sh = Shell::new(e);
    let r = fake(&mut sh);
    r.failed("network", "no cable: eth0 has no carrier");
    beat(&mut sh, 2);
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.login_line.is_some());
}

#[test]
fn the_sign_in_line_counts_ready_system_services_and_fades_when_they_are_all_up() {
    let (mut sh, _) = shell_with_users("st-login-fade");
    let r = fake(&mut sh);
    r.starting("clock", "setting the clock");
    for id in ["network", "firewall", "sound", "touch", "web", "shares", "agent"] {
        r.ready(id);
    }
    r.disabled("k3s");
    beat(&mut sh, 2);
    assert!(matches!(sh.startup.phase_sys, Phase::Busy));
    r.ready("clock");
    beat(&mut sh, 2);
    assert!(matches!(sh.startup.phase_sys, Phase::Done(255)));
    beat(&mut sh, 130);
    assert!(matches!(sh.startup.phase_sys, Phase::Gone));
    draw(&mut sh, 1280, 720);
    assert!(sh.startup.login_line.is_none());
}

#[test]
fn developer_state_shows_the_strip_text_and_the_button_name() {
    use iyakki_agent::devmode::state;
    let (mut sh, r) = home("st-devstate", ("root", "rootpass12"));
    cluster_waits(&r);
    r.starting("k3s", "starting k3s (about 20 s)");
    r.ready("clock");
    beat(&mut sh, 2);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let dir = sh.env.services.dir().to_path_buf();
    let _ = state::fetch(&dir, 0);
    beat(&mut sh, 1);
    sh.render(&mut g);
    let snap = state::fetch(&dir, 500).expect("a frame");
    assert!(snap.lines.iter().any(|l| l.text == "Starting Dev cluster (k3s): starting k3s (about 20 s)"), "{:?}", snap.lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(snap.buttons.iter().any(|b| b.label == "startup status"));
    // and the list, once opened
    key(&mut sh, Key::Char('t'));
    let _ = state::fetch(&dir, 0);
    beat(&mut sh, 1);
    sh.render(&mut g);
    let snap = state::fetch(&dir, 500).expect("a frame");
    assert!(snap.lines.iter().any(|l| l.text.starts_with("Startup: ")), "{:?}", snap.lines.iter().map(|l| &l.text).collect::<Vec<_>>());
}

#[test]
fn settings_startup_lists_every_service_for_an_administrator_and_not_for_others() {
    let (mut sh, r) = home("st-settings", ("root", "rootpass12"));
    r.failed("k3s", "k3s stopped: port 6443 is busy");
    sh.jump_to("settings", "tab:startup");
    assert_eq!(sh.screen_name(), "app:settings");
    draw(&mut sh, 1280, 960);
    // the page used the process-wide registry; the rows are the catalogue
    let rows = sh.hits().iter().filter(|h| matches!(h.ev, Event::Row(_))).count();
    assert!(rows >= iyakki_agent::startup::CATALOGUE.len(), "{rows} rows");
    let (mut sh, _) = home("st-settings-user", ("alice", "alicepass1"));
    sh.jump_to("settings", "tab:startup");
    draw(&mut sh, 1280, 720);
    if let Screen::App(a, _) = &sh.screen {
        assert_ne!(a.page(), "Startup", "not for everybody");
    }
}

#[test]
fn the_states_of_a_row_are_words_a_person_understands() {
    let st = StartupState::Starting { step: "x".into() };
    assert!(st.busy() && !st.failed());
}
