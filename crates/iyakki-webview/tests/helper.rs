//! The client against the real fake helper program: frames through a real shared file, input forwarding, crash and restart.

use std::time::{Duration, Instant};

use iyakki_webview::proto::{Hello, MouseKind, ToHelper, BUTTON_LEFT, HELD_LEFT, MOD_CTRL};
use iyakki_webview::{Config, Event, WebView};

fn config(extra: &[&str], w: u32, h: u32) -> Config {
    let mut c = Config::new(env!("CARGO_BIN_EXE_iyakki-fake-webview"), Hello { w, h, max_w: 1024, max_h: 768, ..Hello::default() });
    c.args = extra.iter().map(|s| s.to_string()).collect();
    c
}

/// Poll until `pred` is true for the events seen so far (returns all of them) or time runs out.
fn until(v: &mut WebView, secs: u64, mut pred: impl FnMut(&WebView, &[Event]) -> bool) -> Vec<Event> {
    let mut all = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        all.extend(v.poll());
        if pred(v, &all) {
            return all;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out; events so far: {:?}", all.iter().filter(|e| !matches!(e, Event::Frame)).collect::<Vec<_>>());
}

fn has_log(ev: &[Event], needle: &str) -> bool {
    ev.iter().any(|e| matches!(e, Event::Log(l) if l.contains(needle)))
}

fn has_title(ev: &[Event], needle: &str) -> bool {
    ev.iter().any(|e| matches!(e, Event::Title(t) if t.contains(needle)))
}

fn px(v: &WebView, x: u32, y: u32) -> [u8; 4] {
    let (w, _) = v.frame_size();
    let o = ((y * w + x) * 4) as usize;
    v.pixels()[o..o + 4].try_into().unwrap()
}

#[test]
fn a_frame_travels_through_a_real_shared_file() {
    let mut v = WebView::start(config(&["--static"], 320, 200)).unwrap();
    let path = v.shm_path().to_path_buf();
    assert!(path.exists());
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777, 0o600);
    assert_eq!(meta.len(), 2 * 1024 * 768 * 4, "two slots of the largest size, sparse");
    let ev = until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    assert!(ev.iter().any(|e| matches!(e, Event::Ready { engine, .. } if engine == "fake")));
    assert_eq!(v.engine(), Some(("fake", "1")));
    assert_eq!(v.frame_size(), (320, 200));
    assert_eq!(v.pixels().len(), 320 * 200 * 4);
    assert_eq!(v.take_dirty(), Some(iyakki_webview::Rect::new(0, 0, 320, 200)), "the first frame is whole");
    assert_eq!(v.take_dirty(), None);
    // BGRA: top-left quarter is red-ish (r=220), top-right green, bottom-left blue, bottom-right yellow-ish; all opaque
    let tl = px(&v, 100, 50);
    assert!(tl[2] > 200 && tl[1] < 80 && tl[0] < 80 && tl[3] == 255, "{tl:?}");
    let tr = px(&v, 250, 50);
    assert!(tr[1] > 180 && tr[2] < 80, "{tr:?}");
    let bl = px(&v, 100, 150);
    assert!(bl[0] > 200, "{bl:?}");
    drop(v);
    assert!(!path.exists(), "the shared file is removed with the view");
}

#[test]
fn the_picture_animates_and_only_the_changed_columns_are_reported() {
    let mut v = WebView::start(config(&["--fps", "60"], 640, 360)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.take_dirty();
    let start = v.frames_received();
    let t = Instant::now();
    let mut small = 0;
    let mut bar_seen = std::collections::HashSet::new();
    while t.elapsed() < Duration::from_millis(1000) {
        if v.poll().contains(&Event::Frame) {
            if let Some(r) = v.take_dirty() {
                if r.w < 100 && r.h == 360 {
                    small += 1;
                }
            }
            for x in (0..640).step_by(4) {
                if px(&v, x, 5) == [255, 255, 255, 255] {
                    bar_seen.insert(x);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let n = v.frames_received() - start;
    assert!(n >= 15, "at least 15 frames in a second, got {n}");
    assert!(small >= 10, "frames carry only the bar's columns: {small}");
    assert!(bar_seen.len() >= 8, "the bar moved across several positions: {}", bar_seen.len());
}

#[test]
fn input_reaches_the_helper_with_exact_values() {
    let mut v = WebView::start(config(&["--static"], 400, 300)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.take_dirty();
    assert!(v.send(ToHelper::Mouse { kind: MouseKind::Move, x: 123, y: 77, button: 0, buttons: 0, mods: 0, clicks: 0 }));
    let ev = until(&mut v, 10, |_, e| has_title(e, "mouse 123,77 move") && e.contains(&Event::Frame));
    assert!(has_log(&ev, "Mouse { kind: Move, x: 123, y: 77"));
    assert_eq!(px(&v, 123, 77), [255, 0, 255, 255], "the helper drew its mouse square where the shell said");
    let dirty = v.take_dirty().unwrap();
    assert!(dirty.w <= 9 + 9 && dirty.h <= 9, "only the square: {dirty:?}");
    // a press with modifiers, a key, a wheel, text
    v.send(ToHelper::Mouse { kind: MouseKind::Down, x: 5, y: 6, button: BUTTON_LEFT, buttons: HELD_LEFT, mods: MOD_CTRL, clicks: 2 });
    v.send(ToHelper::Key { down: true, keysym: 0xff0d, mods: 0, text: "\r".into() });
    v.send(ToHelper::Wheel { x: 1, y: 2, dx: 0, dy: 120, mods: 0 });
    v.send(ToHelper::Text("héllo".into()));
    let ev = until(&mut v, 10, |_, e| has_title(e, "text \"héllo\""));
    assert!(has_log(&ev, "kind: Down, x: 5, y: 6, button: 1, buttons: 1, mods: 2, clicks: 2"));
    assert!(has_title(&ev, "key down 0xff0d mods 0 text \"\\r\""));
    assert!(has_title(&ev, "wheel 1,2 0,120"));
    // the cursor message comes back too
    v.send(ToHelper::Mouse { kind: MouseKind::Move, x: 10, y: 10, button: 0, buttons: 0, mods: 0, clicks: 0 });
    until(&mut v, 10, |_, e| e.contains(&Event::Cursor(1)));
}

#[test]
fn navigation_state_flows_back() {
    let mut v = WebView::start(config(&["--static"], 200, 100)).unwrap();
    v.send(ToHelper::Navigate("https://a.org/one".into()));
    v.send(ToHelper::Navigate("https://a.org/two".into()));
    let ev = until(&mut v, 10, |_, e| e.contains(&Event::History { can_back: true, can_forward: false }));
    assert!(ev.contains(&Event::Url("https://a.org/two".into())));
    assert!(has_title(&ev, "fake page https://a.org/two"));
    assert!(ev.contains(&Event::Loading { loading: true, progress: 30 }) && ev.contains(&Event::Loading { loading: false, progress: 100 }));
    v.send(ToHelper::Back);
    let ev = until(&mut v, 10, |_, e| e.contains(&Event::History { can_back: false, can_forward: true }));
    assert!(ev.contains(&Event::Url("https://a.org/one".into())));
    assert_eq!(v.last_url(), "https://a.org/one");
    v.send(ToHelper::Navigate("fake://download/report.pdf".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Download { url: "https://example.org/files/report.pdf".into(), name: "report.pdf".into() }));
    v.send(ToHelper::Navigate("fake://clipboard".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Clipboard("copied text".into())));
}

#[test]
fn a_crash_is_detected_once_and_a_restart_returns_to_the_last_good_page() {
    let mut v = WebView::start(config(&["--static"], 300, 200)).unwrap();
    v.send(ToHelper::Navigate("https://good.example/page".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Url("https://good.example/page".into())));
    v.send(ToHelper::Zoom(150));
    v.send(ToHelper::Resize { w: 500, h: 250 });
    v.send(ToHelper::Navigate("fake://crash".into()));
    let ev = until(&mut v, 10, |_, e| e.iter().any(|x| matches!(x, Event::Exited(_))));
    let why = ev.iter().find_map(|e| if let Event::Exited(w) = e { Some(w.clone()) } else { None }).unwrap();
    assert!(why.contains("exit status: 3") || why.contains("closed the connection"), "{why}");
    assert!(why.contains("crashing on request"), "the helper's last words are in the message: {why}");
    assert!(v.exited().is_some());
    assert!(!v.send(ToHelper::Reload { hard: false }), "sending to a dead helper says so");
    assert_eq!(v.poll().iter().filter(|e| matches!(e, Event::Exited(_))).count(), 0, "reported once");
    v.restart().unwrap();
    let ev = until(&mut v, 10, |_, e| has_log(e, "Navigate(\"https://good.example/page\")") && e.contains(&Event::Frame));
    assert!(has_log(&ev, "Zoom(150)"));
    assert!(v.exited().is_none());
    assert_eq!(v.frame_size(), (500, 250), "the size before the crash is kept");
}

#[test]
fn resizing_changes_the_picture_and_is_clamped_to_the_room_in_the_file() {
    let mut v = WebView::start(config(&["--static"], 320, 200)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.send(ToHelper::Resize { w: 640, h: 480 });
    until(&mut v, 10, |v, _| v.frame_size() == (640, 480));
    assert_eq!(v.pixels().len(), 640 * 480 * 4);
    v.take_dirty();
    // the quarters moved with the size
    let br = px(&v, 500, 400);
    assert!(br[2] > 200 && br[1] > 200, "bottom right is yellowish: {br:?}");
    v.send(ToHelper::Resize { w: 5000, h: 5000 });
    assert_eq!(v.size_wanted(), (1024, 768));
    until(&mut v, 10, |v, _| v.frame_size() == (1024, 768));
    // a size that did not change sends nothing
    assert!(v.send(ToHelper::Resize { w: 1024, h: 768 }));
}

#[test]
fn a_program_that_is_missing_or_exits_at_once_is_reported_not_hung() {
    assert!(WebView::start(Config::new("/nonexistent/helper", Hello::default())).is_err());
    let mut v = WebView::start(Config::new("/bin/true", Hello::default())).unwrap();
    let ev = until(&mut v, 10, |_, e| e.iter().any(|x| matches!(x, Event::Exited(_))));
    // however the helper's end shows (a closed connection, a broken pipe, an exit status), the person is told it is gone
    assert!(ev.iter().any(|e| matches!(e, Event::Exited(w) if !w.is_empty())));
}

#[test]
fn hidden_views_get_no_frames_and_come_back_whole() {
    let mut v = WebView::start(config(&["--fps", "60"], 200, 100)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.send(ToHelper::SetVisible(false));
    std::thread::sleep(Duration::from_millis(150));
    v.poll();
    let n = v.frames_received();
    std::thread::sleep(Duration::from_millis(300));
    v.poll();
    assert!(v.frames_received() <= n + 1, "no frames while hidden");
    v.send(ToHelper::SetVisible(true));
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
}
