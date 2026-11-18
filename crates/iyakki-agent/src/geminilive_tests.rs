//! Tests of the Gemini live voice session against a FAKE live server: an in-process WebSocket server on this computer that speaks the
//! message shapes of Google's documentation (read 2026-11-15, docs/GEMINI-PROVIDER.md). Nothing here ever reaches Google: a real run
//! needs a real key, a microphone and a speaker, which this lane does not have ("fake server only").

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_link::wsproto;
use serde_json::{json, Value};

#[allow(unused_imports)]
use iyakki_shared::geminilive::*;
#[allow(unused_imports)]
use iyakki_shared::gemini;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use iyakki_shared::gemini::{GuardedUpstream, Tier};
use crate::geminilive_ws::WsConnector;
use crate::voiceassist::{Speaker, TurnLog, Who};

const KEY: &str = "AIzaSyFAKEKEY-0123456789";

/// Tests share the process-wide stop and indicator state, so they run one at a time.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-geminilive-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ---------------------------------------------------------------------------------------------------------- the fake server

enum Out {
    Text(String),
    /// A text message sent as two fragments.
    Fragmented(String),
    Ping,
    Close(u16, String),
    Hang,
}

type Script = Box<dyn FnMut(&Value) -> Vec<Out> + Send>;

struct Fake {
    port: u16,
    got: Arc<Mutex<Vec<String>>>,
    request: Arc<Mutex<String>>,
    conns: Arc<AtomicUsize>,
}

fn ok_json(v: Value) -> Out {
    Out::Text(v.to_string())
}

/// `refuse`: answer the upgrade with this HTTP status instead.
fn fake(mut script: Script, refuse: Option<(u16, &'static str)>) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let (got, request, conns) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(String::new())), Arc::new(AtomicUsize::new(0)));
    let (g2, r2, c2) = (got.clone(), request.clone(), conns.clone());
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut s = loop {
            match l.accept() {
                Ok((s, _)) => break s,
                Err(_) if t0.elapsed() < Duration::from_secs(8) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => return,
            }
        };
        c2.fetch_add(1, Ordering::SeqCst);
        s.set_nonblocking(false).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 4096];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match s.read(&mut b) {
                Ok(0) => return,
                Ok(n) => head.extend_from_slice(&b[..n]),
                Err(_) if t0.elapsed() < Duration::from_secs(8) => {}
                Err(_) => return,
            }
        }
        let text = String::from_utf8_lossy(&head).to_string();
        *r2.lock().unwrap() = text.lines().next().unwrap_or("").to_string();
        if let Some((st, body)) = refuse {
            let _ = s.write_all(format!("HTTP/1.1 {st} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            return;
        }
        let key = text.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key:")).unwrap().trim().to_string();
        let _ = s.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", wsproto::accept_key(&key)).as_bytes());
        let mut p = wsproto::Parser::new(8 * 1024 * 1024);
        let end_at = Instant::now() + Duration::from_secs(8);
        loop {
            if Instant::now() > end_at {
                return;
            }
            match p.next() {
                Ok(Some(wsproto::Message::Text(t))) => {
                    g2.lock().unwrap().push(t.clone());
                    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                    for o in script(&v) {
                        let r = match o {
                            Out::Text(t) => s.write_all(&wsproto::text_frame(&t)),
                            Out::Fragmented(t) => {
                                let (a, z) = t.split_at(t.len() / 2);
                                let mut f1 = wsproto::frame(0x1, a.as_bytes());
                                f1[0] &= 0x7f; // not the last piece
                                s.write_all(&f1).and_then(|()| s.write_all(&wsproto::frame(0x0, z.as_bytes())))
                            }
                            Out::Ping => s.write_all(&wsproto::frame(0x9, b"hi")),
                            Out::Close(c, why) => {
                                let mut pl = c.to_be_bytes().to_vec();
                                pl.extend_from_slice(why.as_bytes());
                                let _ = s.write_all(&wsproto::frame(0x8, &pl));
                                return;
                            }
                            Out::Hang => return,
                        };
                        if r.is_err() {
                            return;
                        }
                    }
                    continue;
                }
                Ok(Some(wsproto::Message::Close)) | Err(_) => return,
                Ok(Some(_)) => continue,
                Ok(None) => {}
            }
            match s.read(&mut b) {
                Ok(0) => return,
                Ok(n) => p.push(&b[..n]),
                Err(_) => {}
            }
        }
    });
    Fake { port, got, request, conns }
}

impl Fake {
    fn sent(&self) -> Vec<Value> {
        self.got.lock().unwrap().iter().filter_map(|t| serde_json::from_str(t).ok()).collect()
    }
    fn audio_messages(&self) -> usize {
        self.sent().iter().filter(|v| v.pointer("/realtimeInput/audio/data").is_some()).count()
    }
}

/// Sends the connection to the fake server on this computer instead of Google (the path and the query are kept and checked).
struct ToFake(u16);
impl LiveConnector for ToFake {
    fn connect(&self, url: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
        let rest = url.strip_prefix("wss://generativelanguage.googleapis.com").expect("the connection goes to Google's host");
        WsConnector::default().connect(&format!("ws://127.0.0.1:{}{rest}", self.0))
    }
}

struct FakeClock(Arc<AtomicU64>);
impl FakeClock {
    fn new() -> FakeClock {
        FakeClock(Arc::new(AtomicU64::new(1_000_000)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

fn deps(dir: &Path, c: Arc<dyn LiveConnector>, clock: &FakeClock) -> LiveDeps {
    let t = clock.0.clone();
    LiveDeps { dir: dir.to_path_buf(), user: Arc::new(|| "ada".to_string()), key: Arc::new(|| Ok(KEY.to_string())), connector: c, now_ms: Arc::new(move || t.load(Ordering::SeqCst)), today: Arc::new(|| "2026-11-15".to_string()) }
}

/// Switched on and the person said yes.
fn allowed(dir: &Path) {
    gemini::update(dir, |c| {
        c.voice.enabled = true;
        Ok(())
    })
    .unwrap();
    gemini::grant_live_on(dir, "ada", "2026-11-15").unwrap();
}

struct Say {
    said: Arc<Mutex<Vec<String>>>,
    stops: Arc<AtomicUsize>,
}
impl Speaker for Say {
    fn say(&mut self, t: &str) -> Result<(), String> {
        self.said.lock().unwrap().push(t.to_string());
        Ok(())
    }
    fn busy(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
    }
    fn level(&self) -> f32 {
        0.0
    }
    fn louder(&mut self) {}
    fn quieter(&mut self) {}
}
fn speaker() -> (Say, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
    let (a, b) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
    (Say { said: a.clone(), stops: b.clone() }, a, b)
}

#[derive(Default)]
struct Log(Vec<(Who, String)>);
impl TurnLog for Log {
    fn record(&mut self, who: Who, text: &str) {
        self.0.push((who, text.to_string()));
    }
}

struct Mic(VecDeque<Pcm>);
impl PcmSource for Mic {
    fn next(&mut self, _: Duration) -> Pcm {
        self.0.pop_front().unwrap_or(Pcm::Quiet)
    }
}

fn pcm(secs: f32) -> Vec<u8> {
    vec![1u8; (secs * BYTES_PER_SEC as f32) as usize]
}

/// The usual server: setupComplete on setup, a scripted answer on the end of the turn.
fn answering(reply: Vec<Value>) -> Script {
    let mut reply = Some(reply);
    Box::new(move |v| {
        if v.get("setup").is_some() {
            return vec![ok_json(json!({"setupComplete": {}}))];
        }
        if v.pointer("/realtimeInput/audioStreamEnd").is_some() {
            return reply.take().unwrap_or_default().into_iter().map(ok_json).collect();
        }
        vec![]
    })
}

fn say_text(t: &str) -> Value {
    json!({"serverContent": {"outputTranscription": {"text": t}}})
}

// ---------------------------------------------------------------------------------------------------------- the tests

#[test]
fn a_full_turn_microphone_in_gemini_text_out_read_aloud_by_the_local_voice() {
    let _s = serial();
    let dir = temp("turn");
    allowed(&dir);
    let reply = vec![
        json!({"serverContent": {"inputTranscription": {"text": "what time is it"}}}),
        say_text("It is "),
        json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": "AAAA"}}]}}}),
        say_text("**ten** o'clock."),
        json!({"serverContent": {"turnComplete": true}}),
    ];
    let f = fake(answering(reply), None);
    let clock = FakeClock::new();
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).expect("starts");
    assert!(mic_open() && crate::clientmedia::indicator().microphones.contains(&INDICATOR_NAME.to_string()), "the red band shows the open microphone");
    let (mut sp, said, _) = speaker();
    let mut log = Log::default();
    let mut mic = Mic(VecDeque::from([Pcm::Chunk(pcm(0.1)), Pcm::Chunk(pcm(0.1)), Pcm::Released]));
    let e = run(&mut s, &mut mic, &mut sp, &mut log);
    assert_eq!(e.reason, EndReason::Finished, "{e:?}");
    assert_eq!(said.lock().unwrap().join(" "), "It is ten o'clock.", "the local voice reads the text, without markdown");
    assert_eq!(log.0, vec![(Who::Person, "what time is it".into()), (Who::Assistant, "It is **ten** o'clock.".into())]);
    let sent = f.sent();
    let setup = &sent[0]["setup"];
    assert_eq!(setup["model"], "models/gemini-3.8-live");
    assert_eq!(setup["generationConfig"]["responseModalities"], json!(["AUDIO"]));
    assert!(setup.get("outputAudioTranscription").is_some() && setup.get("inputAudioTranscription").is_some());
    assert_eq!(f.audio_messages(), 2);
    let a = sent.iter().find(|v| v.pointer("/realtimeInput/audio").is_some()).unwrap();
    assert_eq!(a["realtimeInput"]["audio"]["mimeType"], "audio/pcm;rate=16000");
    assert!(sent.iter().any(|v| v.pointer("/realtimeInput/audioStreamEnd") == Some(&json!(true))));
    let req = f.request.lock().unwrap().clone();
    assert!(req.starts_with("GET /ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key="), "{req}");
    drop(s);
    assert!(!mic_open(), "the sign goes out when the session closes");
    assert!(crate::clientmedia::indicator().microphones.is_empty());
    // the day's count is kept (0.2 s of sound rounds down)
    assert_eq!(gemini::load(&dir).voice.used_date, "2026-11-15");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_wire_handles_ping_and_fragmented_messages() {
    let _s = serial();
    let dir = temp("wire");
    allowed(&dir);
    let mut n = 0;
    let script: Script = Box::new(move |v| {
        if v.get("setup").is_some() {
            return vec![Out::Ping, ok_json(json!({"setupComplete": {}}))];
        }
        n += 1;
        if n == 1 {
            return vec![Out::Fragmented(say_text("In two pieces.").to_string()), Out::Fragmented(json!({"serverContent": {"turnComplete": true}}).to_string())];
        }
        vec![]
    });
    let f = fake(script, None);
    let clock = FakeClock::new();
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).expect("starts");
    s.send_text("hello").unwrap();
    let mut got = Vec::new();
    for _ in 0..20 {
        got.extend(s.poll(Duration::from_millis(100)));
        if got.iter().any(|e| matches!(e, LiveEvent::Reply(_))) {
            break;
        }
    }
    assert!(got.contains(&LiveEvent::Reply("In two pieces.".into())), "{got:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_the_yes_nothing_is_sent_not_even_a_connection_and_the_key_is_not_read() {
    let _s = serial();
    let dir = temp("noyes");
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = reads.clone();
    let mut d = deps(&dir, Arc::new(ToFake(f.port)), &clock);
    d.key = Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
        Ok(KEY.into())
    });
    // off by default
    let e = start(&d).err().unwrap();
    assert!(e.contains("switched off") && e.contains("EXPERIMENTAL") && e.contains("local voice answers instead"), "{e}");
    gemini::update(&dir, |c| {
        c.voice.enabled = true;
        Ok(())
    })
    .unwrap();
    // on, but no yes
    let e = start(&d).err().unwrap();
    assert!(e.contains("no sound was sent to Google") && e.contains("local voice answers instead"), "{e}");
    // the yes to speech-to-text is NOT a yes to an open microphone
    gemini::grant_on(&dir, "ada", "2026-11-15").unwrap();
    assert!(start(&d).is_err());
    // another person's yes does not count either
    gemini::grant_live_on(&dir, "bob", "2026-11-15").unwrap();
    assert!(start(&d).is_err());
    // the tier changed after the yes: asked again
    gemini::grant_live_on(&dir, "ada", "2026-11-15").unwrap();
    gemini::update(&dir, |c| {
        c.tier = Tier::Paid;
        Ok(())
    })
    .unwrap();
    assert!(start(&d).err().unwrap().contains("asked again"));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(f.conns.load(Ordering::SeqCst), 0, "no connection was opened");
    assert_eq!(reads.load(Ordering::SeqCst), 0, "the key was not even read");
    assert!(!mic_open());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn taking_the_yes_back_mid_session_closes_the_microphone_and_sends_nothing_more() {
    let _s = serial();
    let dir = temp("revoke");
    allowed(&dir);
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).unwrap();
    s.send_audio(&pcm(0.1)).unwrap();
    gemini::revoke(&dir, "ada").unwrap();
    clock.advance(600); // the file is read again every half second
    let e = s.send_audio(&pcm(0.1)).unwrap_err();
    assert_eq!(e.reason, EndReason::Refused, "{e:?}");
    assert!(e.line.contains("taken back"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.audio_messages(), 1, "only the audio from before the yes was taken back");
    assert!(s.send_audio(&pcm(0.1)).is_err() && s.end_turn().is_err() && s.send_text("x").is_err());
    assert!(!s.is_open());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_key_never_appears_in_lines_errors_status_or_the_setup() {
    let _s = serial();
    let dir = temp("key");
    allowed(&dir);
    let clock = FakeClock::new();
    // a connector that puts the whole address in its error
    struct Leaky;
    impl LiveConnector for Leaky {
        fn connect(&self, url: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
            Err(ConnectErr::Other(format!("cannot reach {url}")))
        }
    }
    let e = start(&deps(&dir, Arc::new(Leaky), &clock)).err().unwrap();
    assert!(!e.contains(KEY) && e.contains("could not be reached"), "{e}");
    // the real connector with a refusal that echoes the key
    let f = fake(answering(vec![]), Some((403, "{\"error\":{\"message\":\"API key AIzaSyFAKEKEY-0123456789 is bad\"}}")));
    let e = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).err().unwrap();
    assert!(!e.contains(KEY) && e.contains("refused the key"), "{e}");
    // a close from the server that echoes the key
    let f = fake(
        Box::new(|v| if v.get("setup").is_some() { vec![ok_json(json!({"setupComplete": {}}))] } else { vec![Out::Close(1011, format!("bad things with {KEY}"))] }),
        None,
    );
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).unwrap();
    s.send_audio(&pcm(0.1)).unwrap();
    let mut ev = Vec::new();
    for _ in 0..20 {
        ev.extend(s.poll(Duration::from_millis(100)));
        if ev.iter().any(|e| matches!(e, LiveEvent::Ended(_))) {
            break;
        }
    }
    let text = format!("{ev:?}");
    assert!(text.contains("Ended") && !text.contains(KEY), "{text}");
    drop(s);
    assert!(!status_text(&dir, "ada", true, "2026-11-15").contains(KEY));
    assert!(!setup_message("m").to_string().contains("AIza") && !SYSTEM_PROMPT.contains("key"));
    assert!(!last_end_line().unwrap_or_default().contains(KEY));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A connection with no network: replays messages, records what is sent.
struct Mem {
    inbox: Arc<Mutex<VecDeque<Result<Recv, String>>>>,
    sent: Arc<Mutex<Vec<String>>>,
}
impl LiveLink for Mem {
    fn send_text(&mut self, t: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(t.to_string());
        Ok(())
    }
    fn recv(&mut self, _: Duration) -> Result<Recv, String> {
        self.inbox.lock().unwrap().pop_front().unwrap_or(Ok(Recv::Timeout))
    }
    fn close(&mut self) {}
}
struct MemConn(Arc<Mutex<VecDeque<Result<Recv, String>>>>, Arc<Mutex<Vec<String>>>);
impl LiveConnector for MemConn {
    fn connect(&self, _: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
        self.0.lock().unwrap().push_front(Ok(Recv::Text(json!({"setupComplete": {}}).to_string())));
        Ok(Box::new(Mem { inbox: self.0.clone(), sent: self.1.clone() }))
    }
}
fn mem() -> (Arc<MemConn>, Arc<Mutex<VecDeque<Result<Recv, String>>>>, Arc<Mutex<Vec<String>>>) {
    let (i, s) = (Arc::new(Mutex::new(VecDeque::new())), Arc::new(Mutex::new(Vec::new())));
    (Arc::new(MemConn(i.clone(), s.clone())), i, s)
}

#[test]
fn caps_session_length_idle_and_the_daily_meter() {
    let _s = serial();
    let dir = temp("caps");
    allowed(&dir);
    gemini::update(&dir, |c| {
        c.voice.max_session_secs = 60;
        c.voice.idle_secs = 20;
        c.voice.day_cap_secs = 100;
        Ok(())
    })
    .unwrap();
    // idle: nobody speaks and Gemini says nothing
    let clock = FakeClock::new();
    let (c, _, sent) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    clock.advance(19_000);
    assert!(s.send_audio(&pcm(0.1)).is_ok());
    clock.advance(1_000);
    let e = s.send_audio(&pcm(0.1)).unwrap_err();
    assert_eq!(e.reason, EndReason::Idle, "{e:?}");
    assert!(e.line.contains("20 seconds"));
    let n = sent.lock().unwrap().len();
    assert!(s.send_audio(&pcm(0.1)).is_err());
    assert_eq!(sent.lock().unwrap().len(), n, "nothing after the end");
    drop(s);
    // speech keeps it alive; the length cap ends it anyway
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    for _ in 0..5 {
        clock.advance(15_000);
        inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"inputTranscription": {"text": "la "}}}).to_string())));
        let ev = s.poll(Duration::from_millis(1));
        if let Some(LiveEvent::Ended(e)) = ev.last() {
            assert_eq!(e.reason, EndReason::MaxLength, "{e:?}");
            assert!(e.line.contains("60 seconds"));
            break;
        }
    }
    assert_eq!(s.ended().map(|e| e.reason), Some(EndReason::MaxLength));
    drop(s);
    // the daily meter: 90 s used already, a 10 s cap remains
    gemini::update(&dir, |c| {
        c.voice.used_date = "2026-11-15".into();
        c.voice.used_secs = 90;
        Ok(())
    })
    .unwrap();
    let (c, _, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    s.send_audio(&pcm(5.0)).unwrap();
    let e = s.send_audio(&pcm(5.0)).unwrap_err();
    assert_eq!(e.reason, EndReason::DayCap, "{e:?}");
    drop(s);
    assert_eq!(gemini::load(&dir).voice.used_secs, 100, "the count was saved");
    // used up: it does not even start
    let (c, _, _) = mem();
    assert!(start(&deps(&dir, c, &clock)).err().unwrap().contains("daily cap"));
    // yesterday's count does not count today
    gemini::update(&dir, |c| {
        c.voice.used_date = "2026-11-14".into();
        Ok(())
    })
    .unwrap();
    let (c, _, _) = mem();
    assert!(start(&deps(&dir, c, &clock)).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn talking_over_the_answer_drops_it_and_stops_the_voice() {
    let _s = serial();
    let dir = temp("barge");
    allowed(&dir);
    let reply = vec![say_text("A long answer that"), json!({"serverContent": {"interrupted": true}}), say_text("Sorry, go on."), json!({"serverContent": {"turnComplete": true}})];
    let f = fake(answering(reply), None);
    let clock = FakeClock::new();
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).unwrap();
    let (mut sp, said, stops) = speaker();
    let mut log = Log::default();
    let mut mic = Mic(VecDeque::from([Pcm::Chunk(pcm(0.1)), Pcm::Released]));
    let e = run(&mut s, &mut mic, &mut sp, &mut log);
    assert_eq!(e.reason, EndReason::Finished);
    assert_eq!(said.lock().unwrap().join("|"), "Sorry, go on.", "the dropped part is never read");
    assert!(stops.load(Ordering::SeqCst) >= 1, "the speaker was stopped");
    // the local hook
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    inbox.lock().unwrap().push_back(Ok(Recv::Text(say_text("half an ans").to_string())));
    s.poll(Duration::from_millis(1));
    s.interrupt();
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"turnComplete": true}}).to_string())));
    assert!(!s.poll(Duration::from_millis(1)).iter().any(|e| matches!(e, LiveEvent::Reply(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dropped_connection_ends_the_session_with_a_line_for_the_local_voice() {
    let _s = serial();
    let dir = temp("drop");
    allowed(&dir);
    let f = fake(Box::new(|v| if v.get("setup").is_some() { vec![ok_json(json!({"setupComplete": {}}))] } else { vec![Out::Hang] }), None);
    let clock = FakeClock::new();
    let mut s = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).unwrap();
    let (mut sp, said, _) = speaker();
    let mut log = Log::default();
    let mut mic = Mic(VecDeque::from([Pcm::Chunk(pcm(0.1)), Pcm::Chunk(pcm(0.1)), Pcm::Chunk(pcm(0.1))]));
    let mut e = run(&mut s, &mut mic, &mut sp, &mut log);
    for _ in 0..50 {
        if e.reason == EndReason::Dropped {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        e = run(&mut s, &mut Mic(VecDeque::new()), &mut sp, &mut log);
    }
    assert_eq!(e.reason, EndReason::Dropped, "{e:?}");
    assert!(e.line.contains("Google") && !said.lock().unwrap().iter().any(|t| t.contains("Google")));
    assert!(!s.is_open() && !mic_open());
    // no network at all
    struct Down;
    impl LiveConnector for Down {
        fn connect(&self, _: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
            Err(ConnectErr::Other("no route".into()))
        }
    }
    let l = start(&deps(&dir, Arc::new(Down), &clock)).err().unwrap();
    assert!(l.contains("could not be reached") && l.contains("local voice answers instead"), "{l}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_limit_message_says_plainly_that_the_free_limit_is_reached() {
    let _s = serial();
    let dir = temp("limit");
    allowed(&dir);
    let clock = FakeClock::new();
    // 429 at the door
    let f = fake(answering(vec![]), Some((429, "{\"error\":{\"status\":\"RESOURCE_EXHAUSTED\"}}")));
    let l = start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).err().unwrap();
    assert!(l.contains("free Gemini limit is reached") && l.contains("Wait a few minutes") && l.contains("local voice answers instead"), "{l}");
    // the paid tier says it in its own words
    gemini::update(&dir, |c| {
        c.tier = Tier::Paid;
        Ok(())
    })
    .unwrap();
    gemini::grant_live_on(&dir, "ada", "2026-11-15").unwrap();
    let f = fake(answering(vec![]), Some((429, "{}")));
    assert!(start(&deps(&dir, Arc::new(ToFake(f.port)), &clock)).err().unwrap().contains("your key has reached its limit"));
    // a close in the middle of a session
    gemini::update(&dir, |c| {
        c.tier = Tier::Free;
        Ok(())
    })
    .unwrap();
    gemini::grant_live_on(&dir, "ada", "2026-11-15").unwrap();
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    inbox.lock().unwrap().push_back(Ok(Recv::Closed { code: Some(1011), reason: "Quota exceeded for this model".into() }));
    let ev = s.poll(Duration::from_millis(1));
    let Some(LiveEvent::Ended(e)) = ev.last() else { panic!("{ev:?}") };
    assert_eq!(e.reason, EndReason::Limit);
    assert!(e.line.contains("free Gemini limit is reached"), "{}", e.line);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_hard_stop_ends_an_open_session_and_gemini_tool_calls_are_refused_without_results() {
    let _s = serial();
    let dir = temp("stop");
    allowed(&dir);
    let clock = FakeClock::new();
    let (c, inbox, sent) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    // a function call comes although none was offered: refused, and no result leaves
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"toolCall": {"functionCalls": [{"id": "c1", "name": "files_read", "args": {"p": "/secret"}}]}}).to_string())));
    let ev = s.poll(Duration::from_millis(1));
    assert!(ev.contains(&LiveEvent::ToolRefused(vec!["files_read".into()])));
    let last = sent.lock().unwrap().last().unwrap().clone();
    assert!(last.contains("toolResponse") && last.contains("tools are not available") && !last.contains("secret"), "{last}");
    assert!(matches!(live_state(), LiveState::Open { .. }));
    assert!(request_stop());
    let e = s.send_audio(&pcm(0.1)).unwrap_err();
    assert_eq!(e.reason, EndReason::Stopped);
    assert!(!mic_open());
    assert!(matches!(live_state(), LiveState::Closed { last: Some(_) }));
    assert!(!request_stop(), "nothing was open");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goaway_and_error_messages_are_told_plainly() {
    let _s = serial();
    let dir = temp("goaway");
    allowed(&dir);
    let clock = FakeClock::new();
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"goAway": {"timeLeft": "30s"}}).to_string())));
    assert!(s.poll(Duration::from_millis(1)).iter().any(|e| matches!(e, LiveEvent::GoAway(t) if t.contains("30s"))));
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"error": {"message": "API key not valid"}}).to_string())));
    let ev = s.poll(Duration::from_millis(1));
    assert!(matches!(ev.last(), Some(LiveEvent::Ended(e)) if e.reason == EndReason::Failed && e.line.contains("refused the key")), "{ev:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spoken_replies_are_shortened_at_a_sentence_and_point_to_the_screen() {
    let long = "This is the first sentence. ".repeat(40);
    let s = shorten_for_speech(&long, 150);
    let said = s.sentences.join(" ");
    assert!(said.len() < 150 + 60 && said.contains("on the screen"), "{said}");
    assert_eq!(s.full, long.trim());
    let md = shorten_for_speech("See [the page](https://example.org/a) for `code`.", 600);
    assert!(!md.sentences.join(" ").contains("https"), "{:?}", md.sentences);
    assert_eq!(shorten_for_speech("Short.", 600).sentences, vec!["Short.".to_string()]);
}

#[test]
fn settings_are_checked_and_off_by_default() {
    let dir = temp("cfg");
    let c = gemini::load(&dir);
    assert!(!c.voice.enabled && c.voice.key == "Super+V" && c.live_consent.is_empty());
    assert_eq!(valid_shortcut("super+v").unwrap(), "Super+V");
    assert_eq!(valid_shortcut("Ctrl + alt+F9").unwrap(), "Ctrl+Alt+F9");
    for bad in ["V", "Esc", "Super+Esc", "Super+VV", "Foo+V", "Super+Super+V", "Super+", "Ctrl+F13", ""] {
        assert!(valid_shortcut(bad).is_err(), "{bad}");
    }
    assert!(apply(&dir, &LiveChange::default()).is_err());
    assert!(apply(&dir, &LiveChange { max_session_secs: Some(5), ..Default::default() }).unwrap_err().contains("between 30 and 900"));
    assert!(apply(&dir, &LiveChange { model: Some("a b".into()), ..Default::default() }).is_err());
    let v = apply(&dir, &LiveChange { enabled: Some(true), key: Some("alt+g".into()), idle_secs: Some(12), ..Default::default() }).unwrap();
    assert!(v.enabled && v.key == "Alt+G" && v.idle_secs == 12);
    // the setting cannot give the consent
    assert!(gemini::live_consent_state(&dir, "ada") == gemini::ConsentState::Missing);
    // the old file without the new fields still loads (off)
    std::fs::write(dir.join(gemini::FILE), "{\"enabled\": true}").unwrap();
    assert!(!gemini::load(&dir).voice.enabled);
    let card = consent_card_live(Tier::Free);
    assert!(card.contains("EXPERIMENTAL") && card.contains("microphone") && card.contains("results of the tools") && card.contains("FREE"), "{card}");
    assert!(status_text(&dir, "ada", false, "2026-11-15").contains("EXPERIMENTAL") && status_text(&dir, "ada", false, "x").contains("OFF"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_consent_gate_really_knows_the_address_it_is_asked_about() {
    assert!(gemini::is_google_url(&gate_url()));
    let dir = temp("gate");
    let g = gemini::live_gate(&dir, Arc::new(|| "ada".to_string()));
    assert!(g.check_live(&gate_url()).is_err(), "no yes on file");
    assert!(g.check_live("https://example.org/x").is_ok(), "other hosts are not this gate's business");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_messages_have_the_documented_shapes() {
    let a = audio_message(&[0, 1, 2, 3]);
    assert_eq!(a, json!({"realtimeInput": {"audio": {"data": "AAECAw==", "mimeType": "audio/pcm;rate=16000"}}}));
    assert_eq!(audio_end_message(), json!({"realtimeInput": {"audioStreamEnd": true}}));
    assert_eq!(text_message("hi"), json!({"realtimeInput": {"text": "hi"}}));
    let u = connect_url("K");
    assert_eq!(u, "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key=K");
    assert!(setup_message("models/x")["setup"]["model"] == "models/x");
    // the setup of the reference's example: generationConfig.responseModalities, and tools beside it
    let with = setup_message_with("m", &[json!({"name": "radio_play", "description": "d"})]);
    assert_eq!(with["setup"]["tools"], json!([{"functionDeclarations": [{"name": "radio_play", "description": "d"}]}]));
    assert!(with["setup"]["systemInstruction"]["parts"][0]["text"].as_str().unwrap().contains("asks the person on the screen"));
    assert!(setup_message("m")["setup"].get("tools").is_none());
}

// ---------------------------------------------------------------------------------------------------------- slice 2: tool calls

use crate::geminilive_tools::tests::{rig as tool_rig, FakeScreen, FakeTools};

fn tool_call(id: &str, name: &str, args: Value) -> Recv {
    Recv::Text(json!({"toolCall": {"functionCalls": [{"id": id, "name": name, "args": args}]}}).to_string())
}

fn sent_json(sent: &Arc<Mutex<Vec<String>>>) -> Vec<Value> {
    sent.lock().unwrap().iter().filter_map(|t| serde_json::from_str(t).ok()).collect()
}

#[test]
fn function_calls_go_through_the_chat_wrappers_and_approvals_and_a_spoken_yes_approves_nothing() {
    let _s = serial();
    let dir = temp("tools");
    allowed(&dir);
    let screen = FakeScreen::at_screen();
    let (disp, tools, audits, _) = tool_rig(FakeTools::standard(), screen.clone(), false);
    let decls = disp.catalogue().decls.clone();
    let clock = FakeClock::new();
    let (c, inbox, sent) = mem();
    let mut s = start_with(&deps(&dir, c, &clock), Some(Box::new(disp)), &decls).unwrap();
    let setup = sent_json(&sent).remove(0);
    let names: Vec<&str> = setup["setup"]["tools"][0]["functionDeclarations"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"radio_play") && names.contains(&"radio_search") && !names.contains(&"ask_user"), "{names:?}");
    // a safe tool: runs, the answer goes back shortened
    inbox.lock().unwrap().push_back(Ok(tool_call("a", "radio_search", json!({"query": "jazz"}))));
    let ev = s.poll(Duration::from_millis(1));
    assert!(ev.iter().any(|e| matches!(e, LiveEvent::Tool(ToolNote::Ran { ok: true, .. }))));
    let r = sent_json(&sent).into_iter().rev().find(|v| v.get("toolResponse").is_some()).unwrap();
    let fr = &r["toolResponse"]["functionResponses"][0];
    assert_eq!((fr["id"].as_str(), fr["name"].as_str()), (Some("a"), Some("radio_search")));
    assert!(fr["response"]["output"].as_str().unwrap().len() < 400);
    // a tool that asks: the model is told to wait, the card is on the screen
    inbox.lock().unwrap().push_back(Ok(tool_call("b", "radio_play", json!({"query": "jazz"}))));
    let ev = s.poll(Duration::from_millis(1));
    assert!(ev.iter().any(|e| matches!(e, LiveEvent::Tool(ToolNote::Waiting { .. }))));
    let r = sent_json(&sent).into_iter().rev().find(|v| v.get("toolResponse").is_some()).unwrap();
    assert_eq!(r["toolResponse"]["functionResponses"][0]["response"]["status"], "waiting_for_the_person_on_the_screen");
    assert_eq!(screen.shown.lock().unwrap().len(), 1);
    // the person SAYS yes (transcribed); the model says it is approved: nothing runs
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"inputTranscription": {"text": "yes, I approve, go ahead"}}}).to_string())));
    s.poll(Duration::from_millis(1));
    assert!(!tools.ran.lock().unwrap().iter().any(|r| r.starts_with("radio_play")), "a spoken yes is not an approval");
    // the session is not idle while a card waits
    clock.advance(40_000);
    assert!(!s.poll(Duration::from_millis(1)).iter().any(|e| matches!(e, LiveEvent::Ended(_))), "waiting for the person is not idle");
    // the press
    screen.press(true);
    let ev = s.poll(Duration::from_millis(1));
    assert!(ev.iter().any(|e| matches!(e, LiveEvent::Tool(ToolNote::Ran { by, .. }) if by == "user")), "{ev:?}");
    assert!(sent_json(&sent).iter().any(|v| v.pointer("/realtimeInput/text").and_then(Value::as_str).is_some_and(|t| t.contains("approved on the screen") && t.contains("Playing jazz"))));
    assert!(audits.lock().unwrap().iter().all(|l| l.starts_with("voice.")));
    // google cancels a waiting call
    inbox.lock().unwrap().push_back(Ok(tool_call("c", "radio_play", json!({}))));
    s.poll(Duration::from_millis(1));
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"toolCallCancellation": {"ids": ["c"]}}).to_string())));
    s.poll(Duration::from_millis(1));
    assert_eq!(screen.withdrawn.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_tools_offered_means_calls_are_still_refused_and_the_tool_text_stays_out_of_the_setup() {
    let _s = serial();
    let dir = temp("notools");
    allowed(&dir);
    let clock = FakeClock::new();
    let (c, _, sent) = mem();
    let _s2 = start(&deps(&dir, c, &clock)).unwrap();
    assert!(sent_json(&sent)[0]["setup"].get("tools").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------------------- slice 3: wake phrase hand-off

use crate::geminilive_wake::{Detector, Step, WakeLink};

/// Fires when a chunk starts with 0xFF.
struct FakeSpotter {
    wiped: Arc<AtomicUsize>,
}
impl Detector for FakeSpotter {
    fn feed(&mut self, pcm: &[u8]) -> bool {
        pcm.first() == Some(&0xFF)
    }
    fn wipe(&mut self) {
        self.wiped.fetch_add(1, Ordering::SeqCst);
    }
}

fn chunk(b: u8) -> Vec<u8> {
    vec![b; 3200]
}

#[test]
fn the_wake_phrase_opens_the_session_and_nothing_is_sent_before_it_and_the_phrase_itself_is_never_sent() {
    let _s = serial();
    let dir = temp("wake");
    allowed(&dir);
    gemini::update(&dir, |c| {
        c.voice.wake_handoff = true;
        Ok(())
    })
    .unwrap();
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let (wake_on, chimes, wiped) = (Arc::new(std::sync::atomic::AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (w2, c2) = (wake_on.clone(), chimes.clone());
    let mut link = WakeLink::new(Box::new(FakeSpotter { wiped: wiped.clone() }), deps(&dir, Arc::new(ToFake(f.port)), &clock), Arc::new(move || w2.load(Ordering::SeqCst)), Box::new(move || {
        c2.fetch_add(1, Ordering::SeqCst);
    }), None);
    // before the phrase: a lot of talking, all local
    for _ in 0..20 {
        clock.advance(100);
        assert_eq!(link.feed(&chunk(7)), Step::Listening);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.conns.load(Ordering::SeqCst), 0, "no connection before the phrase");
    assert!(f.got.lock().unwrap().is_empty() && !mic_open(), "nothing sent and no session sign before the phrase");
    // the phrase
    let mut fired = chunk(0xFF);
    fired[1] = 0xFF;
    assert_eq!(link.feed(&fired), Step::Started);
    assert_eq!(chimes.load(Ordering::SeqCst), 1, "the chime");
    assert!(mic_open() && link.is_live(), "the red sign is on once it opens");
    assert!(wiped.load(Ordering::SeqCst) >= 1, "the spotter's ring was wiped");
    assert_eq!(link.feed(&chunk(9)), Step::Sent);
    assert_eq!(link.feed(&chunk(9)), Step::Sent);
    std::thread::sleep(Duration::from_millis(200));
    let sent = f.sent();
    let want = audio_message(&chunk(9));
    let audio: Vec<&Value> = sent.iter().filter(|v| v.pointer("/realtimeInput/audio").is_some()).collect();
    assert_eq!(audio.len(), 2);
    assert!(audio.iter().all(|a| **a == want), "only what came after the phrase was sent: neither the talk before it nor the phrase");
    link.release();
    assert!(!mic_open());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_hand_off_needs_every_opt_in_and_the_consent_and_says_why_when_it_cannot_start() {
    let _s = serial();
    let dir = temp("wake-optin");
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let wake_on = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (chimes, w2, c2) = (Arc::new(AtomicUsize::new(0)), wake_on.clone(), Arc::new(AtomicUsize::new(0)));
    let c3 = c2.clone();
    let mut link = WakeLink::new(Box::new(FakeSpotter { wiped: Arc::new(AtomicUsize::new(0)) }), deps(&dir, Arc::new(ToFake(f.port)), &clock), Arc::new(move || w2.load(Ordering::SeqCst)), Box::new(move || {
        c3.fetch_add(1, Ordering::SeqCst);
    }), None);
    let _ = chimes;
    let hit = chunk(0xFF);
    // everything off (defaults)
    assert_eq!(link.feed(&hit), Step::Off);
    // the wake phrase on, the assistant off
    wake_on.store(true, Ordering::SeqCst);
    clock.advance(1000);
    assert_eq!(link.feed(&hit), Step::Off);
    // assistant on, hand-off still off
    gemini::update(&dir, |c| {
        c.voice.enabled = true;
        Ok(())
    })
    .unwrap();
    clock.advance(1000);
    assert_eq!(link.feed(&hit), Step::Off);
    // hand-off on, but no yes
    gemini::update(&dir, |c| {
        c.voice.wake_handoff = true;
        Ok(())
    })
    .unwrap();
    clock.advance(1000);
    let Step::Fallback(line) = link.feed(&hit) else { panic!("a fallback") };
    assert!(line.contains("EXPERIMENTAL") && line.contains("no sound was sent to Google") && line.contains("local voice answers instead"), "{line}");
    assert_eq!(c2.load(Ordering::SeqCst), 1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.conns.load(Ordering::SeqCst), 0, "no yes: not even a connection");
    // switching the wake phrase off stops it at once
    wake_on.store(false, Ordering::SeqCst);
    clock.advance(1000);
    assert_eq!(link.feed(&hit), Step::Off);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------------------- slice 4: Gemini's own voice

use crate::geminilive_audio::{run_gemini_voice, AudioEvent, AudioOut, MicFront, LEARN_MS};
use crate::story::{Playing, Sink};

struct FakePlay {
    on: Arc<std::sync::atomic::AtomicBool>,
    stops: Arc<AtomicUsize>,
}
impl Playing for FakePlay {
    fn is_playing(&mut self) -> bool {
        self.on.load(Ordering::SeqCst)
    }
    fn stop(&mut self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.on.store(false, Ordering::SeqCst);
    }
}

/// Records every WAV it is given; a segment plays until the test lets it end (`finish`) or it is stopped.
#[derive(Default)]
struct FakeSink {
    wavs: Mutex<Vec<Vec<u8>>>,
    live: Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>,
    stops: Arc<AtomicUsize>,
}
impl FakeSink {
    fn finish(&self) {
        for l in self.live.lock().unwrap().iter() {
            l.store(false, Ordering::SeqCst);
        }
    }
}
impl Sink for FakeSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        self.wavs.lock().unwrap().push(wav.to_vec());
        let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.live.lock().unwrap().push(on.clone());
        Ok(Box::new(FakePlay { on, stops: self.stops.clone() }))
    }
}

fn out_rig(sink: Arc<FakeSink>, held: Option<&'static str>, clock: &FakeClock) -> (AudioOut, Arc<Mutex<crate::echocancel::Reference>>) {
    let reference = Arc::new(Mutex::new(crate::echocancel::Reference::default()));
    let t = clock.0.clone();
    let out = AudioOut::new(sink, reference.clone(), Arc::new(move || held.map(str::to_string)), Arc::new(move || t.load(Ordering::SeqCst)));
    (out, reference)
}

/// `ms` of 24 kHz noise as the model would send it.
fn voice24(ms: usize, seed: u32) -> Vec<u8> {
    let mut s = seed;
    (0..ms * 24).flat_map(|_| {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (((s >> 16) as i16) / 3).to_le_bytes()
    }).collect()
}

fn tone16(ms: usize, amp: f32) -> Vec<u8> {
    (0..ms * 16).flat_map(|i| ((amp * 32767.0 * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin()) as i16).to_le_bytes()).collect()
}

#[test]
fn the_models_voice_is_played_in_segments_through_the_shared_sink_one_after_another_and_only_in_memory() {
    let clock = FakeClock::new();
    let sink = Arc::new(FakeSink::default());
    let (mut out, reference) = out_rig(sink.clone(), None, &clock);
    // 100 ms pieces: nothing plays before 300 ms have arrived
    out.push(&voice24(100, 1));
    out.push(&voice24(100, 2));
    assert!(out.step().is_empty() && sink.wavs.lock().unwrap().is_empty());
    out.push(&voice24(100, 3));
    assert_eq!(out.step(), vec![AudioEvent::Started], "the first segment starts at 300 ms");
    let first = sink.wavs.lock().unwrap()[0].clone();
    let w = crate::voice::parse_wav(&first).unwrap();
    assert_eq!((w.rate, w.channels, w.samples.len()), (24_000, 1, 7200), "a WAV of the segment, 24 kHz mono, built in memory");
    // the next ones wait for the first to end: one at a time
    for i in 0..9 {
        out.push(&voice24(100, 10 + i));
    }
    assert!(out.step().is_empty(), "the first is still playing");
    sink.finish();
    out.step();
    assert_eq!(sink.wavs.lock().unwrap().len(), 2);
    out.push(&voice24(300, 40));
    out.end_turn();
    for _ in 0..4 {
        sink.finish();
        out.step();
    }
    sink.finish();
    let ev = out.step();
    assert!(ev.contains(&AudioEvent::Done) || sink.wavs.lock().unwrap().len() >= 3, "{ev:?}");
    let total: usize = sink.wavs.lock().unwrap().iter().map(|w| crate::voice::parse_wav(w).unwrap().samples.len()).sum();
    assert_eq!(total, 24 * (300 + 900 + 300), "every sample that arrived was played, once");
    // the echo reference knows what was played
    assert!(reference.lock().unwrap().for_capture(clock.0.load(Ordering::SeqCst), 800).iter().any(|v| v.abs() > 0.001));
    // the module writes nothing to disk and opens no other playback route
    let src = include_str!("geminilive_audio.rs");
    assert!(!src.contains("std::fs") && !src.contains("File::create") && !src.contains("spawn_player") && !src.contains("aplay"), "memory only, one playback route");
}

#[test]
fn when_another_program_holds_the_speaker_the_question_comes_first_and_a_no_drops_the_audio() {
    let clock = FakeClock::new();
    let sink = Arc::new(FakeSink::default());
    let (mut out, _) = out_rig(sink.clone(), Some("Radio is playing Radio X. Stop it so the Gemini voice can play?"), &clock);
    out.push(&voice24(400, 1));
    let ev = out.step();
    assert!(matches!(&ev[..], [AudioEvent::SpeakerHeld(q)] if q.contains("Radio is playing")), "{ev:?}");
    assert!(out.step().is_empty() && sink.wavs.lock().unwrap().is_empty(), "nothing plays and the question is asked once");
    out.push(&voice24(400, 2));
    out.answer_hold(true);
    assert_eq!(out.step(), vec![AudioEvent::Started]);
    // a no: nothing of that turn is played
    let sink2 = Arc::new(FakeSink::default());
    let (mut out2, _) = out_rig(sink2.clone(), Some("Radio is playing."), &clock);
    out2.push(&voice24(400, 3));
    out2.step();
    out2.answer_hold(false);
    out2.push(&voice24(400, 4));
    out2.end_turn();
    assert!(out2.step().is_empty() && sink2.wavs.lock().unwrap().is_empty() && !out2.busy());
}

#[test]
fn barge_in_stops_the_playing_segment_at_once_drops_the_queue_and_ignores_the_rest_of_that_answer() {
    let clock = FakeClock::new();
    let sink = Arc::new(FakeSink::default());
    let (mut out, reference) = out_rig(sink.clone(), None, &clock);
    out.push(&voice24(1000, 1));
    out.push(&voice24(1000, 2));
    out.step();
    assert!(out.playing());
    clock.advance(200);
    let d = out.barge_in();
    assert!(d.playing && d.queued_ms >= 800, "{d:?}");
    assert_eq!(sink.stops.load(Ordering::SeqCst), 1);
    assert!(!out.busy() && out.step().is_empty());
    // the echo reference stops at that instant
    assert!(reference.lock().unwrap().for_capture(clock.0.load(Ordering::SeqCst) + 100, 1600).iter().all(|v| *v == 0.0));
    // the answer's audio still on its way is not played
    out.push(&voice24(500, 3));
    out.end_turn();
    assert!(out.step().is_empty() && sink.wavs.lock().unwrap().len() == 1);
    // the person finished speaking: the next answer plays
    out.accept_new_turn();
    out.push(&voice24(400, 4));
    assert_eq!(out.step(), vec![AudioEvent::Started]);
}

#[test]
fn the_microphone_front_end_keeps_the_speakers_echo_out_and_lets_the_person_through() {
    let reference = Arc::new(Mutex::new(crate::echocancel::Reference::default()));
    // a long answer of the model, played from t = 1000 ms; the room returns it later and quieter
    let far24: Vec<i16> = voice24(6000, 9).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let far16 = crate::echocancel::resample_24k_to_16k(&far24);
    reference.lock().unwrap().add(1000, far16.clone());
    let echo: Vec<f32> = (0..far16.len()).map(|i| [(20usize, 0.5f32), (90, 0.25)].iter().map(|(d, g)| if i >= *d { far16[i - d] * g } else { 0.0 }).sum()).collect();
    let tone = |i: usize| 0.3 * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin();
    let mut front = MicFront::new(reference);
    let (mut sent_while_echo, mut barged_on_echo) = (0, false);
    let mut person_chunks = 0;
    let mut barge_at = None;
    for k in 0..55 {
        let (a, b) = (k * 1600, (k + 1) * 1600);
        let person = k >= 45;
        let mic: Vec<u8> = (a..b).flat_map(|i| (((echo[i] + if person { tone(i) } else { 0.0 }) * 32767.0) as i16).to_le_bytes()).collect();
        let f = front.step(&mic, 1000 + (k as u64 + 1) * 100, true);
        if !person && (k as u64) * 100 > LEARN_MS + 500 {
            sent_while_echo += usize::from(f.send.is_some());
            barged_on_echo |= f.barge;
        }
        if person {
            person_chunks += 1;
            if f.barge && barge_at.is_none() {
                barge_at = Some(person_chunks);
            }
        }
    }
    assert!(!barged_on_echo, "the speaker's own sound is never a barge-in once the filter has learned the room");
    assert!(sent_while_echo <= 2, "while the model speaks the echo is not sent to the model ({sent_while_echo} chunks leaked)");
    assert!(barge_at.is_some_and(|c| c <= 3), "the person talking over it is a barge-in within a few hundred ms: {barge_at:?}");
    // not speaking: everything goes, cleaned
    let f = front.step(&tone16(100, 0.001), 9000, false);
    assert!(f.send.is_some() && !f.barge);
}

#[test]
fn a_session_with_the_models_voice_plays_it_drops_it_when_the_local_voice_speaks_and_barges_in() {
    let _s = serial();
    let dir = temp("gvoice");
    allowed(&dir);
    gemini::update(&dir, |c| {
        c.voice.voice_out = VoiceOut::Gemini;
        Ok(())
    })
    .unwrap();
    let clock = FakeClock::new();
    let (c, inbox, sent) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    let audio = |seed: u32| Recv::Text(json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": crate::story::base64_encode(&voice24(400, seed))}}]}}}).to_string());
    for i in 0..3 {
        inbox.lock().unwrap().push_back(Ok(audio(i)));
    }
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"outputTranscription": {"text": "Hello."}, "turnComplete": true}}).to_string())));
    let sink = Arc::new(FakeSink::default());
    let (mut out, reference) = out_rig(sink.clone(), None, &clock);
    let mut front = MicFront::new(reference);
    // the person talks (a clear tone) while the model's audio plays, then the microphone goes
    let mut mic = Mic(VecDeque::from([Pcm::Quiet, Pcm::Quiet, Pcm::Chunk(tone16(100, 0.4)), Pcm::Chunk(tone16(100, 0.4)), Pcm::Chunk(tone16(100, 0.4)), Pcm::Chunk(tone16(100, 0.4)), Pcm::Gone]));
    let mut log = Log::default();
    let t = clock.0.clone();
    let mut events = Vec::new();
    let e = run_gemini_voice(&mut s, &mut mic, &mut front, &mut out, &mut log, &move || t.fetch_add(100, Ordering::SeqCst) + 100, &mut |a| events.push(a.clone()), &mut || None);
    assert_eq!(e.reason, EndReason::Stopped);
    assert!(events.contains(&AudioEvent::Started), "the model's voice was played: {events:?}");
    assert_eq!(sink.stops.load(Ordering::SeqCst), 1, "the person talking over it stopped the playback");
    assert!(sent_json(&sent).iter().any(|v| v.pointer("/realtimeInput/audio").is_some()), "the person's audio went on to Google");
    assert_eq!(log.0.last().map(|x| x.1.as_str()), Some("Hello."));
    assert!(sent_json(&sent)[0]["setup"]["realtimeInputConfig"]["activityHandling"] == "START_OF_ACTIVITY_INTERRUPTS");
    drop(s);
    // with the local voice (the default) the audio is dropped on arrival
    gemini::update(&dir, |c| {
        c.voice.voice_out = VoiceOut::Local;
        Ok(())
    })
    .unwrap();
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    inbox.lock().unwrap().push_back(Ok(audio(7)));
    let ev = s.poll(Duration::from_millis(1));
    assert!(!ev.iter().any(|e| matches!(e, LiveEvent::Audio(_))), "no audio reaches the caller when the local voice speaks");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_delay_from_the_end_of_speech_to_the_first_sound_is_logged_and_shown_in_the_status() {
    let _s = serial();
    let dir = temp("latency");
    allowed(&dir);
    clear_latency();
    assert!(status_text(&dir, "ada", true, "2026-11-15").contains("no turn measured yet"));
    let clock = FakeClock::new();
    for (wait, audio_after) in [(900u64, 1200u64), (1100, 1500), (700, 1000)] {
        let (c, inbox, _) = mem();
        gemini::update(&dir, |c| {
            c.voice.voice_out = VoiceOut::Gemini;
            Ok(())
        })
        .unwrap();
        let mut s = start(&deps(&dir, c, &clock)).unwrap();
        s.mark_speech_end();
        clock.advance(wait);
        inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"outputTranscription": {"text": "Hi"}}}).to_string())));
        s.poll(Duration::from_millis(1));
        clock.advance(audio_after - wait);
        inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": crate::story::base64_encode(&voice24(100, 1))}}]}, "turnComplete": true}}).to_string())));
        s.poll(Duration::from_millis(1));
    }
    let t = status_text(&dir, "ada", true, "2026-11-15");
    assert!(t.contains("first sound: last 1.0 s, median 1.2 s over 3 turns") || t.contains("first sound: last 1.0 s, median 1.5 s over 3 turns"), "{t}");
    assert!(t.contains("first text: last 0.7 s, median 0.9 s over 3 turns") && t.contains("Voice out: gemini"), "{t}");
    // a barge-in or an interruption throws the measurement away
    let (c, inbox, _) = mem();
    let mut s = start(&deps(&dir, c, &clock)).unwrap();
    s.mark_speech_end();
    inbox.lock().unwrap().push_back(Ok(Recv::Text(json!({"serverContent": {"interrupted": true, "turnComplete": true}}).to_string())));
    s.poll(Duration::from_millis(1));
    assert!(latency_text().contains("over 3 turns"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------------------- the host (what the shell runs)

use crate::geminilive_host::{spawn, Board, Parts, RunState, SPEAKER_CARD};
use crate::geminilive_tools::tests::FakeTools as HostTools;
use crate::wakelisten::Mic as WlMic;

#[derive(Default)]
struct HostMic {
    opens: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    open: bool,
}
impl WlMic for HostMic {
    fn open(&mut self) -> Result<(), String> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn drain(&mut self) -> Vec<i16> {
        vec![40; 1600]
    }
    fn alive(&mut self) -> bool {
        self.open
    }
    fn close(&mut self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.open = false;
    }
}

struct HostRig {
    parts: Parts,
    said: Arc<Mutex<Vec<String>>>,
    opens: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    sink: Arc<FakeSink>,
    board: Arc<Board>,
    audits: Arc<Mutex<Vec<String>>>,
    stopped_holder: Arc<AtomicUsize>,
}

fn host_rig(dir: &Path, port: u16, clock: &FakeClock, tools: Option<Arc<dyn crate::agent::Tools>>, held: bool) -> HostRig {
    let (sp, said, _) = speaker();
    let (opens, closes) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mic = HostMic { opens: opens.clone(), closes: closes.clone(), open: false };
    let sink = Arc::new(FakeSink::default());
    let board = Board::new();
    board.set_present(true);
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    let stopped_holder = Arc::new(AtomicUsize::new(0));
    let sh = stopped_holder.clone();
    let t = clock.0.clone();
    let parts = Parts {
        live: deps(dir, Arc::new(ToFake(port)), clock),
        tools,
        board: board.clone(),
        audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))),
        auto: Arc::new(|| false),
        mic: Box::new(mic),
        sink: sink.clone(),
        speaker: Box::new(sp),
        log: Box::new(Log::default()),
        held: Arc::new(move || held.then(|| "Radio is playing. Stop it so Gemini can speak?".to_string())),
        stop_holder: Arc::new(move || {
            sh.fetch_add(1, Ordering::SeqCst);
            Ok("Stopped.".into())
        }),
        echo_clock: Arc::new(move || t.load(Ordering::SeqCst)),
    };
    HostRig { parts, said, opens, closes, sink, board, audits, stopped_holder }
}

fn until(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(8), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_manual_session_runs_a_whole_turn_on_its_thread_and_the_local_voice_reads_the_answer() {
    let _s = serial();
    let dir = temp("host-turn");
    allowed(&dir);
    let f = fake(answering(vec![json!({"serverContent": {"inputTranscription": {"text": "hello"}}}), say_text("Good morning."), json!({"serverContent": {"turnComplete": true}})]), None);
    let clock = FakeClock::new();
    let rig = host_rig(&dir, f.port, &clock, None, false);
    let (said, closes, audits) = (rig.said.clone(), rig.closes.clone(), rig.audits.clone());
    let run = spawn(rig.parts);
    until("the session to open", || run.state() == RunState::Open);
    assert!(mic_open() && crate::clientmedia::indicator().microphones.contains(&INDICATOR_NAME.to_string()), "the red sign while the microphone is open");
    until("audio to reach the server", || f.audio_messages() >= 1);
    run.finish_turn();
    until("the session to end", || matches!(run.state(), RunState::Ended(_)));
    assert_eq!(said.lock().unwrap().join(" "), "Good morning.");
    assert_eq!(closes.load(Ordering::SeqCst), 1, "the microphone was closed");
    assert!(!mic_open() && crate::clientmedia::indicator().microphones.is_empty());
    let a = audits.lock().unwrap().join("\n");
    assert!(a.contains("voice.session started") && a.contains("voice.session ended"), "{a}");
    run.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_the_yes_the_session_falls_back_with_one_line_and_the_microphone_never_opens() {
    let _s = serial();
    let dir = temp("host-refuse");
    gemini::update(&dir, |c| {
        c.voice.enabled = true;
        Ok(())
    })
    .unwrap();
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let rig = host_rig(&dir, f.port, &clock, None, false);
    let opens = rig.opens.clone();
    let run = spawn(rig.parts);
    until("the fallback", || matches!(run.state(), RunState::Fallback(_)));
    let RunState::Fallback(l) = run.state() else { unreachable!() };
    assert!(l.contains("EXPERIMENTAL") && l.contains("no sound was sent to Google") && l.contains("local voice answers instead"), "{l}");
    assert_eq!(opens.load(Ordering::SeqCst), 0, "the microphone was never opened");
    assert_eq!(f.conns.load(Ordering::SeqCst), 0, "nothing was sent, not even a connection");
    run.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tool_that_asks_shows_its_card_on_the_board_and_runs_only_after_the_press() {
    let _s = serial();
    let dir = temp("host-card");
    allowed(&dir);
    let mut n = 0;
    let script: Script = Box::new(move |v| {
        if v.get("setup").is_some() {
            return vec![ok_json(json!({"setupComplete": {}}))];
        }
        if v.pointer("/realtimeInput/audio").is_some() {
            n += 1;
            if n == 1 {
                return vec![ok_json(json!({"toolCall": {"functionCalls": [{"id": "k1", "name": "radio_play", "args": {"query": "jazz"}}]}}))];
            }
        }
        if v.pointer("/realtimeInput/text").and_then(Value::as_str).is_some_and(|t| t.contains("approved on the screen")) {
            return vec![ok_json(say_text("Playing jazz now.")), ok_json(json!({"serverContent": {"turnComplete": true}}))];
        }
        vec![]
    });
    let f = fake(script, None);
    let clock = FakeClock::new();
    let tools = Arc::new(HostTools::standard());
    let rig = host_rig(&dir, f.port, &clock, Some(tools.clone()), false);
    let (board, said) = (rig.board.clone(), rig.said.clone());
    let run = spawn(rig.parts);
    until("the card", || !board.pending().is_empty());
    let card = board.pending().remove(0);
    assert_eq!(card.name, "radio_play");
    assert!(tools.ran.lock().unwrap().is_empty(), "nothing ran before the press");
    assert!(board.press(&card.id, true));
    until("the answer", || !said.lock().unwrap().is_empty());
    assert!(tools.ran.lock().unwrap().iter().any(|r| r.starts_with("radio_play")));
    assert_eq!(said.lock().unwrap()[0], "Playing jazz now.");
    run.stop();
    until("the end", || matches!(run.state(), RunState::Ended(_)));
    run.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn esc_ends_the_session_at_once_and_closes_the_microphone() {
    let _s = serial();
    let dir = temp("host-esc");
    allowed(&dir);
    let f = fake(answering(vec![]), None);
    let clock = FakeClock::new();
    let rig = host_rig(&dir, f.port, &clock, None, false);
    let closes = rig.closes.clone();
    let run = spawn(rig.parts);
    until("open", || run.state() == RunState::Open);
    run.stop();
    until("the end", || matches!(run.state(), RunState::Ended(_)));
    let RunState::Ended(l) = run.state() else { unreachable!() };
    assert!(l.contains("stopped"), "{l}");
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(!mic_open());
    run.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gemini_voice_asks_the_speaker_question_as_a_card_and_plays_after_a_yes() {
    let _s = serial();
    let dir = temp("host-speaker");
    allowed(&dir);
    gemini::update(&dir, |c| {
        c.voice.voice_out = VoiceOut::Gemini;
        Ok(())
    })
    .unwrap();
    let audio = |seed: u32| json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": crate::story::base64_encode(&voice24(400, seed))}}]}}});
    let mut first = true;
    let script: Script = Box::new(move |v| {
        if v.get("setup").is_some() {
            return vec![ok_json(json!({"setupComplete": {}}))];
        }
        if v.pointer("/realtimeInput/audio").is_some() && first {
            first = false;
            return vec![ok_json(audio(1)), ok_json(audio(2)), ok_json(json!({"serverContent": {"turnComplete": true}}))];
        }
        vec![]
    });
    let f = fake(script, None);
    let clock = FakeClock::new();
    let rig = host_rig(&dir, f.port, &clock, None, true);
    let (board, sink, stopped) = (rig.board.clone(), rig.sink.clone(), rig.stopped_holder.clone());
    let run = spawn(rig.parts);
    until("the speaker card", || board.pending().iter().any(|c| c.id == SPEAKER_CARD));
    assert!(sink.wavs.lock().unwrap().is_empty(), "nothing plays before the answer");
    assert!(board.press(SPEAKER_CARD, true));
    until("the playback", || !sink.wavs.lock().unwrap().is_empty());
    assert_eq!(stopped.load(Ordering::SeqCst), 1, "the holder was asked to stop after the yes");
    run.stop();
    until("the end", || matches!(run.state(), RunState::Ended(_)));
    run.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_language_is_checked_stored_told_to_gemini_and_left_out_when_automatic() {
    assert_eq!(valid_language("TA-in").unwrap(), "ta-IN");
    assert_eq!(valid_language("auto").unwrap(), "");
    assert_eq!(valid_language("").unwrap(), "");
    assert!(valid_language("klingon").unwrap_err().contains("must be one of"));
    assert!(valid_language("en-US; drop").is_err());
    let plain = setup_message_lang("m", &[], "");
    assert!(plain.pointer("/setup/generationConfig/speechConfig").is_none() && !plain.to_string().contains("Speak and answer in"));
    let ta = setup_message_lang("m", &[], "ta-IN");
    assert_eq!(ta.pointer("/setup/generationConfig/speechConfig/languageCode").and_then(|v| v.as_str()), Some("ta-IN"));
    assert!(ta.to_string().contains("Speak and answer in Tamil (ta-IN)."));
    assert_eq!(ta.pointer("/setup/generationConfig/responseModalities/0").and_then(|v| v.as_str()), Some("AUDIO"), "the rest of the setup is unchanged");
    assert_eq!(setup_message_with("m", &[]), setup_message_lang("m", &[], ""), "the old entry point is the automatic one");
    assert_eq!(setup_message_lang("m", &[], "xx-XX").to_string(), plain.to_string(), "a code that is not on the list is left out, never sent");
    let dir = std::env::temp_dir().join(format!("iyakki-gl-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(apply(&dir, &LiveChange { language: Some("nope".into()), ..Default::default() }).is_err());
    assert_eq!(apply(&dir, &LiveChange { language: Some("hi-IN".into()), ..Default::default() }).unwrap().language, "hi-IN");
    assert_eq!(apply(&dir, &LiveChange { language: Some("auto".into()), ..Default::default() }).unwrap().language, "");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cost_meter_counts_the_tokens_google_reports_per_day_and_says_it_is_a_guide() {
    assert_eq!(usage_tokens(&serde_json::json!({"usageMetadata": {"totalTokenCount": 120}})), Some(120));
    assert_eq!(usage_tokens(&serde_json::json!({"serverContent": {}})), None);
    assert_eq!(usage_tokens(&serde_json::json!({"usageMetadata": {"totalTokenCount": 99_999_999_999u64}})), None, "an absurd number is not believed");
    let mut c = LiveCfg { used_date: "2026-11-16".into(), used_tokens: 4200, language: "ta-IN".into(), ..LiveCfg::default() };
    assert_eq!(c.tokens_today("2026-11-16"), 4200);
    assert_eq!(c.tokens_today("2026-11-17"), 0, "a new day starts again");
    let t = meter_text(&c, "2026-11-16");
    assert!(t.contains("Tokens Google reported today: 4200") && t.contains("Google's own bill is the truth") && t.contains("Language: Tamil."), "{t}");
    c.used_tokens = 0;
    assert!(!meter_text(&c, "2026-11-16").contains("Tokens"), "nothing counted, nothing said");
    assert!(meter_text(&LiveCfg::default(), "d").contains("Language: automatic."));
}

#[test]
fn the_next_segment_starts_a_little_before_the_playing_one_ends_and_the_two_fade_so_the_join_has_no_gap() {
    use crate::geminilive_audio::OVERLAP_MS;
    let clock = FakeClock::new();
    let sink = Arc::new(FakeSink::default());
    let (mut out, _) = out_rig(sink.clone(), None, &clock);
    // loud, steady sound so a fade is easy to see
    let loud = |ms: usize| -> Vec<u8> { (0..ms * 24).flat_map(|_| 10_000i16.to_le_bytes()).collect() };
    out.push(&loud(300));
    assert_eq!(out.step(), vec![AudioEvent::Started]);
    out.push(&loud(800));
    out.push(&loud(100));
    // not yet: the first (300 ms) has more than OVERLAP_MS to go
    clock.advance(100);
    assert!(out.step().is_empty() && sink.wavs.lock().unwrap().len() == 1);
    // OVERLAP_MS before its end the second one starts, while the first is still playing
    clock.advance(300 - 100 - OVERLAP_MS);
    out.step();
    assert_eq!(sink.wavs.lock().unwrap().len(), 2, "started early");
    assert!(sink.live.lock().unwrap()[0].load(Ordering::SeqCst), "the first has not ended");
    let (a, b) = {
        let w = sink.wavs.lock().unwrap();
        (crate::voice::parse_wav(&w[0]).unwrap().samples, crate::voice::parse_wav(&w[1]).unwrap().samples)
    };
    let n = (24 * OVERLAP_MS) as usize;
    // the end of the first fades to nothing, the start of the second comes in from nothing, the middle of both is untouched
    assert!(a[a.len() - 1].abs() < 300 && a[a.len() - n].abs() > 9000, "the first fades out: {} {}", a[a.len() - 1], a[a.len() - n]);
    assert!(b[0].abs() < 10 && b[n - 1].abs() > 9000 && b[b.len() / 2] == 10_000, "the second fades in");
    // the two add to about the same level through the overlap (a straight crossfade)
    let mid = n / 2;
    let sum = i32::from(a[a.len() - n + mid]) + i32::from(b[mid]);
    assert!((sum - 10_000).abs() < 400, "constant level across the join: {sum}");
    // the first ending later changes nothing; the third segment follows the same way
    sink.live.lock().unwrap()[0].store(false, Ordering::SeqCst);
    out.step();
    assert!(out.playing());
    // the last segment of a turn has no fade out
    out.end_turn();
    clock.advance(900);
    for _ in 0..4 {
        sink.finish();
        out.step();
    }
    let last = sink.wavs.lock().unwrap().last().cloned().unwrap();
    let l = crate::voice::parse_wav(&last).unwrap().samples;
    assert_eq!(l[l.len() - 1], 10_000, "the end of the answer is not faded");
    // a barge-in stops both the playing and the early one
    let sink2 = Arc::new(FakeSink::default());
    let (mut o2, _) = out_rig(sink2.clone(), None, &clock);
    o2.push(&loud(300));
    o2.step();
    o2.push(&loud(900));
    clock.advance(300);
    o2.step();
    assert_eq!(sink2.wavs.lock().unwrap().len(), 2);
    o2.barge_in();
    assert_eq!(sink2.stops.load(Ordering::SeqCst), 2, "both were stopped");
}
