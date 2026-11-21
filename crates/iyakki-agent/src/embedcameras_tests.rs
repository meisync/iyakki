//! The embedded camera views over the real hub and the in-house fake RTSP camera on the loopback address (real JPEG frames through the real
//! client and decoder). No real camera was used.

use super::*;
use crate::camdetect::watch::{MotionEnv, ShapeKind};
use crate::cameras::{CamSecrets, Env, NewSpec, NoH264, NoUsb};
use crate::camdetect::geom::Pt;
use crate::clock::Zone;
use iyakki_core_api::cameras::{CamAction, CamEvent};
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const JPG: &[u8] = include_bytes!("../../iyakki-rtsp/tests/data/test-48x32.jpg");

#[derive(Default)]
struct Vault {
    items: Mutex<HashMap<(String, String), String>>,
}

impl CamSecrets for Vault {
    fn get(&self, owner: &str, name: &str) -> Result<zeroize::Zeroizing<String>, String> {
        self.items.lock().unwrap().get(&(owner.into(), name.into())).map(|v| zeroize::Zeroizing::new(v.clone())).ok_or_else(|| "no such secret".into())
    }
    fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), String> {
        self.items.lock().unwrap().insert((owner.into(), name.into()), value.into());
        Ok(())
    }
    fn delete(&self, owner: &str, name: &str) -> Result<(), String> {
        self.items.lock().unwrap().remove(&(owner.into(), name.into()));
        Ok(())
    }
    fn has(&self, owner: &str, name: &str) -> bool {
        self.items.lock().unwrap().contains_key(&(owner.into(), name.into()))
    }
}

struct Rig {
    cams: Arc<AgentCameras>,
    hub: Arc<Hub>,
    audit: Arc<Mutex<Vec<String>>>,
    saved: Arc<Mutex<Vec<PathBuf>>>,
    dir: PathBuf,
    fake: FakeCamera,
    id: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.hub.stop_all();
    }
}

fn who_in(dir: &Path, user: &str) -> Who {
    Who { user: user.into(), manage: user == "root", usb_ok: false, home: dir.join("home").join(user), may_write: Arc::new(|_| true) }
}

fn rig(tag: &str, motion: bool) -> (Rig, Option<Arc<MotionService>>) {
    let dir = std::fs::canonicalize(crate::testdir::unique("embedcam", tag)).unwrap();
    let audit = Arc::new(Mutex::new(vec![]));
    let saved = Arc::new(Mutex::new(vec![]));
    let (a2, s2) = (audit.clone(), saved.clone());
    let hub = Arc::new(Hub::new(Env {
        data: dir.join("data"),
        secrets: Arc::new(Vault::default()),
        people: Arc::new(|| vec!["amy".into(), "bob".into(), "root".into(), "eve".into()]),
        usb: Arc::new(NoUsb),
        h264: Arc::new(NoH264),
        encode: Arc::new(|_, _, _| JPG.to_vec()),
        after_save: Arc::new(move |p| s2.lock().unwrap().push(p.to_path_buf())),
        audit: Arc::new(move |u, e, d| a2.lock().unwrap().push(format!("{u}|{e}|{d}"))),
        backoff_ms: 30,
        idle: Duration::from_millis(400),
    }));
    let fake = FakeCamera::rtsp(Config { user: Some(("admin".into(), "pw-12345".into())), digest: true, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() });
    let d2 = dir.clone();
    let who_of: WhoFn = Arc::new(move |u| who_in(&d2, u));
    let amy = who_in(&dir, "amy");
    let id = hub.save(&amy, None, &NewSpec { name: "Front door".into(), address: format!("rtsp://127.0.0.1:{}/live", fake.port), user: "admin".into(), allowed: vec!["bob".into()] }, Some("pw-12345")).unwrap();
    // saving does not start a stream; the fake camera has seen nobody yet
    let now = Arc::new(AtomicU64::new(1_700_000_000_000));
    let svc = motion.then(|| {
        Arc::new(MotionService::new(
            hub.clone(),
            MotionEnv { now_ms: Arc::new(move || now.load(Ordering::SeqCst)), zone: Arc::new(Zone::utc), recording: Arc::new(|_| false), on_active: Arc::new(|_| {}), key_every: Duration::ZERO, retention_every: Duration::from_secs(3600) },
        ))
    });
    let cams = Arc::new(AgentCameras::new(hub.clone(), who_of, svc.clone(), None));
    // no test asks a real address whether it can move
    cams.set_ptz_probe(Arc::new(|_, _, _, _| Ok(None)));
    (Rig { cams, hub, audit, saved, dir, fake, id }, svc)
}

fn spec(ids: &[&str]) -> ViewSpec {
    ViewSpec { cameras: ids.iter().map(|s| s.to_string()).collect(), ..ViewSpec::default() }
}

/// Open a view and allow its card, as the person would.
fn open_allowed(r: &Rig, user: &str, app: &str, ids: &[&str]) -> String {
    let v = r.cams.open(user, app, &spec(ids)).unwrap();
    let ev = r.cams.events(user, &v.id);
    let Some(CamEvent::Approval { call, action: CamAction::View, .. }) = ev.into_iter().find(|e| matches!(e, CamEvent::Approval { .. })) else { panic!("no card") };
    r.cams.decide(user, &v.id, &call, true).unwrap();
    v.id
}

fn wait_frame(r: &Rig, user: &str, view: &str, cam: &str, after: u64, format: FrameFormat) -> CamFrame {
    let t = Instant::now();
    loop {
        if let Some(f) = r.cams.frame(user, view, cam, after, format).unwrap() {
            return f;
        }
        assert!(t.elapsed() < Duration::from_secs(8), "no picture; {:?}", r.cams.list(user, "app").first().map(|c| (c.status, c.detail.clone())));
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn files_under(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn a_person_sees_only_the_cameras_they_may_watch_and_a_manager_who_may_not_watch_sees_nothing() {
    let (r, _) = rig("vis", false);
    assert_eq!(r.cams.list("amy", "app").iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Front door"]);
    assert_eq!(r.cams.list("bob", "app").len(), 1, "on the camera's list");
    assert!(r.cams.list("eve", "app").is_empty(), "not on the list: nothing");
    assert!(r.cams.list("root", "app").is_empty(), "an administrator manages the camera but is not given the picture");
    assert_eq!(r.cams.open("eve", "app", &spec(&[&r.id])).unwrap_err(), NOT_YOURS);
    assert_eq!(r.cams.open("eve", "app", &spec(&["no-such"])).unwrap_err(), NOT_YOURS, "the same words for a camera that is not there");
    assert!(r.cams.open("root", "app", &spec(&[&r.id])).is_err());
    assert!(r.cams.list("amy", "app").iter().all(|c| !c.ptz), "no camera source of the hub has pan and tilt");
    assert!(r.fake.requests().is_empty(), "listing and refusing touch no camera");
    assert_eq!(r.hub.running(), 0);
}

#[test]
fn no_stream_starts_before_the_card_is_allowed_and_a_denied_card_shows_nothing() {
    let (r, _) = rig("consent", false);
    let v = r.cams.open("amy", "app", &spec(&[&r.id])).unwrap();
    let st = r.cams.state("amy", &v.id).unwrap();
    assert!(matches!(st.phase, ViewPhase::AwaitingConsent { .. }) && !st.indicator);
    for _ in 0..5 {
        assert!(r.cams.frame("amy", &v.id, &r.id, 0, FrameFormat::Rgba).unwrap().is_none());
    }
    assert_eq!(r.hub.running(), 0, "asking for pictures before consent starts no stream");
    let ev = r.cams.events("amy", &v.id);
    let CamEvent::Approval { call, summary, detail, .. } = &ev[0] else { panic!("{ev:?}") };
    assert!(summary.contains("app wants to show 1 camera: Front door") && detail.contains("Nothing is recorded"));
    r.cams.decide("amy", &v.id, call, false).unwrap();
    assert!(matches!(r.cams.state("amy", &v.id).unwrap().phase, ViewPhase::Denied { .. }));
    assert!(r.cams.frame("amy", &v.id, &r.id, 0, FrameFormat::Rgba).unwrap().is_none());
    assert_eq!(r.hub.running(), 0);
    assert!(r.fake.requests().is_empty(), "the camera was never contacted");
    let a = r.audit.lock().unwrap().clone();
    assert!(a.contains(&"amy|cameras.consent|app denied".to_string()), "{a:?}");
}

#[test]
fn an_allowed_view_shows_real_frames_from_the_loopback_camera_in_both_formats_and_the_sign_is_on() {
    let (r, _) = rig("live", false);
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    let st = r.cams.state("amy", &view).unwrap();
    assert!(st.indicator && st.phase == ViewPhase::Open);
    assert_eq!(viewing_names_of(&r), vec!["Front door".to_string()]);
    let f = wait_frame(&r, "amy", &view, &r.id, 0, FrameFormat::Rgba);
    assert_eq!((f.width, f.height, f.data.len()), (48, 32, 48 * 32 * 4));
    assert!(f.data.chunks_exact(4).all(|p| p[3] == 255), "opaque pixels from the real decoder");
    let j = wait_frame(&r, "amy", &view, &r.id, f.seq, FrameFormat::Jpeg);
    assert!(j.seq > f.seq && j.data.starts_with(&[0xFF, 0xD8]) && j.format == FrameFormat::Jpeg);
    let info = r.cams.list("amy", "app");
    assert_eq!(info[0].status, CamStatus::Live);
    // bob is on the list, watches the same camera in a view of his own; amy's view is not his
    let bobs = open_allowed(&r, "bob", "app", &[&r.id]);
    assert!(r.cams.state("bob", &view).is_err() && r.cams.state("amy", &bobs).is_err());
    wait_frame(&r, "bob", &bobs, &r.id, 0, FrameFormat::Rgba);
    r.cams.close("amy", &view);
    r.cams.close("bob", &bobs);
    assert!(r.cams.state("amy", &view).is_err());
}

fn viewing_names_of(r: &Rig) -> Vec<String> {
    r.cams.indicator_names()
}

#[test]
fn a_hidden_or_closed_view_lets_the_camera_go_after_the_idle_time_and_a_shown_one_keeps_it() {
    let (r, _) = rig("hidden", false);
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    wait_frame(&r, "amy", &view, &r.id, 0, FrameFormat::Rgba);
    assert_eq!(r.hub.running(), 1);
    r.cams.set_visible("amy", &view, false).unwrap();
    assert!(!r.cams.state("amy", &view).unwrap().indicator, "hidden: the sign is off");
    assert!(viewing_names_of(&r).is_empty());
    assert!(r.cams.frame("amy", &view, &r.id, 0, FrameFormat::Rgba).unwrap().is_none(), "hidden: nothing is pulled");
    let t = Instant::now();
    while r.hub.running() > 0 {
        assert!(t.elapsed() < Duration::from_secs(6), "the stream stayed open with nobody looking");
        std::thread::sleep(Duration::from_millis(50));
    }
    r.cams.set_visible("amy", &view, true).unwrap();
    wait_frame(&r, "amy", &view, &r.id, 0, FrameFormat::Rgba);
    assert_eq!(r.hub.running(), 1, "visible again: the stream is back");
}

#[test]
fn a_view_writes_nothing_and_a_snapshot_asks_every_time_and_saves_one_jpeg_in_the_persons_pictures_only() {
    let (r, _) = rig("snap", false);
    let before = files_under(&r.dir);
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    let f = wait_frame(&r, "amy", &view, &r.id, 0, FrameFormat::Rgba);
    for _ in 0..5 {
        let _ = r.cams.frame("amy", &view, &r.id, f.seq, FrameFormat::Jpeg);
        std::thread::sleep(Duration::from_millis(30));
    }
    r.cams.events("amy", &view);
    let during: Vec<PathBuf> = files_under(&r.dir).into_iter().filter(|p| !before.contains(p)).collect();
    assert!(during.iter().all(|p| !p.to_string_lossy().ends_with(".jpg") && !p.starts_with(r.dir.join("home"))), "watching wrote no picture: {during:?}");
    assert!(r.saved.lock().unwrap().is_empty());
    // the first snapshot: denied, nothing is written
    let call = r.cams.snapshot("amy", &view, &r.id).unwrap();
    r.cams.decide("amy", &view, &call, false).unwrap();
    assert!(!r.dir.join("home/amy/Pictures").exists());
    // the second: allowed, one file in Pictures
    let call = r.cams.snapshot("amy", &view, &r.id).unwrap();
    assert!(!r.dir.join("home/amy/Pictures").exists(), "nothing before the card is allowed");
    r.cams.decide("amy", &view, &call, true).unwrap();
    let ev = r.cams.events("amy", &view);
    let Some(CamEvent::Snapshot { file, folder, .. }) = ev.iter().find(|e| matches!(e, CamEvent::Snapshot { .. })) else { panic!("{ev:?}") };
    assert_eq!(folder, "Pictures");
    assert!(file.starts_with("camera-front-door-") && !file.contains('/'), "a name, not a path: {file}");
    let saved = files_under(&r.dir.join("home"));
    assert_eq!(saved.len(), 1);
    assert!(saved[0].starts_with(r.dir.join("home/amy/Pictures")));
    // a view needs its own allowed card before a snapshot is even asked for
    let v2 = r.cams.open("amy", "app", &spec(&[&r.id])).unwrap();
    assert!(r.cams.snapshot("amy", &v2.id, &r.id).unwrap_err().contains("not open"));
    // an account that may not write there gets a sentence
    let (r2, _) = rig("snapw", false);
    let d3 = r2.dir.clone();
    let deny: WhoFn = Arc::new(move |u| Who { may_write: Arc::new(|_| false), ..who_in(&d3, u) });
    let cams2 = AgentCameras::new(r2.hub.clone(), deny, None, None);
    let v = cams2.open("amy", "app", &spec(&[&r2.id])).unwrap();
    let CamEvent::Approval { call, .. } = cams2.events("amy", &v.id).remove(0) else { panic!() };
    cams2.decide("amy", &v.id, &call, true).unwrap();
    let t = Instant::now();
    while cams2.frame("amy", &v.id, &r2.id, 0, FrameFormat::Rgba).unwrap().is_none() {
        assert!(t.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(20));
    }
    let c = cams2.snapshot("amy", &v.id, &r2.id).unwrap();
    cams2.decide("amy", &v.id, &c, true).unwrap();
    assert!(cams2.events("amy", &v.id).iter().any(|e| matches!(e, CamEvent::Failed { error, .. } if error.contains("may not save"))));
    assert!(!r2.dir.join("home/amy/Pictures").exists());
}

#[test]
fn the_audit_has_names_only_and_pan_tilt_zoom_is_refused_in_words() {
    let (r, _) = rig("audit", false);
    // the lines the person's own adding of the camera wrote (the hub's `camera.add`) are the hub's, not the view's
    let base = r.audit.lock().unwrap().len();
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    wait_frame(&r, "amy", &view, &r.id, 0, FrameFormat::Rgba);
    let e = r.cams.ptz("amy", &view, &r.id, PtzCommand::Move { pan: 1.0, tilt: 0.0, zoom: 0.0 }).unwrap_err();
    assert!(e.contains("no pan, tilt or zoom"), "{e}");
    assert!(r.cams.ptz("amy", &view, "other", PtzCommand::Stop).unwrap_err() == NOT_YOURS);
    r.cams.close("amy", &view);
    let a: Vec<String> = r.audit.lock().unwrap()[base..].to_vec();
    assert!(a.contains(&"amy|cameras.open|app: Front door".to_string()), "{a:?}");
    assert!(a.contains(&"amy|cameras.consent|app allowed".to_string()) && a.contains(&"amy|cameras.close|app".to_string()));
    let text = a.join("\n");
    assert!(!text.contains("pw-12345") && !text.contains("127.0.0.1") && !text.contains("rtsp"), "no address and no password in the audit: {text}");
    assert!(r.fake.requests().iter().all(|l| !l.contains("pw-12345")));
}

#[test]
fn limits_are_sentences_and_a_camera_taken_off_the_list_stops_the_view() {
    let (r, _) = rig("limits", false);
    let a = r.cams.open("bob", "app", &spec(&[&r.id])).unwrap();
    let _b = r.cams.open("bob", "app", &spec(&[&r.id])).unwrap();
    assert!(r.cams.open("bob", "app", &spec(&[&r.id])).unwrap_err().contains("this app may have 2"));
    r.cams.open("bob", "second", &spec(&[&r.id])).unwrap();
    r.cams.open("bob", "third", &spec(&[&r.id])).unwrap();
    assert!(r.cams.open("bob", "fourth", &spec(&[&r.id])).unwrap_err().contains("4 camera views"));
    assert!(r.cams.open("bob", "x", &spec(&["a", "b", "c", "d", "e"])).unwrap_err().contains("at most 4"));
    let mut audio = spec(&[&r.id]);
    audio.audio = true;
    assert!(r.cams.open("amy", "app", &audio).unwrap_err().contains("no sound"));
    r.cams.close("bob", &a.id);
    // amy takes bob off the list: bob's allowed view stops getting pictures
    let (r, _) = rig("revoke", false);
    let view = open_allowed(&r, "bob", "app", &[&r.id]);
    wait_frame(&r, "bob", &view, &r.id, 0, FrameFormat::Rgba);
    r.hub.save(&who_in(&r.dir, "amy"), Some(&r.id), &NewSpec { name: "Front door".into(), address: format!("rtsp://127.0.0.1:{}/live", r.fake.port), user: "admin".into(), allowed: vec![] }, None).unwrap();
    assert!(r.cams.frame("bob", &view, &r.id, 0, FrameFormat::Rgba).unwrap_err().contains("not on its list"));
    assert!(r.cams.list("bob", "app").is_empty());
}

#[test]
fn zones_and_motion_marks_are_the_owners_and_read_only() {
    let (r, svc) = rig("overlay", true);
    let svc = svc.unwrap();
    let amy = who_in(&r.dir, "amy");
    let pts = [Pt::new(0.1, 0.1), Pt::new(0.9, 0.1), Pt::new(0.5, 0.9)];
    svc.shape_set(&amy, &r.id, ShapeKind::Zone, "porch", true, 5, &pts).unwrap();
    svc.shape_set(&amy, &r.id, ShapeKind::Mask, "neighbour", false, 5, &pts).unwrap();
    let a = open_allowed(&r, "amy", "app", &[&r.id]);
    let b = open_allowed(&r, "bob", "app", &[&r.id]);
    let o = r.cams.overlay("amy", &a, &r.id).unwrap();
    assert_eq!(o.zones.iter().map(|z| (z.name.as_str(), z.mask)).collect::<Vec<_>>(), vec![("porch", false), ("neighbour", true)]);
    assert_eq!(o.zones[0].points.len(), 3);
    assert_eq!(r.cams.overlay("bob", &b, &r.id).unwrap(), Overlay::default(), "the zones of the owner's detector are not shown to somebody the camera is shared with");
    assert!(r.cams.overlay("bob", &a, &r.id).is_err(), "bob has no access to amy's view");
    let pending = r.cams.open("amy", "app", &spec(&[&r.id])).unwrap();
    assert_eq!(r.cams.overlay("amy", &pending.id, &r.id).unwrap(), Overlay::default(), "a view that was not allowed shows no overlay");
    // the mapping of a detector event
    let m = mark_of(&iyakki_camdetect::events::Event { id: 7, start_ms: 99, end_ms: None, zone: "porch".into(), score: 33, peak: 40, bbox: [0.1, 0.2, 0.3, 0.4], person: None, has_thumb: false });
    assert_eq!((m.id, m.at_ms, m.zone.as_str(), m.score, m.bbox, m.active), (7, 99, "porch", 33, [0.1, 0.2, 0.3, 0.4], true));
}

#[test]
fn the_views_wait_for_the_running_hub_and_rights_come_from_the_accounts_policies() {
    use iyakki_sec::policy::Role;
    let dir = std::fs::canonicalize(crate::testdir::unique("embedcam", "running")).unwrap();
    let data = dir.join("data");
    let policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync> = Arc::new(|u| match u {
        "amy" | "bob" => Some(Role::User.template()),
        "root" => Some(Role::Admin.template()),
        _ => None,
    });
    let cams = from_running(&data, policy_of);
    // the sign-in has not started the hub yet: nothing is listed, a view is refused in words, nothing is made
    assert!(cams.list("amy", "app").is_empty());
    assert!(cams.open("amy", "app", &spec(&["x"])).unwrap_err().contains("not running yet"));
    assert!(crate::cameras::existing(&data).is_none(), "asking never makes a hub");
    // the hub appears (the sign-in's resume)
    let fake = FakeCamera::rtsp(Config { frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() });
    let hub = crate::cameras::shared(Env {
        data: data.clone(),
        secrets: Arc::new(Vault::default()),
        people: Arc::new(|| vec!["amy".into(), "bob".into(), "root".into()]),
        usb: Arc::new(NoUsb),
        h264: Arc::new(NoH264),
        encode: Arc::new(|_, _, _| JPG.to_vec()),
        after_save: Arc::new(|_| {}),
        audit: Arc::new(|_, _, _| {}),
        backoff_ms: 30,
        idle: Duration::from_millis(400),
    });
    let amy = Who { user: "amy".into(), manage: false, usb_ok: false, home: dir.join("home/amy"), may_write: Arc::new(|_| true) };
    let id = hub.save(&amy, None, &NewSpec { name: "Gate".into(), address: format!("rtsp://127.0.0.1:{}/live", fake.port), user: String::new(), allowed: vec![] }, None).unwrap();
    assert_eq!(cams.list("amy", "app").iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Gate"]);
    assert!(cams.list("bob", "app").is_empty() && cams.list("root", "app").is_empty() && cams.list("ghost", "app").is_empty(), "other accounts, an administrator and an account that does not exist see nothing");
    let v = cams.open("amy", "app", &spec(&[&id])).unwrap();
    let CamEvent::Approval { call, .. } = cams.events("amy", &v.id).remove(0) else { panic!() };
    cams.decide("amy", &v.id, &call, true).unwrap();
    let t = Instant::now();
    let f = loop {
        if let Some(f) = cams.frame("amy", &v.id, &id, 0, FrameFormat::Rgba).unwrap() {
            break f;
        }
        assert!(t.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!((f.width, f.height), (48, 32));
    // the second handle over the same machine finds the same view
    let again = from_running(&data, Arc::new(|_| None));
    assert!(again.state("amy", &v.id).is_ok());
    assert_eq!(viewing_names(&data), vec!["Gate".to_string()]);
    cams.close("amy", &v.id);
    assert!(viewing_names(&data).is_empty());
    hub.stop_all();
}

#[test]
fn the_persons_switch_closes_their_views_stops_the_stream_and_refuses_new_ones_and_is_kept_for_next_time() {
    let (r, _) = rig("switch", false);
    assert!(r.cams.enabled("amy") && r.cams.enabled("bob") && r.cams.open_views("amy") == 0);
    let a = open_allowed(&r, "amy", "app", &[&r.id]);
    let b = open_allowed(&r, "bob", "app", &[&r.id]);
    wait_frame(&r, "amy", &a, &r.id, 0, FrameFormat::Rgba);
    assert_eq!((r.cams.open_views("amy"), r.cams.open_views("bob")), (1, 1));
    r.cams.set_enabled("amy", false).unwrap();
    assert_eq!((r.cams.open_views("amy"), r.cams.open_views("bob")), (0, 1), "bob's view is not amy's to close");
    assert!(r.cams.state("amy", &a).is_err() && r.cams.state("bob", &b).is_ok());
    assert_eq!(r.cams.open("amy", "app", &spec(&[&r.id])).unwrap_err(), iyakki_core_api::cameras::SWITCHED_OFF);
    assert!(!r.cams.enabled("amy") && r.cams.enabled("bob"));
    // the switch is a file of the person's own: another handle over the same machine reads it, and a name that is not a name is refused
    let again = AgentCameras::lazy(&r.dir.join("data"), Arc::new(|| None));
    assert!(!again.enabled("amy") && again.enabled("bob"));
    assert!(r.cams.set_enabled("../x", false).is_err() && !r.cams.enabled("../x"));
    assert!(r.audit.lock().unwrap().iter().any(|l| l == "amy|cameras.switch|off"));
    r.cams.set_enabled("amy", true).unwrap();
    open_allowed(&r, "amy", "app", &[&r.id]);
    // with the person's own view gone, the hub lets the camera go after the idle time (bob still looks, so only check amy's count)
    assert_eq!(r.cams.open_views("amy"), 1);
}

/// A camera on the ONVIF side that remembers what it was told (the in-house client over a recording transport).
#[derive(Clone, Default)]
struct OnvifFake(Arc<Mutex<Vec<String>>>);
impl crate::onvif::Transport for OnvifFake {
    fn post(&self, url: &str, action: &str, body: &str) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("{} {}", url.rsplit('/').next().unwrap_or(""), action.rsplit('/').next().unwrap_or("")));
        let _ = body;
        Ok(if action.ends_with("GetCapabilities") {
            "<a><tt:Media><tt:XAddr>http://c/media</tt:XAddr></tt:Media><tt:PTZ><tt:XAddr>http://c/ptz</tt:XAddr></tt:PTZ></a>".into()
        } else if action.ends_with("GetProfiles") {
            "<a><trt:Profiles token=\"t1\"><tt:PTZConfiguration/></trt:Profiles></a>".into()
        } else {
            "<a/>".into()
        })
    }
}

struct FixedSource;
impl crate::onvif::Source for FixedSource {
    fn created(&self) -> String {
        "2026-11-18T00:00:00Z".into()
    }
    fn nonce(&self) -> [u8; 16] {
        [7; 16]
    }
}

#[test]
fn a_list_of_a_network_camera_nobody_asked_starts_the_question_so_the_buttons_show_at_the_next_look() {
    let (r, _) = rig("ptzlist", false);
    let fake = OnvifFake::default();
    let f2 = fake.clone();
    let asked = Arc::new(Mutex::new(0u32));
    let a2 = asked.clone();
    r.cams.set_ptz_probe(Arc::new(move |_id, _addr, user, pass| {
        *a2.lock().unwrap() += 1;
        let c = crate::onvif::Onvif::new(Box::new(f2.clone()), Box::new(FixedSource), user, pass);
        crate::onvif::PtzCamera::probe(c, "http://c/device_service")
    }));
    // off until asked for: a list never touches the network
    assert!(r.cams.list("amy", "app").iter().all(|c| !c.ptz));
    r.cams.wait_probes();
    assert_eq!(*asked.lock().unwrap(), 0);
    r.cams.probe_on_list(true);
    assert!(r.cams.list("amy", "app").iter().all(|c| !c.ptz), "the answer is not there yet at the first look");
    r.cams.wait_probes();
    assert!(r.cams.list("amy", "app").iter().all(|c| c.ptz), "the buttons are there at the next look");
    r.cams.wait_probes();
    assert_eq!(*asked.lock().unwrap(), 1, "asked once, not at every list");
}

#[test]
fn a_network_camera_that_can_move_says_so_asks_once_per_view_and_then_moves() {
    let (r, _) = rig("ptz", false);
    let fake = OnvifFake::default();
    let f2 = fake.clone();
    // the probe is handed the stream address, the user name and the password from the vault, and never shows them
    r.cams.set_ptz_probe(Arc::new(move |_id, addr, user, pass| {
        assert!(addr.starts_with("rtsp://127.0.0.1:") && user == "admin" && pass == "pw-12345");
        let c = crate::onvif::Onvif::new(Box::new(f2.clone()), Box::new(FixedSource), user, pass);
        crate::onvif::PtzCamera::probe(c, "http://c/device_service")
    }));
    assert!(r.cams.list("amy", "app").iter().all(|c| !c.ptz), "not asked yet");
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    assert!(r.cams.probe_ptz("amy", &r.id).is_some());
    assert!(r.cams.list("amy", "app").iter().all(|c| c.ptz), "now known");
    // the first command asks, and nothing moves until the person says yes
    let call = r.cams.ptz("amy", &view, &r.id, PtzCommand::Move { pan: 0.5, tilt: 0.0, zoom: 0.0 }).unwrap().expect("a card");
    assert!(!fake.0.lock().unwrap().iter().any(|l| l.ends_with("ContinuousMove")));
    let card = r.cams.events("amy", &view).into_iter().find_map(|e| match e { CamEvent::Approval { action: CamAction::Ptz, summary, .. } => Some(summary), _ => None }).expect("the ptz card");
    assert!(card.contains("Front door"), "{card}");
    r.cams.decide("amy", &view, &call, true).unwrap();
    assert!(fake.0.lock().unwrap().iter().any(|l| l == "ptz ContinuousMove"));
    // later commands for this camera and view run at once
    assert_eq!(r.cams.ptz("amy", &view, &r.id, PtzCommand::Stop).unwrap(), None);
    assert!(fake.0.lock().unwrap().iter().any(|l| l == "ptz Stop"));
    assert!(r.audit.lock().unwrap().iter().any(|l| l.contains("cameras.ptz") && l.contains("Front door")));
    assert!(r.audit.lock().unwrap().iter().all(|l| !l.contains("pw-12345")));
}

#[test]
fn a_no_to_the_first_move_moves_nothing_and_a_camera_that_cannot_move_refuses_in_a_sentence() {
    let (r, _) = rig("ptz-no", false);
    let fake = OnvifFake::default();
    let f2 = fake.clone();
    r.cams.set_ptz_probe(Arc::new(move |_, _, user, pass| crate::onvif::PtzCamera::probe(crate::onvif::Onvif::new(Box::new(f2.clone()), Box::new(FixedSource), user, pass), "http://c/device_service")));
    let view = open_allowed(&r, "amy", "app", &[&r.id]);
    let call = r.cams.ptz("amy", &view, &r.id, PtzCommand::Move { pan: 1.0, tilt: 1.0, zoom: 0.0 }).unwrap().unwrap();
    r.cams.decide("amy", &view, &call, false).unwrap();
    assert!(!fake.0.lock().unwrap().iter().any(|l| l.ends_with("ContinuousMove")));
    // asked again: it asks again (a no is not remembered as a yes)
    assert!(r.cams.ptz("amy", &view, &r.id, PtzCommand::Stop).unwrap().is_some());
    let (r2, _) = rig("ptz-none", false);
    let v2 = open_allowed(&r2, "amy", "app", &[&r2.id]);
    assert!(r2.cams.ptz("amy", &v2, &r2.id, PtzCommand::Stop).unwrap_err().contains("no pan, tilt or zoom"));
}
