//! The real microphone and voice-engine adapters of `iyakki.speech.listen` (F-SDK-16) over fake recorders and fake voice backends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_shared::voicedesk::{Capture as RecCapture, Mic, Recorders, T2Mic};
use serde_json::json;

use super::*;
use crate::store::sdkv2_backends::{Backend, Who};
use crate::store::sdkv2_listen::ListenBackend;

struct Rec {
    samples: Vec<i16>,
    started: Mutex<Vec<(String, usize)>>,
    /// The recording stays alive until the caller stops it (a microphone that never ends).
    forever: bool,
}

struct Running {
    samples: Vec<i16>,
    forever: bool,
}

impl RecCapture for Running {
    fn level(&self) -> f32 {
        0.0
    }
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
    fn is_full(&self) -> bool {
        false
    }
    fn is_alive(&mut self) -> bool {
        self.forever
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        Ok(self.samples.clone())
    }
}

impl Recorders for Rec {
    fn mics(&self) -> Vec<Mic> {
        vec![Mic { id: "plughw:0,0".into(), label: "Built-in".into(), usb: false }, Mic { id: "plughw:1,0".into(), label: "USB PnP Sound Device".into(), usb: true }]
    }
    fn product(&self) -> String {
        "test".into()
    }
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn RecCapture>, String> {
        self.started.lock().unwrap().push((cmd.to_string(), max_secs));
        Ok(Box::new(Running { samples: self.samples.clone(), forever: self.forever }))
    }
}

fn capture(rec: Arc<Rec>, chosen: &str, t2: T2Mic) -> RecorderCapture {
    let chosen = chosen.to_string();
    RecorderCapture::new(rec, Arc::new(|| "arecord -q -f S16_LE -r 16000 -c 1 -t raw".to_string()), Arc::new(move || chosen.clone()), Arc::new(move || t2)).with_tick(Duration::ZERO)
}

fn rec(samples: Vec<i16>, forever: bool) -> Arc<Rec> {
    Arc::new(Rec { samples, started: Mutex::new(Vec::new()), forever })
}

#[test]
fn the_microphone_is_the_persons_choice_or_the_first_and_an_unknown_name_is_not_guessed() {
    let r = rec(vec![], false);
    assert_eq!(capture(r.clone(), "", T2Mic::Absent).microphone(None).unwrap(), "Built-in");
    assert_eq!(capture(r.clone(), "plughw:1,0", T2Mic::Absent).microphone(None).unwrap(), "USB PnP Sound Device");
    assert_eq!(capture(r.clone(), "", T2Mic::Off).microphone(Some("plughw:1,0")).unwrap(), "USB PnP Sound Device");
    assert!(capture(r.clone(), "", T2Mic::Absent).microphone(Some("plughw:9,9")).unwrap_err().contains("no microphone called"));
    struct None_;
    impl Recorders for None_ {
        fn mics(&self) -> Vec<Mic> {
            vec![]
        }
        fn product(&self) -> String {
            String::new()
        }
        fn start(&self, _: &str, _: usize) -> Result<Box<dyn RecCapture>, String> {
            Err("no".into())
        }
    }
    let c = RecorderCapture::new(Arc::new(None_), Arc::new(String::new), Arc::new(String::new), Arc::new(|| T2Mic::Absent));
    assert!(c.microphone(None).unwrap_err().contains("USB microphone"));
}

#[test]
fn a_t2_laptop_with_its_capture_streams_on_is_never_listened_through() {
    let r = rec(vec![1], false);
    let c = capture(r.clone(), "", T2Mic::On);
    assert!(c.microphone(None).unwrap_err().contains("not usable by apps"));
    assert!(c.record(None, 3, &AtomicBool::new(false)).is_err());
    assert!(r.started.lock().unwrap().is_empty(), "nothing was started");
}

#[test]
fn the_recording_is_aimed_at_the_microphone_bounded_by_the_seconds_and_stops_when_told() {
    let r = rec(vec![5, 6, 7], true);
    let c = capture(r.clone(), "plughw:1,0", T2Mic::Absent);
    // a recorder that never ends is stopped by the flag at once
    let stop = AtomicBool::new(true);
    assert_eq!(c.record(None, 5, &stop).unwrap(), vec![5, 6, 7]);
    let started = r.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert!(started[0].0.contains("-D") && started[0].0.contains("1,0"), "{}", started[0].0);
    assert_eq!(started[0].1, 5);
    // a recorder that ends by itself (a device that went away) returns what it has
    let r2 = rec(vec![9], false);
    assert_eq!(capture(r2, "", T2Mic::Absent).record(None, 2, &AtomicBool::new(false)).unwrap(), vec![9]);
}

struct Eng {
    kind: Kind,
    on: bool,
    answer: Result<String, String>,
}

impl VoiceBackend for Eng {
    fn id(&self) -> &str {
        "fake"
    }
    fn label(&self) -> String {
        "fake".into()
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn configured(&self) -> bool {
        self.on
    }
    fn transcribe(&self, _: &[u8], _: &str) -> Result<String, String> {
        self.answer.clone()
    }
}

fn recogniser(e: Option<Eng>) -> VoiceRecogniser {
    let e: Option<Arc<dyn VoiceBackend>> = e.map(|e| Arc::new(e) as Arc<dyn VoiceBackend>);
    VoiceRecogniser::new(Arc::new(move || e.clone()))
}

fn loud() -> Vec<i16> {
    vec![4000; 1600]
}

#[test]
fn only_an_engine_on_this_computer_is_used_and_a_missing_one_says_what_to_do() {
    assert!(recogniser(None).ready().unwrap_err().contains("Settings -> Voice"));
    assert!(recogniser(Some(Eng { kind: Kind::OnDevice, on: false, answer: Ok("x".into()) })).ready().is_err());
    for k in [Kind::Network, Kind::Cloud] {
        let e = recogniser(Some(Eng { kind: k, on: true, answer: Ok("x".into()) }));
        assert!(e.ready().unwrap_err().contains("stays on this computer"));
        assert!(e.transcribe(&loud(), None).is_err(), "no sound is sent anywhere else");
    }
    let ok = recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Ok("open the calendar".into()) }));
    assert!(ok.ready().is_ok());
    assert_eq!(ok.transcribe(&loud(), Some("en")).unwrap(), "open the calendar");
}

#[test]
fn nothing_heard_is_an_empty_text_and_a_failure_stays_a_failure() {
    let silent = recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Ok("hello".into()) }));
    assert_eq!(silent.transcribe(&[0; 1600], None).unwrap(), "", "a silent recording never reaches the engine");
    assert_eq!(silent.transcribe(&[], None).unwrap(), "");
    let blank = recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Ok(String::new()) }));
    assert_eq!(blank.transcribe(&loud(), None).unwrap(), "");
    let broken = recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Err("the model file is damaged".into()) }));
    assert!(broken.transcribe(&loud(), None).unwrap_err().contains("damaged"));
}

#[test]
fn the_real_adapters_run_behind_the_bridges_backend_and_name_the_microphone() {
    let r = rec(loud(), false);
    let be = ListenBackend::new(Arc::new(capture(r, "plughw:1,0", T2Mic::Absent)), Arc::new(recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Ok("hi".into()) }))));
    let who = Who { user: "tester".into(), app: "t".into() };
    let a = be.call(&who, "speech.available", &json!({})).unwrap();
    assert_eq!(a["microphone"], "USB PnP Sound Device");
    // with no engine nobody is asked to allow listening
    let be2 = ListenBackend::new(Arc::new(capture(rec(vec![], false), "", T2Mic::Absent)), Arc::new(recogniser(None)));
    let e = be2.call(&who, "speech.available", &json!({})).unwrap_err();
    assert!(format!("{e:?}").contains("no voice engine"), "{e:?}");
    let _ = Ordering::SeqCst;
}

#[test]
fn the_listening_backend_is_connected_to_the_sdk_only_once_the_sdk_is_shared() {
    use crate::store::tile::Outbox;
    use crate::store::tilesdk::TileSdk;
    let dir = std::env::temp_dir().join(format!("iyakki-listen-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sdk = TileSdk::fake("t", Arc::new(Mutex::new(Outbox::default())), &dir);
    let be = Arc::new(ListenBackend::new(Arc::new(capture(rec(loud(), false), "", T2Mic::Absent)), Arc::new(recogniser(Some(Eng { kind: Kind::OnDevice, on: true, answer: Ok("hi".into()) })))));
    sdk.bind_listener(be.clone());
    let who = Who { user: "tester".into(), app: "t".into() };
    let before = be.call(&who, "speech.listen", &json!({"id": 1, "max_seconds": 1})).unwrap_err();
    assert!(format!("{before:?}").contains("not connected"), "{before:?}");
    let sdk = Arc::new(sdk);
    sdk.shared();
    assert!(be.call(&who, "speech.listen", &json!({"id": 1, "max_seconds": 1})).is_ok(), "connected now");
    sdk.shared(); // a second call does nothing
    let _ = std::fs::remove_dir_all(&dir);
}
