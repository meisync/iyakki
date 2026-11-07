//! Device-key sign-in against the real web server (`POST /api/login-key`, docs/development/CONNECTOR-APP-PLAN.md 3a): a good signature,
//! the wrong key, a replayed signature, an expired one, a revoked key; enrolling and revoking over the signed-in session; and the
//! connector's raw link (`iyakki_link::rawlink`) signing in with a key over real TLS and receiving the tile stream untouched.
//! The screen behind the server is a fake that sends a picture; the account rules are the gate's, here a fake with one account.

use iyakki_link::cloudclient::Pin;
use iyakki_link::rawlink::{Auth, Frame, LinkError, RawWs};
use iyakki_link::sigscheme::{self, DeviceKey};
use iyakki_remote::devicekeys::{DeviceKeys, LOGIN_PATH, WINDOW_SECS};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fake {
    armed: bool,
    sent: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "alice".into()
    }
    fn input(&mut self, _: Input) {}
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<iyakki_remote::screen::Frame> {
        std::thread::sleep(Duration::from_millis(3));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn set_stream(&mut self, _: bool) {}
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        if self.armed && !self.sent {
            self.sent = true;
            return Some(Pixels { w: 640, h: 400, rgba: (0..640 * 400).flat_map(|_| [10u8, 20, 30, 255]).collect() });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    keys: Arc<DeviceKeys>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "alice" && password == "right" {
            Ok(Box::new(Fake { armed: false, sent: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn device_keys(&self, user: &str) -> Vec<[u8; 32]> {
        self.keys.keys_of(user)
    }
    fn login_key(&self, user: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "alice" {
            Ok(Box::new(Fake { armed: false, sent: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn enroll_device_key(&self, user: &str, key: &str) -> Result<String, String> {
        self.keys.enroll(user, key)
    }
    fn revoke_device_key(&self, user: &str, fp: &str) -> Result<bool, String> {
        self.keys.revoke(user, fp)
    }
    fn list_device_keys(&self, user: &str) -> Vec<String> {
        self.keys.fingerprints(user)
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn plain() -> (Server, Arc<DeviceKeys>) {
    let keys = Arc::new(DeviceKeys::in_memory());
    let s = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { keys: keys.clone() })).unwrap();
    (s, keys)
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

fn key_login(addr: std::net::SocketAddr, header: &str) -> (u16, String, String) {
    http(addr, &format!("POST {LOGIN_PATH} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {header}\r\nContent-Length: 0\r\n\r\n"))
}

fn cookie_and_csrf(head: &str, body: &str) -> (String, String) {
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).expect("a session cookie").split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(body).unwrap()["csrf"].as_str().unwrap().to_string();
    (cookie, csrf)
}

fn with_cookie(addr: std::net::SocketAddr, method: &str, path: &str, cookie: &str, csrf: Option<&str>, body: &str) -> (u16, String, String) {
    let x = csrf.map(|c| format!("X-Csrf: {c}\r\n")).unwrap_or_default();
    http(addr, &format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nCookie: {cookie}\r\n{x}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
}

#[test]
fn a_good_signature_opens_a_session_like_a_password_does() {
    let (s, keys) = plain();
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.ssh_public_line("laptop")).unwrap();
    let a = s.local_addr();
    let (code, head, body) = key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap());
    assert_eq!(code, 200, "{body}");
    assert!(head.contains("HttpOnly") && head.contains("SameSite=Strict"), "{head}");
    let (cookie, csrf) = cookie_and_csrf(&head, &body);
    assert_eq!(csrf.len(), 64, "an opaque token, not a JWT");
    assert!(!body.contains("eyJ") && !head.contains("eyJ"));
    let (code, _, b) = with_cookie(a, "GET", "/api/session", &cookie, None, "");
    assert_eq!(code, 200, "{b}");
    assert!(b.contains("alice") && b.contains(&csrf));
}

#[test]
fn the_wrong_key_an_unknown_account_and_no_header_get_the_same_refusal() {
    let (s, keys) = plain();
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.public_hex()).unwrap();
    let a = s.local_addr();
    let stranger = DeviceKey::from_seed([8; 32]);
    let wrong = key_login(a, &stranger.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap());
    let unknown = key_login(a, &k.sign_request("nobody", "POST", LOGIN_PATH, now(), b"").unwrap());
    let none = http(a, &format!("POST {LOGIN_PATH} HTTP/1.1\r\nHost: {a}\r\nContent-Length: 0\r\n\r\n"));
    let junk = key_login(a, "IYAKKI-SIG id=alice,ts=1,sig=00");
    for r in [&wrong, &unknown, &none, &junk] {
        assert_eq!(r.0, 401, "{r:?}");
        assert_eq!(r.2, wrong.2, "the same body: nothing says which part was wrong");
        assert!(!r.1.contains("Set-Cookie"));
    }
}

#[test]
fn a_replayed_signature_is_refused() {
    let (s, keys) = plain();
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.public_hex()).unwrap();
    let a = s.local_addr();
    let h = k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap();
    assert_eq!(key_login(a, &h).0, 200);
    assert_eq!(key_login(a, &h).0, 401, "the exact same request, a second time");
    assert_eq!(key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap()).0, 200, "a new nonce is a new request");
}

#[test]
fn an_expired_or_future_signature_is_refused() {
    let (s, keys) = plain();
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.public_hex()).unwrap();
    let a = s.local_addr();
    assert_eq!(key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now() - WINDOW_SECS - 5, b"").unwrap()).0, 401, "expired");
    assert_eq!(key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now() + WINDOW_SECS + 5, b"").unwrap()).0, 401, "from the future");
    assert_eq!(key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap()).0, 200);
}

#[test]
fn a_signature_for_another_route_or_method_does_not_sign_in() {
    let (s, keys) = plain();
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.public_hex()).unwrap();
    let a = s.local_addr();
    for (m, p) in [("GET", LOGIN_PATH), ("POST", "/api/login"), ("POST", "/v1/enroll")] {
        assert_eq!(key_login(a, &k.sign_request("alice", m, p, now(), b"").unwrap()).0, 401, "{m} {p}");
    }
}

#[test]
fn enrolling_over_a_password_session_then_revoking_cuts_the_device_off_at_once() {
    let (s, _keys) = plain();
    let a = s.local_addr();
    let k = DeviceKey::from_seed([7; 32]);
    // sign in with the password and enrol the device's public key (only with the CSRF value, only for one's own account)
    let body = r#"{"user":"alice","password":"right"}"#;
    let (code, head, resp) = http(a, &format!("POST /api/login HTTP/1.1\r\nHost: {a}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert_eq!(code, 200, "{resp}");
    let (pw_cookie, pw_csrf) = cookie_and_csrf(&head, &resp);
    let enrol = format!(r#"{{"key":"{}"}}"#, k.ssh_public_line("laptop"));
    assert_eq!(with_cookie(a, "POST", "/api/device-keys", &pw_cookie, None, &enrol).0, 403, "no CSRF value");
    assert_eq!(with_cookie(a, "POST", "/api/device-keys", "iyakki_session=nope", Some(&pw_csrf), &enrol).0, 401, "no session");
    let rsa = r#"{"key":"ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ x"}"#;
    let (code, _, b) = with_cookie(a, "POST", "/api/device-keys", &pw_cookie, Some(&pw_csrf), rsa);
    assert_eq!(code, 400, "{b}");
    assert!(b.contains("RSA"));
    let (code, _, b) = with_cookie(a, "POST", "/api/device-keys", &pw_cookie, Some(&pw_csrf), &enrol);
    assert_eq!(code, 200, "{b}");
    let fp = serde_json::from_str::<serde_json::Value>(&b).unwrap()["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fp, sigscheme::fingerprint(&k.public()));
    assert!(with_cookie(a, "GET", "/api/device-keys", &pw_cookie, None, "").2.contains(&fp));
    // the device signs in
    let (code, head, resp) = key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap());
    assert_eq!(code, 200, "{resp}");
    let (dev_cookie, _) = cookie_and_csrf(&head, &resp);
    assert_eq!(with_cookie(a, "GET", "/api/session", &dev_cookie, None, "").0, 200);
    // revoke it from the password session: the key's session ends at once, the password session stays, the key signs nothing more
    let (code, _, b) = with_cookie(a, "POST", "/api/device-keys/revoke", &pw_cookie, Some(&pw_csrf), &format!(r#"{{"fingerprint":"{fp}"}}"#));
    assert_eq!(code, 200, "{b}");
    assert!(b.contains(r#""sessions_ended":1"#), "{b}");
    assert_eq!(with_cookie(a, "GET", "/api/session", &dev_cookie, None, "").0, 401, "the device's session is gone");
    assert_eq!(with_cookie(a, "GET", "/api/session", &pw_cookie, None, "").0, 200, "the person's own session is untouched");
    assert_eq!(key_login(a, &k.sign_request("alice", "POST", LOGIN_PATH, now(), b"").unwrap()).0, 401, "a revoked key");
    assert_eq!(with_cookie(a, "POST", "/api/device-keys/revoke", &pw_cookie, Some(&pw_csrf), &format!(r#"{{"fingerprint":"{fp}"}}"#)).0, 404);
}

#[test]
fn the_header_is_byte_compatible_with_the_vaults_scheme() {
    use iyakki_agent::cloudkeys::{self, MemberKeys, PublicKeys};
    // signed by the vault's code, verified by this one
    let m = MemberKeys::from_seed([5; 32]);
    let public = <[u8; 32]>::try_from(sigscheme::unhex(&m.public().sign).unwrap()).unwrap();
    let h = m.sign_request_with_nonce("acme/ann", "POST", LOGIN_PATH, 1234, b"body", &"cd".repeat(16));
    assert!(sigscheme::parse_authorization(&h).unwrap().verify(&public, "POST", LOGIN_PATH, b"body"));
    // signed by this one, verified by the vault's code
    let k = DeviceKey::from_seed([6; 32]);
    let h = k.sign_request_with_nonce("acme/ann", "POST", LOGIN_PATH, 1234, b"body", &"cd".repeat(16));
    let keys = PublicKeys { wrap: "11".repeat(32), sign: k.public_hex() };
    assert!(cloudkeys::parse_authorization(&h).unwrap().verify(&keys, "POST", LOGIN_PATH, b"body"));
    assert!(!cloudkeys::parse_authorization(&h).unwrap().verify(&keys, "POST", LOGIN_PATH, b"other"));
}

fn tls_rig(tag: &str) -> (Server, Arc<DeviceKeys>, Pin, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-devkey-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = iyakki_remote::tls::identity(&dir, &["acme.example".into()]).expect("a certificate");
    let pin = Pin::parse(&id.fingerprint).expect("a pin");
    let keys = Arc::new(DeviceKeys::in_memory());
    let s = Server::start(Config { addr: "127.0.0.1:0".into(), tls: Some(id.config) }, Arc::new(FakeGate { keys: keys.clone() })).unwrap();
    (s, keys, pin, dir)
}

#[test]
fn the_connectors_raw_link_signs_in_with_a_key_over_tls_and_gets_the_tile_stream_untouched() {
    let (s, keys, pin, dir) = tls_rig("raw");
    let k = DeviceKey::from_seed([7; 32]);
    keys.enroll("alice", &k.public_hex()).unwrap();
    let target = iyakki_link::cloudclient::Target { addr: s.local_addr().to_string(), pin };
    let mut ws = RawWs::open(&target, &Auth::Key { user: "alice".into(), key: DeviceKey::from_seed([7; 32]) }).expect("signed in");
    ws.send_text(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#).unwrap();
    ws.send_text(r#"{"t":"size","w":640,"h":400}"#).unwrap();
    let t0 = Instant::now();
    let (mut hello, mut frame) = (false, None);
    while t0.elapsed() < Duration::from_secs(10) && frame.is_none() {
        match ws.poll().expect("link stays up") {
            Some(Frame::Text(t)) if t.contains("hello_ok") => hello = true,
            Some(Frame::Binary(b)) => frame = Some(b),
            _ => {}
        }
    }
    assert!(hello, "the tenant's hello_ok arrived as text");
    let frame = frame.expect("a picture message");
    // untouched: it is a version 2 frame the link's own decoder reads, exactly as the tenant built it
    let mut canvas = iyakki_link::pixelcodec::Canvas::default();
    let h = iyakki_link::pixelcodec::decode_frame(&frame, &mut canvas).expect("a valid frame");
    assert_eq!((h.screen_w, h.screen_h), (640, 400));
    ws.close();
    // a key that is not enrolled is refused, with the server's words
    let other = Auth::Key { user: "alice".into(), key: DeviceKey::from_seed([9; 32]) };
    assert!(matches!(RawWs::open(&target, &other), Err(LinkError::Refused(_))));
    // a different pin is a hard stop before anything is signed
    let wrong = iyakki_link::cloudclient::Target { addr: target.addr.clone(), pin: Pin([1; 32]) };
    assert!(matches!(RawWs::open(&wrong, &Auth::Key { user: "alice".into(), key: DeviceKey::from_seed([7; 32]) }), Err(LinkError::PinChanged(_))));
    let _ = std::fs::remove_dir_all(dir);
}
