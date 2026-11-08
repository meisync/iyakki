//! The assistant's Bluetooth tools against a fake list of devices: the list is free, changes ask, forgetting asks every time, a failed
//! connection comes back with its plain reason, and an unknown or ambiguous name is refused before anything is asked.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{hard_ask, Risk, ToolDef, Tools};
use crate::bluetoothtools::*;
use iyakki_devctl::bthid::{DeviceView, Devices, LinkState};

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
    devs: Mutex<Vec<DeviceView>>,
    calls: Mutex<Vec<String>>,
    /// what the connection turns into after connect
    outcome: LinkState,
}

impl Devices for Fake {
    fn list(&self) -> Vec<DeviceView> {
        self.devs.lock().unwrap().clone()
    }
    fn connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("connect {a}"));
        self.devs.lock().unwrap().iter_mut().for_each(|d| d.state = self.outcome.clone());
        Ok(())
    }
    fn disconnect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("disconnect {a}"));
        Ok(())
    }
    fn forget(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("forget {a}"));
        self.devs.lock().unwrap().clear();
        Ok(())
    }
}

fn view(a: &str, n: &str, kind: &str) -> DeviceView {
    DeviceView { address: a.into(), addr_type: 0, name: n.into(), kind: kind.into(), state: LinkState::Disconnected }
}

fn rig(outcome: LinkState) -> (BluetoothTools, Arc<Fake>) {
    let f = Arc::new(Fake { devs: Mutex::new(vec![view("AA:BB:CC:DD:EE:01", "K380", "keyboard"), view("AA:BB:CC:DD:EE:02", "Mouse", "mouse"), view("AA:BB:CC:DD:EE:03", "Mouse", "mouse")]), calls: Mutex::new(vec![]), outcome });
    (BluetoothTools::new(Arc::new(Base), f.clone()).with_wait(Duration::from_millis(30), Duration::from_millis(5)), f)
}

#[test]
fn the_list_is_free_the_changes_ask_and_forgetting_asks_every_time() {
    let (t, _f) = rig(LinkState::Connected);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(READ_TOOL), Risk::Safe);
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert!(t.grant_blocker("bluetooth_forget", &json!({})).is_some());
    assert!(hard_ask("bluetooth_forget", &json!({})).is_some(), "forget is on the hard-ask list");
    assert!(PROMPT_RULE.contains("not available yet"));
    let l = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(l.contains("K380 · keyboard · AA:BB:CC:DD:EE:01 · paired, not connected"), "{l}");
}

#[test]
fn connect_reports_the_result_or_the_reason_and_the_card_names_the_device() {
    let (t, f) = rig(LinkState::Connected);
    let card = t.preview("bluetooth_connect", &json!({"device": "k380"})).unwrap();
    assert!(card.contains("“K380” (keyboard, AA:BB:CC:DD:EE:01)"), "{card}");
    assert_eq!(t.describe("bluetooth_connect", &json!({"device": "k380"})), "Connect the Bluetooth “K380” (keyboard, AA");
    assert!(t.call("bluetooth_connect", &json!({"device": "k380"})).unwrap().contains("works as a keyboard"));
    assert_eq!(f.calls.lock().unwrap().clone(), vec!["connect AA:BB:CC:DD:EE:01"]);
    let (t, _f) = rig(LinkState::Failed("the device did not answer: switch it on".into()));
    let e = t.call("bluetooth_connect", &json!({"device": "AA:BB:CC:DD:EE:01"})).unwrap_err();
    assert!(e.contains("K380 did not connect: the device did not answer: switch it on"), "{e}");
    let (t, _f) = rig(LinkState::Connecting);
    assert!(t.call("bluetooth_connect", &json!({"device": "k380"})).unwrap().contains("Still connecting"));
}

#[test]
fn disconnect_and_forget_act_on_the_named_device_and_unknown_or_ambiguous_names_are_refused_first() {
    let (t, f) = rig(LinkState::Connected);
    t.call("bluetooth_disconnect", &json!({"device": "K380"})).unwrap();
    assert!(t.preview("bluetooth_forget", &json!({"device": "K380"})).unwrap().starts_with("FORGET the Bluetooth “K380”"));
    assert!(t.refusal("bluetooth_forget", &json!({"device": "AA:BB:CC:DD:EE:02"})).is_none());
    t.call("bluetooth_forget", &json!({"device": "K380"})).unwrap();
    assert_eq!(f.calls.lock().unwrap().clone(), vec!["disconnect AA:BB:CC:DD:EE:01", "forget AA:BB:CC:DD:EE:01"]);
    f.devs.lock().unwrap().extend([view("AA:BB:CC:DD:EE:02", "Mouse", "mouse"), view("AA:BB:CC:DD:EE:03", "Mouse", "mouse")]);
    for bad in [json!({}), json!({"device": "nope"}), json!({"device": "mouse"})] {
        assert!(t.refusal("bluetooth_forget", &bad).is_some(), "{bad}");
    }
    let (t, f) = rig(LinkState::Connected);
    f.devs.lock().unwrap().clear();
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("Bluetooth audio is not available yet"));
}
