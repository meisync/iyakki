//! The shell's red sign says when an app shows a camera in its own window (embedded camera views, F-SDK-165): it comes with the view being
//! allowed, open and visible, goes with a hidden or closed view, and the names come from the core.

use super::tests::*;
use super::*;
use iyakki_core_api::cameras::{CamEvent, ViewSpec};
use iyakki_core_api::Core as _;
use iyakki_core_fake::{FakeCam, FakeCore};

fn tick(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

#[test]
fn the_red_sign_follows_the_camera_views_the_core_reports() {
    let (mut sh, _) = shell_with_users("camviews-sign");
    let core = Arc::new(FakeCore::new());
    sh.set_core_for_test(core.clone());
    login(&mut sh, "alice", "alicepass1");
    core.cameras_fake().add(FakeCam::new("front", "Front door", "alice"));
    tick(&mut sh, 30);
    assert!(!sh.cam_views_sign, "no view, no sign");
    let v = core.cameras().open("alice", "sample", &ViewSpec { cameras: vec!["front".into()], ..Default::default() }).unwrap();
    tick(&mut sh, 30);
    assert!(!sh.cam_views_sign, "a view waiting for the card shows nothing, so there is no sign");
    let CamEvent::Approval { call, .. } = core.cameras().events("alice", &v.id).remove(0) else { panic!() };
    core.cameras().decide("alice", &v.id, &call, true).unwrap();
    tick(&mut sh, 30);
    assert!(sh.cam_views_sign, "the camera is on screen: the red sign");
    assert_eq!(core.cameras().indicator_names(), vec!["Front door".to_string()]);
    core.cameras().set_visible("alice", &v.id, false).unwrap();
    tick(&mut sh, 30);
    assert!(!sh.cam_views_sign, "a hidden view pulls nothing and the sign goes");
    core.cameras().set_visible("alice", &v.id, true).unwrap();
    tick(&mut sh, 30);
    assert!(sh.cam_views_sign);
    core.cameras().close("alice", &v.id);
    tick(&mut sh, 30);
    assert!(!sh.cam_views_sign);
}
