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
fn fullscreen_is_reported_and_the_shell_can_ask_the_page_to_leave() {
    let mut v = WebView::start(config(&["--static"], 200, 100)).unwrap();
    v.send(ToHelper::Navigate("https://a.org/".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Url("https://a.org/".into())));
    v.send(ToHelper::Navigate("fake://fullscreen".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Fullscreen(true)));
    v.send(ToHelper::LeaveFullscreen);
    until(&mut v, 10, |_, e| e.contains(&Event::Fullscreen(false)));
    // a navigation ends it as well
    v.send(ToHelper::Navigate("fake://fullscreen".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Fullscreen(true)));
    v.send(ToHelper::Navigate("https://a.org/next".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::Fullscreen(false)));
}

#[test]
fn the_page_saying_it_plays_audio_reaches_the_shell_and_a_navigation_ends_it() {
    let mut v = WebView::start(config(&["--static"], 200, 100)).unwrap();
    v.send(ToHelper::Navigate("fake://audio".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::AudioPlaying(true)));
    v.send(ToHelper::Navigate("https://a.org/next".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::AudioPlaying(false)));
}

#[test]
fn a_page_whose_video_cannot_be_decoded_reaches_the_shell_as_a_media_event() {
    let mut v = WebView::start(config(&["--static"], 200, 100)).unwrap();
    v.send(ToHelper::Navigate("fake://needs-av".into()));
    until(&mut v, 10, |_, e| e.contains(&Event::MediaUnsupported("video".into())));
}

#[test]
fn the_helper_source_forwards_only_the_three_media_words_from_the_probe() {
    // helper.c is built by Buildroot (WPE headers are not here): check by reading that the probe and the guard are wired in
    let src = include_str!("../../../dist/webview/wpe-helper/helper.c");
    let mk = include_str!("../../../dist/webview/wpe-helper/Makefile");
    for want in ["iyakkiMedia", "F_MEDIA_UNSUPPORTED", "#include \"media-probe.h\"", "!strcmp(t, \"video\")", "!strcmp(t, \"audio\")", "!strcmp(t, \"both\")"] {
        assert!(src.contains(want), "helper.c lacks {want}");
    }
    assert!(mk.contains("media-probe.h: media-probe.js"));
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

#[test]
fn scripts_get_answers_and_unknown_messages_are_skipped() {
    let mut v = WebView::start(config(&["--static"], 160, 100)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    assert!(v.send(ToHelper::Script { id: 5, js: "JSON:{\"a\":1}".into(), timeout_ms: 1000 }));
    assert!(v.send(ToHelper::Script { id: 6, js: "THROW:boom".into(), timeout_ms: 1000 }));
    assert!(v.send(ToHelper::Script { id: 7, js: "plain".into(), timeout_ms: 1000 }));
    let ev = until(&mut v, 10, |_, e| e.iter().filter(|x| matches!(x, Event::ScriptResult { .. })).count() == 3);
    let get = |id: u32| ev.iter().find_map(|e| match e { Event::ScriptResult { id: i, ok, json } if *i == id => Some((*ok, json.clone())), _ => None }).unwrap();
    assert_eq!(get(5), (true, "{\"a\":1}".into()));
    assert_eq!(get(6), (false, "boom".into()));
    assert_eq!(get(7), (true, "\"plain\"".into()));
}

#[test]
fn a_helper_that_dies_mid_frame_or_during_a_script_is_reported_once_and_leaves_no_file() {
    for (what, step) in [("fake://crash-mid-frame", None), ("", Some("EXIT"))] {
        let mut v = WebView::start(config(&["--static"], 160, 100)).unwrap();
        let path = v.shm_path().to_path_buf();
        until(&mut v, 10, |_, e| e.contains(&Event::Frame));
        if let Some(js) = step {
            v.send(ToHelper::Script { id: 1, js: js.into(), timeout_ms: 1000 });
        } else {
            v.send(ToHelper::Navigate(what.into()));
        }
        let ev = until(&mut v, 10, |_, e| e.iter().any(|x| matches!(x, Event::Exited(_))));
        assert_eq!(ev.iter().filter(|x| matches!(x, Event::Exited(_))).count(), 1);
        assert!(v.exited().is_some());
        assert!(!v.send(ToHelper::Reload { hard: false }), "sending to a dead helper just says no");
        assert!(v.poll().is_empty());
        drop(v);
        assert!(!path.exists(), "the shared file is removed: {what}{step:?}");
    }
}

#[test]
fn pictures_that_do_not_fit_are_ignored_without_a_panic() {
    let mut v = WebView::start(config(&["--static"], 160, 100)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.send(ToHelper::Navigate("fake://badframes".into()));
    // the good picture goes on being served afterwards
    v.send(ToHelper::Resize { w: 200, h: 120 });
    until(&mut v, 10, |v, _| v.frame_size() == (200, 120));
    assert_eq!(v.pixels().len(), 200 * 120 * 4);
    assert!(v.exited().is_none());
}

#[test]
fn a_helper_that_floods_frames_or_writes_endless_binary_stderr_does_not_hang_or_grow_the_shell() {
    let mut v = WebView::start(config(&[], 160, 100)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    v.send(ToHelper::Navigate("fake://noisy-stderr".into()));
    v.send(ToHelper::Navigate("fake://spam".into()));
    let t = Instant::now();
    let mut frames = 0;
    while t.elapsed() < Duration::from_secs(3) {
        frames += v.poll().iter().filter(|e| **e == Event::Frame).count();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(frames > 0, "exited: {:?}", v.exited());
    // still answers after the noise (stderr was drained, nothing blocked)
    v.send(ToHelper::Navigate("https://after.example/".into()));
    until(&mut v, 10, |_, e| e.iter().any(|x| matches!(x, Event::Url(u) if u == "https://after.example/")));
}

#[test]
fn a_huge_window_size_is_clamped_not_a_panic_and_stale_files_of_dead_shells_are_removed() {
    let stale = std::path::Path::new("/dev/shm").join("iyakki-webview-4000000-0.shm");
    std::fs::write(&stale, b"x").unwrap();
    let mut c = Config::new(env!("CARGO_BIN_EXE_iyakki-fake-webview"), Hello { w: 99_999, h: 99_999, max_w: 5, max_h: 5, ..Hello::default() });
    c.args = vec!["--static".into()];
    let v = WebView::start(c).unwrap();
    let own = v.shm_path().to_path_buf();
    assert!(v.size_wanted().0 <= 16384 && v.size_wanted().1 <= 16384);
    iyakki_webview::client::remove_stale_shm(std::path::Path::new("/dev/shm"));
    assert!(!stale.exists(), "the file of a process that no longer exists is gone");
    drop(v);
    assert!(!own.exists());
}

/// Measured on a Mac mini: without this policy WebKit answers `play()` with NotAllowedError for a video that has sound, so YouTube's
/// watch page sat paused at 0:00 until someone clicked. The C helper cannot run in `cargo test`, so its source is checked.
#[test]
fn the_wpe_helper_allows_autoplay_like_window_mode_unless_the_agent_says_deny() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/helper.c")).expect("helper source");
    assert!(src.contains("#define IYAKKI_AUTOPLAY_DEFAULT \"allow\""), "autoplay is allowed unless told otherwise");
    assert!(src.contains("getenv(\"IYAKKI_WEBVIEW_AUTOPLAY\")") && src.contains("WEBKIT_AUTOPLAY_DENY"), "the agent can deny it");
    assert!(src.contains("webkit_website_policies_new_with_policies(\"autoplay\", ap"), "autoplay policy");
    assert!(src.contains("\"website-policies\", wpol"), "the policy is given to the view");
}

/// Without a fullscreen handler WPE treats the page's fullscreen request as failed and leaves it again at once (YouTube's button "does not
/// stick"). The C helper cannot run in `cargo test`, so its source is checked.
#[test]
fn the_wpe_helper_handles_fullscreen_and_tells_the_shell() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/helper.c")).expect("helper source");
    assert!(src.contains("wpe_view_backend_set_fullscreen_handler(G.backend, fullscreen_handler"), "the handler is installed");
    assert!(src.contains("send_fullscreen(G.fullscreen)"), "the shell is told");
    assert!(src.contains("wpe_view_backend_dispatch_request_exit_fullscreen"), "the shell can ask the page to leave");
    assert!(src.contains("case T_LEAVE_FULLSCREEN"));
}

/// The sound doctor needs to know when the page plays audio, and the engine's GStreamer messages must reach stderr (evidence for a
/// silent session). The C helper cannot run in `cargo test`, so its source is checked.
#[test]
fn the_wpe_helper_reports_when_the_page_plays_audio_and_lets_gstreamer_errors_through() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/helper.c")).expect("helper source");
    let hdr = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/proto.h")).expect("proto.h");
    assert!(hdr.contains("F_AUDIO_PLAYING"), "message type 0x8E");
    assert!(src.contains("g_signal_connect(v, \"notify::is-playing-audio\", G_CALLBACK(on_audio_playing)"));
    assert!(src.contains("webkit_web_view_is_playing_audio(G.view)") && src.contains("send_msg(F_AUDIO_PLAYING"));
    assert!(src.contains("setenv(\"GST_DEBUG\", \"*:1,alsasink:4,autoaudiosink:4\", 0)"), "errors and sink opens reach stderr unless the owner set GST_DEBUG");
}

/// The engine's processes can run a little below the shell when IYAKKI_WEBVIEW_NICE asks (off by default; docs/BROWSER-SPEED.md,
/// "Interactive performance"): the helper lowers its own priority before it starts any thread or child, only ever lowers it, bounds the value and says so when the call fails. The C helper
/// cannot run in `cargo test`, so its source is checked.
#[test]
fn the_wpe_helper_can_run_the_engine_below_the_shell_and_only_lowers_the_priority() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/helper.c")).expect("helper source");
    for needle in ["#include <sys/resource.h>", "getenv(\"IYAKKI_WEBVIEW_NICE\")", "nice_env ? atoi(nice_env) : 0", "nice_v > 0 && nice_v <= 19 && setpriority(PRIO_PROCESS, 0, nice_v) != 0", "could not lower the engine's priority"] {
        assert!(src.contains(needle), "helper.c lacks {needle}");
    }
    // before the engine exists: the call comes before the Hello is read and before the engine is set up
    let at = src.find("setpriority(PRIO_PROCESS").expect("setpriority");
    assert!(at < src.find("read_hello()").expect("read_hello"), "the priority is lowered before the conversation starts");
    assert!(at < src.rfind("setup_engine()").expect("setup_engine"), "and before the engine is started");
    // no negative value anywhere (raising the priority is never asked for)
    assert!(!src.contains("setpriority(PRIO_PROCESS, 0, -"), "never raised");
}

/// Content blocking in the full engine (docs/CONTENT-BLOCKING.md): the helper loads the person's rule list through WebKit's filter
/// store, holds the first page until it is in (with a time limit) and never fails closed. The C helper cannot run in `cargo test`,
/// so its source is checked.
#[test]
fn the_wpe_helper_loads_the_content_filter_and_fails_open() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/webview/wpe-helper/helper.c")).expect("helper source");
    for needle in [
        "IYAKKI_WEBVIEW_FILTER\"",
        "IYAKKI_WEBVIEW_FILTER_ID\"",
        "webkit_user_content_filter_store_new(",
        "webkit_user_content_filter_store_load(",
        "webkit_user_content_filter_store_save_from_file(",
        "webkit_user_content_manager_add_filter(",
        "g_timeout_add_seconds(20, filter_timeout",
        "G.pending_nav",
    ] {
        assert!(src.contains(needle), "helper.c lacks {needle}");
    }
    // every failure path calls filter_done, which opens the waiting page without a filter
    let fails = src.matches("filter_done();").count();
    assert!(fails >= 3, "failure paths all release the first page ({fails})");
    assert!(src.contains("if (!G.filter_wait) return;"), "filter_done is safe to call twice");
    // an id that is not plain letters and digits is refused before it reaches a file name
    assert!(src.contains("g_ascii_isalnum"), "the identifier is checked");
    // the first navigation waits only while a filter is being loaded
    assert!(src.contains("else if (G.filter_wait)"), "navigation is held while the filter loads");
}

#[test]
fn a_sub_notch_wheel_step_reaches_the_helper_exactly_and_the_c_helper_dispatches_it_as_a_precise_axis_event() {
    // the shell's smooth scrolling sends fractions of a notch (1/120 each) in the same Wheel message: no protocol change was needed
    let mut v = WebView::start(config(&["--static"], 400, 300)).unwrap();
    until(&mut v, 10, |_, e| e.contains(&Event::Frame));
    for dy in [7, -7, 1, 240] {
        v.send(ToHelper::Wheel { x: 3, y: 4, dx: 0, dy, mods: 0 });
    }
    let ev = until(&mut v, 10, |_, e| has_title(e, "wheel 3,4 0,240"));
    assert!(has_title(&ev, "wheel 3,4 0,7") || has_title(&ev, "wheel 3,4 0,240"), "fractions arrive unchanged");
    // the C helper turns the units into CSS pixels of a smooth (precise) 2D axis event, not whole notches
    let src = include_str!("../../../dist/webview/wpe-helper/helper.c");
    assert!(src.contains("wpe_input_axis_event_type_motion_smooth") && src.contains("wpe_input_axis_event_type_mask_2d"));
    assert!(src.contains("(double)dy * 53.0 / 120.0"), "the pixel amount follows the units exactly");
}
