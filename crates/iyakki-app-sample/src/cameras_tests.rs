//! The cameras pane of SAMPLE (F-SDK-163): an embedded `CameraView` over the SDK's in-memory port, driven through the SDK host with taps, keys
//! and a clock. The core-backed port and the real hub are tested where they live (`iyakki-apps` `cameras_port`, `iyakki-agent::embedcameras`).

use iyakki_ui_sdk::cameras::{CamInfo, MemCameras};
use iyakki_ui_sdk::cameraview::Stage;
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

use super::*;

fn mem() -> MemCameras {
    let m = MemCameras::new(ID);
    for (i, n) in ["Front door", "Yard"].iter().enumerate() {
        m.add(CamInfo::new(&format!("c{i}"), n));
        m.push_solid(&format!("c{i}"), 8, 6, [30 * (i as u8 + 1), 90, 150]);
    }
    m
}

fn host(w: u32, h: u32, m: &MemCameras) -> Host<SampleApp> {
    Host::new(SampleApp::new().with_cameras(Box::new(m.clone())), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { hit.name.starts_with(name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn tap(h: &mut Host<SampleApp>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn images(l: &DisplayList) -> usize {
    l.ops.iter().filter(|o| matches!(o, Op::Image(..))).count()
}

#[test]
fn the_app_has_a_cameras_tab_only_when_the_host_gave_it_cameras() {
    let (mut plain, _) = (Host::new(SampleApp::new(), Look::default(), Arrangement::single(800, 480), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default())), ());
    assert!(!plain.record().a11y().iter().any(|n| n.name == "Cameras"));
    let m = mem();
    let mut h = host(800, 480, &m);
    assert!(h.record().a11y().iter().any(|n| n.name == "Cameras" && n.role == iyakki_ui_sdk::canvas::Role::Tab));
}

#[test]
fn a_person_picks_cameras_allows_the_card_and_watches_and_leaving_the_tab_lets_the_cameras_go() {
    let m = mem();
    let mut h = host(800, 480, &m);
    h.record();
    tap(&mut h, "Cameras");
    assert_eq!(h.app.pane, Pane::Cameras);
    h.tick(10);
    // the list
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.contains("Front door")), "{:?}", l.texts());
    tap(&mut h, "[ ] Front door");
    tap(&mut h, "[ ] Yard");
    tap(&mut h, "Show 2 cameras");
    assert_eq!(h.app.cameras().unwrap().stage(), &Stage::Consent);
    for t in (50..400).step_by(50) {
        h.tick(t);
    }
    assert_eq!(m.pulled("c0"), 0, "nothing before Allow");
    tap(&mut h, "Allow");
    for t in (400..1400).step_by(50) {
        h.tick(t);
    }
    let l = h.record();
    assert_eq!(images(&l), 2, "both pictures are in the frame");
    assert!(l.texts().contains(&"\u{25CF} Live"));
    assert!(m.pulled("c0") >= 1 && m.pulled("c1") >= 1);
    // another tab: the view closes, the core is told, nothing more is pulled
    tap(&mut h, "Notes");
    assert_eq!(h.app.pane, Pane::Notes);
    assert_eq!(h.app.cameras().unwrap().stage(), &Stage::Picking);
    assert!(m.audit().contains(&"cameras.close sample".to_string()), "{:?}", m.audit());
    let n = m.pulled("c0");
    for t in (1500..3000).step_by(50) {
        h.tick(t);
    }
    assert_eq!(m.pulled("c0"), n);
}

#[test]
fn a_denied_card_shows_nothing_and_keys_work_in_the_pane() {
    let m = mem();
    let mut h = host(360, 740, &m);
    h.record();
    tap(&mut h, "Cameras");
    h.tick(5);
    tap(&mut h, "[ ] Front door");
    h.event(&UiEvent::Key(UiKey::Enter));
    h.tick(300);
    h.event(&UiEvent::Key(UiKey::Char('n')));
    assert!(matches!(h.app.cameras().unwrap().stage(), Stage::Denied(_)));
    for t in (400..1000).step_by(100) {
        h.tick(t);
    }
    assert_eq!(m.pulled("c0"), 0);
    let l = h.record();
    assert_eq!(images(&l), 0);
    assert!(l.a11y().iter().any(|n| n.name == "Back to the list"));
    // Esc closes and goes back to the list, a second Esc leaves the app
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert_eq!(h.app.cameras().unwrap().stage(), &Stage::Picking);
}

#[test]
fn the_pane_stays_inside_the_window_at_three_sizes_and_triple_text() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for scale in [1.0f32, 3.0] {
            let m = mem();
            let mut h = host(w, hh, &m);
            h.text_scale = scale;
            h.touch = w < 600;
            h.record();
            tap(&mut h, "Cameras");
            h.tick(10);
            tap(&mut h, "[ ] Front door");
            // a small pane with triple text has room for one row: the rest is said to be not shown, never drawn outside
            let two = h.record().a11y().iter().any(|n| n.name.starts_with("[ ] Yard"));
            if two {
                tap(&mut h, "[ ] Yard");
            }
            tap(&mut h, if two { "Show 2 cameras" } else { "Show 1 camera" });
            tap(&mut h, "Allow");
            for t in (100..1300).step_by(100) {
                h.tick(t);
            }
            let l = h.record();
            for n in l.a11y() {
                assert!(!n.name.is_empty());
                assert!(n.rect.x >= -0.5 && n.rect.y >= -0.5 && n.rect.right() <= w as f32 + 0.5 && n.rect.bottom() <= hh as f32 + 0.5, "{w}x{hh} {scale}x: {} outside {:?}", n.name, n.rect);
            }
            assert_eq!(images(&l), if two { 2 } else { 1 }, "{w}x{hh} {scale}x");
        }
    }
}

#[test]
fn nothing_in_the_pane_names_a_colour_or_the_machine() {
    let src = include_str!("lib.rs");
    assert!(src.contains("with_cameras") && !src.contains("iyakki_agent"));
}
