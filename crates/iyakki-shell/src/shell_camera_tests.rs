//! The CAMERA tile on the home screen: greyed with a plain hint while there is no camera, usable within a couple of
//! seconds of one being plugged in, in the Web & Media group, and not for guests.

use super::tests::*;
use super::*;
use crate::camera::detect::tests::FakeMachine;
use iyakki_sec::policy::Role;
use crate::camera::Presence;

fn camera_tile_text(sh: &mut Shell, g: &mut Gfx) -> (bool, Vec<String>) {
    sh.set_group(Group::WebMedia);
    sh.render(g);
    let i = sh.group_tiles().iter().position(|t| t.app == PApp::Camera).expect("a CAMERA tile in Web & Media");
    (sh.camera.available(), vec![i.to_string()])
}

#[test]
fn the_camera_tile_is_greyed_without_a_camera_and_wakes_up_when_one_is_plugged_in() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::Camera).count(), 1);
    assert_eq!(Group::of(PApp::Camera), Some(Group::WebMedia));
    assert_eq!(PApp::Camera.id(), "camera");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Camera), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Camera), "guests get no camera, as they get no microphone");
    // the greyed hint fits a tile at the smallest supported size (the drawing shrinks text down to 8 px times the scale)
    for (w, h) in [(1024u32, 768u32), (1280, 720), (1920, 1080)] {
        let mut g = Gfx::new(w, h);
        let s = scale_for(w, h);
        let r = launcher::page_rects(w, h, s, TILES.len(), 0)[0];
        assert!(g.text_width(8.0 * s, Presence::none().tile_hint(), Face::Sans) <= r.w - 24.0 * s, "{w}x{h}");
    }
    let m = FakeMachine::new("shell-tile");
    let (mut sh, _) = shell_with_users("camera-tile");
    sh.set_camera_for_test(m.presence());
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    let (avail, _) = camera_tile_text(&mut sh, &mut g);
    assert!(!avail);
    // greyed: the hint instead of the subtitle, and choosing it explains instead of opening
    sh.render(&mut g);
    let grey = g.rgba().to_vec();
    let items = sh.group_items();
    let i = items.iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Camera)).unwrap();
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "launcher", "a greyed tile does not open");
    assert!(sh.toast_text().is_some_and(|t| t.contains("Connect a USB camera")), "{:?}", sh.toast_text());
    // plugged in: within 60 ticks (two seconds) the tile is live and opens the app
    m.plug_webcam(0, "HD 4MP WEBCAM");
    for _ in 0..61 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.camera.available());
    sh.render(&mut g);
    assert_ne!(g.rgba(), &grey[..], "the tile is drawn differently once a camera is there");
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "app:camera");
    sh.handle(&Event::Key(Key::Esc));
    // pulled out again: greyed again
    m.unplug("video0");
    m.unplug("video1");
    for _ in 0..61 {
        sh.handle(&Event::Tick);
    }
    assert!(!sh.camera.available());
}

/// Screenshots of the tile for the docs: `cargo test -p iyakki-shell --lib camera_tile_screenshots -- --ignored`.
#[test]
#[ignore]
fn camera_tile_screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/media/shots");
    std::fs::create_dir_all(&out).unwrap();
    let m = FakeMachine::new("shell-shots");
    let (mut sh, _) = shell_with_users("camera-shots");
    sh.set_camera_for_test(m.presence());
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.set_group(Group::WebMedia);
    let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::Camera)).unwrap();
    sh.sel = i;
    sh.handle(&Event::Tick);
    sh.render(&mut g);
    g.save_png(&out.join("camera-tile-greyed.png")).unwrap();
    m.plug_webcam(0, "HD 4MP WEBCAM");
    for _ in 0..61 {
        sh.handle(&Event::Tick);
    }
    sh.render(&mut g);
    g.save_png(&out.join("camera-tile-enabled.png")).unwrap();
}

#[test]
fn a_red_sign_shows_on_every_screen_while_the_assistant_uses_the_camera() {
    use iyakki_agent::camtools::CameraBackend;
    let m = FakeMachine::new("shell-sign");
    m.plug_webcam(0, "HD 4MP WEBCAM");
    let (mut sh, _) = shell_with_users("camera-sign");
    let cam = Arc::new(crate::camera::agentcam::AgentCamera::with(m.presence(), Arc::new(crate::camera::fake::FakeOpener::default())).without_settling());
    sh.env.services.camera.set_backend(Some(cam.clone()));
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    let s = scale_for(1280, 720);
    let probe = |g: &Gfx| {
        let (x, y) = ((16.0 * s + 17.0 * s) as u32, (720.0 - 64.0 * s + 16.0 * s) as u32);
        let i = ((y * 1280 + x) * 4) as usize;
        [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2]]
    };
    sh.render(&mut g);
    assert!(probe(&g)[0] < 200 || probe(&g)[1] > 100, "no sign while the camera is idle");
    let d = m.dir().join("v");
    std::fs::create_dir_all(&d).unwrap();
    cam.record_start(&d, 30).unwrap();
    assert!(sh.env.services.camera.in_use().unwrap().contains("recording"));
    for _ in 0..16 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.take_stale(), "the sign asks for a frame");
    for app in ["media", "settings"] {
        assert!(sh.open_for_demo(app));
        sh.render(&mut g);
        let px = probe(&g);
        assert!(px[0] > 200 && px[1] < 100 && px[2] < 110, "the red dot of the sign on {app}: {px:?}");
    }
    cam.record_stop().unwrap();
    assert!(sh.env.services.camera.in_use().is_none());
}
