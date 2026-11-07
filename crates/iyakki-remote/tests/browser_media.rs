//! The browser's clipboard items, sound, microphone and camera over the real server and a real WebSocket, with a fake screen that
//! owns the session's `BrowserMedia`. Nothing here uses a real browser, microphone or camera.

use iyakki_agent::browsermedia::{BrowserMedia, Kind, Policy};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Fake {
    media: Arc<BrowserMedia>,
    inputs: Arc<Mutex<Vec<String>>>,
    rich: Arc<Mutex<Option<Vec<u8>>>>,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.inputs.lock().unwrap().push(format!("{i:?}"));
    }
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        std::thread::sleep(Duration::from_millis(5));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn media(&self) -> Option<Arc<BrowserMedia>> {
        Some(self.media.clone())
    }
    fn take_rich_copy(&mut self) -> Option<Vec<u8>> {
        self.rich.lock().unwrap().take()
    }
    fn finished(&self) -> bool {
        false
    }
}

struct World {
    media: Arc<BrowserMedia>,
    audit: Arc<Mutex<Vec<String>>>,
    inputs: Arc<Mutex<Vec<String>>>,
    rich: Arc<Mutex<Option<Vec<u8>>>>,
}

struct G(Arc<BrowserMedia>, Arc<Mutex<Vec<String>>>, Arc<Mutex<Option<Vec<u8>>>>);
impl Gate for G {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { media: self.0.clone(), inputs: self.1.clone(), rich: self.2.clone() }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

fn start(policy: Policy) -> (Server, World) {
    let audit = Arc::new(Mutex::new(Vec::new()));
    let a2 = audit.clone();
    let media = BrowserMedia::new(policy, Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))), None);
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let rich = Arc::new(Mutex::new(None));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(G(media.clone(), inputs.clone(), rich.clone()))).unwrap();
    (srv, World { media, audit, inputs, rich })
}

fn login(addr: SocketAddr) -> TcpStream {
    let body = r#"{"user":"ann","password":"right"}"#;
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("POST /api/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let cookie = text.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(text.split_once("\r\n\r\n").unwrap().1).unwrap()["csrf"].as_str().unwrap().to_string();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nCookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).unwrap_or(0) == 0 {
            break;
        }
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).contains(" 101 "), "upgraded");
    s
}

fn masked(op: u8, p: &[u8]) -> Vec<u8> {
    let mask = [1u8, 2, 3, 4];
    let mut f = vec![0x80 | op];
    match p.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n < 65536 => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(&mask);
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

fn text(s: &mut TcpStream, t: &str) {
    s.write_all(&masked(1, t.as_bytes())).unwrap();
}
fn binary(s: &mut TcpStream, ch: u8, body: &[u8]) {
    let mut v = vec![ch];
    v.extend_from_slice(body);
    s.write_all(&masked(2, &v)).unwrap();
}

fn read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).unwrap();
    let len = match h[1] & 0x7f {
        126 => {
            let mut l = [0u8; 2];
            s.read_exact(&mut l).unwrap();
            u16::from_be_bytes(l) as usize
        }
        127 => {
            let mut l = [0u8; 8];
            s.read_exact(&mut l).unwrap();
            u64::from_be_bytes(l) as usize
        }
        n => n as usize,
    };
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).unwrap();
    (h[0] & 0x0f, p)
}

/// Frames until one matches, or panic after a few seconds.
fn until(s: &mut TcpStream, what: &str, mut ok: impl FnMut(u8, &[u8]) -> bool) -> (u8, Vec<u8>) {
    let t0 = Instant::now();
    loop {
        let (op, p) = read_frame(s);
        if ok(op, &p) {
            return (op, p);
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "never got {what}");
    }
}

fn wait(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(5), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn has(p: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(p).contains(needle)
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nbody";

#[test]
fn the_page_is_told_what_is_allowed_and_a_switch_then_binary_microphone_and_camera_data_reach_the_session() {
    let (srv, w) = start(Policy::allow_all());
    let mut s = login(srv.local_addr());
    text(&mut s, r#"{"t":"hello","v":2}"#);
    let (_, st) = until(&mut s, "media_state", |op, p| op == 1 && has(p, "media_state"));
    assert!(has(&st, "\"mic\":false") && has(&st, "\"allowed\""), "{}", String::from_utf8_lossy(&st));

    binary(&mut s, 1, &[1, 0, 2, 0]); // before the switch: dropped
    text(&mut s, r#"{"t":"media","kind":"mic","on":true}"#);
    wait("mic on", || w.media.is_on(Kind::Mic));
    binary(&mut s, 1, &[5, 0, 6, 0]);
    wait("mic data", || !w.media.take_mic().is_empty() && w.audit.lock().unwrap().iter().any(|l| l == "browser.media.on mic"));
    // the new state is sent back
    until(&mut s, "state with mic on", |op, p| op == 1 && has(p, "\"mic\":true"));

    text(&mut s, r#"{"t":"media","kind":"camera","on":true}"#);
    wait("camera on", || w.media.is_on(Kind::Camera));
    binary(&mut s, 2, &[0xFF, 0xD8, 0xFF, 0xD9]);
    wait("camera frame", || w.media.camera().is_some());
    assert_eq!(w.media.camera().unwrap(), vec![0xFF, 0xD8, 0xFF, 0xD9]);

    // closing the page switches everything off
    drop(s);
    wait("everything off", || !w.media.is_on(Kind::Mic) && !w.media.is_on(Kind::Camera) && w.media.camera().is_none());
    let audit = w.audit.lock().unwrap().clone();
    assert!(audit.contains(&"browser.media.off mic".to_string()) && audit.contains(&"browser.media.off camera".to_string()), "{audit:?}");
    assert!(audit.iter().all(|l| l.starts_with("browser.")), "kind and time only: {audit:?}");
}

#[test]
fn a_denied_policy_answers_the_switch_with_a_denial_and_keeps_nothing() {
    let (srv, w) = start(Policy::deny_all());
    let mut s = login(srv.local_addr());
    text(&mut s, r#"{"t":"media","kind":"camera","on":true}"#);
    until(&mut s, "media_denied", |op, p| op == 1 && has(p, "media_denied") && has(p, "camera"));
    binary(&mut s, 2, &[0xFF, 0xD8, 1, 2]);
    binary(&mut s, 3, PNG);
    std::thread::sleep(Duration::from_millis(200));
    assert!(w.media.camera().is_none() && w.media.clips().is_empty());
    assert!(w.audit.lock().unwrap().iter().any(|l| l == "browser.media.denied camera"));
    assert!(w.inputs.lock().unwrap().iter().all(|i| !i.starts_with("Rich")));
}

#[test]
fn clipboard_images_and_html_go_in_both_ways_with_limits() {
    let (srv, w) = start(Policy::allow_all());
    let mut s = login(srv.local_addr());
    binary(&mut s, 3, PNG);
    binary(&mut s, 4, "<b>hi</b>".as_bytes());
    binary(&mut s, 3, b"not a png");
    wait("rich input", || w.inputs.lock().unwrap().iter().filter(|i| i.starts_with("Rich")).count() == 2);
    assert_eq!(w.media.clips().len(), 2, "the bad one was dropped");
    // a PNG of nearly the largest size still fits in one message
    let mut big = PNG.to_vec();
    big.resize(iyakki_agent::browsermedia::MAX_CLIP_IMAGE - 8, 7);
    binary(&mut s, 3, &big);
    wait("big image", || w.media.clips().len() == 3);

    // out: only a version 2 page (it said hello) gets binary clipboard messages
    text(&mut s, r#"{"t":"hello","v":2}"#);
    *w.rich.lock().unwrap() = BrowserMedia::clip_out(iyakki_agent::browsermedia::ClipKind::Image, PNG);
    let (op, p) = until(&mut s, "image message", |op, p| op == 2 && p.first() == Some(&0x11));
    assert_eq!((op, &p[1..]), (2, PNG));
}

#[test]
fn sound_reaches_a_version_2_page_only_after_it_started_its_sound() {
    let (srv, w) = start(Policy::allow_all());
    let mut s = login(srv.local_addr());
    text(&mut s, r#"{"t":"hello","v":2}"#);
    assert!(!w.media.push_audio(48_000, 2, &[1, 2, 3, 4]), "the page has not started its sound");
    text(&mut s, r#"{"t":"audio","on":true}"#);
    wait("audio on", || w.media.sources().audio_out);
    assert!(w.media.push_audio(48_000, 2, &[1, 2, 3, 4]));
    let (_, p) = until(&mut s, "sound message", |op, p| op == 2 && p.first() == Some(&0x10));
    let a = (p[1], u32::from_le_bytes(p[2..6].try_into().unwrap()), &p[10..]);
    assert_eq!(a, (2, 48_000, &[1u8, 0, 2, 0, 3, 0, 4, 0][..]));
    text(&mut s, r#"{"t":"audio","on":false}"#);
    wait("audio off", || !w.media.sources().audio_out);
    assert!(!w.media.push_audio(48_000, 2, &[1, 2]));
}

#[test]
fn the_microphone_limit_is_told_to_the_page() {
    let (srv, w) = start(Policy::allow_all());
    let mut s = login(srv.local_addr());
    text(&mut s, r#"{"t":"media","kind":"mic","on":true}"#);
    wait("mic on", || w.media.is_on(Kind::Mic));
    // 300 seconds of sound in one-second messages
    for _ in 0..300 {
        binary(&mut s, 1, &vec![0u8; 32_000]);
        // keep the receive side drained so the server's writes never block
        s.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let mut sink = [0u8; 4096];
        let _ = s.read(&mut sink);
    }
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    wait("limit", || !w.media.is_on(Kind::Mic));
    assert!(w.audit.lock().unwrap().iter().any(|l| l == "browser.media.limit mic"));
}

#[test]
fn a_tile_asking_for_the_microphone_reaches_the_page_and_a_broadcast_reaches_only_a_started_page() {
    let (srv, w) = start(Policy::allow_all());
    let mut s = login(srv.local_addr());
    text(&mut s, r#"{"t":"hello","v":2}"#);
    assert_eq!(w.media.request(Kind::Mic), iyakki_agent::browsermedia::Requested::Asked);
    let (_, p) = until(&mut s, "media_ask", |op, p| op == 1 && String::from_utf8_lossy(p).contains("media_ask"));
    assert!(String::from_utf8_lossy(&p).contains("\"mic\""));
    assert!(!w.media.is_on(Kind::Mic), "asking does not switch it on");
    // the mixer's tap: nothing is taken until the page starts its sound
    iyakki_agent::browsermedia::register("ann", w.media.clone());
    assert!(!iyakki_agent::browsermedia::broadcast_audio(48_000, 2, &[1, 2]));
    text(&mut s, r#"{"t":"audio","on":true}"#);
    wait("audio on", || w.media.sources().audio_out);
    assert!(iyakki_agent::browsermedia::broadcast_audio(48_000, 2, &[9, 9]));
    until(&mut s, "sound message", |op, p| op == 2 && p.first() == Some(&0x10));
    iyakki_agent::browsermedia::unregister("ann");
}

#[test]
fn a_denied_policy_is_not_asked_through_the_page() {
    let (_srv, w) = start(Policy::deny_all());
    assert_eq!(w.media.request(Kind::Camera), iyakki_agent::browsermedia::Requested::Denied);
    assert!(w.audit.lock().unwrap().iter().any(|l| l == "browser.media.denied camera"));
    assert!(w.media.take_messages().iter().all(|m| !m.contains("media_ask")));
}
