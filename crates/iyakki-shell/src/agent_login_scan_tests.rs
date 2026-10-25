//! The camera scan of the sign-in dialog with cameras made up in software: no camera, a refused question, a camera that is busy, a
//! code read, an address refused, a code of the wrong shape, the time limit, a camera pulled out, and what stops the camera.

use super::*;
use crate::camera::{CamDevice, Opener, Pix, RawFrame};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use crate::camera::detect::tests::FakeMachine;
use crate::camera::{pixels, Capture, Control, Mode, Opened};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) const CODE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789#st-ate_0123456789ABCDEFGHIJKLMNOPQ";

/// A picture of a QR code of `text` (empty text: a blank wall) as a camera frame: dark modules on a light card in a grey room.
pub(crate) fn frame(text: &str, mirror: bool, w: u32, h: u32) -> RawFrame {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let q = if text.is_empty() { None } else { iyakki_agent::qr::encode(text.as_bytes()) };
    let n = q.as_ref().map_or(0, |q| q.size + 8);
    let scale = if n == 0 { 1 } else { (h as usize * 3 / 4 / n).max(1) };
    let (ox, oy) = ((w as usize - n * scale) / 2, (h as usize - n * scale) / 2);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut v = 150u8;
            if let Some(q) = &q {
                let (mut px, py) = (x as i64 - ox as i64, y as i64 - oy as i64);
                if mirror && px >= 0 {
                    px = (n * scale) as i64 - 1 - px;
                }
                if px >= 0 && py >= 0 && (px as usize) < n * scale && (py as usize) < n * scale {
                    let (mx, my) = (px as usize / scale, py as usize / scale);
                    v = if mx >= 4 && my >= 4 && q.get(mx - 4, my - 4) { 30 } else { 225 };
                }
            }
            let i = (y * w as usize + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    RawFrame { pix: Pix::Yuyv, width: w, height: h, data: pixels::rgba_to_yuyv(w, h, &rgba) }
}

/// What the made-up camera shows right now (changed by the test while it runs), and how it behaves.
#[derive(Clone)]
pub(crate) struct Stage {
    pub text: Arc<Mutex<String>>,
    pub mirror: Arc<AtomicBool>,
    pub opens: Arc<AtomicU64>,
    /// Set when the camera was closed (the capture was dropped).
    pub closed: Arc<AtomicBool>,
    pub open_error: Arc<Mutex<Option<String>>>,
    pub fail_after: Option<u64>,
}

impl Stage {
    pub fn new(text: &str) -> Stage {
        Stage { text: Arc::new(Mutex::new(text.to_string())), mirror: Default::default(), opens: Default::default(), closed: Default::default(), open_error: Default::default(), fail_after: None }
    }
    pub fn show(&self, text: &str) {
        *self.text.lock().unwrap() = text.to_string();
    }
    pub fn opener(&self) -> Arc<dyn Opener> {
        Arc::new(self.clone())
    }
}

struct Cap {
    n: u64,
    st: Stage,
    cache: Option<(String, bool, RawFrame)>,
}

impl Drop for Cap {
    fn drop(&mut self) {
        self.st.closed.store(true, Ordering::SeqCst);
    }
}

impl Capture for Cap {
    fn mode(&self) -> Mode {
        Mode { pix: Pix::Yuyv, width: 640, height: 480 }
    }
    fn next(&mut self, _timeout_ms: u32) -> Result<Option<RawFrame>, String> {
        if self.st.fail_after.is_some_and(|k| self.n >= k) {
            return Err("the camera was disconnected".into());
        }
        std::thread::sleep(Duration::from_millis(15));
        self.n += 1;
        let (text, mirror) = (self.st.text.lock().unwrap().clone(), self.st.mirror.load(Ordering::SeqCst));
        // the picture only changes when the test changes it: make it once
        if !self.cache.as_ref().is_some_and(|(t, m, _)| *t == text && *m == mirror) {
            self.cache = Some((text.clone(), mirror, frame(&text, mirror, 640, 480)));
        }
        Ok(self.cache.as_ref().map(|c| c.2.clone()))
    }
    fn set_control(&mut self, _id: u32, _value: i32) -> Result<(), String> {
        Ok(())
    }
}

impl Opener for Stage {
    fn open(&self, _dev: &CamDevice, _want: Option<Mode>) -> Result<Opened, String> {
        if let Some(e) = self.open_error.lock().unwrap().clone() {
            return Err(e);
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.closed.store(false, Ordering::SeqCst);
        Ok(Opened { cap: Box::new(Cap { n: 0, st: self.clone(), cache: None }), modes: vec![], controls: Vec::<Control>::new() })
    }
}

/// The decoder's time budget per frame in tests: far more than any busy machine takes, so a loaded run never makes it give up
/// (the real 30 ms is tested on its own in `the_production_budget_is_the_decoders_own`).
pub(crate) const TEST_BUDGET: Duration = Duration::from_secs(10);

/// A machine with one webcam (or none) and the scan state machine on top of it.
pub(crate) fn rig(tag: &str, camera: bool, st: &Stage, limit: Duration) -> (FakeMachine, Scan) {
    let machine = FakeMachine::new(&format!("scan-{tag}"));
    if camera {
        machine.plug_webcam(0, "FaceTime HD Camera");
    }
    let src = CamSource::with(machine.presence(), st.opener());
    (machine, Scan::new(src).with_limit(limit).with_budget(TEST_BUDGET))
}

fn poll_until(sc: &mut Scan, what: &str, mut done: impl FnMut(&Scan, Option<String>) -> bool) {
    let t0 = Instant::now();
    loop {
        let got = sc.poll(true);
        if done(sc, got) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "timed out waiting for {what}: {:?} {:?}", sc.step(), sc.said());
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ------------------------------------------------------------------------------------------------ the state machine

#[test]
fn with_no_camera_the_person_is_told_so_and_nothing_is_asked_or_opened() {
    let st = Stage::new(CODE);
    let (_m, mut sc) = rig("nocam", false, &st, Duration::from_secs(60));
    sc.ask();
    assert_eq!(sc.step(), Step::Off, "no question for a camera that is not there");
    let (msg, ok) = sc.said().unwrap();
    assert!(!ok && msg.contains("no camera"), "{msg}");
    assert!(!sc.on());
    assert_eq!(st.opens.load(Ordering::SeqCst), 0);
    // allow without asking does nothing
    sc.allow();
    assert!(!sc.on());
}

#[test]
fn the_camera_opens_only_after_a_yes_and_a_no_keeps_it_off() {
    let st = Stage::new(CODE);
    let (_m, mut sc) = rig("ask", true, &st, Duration::from_secs(60));
    sc.ask();
    assert_eq!(sc.step(), Step::Asking);
    assert_eq!(sc.camera_name(), "FaceTime HD Camera");
    assert_eq!(st.opens.load(Ordering::SeqCst), 0, "asking does not open the camera");
    assert_eq!(sc.poll(true), None, "nothing is read before the yes");
    sc.deny();
    assert_eq!((sc.step(), sc.on()), (Step::Off, false));
    assert!(sc.said().unwrap().0.contains("stays off"));
    assert_eq!(st.opens.load(Ordering::SeqCst), 0, "permission denied: never opened");
    // a second time, yes
    sc.ask();
    sc.allow();
    assert_eq!((sc.step(), sc.on()), (Step::On, true));
    assert_eq!(st.opens.load(Ordering::SeqCst), 1);
    sc.stop(Some("The camera is off."));
    assert!(!sc.on());
    // the capture is dropped on a thread of its own: give it a moment
    poll_until(&mut sc, "the camera to close", |_, _| st.closed.load(Ordering::SeqCst));
}

#[test]
fn a_busy_or_refused_camera_is_reported_plainly_and_stays_off() {
    let st = Stage::new(CODE);
    for (err, want) in [("Device or resource busy", "in use"), ("open /dev/video0: Permission denied", "not allowed"), ("weird failure", "could not be opened")] {
        *st.open_error.lock().unwrap() = Some(err.to_string());
        let (_m, mut sc) = rig("busy", true, &st, Duration::from_secs(60));
        sc.ask();
        sc.allow();
        assert_eq!((sc.step(), sc.on()), (Step::Off, false), "{err}");
        let (msg, ok) = sc.said().unwrap();
        assert!(!ok && msg.contains(want), "{msg}");
        assert!(!msg.contains(err), "the system's words are not passed on raw");
    }
}

#[test]
fn a_code_shown_to_the_camera_is_read_and_the_camera_turns_off() {
    let st = Stage::new("");
    let (_m, mut sc) = rig("ok", true, &st, Duration::from_secs(60));
    sc.ask();
    sc.allow();
    // a blank wall first: nothing happens and the camera stays on
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(400) {
        assert_eq!(sc.poll(true), None);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sc.on() && sc.secs_left() > 50);
    st.show(CODE);
    let mut got = None;
    poll_until(&mut sc, "the code", |_, c| {
        got = c;
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some(CODE));
    assert!(!sc.on(), "off as soon as a code is read");
    assert_eq!(sc.step(), Step::Off);
    assert!(sc.said().unwrap().0.starts_with("Code read") && sc.said().unwrap().1);
    assert!(!sc.said().unwrap().0.contains("AbCdEf"), "the message never repeats the code");
    poll_until(&mut sc, "the camera to close", |_, _| st.closed.load(Ordering::SeqCst));
}

#[test]
fn a_mirrored_picture_works_too() {
    let st = Stage::new(CODE);
    st.mirror.store(true, Ordering::SeqCst);
    let (_m, mut sc) = rig("mirror", true, &st, Duration::from_secs(60));
    sc.ask();
    sc.allow();
    let mut got = None;
    poll_until(&mut sc, "the mirrored code", |_, c| {
        got = c;
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some(CODE));
}

#[test]
fn an_address_or_a_wrong_shape_is_refused_with_a_message_and_scanning_goes_on() {
    let st = Stage::new("https://claude.example.test/oauth/authorize?code=true&state=abc");
    let (_m, mut sc) = rig("shape", true, &st, Duration::from_secs(60));
    sc.ask();
    sc.allow();
    poll_until(&mut sc, "the refusal of the address", |sc, c| {
        assert!(c.is_none(), "an address is never handed back");
        sc.said().is_some()
    });
    assert!(sc.said().unwrap().0.contains("web address") && !sc.said().unwrap().1);
    assert!(!sc.said().unwrap().0.contains("claude.example"), "the message does not repeat what was scanned");
    assert!(sc.on(), "still looking");
    // something short, and a code without the # Claude's has
    for (text, msg) in [("hello", "does not look like"), (&CODE.replace('#', "-")[..], "does not look like")] {
        st.show(text);
        // wait for the message instead of for a fixed time: how long a frame takes depends on how busy the machine is
        poll_until(&mut sc, text, |sc, c| {
            assert_eq!(c, None, "{text}");
            sc.said().is_some_and(|(m, ok)| m.contains(msg) && !*ok)
        });
        assert!(sc.on());
    }
    // then the right one
    st.show(CODE);
    let mut got = None;
    poll_until(&mut sc, "the right code", |_, c| {
        got = c;
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some(CODE));
}

#[test]
fn the_camera_stops_by_itself_at_the_time_limit() {
    let st = Stage::new("");
    let (_m, mut sc) = rig("limit", true, &st, Duration::from_millis(500));
    sc.ask();
    sc.allow();
    poll_until(&mut sc, "the time limit", |sc, _| sc.step() == Step::Off);
    assert!(!sc.on());
    let (msg, ok) = sc.said().unwrap();
    assert!(!ok && msg.contains("No code was read") && msg.contains("camera is off"), "{msg}");
    poll_until(&mut sc, "the camera to close", |_, _| st.closed.load(Ordering::SeqCst));
    assert_eq!(SCAN_SECS, 60, "the real limit is one minute");
}

#[test]
fn the_production_budget_is_the_decoders_own_and_a_given_one_is_used_as_given() {
    // the session decodes with the decoder's own 30 ms (up to 150 ms) unless a test gives it one
    assert_eq!(iyakki_agent::qr_decode::default_budget(640, 480), Duration::from_millis(30));
    assert!(iyakki_agent::qr_decode::default_budget(1920, 1080) <= Duration::from_millis(150));
    let f = frame(CODE, true, 640, 480);
    assert_eq!(crate::scan_core::decode_frame_with(&f, Some(TEST_BUDGET)).as_deref(), Some(CODE.as_bytes()), "mirrored, generous budget");
    // a budget of nothing never reads more than the first candidate, and never panics
    let _ = crate::scan_core::decode_frame_with(&f, Some(Duration::ZERO));
}

#[test]
fn a_camera_that_stops_sending_is_turned_off_with_a_message() {
    let mut st = Stage::new("");
    st.fail_after = Some(5);
    let (_m, mut sc) = rig("pulled", true, &st, Duration::from_secs(60));
    sc.ask();
    sc.allow();
    poll_until(&mut sc, "the failure", |sc, _| sc.step() == Step::Off);
    assert!(sc.said().unwrap().0.contains("stopped sending") && !sc.said().unwrap().1);
}

#[test]
fn dropping_the_scan_closes_the_camera() {
    let st = Stage::new("");
    let (_m, mut sc) = rig("drop", true, &st, Duration::from_secs(60));
    sc.ask();
    sc.allow();
    assert!(sc.on());
    drop(sc);
    assert!(st.closed.load(Ordering::SeqCst), "the dialog going away takes the camera with it");
}

// ------------------------------------------------------------------------------------------------ what may be filled in

#[test]
fn only_the_shape_of_a_sign_in_code_is_accepted() {
    assert_eq!(validate(CODE.as_bytes(), true).as_deref(), Ok(CODE));
    assert_eq!(validate(format!("  {CODE}\r\n").as_bytes(), true).as_deref(), Ok(CODE), "whitespace is cleaned off");
    // a code without the # is fine for a program that wants no state
    let plain = CODE.replace('#', "-");
    assert!(validate(plain.as_bytes(), true).is_err());
    assert_eq!(validate(plain.as_bytes(), false).as_deref(), Ok(plain.as_str()));
    for bad in [
        "https://claude.example.test/oauth/code/callback?code=abc&state=def",
        "HTTP://EXAMPLE.TEST/?code=AbCdEfGhIjKlMnOpQrStUvWx#state123456",
        "http://x.test",
        "www.example.test/AbCdEfGhIjKlMnOpQrStUvWxYz#0123456789",
        "mailto:someone@example.test",
        "javascript:alert(1)",
        "data:text/plain;base64,QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo#",
        "WIFI:T:WPA;S:home;P:secret;;",
        "short#code",
        "has a space in it #0123456789012345678901234567890",
        "",
    ] {
        let r = validate(bad.as_bytes(), true);
        assert!(r.is_err(), "{bad} must be refused");
        let msg = r.unwrap_err();
        assert!(!msg.contains(bad) || bad.is_empty(), "the message must not repeat the scanned text: {msg}");
    }
    assert!(validate(&[0xFF, 0xFE, 0x00, 0x41], true).is_err(), "not text");
    let long = format!("{}#{}", "a".repeat(1500), "b".repeat(1500));
    assert!(validate(long.as_bytes(), true).is_err(), "far too long for a code");
    // an address is refused even when it has a code in it: never put in the field
    let e = validate(b"https://platform.claude.example.test/oauth/code/callback?code=AbCdEfGhIjKlMnOpQrStUvWxYz&state=0123456789", true).unwrap_err();
    assert!(e.contains("web address") && e.contains("Nothing was opened"), "{e}");
}

#[test]
fn frames_of_both_kinds_are_decoded_and_odd_frames_are_not() {
    let f = frame(CODE, false, 640, 480);
    assert_eq!(decode_frame(&f).as_deref(), Some(CODE.as_bytes()));
    let (w, h, rgba) = f.to_rgba().unwrap();
    let jpeg = crate::camera::jpeg::encode_rgba(w, h, &rgba, 90);
    let m = RawFrame { pix: Pix::Mjpeg, width: w, height: h, data: jpeg };
    assert_eq!(decode_frame(&m).as_deref(), Some(CODE.as_bytes()), "an MJPEG frame");
    assert_eq!(decode_frame(&frame("", false, 640, 480)), None);
    assert_eq!(decode_frame(&RawFrame { pix: Pix::Yuyv, width: 640, height: 480, data: vec![0; 10] }), None, "a short frame");
    assert_eq!(decode_frame(&RawFrame { pix: Pix::Mjpeg, width: 640, height: 480, data: vec![1, 2, 3] }), None, "not a JPEG");
    assert_eq!(decode_frame(&RawFrame { pix: Pix::Yuyv, width: u32::MAX, height: u32::MAX, data: vec![] }), None, "absurd sizes");
}

#[test]
fn the_scan_code_never_logs_audits_or_stores_what_it_reads() {
    // the code of this file, without its tests
    let src = include_str!("agent_login_scan.rs");
    let body = src.split("#[cfg(test)]").next().unwrap();
    for bad in ["println!", "eprintln!", "dbg!", "audit", "log::", "tracing::", "std::fs", "File::", "write(", "clipboard", "open_url", "Command"] {
        assert!(!body.contains(bad), "the scan code must not use {bad}");
    }
}
