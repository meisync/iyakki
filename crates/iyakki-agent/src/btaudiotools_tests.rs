//! The Bluetooth audio tools against a fake audio control: the status is free, the changes ask, the card names the device, and a
//! connection that does not happen comes back with its reason.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::btaudiotools::*;
use iyakki_core_local::devctl::a2dp::{AudioControl, AudioDeviceView, AudioState, AudioView};
use iyakki_core_local::devctl::bthid::{DeviceView, Devices, LinkState};

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
    assert!(PROMPT_RULE.contains("bluetooth_audio_send_start") && PROMPT_RULE.contains("fake speaker only"));
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

#[test]
fn the_keys_of_a_phone_change_the_volume_and_the_player_only_when_something_can_change_and_clearing_stops_them() {
    use iyakki_core_local::devctl::avrcp::{Act, MediaKey};
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let dir = std::env::temp_dir().join(format!("iyakki-btremote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = crate::services::Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
    svc.master_set(40).unwrap();
    assert_eq!(do_remote(&svc, Act::Step(5)), "volume 45");
    assert_eq!(do_remote(&svc, Act::Step(-100)), "volume 0");
    assert_eq!(do_remote(&svc, Act::SetPercent(60)), "volume 60");
    assert_eq!(do_remote(&svc, Act::ToggleMute), "muted");
    assert_eq!(svc.master().percent, 60);
    // nothing plays: the player keys take nothing (and say nothing)
    for k in [MediaKey::Play, MediaKey::Pause, MediaKey::Stop] {
        assert_eq!(do_remote(&svc, Act::Media(k)), "", "{k:?}");
    }
    // next and previous step the shared queue: nothing without an active queue, then one track at a time, never past either end
    {
        use crate::media::tests::FakeLauncher;
        let l = Arc::new(FakeLauncher::default());
        let p = crate::media::MediaPlayer::new(l.clone());
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "");
        p.set_queue("alice", vec!["/m/a.mp3".into(), "/m/b.mp3".into(), "/m/c.mp3".into()], 0, false);
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "", "an inactive queue is left alone");
        p.set_queue("alice", vec!["/m/a.mp3".into(), "/m/b.mp3".into(), "/m/c.mp3".into()], 0, true);
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Previous)), "", "no track before the first");
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "next b.mp3");
        assert_eq!(l.launched.lock().unwrap().as_slice(), ["/m/b.mp3"]);
        assert_eq!((p.queue_state().pos, p.queue_state().active, p.now_playing().as_deref()), (1, true, Some("b.mp3")));
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "next c.mp3");
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "", "no track after the last");
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Previous)), "previous b.mp3");
        assert_eq!(l.launched.lock().unwrap().as_slice(), ["/m/b.mp3", "/m/c.mp3", "/m/b.mp3"]);
        assert!(l.procs.lock().unwrap()[0].lock().unwrap().1 && l.procs.lock().unwrap()[1].lock().unwrap().1, "each step stops the one before");
        *l.fail.lock().unwrap() = true;
        assert_eq!(do_remote_with(&svc, &p, Act::Media(MediaKey::Next)), "", "a player that cannot start says nothing was done");
        assert_eq!(p.queue_state().pos, 1, "and the queue stays where it was");
    }
    // the handler follows the session: with it installed a key reaches the services, cleared it does nothing
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    install_remote(&svc, "alice", move |who, what| s2.lock().unwrap().push(format!("{who}: {what}")));
    iyakki_core_local::devctl::a2dp_sys::run_remote_for_test(Act::SetPercent(30));
    assert_eq!(svc.master().percent, 30);
    assert_eq!(seen.lock().unwrap().as_slice(), ["alice: bluetooth.remote volume 30"]);
    clear_remote();
    iyakki_core_local::devctl::a2dp_sys::run_remote_for_test(Act::SetPercent(90));
    assert_eq!(svc.master().percent, 30, "after sign-out the keys change nothing");
}

// ---------------------------------------------------------------------------------------------- sending sound to headphones

use iyakki_core_local::devctl::a2dp_send::{SendControl, SendView};

#[derive(Default)]
struct FakeSend {
    live: Mutex<Option<String>>,
    samples: Mutex<usize>,
    refuse: Option<String>,
}

impl SendControl for FakeSend {
    fn view(&self) -> SendView {
        match self.live.lock().unwrap().clone() {
            Some(d) => SendView { active: true, device: d, codec: "SBC 44.1 kHz joint stereo".into(), packets: 3, delay_ms: None, why: String::new() },
            None => SendView::default(),
        }
    }
    fn start(&self, _a: &str, name: &str) -> Result<(), String> {
        if let Some(e) = &self.refuse {
            return Err(e.clone());
        }
        *self.live.lock().unwrap() = Some(name.to_string());
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.live.lock().unwrap().take().map(|_| ()).ok_or("no sound is being sent to a device".into())
    }
    fn write(&self, pcm: &[i16], rate: u32, ch: usize) -> Result<(), String> {
        assert_eq!((rate, ch), (48_000, 2));
        *self.samples.lock().unwrap() += pcm.len();
        Ok(())
    }
}

fn send_rig(devs: Vec<DeviceView>, send: Arc<FakeSend>) -> BtAudioTools {
    let (t, _) = rig(devs, None, true);
    t.with_send(send)
}

#[test]
fn the_send_tools_ask_except_the_status_and_name_the_headphones_on_the_card() {
    let s = Arc::new(FakeSend::default());
    let t = send_rig(vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones"), dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], s.clone());
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(SEND_STATUS_TOOL), Risk::Safe);
    for n in SEND_TOOLS {
        assert_eq!(risk(n), Risk::Ask, "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
    }
    let card = t.preview("bluetooth_audio_send_start", &json!({})).unwrap();
    assert!(card.contains("“Buds” (AA:BB:CC:DD:EE:05)") && card.contains("SBC"), "{card}");
    // stop and the tone have nothing to act on yet
    assert!(t.refusal("bluetooth_audio_send_stop", &json!({})).is_some());
    assert!(t.refusal("bluetooth_audio_send_tone", &json!({})).unwrap().contains("start sending"));
    assert!(t.call(SEND_STATUS_TOOL, &json!({})).unwrap().contains("not sending"));
}

#[test]
fn start_tone_and_stop_run_in_order_and_the_tone_is_two_seconds_of_stereo_sound() {
    let s = Arc::new(FakeSend::default());
    let t = send_rig(vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones")], s.clone());
    let r = t.call("bluetooth_audio_send_start", &json!({"device": "buds"})).unwrap();
    assert!(r.contains("Buds") && r.contains("SBC"), "{r}");
    assert!(t.call(SEND_STATUS_TOOL, &json!({})).unwrap().contains("sending sound to Buds"));
    assert!(t.preview("bluetooth_audio_send_tone", &json!({})).unwrap().contains("two second"));
    t.call("bluetooth_audio_send_tone", &json!({})).unwrap();
    assert_eq!(*s.samples.lock().unwrap(), 2 * 96_000);
    assert!(t.preview("bluetooth_audio_send_stop", &json!({})).unwrap().contains("Buds"));
    t.call("bluetooth_audio_send_stop", &json!({})).unwrap();
    assert!(t.call("bluetooth_audio_send_stop", &json!({})).is_err());
    let tone = test_tone();
    assert_eq!(tone.len(), 192_000);
    assert!(tone.iter().map(|v| v.unsigned_abs()).max().unwrap() <= 6000 && tone[..200].iter().all(|v| v.unsigned_abs() < 600));
}

#[test]
fn no_headphones_two_headphones_and_a_refusing_speaker_each_say_why() {
    let t = send_rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], Arc::new(FakeSend::default()));
    assert!(t.refusal("bluetooth_audio_send_start", &json!({})).unwrap().contains("no headphones or speaker are paired"));
    let two = vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones"), dev("AA:BB:CC:DD:EE:06", "Boom", "speaker")];
    let t = send_rig(two.clone(), Arc::new(FakeSend::default()));
    assert!(t.refusal("bluetooth_audio_send_start", &json!({})).unwrap().contains("more than one"));
    assert!(t.call("bluetooth_audio_send_start", &json!({"device": "boom"})).is_ok());
    let t = send_rig(two, Arc::new(FakeSend { refuse: Some("the device only takes AAC, which this version cannot send; it needs SBC".into()), ..Default::default() }));
    assert!(t.call("bluetooth_audio_send_start", &json!({"device": "buds"})).unwrap_err().contains("needs SBC"));
    // without a sending side the tools say so instead of failing oddly
    let (t, _) = rig(vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones")], None, true);
    assert!(t.call("bluetooth_audio_send_start", &json!({})).unwrap_err().contains("not available here"));
}

#[test]
fn the_last_headphones_are_remembered_to_break_the_tie_between_two_paired_sinks_and_stopping_forgets_them() {
    let dir = std::env::temp_dir().join(format!("iyakki-btmem-{}-remember", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bluetooth-sink.json");
    let two = vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones"), dev("AA:BB:CC:DD:EE:06", "Boom", "speaker")];
    let t = send_rig(two.clone(), Arc::new(FakeSend::default())).with_memory(file.clone());
    assert!(t.refusal("bluetooth_audio_send_start", &json!({})).unwrap().contains("more than one"));
    t.call("bluetooth_audio_send_start", &json!({"device": "boom", "mute_speakers": false})).unwrap();
    assert_eq!(t.remembered(), Some(("AA:BB:CC:DD:EE:06".into(), false)));
    // a new process (a new tool object) meant the same speaker without being told
    let again = send_rig(two, Arc::new(FakeSend::default())).with_memory(file.clone());
    assert!(again.preview("bluetooth_audio_send_start", &json!({})).unwrap().contains("“Boom”"));
    t.call("bluetooth_audio_send_stop", &json!({})).unwrap();
    assert!(t.remembered().is_none() && !file.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
