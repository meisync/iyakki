//! The clipboard's "Scan QR" with cameras made up in software: QR pictures from our own encoder for a short text, a long
//! token-like secret, an address, text over the limit and pictures with no code; hiding and Show; the secret flag; and what stops
//! the camera. Nothing here waits for a fixed time: every wait is for a condition (a generous limit only ends a hung test).

use super::*;
use crate::agent_login_scan::tests::Stage;
use crate::camera::detect::tests::FakeMachine;
use crate::ui::clipboard as cb;
use std::sync::atomic::Ordering;
use std::time::Instant;

const TOKEN: &str = "k9Xq2LmVb7TzR4wYc1HdN8sPfJ6gUe3A";
const SK: &str = "sk-ant-api03-Qw3rTy9uIoP1aSdF5gHjK7lZxC2vBnM4";

fn rig(tag: &str, camera: bool, st: &Stage) -> (FakeMachine, Arc<CamSource>) {
    let machine = FakeMachine::new(&format!("clipscan-{tag}"));
    if camera {
        machine.plug_webcam(0, "FaceTime HD Camera");
    }
    let src = CamSource::with(machine.presence(), st.opener());
    (machine, src)
}

fn pump(c: &mut ClipScan, what: &str, mut done: impl FnMut(&ClipScan) -> bool) {
    let t0 = Instant::now();
    loop {
        c.tick();
        if done(c) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "timed out waiting for {what}: {:?}", c.session().said());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A dialog with the camera on and `text` shown to it, waiting until it was read.
fn read(tag: &str, text: &str) -> (FakeMachine, Stage, ClipScan) {
    cb::clear();
    let st = Stage::new(text);
    let (m, src) = rig(tag, true, &st);
    let mut c = ClipScan::open(src);
    assert_eq!(c.session().step(), Step::Asking);
    assert_eq!(c.handle(&Event::Tab(T_ALLOW)), Out::Stay);
    pump(&mut c, "the read", |c| c.reviewing());
    (m, st, c)
}

#[test]
fn a_short_text_is_shown_before_it_is_copied_and_then_copied_normally() {
    let (_m, st, mut c) = read("short", "meeting at 10 in room 4");
    assert!(!c.session().on(), "the camera is off as soon as something is read");
    assert_eq!(c.shown().as_deref(), Some("meeting at 10 in room 4"));
    assert!(!c.masked() && !c.secret(), "plain text: not hidden, not a secret");
    assert!(cb::history().is_empty() && cb::take_host_copy().is_none(), "nothing is copied before the person says so");
    assert_eq!(c.handle(&Event::Key(Key::Enter)), Out::Copied { chars: 23, secret: false });
    assert_eq!(cb::latest().as_deref(), Some("meeting at 10 in room 4"));
    assert_eq!(cb::take_host_copy().as_deref(), Some("meeting at 10 in room 4"));
    drop(c);
    assert!(st.closed.load(Ordering::SeqCst));
}

#[test]
fn a_long_token_like_secret_is_hidden_until_show_and_is_a_secret_by_default() {
    let (_m, _st, mut c) = read("secret", SK);
    assert!(c.masked() && c.secret(), "hidden, and the Secret box starts ticked");
    let shown = c.shown().unwrap();
    assert!(shown.chars().all(|ch| ch == '•') && !shown.contains("sk-"), "{shown}");
    c.handle(&Event::Tab(T_SHOW));
    assert_eq!(c.shown().as_deref(), Some(SK), "Show reveals it");
    assert!(c.secret(), "showing does not change the Secret box");
    c.handle(&Event::Tab(T_SHOW));
    assert!(c.masked());
    // the box can be switched off: then it is a normal copy in the history
    c.handle(&Event::Tab(T_SECRET));
    assert!(!c.secret());
    assert_eq!(c.handle(&Event::Tab(T_COPY)), Out::Copied { chars: SK.chars().count(), secret: false });
    assert_eq!(cb::history(), vec![SK.to_string()]);
}

#[test]
fn a_secret_copy_stays_out_of_the_history_and_pastes_once() {
    let (_m, _st, mut c) = read("secret-copy", TOKEN);
    assert!(c.masked(), "a long token-like string is hidden");
    assert_eq!(c.handle(&Event::Tab(T_COPY)), Out::Copied { chars: 32, secret: true });
    assert!(cb::history().is_empty(), "not in the history list");
    assert_eq!(cb::secret_waiting().map(|s| s.0), Some(32));
    assert_eq!(cb::take_host_copy().as_deref(), Some(TOKEN), "the host gets it once, to paste on the computer");
    assert_eq!(cb::paste().as_deref(), Some(TOKEN));
    assert_eq!(cb::paste(), None, "gone after the first paste");
    assert_eq!(cb::take_host_copy().as_deref(), Some(""), "and the host's copy is overwritten");
}

#[test]
fn an_address_is_shown_and_copied_as_text_and_never_opened() {
    let url = "https://example.test/path?x=1";
    let (_m, _st, mut c) = read("url", url);
    assert!(!c.masked(), "an ordinary address is not hidden");
    assert_eq!(c.shown().as_deref(), Some(url));
    assert_eq!(c.handle(&Event::Key(Key::Enter)), Out::Copied { chars: url.len(), secret: false });
    assert_eq!(cb::latest().as_deref(), Some(url));
    let body = include_str!("clip_scan.rs").split("#[cfg(test)]").next().unwrap();
    for bad in ["open_url", "open_in_browser", "Command", "xdg-open", "newtab"] {
        assert!(!body.contains(bad), "the scan must not open anything: {bad}");
    }
}

#[test]
fn long_text_is_cut_on_screen_but_copied_whole() {
    // about 200 characters: what a QR code of the size we read can hold
    let all: String = (0..40).map(|i| format!("w{i:02} ")).collect::<String>().trim().to_string();
    assert!(all.chars().count() > SHOW_CHARS && all.len() < 230);
    let (_m, _st, mut c) = read("long", &all);
    let shown = c.shown().unwrap();
    assert!(shown.ends_with('…') && shown.chars().count() == SHOW_CHARS + 1, "{}", shown.len());
    assert_eq!(c.handle(&Event::Key(Key::Enter)), Out::Copied { chars: all.chars().count(), secret: false });
    assert_eq!(cb::latest().as_deref(), Some(all.as_str()));
}

#[test]
fn text_over_the_limit_is_refused_without_repeating_it_and_scanning_goes_on() {
    cb::clear();
    let big = "z".repeat(60);
    let st = Stage::new(&big);
    let (_m, src) = rig("limit", true, &st);
    let mut c = ClipScan::open(src).with_max_bytes(40);
    c.handle(&Event::Tab(T_ALLOW));
    pump(&mut c, "the refusal", |c| c.session().said().is_some());
    let (m, ok) = c.session().said().unwrap().clone();
    assert!(!ok && m.contains("too much") && !m.contains("zzzz"), "{m}");
    assert!(c.session().on() && !c.reviewing(), "still looking");
    st.show("short enough");
    pump(&mut c, "the short text", |c| c.reviewing());
    assert_eq!(c.shown().as_deref(), Some("short enough"));
    assert!(cb::history().is_empty());
}

#[test]
fn pictures_with_no_code_read_nothing_and_leave_the_camera_on() {
    cb::clear();
    let st = Stage::new("");
    let (_m, src) = rig("garbage", true, &st);
    let mut c = ClipScan::open(src);
    c.handle(&Event::Tab(T_ALLOW));
    for _ in 0..40 {
        c.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!c.reviewing() && c.session().on());
    assert!(cb::history().is_empty() && cb::take_host_copy().is_none());
}

#[test]
fn no_camera_is_said_plainly() {
    let st = Stage::new("x");
    let (_m, src) = rig("nocam", false, &st);
    let c = ClipScan::open(src);
    assert_eq!(c.session().step(), Step::Off);
    let (m, ok) = c.session().said().unwrap();
    assert!(!ok && m.contains("no camera"), "{m}");
    assert_eq!(st.opens.load(Ordering::SeqCst), 0);
}

#[test]
fn a_no_keeps_the_camera_off_and_esc_stops_it_and_closes() {
    cb::clear();
    let st = Stage::new("");
    let (_m, src) = rig("stop", true, &st);
    let mut c = ClipScan::open(src);
    assert_eq!(c.handle(&Event::Key(Key::Esc)), Out::Close);
    assert_eq!(st.opens.load(Ordering::SeqCst), 0, "asked first: nothing was opened");
    let (_m2, src) = rig("stop2", true, &st);
    let mut c = ClipScan::open(src);
    c.handle(&Event::Key(Key::Enter));
    assert!(c.session().on());
    assert_eq!(c.handle(&Event::Key(Key::Esc)), Out::Close);
    drop(c);
    assert!(st.closed.load(Ordering::SeqCst), "closing takes the camera with it");
    // the Stop button too
    let (_m3, src) = rig("stop3", true, &st);
    let mut c = ClipScan::open(src);
    c.handle(&Event::Tab(T_ALLOW));
    assert_eq!(c.handle(&Event::Tab(T_STOP)), Out::Close);
}

#[test]
fn the_camera_stops_by_itself_at_the_time_limit_and_scan_again_asks_again() {
    cb::clear();
    let st = Stage::new("");
    let (_m, src) = rig("time", true, &st);
    let mut c = ClipScan::open(src).with_limit(Duration::from_millis(400));
    c.handle(&Event::Tab(T_ALLOW));
    pump(&mut c, "the time limit", |c| c.session().step() == Step::Off);
    assert!(c.session().said().unwrap().0.contains("No code was read"));
    assert_eq!(c.handle(&Event::Tab(T_AGAIN)), Out::Stay);
    assert_eq!(c.session().step(), Step::Asking, "asks again before turning the camera on");
    assert_eq!(c.handle(&Event::Tab(T_CANCEL)), Out::Close);
}

#[test]
fn scan_again_and_cancel_in_the_review_copy_nothing() {
    let (_m, st, mut c) = read("again", "first text here");
    assert_eq!(c.handle(&Event::Tab(T_AGAIN)), Out::Stay);
    assert!(!c.reviewing() && c.session().step() == Step::Asking);
    st.show("second text here");
    c.handle(&Event::Tab(T_ALLOW));
    pump(&mut c, "the second read", |c| c.reviewing());
    assert_eq!(c.shown().as_deref(), Some("second text here"));
    assert_eq!(c.handle(&Event::Key(Key::Esc)), Out::Close);
    assert!(cb::history().is_empty() && cb::take_host_copy().is_none(), "nothing was copied");
}

#[test]
fn the_scan_code_never_logs_stores_or_audits_what_it_reads() {
    for (name, src) in [("clip_scan.rs", include_str!("clip_scan.rs")), ("scan_core.rs", include_str!("scan_core.rs"))] {
        let body = src.split("#[cfg(test)]").next().unwrap();
        for bad in ["println!", "eprintln!", "dbg!", "log::", "tracing::", "std::fs", "File::", "write(", "record(", "open_url"] {
            assert!(!body.contains(bad), "{name} must not use {bad}");
        }
    }
}

#[test]
fn the_history_limit_is_not_changed_by_scanning() {
    cb::clear();
    cb::set_limit(3);
    for i in 0..5 {
        cb::copy(&format!("note {i}"));
    }
    let (_m, _st, mut c) = {
        let st = Stage::new(TOKEN);
        let (m, src) = rig("limit-keep", true, &st);
        let mut c = ClipScan::open(src);
        c.handle(&Event::Tab(T_ALLOW));
        pump(&mut c, "the read", |c| c.reviewing());
        (m, st, c)
    };
    assert_eq!(c.handle(&Event::Tab(T_COPY)), Out::Copied { chars: 32, secret: true });
    assert_eq!((cb::limit(), cb::history().len()), (3, 3), "a secret takes no place in the list");
    assert_eq!(cb::history(), vec!["note 4", "note 3", "note 2"]);
    cb::clear();
    cb::set_limit(cb::DEFAULT_LIMIT);
}

// ------------------------------------------------------------------------------------------------ in the shell

#[test]
fn the_clipboard_popup_has_scan_qr_and_the_shell_audits_only_the_count() {
    use crate::shell::tests::{login, shell_with_users};
    cb::clear();
    let (mut sh, _) = shell_with_users("clipscan-shell");
    login(&mut sh, "alice", "alicepass1");
    let st = Stage::new(TOKEN);
    let (_m, src) = rig("shell", true, &st);
    sh.set_scan_camera(src);
    let mut g = crate::gfx::Gfx::new(1280, 720);
    sh.handle(&Event::Key(Key::Ctrl('V')));
    assert!(sh.clipboard_picker_open());
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(912) && h.label.contains("Scan QR")), "the button is on the popup");
    sh.handle(&Event::Tab(912));
    assert!(!sh.clipboard_picker_open() && sh.clipboard_scan().is_some(), "the question is up");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(T_ALLOW)), "asked first");
    assert_eq!(st.opens.load(Ordering::SeqCst), 0);
    sh.handle(&Event::Tab(T_ALLOW));
    let t0 = Instant::now();
    while !sh.clipboard_scan().is_some_and(|c| c.reviewing()) {
        sh.handle(&Event::Tick);
        assert!(t0.elapsed() < Duration::from_secs(60), "never read");
        std::thread::sleep(Duration::from_millis(10));
    }
    sh.render(&mut g);
    for l in ["Copy", "Scan again", "Cancel", "Show"] {
        assert!(sh.hits().iter().any(|h| h.label == l), "{l}");
    }
    sh.handle(&Event::Tab(T_COPY));
    assert!(sh.clipboard_scan().is_none());
    // Ctrl+V pastes the secret once
    assert_eq!(cb::secret_waiting().map(|s| s.0), Some(32));
    let log = std::fs::read_to_string(sh.audit_log_path()).unwrap();
    assert!(log.contains("clipboard scan: 32 characters, secret yes"), "{log}");
    assert!(!log.contains(TOKEN), "the text is never in the audit");
    cb::clear();
}

#[test]
fn ctrl_k_in_the_popup_opens_the_scan_and_esc_closes_it() {
    use crate::shell::tests::{login, shell_with_users};
    cb::clear();
    let (mut sh, _) = shell_with_users("clipscan-ctrlk");
    login(&mut sh, "alice", "alicepass1");
    let st = Stage::new("");
    let (_m, src) = rig("shell2", true, &st);
    sh.set_scan_camera(src);
    sh.handle(&Event::Key(Key::Ctrl('V')));
    sh.handle(&Event::Key(Key::Ctrl('k')));
    assert!(sh.clipboard_scan().is_some());
    sh.handle(&Event::Key(Key::Enter));
    assert!(sh.clipboard_scan().is_some_and(|c| c.session().on()));
    sh.handle(&Event::Key(Key::Esc));
    assert!(sh.clipboard_scan().is_none());
    let t0 = Instant::now();
    while !st.closed.load(Ordering::SeqCst) {
        assert!(t0.elapsed() < Duration::from_secs(30), "the camera stayed on");
        std::thread::sleep(Duration::from_millis(10));
    }
}
