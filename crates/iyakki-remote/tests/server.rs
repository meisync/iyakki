use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Screen};
use iyakki_remote::server::{Config, Server};
use iyakki_remote::ws;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Log {
    inputs: Vec<String>,
    viewing: Vec<bool>,
    /// Messages the fake screen wants the page's speech synthesis to get.
    speech: Vec<String>,
}

struct Fake {
    log: Arc<Mutex<Log>>,
    sent: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.log.lock().unwrap().inputs.push(format!("{i:?}"));
    }
    fn viewing(&mut self, on: bool) {
        self.log.lock().unwrap().viewing.push(on);
        self.sent = false;
    }
    fn step(&mut self) -> Option<Frame> {
        if !self.sent {
            self.sent = true;
            return Some(Frame { x: 1, y: 2, w: 3, h: 4, png: vec![9, 9, 9] });
        }
        std::thread::sleep(Duration::from_millis(5));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn take_speech(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log.lock().unwrap().speech)
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    log: Arc<Mutex<Log>>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { log: self.log.clone(), sent: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

fn start() -> (Server, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let s = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { log: log.clone() })).unwrap();
    (s, log)
}

fn http(addr: std::net::SocketAddr, raw: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let code = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    (code, head, body)
}

fn post_login(addr: std::net::SocketAddr, user: &str, pass: &str, extra: &str) -> (u16, String, String) {
    let body = format!(r#"{{"user":"{user}","password":"{pass}"}}"#);
    http(addr, &format!("POST /api/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len()))
}

fn signed_in(addr: std::net::SocketAddr) -> (String, String) {
    let (code, head, body) = post_login(addr, "ann", "right", "");
    assert_eq!(code, 200, "{body}");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).expect("cookie").split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(&body).unwrap()["csrf"].as_str().unwrap().to_string();
    (cookie, csrf)
}

fn masked_text(s: &str) -> Vec<u8> {
    let mask = [1u8, 2, 3, 4];
    let p = s.as_bytes();
    let mut f = vec![0x81];
    if p.len() < 126 {
        f.push(0x80 | p.len() as u8);
    } else {
        f.push(0x80 | 126);
        f.extend_from_slice(&(p.len() as u16).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

fn open_ws(addr: std::net::SocketAddr, cookie: &str, csrf: &str) -> Result<TcpStream, (u16, String)> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nCookie: {cookie}\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).unwrap_or(0) == 0 {
            break;
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let code: u16 = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    if code == 101 {
        assert!(text.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "accept key: {text}");
        Ok(s)
    } else {
        Err((code, text))
    }
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
        n => n as usize,
    };
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).unwrap();
    (h[0] & 0x0f, p)
}

#[test]
fn the_page_and_the_health_check_are_served_with_safety_headers() {
    let (srv, _) = start();
    let (code, head, body) = http(srv.local_addr(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(code, 200);
    assert!(body.contains("<canvas") && body.contains("Sign in"));
    assert!(head.contains("X-Frame-Options: DENY") && head.contains("Content-Security-Policy"));
    assert_eq!(http(srv.local_addr(), "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").0, 200);
    assert_eq!(http(srv.local_addr(), "GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n").0, 404);
    assert_eq!(http(srv.local_addr(), "DELETE / HTTP/1.1\r\nHost: x\r\n\r\n").0, 405);
    assert_eq!(http(srv.local_addr(), "GET /api/session HTTP/1.1\r\nHost: x\r\n\r\n").0, 401);
    assert_eq!(http(srv.local_addr(), "garbage\r\n\r\n").0, 400);
}

#[test]
fn signing_in_gives_a_private_cookie_and_wrong_passwords_are_refused() {
    let (srv, _) = start();
    let a = srv.local_addr();
    assert_eq!(post_login(a, "ann", "wrong", "").0, 401);
    let (code, head, body) = post_login(a, "ann", "right", "");
    assert_eq!(code, 200, "{body}");
    let cookie_line = head.lines().find(|l| l.starts_with("Set-Cookie:")).expect("cookie");
    assert!(cookie_line.contains("HttpOnly") && cookie_line.contains("SameSite=Strict"));
    assert!(!cookie_line.contains("Secure"), "plain HTTP cannot carry a Secure cookie");
    let (cookie, csrf) = signed_in(a);
    let (code, _, body) = http(a, &format!("GET /api/session HTTP/1.1\r\nHost: {a}\r\nCookie: {cookie}\r\n\r\n"));
    assert_eq!(code, 200);
    assert!(body.contains(&csrf) && body.contains("ann"));
    assert_eq!(http(a, &format!("GET /api/session HTTP/1.1\r\nHost: {a}\r\nCookie: iyakki_session=00\r\n\r\n")).0, 401);
}

#[test]
fn a_sign_in_from_another_site_or_as_a_plain_form_is_refused() {
    let (srv, _) = start();
    let a = srv.local_addr();
    assert_eq!(post_login(a, "ann", "right", "Origin: http://evil.example\r\n").0, 403);
    let body = "user=ann&password=right";
    let (code, _, _) = http(a, &format!("POST /api/login HTTP/1.1\r\nHost: {a}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert_eq!(code, 403);
}

#[test]
fn too_many_sign_in_tries_from_one_address_are_stopped() {
    let (srv, _) = start();
    let a = srv.local_addr();
    let mut last = 0;
    for _ in 0..12 {
        last = post_login(a, "ann", "wrong", "").0;
    }
    assert_eq!(last, 429);
    assert_eq!(post_login(a, "ann", "right", "").0, 429, "even the right password waits");
}

#[test]
fn the_screen_is_streamed_over_a_websocket_and_input_comes_back() {
    let (srv, log) = start();
    let a = srv.local_addr();
    let (cookie, csrf) = signed_in(a);
    let mut s = open_ws(a, &cookie, &csrf).expect("upgraded");
    let (op, payload) = read_frame(&mut s);
    assert_eq!(op, 2, "a binary frame");
    assert_eq!(payload, vec![1, 0, 2, 0, 3, 0, 4, 0, 9, 9, 9]);
    s.write_all(&masked_text(r#"{"t":"key","key":"a"}"#)).unwrap();
    s.write_all(&masked_text(r#"{"t":"move","x":4,"y":5}"#)).unwrap();
    s.write_all(&masked_text(r#"{"t":"reboot"}"#)).unwrap(); // not understood, ignored
    let t0 = Instant::now();
    loop {
        let l = log.lock().unwrap();
        if l.inputs.iter().any(|i| i.contains("MouseMove(4, 5)")) {
            assert!(l.inputs.iter().any(|i| i.contains("Char('a')")));
            assert!(!l.inputs.iter().any(|i| i.contains("reboot")));
            break;
        }
        drop(l);
        assert!(t0.elapsed() < Duration::from_secs(5), "input never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    s.write_all(&[0x88, 0x80, 0, 0, 0, 0]).unwrap(); // close
    let t0 = Instant::now();
    while log.lock().unwrap().viewing.last() != Some(&false) {
        assert!(t0.elapsed() < Duration::from_secs(5), "the screen was not told nobody is looking");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(log.lock().unwrap().viewing, vec![true, false]);
}

#[test]
fn a_websocket_needs_the_cookie_the_csrf_token_and_the_same_origin() {
    let (srv, _) = start();
    let a = srv.local_addr();
    let (cookie, csrf) = signed_in(a);
    assert_eq!(open_ws(a, "iyakki_session=00", &csrf).err().unwrap().0, 401);
    assert_eq!(open_ws(a, &cookie, "wrong").err().unwrap().0, 403);
    // another site's page cannot drive the session even with the cookie
    let mut s = TcpStream::connect(a).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {a}\r\nOrigin: http://evil.example\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: x\r\nCookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
}

#[test]
fn a_session_is_watched_from_one_window_at_a_time() {
    let (srv, _) = start();
    let a = srv.local_addr();
    let (cookie, csrf) = signed_in(a);
    let mut first = open_ws(a, &cookie, &csrf).expect("first");
    let _ = read_frame(&mut first);
    assert_eq!(open_ws(a, &cookie, &csrf).err().unwrap().0, 409);
    drop(first);
}

#[test]
fn signing_out_needs_the_csrf_token_and_ends_the_session() {
    let (srv, _) = start();
    let a = srv.local_addr();
    let (cookie, csrf) = signed_in(a);
    let out = |extra: &str| http(a, &format!("POST /api/logout HTTP/1.1\r\nHost: {a}\r\nCookie: {cookie}\r\n{extra}Content-Length: 0\r\n\r\n")).0;
    assert_eq!(out(""), 403);
    assert_eq!(out("X-CSRF: wrong\r\n"), 403);
    assert_eq!(srv.sessions(), 1);
    assert_eq!(out(&format!("X-CSRF: {csrf}\r\n")), 200);
    assert_eq!(srv.sessions(), 0);
    assert_eq!(http(a, &format!("GET /api/session HTTP/1.1\r\nHost: {a}\r\nCookie: {cookie}\r\n\r\n")).0, 401);
}

#[test]
fn oversized_and_slow_requests_do_not_hold_the_server() {
    let (srv, _) = start();
    let a = srv.local_addr();
    let big = "x".repeat(100_000);
    assert_eq!(http(a, &format!("GET /{big} HTTP/1.1\r\nHost: x\r\n\r\n")).0, 431);
    assert_eq!(http(a, "POST /api/login HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n").0, 413);
    // the server still answers
    assert_eq!(http(a, "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").0, 200);
    let _ = ws::sha1(b"");
}

#[test]
fn https_works_with_a_made_certificate_and_the_cookie_is_marked_secure() {
    use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
    let dir = std::env::temp_dir().join(format!("iyakki-remote-tls-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = iyakki_remote::tls::identity(&dir, &["localhost".into()]).unwrap();
    let log = Arc::new(Mutex::new(Log::default()));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: Some(id.config.clone()) }, Arc::new(FakeGate { log })).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(&std::fs::read(dir.join("cert.pem")).unwrap()) {
        roots.add(c.unwrap()).unwrap();
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
    let tls_get = |req: &str| {
        let conn = rustls::ClientConnection::new(Arc::new(cfg.clone()), ServerName::try_from("localhost").unwrap()).unwrap();
        let tcp = TcpStream::connect(srv.local_addr()).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut t = rustls::StreamOwned::new(conn, tcp);
        t.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = t.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    };
    let r = tls_get("GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("ok"), "{r}");
    let body = r#"{"user":"ann","password":"right"}"#;
    let r = tls_get(&format!("POST /api/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert!(r.contains("Set-Cookie:") && r.contains("; Secure"), "{r}");
    // plain HTTP on the TLS port is not answered with data
    let (code, _, _) = http(srv.local_addr(), "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_ne!(code, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn speech_messages_reach_the_page_as_text_frames_and_the_pages_events_come_back_as_input() {
    use iyakki_agent::hostvoice::{FromClient, ToClient};
    let (srv, log) = start();
    let a = srv.local_addr();
    let (cookie, csrf) = signed_in(a);
    let mut s = open_ws(a, &cookie, &csrf).expect("upgraded");
    let (op, _) = read_frame(&mut s);
    assert_eq!(op, 2);
    let speak = ToClient::Speak { id: 5, text: "Once upon a time.".into(), voice: Some("Samantha".into()), lang: Some("en-US".into()), rate: 1.0 };
    log.lock().unwrap().speech.push(speak.encode());
    let (op, payload) = read_frame(&mut s);
    assert_eq!(op, 1, "a text frame");
    assert_eq!(ToClient::decode(std::str::from_utf8(&payload).unwrap()), Some(speak));
    for e in [FromClient::Voices { supported: true, voices: vec![] }, FromClient::Started { id: 5 }, FromClient::Boundary { id: 5, char_index: 4 }, FromClient::Ended { id: 5 }] {
        s.write_all(&masked_text(&e.encode())).unwrap();
    }
    let t0 = Instant::now();
    loop {
        let l = log.lock().unwrap();
        if l.inputs.iter().any(|i| i.contains("Ended { id: 5 }")) {
            let speech: Vec<_> = l.inputs.iter().filter(|i| i.starts_with("Speech")).collect();
            assert_eq!(speech.len(), 4, "{speech:?}");
            break;
        }
        drop(l);
        assert!(t0.elapsed() < Duration::from_secs(5), "the page's speech events never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_page_has_the_speech_code_and_only_reaches_for_it_when_asked() {
    let (srv, _) = start();
    let (_, _, body) = http(srv.local_addr(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    for needed in ["speechSynthesis", "SpeechSynthesisUtterance", "voiceschanged", "onboundary", "sp_voices", "sp_start", "sp_end", "sp_error", "speak_cancel", "speak_pause", "speak_resume", "not-allowed"] {
        assert!(body.contains(needed), "the page lacks {needed}");
    }
    assert!(!body.contains("SpeechRecognition"), "dictation is not part of this page yet");
}
