use super::*;
use crate::appcaps::{record_approval, CapabilityRequest, MemoryBook};
use std::sync::atomic::{AtomicI64, Ordering};

struct Card(CardAnswer, Mutex<Vec<String>>);
impl CardAsk for Card {
    fn ask(&self, c: &Caller, cap: &str, label: &str) -> CardAnswer {
        self.1.lock().unwrap().push(format!("{} {cap} {label}", c.app));
        self.0
    }
}

fn req(name: &str, param: Option<&str>) -> CapabilityRequest {
    CapabilityRequest { name: name.into(), why: "because".into(), param: param.map(str::to_string), extra: Default::default() }
}

fn builtin(app: &str) -> Caller {
    Caller { app: app.into(), kind: CallerKind::Builtin, person: "alice".into(), device: "box".into() }
}

fn store(inst: &str) -> Caller {
    Caller { app: "notes".into(), kind: CallerKind::Store { installation: inst.into() }, person: "alice".into(), device: "box".into() }
}

struct Rig {
    broker: Broker,
    log: Arc<MemoryLog>,
    now: Arc<AtomicI64>,
    asked: Arc<Card>,
}

fn rig(book: MemoryBook, answer: CardAnswer) -> Rig {
    let log = Arc::new(MemoryLog::new(100));
    let now = Arc::new(AtomicI64::new(1000));
    let asked = Arc::new(Card(answer, Mutex::new(vec![])));
    let n2 = now.clone();
    let broker = Broker::new(Arc::new(book), asked.clone(), log.clone(), Arc::new(move || n2.load(Ordering::SeqCst)));
    Rig { broker, log, now, asked }
}

fn call<'a>(c: &'a Caller, cap: &'a str, unit: Option<&'a str>) -> Call<'a> {
    Call { caller: c, capability: cap, unit, label: "Front" }
}

#[test]
fn a_builtin_app_in_the_table_may_view_a_camera_and_it_is_logged() {
    let r = rig(MemoryBook::default(), CardAnswer::Deny);
    let c = builtin("cameras");
    assert_eq!(r.broker.decide(&call(&c, "camera.view", Some("usb:1"))), Ok(()));
    let rows = r.log.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].app.as_str(), rows[0].person.as_str(), rows[0].device.as_str(), rows[0].capability.as_str(), rows[0].target.as_str(), rows[0].allowed), ("cameras", "alice", "box", "camera.view", "Front", true));
    assert!(r.asked.1.lock().unwrap().is_empty(), "camera.view is not a hard ask");
}

#[test]
fn refusal_unknown_capability() {
    let r = rig(MemoryBook::default(), CardAnswer::Allow);
    let c = builtin("cameras");
    let e = r.broker.decide(&call(&c, "camera.peek", None)).unwrap_err();
    assert_eq!((e.clone(), e.code()), (Denial::UnknownCapability, "unknown_capability"));
    let rows = r.log.rows();
    assert!(!rows[0].allowed && rows[0].reason == "unknown_capability");
}

#[test]
fn refusal_missing_grant_for_a_store_app_and_for_a_builtin_outside_the_table() {
    let r = rig(MemoryBook::default(), CardAnswer::Allow);
    // no approval at all
    assert_eq!(r.broker.decide(&call(&store("tile/user/alice/notes"), "camera.view", None)), Err(Denial::NotGranted));
    // an approval that does not list the capability
    let mut book = MemoryBook::default();
    book.put("tile/user/alice/notes", record_approval("alice", "notes", "1", "ab", 1, &[req("notify", None)]));
    let r2 = rig(book, CardAnswer::Allow);
    assert_eq!(r2.broker.decide(&call(&store("tile/user/alice/notes"), "camera.view", None)), Err(Denial::NotGranted));
    // a granted camera is not another camera
    let mut book = MemoryBook::default();
    book.put("i", record_approval("alice", "notes", "1", "ab", 1, &[req("camera.view", Some("usb:1"))]));
    let r3 = rig(book, CardAnswer::Allow);
    assert_eq!(r3.broker.decide(&call(&store("i"), "camera.view", Some("usb:1"))), Ok(()));
    assert_eq!(r3.broker.decide(&call(&store("i"), "camera.view", Some("usb:2"))), Err(Denial::NotGranted));
    // a built-in app that is not in the table for a table-only capability
    assert_eq!(r.broker.decide(&call(&builtin("notes"), "bluetooth.use", None)), Err(Denial::NotGranted));
    assert_eq!(r.broker.decide(&call(&builtin("notes"), "usb.use", None)), Err(Denial::NotGranted));
    assert!(r.log.rows().iter().all(|e| !e.allowed && e.reason == "not_granted"));
}

#[test]
fn refusal_revoked_grant_is_refused_from_the_next_call() {
    let mut a = record_approval("alice", "notes", "1", "ab", 1, &[req("camera.view", None)]);
    let mut book = MemoryBook::default();
    book.put("i", a.clone());
    let r = rig(book, CardAnswer::Allow);
    assert_eq!(r.broker.decide(&call(&store("i"), "camera.view", Some("usb:1"))), Ok(()));
    assert!(a.revoke("camera.view") && !a.revoke("camera.view") && !a.revoke("notify"));
    let mut book = MemoryBook::default();
    book.put("i", a);
    let r = rig(book, CardAnswer::Allow);
    let e = r.broker.decide(&call(&store("i"), "camera.view", Some("usb:1"))).unwrap_err();
    assert_eq!(e.code(), "revoked");
    assert_eq!(r.log.rows()[0].reason, "revoked");
}

#[test]
fn refusal_hard_ask_without_a_card_or_with_a_no() {
    let mut book = MemoryBook::default();
    book.put("i", record_approval("alice", "notes", "1", "ab", 1, &[req("microphone.listen", None)]));
    // a card that nobody can answer
    let log = Arc::new(MemoryLog::new(10));
    let b = Broker::new(Arc::new(book), Arc::new(NoCard), log.clone(), Arc::new(|| 5));
    assert_eq!(b.decide(&call(&store("i"), "microphone.listen", None)), Err(Denial::NoCard));
    assert_eq!(log.rows()[0].reason, "no_card");
    // a no
    let mut book = MemoryBook::default();
    book.put("i", record_approval("alice", "notes", "1", "ab", 1, &[req("microphone.listen", None)]));
    let r = rig(book, CardAnswer::Deny);
    assert_eq!(r.broker.decide(&call(&store("i"), "microphone.listen", None)), Err(Denial::CardDeclined));
    assert_eq!(r.asked.1.lock().unwrap().len(), 1);
    // built-in app that may ask: still a card each time
    let r = rig(MemoryBook::default(), CardAnswer::Deny);
    assert_eq!(r.broker.decide(&call(&builtin("cameras"), "camera.record", Some("usb:1"))), Err(Denial::CardDeclined));
    // an app that holds nothing is refused before any card is shown
    assert_eq!(r.broker.decide(&call(&store("none"), "screen.capture", None)), Err(Denial::NotGranted));
    assert_eq!(r.asked.1.lock().unwrap().len(), 1);
}

#[test]
fn a_hard_ask_with_a_yes_is_allowed_each_time_asked_each_time() {
    let mut book = MemoryBook::default();
    book.put("i", record_approval("alice", "notes", "1", "ab", 1, &[req("microphone.listen", None)]));
    let r = rig(book, CardAnswer::Allow);
    for _ in 0..3 {
        assert_eq!(r.broker.decide(&call(&store("i"), "microphone.listen", None)), Ok(()));
    }
    assert_eq!(r.asked.1.lock().unwrap().len(), 3, "the card is shown at every use");
}

#[test]
fn repeated_uses_are_one_log_line_per_window_and_the_log_is_bounded() {
    let r = rig(MemoryBook::default(), CardAnswer::Deny);
    let c = builtin("cameras");
    for _ in 0..50 {
        r.broker.decide(&call(&c, "camera.view", Some("usb:1"))).unwrap();
    }
    assert_eq!(r.log.rows().len(), 1, "ten pictures a second are one line");
    r.now.store(1000 + COALESCE_ALLOWED_SECS, Ordering::SeqCst);
    r.broker.decide(&call(&c, "camera.view", Some("usb:1"))).unwrap();
    r.broker.decide(&call(&c, "camera.view", Some("usb:2"))).unwrap();
    assert_eq!(r.log.rows().len(), 3);
    // a denial after allowed uses is its own line
    let _ = r.broker.decide(&call(&c, "camera.peek", None));
    assert_eq!(r.log.rows().len(), 4);
    let small = MemoryLog::new(3);
    for i in 0..10 {
        small.record(AccessEvent { at: i, app: "a".into(), person: "p".into(), device: "d".into(), capability: "c".into(), target: "t".into(), allowed: true, reason: String::new() });
    }
    assert_eq!(small.rows().iter().map(|e| e.at).collect::<Vec<_>>(), [7, 8, 9]);
}

#[test]
fn log_lines_hold_no_control_characters_and_round_trip_as_json() {
    let r = rig(MemoryBook::default(), CardAnswer::Deny);
    let c = builtin("cameras");
    r.broker.decide(&Call { caller: &c, capability: "camera.view", unit: None, label: "Front\ndoor\u{7}" }).unwrap();
    let e = r.log.rows().remove(0);
    assert_eq!(e.target, "Frontdoor");
    let j = serde_json::to_string(&e).unwrap();
    assert!(!j.contains('\n'));
    assert_eq!(serde_json::from_str::<AccessEvent>(&j).unwrap(), e);
    assert_eq!(Denial::NoCard.to_string(), "denied (no_card): this needs your answer on the screen, and there is no screen to ask on");
}

#[test]
fn the_label_is_only_worked_out_when_a_line_is_written() {
    let r = rig(MemoryBook::default(), CardAnswer::Deny);
    let c = builtin("cameras");
    let n = std::sync::atomic::AtomicUsize::new(0);
    for _ in 0..20 {
        let call = Call { caller: &c, capability: "camera.view", unit: Some("usb:1"), label: "" };
        r.broker
            .decide_with(&call, &|| {
                n.fetch_add(1, Ordering::SeqCst);
                "Front".into()
            })
            .unwrap();
    }
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_eq!(r.log.rows()[0].target, "Front");
}

fn client(id: &str, device: &str) -> Caller {
    Caller { app: "client".into(), kind: CallerKind::Client { device_id: id.into() }, person: "alice".into(), device: device.into() }
}

#[test]
fn a_client_device_holds_only_what_is_stored_under_its_key_and_the_log_names_the_device() {
    let mut book = MemoryBook::default();
    book.0.insert(client_key("d1"), record_approval("alice", "client", "1", "", 1, &[req("camera.view", None)]));
    let r = rig(book, CardAnswer::Allow);
    let c = client("d1", "Maya's laptop");
    assert_eq!(r.broker.decide(&call(&c, "camera.view", Some("usb:1"))), Ok(()));
    let rows = r.log.rows();
    assert_eq!((rows[0].app.as_str(), rows[0].device.as_str(), rows[0].person.as_str(), rows[0].allowed), ("client", "Maya's laptop", "alice", true));
    // another device, and a capability that was not given, are refused
    assert_eq!(r.broker.decide(&call(&client("d2", "Other"), "camera.view", None)), Err(Denial::NotGranted));
    assert_eq!(r.broker.decide(&call(&c, "camera.record", None)), Err(Denial::NotGranted));
}

#[test]
fn a_client_device_is_never_a_builtin_whatever_it_calls_itself() {
    let r = rig(MemoryBook::default(), CardAnswer::Allow);
    let mut c = client("d1", "x");
    c.app = "cameras".into(); // the name of a built-in app in the table
    assert_eq!(r.broker.decide(&call(&c, "camera.view", None)), Err(Denial::NotGranted));
}
