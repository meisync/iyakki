//! VOICE and CAMERA in a browser session use the browser's microphone and camera; in a local session the machine's.
//! (No real browser, microphone or camera: the page's side is played by `BrowserMedia::set` and `binary`.)

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

/// A test context whose person has a name of its own: the browser sessions are kept by name in a table the whole process shares, so
/// tests that all ran as "admin" overwrote each other's fake browser when they ran in parallel.
fn cx_own_user(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    cx.session.user = format!("{tag}-person");
    cx
}
use iyakki_agent::browsermedia::{self, BrowserMedia, Kind, Policy, CH_MIC};
use std::sync::Mutex;

struct Registered(String);
impl Drop for Registered {
    fn drop(&mut self) {
        browsermedia::unregister(&self.0);
    }
}

fn browser(cx: &AppCx, p: Policy) -> (Arc<BrowserMedia>, Arc<Mutex<Vec<String>>>, Registered) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    let m = BrowserMedia::new(p, Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))), None);
    browsermedia::register(&cx.session.user, m.clone());
    (m, log, Registered(cx.session.user.clone()))
}

#[test]
fn a_local_session_keeps_the_machines_microphone_and_camera() {
    let cx = cx_own_user("vb-local");
    browsermedia::unregister(&cx.session.user);
    let v = VoiceApp::new(&cx);
    assert_ne!(v.rig.recorders.product(), "web browser");
    assert!(!v.rig.recorders.mics().iter().any(|m| m.id == "browser"));
}

#[test]
fn a_browser_session_dictates_and_records_with_the_browsers_microphone() {
    let cx = cx_own_user("vb-voice");
    let (m, log, _r) = browser(&cx, Policy::allow_all());
    let mut v = VoiceApp::new(&cx).with_recordings_dir(temp_dir("vb-voice-rec"));
    assert_eq!(v.rig.recorders.product(), "web browser");
    v.look(&cx);
    assert_eq!(v.mics.len(), 1, "the browser's microphone is the only one listed");
    // the microphone is off: Record asks the page to show its Allow button and starts nothing
    v.rec_start(&cx);
    assert!(!v.is_recording_to_file());
    assert!(v.status_text().is_some_and(|(t, ok)| t.contains("Allow the microphone in your browser") && !ok), "{:?}", v.status_text());
    assert!(m.take_messages().iter().any(|s| s.contains("media_ask")));
    // the person clicks Allow in the page; now Record works and takes the page's sound
    m.set(Kind::Mic, true).unwrap();
    v.rec_start(&cx);
    assert!(v.is_recording_to_file());
    let mut chunk = vec![CH_MIC];
    chunk.extend(std::iter::repeat_n([0x10u8, 0x10], 800).flatten());
    m.binary(&chunk);
    assert!(log.lock().unwrap().contains(&"browser.media.on mic".to_string()));
}

#[test]
fn a_denied_browser_microphone_is_refused_in_the_tile() {
    let cx = cx_own_user("vb-denied");
    let (m, log, _r) = browser(&cx, Policy::deny_all());
    let mut v = VoiceApp::new(&cx).with_recordings_dir(temp_dir("vb-denied-rec"));
    v.look(&cx);
    v.rec_start(&cx);
    assert!(!v.is_recording_to_file());
    assert!(v.status_text().is_some_and(|(t, _)| t.contains("may not use the microphone")), "{:?}", v.status_text());
    assert!(log.lock().unwrap().contains(&"browser.media.denied mic".to_string()));
    assert!(m.take_messages().iter().all(|s| !s.contains("media_ask")));
}

#[test]
fn the_camera_tile_lists_only_the_browsers_camera_in_a_browser_session() {
    let cx = cx_own_user("vb-cam");
    let (_m, _log, _r) = browser(&cx, Policy::allow_all());
    let c = crate::apps::camera::CameraApp::new(&cx);
    assert_eq!(c.camera_names(), vec!["Browser camera".to_string()]);
}
