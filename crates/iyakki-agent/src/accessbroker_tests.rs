//! The local broker: the file log (bounded, private, no content), the approvals read afresh, revoke, and the camera hub's route through the broker.

use iyakki_shared::accessbroker::*;
use crate::accessbroker_revoke::revoke;
use std::path::{Path, PathBuf};
use iyakki_core_api::broker::{AccessEvent, AccessSink, CardAsk};
use std::sync::Arc;
use crate::cameras::{self, CamSecrets, Env, Hub, NoH264, UsbCam, UsbProvider, Who};
use iyakki_core_api::appcaps::{record_approval, CapabilityRequest};
use iyakki_core_api::broker::{Call, Caller, CallerKind, CardAnswer, Denial};
use std::time::Duration;
use zeroize::Zeroizing;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-acc-{tag}-{}-{}", std::process::id(), crate::clock::now_unix()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ev(at: i64, app: &str, allowed: bool) -> AccessEvent {
    AccessEvent { at, app: app.into(), person: "amy".into(), device: "box".into(), capability: "camera.view".into(), target: "HD Webcam".into(), allowed, reason: if allowed { String::new() } else { "not_granted".into() } }
}

fn req(name: &str) -> CapabilityRequest {
    CapabilityRequest { name: name.into(), why: "x".into(), param: None, extra: Default::default() }
}

fn put(data: &Path, inst: &str, reqs: &[CapabilityRequest]) {
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    use iyakki_core_api::appcaps::ApprovalBook;
    book.put(inst, record_approval("amy", "notes", "1.0", "abc", 7, reqs));
}

#[test]
fn the_log_is_private_bounded_newest_first_and_holds_no_content() {
    let d = tmp("log");
    let log = FileLog::with_cap(&d, 20);
    for i in 0..100 {
        log.record(ev(i, "cameras", i % 2 == 0));
    }
    let rows = log.recent(1000);
    assert!(rows.len() <= 22 && rows.len() >= 20, "{}", rows.len());
    assert_eq!(rows[0].at, 99, "newest first");
    assert!(rows.windows(2).all(|w| w[0].at > w[1].at));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(d.join("access/log.jsonl")).unwrap().permissions().mode() & 0o777, 0o600);
    let text = std::fs::read_to_string(d.join("access/log.jsonl")).unwrap();
    assert!(text.lines().count() <= 22);
    assert_eq!(log.recent(3).len(), 3);
    // a fresh handle sees the same file and goes on counting from it
    let again = FileLog::with_cap(&d, 20);
    again.record(ev(100, "cameras", true));
    assert_eq!(again.recent(1)[0].at, 100);
    log.clear();
    assert!(log.recent(10).is_empty() && !d.join("access/log.jsonl").exists());
}

#[test]
fn approvals_are_read_afresh_so_a_revoke_applies_at_the_next_call() {
    let d = tmp("revoke");
    put(&d, "tile/user/amy/notes", &[req("camera.view"), req("notify")]);
    let (broker, log) = local(&d, None);
    let caller = Caller { app: "notes".into(), kind: CallerKind::Store { installation: "tile/user/amy/notes".into() }, person: "amy".into(), device: "box".into() };
    let call = Call { caller: &caller, capability: "camera.view", unit: Some("usb:video0"), label: "HD Webcam" };
    assert_eq!(broker.decide(&call), Ok(()));
    let held = held_list(&d);
    assert_eq!((held.len(), held[0].holds.clone(), held[0].app.as_str()), (1, vec!["camera.view".to_string(), "notify".to_string()], "notes"));
    assert_eq!(revoke(&d, "tile/user/amy/notes", "camera.view"), Ok(true));
    assert_eq!(revoke(&d, "tile/user/amy/notes", "camera.view"), Ok(false), "already revoked");
    assert!(revoke(&d, "no/such", "camera.view").is_err());
    let held = held_list(&d);
    assert_eq!((held[0].holds.clone(), held[0].revoked.clone()), (vec!["notify".to_string()], vec!["camera.view".to_string()]));
    // the same broker, the next call
    let call2 = Call { label: "HD Webcam", ..call };
    std::thread::sleep(Duration::from_millis(1));
    assert_eq!(broker.decide(&call2), Err(Denial::Revoked));
    let rows = log.recent(10);
    assert_eq!(rows.len(), 2, "the allowed line and the refusal");
    assert!(!rows[0].allowed && rows[0].reason == "revoked" && rows[1].allowed);
}

#[test]
fn hard_asks_have_no_card_in_the_local_broker_until_the_shell_gives_one() {
    let d = tmp("hard");
    put(&d, "tile/user/amy/notes", &[req("microphone.listen")]);
    let (broker, log) = local(&d, None);
    let caller = Caller { app: "notes".into(), kind: CallerKind::Store { installation: "tile/user/amy/notes".into() }, person: "amy".into(), device: "box".into() };
    assert_eq!(broker.decide(&Call { caller: &caller, capability: "microphone.listen", unit: None, label: "" }), Err(Denial::NoCard));
    assert_eq!(log.recent(1)[0].reason, "no_card");
}

// ---- the camera hub's route

#[derive(Default)]
struct Vault;
impl CamSecrets for Vault {
    fn get(&self, _: &str, _: &str) -> Result<Zeroizing<String>, String> {
        Err("none".into())
    }
    fn set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn has(&self, _: &str, _: &str) -> bool {
        false
    }
}

struct OneUsb;
struct Solid;
impl cameras::Feed for Solid {
    fn next(&mut self) -> Result<cameras::Picture, cameras::FeedErr> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(cameras::Picture::Rgba { width: 8, height: 4, data: [200, 100, 50, 255].repeat(32) })
    }
}
impl UsbProvider for OneUsb {
    fn list(&self) -> Vec<UsbCam> {
        vec![UsbCam { id: "video0".into(), name: "HD Webcam".into(), builtin: false }]
    }
    fn open(&self, _: &str) -> Result<Box<dyn cameras::Feed>, cameras::FeedErr> {
        Ok(Box::new(Solid))
    }
}

fn hub(d: &Path, usb: Arc<dyn UsbProvider>) -> Hub {
    Hub::new(Env {
        data: d.to_path_buf(),
        secrets: Arc::new(Vault),
        people: Arc::new(|| vec!["amy".into()]),
        usb,
        h264: Arc::new(NoH264),
        encode: Arc::new(|_, _, _| vec![0xFF, 0xD8, 1, 2, 3]),
        after_save: Arc::new(|_| {}),
        audit: Arc::new(|_, _, _| {}),
        backoff_ms: 30,
        idle: Duration::from_millis(500),
    })
}

fn amy(d: &Path) -> Who {
    Who { user: "amy".into(), manage: false, usb_ok: true, home: d.join("home/amy"), may_write: Arc::new(|_| true) }
}

#[test]
fn the_cameras_app_views_and_snapshots_through_the_broker_and_each_use_is_logged_once() {
    let d = tmp("route");
    let h = hub(&d, Arc::new(OneUsb));
    let (broker, log) = local(&d, None);
    h.set_broker(broker, "iyakki-test");
    let who = amy(&d);
    let t = std::time::Instant::now();
    let mut got = false;
    while t.elapsed() < Duration::from_secs(5) {
        if h.view_on_screen(&who, "usb:video0", false).unwrap().is_some() {
            got = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(got, "a picture arrived through the broker");
    let p = h.snapshot(&who, "usb:video0").unwrap();
    assert!(p.exists());
    let rows = log.recent(10);
    assert_eq!(rows.len(), 1, "dozens of view calls and a snapshot are one line");
    assert_eq!((rows[0].app.as_str(), rows[0].person.as_str(), rows[0].device.as_str(), rows[0].capability.as_str(), rows[0].target.as_str(), rows[0].allowed), ("cameras", "amy", "iyakki-test", "camera.view", "HD Webcam", true));
    // leaving the tile still closes the camera at once (KI-080)
    assert_eq!(h.running(), 1);
    h.release_usb();
    assert_eq!(h.running(), 0);
    h.stop_all();
}

#[test]
fn refusal_per_route_view_and_snapshot_never_open_the_camera() {
    // each case: (what the caller is, what is stored) -> the stable code of the refusal
    let cases: Vec<(&str, Option<Vec<CapabilityRequest>>, bool, &str)> = vec![
        ("missing grant, no approval", None, false, "not_granted"),
        ("missing grant, other capability", Some(vec![req("notify")]), false, "not_granted"),
        ("revoked grant", Some(vec![req("camera.view")]), true, "revoked"),
    ];
    for (n, (name, stored, revoke_it, code)) in cases.into_iter().enumerate() {
        let d = tmp(&format!("deny{n}"));
        if let Some(r) = &stored {
            put(&d, "tile/user/amy/notes", r);
            if revoke_it {
                assert_eq!(revoke(&d, "tile/user/amy/notes", "camera.view"), Ok(true));
            }
        }
        let opened = Arc::new(std::sync::atomic::AtomicU32::new(0));
        struct Counted(Arc<std::sync::atomic::AtomicU32>);
        impl UsbProvider for Counted {
            fn list(&self) -> Vec<UsbCam> {
                OneUsb.list()
            }
            fn open(&self, id: &str) -> Result<Box<dyn cameras::Feed>, cameras::FeedErr> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                OneUsb.open(id)
            }
        }
        let h = hub(&d, Arc::new(Counted(opened.clone())));
        let (broker, log) = local(&d, None);
        h.set_broker_as(broker, "box", "notes", CallerKind::Store { installation: "tile/user/amy/notes".into() });
        let who = amy(&d);
        let e = h.view(&who, "usb:video0", false).unwrap_err();
        assert!(e.contains(code), "{name}: {e}");
        let e = h.snapshot(&who, "usb:video0").unwrap_err();
        assert!(e.contains(code), "{name}: {e}");
        assert_eq!(opened.load(std::sync::atomic::Ordering::SeqCst), 0, "{name}: the camera was never opened");
        assert_eq!(h.running(), 0, "{name}");
        let rows = log.recent(10);
        assert_eq!(rows.len(), 1, "{name}: refusals are logged, folded into one line");
        assert!(!rows[0].allowed && rows[0].reason == code && rows[0].app == "notes", "{name}");
    }
}

#[test]
fn refusal_unknown_capability_and_hard_ask_without_a_card_through_the_local_broker() {
    let d = tmp("unk");
    let (broker, log) = local(&d, None);
    let builtin = Caller { app: "cameras".into(), kind: CallerKind::Builtin, person: "amy".into(), device: "box".into() };
    assert_eq!(broker.decide(&Call { caller: &builtin, capability: "camera.view.everything", unit: None, label: "" }).unwrap_err().code(), "unknown_capability");
    // a built-in app may ask for recording; with no card it is refused, with a card that says no it is refused, with yes it is allowed
    let rec = Call { caller: &builtin, capability: "camera.record", unit: Some("usb:video0"), label: "HD Webcam" };
    assert_eq!(broker.decide(&rec).unwrap_err().code(), "no_card");
    struct Fixed(CardAnswer);
    impl CardAsk for Fixed {
        fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
            self.0
        }
    }
    let (b2, _) = local(&d, Some(Arc::new(Fixed(CardAnswer::Deny))));
    assert_eq!(b2.decide(&rec).unwrap_err().code(), "card_declined");
    let (b3, _) = local(&d, Some(Arc::new(Fixed(CardAnswer::Allow))));
    assert_eq!(b3.decide(&rec), Ok(()));
    assert!(log.recent(10).iter().any(|e| e.reason == "unknown_capability") && log.recent(10).iter().any(|e| e.reason == "no_card"));
}
