use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use crate::camera::detect::tests::FakeMachine;
use crate::camera::fake::FakeOpener;
use crate::ui::Key;
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use iyakki_sec::policy::{App as PApp, Role};

const JPG: &[u8] = include_bytes!("../../../iyakki-rtsp/tests/data/test-48x32.jpg");

fn admin_cx(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn usb(tag: &str, yuyv: bool) -> (FakeMachine, Arc<FakeOpener>, Arc<ShellUsb>) {
    let m = FakeMachine::new(tag);
    m.plug_webcam(0, "HD Webcam");
    let op = Arc::new(FakeOpener { yuyv_only: yuyv, ..Default::default() });
    let u = Arc::new(ShellUsb::with(m.presence(), op.clone()));
    (m, op, u)
}

fn frame_of(src: &ShellCameras, id: &str, big: bool) -> api::Frame {
    let t = Instant::now();
    loop {
        if let Some(f) = api::CameraSource::view(src, id, big) {
            return f;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no picture from {id}: {:?}", api::CameraSource::cameras(src).iter().map(|c| c.status.clone()).collect::<Vec<_>>());
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn usb_cameras_come_from_the_shared_camera_module_one_per_physical_camera_and_open_only_when_looked_at() {
    let cx = admin_cx("cams-usb");
    let (_m, op, u) = usb("cams-usb", false);
    let hub = hub_with(&cx, u);
    let src = ShellCameras { hub: hub.clone(), who: who_for(&cx) };
    let list = api::CameraSource::cameras(&src);
    assert_eq!(list.len(), 1, "the picture node and the metadata node are one camera");
    assert_eq!((list[0].name.as_str(), list[0].kind, list[0].address.as_str()), ("HD Webcam", api::Kind::Usb, ""));
    assert!(op.opened.lock().unwrap().is_empty(), "listing a camera does not open it");
    let f = frame_of(&src, &list[0].id, false);
    assert!(f.width <= iyakki_agent::cameras::SMALL_W && f.width > 0);
    assert_eq!(op.opened.lock().unwrap().len(), 1);
    // a snapshot of an MJPEG camera is the camera's own picture, saved as a JPEG that decodes
    let p = api::CameraSource::snapshot(&src, &list[0].id).unwrap();
    assert!(p.contains("/Pictures/camera-hd-webcam-") && p.ends_with(".jpg"), "{p}");
    assert!(iyakki_web::jpeg::decode(&std::fs::read(&p).unwrap()).is_ok());
    hub.stop_all();
}

#[test]
fn a_yuyv_only_camera_is_snapshotted_through_the_encoder() {
    let cx = admin_cx("cams-yuyv");
    let (_m, _op, u) = usb("cams-yuyv", true);
    let src = ShellCameras { hub: hub_with(&cx, u), who: who_for(&cx) };
    let id = api::CameraSource::cameras(&src)[0].id.clone();
    frame_of(&src, &id, false);
    let p = api::CameraSource::snapshot(&src, &id).unwrap();
    let b = iyakki_web::jpeg::decode(&std::fs::read(&p).unwrap()).unwrap();
    assert!(b.width >= 320, "{}", b.width);
    src.hub.stop_all();
}

#[test]
fn the_tile_shows_the_red_band_for_the_usb_camera_and_a_network_camera_goes_live_with_digest_sign_in() {
    let cx = admin_cx("cams-tile");
    let (_m, _op, u) = usb("cams-tile", false);
    let hub = hub_with(&cx, u);
    let cam = FakeCamera::rtsp(Config { user: Some(("admin".into(), "s3cret".into())), digest: true, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() });
    let mut app = CamerasApp::new(&cx);
    let mut g = Gfx::new(1200, 700);
    let t = Instant::now();
    let mut tick = 0u64;
    // add the network camera through the tile's form, as a person would
    app.handle(&Event::Key(Key::Char('a')), &cx);
    for field in [format!("Front door"), format!("rtsp://127.0.0.1:{}/live", cam.port), "admin".to_string(), "s3cret".to_string()] {
        for ch in field.chars() {
            app.handle(&Event::Key(Key::Char(ch)), &cx);
        }
        app.handle(&Event::Key(Key::Tab), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    loop {
        g.clear([9, 11, 20, 255]);
        g.clear_hits();
        app.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, tick);
        tick += 20;
        let live = iyakki_tile_api::cameras::CameraSource::cameras(&ShellCameras { hub: hub.clone(), who: who_for(&cx) }).iter().filter(|c| c.status == api::Status::Live).count();
        if live == 2 {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(12), "not both live: {:?}", iyakki_tile_api::cameras::CameraSource::cameras(&ShellCameras { hub: hub.clone(), who: who_for(&cx) }).iter().map(|c| (c.name.clone(), c.status.clone())).collect::<Vec<_>>());
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(app.inner().privacy_banner_shown(), "the USB camera is on the screen, so the band is");
    assert_eq!(app.inner().shown_ids().len(), 2);
    // the password is in the vault, not in the saved record or the audit log
    let record = std::fs::read_to_string(cx.services.dir().join("cameras/cameras.json")).unwrap();
    assert!(!record.contains("s3cret"), "{record}");
    assert_eq!(&*cx.services.user_get("root", &format!("cam.{}", hub.specs()[0].id)).unwrap(), "s3cret");
    // a snapshot by key says where the file went
    app.handle(&Event::Key(Key::Char('s')), &cx);
    let (note, ok) = app.inner().note().map(|(t, o)| (t.to_string(), o)).unwrap();
    assert!(ok && note.contains("/Pictures/camera-"), "{note}");
    hub.stop_all();
}

#[test]
fn another_person_sees_only_the_cameras_shared_with_them() {
    let cx = admin_cx("cams-access");
    let hub = hub_with(&cx, Arc::new(iyakki_agent::cameras::NoUsb));
    let root = who_for(&cx);
    let mut bob_cx_policy = Role::Operator.template();
    bob_cx_policy.fs_write = vec!["/home/bob".into()];
    let bob = agent::Who { user: "bob".into(), manage: false, usb_ok: bob_cx_policy.allows_app(PApp::Camera), home: PathBuf::from("/home/bob"), may_write: Arc::new(|_| true) };
    // root's camera; bob is not an account in this store, so the list refuses him until he exists
    assert!(hub.save(&root, None, &agent::NewSpec { name: "Hall".into(), address: "rtsp://127.0.0.1:1/x".into(), user: String::new(), allowed: vec!["bob".into()] }, None).unwrap_err().contains("no account"));
    cx.store.lock().unwrap().create_user(&cx.session, "bob", "bobpass1234", Role::Operator).unwrap();
    let id = hub.save(&root, None, &agent::NewSpec { name: "Hall".into(), address: "rtsp://127.0.0.1:1/x".into(), user: String::new(), allowed: vec!["bob".into()] }, None).unwrap();
    assert_eq!(hub.cameras(&bob).len(), 1);
    hub.save(&root, Some(&id), &agent::NewSpec { name: "Hall".into(), address: "rtsp://127.0.0.1:1/x".into(), user: String::new(), allowed: vec![] }, None).unwrap();
    assert!(hub.cameras(&bob).is_empty());
    assert!(hub.view(&bob, &id, false).is_err());
    hub.stop_all();
}

#[test]
fn the_tile_the_policy_and_the_assistants_tools_are_wired() {
    assert_eq!((PApp::Cameras.id(), PApp::Cameras.label()), ("cameras", "CAMERAS".to_string()));
    assert!(PApp::ALL.contains(&PApp::Cameras));
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::Cameras), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::Cameras), "guests watch no cameras");
    let cx = admin_cx("cams-tools");
    struct NoTools;
    impl iyakki_agent::agent::Tools for NoTools {
        fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
            vec![]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Err("no".into())
        }
        fn describe(&self, n: &str, _: &serde_json::Value) -> String {
            n.into()
        }
    }
    let t = tools_for(&cx, Arc::new(NoTools));
    let names: Vec<String> = t.defs().into_iter().map(|d| d.spec.name).collect();
    for n in iyakki_agent::camerastools::TOOLS {
        assert!(names.contains(&n.to_string()), "{n}");
    }
}

#[test]
fn recording_through_the_shell_keeps_its_key_in_the_owners_vault_shows_the_red_sign_and_the_sign_stops_it() {
    let cx = admin_cx("cams-rec");
    let cam = FakeCamera::rtsp(Config { user: Some(("admin".into(), "pw-12345".into())), digest: true, frames: vec![JPG.to_vec()], interval_ms: 20, ..Default::default() });
    let hub = hub_with(&cx, Arc::new(iyakki_agent::cameras::NoUsb));
    let src = ShellCameras { hub: hub.clone(), who: who_for(&cx) };
    let id = api::CameraSource::save(&src, None, &api::NewCam { name: "Front door".into(), address: format!("rtsp://127.0.0.1:{}/live", cam.port), user: "admin".into(), allowed: vec![] }, Some("pw-12345")).unwrap();
    let data = cx.services.dir().to_path_buf();
    // off by default: no mark, no sign, no key, nothing contacted
    assert!(api::CameraSource::recording_ids(&src).is_empty() && !rec_sign_changed(&data));
    assert!(cx.services.user_get("root", &format!("camrec.{id}")).is_err());
    let info = api::CameraSource::rec_info(&src, &id).unwrap();
    assert_eq!((info.mode, info.recording, info.owner), (api::RecMode::Off, false, true));
    assert!(info.key_note.contains("lose your vault"));
    // on: the key goes to the vault, the mark and the sign appear
    api::CameraSource::rec_set(&src, &id, &api::RecChange { mode: Some(api::RecMode::Continuous), ..Default::default() }).unwrap();
    let key = cx.services.user_get("root", &format!("camrec.{id}")).unwrap();
    assert_eq!(key.len(), 64, "a 256-bit key as hex, in the owner's vault");
    assert_eq!(api::CameraSource::recording_ids(&src), vec![id.clone()]);
    assert_eq!(iyakki_agent::camrec::active_names(&data), vec!["Front door".to_string()]);
    assert!(rec_sign_changed(&data), "the shell redraws when the sign appears");
    assert!(!rec_sign_changed(&data), "and only then");
    let settings = std::fs::read_to_string(data.join("cameras/recordings/settings.json")).unwrap();
    assert!(!settings.contains(key.as_str()) && !settings.contains("pw-12345"), "no key or password in the setting file");
    // the red sign's click turns every recording of the person off
    assert_eq!(iyakki_agent::camrec::stop_all_for(&data, "root"), 1);
    assert!(api::CameraSource::recording_ids(&src).is_empty() && rec_sign_changed(&data));
    assert_eq!(api::CameraSource::rec_info(&src, &id).unwrap().mode, api::RecMode::Off);
    // somebody else's click stops nothing, and a refused change says why
    api::CameraSource::rec_set(&src, &id, &api::RecChange { mode: Some(api::RecMode::Continuous), ..Default::default() }).unwrap();
    assert_eq!(iyakki_agent::camrec::stop_all_for(&data, "nobody"), 0);
    assert!(api::CameraSource::rec_set(&src, &id, &api::RecChange { windows: Some(vec!["whenever".into()]), ..Default::default() }).is_err());
    api::CameraSource::rec_set(&src, &id, &api::RecChange { mode: Some(api::RecMode::Off), ..Default::default() }).unwrap();
    iyakki_agent::camrec::for_hub(&hub).shutdown();
    hub.stop_all();
}

#[test]
fn motion_through_the_shell_keeps_the_key_in_the_owners_vault_shows_the_red_sign_and_the_sign_stops_it() {
    use iyakki_agent::camdetect::watch::AlertSink;
    let cx = admin_cx("cams-motion");
    let hub = hub_with(&cx, Arc::new(iyakki_agent::cameras::NoUsb));
    let motion = motion_for(&cx);
    assert!(motion.has_sink(), "alerts go to the inbox");
    let src = ShellCameras { hub: hub.clone(), who: who_for(&cx) };
    let id = api::CameraSource::save(&src, None, &api::NewCam { name: "Front door".into(), address: "rtsp://127.0.0.1:9/live".into(), user: "admin".into(), allowed: vec![] }, Some("pw-12345")).unwrap();
    let data = cx.services.dir().to_path_buf();
    // off by default: no key, no sign, no mark
    assert!(cx.services.user_get("root", &format!("camrec.{id}")).is_err());
    assert!(api::CameraSource::motion_ids(&src).is_empty() && sign_names(&data).is_empty());
    let info = api::CameraSource::motion_info(&src, &id).unwrap();
    assert_eq!((info.mode, info.owner, info.alerts_on), (api::MotionMode::Off, true, false));
    assert!(info.person_note.contains("not available"), "{}", info.person_note);
    // shapes: a good mask and zone are kept; a bow tie is refused in words
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    api::CameraSource::motion_shape_set(&src, &id, true, &api::ShapeInfo { name: "Street".into(), include: false, sensitivity: 5, points: rect(0.5, 0.0, 1.0, 0.5) }).unwrap();
    api::CameraSource::motion_shape_set(&src, &id, false, &api::ShapeInfo { name: "Porch".into(), include: true, sensitivity: 6, points: rect(0.0, 0.5, 0.5, 1.0) }).unwrap();
    let bow = api::ShapeInfo { name: "Bow".into(), include: true, sensitivity: 5, points: vec![(0.1, 0.1), (0.9, 0.9), (0.9, 0.1), (0.1, 0.9)] };
    assert!(api::CameraSource::motion_shape_set(&src, &id, false, &bow).unwrap_err().contains("crosses itself"));
    let info = api::CameraSource::motion_info(&src, &id).unwrap();
    assert_eq!((info.masks.len(), info.zones.len()), (1, 1));
    // on: the key goes to the vault, the red sign says "motion watch"
    api::CameraSource::motion_set(&src, &id, &api::MotionChange { mode: Some(api::MotionMode::Always), alerts_on: Some(true), ..Default::default() }).unwrap();
    assert_eq!(cx.services.user_get("root", &format!("camrec.{id}")).unwrap().len(), 64);
    assert_eq!(sign_names(&data), vec!["Front door (motion watch)".to_string()]);
    assert!(rec_sign_changed(&data) && !rec_sign_changed(&data));
    let settings = std::fs::read_to_string(data.join("cameras/motion/settings.json")).unwrap();
    assert!(!settings.contains("pw-12345") && settings.contains("Porch"));
    // the notice sink is the person's inbox, with a link back to the camera
    iyakki_agent::camdetect::watch::InboxSink(cx.services.clone()).notify("root", &id, &iyakki_agent::camdetect::alerts::Notice { title: "Motion at Front door".into(), body: "02:14 (Porch). Open CAMERAS to see it.".into(), count: 1, grouped: false, event_ms: 1, thumbnail: false }, None);
    let note = cx.services.inbox.list("root").into_iter().find(|n| n.title == "Motion at Front door").expect("a notice in the inbox");
    assert_eq!(note.link.as_ref().map(|l| (l.app.as_str(), l.id.as_str())), Some(("cameras", id.as_str())));
    // the assistant's motion tools are offered
    struct NoTools;
    impl iyakki_agent::agent::Tools for NoTools {
        fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
            vec![]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Err("no".into())
        }
        fn describe(&self, n: &str, _: &serde_json::Value) -> String {
            n.into()
        }
    }
    let names: Vec<String> = tools_for(&cx, Arc::new(NoTools)).defs().into_iter().map(|d| d.spec.name).collect();
    for n in ["camera_motion_status", "camera_motion_set", "camera_events_list", "camera_zone_set"] {
        assert!(names.contains(&n.to_string()), "{n}");
    }
    // the red sign's click switches the all-the-time watch off; somebody else's click stops nothing
    assert_eq!(stop_all_signs(&data, "nobody"), 0);
    assert_eq!(stop_all_signs(&data, "root"), 1);
    assert!(sign_names(&data).is_empty() && rec_sign_changed(&data));
    assert_eq!(api::CameraSource::motion_info(&src, &id).unwrap().mode, api::MotionMode::Off);
    iyakki_agent::camdetect::watch::for_hub(&hub).shutdown();
    hub.stop_all();
}
