//! The pairing tools against a fake controller: scanning and pairing ask, status is free, a number the other device shows is never
//! taken through the chat, and the states come back as plain instructions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::bluetoothpair::*;
use iyakki_devctl::btmgmt::{Found, Kind};
use iyakki_devctl::bthid::PairState;

struct Base;
impl Tools for Base {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.to_string()
    }
}

struct Fake {
    found: Vec<Found>,
    /// What the state becomes after `start`, and after a yes.
    after_start: PairState,
    after_yes: PairState,
    state: Mutex<PairState>,
    log: Mutex<Vec<String>>,
    scan_err: Option<String>,
}

impl PairControl for Fake {
    fn scan(&self, secs: u64) -> Result<Vec<Found>, String> {
        self.log.lock().unwrap().push(format!("scan {secs}"));
        match &self.scan_err {
            Some(e) => Err(e.clone()),
            None => Ok(self.found.clone()),
        }
    }
    fn start(&self, f: &Found) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("start {}", f.address));
        *self.state.lock().unwrap() = self.after_start.clone();
        Ok(())
    }
    fn state(&self) -> PairState {
        self.state.lock().unwrap().clone()
    }
    fn answer(&self, yes: bool) -> bool {
        self.log.lock().unwrap().push(format!("answer {yes}"));
        let mut s = self.state.lock().unwrap();
        if !matches!(*s, PairState::Confirm(..) | PairState::ConfirmNoNumber(_)) {
            return false;
        }
        *s = if yes { self.after_yes.clone() } else { PairState::Done(Err("cancelled".into())) };
        true
    }
}

fn found(a: &str, n: &str, k: Kind, rssi: i8) -> Found {
    Found { address: a.into(), addr_type: 0, rssi, name: n.into(), class: None, appearance: None, services: vec![], kind: k }
}

fn rig(after_start: PairState, after_yes: PairState) -> (BluetoothPairTools, Arc<Fake>) {
    let f = Arc::new(Fake {
        found: vec![
            found("AA:00:00:00:00:01", "Buds", Kind::Headphones, -40),
            found("AA:00:00:00:00:02", "", Kind::Other, -30),
            found("AA:00:00:00:00:03", "K380", Kind::Keyboard, -60),
            found("AA:00:00:00:00:04", "Pixel", Kind::Phone, -70),
            found("AA:00:00:00:00:05", "Pixel", Kind::Phone, -75),
        ],
        after_start,
        after_yes,
        state: Mutex::new(PairState::Idle),
        log: Mutex::new(vec![]),
        scan_err: None,
    });
    (BluetoothPairTools::new(Arc::new(Base), f.clone()).with_wait(Duration::from_millis(30), Duration::from_millis(5)), f)
}

fn scanned(t: &BluetoothPairTools) {
    t.call(SCAN, &json!({})).unwrap();
}

#[test]
fn scanning_pairing_and_answering_ask_and_the_status_is_free() {
    let (t, _) = rig(PairState::Idle, PairState::Idle);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(PAIR_STATUS), Risk::Safe);
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert!(t.grant_blocker(PAIR, &json!({})).is_some() && t.grant_blocker(PAIR_ANSWER, &json!({})).is_some());
    // no tool takes digits
    for d in &defs {
        let props = d.spec.schema.get("properties").cloned().unwrap_or(json!({}));
        assert!(props.get("passkey").is_none() && props.get("pin").is_none() && props.get("code").is_none(), "{}", d.spec.name);
    }
}

#[test]
fn the_scan_lists_pairable_named_strong_devices_first_and_says_what_cannot_be_paired() {
    let (t, f) = rig(PairState::Idle, PairState::Idle);
    let out = t.call(SCAN, &json!({})).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines[0].starts_with("K380") || lines[0].starts_with("Pixel"), "{out}");
    assert!(lines[0..3].iter().all(|l| !l.contains("cannot be paired")), "{out}");
    assert!(out.contains("Buds · headphones") && out.contains("cannot be paired in this version"));
    assert!(lines[4].contains("AA:00:00:00:00:02") || lines[3].contains("AA:00:00:00:00:02"), "{out}");
    assert_eq!(f.log.lock().unwrap()[0], "scan 8");
    assert!(scan_text(&[]).contains("pairing mode"));
    let many: Vec<Found> = (0..20).map(|i| found(&format!("AA:00:00:00:01:{i:02}"), "X", Kind::Mouse, -50)).collect();
    assert!(scan_text(&many).ends_with("... and 5 more"));
}

#[test]
fn a_scan_failure_comes_back_with_its_reason_and_pairing_needs_a_scan_first() {
    let f2 = Arc::new(Fake { scan_err: Some("Bluetooth is off".into()), found: vec![], after_start: PairState::Idle, after_yes: PairState::Idle, state: Mutex::new(PairState::Idle), log: Mutex::new(vec![]) });
    let t = BluetoothPairTools::new(Arc::new(Base), f2);
    assert_eq!(t.call(SCAN, &json!({})), Err("Bluetooth is off".into()));
    assert!(t.refusal(PAIR, &json!({"device": "K380"})).unwrap().contains("scan first"));
    let (t, _) = rig(PairState::Idle, PairState::Idle);
    assert!(t.refusal(PAIR, &json!({"device": "K380"})).unwrap().contains("scan first"));
    scanned(&t);
    assert!(t.refusal(PAIR, &json!({})).unwrap().contains("name the device"));
    assert!(t.refusal(PAIR, &json!({"device": "Nobody"})).unwrap().contains("did not find"));
    assert!(t.refusal(PAIR, &json!({"device": "Pixel"})).unwrap().contains("more than one"));
    assert!(t.refusal(PAIR, &json!({"device": "Buds"})).unwrap().contains("headphones"));
    assert!(t.refusal(PAIR, &json!({"device": "K380"})).is_none());
    assert!(t.refusal(PAIR, &json!({"device": "aa:00:00:00:00:04"})).is_none(), "an address matches without regard to case");
}

#[test]
fn the_card_names_the_device_and_says_the_person_must_be_there() {
    let (t, _) = rig(PairState::Idle, PairState::Idle);
    scanned(&t);
    let card = t.preview(PAIR, &json!({"device": "K380"})).unwrap();
    assert!(card.contains("keyboard") && card.contains("K380") && card.contains("AA:00:00:00:00:03") && card.contains("at the device"), "{card}");
    assert!(t.describe(PAIR, &json!({"device": "K380"})).starts_with("Pair this computer"));
    assert!(t.preview(SCAN, &json!({})).unwrap().contains("Nothing is paired"));
    assert!(t.preview(PAIR_STATUS, &json!({})).is_none());
}

#[test]
fn a_number_to_compare_is_given_to_the_person_and_the_answer_finishes_the_pairing() {
    let (t, f) = rig(PairState::Confirm("K380".into(), 42), PairState::Done(Ok("K380 is paired".into())));
    scanned(&t);
    let out = t.call(PAIR, &json!({"device": "K380"})).unwrap();
    assert!(out.contains("000042") && out.contains("bluetooth_pair_answer"), "{out}");
    assert!(t.call(PAIR_STATUS, &json!({})).unwrap().contains("000042"));
    // the card for the answer says what a yes means; nothing waits after it is used
    assert!(t.preview(PAIR_ANSWER, &json!({"yes": true})).unwrap().contains("YES"));
    assert!(t.preview(PAIR_ANSWER, &json!({"yes": false})).unwrap().contains("NO"));
    assert!(t.refusal(PAIR_ANSWER, &json!({})).unwrap().contains("yes (true) or no (false)"));
    let out = t.call(PAIR_ANSWER, &json!({"yes": true})).unwrap();
    assert_eq!(out, "Paired: K380 is paired");
    assert_eq!(f.log.lock().unwrap().last().unwrap(), "answer true");
    assert!(t.refusal(PAIR_ANSWER, &json!({"yes": true})).unwrap().contains("no pairing waits"));
    assert!(t.call(PAIR_ANSWER, &json!({"yes": true})).is_err());
}

#[test]
fn a_no_cancels_and_a_failed_pairing_is_an_error_with_the_reason() {
    let (t, _) = rig(PairState::ConfirmNoNumber("Pixel".into()), PairState::Idle);
    scanned(&t);
    let out = t.call(PAIR, &json!({"device": "aa:00:00:00:00:04"})).unwrap();
    assert!(out.contains("shows no number"), "{out}");
    assert_eq!(t.call(PAIR_ANSWER, &json!({"yes": false})).unwrap(), "Answered no: the pairing is cancelled.");
    assert_eq!(t.call(PAIR_STATUS, &json!({})).unwrap(), "Pairing failed: cancelled");
    let (t, _) = rig(PairState::Done(Err("the device refused".into())), PairState::Idle);
    scanned(&t);
    let e = t.call(PAIR, &json!({"device": "K380"})).unwrap_err();
    assert_eq!(e, "pairing with K380 failed: the device refused");
}

#[test]
fn a_number_the_other_device_shows_is_typed_in_settings_and_never_repeated_through_the_chat() {
    let (t, _) = rig(PairState::TypePasskey("Pixel".into()), PairState::Idle);
    scanned(&t);
    let out = t.call(PAIR, &json!({"device": "aa:00:00:00:00:04"})).unwrap();
    assert!(out.contains("Settings -> Bluetooth") && out.contains("must not go through the chat"), "{out}");
    let (t, _) = rig(PairState::TypePin("K380".into(), true), PairState::Idle);
    scanned(&t);
    assert!(t.call(PAIR, &json!({"device": "K380"})).unwrap().contains("PIN"));
    // numbers the computer shows are told to the person
    assert!(state_text(&PairState::ShowPasskey("K380".into(), 7)).contains("000007"));
    assert!(state_text(&PairState::ShowPin("K380".into(), "0000".into())).contains("0000"));
}

#[test]
fn a_pairing_that_is_still_running_says_so_and_idle_states_are_plain() {
    let (t, _) = rig(PairState::Running("K380".into()), PairState::Idle);
    scanned(&t);
    let out = t.call(PAIR, &json!({"device": "K380"})).unwrap();
    assert!(out.starts_with("Still pairing with K380"), "{out}");
    assert_eq!(state_text(&PairState::Idle), "No pairing is running.");
    assert!(state_text(&PairState::Running("X".into())).contains("in progress"));
    assert!(PROMPT_RULE.contains("must not take the digits"));
    // other tools pass through to the inner set
    assert!(t.call("other", &json!({})).is_err());
    assert!(t.pre_approved("other") == Base.pre_approved("other"));
}
