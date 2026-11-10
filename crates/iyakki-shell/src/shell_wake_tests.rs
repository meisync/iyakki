//! The wake phrase on every screen: off by default (nothing opened, no sign), a red sign on every screen while the microphone is open
//! to the spotter, a click on it switches the wake phrase off (audited), a Voice tile takes the microphone, and a detection opens the
//! Voice tile. A fake microphone and a fake spotter stand in for the machine's.
use super::tests::*;
use super::*;
use iyakki_agent::wakelisten as wl;
use std::sync::Mutex as M;

const LABEL: &str = "Switch the wake phrase off";

/// The wake request flag is process wide: these tests take turns.
static TURN: M<()> = M::new(());

#[derive(Default)]
struct MicLog {
    opens: u32,
    open: bool,
    frames: Vec<Vec<i16>>,
}
#[derive(Clone, Default)]
struct FakeMic(Arc<M<MicLog>>);
impl wl::Mic for FakeMic {
    fn open(&mut self) -> Result<(), String> {
        let mut l = self.0.lock().unwrap();
        l.opens += 1;
        l.open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.0.lock().unwrap().open
    }
    fn drain(&mut self) -> Vec<i16> {
        let mut l = self.0.lock().unwrap();
        if l.frames.is_empty() { Vec::new() } else { l.frames.remove(0) }
    }
    fn alive(&mut self) -> bool {
        true
    }
    fn close(&mut self) {
        self.0.lock().unwrap().open = false;
    }
}

fn setup(tag: &str) -> (Shell, Gfx, FakeMic, std::sync::MutexGuard<'static, ()>) {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let mic = FakeMic::default();
    let m2 = mic.clone();
    sh.wake.listener_maker = Some(Arc::new(move || wl::testing::listener(Box::new(m2.clone()))));
    let _ = wl::take_session_request();
    (sh, Gfx::new(1280, 720), mic, turn)
}

fn put_model(sh: &Shell) {
    wl::testing::write_model(sh.env.services.dir());
}

fn opt_in(sh: &Shell) {
    sh.env.services.settings.lock().unwrap().update(|s| s.voice.my_on = true).unwrap();
    wl::set_opt_in(&sh.env.services, true).unwrap();
}

fn sign(sh: &mut Shell, g: &mut Gfx) -> Option<R> {
    *g = Gfx::new(1280, 720);
    sh.render(g);
    sh.hits().iter().find(|h| h.label == LABEL).map(|h| h.rect)
}

/// Tick until `ok` holds (the listener runs on its own thread).
fn until(sh: &mut Shell, ok: impl Fn(&Shell) -> bool) -> bool {
    for _ in 0..300 {
        sh.handle(&Event::Tick);
        if ok(sh) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

#[test]
fn off_by_default_nothing_is_opened_and_no_sign_shows() {
    let (mut sh, mut g, mic, _t) = setup("wk-off");
    put_model(&sh);
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(sign(&mut sh, &mut g).is_none());
    assert_eq!(mic.0.lock().unwrap().opens, 0, "the microphone was never opened");
}

#[test]
fn opted_in_without_a_model_nothing_listens_and_the_switch_refuses_with_the_reason() {
    let (mut sh, mut g, mic, _t) = setup("wk-nomodel");
    sh.env.services.settings.lock().unwrap().update(|s| s.voice.my_on = true).unwrap();
    let why = wl::set_opt_in(&sh.env.services, true).unwrap_err();
    assert!(why.contains("no wake phrase model yet"), "{why}");
    assert!(!sh.env.services.voice_settings().my_wake, "the switch stays off");
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(sign(&mut sh, &mut g).is_none() && mic.0.lock().unwrap().opens == 0);
}

#[test]
fn the_sign_is_on_every_screen_while_listening_and_gone_in_the_voice_tile_which_has_the_microphone() {
    let (mut sh, mut g, mic, _t) = setup("wk-sign");
    put_model(&sh);
    opt_in(&sh);
    assert!(until(&mut sh, |s| s.wake.sign), "the microphone opens for the spotter");
    let r = sign(&mut sh, &mut g).expect("launcher");
    assert!(r.x >= 0.0 && r.y + r.h <= 720.0 && r.x + r.w <= 1280.0, "{r:?}");
    for app in [PApp::Chat, PApp::Browser, PApp::Camera, PApp::Settings] {
        sh.open_for_test(app);
        assert!(until(&mut sh, |s| s.wake.sign), "{app:?}");
        assert!(sign(&mut sh, &mut g).is_some(), "{app:?}: a tile cannot hide the sign");
        sh.handle(&Event::Key(Key::Esc));
    }
    sh.open_for_test(PApp::Voice);
    assert!(until(&mut sh, |s| !s.wake.sign), "the Voice tile has the microphone");
    assert!(!mic.0.lock().unwrap().open, "and the spotter's microphone is really closed");
    assert!(sign(&mut sh, &mut g).is_none());
}

#[test]
fn muting_closes_the_microphone_and_the_sign() {
    let (mut sh, mut g, mic, _t) = setup("wk-mute");
    put_model(&sh);
    opt_in(&sh);
    assert!(until(&mut sh, |s| s.wake.sign));
    sh.env.services.settings.lock().unwrap().update(|s| s.voice.my_muted = true).unwrap();
    assert!(until(&mut sh, |s| !s.wake.sign));
    assert!(!mic.0.lock().unwrap().open && sign(&mut sh, &mut g).is_none());
}

#[test]
fn a_click_on_the_sign_switches_the_wake_phrase_off_and_audits_it() {
    let (mut sh, mut g, mic, _t) = setup("wk-click");
    put_model(&sh);
    opt_in(&sh);
    assert!(until(&mut sh, |s| s.wake.sign));
    assert!(sign(&mut sh, &mut g).is_some());
    sh.handle(&Event::Tab(916));
    assert!(!sh.env.services.voice_settings().my_wake);
    assert!(sign(&mut sh, &mut g).is_none() && !mic.0.lock().unwrap().open);
    assert!(audit_text(&sh).contains("wake-off"), "the audit log has the off line");
}

#[test]
fn signing_out_stops_the_listener() {
    let (mut sh, _g, mic, _t) = setup("wk-signout");
    put_model(&sh);
    opt_in(&sh);
    assert!(until(&mut sh, |s| s.wake.sign));
    sh.session = None;
    assert!(until(&mut sh, |s| !s.wake.sign));
    assert!(!mic.0.lock().unwrap().open, "nothing listens at the sign-in screen");
}

#[test]
fn a_detection_opens_the_voice_tile_and_asks_for_the_session_once() {
    let (mut sh, _g, mic, _t) = setup("wk-detect");
    put_model(&sh);
    opt_in(&sh);
    // a harmless record command: the tap that follows must never open the machine's real microphone
    sh.env.services.settings.lock().unwrap().update(|s| s.voice.record_cmd = "cat /dev/zero".into()).unwrap();
    assert!(until(&mut sh, |s| s.wake.sign));
    mic.0.lock().unwrap().frames.push(vec![32000; 1600]);
    assert!(until(&mut sh, |s| s.screen_name() == "app:voice"), "the Voice tile opened for the detection");
    assert!(audit_text(&sh).contains("voice.wake"), "the detection is audited (no sound)");
    // the tile consumed the request on its first beat: a later tile does not tap by itself
    for _ in 0..10 {
        sh.handle(&Event::Tick);
    }
    assert!(!wl::take_session_request(), "one detection, one request, taken once");
}
