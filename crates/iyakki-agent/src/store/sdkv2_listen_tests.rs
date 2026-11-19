//! `iyakki.speech.listen` through the real bridge (cards, limits, audit, events) with the real [`ListenBackend`] over a fake microphone
//! and a fake voice engine. No microphone is opened. What a person said into a real one is not covered (needs hardware, KI-726).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_js::modules::Bridge;
use serde_json::{json, Value};

use super::sdkv2_backends::Backends;
use super::sdkv2_listen::*;
use super::tile::*;
use super::tilesdk::*;
use crate::usage::UsageLog;

struct Mic {
    present: bool,
    /// Samples recorded per call; the fake waits `ms` unless stopped.
    ms: u64,
    stopped_early: AtomicBool,
    recorded: Mutex<Vec<Pcm>>,
}

impl Mic {
    fn new(present: bool, ms: u64) -> Arc<Mic> {
        Arc::new(Mic { present, ms, stopped_early: AtomicBool::new(false), recorded: Mutex::new(Vec::new()) })
    }
}

impl Capture for Mic {
    fn microphone(&self, _: Option<&str>) -> Result<String, String> {
        if self.present { Ok("USB Mic".into()) } else { Err("Plug in a USB microphone.".into()) }
    }
    fn record(&self, _: Option<&str>, _: u64, stop: &AtomicBool) -> Result<Pcm, String> {
        let end = std::time::Instant::now() + Duration::from_millis(self.ms);
        while std::time::Instant::now() < end {
            if stop.load(Ordering::SeqCst) {
                self.stopped_early.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let pcm = vec![1234i16; 1600];
        self.recorded.lock().unwrap().push(pcm.clone());
        Ok(pcm)
    }
}

struct Engine {
    text: Result<String, String>,
    ready: Result<(), String>,
    seen: Mutex<Vec<(usize, Option<String>)>>,
    calls: AtomicUsize,
}

impl Engine {
    fn new(text: Result<&str, &str>) -> Arc<Engine> {
        Arc::new(Engine { text: text.map(str::to_string).map_err(str::to_string), ready: Ok(()), seen: Mutex::new(Vec::new()), calls: AtomicUsize::new(0) })
    }
}

impl Recogniser for Engine {
    fn ready(&self) -> Result<(), String> {
        self.ready.clone()
    }
    fn transcribe(&self, pcm: &[i16], lang: Option<&str>) -> Result<String, String> {
        assert!(pcm.iter().all(|s| *s == 1234), "the engine gets the recording");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push((pcm.len(), lang.map(str::to_string)));
        self.text.clone()
    }
}

struct Rig {
    host: TileHost,
    sdk: Arc<TileSdk>,
    out: Arc<Mutex<Outbox>>,
    be: Arc<ListenBackend>,
    dir: PathBuf,
}

fn rig(tag: &str, be: ListenBackend) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-listen-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Arc::new(Mutex::new(Outbox::default()));
    let be = Arc::new(be);
    let mut b = Backends::missing();
    b.listener = be.clone();
    let file = dir.join("appdata").join("tile-sdk").join("tester").join("t.json");
    let sdk = Arc::new(TileSdk::new("t", "tester", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&dir.join("usage.jsonl")))), out.clone(), Some(&file)).with_backends(b));
    let s = sdk.clone();
    be.set_sink(Arc::new(move |h, r| s.v2().heard(h, r)));
    let info = AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false };
    let host = TileHost::new(info, Caps::all_v2(), TileStorage::memory(), out.clone()).with_sdk(sdk.clone());
    Rig { host, sdk, out, be, dir }
}

impl Rig {
    fn call(&mut self, op: &str, a: Value) -> Result<Value, (String, String)> {
        self.host.call(op, &a).map_err(|e| SdkError::decode(&e).unwrap_or_else(|| ("plain".into(), e)))
    }
    fn code(&mut self, op: &str, a: Value) -> String {
        match self.call(op, a) {
            Err((c, _)) => c,
            Ok(v) => panic!("{op} should fail, gave {v}"),
        }
    }
    fn listen(&mut self, a: Value) -> u32 {
        let v = self.call("speech.listen", a).unwrap();
        let id = v["pending"].as_u64().expect("a card") as u32;
        assert!(self.sdk.answer(id, true));
        let d = {
            let mut o = self.out.lock().unwrap();
            let i = o.done.iter().position(|d| d.id == id).unwrap();
            o.done.remove(i)
        };
        assert!(d.ok, "{}", d.value);
        serde_json::from_str::<Value>(&d.value).unwrap()["handle"].as_u64().unwrap() as u32
    }
    fn events(&self, h: u32) -> Vec<(String, Value)> {
        self.out.lock().unwrap().emits.iter().filter(|e| e.handle == h).map(|e| (e.event.clone(), e.payload.clone())).collect()
    }
    fn wait_for(&self, h: u32, n: usize) -> Vec<(String, Value)> {
        for _ in 0..500 {
            let e = self.events(h);
            if e.len() >= n {
                return e;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.events(h)
    }
}

#[test]
fn the_spoken_words_reach_the_app_as_text_only_after_the_person_said_yes_and_the_sound_is_thrown_away() {
    let (mic, eng) = (Mic::new(true, 30), Engine::new(Ok("turn on the light")));
    let mut r = rig("ok", ListenBackend::new(mic.clone(), eng.clone()));
    let h = r.listen(json!({"lang": "ta-IN", "maxSeconds": 10}));
    let ev = r.wait_for(h, 2);
    assert_eq!(ev[0], ("result".to_string(), json!({"text": "turn on the light"})), "{ev:?}");
    assert_eq!(ev[1].0, "done");
    for (_, payload) in &ev {
        assert!(!payload.to_string().contains("1234"), "no sound in an event");
    }
    assert_eq!(eng.seen.lock().unwrap()[0], (1600, Some("ta-IN".to_string())));
    assert!(wait_until(|| !r.be.busy()), "the recording slot is free again");
    assert!(r.out.lock().unwrap().audit.iter().any(|(a, d)| a == "speech.listen" && d.contains("USB Mic")));
    // a second use works after the first ended
    let h2 = r.listen(json!({}));
    assert_eq!(r.wait_for(h2, 2)[0].0, "result");
    let _ = std::fs::remove_dir_all(&r.dir);
}

fn wait_until(mut f: impl FnMut() -> bool) -> bool {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn nobody_is_asked_to_allow_what_cannot_work() {
    // no microphone
    let mut r = rig("nomic", ListenBackend::new(Mic::new(false, 1), Engine::new(Ok("x"))));
    assert_eq!(r.code("speech.listen", json!({})), "Unavailable");
    assert!(r.out.lock().unwrap().requests.is_empty(), "no card for what cannot work");
    // no engine
    let mut eng = Engine::new(Ok("x"));
    Arc::get_mut(&mut eng).unwrap().ready = Err("Install the voice pack in Settings -> Voice.".into());
    let mut r2 = rig("noengine", ListenBackend::new(Mic::new(true, 1), eng));
    let (c, msg) = r2.call("speech.listen", json!({})).unwrap_err();
    assert_eq!(c, "Unavailable");
    assert!(msg.contains("voice pack") || msg.contains("voice engine"), "{msg}");
    assert!(r2.out.lock().unwrap().requests.is_empty());
    // a T2 laptop stays Unavailable until capture is proven safe there, even with a microphone and an engine
    let mut r3 = rig("t2", ListenBackend::new(Mic::new(true, 1), Engine::new(Ok("x"))).blocked("Use dictation in the chat; the microphone of this laptop is not usable by apps yet."));
    assert_eq!(r3.code("speech.listen", json!({})), "Unavailable");
    assert!(r3.out.lock().unwrap().requests.is_empty());
    for d in [&r.dir, &r2.dir, &r3.dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_no_from_the_person_never_opens_the_microphone() {
    let (mic, eng) = (Mic::new(true, 5), Engine::new(Ok("secret")));
    let mut r = rig("deny", ListenBackend::new(mic.clone(), eng.clone()));
    let v = r.call("speech.listen", json!({})).unwrap();
    let id = v["pending"].as_u64().unwrap() as u32;
    assert!(r.sdk.answer(id, false));
    std::thread::sleep(Duration::from_millis(100));
    assert!(mic.recorded.lock().unwrap().is_empty() && eng.calls.load(Ordering::SeqCst) == 0, "nothing was recorded or recognised");
    assert!(!r.be.busy());
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn cancelling_stops_the_recording_at_once_and_nothing_is_reported_or_recognised() {
    let (mic, eng) = (Mic::new(true, 30_000), Engine::new(Ok("never heard")));
    let mut r = rig("cancel", ListenBackend::new(mic.clone(), eng.clone()));
    let h = r.listen(json!({"maxSeconds": 30}));
    assert!(wait_until(|| r.be.busy()));
    r.call("speech.cancel", json!({"handle": h})).unwrap();
    assert!(wait_until(|| !r.be.busy()), "the recording ended long before 30 s");
    assert!(mic.stopped_early.load(Ordering::SeqCst));
    assert_eq!(eng.calls.load(Ordering::SeqCst), 0, "a cancelled recording is not recognised");
    let ev = r.events(h);
    assert_eq!(ev.iter().filter(|e| e.0 == "result").count(), 0, "{ev:?}");
    assert!(ev.iter().any(|e| e.0 == "done" && e.1["cancelled"] == true));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn silence_is_a_timeout_and_an_engine_failure_is_unavailable_never_a_made_up_text() {
    let mut r = rig("silence", ListenBackend::new(Mic::new(true, 5), Engine::new(Ok("   "))));
    let h = r.listen(json!({}));
    let ev = r.wait_for(h, 1);
    assert_eq!(ev[0].0, "error", "{ev:?}");
    assert!(ev[0].1.to_string().contains("Timeout"), "{ev:?}");
    let mut r2 = rig("enginefail", ListenBackend::new(Mic::new(true, 5), Engine::new(Err("the voice pack is damaged"))));
    let h = r2.listen(json!({}));
    let ev = r2.wait_for(h, 1);
    assert_eq!(ev[0].0, "error");
    assert!(ev[0].1.to_string().contains("Unavailable"), "{ev:?}");
    for d in [&r.dir, &r2.dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}
