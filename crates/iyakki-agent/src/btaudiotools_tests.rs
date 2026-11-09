//! The Bluetooth audio tools against a fake audio control: the status is free, the changes ask, the card names the device, and a
//! connection that does not happen comes back with its reason.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::btaudiotools::*;
use iyakki_devctl::a2dp::{AudioControl, AudioDeviceView, AudioState, AudioView};
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

struct Devs(Vec<DeviceView>);
impl Devices for Devs {
    fn list(&self) -> Vec<DeviceView> {
        self.0.clone()
    }
    fn connect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn forget(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct Audio {
    view: Mutex<AudioView>,
    /// what the view becomes after connect
    after: Option<AudioDeviceView>,
    calls: Mutex<Vec<String>>,
}

impl AudioControl for Audio {
    fn view(&self) -> AudioView {
        self.view.lock().unwrap().clone()
    }
    fn connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("connect {a}"));
        if let Some(d) = self.after.clone() {
            self.view.lock().unwrap().device = Some(d);
        }
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("disconnect".into());
        self.view.lock().unwrap().device = None;
        Ok(())
    }
    fn pairing_open(&self, on: bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("pairing {on}"));
        self.view.lock().unwrap().pairing_open = on;
        Ok(())
    }
}

fn dev(a: &str, n: &str, kind: &str) -> DeviceView {
    DeviceView { address: a.into(), addr_type: 0, name: n.into(), kind: kind.into(), state: LinkState::Disconnected }
}

fn playing(a: &str, n: &str) -> AudioDeviceView {
    AudioDeviceView { address: a.into(), name: n.into(), state: AudioState::Playing, codec: "SBC 44.1 kHz joint stereo".into(), error: None, packets: 120, lost: 2 }
}

fn rig(devs: Vec<DeviceView>, after: Option<AudioDeviceView>, ready: bool) -> (BtAudioTools, Arc<Audio>) {
    let a = Arc::new(Audio { view: Mutex::new(AudioView { ready, why_not: if ready { String::new() } else { "the Bluetooth audio channels could not be opened".into() }, ..Default::default() }), after, calls: Mutex::new(vec![]) });
    (BtAudioTools::new(Arc::new(Base), Arc::new(Devs(devs)), a.clone()).with_wait(Duration::from_millis(30), Duration::from_millis(5)), a)
}

#[test]
fn the_status_is_free_and_connect_disconnect_and_pairing_each_ask() {
    let (t, _) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], None, true);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(STATUS_TOOL), Risk::Safe);
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    assert!(PROMPT_RULE.contains("cannot send sound to headphones"));
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("ready"));
}

#[test]
fn the_status_names_the_device_the_codec_and_the_packets_or_says_why_nothing_works() {
    let (t, a) = rig(vec![], None, true);
    a.view.lock().unwrap().device = Some(playing("AA:BB:CC:DD:EE:01", "Pixel 8"));
    let s = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(s.contains("playing from Pixel 8") && s.contains("SBC 44.1 kHz joint stereo") && s.contains("lost 2"), "{s}");
    let (t, _) = rig(vec![], None, false);
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("could not be opened"));
}

#[test]
fn connect_waits_for_the_phone_and_reports_playing_or_the_reason() {
    let (t, a) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], Some(playing("AA:BB:CC:DD:EE:01", "Pixel 8")), true);
    let card = t.preview("bluetooth_audio_connect", &json!({})).unwrap();
    assert!(card.contains("“Pixel 8” (AA:BB:CC:DD:EE:01)"), "{card}");
    let r = t.call("bluetooth_audio_connect", &json!({"device": "pixel 8"})).unwrap();
    assert!(r.contains("Pixel 8 is playing"), "{r}");
    assert_eq!(a.calls.lock().unwrap().clone(), vec!["connect AA:BB:CC:DD:EE:01"]);
    // the phone never shows up
    let (t, _) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], None, true);
    assert!(t.call("bluetooth_audio_connect", &json!({})).unwrap().contains("Still connecting"));
    // the sink could not start
    let (t, _) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], None, false);
    assert!(t.call("bluetooth_audio_connect", &json!({})).unwrap_err().contains("did not connect"));
}

#[test]
fn no_phone_or_two_phones_are_refused_before_anything_is_asked() {
    let (t, _) = rig(vec![], None, true);
    assert!(t.refusal("bluetooth_audio_connect", &json!({})).unwrap().contains("no phone or laptop is paired"));
    let (t, _) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone"), dev("AA:BB:CC:DD:EE:02", "Galaxy", "phone"), dev("AA:BB:CC:DD:EE:03", "K380", "keyboard")], None, true);
    assert!(t.refusal("bluetooth_audio_connect", &json!({})).unwrap().contains("more than one"));
    assert!(t.refusal("bluetooth_audio_connect", &json!({"device": "galaxy"})).is_none());
    assert!(t.refusal("bluetooth_audio_connect", &json!({"device": "k380"})).is_some(), "a keyboard is not an audio source");
    assert!(t.refusal("bluetooth_audio_disconnect", &json!({})).unwrap().contains("no phone or laptop is connected"));
}

#[test]
fn disconnect_and_pairing_act_and_the_pairing_card_says_who_answers_the_number() {
    let (t, a) = rig(vec![], None, true);
    a.view.lock().unwrap().device = Some(playing("AA:BB:CC:DD:EE:01", "Pixel 8"));
    assert!(t.preview("bluetooth_audio_disconnect", &json!({})).unwrap().contains("Pixel 8"));
    t.call("bluetooth_audio_disconnect", &json!({})).unwrap();
    let card = t.preview("bluetooth_audio_pairing", &json!({})).unwrap();
    assert!(card.contains("two minutes") && card.contains("number comparison"), "{card}");
    assert!(t.preview("bluetooth_audio_pairing", &json!({"on": false})).unwrap().contains("Close pairing"));
    t.call("bluetooth_audio_pairing", &json!({})).unwrap();
    t.call("bluetooth_audio_pairing", &json!({"on": false})).unwrap();
    assert_eq!(a.calls.lock().unwrap().clone(), vec!["disconnect", "pairing true", "pairing false"]);
}
