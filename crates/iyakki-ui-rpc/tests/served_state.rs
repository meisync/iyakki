//! `iyakki dev state` of the UI program over the core's socket (F-SPLIT-03, D-SPLIT-03-APPS): the program tells the core it is drawing, learns
//! whether a computer asked for the screen, and hands the raw frame over; the safety rules run in the core. Real loopback transport, the real
//! slots of `iyakki_shared::devmode_state`, a manual clock for the pace, nothing waits for time.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_core_api::contract::RateLimit;
use iyakki_shared::devmode_state::{self as state, Raw, RawField, Rect, RemoteState};
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{remote_dev_state, serve_dev_state, serve_uploads, Host, RemoteDevState, Rpc, BEAT, PUBLISH};

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn machine(host: Host) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(Arc::new(host)), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A clock the test moves.
struct Clock {
    base: Instant,
    ms: AtomicU64,
}
impl Clock {
    fn new() -> Arc<Clock> {
        Arc::new(Clock { base: Instant::now(), ms: AtomicU64::new(0) })
    }
    fn advance(&self, d: Duration) {
        self.ms.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
    }
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.ms.load(Ordering::SeqCst))
    }
}

fn setup(tag: &str) -> (Machine, Arc<RemoteDevState>, Arc<Clock>, std::path::PathBuf) {
    // the slots are keyed by the folder: one name per test, never a folder that is touched
    let dir = std::path::PathBuf::from(format!("/iyakki-served-state-{}-{tag}", std::process::id()));
    let m = machine(Host::new().with_root(serve_uploads()).with_root(serve_dev_state(dir.clone())));
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap());
    let clock = Clock::new();
    let c2 = clock.clone();
    let remote = remote_dev_state(&rpc, 0, Arc::new(move || c2.now()));
    (m, remote, clock, dir)
}

fn frame(app: &str, text: &str, secret_text: &str) -> Raw {
    Raw {
        app: app.into(),
        page: String::new(),
        size: (800, 480),
        texts: vec![(10.0, 10.0, text.into()), (10.0, 60.0, secret_text.into())],
        hits: vec![("Save".into(), Rect { x: 0.0, y: 100.0, w: 50.0, h: 20.0 })],
        fields: vec![RawField { rect: Rect { x: 0.0, y: 50.0, w: 300.0, h: 30.0 }, focused: true, secret: true }],
    }
}

#[test]
fn the_program_tells_the_core_it_draws_learns_when_a_frame_is_wanted_and_the_core_makes_the_frame_safe() {
    let (_m, remote, clock, dir) = setup("beat");
    assert!(!state::alive(&dir), "before the first beat nobody draws");
    assert!(state::fetch(&dir, 0).is_err(), "a computer that asks a core with no screen is told so");
    remote.heartbeat();
    assert!(state::alive(&dir), "the beat reached the core's slot");
    assert!(!remote.wanted(), "nobody asked yet");
    // a computer asks (the slot says "wanted" for a few seconds); the program learns at its next beat
    let _ = state::fetch(&dir, 0);
    remote.heartbeat();
    assert!(!remote.wanted(), "a beat within the pace does not ask again: the answer is the last one");
    clock.advance(BEAT);
    remote.heartbeat();
    assert!(remote.wanted(), "the next beat learned that a frame is wanted");
    // the frame is raw: a password typed in a secret field is made `[hidden]` by the CORE's rules
    remote.publish(&frame("Settings", "Wi-Fi password", "hunter2-secret"));
    let snap = state::fetch(&dir, 500).expect("the frame arrived");
    assert_eq!((snap.app.as_str(), snap.size), ("Settings", (800, 480)));
    let said: Vec<&str> = snap.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(said.contains(&"Wi-Fi password") && said.contains(&"[hidden]") && !said.iter().any(|t| t.contains("hunter2")), "{said:?}");
    assert_eq!(snap.buttons.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["Save"]);
}

#[test]
fn private_apps_give_no_text_through_the_socket_either() {
    let (_m, remote, _clock, dir) = setup("private");
    remote.heartbeat();
    remote.publish(&frame("Chat", "a message of the person", "x"));
    let snap = state::fetch(&dir, 500).unwrap();
    assert!(snap.text_hidden && snap.lines.is_empty(), "a private app hands over button names only, decided in the core");
    assert_eq!(snap.buttons.len(), 1);
}

/// A core that only counts what the program asks of it.
#[derive(Default)]
struct Counting {
    beats: std::sync::atomic::AtomicUsize,
    frames: std::sync::atomic::AtomicUsize,
}
impl iyakki_ui_rpc::Served for Counting {
    fn iface(&self) -> &'static str {
        "DevState"
    }
    fn serve(&self, _reg: &iyakki_ui_rpc::Registry<'_>, method: &str, _a: serde_json::Value) -> Result<serde_json::Value, String> {
        match method {
            "beat" => {
                self.beats.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!(true))
            }
            "publish" => {
                self.frames.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!(true))
            }
            other => Err(other.into()),
        }
    }
}

#[test]
fn beats_and_frames_are_paced_on_the_clock_the_program_was_given() {
    let counting = Arc::new(Counting::default());
    let m = machine(Host::new().with_root(serve_uploads()).with_root(iyakki_ui_rpc::Obj::Served(counting.clone())));
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap());
    let clock = Clock::new();
    let c2 = clock.clone();
    let remote = remote_dev_state(&rpc, 0, Arc::new(move || c2.now()));
    // sixty frames a second would be sixty calls: the shell calls every frame, the core is asked at the pace
    for _ in 0..60 {
        remote.heartbeat();
        remote.publish(&frame("Home", "Welcome", "x"));
    }
    assert_eq!((counting.beats.load(Ordering::SeqCst), counting.frames.load(Ordering::SeqCst)), (1, 1));
    assert!(remote.wanted(), "the answer of the one beat stands until the next");
    clock.advance(PUBLISH);
    remote.heartbeat();
    remote.publish(&frame("Home", "Welcome", "x"));
    assert_eq!((counting.beats.load(Ordering::SeqCst), counting.frames.load(Ordering::SeqCst)), (1, 2), "a frame goes every PUBLISH, a beat every BEAT");
    clock.advance(BEAT);
    remote.heartbeat();
    assert_eq!(counting.beats.load(Ordering::SeqCst), 2);
}

#[test]
fn a_frame_bigger_than_a_line_goes_up_in_chunks_and_a_lost_core_wants_nothing() {
    let (_m, remote, _clock, dir) = setup("big");
    remote.heartbeat();
    // 3000 lines of 160 characters is far more than one line of the transport: the core's limits cut it to what it keeps
    let mut raw = frame("Settings", "t", "x");
    raw.texts = (0..3000).map(|i| (0.0, i as f32, format!("{i:0>160}"))).collect();
    remote.publish(&raw);
    let snap = state::fetch(&dir, 500).unwrap();
    assert_eq!((snap.lines.len(), snap.cut), (state::MAX_TEXTS, true));
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let lost = remote_dev_state(&rpc, 3, Arc::new(Instant::now));
    lost.heartbeat();
    lost.publish(&raw);
    assert!(!lost.wanted());
}
