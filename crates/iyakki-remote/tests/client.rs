//! The session endpoint for clients over a real local socket, against fake screens and a fake core (no TLS, no hardware): hello,
//! one-time link and enrolment, the signed challenge, the person's password, a broker call allowed and one refused, a picture frame,
//! input, events with resume, and a revoke that closes the open session. Then a refusal test for every route and every step.

use iyakki_agent::clientlink as link;
use iyakki_core_api::appcaps::{record_approval, Approval, CapabilityRequest};
use iyakki_core_api::broker::{client_key, ApprovalLookup, Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_core_api::client::{self as proto, ClientMsg, ErrorCode, ServerMsg};
use iyakki_link::sigscheme::DeviceKey;
use iyakki_remote::clientsession::{CallHandler, ClientEndpoint, FailLimit};
use iyakki_remote::codec::{decode_frame, Canvas};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------------------------ the fakes

struct Pic {
    rgba: Vec<u8>,
    ver: u64,
    delivered: u64,
    inputs: Vec<String>,
    viewing: bool,
}

struct Fake {
    pic: Arc<Mutex<Pic>>,
    armed: bool,
}

const W: u32 = 128;
const H: u32 = 64;

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.pic.lock().unwrap().inputs.push(format!("{i:?}"));
    }
    fn viewing(&mut self, on: bool) {
        self.pic.lock().unwrap().viewing = on;
    }
    fn step(&mut self) -> Option<Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        let mut p = self.pic.lock().unwrap();
        if self.armed && p.ver != p.delivered {
            self.armed = false;
            p.delivered = p.ver;
            return Some(Pixels { w: W, h: H, rgba: p.rgba.clone() });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    pic: Arc<Mutex<Pic>>,
    ep: Arc<ClientEndpoint>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { pic: self.pic.clone(), armed: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn client_endpoint(&self) -> Option<&ClientEndpoint> {
        Some(&self.ep)
    }
}

/// Every client device named in `granted` holds `camera.view` and nothing else.
struct Book(Mutex<Vec<String>>);
impl ApprovalLookup for Book {
    fn approval(&self, installation: &str) -> Option<Approval> {
        let ids = self.0.lock().unwrap();
        ids.iter().any(|i| client_key(i) == installation).then(|| record_approval("ann", "client", "1", "", 1, &[CapabilityRequest { name: "camera.view".into(), why: "test".into(), param: None, extra: Default::default() }]))
    }
}

struct NoAsk;
impl CardAsk for NoAsk {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        CardAnswer::Unavailable
    }
}

struct Calls;
impl CallHandler for Calls {
    fn role_allows(&self, person: &str, capability: &str) -> bool {
        person == "ann" && capability != "secrets.read"
    }
    fn perform(&self, _: &str, capability: &str, unit: Option<&str>) -> Result<String, String> {
        Ok(format!("{capability}:{}", unit.unwrap_or("-")))
    }
}

struct Rig {
    srv: Server,
    ep: Arc<ClientEndpoint>,
    data: PathBuf,
    pic: Arc<Mutex<Pic>>,
    log: Arc<MemoryLog>,
    audit: Arc<Mutex<Vec<String>>>,
    book: Arc<Book>,
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-clientsess-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig(tag: &str, on: bool, limit: usize) -> Rig {
    let data = dir(tag);
    if on {
        link::set_enabled(&data, true).unwrap();
    }
    let log = Arc::new(MemoryLog::new(200));
    let book = Arc::new(Book(Mutex::new(vec![])));
    let broker = Arc::new(Broker::new(book.clone(), Arc::new(NoAsk), log.clone(), Arc::new(|| iyakki_agent::clock::now_unix())));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let ep = Arc::new(ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(move |u: &str, a: &str, d: &str| a2.lock().unwrap().push(format!("{u}|{a}|{d}"))), FailLimit::new(limit, Duration::from_secs(60))));
    let rgba: Vec<u8> = (0..W * H).flat_map(|_| [10u8, 20, 30, 255]).collect();
    let pic = Arc::new(Mutex::new(Pic { rgba, ver: 1, delivered: 0, inputs: vec![], viewing: false }));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { pic: pic.clone(), ep: ep.clone() })).unwrap();
    Rig { srv, ep, data, pic, log, audit, book }
}

// ------------------------------------------------------------------------------------------------------------------ the client

fn masked(op: u8, p: &[u8]) -> Vec<u8> {
    let mask = [7u8, 1, 4, 2];
    let mut f = vec![0x80 | op];
    if p.len() < 126 {
        f.push(0x80 | p.len() as u8);
    } else if p.len() < 65536 {
        f.push(0x80 | 126);
        f.extend_from_slice(&(p.len() as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(p.len() as u64).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

enum Got {
    Msg(ServerMsg),
    Bin(Vec<u8>),
    Close,
}

struct Client {
    s: TcpStream,
}

fn raw_http(addr: SocketAddr, raw: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    (text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), text)
}

fn upgrade_request(addr: SocketAddr, extra: &str) -> String {
    format!("GET /client/ws HTTP/1.1\r\nHost: {addr}\r\n{extra}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n")
}

impl Client {
    fn open(addr: SocketAddr) -> Client {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(upgrade_request(addr, "").as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));
        Client { s }
    }

    fn send(&mut self, m: &ClientMsg) {
        self.s.write_all(&masked(1, proto::encode_client(m).as_bytes())).unwrap();
    }

    fn send_raw(&mut self, t: &str) {
        self.s.write_all(&masked(1, t.as_bytes())).unwrap();
    }

    fn next(&mut self, wait: Duration) -> Option<Got> {
        self.s.set_read_timeout(Some(wait)).unwrap();
        let mut h = [0u8; 2];
        if self.s.read_exact(&mut h).is_err() {
            return None;
        }
        self.s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let len = match h[1] & 0x7f {
            126 => {
                let mut l = [0u8; 2];
                self.s.read_exact(&mut l).ok()?;
                u16::from_be_bytes(l) as usize
            }
            127 => {
                let mut l = [0u8; 8];
                self.s.read_exact(&mut l).ok()?;
                u64::from_be_bytes(l) as usize
            }
            n => n as usize,
        };
        let mut p = vec![0u8; len];
        self.s.read_exact(&mut p).ok()?;
        Some(match h[0] & 0x0f {
            1 => Got::Msg(proto::decode_server(std::str::from_utf8(&p).unwrap()).expect("a message of the protocol")),
            2 => Got::Bin(p),
            _ => Got::Close,
        })
    }

    /// The next text message (binary ones are skipped), or panic after 8 s.
    fn msg(&mut self) -> ServerMsg {
        let t0 = Instant::now();
        loop {
            assert!(t0.elapsed() < Duration::from_secs(8), "no message");
            match self.next(Duration::from_millis(200)) {
                Some(Got::Msg(m)) => return m,
                Some(Got::Close) => panic!("closed"),
                _ => {}
            }
        }
    }

    fn err(&mut self) -> ErrorCode {
        match self.msg() {
            ServerMsg::Error { code, .. } => code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn binary(&mut self) -> Vec<u8> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(8) {
            if let Some(Got::Bin(b)) = self.next(Duration::from_millis(100)) {
                return b;
            }
        }
        panic!("no picture");
    }

    /// True once the instance has closed the connection (a close frame or end of stream) within `wait`.
    fn closed_within(&mut self, wait: Duration) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            let mut b = [0u8; 1];
            self.s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.s.peek(&mut b) {
                Ok(0) => return true,
                Ok(_) => {
                    if let Some(Got::Close) = self.next(Duration::from_millis(200)) {
                        return true;
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }

    fn hello(&mut self) -> String {
        self.send(&ClientMsg::Hello { v: 1, min_v: 1, offered: vec!["picture".into(), "input".into(), "events".into(), "calls".into()], needed: vec![], client: "test".into() });
        match self.msg() {
            ServerMsg::HelloOk { challenge, caps, v, .. } => {
                assert_eq!(v, 1);
                assert!(caps.iter().any(|c| c == "picture"));
                challenge
            }
            other => panic!("{other:?}"),
        }
    }
}

fn now() -> u64 {
    iyakki_agent::clock::now_unix() as u64
}

/// Hello, enrol with a fresh link, answer the challenge. Returns the open client, the device id and the key.
fn enrolled(r: &Rig, name: &str, seed: u8) -> (Client, String, DeviceKey) {
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([seed; 32]);
    let mut c = Client::open(r.srv.local_addr());
    c.hello();
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: name.into() });
    let (device, challenge) = match c.msg() {
        ServerMsg::Enrolled { device, challenge } => (device, challenge),
        other => panic!("{other:?}"),
    };
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &challenge, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { ref name, .. } if name == "Maya's laptop" || !name.is_empty()));
    (c, device, key)
}

/// A returning device: hello and a signed challenge.
fn returning(r: &Rig, device: &str, key: &DeviceKey) -> Client {
    let mut c = Client::open(r.srv.local_addr());
    let ch = c.hello();
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(key, device, &ch, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { .. }));
    c
}

fn sign_in(c: &mut Client) {
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.msg(), ServerMsg::SignedIn { person: "ann".into() });
}

// ------------------------------------------------------------------------------------------------------------------ the flow

#[test]
fn hello_enrol_signed_connect_sign_in_calls_picture_input_events_and_revoke() {
    let r = rig("flow", true, 50);
    let (mut c, device, key) = enrolled(&r, "Maya's laptop", 11);
    r.book.0.lock().unwrap().push(device.clone());
    let listed = link::list_devices(&r.data);
    assert_eq!((listed.len(), listed[0].name.as_str(), listed[0].by.as_str()), (1, "Maya's laptop", "ann"));
    assert_eq!(r.ep.open_sessions(), 1);

    // the device alone has no person: nothing person-level is served
    c.send(&ClientMsg::Call { id: 1, capability: "camera.view".into(), unit: None });
    assert_eq!(c.err(), ErrorCode::NotSignedIn);
    c.send(&ClientMsg::PictureOpen { deflate: false });
    assert_eq!(c.err(), ErrorCode::NotSignedIn);
    c.send(&ClientMsg::Input { msg: r#"{"t":"down","x":1,"y":2}"#.into() });
    assert_eq!(c.err(), ErrorCode::NotSignedIn);
    c.send(&ClientMsg::Resume { last_seq: 0 });
    assert_eq!(c.err(), ErrorCode::NotSignedIn);
    assert!(r.pic.lock().unwrap().inputs.is_empty() && !r.pic.lock().unwrap().viewing);

    sign_in(&mut c);

    // one call the broker allows, one it refuses, one the role refuses
    c.send(&ClientMsg::Call { id: 2, capability: "camera.view".into(), unit: Some("usb:1".into()) });
    assert_eq!(c.msg(), ServerMsg::CallOk { id: 2, detail: "camera.view:usb:1".into() });
    c.send(&ClientMsg::Call { id: 3, capability: "camera.record".into(), unit: Some("usb:1".into()) });
    assert!(matches!(c.msg(), ServerMsg::CallErr { id: 3, ref code, .. } if code == "not_granted"));
    c.send(&ClientMsg::Call { id: 4, capability: "secrets.read".into(), unit: None });
    assert!(matches!(c.msg(), ServerMsg::CallErr { id: 4, ref code, .. } if code == "role_denied"));
    c.send(&ClientMsg::Call { id: 5, capability: "no.such.thing".into(), unit: None });
    assert!(matches!(c.msg(), ServerMsg::CallErr { id: 5, ref code, .. } if code == "unknown_capability"));
    // the access log names the device and the person for allowed and refused alike
    let rows = r.log.rows();
    let allowed = rows.iter().find(|e| e.capability == "camera.view").unwrap();
    assert_eq!((allowed.app.as_str(), allowed.device.as_str(), allowed.person.as_str(), allowed.allowed), ("client", "Maya's laptop", "ann", true));
    let refused = rows.iter().find(|e| e.capability == "camera.record").unwrap();
    assert_eq!((refused.device.as_str(), refused.person.as_str(), refused.allowed, refused.reason.as_str()), ("Maya's laptop", "ann", false, "not_granted"));

    // one picture stream, in the existing tile codec, tagged as stream 1
    c.send(&ClientMsg::PictureOpen { deflate: false });
    assert_eq!(c.msg(), ServerMsg::PictureOk);
    let b = c.binary();
    let (stream, payload) = proto::split_binary(&b).unwrap();
    assert_eq!(stream, proto::STREAM_PICTURE);
    let mut canvas = Canvas::default();
    let h = decode_frame(payload, &mut canvas).unwrap();
    assert_eq!((canvas.w, canvas.h), (W as usize, H as usize));
    assert!(canvas.rgba.chunks(4).all(|p| p == [10, 20, 30, 255]));
    c.send(&ClientMsg::Ack { seq: h.seq });

    // input arrives at the person's screen; a client cannot switch on the screen's microphone
    c.send(&ClientMsg::Input { msg: r#"{"t":"down","x":5,"y":6}"#.into() });
    c.send(&ClientMsg::Input { msg: r#"{"t":"media","kind":"mic","on":true}"#.into() });
    assert_eq!(c.err(), ErrorCode::Unsupported);
    let t0 = Instant::now();
    while r.pic.lock().unwrap().inputs.is_empty() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = r.pic.lock().unwrap().inputs.clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].contains("MouseDown"), "{seen:?}");

    // events: only this person's, with sequence numbers, and resume after a gap in time
    let s1 = r.ep.publish("ann", "note", "one");
    let _other = r.ep.publish("bob", "note", "not for ann");
    let s3 = r.ep.publish("", "all", "two");
    assert_eq!(c.msg(), ServerMsg::Event { seq: s1, topic: "note".into(), body: "one".into() });
    assert_eq!(c.msg(), ServerMsg::Event { seq: s3, topic: "all".into(), body: "two".into() });
    c.send(&ClientMsg::Bye);
    assert!(c.closed_within(Duration::from_secs(3)));
    r.ep.publish("ann", "late", "while away");
    let mut c2 = returning(&r, &device, &key);
    sign_in(&mut c2);
    c2.send(&ClientMsg::Resume { last_seq: s3 });
    assert!(matches!(c2.msg(), ServerMsg::ResumeOk { count: 1, .. }));
    assert!(matches!(c2.msg(), ServerMsg::Event { ref topic, .. } if topic == "late"));
    c2.send(&ClientMsg::Resume { last_seq: 9999 });
    assert!(matches!(c2.msg(), ServerMsg::ResumeGap { .. }), "a client ahead of the instance starts again");

    // a revoke closes the open session and the device cannot connect again
    assert_eq!(r.ep.revoke(&device, "ann"), Ok(true));
    let mut bye = false;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        match c2.next(Duration::from_millis(200)) {
            Some(Got::Msg(ServerMsg::Bye { reason })) => {
                assert_eq!(reason, "revoked");
                bye = true;
            }
            Some(Got::Close) => break,
            _ => {}
        }
    }
    assert!(bye, "told why");
    let mut c3 = Client::open(r.srv.local_addr());
    let ch = c3.hello();
    c3.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &ch, now()).unwrap() });
    assert_eq!(c3.err(), ErrorCode::DeviceRevoked);

    // the audit log has the facts and none of the secrets
    let audit = r.audit.lock().unwrap().join("\n");
    for want in ["client.enrol", "client.signin", "client.revoke"] {
        assert!(audit.contains(want), "{audit}");
    }
    assert!(!audit.contains("right") && !audit.contains(&key.seed_hex().unwrap()), "{audit}");
    let t0 = Instant::now();
    while r.ep.open_sessions() > 0 && t0.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(r.ep.open_sessions(), 0);
    assert!(!r.pic.lock().unwrap().viewing, "the screen is no longer watched");
}

#[test]
fn a_revoke_made_in_settings_through_the_file_closes_an_open_session_within_two_seconds() {
    let r = rig("filerevoke", true, 50);
    let (mut c, device, _) = enrolled(&r, "Phone", 12);
    assert_eq!(link::revoke_device(&r.data, &device, now() as i64), Ok(true));
    let mut got_bye = false;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(4) {
        match c.next(Duration::from_millis(200)) {
            Some(Got::Msg(ServerMsg::Bye { .. })) => got_bye = true,
            Some(Got::Close) => break,
            _ => {}
        }
    }
    assert!(got_bye && t0.elapsed() < Duration::from_secs(3));
}

#[test]
fn switching_the_setting_off_closes_open_sessions() {
    let r = rig("switchoff", true, 50);
    let (mut c, _, _) = enrolled(&r, "Phone", 13);
    link::set_enabled(&r.data, false).unwrap();
    let mut bye = None;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(4) {
        match c.next(Duration::from_millis(200)) {
            Some(Got::Msg(ServerMsg::Bye { reason })) => bye = Some(reason),
            Some(Got::Close) => break,
            _ => {}
        }
    }
    assert_eq!(bye.as_deref(), Some("disabled"));
}

// ------------------------------------------------------------------------------------------------------------------ refusals, every route

#[test]
fn every_route_is_a_plain_404_while_the_setting_is_off() {
    let r = rig("off", false, 50);
    let a = r.srv.local_addr();
    for (m, p) in [("GET", "/connect"), ("POST", "/connect"), ("GET", "/client/ws"), ("GET", "/client/other")] {
        let (code, _) = raw_http(a, &format!("{m} {p} HTTP/1.1\r\nHost: {a}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        assert_eq!(code, 404, "{m} {p}");
    }
    let (code, text) = raw_http(a, &upgrade_request(a, ""));
    assert_eq!(code, 404, "no upgrade while off: {text}");
    // and with the setting on, the same upgrade works
    link::set_enabled(&r.data, true).unwrap();
    let (code, _) = raw_http(a, &upgrade_request(a, "Connection: close\r\n"));
    assert!(code == 101 || code == 0, "{code}");
}

#[test]
fn the_http_routes_refuse_what_they_should() {
    let r = rig("http", true, 50);
    let a = r.srv.local_addr();
    let (code, text) = raw_http(a, &format!("GET /connect HTTP/1.1\r\nHost: {a}\r\nConnection: close\r\n\r\n"));
    assert_eq!(code, 200);
    assert!(text.contains("Referrer-Policy: no-referrer") && text.contains("no-store"));
    for m in ["POST", "PUT", "DELETE"] {
        let (code, _) = raw_http(a, &format!("{m} /connect HTTP/1.1\r\nHost: {a}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        assert!(code == 404 || code == 405, "{m} /connect is {code}");
    }
    // a plain GET of the socket route without an upgrade is not served
    let (code, _) = raw_http(a, &format!("GET /client/ws HTTP/1.1\r\nHost: {a}\r\nConnection: close\r\n\r\n"));
    assert_eq!(code, 404);
    // a page of another origin cannot open the socket; a POST upgrade is refused too
    let (code, _) = raw_http(a, &upgrade_request(a, "Origin: https://evil.example\r\n"));
    assert_eq!(code, 403);
    let (code, _) = raw_http(a, &upgrade_request(a, "").replacen("GET", "POST", 1));
    assert_eq!(code, 403);
    // no key: no upgrade
    let (code, _) = raw_http(a, &format!("GET /client/ws HTTP/1.1\r\nHost: {a}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"));
    assert!(code == 400 || code == 404, "{code}");
    // the page's own cookie door does not open the client socket and vice versa: no cookie is looked at
    let (code, _) = raw_http(a, &upgrade_request(a, "Cookie: iyakki_session=00\r\n").replace("/client/ws", "/ws?csrf=00"));
    assert_eq!(code, 401);
}

#[test]
fn nothing_works_before_hello_and_hello_is_checked() {
    let r = rig("hello", true, 50);
    let a = r.srv.local_addr();
    for m in [ClientMsg::Auth { authorization: "x".into() }, ClientMsg::SignIn { user: "ann".into(), password: "right".into() }, ClientMsg::Call { id: 1, capability: "camera.view".into(), unit: None }, ClientMsg::PictureOpen { deflate: false }] {
        let mut c = Client::open(a);
        c.send(&m);
        assert_eq!(c.err(), ErrorCode::BadSequence, "{m:?}");
        assert!(c.closed_within(Duration::from_secs(3)));
    }
    // a version nobody speaks, and a capability the instance does not have
    let mut c = Client::open(a);
    c.send(&ClientMsg::Hello { v: 9, min_v: 7, offered: vec![], needed: vec![], client: String::new() });
    assert_eq!(c.err(), ErrorCode::VersionMismatch);
    let mut c = Client::open(a);
    c.send(&ClientMsg::Hello { v: 1, min_v: 1, offered: vec![], needed: vec!["hologram".into()], client: String::new() });
    assert_eq!(c.err(), ErrorCode::CapabilityMissing);
    // junk and a second hello
    let mut c = Client::open(a);
    c.send_raw("{\"t\":\"hello\"");
    assert_eq!(c.err(), ErrorCode::BadMessage);
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Hello { v: 1, min_v: 1, offered: vec![], needed: vec![], client: String::new() });
    assert_eq!(c.err(), ErrorCode::BadSequence);
}

#[test]
fn a_link_works_once_and_unknown_tokens_and_bad_keys_are_refused() {
    let r = rig("tokens", true, 50);
    let a = r.srv.local_addr();
    let key = DeviceKey::from_seed([21; 32]);
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Enrol { token: token.clone(), public: key.public_hex(), name: "One".into() });
    assert!(matches!(c.msg(), ServerMsg::Enrolled { .. }));
    // the same link again
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Enrol { token, public: DeviceKey::from_seed([22; 32]).public_hex(), name: "Two".into() });
    assert_eq!(c.err(), ErrorCode::TokenUsed);
    // a token nobody made
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Enrol { token: "f".repeat(64), public: key.public_hex(), name: "x".into() });
    assert_eq!(c.err(), ErrorCode::TokenUnknown);
    // a good link, but not a key we take, or a name that is not a name
    for (public, name) in [("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ x".to_string(), "x".to_string()), (DeviceKey::from_seed([23; 32]).public_hex(), String::new()), (DeviceKey::from_seed([23; 32]).public_hex(), "bad\u{7}name".to_string())] {
        let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
        let mut c = Client::open(a);
        c.hello();
        c.send(&ClientMsg::Enrol { token, public, name });
        assert_eq!(c.err(), ErrorCode::BadEnrolment);
    }
    assert_eq!(link::list_devices(&r.data).len(), 1);
    // a token cannot be sent with the audit log as witness
    let audit = r.audit.lock().unwrap().join("\n");
    assert!(audit.contains("client.enrol.refused") && !audit.contains(&"f".repeat(64)));
}

#[test]
fn a_bad_signature_a_stranger_and_a_replayed_challenge_are_refused() {
    let r = rig("sig", true, 50);
    let a = r.srv.local_addr();
    let (_c, device, key) = enrolled(&r, "Phone", 31);
    // signed by another key
    let mut c = Client::open(a);
    let ch = c.hello();
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&DeviceKey::from_seed([32; 32]), &device, &ch, now()).unwrap() });
    assert_eq!(c.err(), ErrorCode::BadSignature);
    // a device that was never enrolled
    let mut c = Client::open(a);
    let ch = c.hello();
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, "dnotenrolled", &ch, now()).unwrap() });
    assert_eq!(c.err(), ErrorCode::DeviceUnknown);
    // an answer from another connection (a captured one) fails on this connection's challenge
    let mut c1 = Client::open(a);
    let ch1 = c1.hello();
    let captured = link::sign_answer(&key, &device, &ch1, now()).unwrap();
    let mut c2 = Client::open(a);
    c2.hello();
    c2.send(&ClientMsg::Auth { authorization: captured.clone() });
    assert_eq!(c2.err(), ErrorCode::BadSignature);
    // an old signature (outside the time window)
    let mut c = Client::open(a);
    let ch = c.hello();
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &ch, now() - 3600).unwrap() });
    assert_eq!(c.err(), ErrorCode::BadSignature);
    // the real one works once; the same answer again on the same connection is a replay
    c1.send(&ClientMsg::Auth { authorization: captured.clone() });
    assert!(matches!(c1.msg(), ServerMsg::Authed { .. }));
    c1.send(&ClientMsg::Auth { authorization: captured });
    assert_eq!(c1.err(), ErrorCode::Replayed);
    // junk instead of a header
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Auth { authorization: "Bearer abc".into() });
    assert_eq!(c.err(), ErrorCode::BadMessage);
}

#[test]
fn a_device_key_alone_never_gives_a_person_and_a_wrong_password_is_refused() {
    let r = rig("person", true, 50);
    let a = r.srv.local_addr();
    // sign-in before the device is proven
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.err(), ErrorCode::NotAuthenticated);
    // a proven device with a wrong password, then unknown account (one answer), then the right one
    let (mut c, _, _) = enrolled(&r, "Phone", 41);
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "wrong".into() });
    assert_eq!(c.err(), ErrorCode::SignInRefused);
    c.send(&ClientMsg::SignIn { user: "nobody".into(), password: "right".into() });
    assert_eq!(c.err(), ErrorCode::SignInRefused);
    c.send(&ClientMsg::Call { id: 1, capability: "camera.view".into(), unit: None });
    assert_eq!(c.err(), ErrorCode::NotSignedIn);
    sign_in(&mut c);
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.err(), ErrorCode::BadSequence);
    // the password never reaches the audit log or the access log
    let audit = r.audit.lock().unwrap().join("\n");
    assert!(!audit.contains("wrong") && !audit.contains("right"));
}

#[test]
fn failed_tokens_signatures_and_passwords_are_rate_limited_per_address() {
    let r = rig("rate", true, 4);
    let a = r.srv.local_addr();
    let key = DeviceKey::from_seed([51; 32]);
    for _ in 0..4 {
        let mut c = Client::open(a);
        c.hello();
        c.send(&ClientMsg::Enrol { token: "a".repeat(64), public: key.public_hex(), name: "x".into() });
        assert_eq!(c.err(), ErrorCode::TokenUnknown);
    }
    // the fifth try is not even looked at, and a valid link is not spent by it
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let mut c = Client::open(a);
    c.hello();
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: "x".into() });
    assert_eq!(c.err(), ErrorCode::RateLimited);
    assert_eq!(link::open_tokens(&r.data, now() as i64), 1, "the link was not spent");
    // failed signatures count from the same address too
    let mut c = Client::open(a);
    let ch = c.hello();
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, "dnone", &ch, now()).unwrap() });
    assert_eq!(c.err(), ErrorCode::RateLimited);
}

#[test]
fn wrong_passwords_are_limited_and_end_the_connection() {
    let r = rig("pwlimit", true, 50);
    let (mut c, _, _) = enrolled(&r, "Phone", 61);
    for _ in 0..4 {
        c.send(&ClientMsg::SignIn { user: "ann".into(), password: "nope".into() });
        assert_eq!(c.err(), ErrorCode::SignInRefused);
    }
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "nope".into() });
    assert_eq!(c.err(), ErrorCode::SignInRefused);
    assert!(c.closed_within(Duration::from_secs(3)), "the fifth wrong password ends the connection");
}

#[test]
fn a_client_cannot_use_the_screens_microphone_or_send_binary() {
    let r = rig("input", true, 50);
    let (mut c, _, _) = enrolled(&r, "Phone", 71);
    sign_in(&mut c);
    c.send(&ClientMsg::Input { msg: r#"{"t":"media","kind":"mic","on":true}"#.into() });
    let code = c.err();
    assert!(matches!(code, ErrorCode::Unsupported | ErrorCode::BadMessage), "{code:?}");
    c.send(&ClientMsg::Input { msg: "not json".into() });
    assert_eq!(c.err(), ErrorCode::BadMessage);
    c.s.write_all(&masked(2, &[1, 2, 3])).unwrap();
    assert_eq!(c.err(), ErrorCode::Unsupported);
    assert!(r.pic.lock().unwrap().inputs.is_empty());
}

#[test]
fn only_a_few_clients_can_be_open_at_once_and_a_long_message_is_refused() {
    let r = rig("many", true, 500);
    let mut keep = Vec::new();
    for i in 0..iyakki_remote::clientsession::MAX_SESSIONS as u8 {
        keep.push(enrolled(&r, &format!("Dev {i}"), 80 + i));
    }
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([120; 32]);
    let mut c = Client::open(r.srv.local_addr());
    let _ = c.hello();
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: "Late".into() });
    let (device, ch) = match c.msg() {
        ServerMsg::Enrolled { device, challenge } => (device, challenge),
        other => panic!("{other:?}"),
    };
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &ch, now()).unwrap() });
    assert_eq!(c.err(), ErrorCode::RateLimited, "too many clients are connected");
    let mut c = Client::open(r.srv.local_addr());
    c.send_raw(&format!(r#"{{"t":"ping","ts":1,"pad":"{}"}}"#, "a".repeat(proto::MAX_TEXT)));
    assert_eq!(c.err(), ErrorCode::TooBig);
}
