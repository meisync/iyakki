//! The sign of a remote session whose browser microphone or camera is live: drawn on every screen, over every app, a click stops it.
use super::tests::*;
use super::*;
use iyakki_agent::browsermedia::{BrowserMedia, Kind, Policy};
use std::sync::Mutex as M;

const LABEL: &str = "Stop the browser's microphone and camera";

/// The registry is process wide and every test signs in as `root`: these tests take turns.
static TURN: M<()> = M::new(());

struct Registered(String, #[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
impl Drop for Registered {
    fn drop(&mut self) {
        iyakki_agent::browsermedia::unregister(&self.0);
    }
}

fn remote(tag: &str) -> (Shell, Gfx, Arc<BrowserMedia>, Arc<M<Vec<String>>>, Registered) {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let log = Arc::new(M::new(Vec::new()));
    let l = log.clone();
    let m = BrowserMedia::new(Policy::allow_all(), Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))), None);
    let user = sh.session.as_ref().unwrap().user.clone();
    iyakki_agent::browsermedia::register(&user, m.clone());
    (sh, Gfx::new(1280, 720), m, log, Registered(user, turn))
}

fn sign(sh: &mut Shell, g: &mut Gfx) -> Option<R> {
    *g = Gfx::new(1280, 720);
    sh.render(g);
    sh.hits().iter().find(|h| h.label == LABEL).map(|h| h.rect)
}

#[test]
fn no_sign_while_nothing_is_live_and_a_local_session_never_has_one() {
    let (mut sh, mut g, m, _, _r) = remote("rs-none");
    assert!(sign(&mut sh, &mut g).is_none());
    m.set(Kind::Mic, true).unwrap();
    assert!(sign(&mut sh, &mut g).is_some());
    m.set(Kind::Mic, false).unwrap();
    assert!(sign(&mut sh, &mut g).is_none(), "gone with the microphone");
    let (mut local, _) = shell_with_users("rs-local");
    login(&mut local, "root", "rootpass12");
    iyakki_agent::browsermedia::unregister("root");
    assert!(sign(&mut local, &mut g).is_none());
}

#[test]
fn the_sign_is_on_every_screen_and_inside_it() {
    let (mut sh, mut g, m, _, _r) = remote("rs-every");
    m.set(Kind::Camera, true).unwrap();
    let r = sign(&mut sh, &mut g).expect("launcher");
    assert!(r.x >= 0.0 && r.y + r.h <= 720.0 && r.x + r.w <= 1280.0, "{r:?}");
    for app in [PApp::Chat, PApp::Browser, PApp::Camera, PApp::Voice, PApp::Settings] {
        sh.open_for_test(app);
        let r = sign(&mut sh, &mut g);
        assert!(r.is_some(), "{app:?}: a tile cannot hide the sign");
        sh.handle(&Event::Key(Key::Esc));
    }
}

#[test]
fn a_click_on_the_sign_switches_both_off_and_audits_it() {
    let (mut sh, mut g, m, log, _r) = remote("rs-click");
    m.set(Kind::Mic, true).unwrap();
    m.set(Kind::Camera, true).unwrap();
    assert!(m.indicator_text().unwrap().contains("microphone and camera"));
    assert!(sign(&mut sh, &mut g).is_some());
    sh.handle(&Event::Tab(914));
    assert!(!m.is_on(Kind::Mic) && !m.is_on(Kind::Camera));
    assert!(m.take_messages().iter().any(|s| s.contains("media_state")), "the page is told, so it stops its devices");
    let l = log.lock().unwrap().clone();
    assert!(l.contains(&"browser.media.stopped shell".to_string()) && l.contains(&"browser.media.off mic".to_string()) && l.contains(&"browser.media.off camera".to_string()), "{l:?}");
    assert!(sign(&mut sh, &mut g).is_none());
    sh.handle(&Event::Tab(914));
    assert_eq!(log.lock().unwrap().iter().filter(|l| *l == "browser.media.stopped shell").count(), 1, "nothing to stop, nothing audited");
}

#[test]
fn the_camera_tile_opens_in_a_browser_session_on_a_machine_without_a_camera() {
    let (mut sh, mut g, _m, _, _r) = remote("rs-cam-tile");
    assert!(!sh.camera.available(), "this machine has no camera");
    assert!(sh.browser_camera_offered() && sh.camera_ready(), "the browser's camera stands in for it");
    sh.render(&mut g);
    sh.set_group(Group::WebMedia);
    let items = sh.group_items();
    let i = items.iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Camera)).unwrap();
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "app:camera", "not greyed, no 'Connect a USB camera'");
    sh.handle(&Event::Key(Key::Esc));
    // a policy that forbids the camera keeps the tile greyed with the usual hint
    let user = sh.session.as_ref().unwrap().user.clone();
    iyakki_agent::browsermedia::register(&user, BrowserMedia::silent(Policy { camera: false, ..Policy::allow_all() }));
    assert!(!sh.browser_camera_offered() && !sh.camera_ready());
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "launcher");
    // and a session on the machine itself (no page) is as before
    iyakki_agent::browsermedia::unregister(&user);
    assert!(!sh.browser_camera_offered());
}
