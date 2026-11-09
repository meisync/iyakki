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
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("sending sound to Bluetooth headphones is not available yet"));
}

// ---- a phone's Bluetooth tethering

use iyakki_devctl::bt_pan::{Tether, TetherState, TetherView};

struct FakeTether {
    view: Mutex<TetherView>,
    /// what the state becomes after connect
    next: TetherState,
    refuse: Option<String>,
    calls: Mutex<Vec<String>>,
}

impl Tether for FakeTether {
    fn view(&self) -> TetherView {
        self.view.lock().unwrap().clone()
    }
    fn connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("connect {a}"));
        if let Some(e) = &self.refuse {
            return Err(e.clone());
        }
        *self.view.lock().unwrap() = TetherView { address: a.into(), name: "Pixel".into(), state: self.next.clone() };
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("disconnect".into());
        self.view.lock().unwrap().state = TetherState::Idle;
        Ok(())
    }
}

fn tether_rig(next: TetherState, refuse: Option<&str>, phones: usize) -> (BluetoothTools, Arc<FakeTether>) {
    let mut devs = vec![view("AA:BB:CC:DD:EE:01", "K380", "keyboard")];
    for i in 0..phones {
        devs.push(view(&format!("AA:BB:CC:DD:FF:0{i}"), &format!("Pixel{i}"), "phone"));
    }
    let f = Arc::new(Fake { devs: Mutex::new(devs), calls: Mutex::new(vec![]), outcome: LinkState::Connected });
    let t = Arc::new(FakeTether { view: Mutex::new(TetherView::idle()), next, refuse: refuse.map(String::from), calls: Mutex::new(vec![]) });
    (BluetoothTools::new(Arc::new(Base), f).with_tether(t.clone()).with_wait(Duration::from_millis(30), Duration::from_millis(5)), t)
}

#[test]
fn the_tether_tools_exist_status_is_free_and_connect_and_disconnect_ask_every_time() {
    let (t, _) = tether_rig(TetherState::Connected, None, 1);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk("bluetooth_tether_status"), Risk::Safe);
    for n in ["bluetooth_tether_connect", "bluetooth_tether_disconnect"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert!(PROMPT_RULE.contains("bluetooth_tether_connect") && PROMPT_RULE.contains("data"));
    let card = t.preview("bluetooth_tether_connect", &json!({})).unwrap();
    assert!(card.contains("Pixel0") && card.contains("data plan") && card.contains("switched on in the phone"), "{card}");
}

#[test]
fn connect_reports_the_result_and_a_failure_comes_back_with_the_phones_reason() {
    let (t, f) = tether_rig(TetherState::Connected, None, 1);
    let r = t.call("bluetooth_tether_connect", &json!({})).unwrap();
    assert!(r.contains("Connected") && r.contains("Pixel0"), "{r}");
    assert_eq!(f.calls.lock().unwrap().clone(), vec!["connect AA:BB:CC:DD:FF:00"]);
    assert!(t.call("bluetooth_tether_status", &json!({})).unwrap().contains("connected: this computer uses the internet of Pixel"));
    t.call("bluetooth_tether_disconnect", &json!({})).unwrap();
    assert_eq!(f.calls.lock().unwrap().last().unwrap(), "disconnect");
    let (t, _) = tether_rig(TetherState::Failed("The phone refused the network connection. Switch on Bluetooth tethering in the phone's settings.".into()), None, 1);
    let e = t.call("bluetooth_tether_connect", &json!({})).unwrap_err();
    assert!(e.contains("did not connect") && e.contains("Switch on Bluetooth tethering"), "{e}");
    assert!(t.call("bluetooth_tether_status", &json!({})).unwrap().contains("the last try failed"));
    let (t, _) = tether_rig(TetherState::Connecting, None, 1);
    assert!(t.call("bluetooth_tether_connect", &json!({})).unwrap().contains("Still connecting"));
}

#[test]
fn a_refusal_found_at_once_is_returned_and_no_phone_or_two_phones_are_said_plainly() {
    let (t, _) = tether_rig(TetherState::Connected, Some("This computer has no Bluetooth controller."), 1);
    assert_eq!(t.call("bluetooth_tether_connect", &json!({})).unwrap_err(), "This computer has no Bluetooth controller.");
    let (t, f) = tether_rig(TetherState::Connected, None, 0);
    let e = t.call("bluetooth_tether_connect", &json!({})).unwrap_err();
    assert!(e.contains("no phone is paired") && f.calls.lock().unwrap().is_empty(), "{e}");
    assert!(t.refusal("bluetooth_tether_connect", &json!({})).is_some(), "refused before a card is shown");
    let (t, _) = tether_rig(TetherState::Connected, None, 2);
    assert!(t.call("bluetooth_tether_connect", &json!({})).unwrap_err().contains("more than one phone"));
    assert!(t.call("bluetooth_tether_connect", &json!({"phone": "pixel1"})).is_ok(), "a name picks one");
    assert!(t.call("bluetooth_tether_connect", &json!({"phone": "K380"})).unwrap_err().contains("no such paired"), "a keyboard is not a phone");
}
