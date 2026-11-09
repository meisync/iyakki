use super::*;
use crate::cameras::{Env, NoH264, NoUsb, VaultCamSecrets};
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use std::sync::Mutex;

const JPG: &[u8] = include_bytes!("../../iyakki-rtsp/tests/data/test-48x32.jpg");

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, _: &str, _: &Value) -> Result<String, String> {
        Err("no".into())
    }
    fn describe(&self, n: &str, _: &Value) -> String {
        n.into()
    }
}

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Rig {
    tools: CameraViewTools,
    svc: Arc<Services>,
    audit: Arc<Mutex<Vec<String>>>,
    dir: std::path::PathBuf,
}

fn rig(tag: &str, wait_ms: u64) -> Rig {
    let dir = std::fs::canonicalize(crate::testdir::unique("camtools", tag)).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    svc.on_login("amy", "amy-pass-123", false);
    let audit = Arc::new(Mutex::new(vec![]));
    let a2 = audit.clone();
    let env = Env {
        data: dir.join("data"),
        secrets: Arc::new(VaultCamSecrets(svc.clone())),
        people: Arc::new(|| vec!["amy".into(), "bob".into()]),
        usb: Arc::new(NoUsb),
        h264: Arc::new(NoH264),
        encode: Arc::new(|_, _, _| vec![0xFF, 0xD8, 0, 0, 0]),
        after_save: Arc::new(|_| {}),
        audit: Arc::new(move |u, e, d| a2.lock().unwrap().push(format!("{u}|{e}|{d}"))),
        backoff_ms: 30,
        idle: Duration::from_secs(5),
    };
    let hub = Arc::new(Hub::new(env));
    let who = Who { user: "amy".into(), manage: false, usb_ok: true, home: dir.join("home/amy"), may_write: Arc::new(|_| true) };
    let tools = CameraViewTools::new(Arc::new(NoTools), hub, who, svc.clone()).with_secret_wait(Duration::from_millis(wait_ms));
    Rig { tools, svc, audit, dir }
}

fn cfg(pw: Option<(&str, &str)>) -> Config {
    Config { user: pw.map(|(u, p)| (u.to_string(), p.to_string())), digest: true, frames: vec![JPG.to_vec()], interval_ms: 15, ..Default::default() }
}

#[test]
fn listing_is_free_and_everything_else_asks_and_snapshots_and_removals_never_get_a_standing_grant() {
    let r = rig("risk", 50);
    let defs = r.tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk("cameras_list"), Risk::Safe);
    for n in ["camera_snapshot", "cameras_add", "cameras_remove", "cameras_access"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    assert!(defs.iter().find(|d| d.spec.name == "cameras_list").unwrap().untrusted, "camera names and errors come from devices");
    assert!(r.tools.grant_blocker("camera_snapshot", &json!({"camera": "x"})).is_some());
    assert!(r.tools.grant_blocker("cameras_remove", &json!({"camera": "x"})).is_some());
    assert!(r.tools.grant_blocker("cameras_add", &json!({})).is_none());
    assert!(!r.tools.pre_approved("camera_snapshot") && !r.tools.approved_always("cameras_add"));
}

#[test]
fn a_password_can_never_pass_through_a_tool_neither_as_a_field_nor_inside_the_address() {
    let r = rig("pw", 50);
    for args in [json!({"name": "a", "address": "rtsp://h/x", "password": "hunter2"}), json!({"name": "a", "address": "rtsp://admin:hunter2@h/x"}), json!({"name": "a", "address": "rtsp://admin@h/x"}), json!({"name": "a", "address": "rtsp://h/x", "token": "t"})] {
        let e = r.tools.refusal("cameras_add", &args).expect("refused");
        assert!(!e.contains("hunter2"), "{e}");
    }
    assert!(r.tools.refusal("cameras_add", &json!({"name": "a", "address": "rtsp://h/x", "user": "admin", "password": true})).is_none());
    assert!(r.tools.refusal("cameras_add", &json!({"name": "a"})).is_some());
    assert!(r.tools.refusal("cameras_add", &json!({"name": "a", "address": "ftp://h"})).is_some());
    assert!(r.tools.refusal("camera_snapshot", &json!({})).is_some());
    let card = r.tools.preview("cameras_add", &json!({"name": "Door", "address": "rtsp://10.0.0.5/live", "user": "admin", "password": true, "allowed": ["bob"]})).unwrap();
    assert!(card.contains("masked form") && card.contains("bob") && !card.contains("hunter2"));
}

#[test]
fn adding_with_a_password_opens_the_masked_form_and_the_assistant_only_learns_it_was_stored() {
    let cam = FakeCamera::rtsp(cfg(Some(("admin", "s3cret"))));
    let r = rig("add", 5000);
    let svc = r.svc.clone();
    // the person answers the masked form while the tool waits
    let answerer = std::thread::spawn(move || {
        let t = Instant::now();
        loop {
            if let Some(req) = svc.secret_requests.pending("amy").first().cloned() {
                assert!(req.purpose.contains("Front door") && req.kind == "password");
                svc.answer_secret_request("amy", &req.id, "cam.front", "s3cret").unwrap();
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "no secret request appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let out = r.tools.call("cameras_add", &json!({"name": "Front door", "address": format!("rtsp://127.0.0.1:{}/live", cam.port), "user": "admin", "password": true})).unwrap();
    answerer.join().unwrap();
    assert!(out.contains("stored in the person's vault") && !out.contains("s3cret"), "{out}");
    assert_eq!(&*r.svc.user_get("amy", "cam.front").unwrap(), "s3cret");
    let record = std::fs::read_to_string(r.dir.join("data/cameras/cameras.json")).unwrap();
    assert!(!record.contains("s3cret") && record.contains("\"secret\": \"cam.front\""), "{record}");
    // the stream signs in with that password: snapshot works (asks happen in the agent loop; the call itself saves one picture)
    let snap = r.tools.call("camera_snapshot", &json!({"camera": "Front door"})).unwrap();
    assert!(snap.contains("IMAGE: ") && snap.contains("/Pictures/camera-front-door-"), "{snap}");
    let list = r.tools.call("cameras_list", &json!({})).unwrap();
    assert!(list.contains("Front door") && list.contains(&format!("rtsp://127.0.0.1:{}/live", cam.port)) && !list.contains("s3cret") && !list.contains("admin:"), "{list}");
    assert!(r.audit.lock().unwrap().iter().all(|l| !l.contains("s3cret")));
    r.tools.hub.stop_all();
}

#[test]
fn cancelling_the_form_or_a_locked_vault_keeps_the_camera_without_a_password_and_says_so() {
    let r = rig("cancel", 5000);
    let svc = r.svc.clone();
    let canceller = std::thread::spawn(move || {
        let t = Instant::now();
        loop {
            if let Some(req) = svc.secret_requests.pending("amy").first().cloned() {
                svc.cancel_secret_request("amy", &req.id).unwrap();
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let out = r.tools.call("cameras_add", &json!({"name": "A", "address": "rtsp://127.0.0.1:1/x", "password": true})).unwrap();
    canceller.join().unwrap();
    assert!(out.contains("chose not to store"), "{out}");
    // no answer in time
    let r2 = rig("timeout", 60);
    let out = r2.tools.call("cameras_add", &json!({"name": "B", "address": "rtsp://127.0.0.1:1/x", "password": true})).unwrap();
    assert!(out.contains("No password was entered in time"), "{out}");
    // a locked vault
    let r3 = rig("locked", 60);
    r3.svc.on_logout("amy", false);
    let out = r3.tools.call("cameras_add", &json!({"name": "C", "address": "rtsp://127.0.0.1:1/x", "password": true})).unwrap();
    assert!(out.contains("NOT stored") && out.contains("vault is locked"), "{out}");
}

#[test]
fn removing_and_sharing_work_by_name_and_respect_ownership() {
    let r = rig("share", 50);
    r.tools.call("cameras_add", &json!({"name": "Yard", "address": "rtsp://127.0.0.1:1/y"})).unwrap();
    let out = r.tools.call("cameras_access", &json!({"camera": "yard", "people": ["bob"]})).unwrap();
    assert!(out.contains("you and bob"));
    assert!(r.tools.call("cameras_access", &json!({"camera": "Yard", "people": ["nobody"]})).unwrap_err().contains("no account"));
    assert!(r.tools.call("cameras_list", &json!({})).unwrap().contains("also watched by bob"));
    let card = r.tools.preview("cameras_remove", &json!({"camera": "Yard"})).unwrap();
    assert!(card.contains("REMOVE") && card.contains("rtsp://127.0.0.1:1/y"));
    r.tools.call("cameras_remove", &json!({"camera": "Yard"})).unwrap();
    assert!(r.tools.call("cameras_list", &json!({})).unwrap().starts_with("No cameras"));
    assert!(r.tools.call("camera_snapshot", &json!({"camera": "Yard"})).unwrap_err().contains("no camera called"));
}

// ------------------------------------------------------------------------------------------------ recording (F-CAM-03)

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn rec_env(secrets: Arc<dyn crate::cameras::CamSecrets>) -> crate::camrec::RecEnv {
    crate::camrec::RecEnv {
        source: Arc::new(crate::camrec::NetSource(secrets)),
        free_space: Arc::new(|_| Some(500_000 * 1024 * 1024)),
        now_ms: Arc::new(now_ms),
        zone: Arc::new(crate::clock::Zone::utc),
        on_active: Arc::new(|_| {}),
        segment_ms: 150,
        retention_every: Duration::from_secs(3600),
        backoff_ms: 20,
        retry_after: Duration::from_millis(60),
    }
}

fn rec_rig(tag: &str) -> (Rig, Arc<crate::camrec::Recorder>, FakeCamera, String) {
    let cam = FakeCamera::rtsp(cfg(Some(("admin", "s3cret"))));
    let r = rig(tag, 50);
    let rec = Arc::new(crate::camrec::Recorder::new(r.tools.hub.clone(), rec_env(Arc::new(VaultCamSecrets(r.svc.clone())))));
    let Rig { tools, svc, audit, dir } = r;
    let tools = tools.with_recorder(rec.clone());
    let r = Rig { tools, svc, audit, dir };
    let id = r.tools.hub.save(&amy_who(&r), None, &crate::cameras::NewSpec { name: "Front door".into(), address: format!("rtsp://127.0.0.1:{}/live", cam.port), user: "admin".into(), allowed: vec!["bob".into()] }, Some("s3cret")).unwrap();
    (r, rec, cam, id)
}

fn amy_who(r: &Rig) -> Who {
    Who { user: "amy".into(), manage: false, usb_ok: true, home: r.dir.join("home/amy"), may_write: Arc::new(|_| true) }
}

#[test]
fn the_recording_tools_exist_only_with_a_recorder_and_the_right_ones_ask_every_time() {
    let r = rig("rec-defs", 50);
    assert!(!r.tools.defs().iter().any(|d| d.spec.name.starts_with("camera_recording") || d.spec.name == "camera_export"), "no recorder, no recording tools");
    let (r, _rec, _cam, _id) = rec_rig("rec-defs2");
    let defs = r.tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing")).risk;
    assert_eq!((risk("camera_recording_status"), risk("camera_recordings_list")), (Risk::Safe, Risk::Safe));
    assert_eq!((risk("camera_recording_set"), risk("camera_export")), (Risk::Ask, Risk::Ask));
    for n in ["camera_recording_set", "camera_export"] {
        assert!(r.tools.grant_blocker(n, &json!({"camera": "x"})).is_some(), "{n}: no standing grant");
        assert!(!r.tools.pre_approved(n) && !r.tools.approved_always(n));
    }
    // turning recording ON and exporting are hard asks (Auto mode does not skip them); turning it OFF is not
    assert!(crate::agent::hard_ask("camera_recording_set", &json!({"camera": "x", "mode": "continuous"})).is_some());
    assert!(crate::agent::hard_ask("camera_recording_set", &json!({"camera": "x", "mode": "schedule", "windows": ["daily 22:00-06:00"]})).is_some());
    assert!(crate::agent::hard_ask("camera_recording_set", &json!({"camera": "x", "mode": "off"})).is_none());
    assert!(crate::agent::hard_ask("camera_export", &json!({"camera": "x", "from": "a", "to": "b"})).is_some());
    assert!(PROMPT_RULE.contains("camera_recording_set") && PROMPT_RULE.contains("lost"));
}

#[test]
fn bad_recording_requests_are_refused_before_asking() {
    let (r, _rec, _cam, _id) = rec_rig("rec-bad");
    for args in [json!({"camera": "Front door"}), json!({"camera": "Front door", "mode": "sometimes"}), json!({"camera": "Front door", "mode": "schedule", "windows": ["whenever"]}), json!({"camera": "Front door", "max_days": -3}), json!({"camera": "Front door", "max_gb": "lots"}), json!({"camera": "Front door", "password": "x", "mode": "off"})] {
        assert!(r.tools.refusal("camera_recording_set", &args).is_some(), "{args}");
    }
    assert!(r.tools.refusal("camera_export", &json!({"camera": "Front door", "from": "2026-11-08 10:00"})).is_some());
    assert!(r.tools.refusal("camera_recording_set", &json!({"camera": "Front door", "mode": "off"})).is_none());
    assert!(r.tools.refusal("camera_recording_set", &json!({"camera": "Front door", "windows": ["mon-fri 08:00-18:00"], "max_gb": 2.5, "max_days": 7})).is_none());
}

#[test]
fn the_card_for_turning_recording_on_says_what_is_stored_where_and_what_losing_the_vault_means() {
    let (r, rec, _cam, id) = rec_rig("rec-card");
    let card = r.tools.preview("camera_recording_set", &json!({"camera": "Front door", "mode": "continuous", "max_days": 5, "max_gb": 2})).unwrap();
    for must in ["START RECORDING", "Front door", "encrypted", "vault", "red REC", "one click", "5 days", "2.0 GB", "lose your vault"] {
        assert!(card.contains(must), "{must}: {card}");
    }
    let card = r.tools.preview("camera_recording_set", &json!({"camera": "Front door", "mode": "schedule", "windows": ["mon-fri 08:00-18:00"]})).unwrap();
    assert!(card.contains("only during weekdays 08:00-18:00"), "{card}");
    assert!(r.tools.preview("camera_recording_set", &json!({"camera": "Front door", "mode": "off"})).unwrap().contains("TURN OFF"));
    assert!(r.tools.describe("camera_recording_set", &json!({"camera": "Front door", "mode": "continuous"})).contains("START RECORDING"));
    // nothing was changed by looking at cards
    assert_eq!(rec.settings_of(&id).mode, crate::camrec::Mode::Off);
}

#[test]
fn recording_by_chat_end_to_end_status_set_list_export_and_stop() {
    let (r, rec, _cam, id) = rec_rig("rec-e2e");
    let st = r.tools.call("camera_recording_status", &json!({})).unwrap();
    assert!(st.contains("Front door") && st.contains("not recording") && st.contains("mode off") && st.contains("lose your vault"), "{st}");
    let out = r.tools.call("camera_recording_set", &json!({"camera": "Front door", "mode": "continuous", "max_days": 3, "max_gb": 1})).unwrap();
    assert!(out.contains("Recording is ON") && out.contains("REC sign") && out.contains("Kept 3 days"), "{out}");
    assert!(rec.recording(&id));
    let t = Instant::now();
    while rec.status(&amy_who(&r), &id).unwrap().segments < 1 {
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", rec.status(&amy_who(&r), &id).unwrap().state);
        std::thread::sleep(Duration::from_millis(30));
    }
    let st = r.tools.call("camera_recording_status", &json!({"camera": "Front door"})).unwrap();
    assert!(st.contains("stored:") && st.contains("keeps 3 days"), "{st}");
    let list = r.tools.call("camera_recordings_list", &json!({"camera": "front door"})).unwrap();
    assert!(list.contains("pictures") && list.contains("recorded"), "{list}");
    // export the recorded range
    let days = rec.days(&amy_who(&r), &id).unwrap();
    let (a, b) = days[0].spans[0];
    let args = json!({"camera": "Front door", "from": rec.time_text(a), "to": rec.time_text(b + 1000)});
    let card = r.tools.preview("camera_export", &args).unwrap();
    assert!(card.contains("EXPORT") && card.contains("NOT encrypted"), "{card}");
    let out = r.tools.call("camera_export", &args).unwrap();
    assert!(out.contains("Exported") && out.contains("/Pictures/camera-front-door-"), "{out}");
    let folder = std::path::PathBuf::from(out.split(" to ").nth(1).unwrap().split(". ").next().unwrap().trim_end_matches('.'));
    assert!(std::fs::read(folder.join("frame-000001.jpg")).unwrap().starts_with(&[0xFF, 0xD8]));
    // one call stops it
    let out = r.tools.call("camera_recording_set", &json!({"camera": "Front door", "mode": "off"})).unwrap();
    assert!(out.contains("Recording is OFF"), "{out}");
    assert!(!rec.recording(&id));
    // the log has all of it, without the camera's password
    let log = serde_json::to_string(&rec.audit(&amy_who(&r), &id, 100).unwrap()).unwrap();
    for ev in ["recording.start", "recording.setting", "export", "recording.stop"] {
        assert!(log.contains(ev), "{ev}: {log}");
    }
    assert!(!log.contains("s3cret"));
    rec.shutdown();
    r.tools.hub.stop_all();
}

#[test]
fn a_person_the_camera_is_shared_with_can_see_that_it_records_but_cannot_turn_it_on_list_or_export() {
    let (r, rec, _cam, id) = rec_rig("rec-bob");
    r.tools.call("camera_recording_set", &json!({"camera": "Front door", "mode": "continuous"})).unwrap();
    let bob = Who { user: "bob".into(), manage: false, usb_ok: false, home: r.dir.join("home/bob"), may_write: Arc::new(|_| true) };
    let tools = CameraViewTools::new(Arc::new(NoTools), r.tools.hub.clone(), bob, r.svc.clone()).with_recorder(rec.clone());
    let st = tools.call("camera_recording_status", &json!({})).unwrap();
    assert!(st.contains("recording") && st.contains("owner amy") && !st.contains("stored:"), "{st}");
    assert!(tools.call("camera_recording_set", &json!({"camera": "Front door", "mode": "off"})).is_err(), "not even to switch it off");
    assert!(tools.call("camera_recordings_list", &json!({"camera": "Front door"})).unwrap_err().contains("only the person who added"));
    assert!(tools.call("camera_export", &json!({"camera": "Front door", "from": "2026-01-01 00:00", "to": "2026-01-02 00:00"})).unwrap_err().contains("only the person who added"));
    assert!(rec.recording(&id), "bob changed nothing");
    r.tools.call("camera_recording_set", &json!({"camera": "Front door", "mode": "off"})).unwrap();
    rec.shutdown();
    r.tools.hub.stop_all();
}

#[test]
fn only_an_administrator_changes_the_limits_for_all_cameras_and_the_floor_has_a_minimum() {
    let (r, rec, _cam, _id) = rec_rig("rec-limits");
    assert_eq!(r.tools.defs().iter().find(|d| d.spec.name == "camera_recording_limits").unwrap().risk, Risk::Ask);
    assert!(r.tools.grant_blocker("camera_recording_limits", &json!({})).is_some());
    assert!(r.tools.refusal("camera_recording_limits", &json!({})).is_some());
    assert!(r.tools.refusal("camera_recording_limits", &json!({"total_max_gb": "all"})).is_some());
    let e = r.tools.call("camera_recording_limits", &json!({"total_max_gb": 50})).unwrap_err();
    assert!(e.contains("administrator"), "{e}");
    let root = Who { user: "root".into(), manage: true, usb_ok: false, home: r.dir.join("home/root"), may_write: Arc::new(|_| true) };
    let t = CameraViewTools::new(Arc::new(NoTools), r.tools.hub.clone(), root, r.svc.clone()).with_recorder(rec.clone());
    let card = t.preview("camera_recording_limits", &json!({"total_max_gb": 50, "min_free_gb": 2})).unwrap();
    assert!(card.contains("50.0 GB") && card.contains("stops below 2.0 GB"), "{card}");
    let out = t.call("camera_recording_limits", &json!({"total_max_gb": 50, "min_free_gb": 2})).unwrap();
    assert!(out.contains("51200 MB") && out.contains("2048 MB"), "{out}");
    assert_eq!(rec.policy().total_max_mb, 51_200);
    let e = t.call("camera_recording_limits", &json!({"min_free_gb": 0.1})).unwrap_err();
    assert!(e.contains("floor"), "{e}");
    r.tools.hub.stop_all();
}
