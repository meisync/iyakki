//! Settings -> Last session log and the "ended unexpectedly" note on the Overview page: who sees it, what the viewer shows (the last 200 lines),
//! scrolling, switching between the crash, the boot before and this boot, "I have seen it", and the page at phone to 4K sizes and in a narrow pane.

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_storage::crashlog as cl;

fn app(tag: &str) -> (AppCx, SettingsApp, std::path::PathBuf) {
    let cx = cx_with_admin_store(tag);
    let dir = temp_dir(&format!("{tag}-log"));
    let a = SettingsApp::new(&cx).with_crash_dir(dir.clone());
    (cx, a, dir)
}

fn crash(dir: &std::path::Path, lines: usize) {
    let s = |id: &str| cl::Session { boot_id: id.into(), started: 1_792_771_000, cmdline: "quiet".into(), clean: false };
    cl::begin_session(dir, &s("aaaa0001"));
    let mut t = String::new();
    for i in 0..lines {
        t.push_str(&format!("2026-10-23 16:00:{:02} [{:>5}.000000] <6> kernel line {i}\n", i % 60, i));
    }
    std::fs::write(dir.join("kmsg-aaaa0001.log"), t).unwrap();
    cl::begin_session(dir, &s("aaaa0002"));
}

fn go(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    for _ in 0..60 {
        if a.tab() == t {
            return;
        }
        a.handle(&Event::Key(Key::Right), cx);
    }
    panic!("no page {t:?}");
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

#[test]
fn the_overview_lists_an_unexpected_end_once_and_the_button_opens_the_log() {
    let (cx, mut a, dir) = app("crash-ov");
    assert!(!a.attention(&cx).iter().any(|x| x.tab == Tab::SessionLog), "nothing to report on a clean history");
    crash(&dir, 30);
    let at = a.attention(&cx).into_iter().find(|x| x.tab == Tab::SessionLog).expect("listed");
    assert!(at.title.starts_with("The last session ended unexpectedly (2026-10-"), "{}", at.title);
    assert!(at.why.contains("Open the log"));
    a.go_to(Tab::SessionLog);
    assert_eq!(a.tab(), Tab::SessionLog);
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert!(!a.attention(&cx).iter().any(|x| x.tab == Tab::SessionLog), "I have seen it clears the note");
    assert!(dir.join("kmsg-crash-aaaa0001.log").exists(), "the file stays");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("unclean-shutdown.seen")));
}

#[test]
fn the_viewer_shows_the_last_200_lines_and_scrolls() {
    let (cx, mut a, dir) = app("crash-view");
    crash(&dir, 500);
    go(&mut a, &cx, Tab::SessionLog);
    let g = draw(&mut a, &cx, 1280, 720);
    let _ = g;
    assert_eq!(a.crash.lines.len(), VIEW_LINES_FOR_TEST);
    assert!(a.crash.lines.last().unwrap().contains("kernel line 499") && a.crash.lines[0].contains("kernel line 300"));
    assert_eq!(a.crash.scroll, VIEW_LINES_FOR_TEST - 1, "starts at the end: that is where a crash is");
    for k in [Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Up, Key::Down] {
        a.handle(&Event::Key(k), &cx);
    }
    a.handle(&Event::Key(Key::Home), &cx);
    assert_eq!(a.crash.scroll, 0);
    a.handle(&Event::Key(Key::PageDown), &cx);
    assert_eq!(a.crash.scroll, 10);
    // other boots: nothing kept for "the boot before" yet, then this boot has no file
    a.handle(&Event::Key(Key::Char('p')), &cx);
    assert!(a.crash.lines.is_empty() && a.crash.name.is_empty());
    let _ = draw(&mut a, &cx, 1280, 720);
    std::fs::write(dir.join("kmsg-previous-1.log"), "[ 1.0] <6> earlier\n").unwrap();
    a.handle(&Event::Key(Key::Char('r')), &cx);
    assert_eq!(a.crash.lines, ["[ 1.0] <6> earlier"]);
}

const VIEW_LINES_FOR_TEST: usize = 200;

#[test]
fn only_an_administrator_has_the_page() {
    let (_cx, a, _d) = app("crash-admin");
    assert!(a.tabs().contains(&Tab::SessionLog));
    let mut cx = cx_with_admin_store("crash-user");
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::SessionLog));
}

#[test]
fn the_page_fits_from_a_phone_to_4k_and_in_a_narrow_pane() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (3840, 2160), (480, 900)] {
        let (cx, mut a, dir) = app(&format!("crash-geo-{w}"));
        crash(&dir, 300);
        go(&mut a, &cx, Tab::SessionLog);
        let g = draw(&mut a, &cx, w, h);
        let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.rect.h < 90.0).collect();
        assert!(buttons.len() >= 3, "{w}x{h}: three buttons, got {}", buttons.len());
        for (i, b) in buttons.iter().enumerate() {
            assert!(b.rect.x >= 0.0 && b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y >= 0.0 && b.rect.y + b.rect.h <= h as f32 + 0.5, "{w}x{h} {} {:?}", b.label, b.rect);
            let o = |p: R, q: R| p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
            assert!(buttons[i + 1..].iter().all(|n| !o(b.rect, n.rect)), "{w}x{h}: {} overlaps another button", b.label);
        }
    }
}
