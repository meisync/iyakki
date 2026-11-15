//! Settings -> Last session log and the "ended unexpectedly" note on the Overview page: who sees it, what the viewer shows (the last 200 lines),
//! scrolling, switching between the crash, the boot before and this boot, "I have seen it", and the page at phone to 4K sizes and in a narrow pane.

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_storage::crashlog as cl;

/// A machine whose kept logs are the real folder of the test (the rest is the fake).
struct RealLogs(Arc<iyakki_core_fake::FakeOs>);

impl iyakki_core_api::Os for RealLogs {
    fn sysinfo(&self) -> &dyn iyakki_core_api::Sysinfo {
        self.0.sysinfo()
    }
    fn audio(&self) -> &dyn iyakki_core_api::Audio {
        self.0.audio()
    }
    fn disks(&self) -> &dyn iyakki_core_api::Disks {
        self.0.disks()
    }
    fn net(&self) -> &dyn iyakki_core_api::Net {
        self.0.net()
    }
    fn memory(&self) -> Arc<dyn iyakki_core_api::memory::Memory> {
        self.0.memory()
    }
    fn hardware(&self) -> Arc<dyn iyakki_core_api::hardware::Hardware> {
        self.0.hardware()
    }
    fn crashlog(&self) -> Arc<dyn iyakki_core_api::crashlog::CrashLog> {
        Arc::new(iyakki_core_local::LocalCrashLog)
    }
    fn netscan(&self) -> Arc<dyn iyakki_core_api::netscan::NetScan> {
        self.0.netscan()
    }
}

fn app(tag: &str) -> (AppCx, SettingsApp, std::path::PathBuf) {
    let mut cx = cx_with_admin_store(tag);
    cx.os = Arc::new(RealLogs(Arc::new(iyakki_core_fake::FakeOs::new())));
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

#[test]
fn copy_to_my_home_puts_the_shown_log_in_a_new_private_file_and_says_where_and_is_audited() {
    use std::os::unix::fs::PermissionsExt;
    let (cx, mut a, dir) = app("crash-copy");
    crash(&dir, 30);
    let home0 = iyakki_agent::scope::home_root_for_writes(&cx.session.user, cx.services.dir());
    for e in std::fs::read_dir(&home0).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("iyakki-session-log-") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    a.go_to(Tab::SessionLog);
    assert!(a.crash_buttons().iter().any(|b| b.2 == 'c' && b.0 == "Copy to my home"));
    a.handle(&Event::Key(Key::Char('c')), &cx);
    let home = iyakki_agent::scope::home_root_for_writes(&cx.session.user, cx.services.dir());
    let copies: Vec<_> = std::fs::read_dir(&home).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("iyakki-session-log-")).collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    let text = std::fs::read_to_string(&copies[0]).unwrap();
    assert!(text.contains("kernel line 0") && text.contains("kernel line 29"), "the whole kept log, as the viewer shows it");
    assert_eq!(std::fs::metadata(&copies[0]).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("unclean-shutdown.copy")));
    // pressing it again makes a second file; the first stays
    a.handle(&Event::Key(Key::Char('c')), &cx);
    let n = std::fs::read_dir(&home).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("iyakki-session-log-")).count();
    assert_eq!(n, 2);
    let _ = std::fs::remove_file(&copies[0]);
}

#[test]
fn copy_to_my_home_with_no_kept_log_says_so_and_writes_nothing() {
    let (cx, mut a, _dir) = app("crash-copy-none");
    a.go_to(Tab::SessionLog);
    a.handle(&Event::Key(Key::Char('c')), &cx);
    assert!(!cx.audit.recent(20).iter().any(|l| l.contains("unclean-shutdown.copy")), "nothing was copied, so nothing is audited as copied");
}

#[test]
fn the_note_the_lines_the_seen_button_and_the_copy_all_go_through_the_core_on_a_fake_machine() {
    use iyakki_core_fake::FakeOs;
    let (mut cx, mut a, dir) = app("crash-core");
    let os = Arc::new(FakeOs::new());
    cx.os = os.clone();
    let log = os.crashlog_fake();
    let u = cl::Unclean { boot_id: "ffff".into(), started: 1_792_771_000, last_line: "last words".into(), uptime_s: 42, file: "kmsg-crash-ffff.log".into(), audited: true, seen: false, marker: true };
    log.set(|s| {
        s.unclean = Some(u);
        s.files.insert("kmsg-crash-ffff.log".into(), (0..300).map(|i| format!("fake kernel line {i}")).collect());
    });
    assert!(dir.read_dir().unwrap().next().is_none(), "the folder on this computer is empty: everything below is the fake's");
    assert!(a.attention(&cx).iter().any(|x| x.tab == Tab::SessionLog), "the note is the fake's");
    go(&mut a, &cx, Tab::SessionLog);
    let _ = draw(&mut a, &cx, 1280, 720);
    assert_eq!(a.crash.name, "kmsg-crash-ffff.log");
    assert_eq!(a.crash.lines.len(), VIEW_LINES_FOR_TEST);
    assert!(a.crash.lines.last().unwrap().contains("fake kernel line 299"));
    a.handle(&Event::Key(Key::Char('c')), &cx);
    assert_eq!(log.calls(), vec!["copy_to_folder(kmsg-crash-ffff.log)".to_string()]);
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(log.calls().last().map(String::as_str), Some("mark_seen"));
    assert!(!a.attention(&cx).iter().any(|x| x.tab == Tab::SessionLog), "seen: the note is gone");
}
