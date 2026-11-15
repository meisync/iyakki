//! `iyakki-connect window` against a FAKE instance over real TLS on the loopback (no browser here; the real one is in
//! `iyakki-remote/tests/window_browser.rs`): the launch address and its cookie, every refusal of the loopback server, the instance's page
//! passed through with its own policy while `keys.js` is replaced, the session socket relayed byte for byte with the instance seeing its own
//! Host and Origin and no cookie, signing only for a challenge the program saw, the device id believed only when the instance sent it, and a
//! changed certificate stopping everything. Signing is checked against the golden vectors the engine and the instance's verifier share.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_connect::instances::Instances;
use iyakki_connect::window::{sign_relay, FrameScan, Identity, Options, Persist, Seen, Window, WS_PATH};
use iyakki_link::cloudclient::{pin_of, Pin, Target};
use iyakki_link::sigscheme::{parse_authorization, DeviceKey};
use iyakki_link::wsproto::{accept_key, frame, text_frame};
use serde_json::{json, Value};
use sha2::Digest;

// ------------------------------------------------------------------------------------------------------------------ signing

fn vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../../iyakki-client-engine/tests/golden/sig.json")).unwrap()["vectors"].as_array().unwrap().clone()
}

fn seen_with(challenge: &str) -> Seen {
    let mut s = Seen::default();
    s.note(&json!({"t": "hello_ok", "v": 1, "caps": [], "challenge": challenge, "tile": 64, "max_message": 1000}).to_string());
    s
}

#[test]
fn the_program_signs_exactly_the_golden_vectors_and_only_for_a_challenge_it_saw() {
    for v in vectors() {
        let (device, challenge, ts) = (v["device"].as_str().unwrap(), v["challenge"].as_str().unwrap(), v["ts"].as_u64().unwrap());
        let key = DeviceKey::from_hex(v["seed"].as_str().unwrap()).unwrap();
        let text = v["text"].as_str().unwrap();
        let mut seen = seen_with(challenge);
        let sig = sign_relay(text, &key, &[device], &mut seen, ts).unwrap();
        let golden = v["authorization"].as_str().unwrap().rsplit_once("sig=").unwrap().1;
        assert_eq!(sig, golden, "the signature is the one the engine and the instance's scheme make: {v}");
        // and the whole header verifies as the instance verifies it
        let header = format!("IYAKKI-SIG id={device},ts={ts},nonce={},sig={sig}", v["nonce"].as_str().unwrap());
        let parsed = parse_authorization(&header).unwrap();
        assert!(parsed.verify(&key.public(), "GET", WS_PATH, challenge.as_bytes()));
        // a challenge is signed for once
        assert_eq!(sign_relay(text, &key, &[device], &mut seen, ts), Err("no such challenge came from the instance (or it was signed already)"));
    }
}

#[test]
fn everything_but_a_fresh_sign_in_for_this_device_is_refused() {
    let v = &vectors()[0];
    let (device, challenge, ts) = (v["device"].as_str().unwrap(), v["challenge"].as_str().unwrap(), v["ts"].as_u64().unwrap());
    let key = DeviceKey::from_hex(v["seed"].as_str().unwrap()).unwrap();
    let text = v["text"].as_str().unwrap();
    let try_sign = |t: &str, devices: &[&str], seen: &mut Seen, now: u64| sign_relay(t, &key, devices, seen, now);
    // a challenge the instance never sent
    assert!(try_sign(text, &[device], &mut Seen::default(), ts).is_err());
    // another device id, a device not enrolled here
    assert_eq!(try_sign(text, &["dsomeoneelse"], &mut seen_with(challenge), ts), Err("that is not this device"));
    // another route or method (a page asking for a vault or login signature)
    for bad in [text.replace("/client/ws", "/api/login-key"), text.replace("\nGET\n", "\nPOST\n"), text.replace("iyakki-vault-v2", "iyakki-vault-v1")] {
        assert!(try_sign(&bad, &[device], &mut seen_with(challenge), ts).is_err(), "{bad:?}");
    }
    // the clock: a minute either way is the limit
    assert!(try_sign(text, &[device], &mut seen_with(challenge), ts + 60).is_ok());
    assert!(try_sign(text, &[device], &mut seen_with(challenge), ts + 61).is_err());
    assert!(try_sign(text, &[device], &mut seen_with(challenge), ts - 61).is_err());
    // shapes
    for bad in ["", "hello", &format!("{text}\nextra"), &text.replace("\n00112233445566778899aabbccddeeff", "\nzz"), &text.replacen("1700000000", "01700000000", 1)] {
        assert!(try_sign(bad, &[device], &mut seen_with(challenge), ts).is_err(), "{bad:?}");
    }
    // a challenge whose hash is not the one in the text
    assert!(try_sign(text, &[device], &mut seen_with("another challenge"), ts).is_err());
}

#[test]
fn the_instances_messages_are_read_along_in_pieces_and_big_ones_are_skipped() {
    let mut stream = Vec::new();
    stream.extend(text_frame(&json!({"t": "hello_ok", "challenge": "c1"}).to_string()));
    stream.extend(frame(2, &vec![7u8; 70_000])); // a picture: skipped by length
    stream.extend(text_frame(&json!({"t": "enrolled", "device": "dnew", "challenge": "c2"}).to_string()));
    stream.extend(frame(1, &vec![b'x'; 20_000])); // a big text: skipped
    stream.extend(text_frame("{\"t\":\"pong\"}"));
    let mut out = Vec::new();
    let mut scan = FrameScan::default();
    for piece in stream.chunks(7) {
        scan.push(piece, &mut out);
    }
    assert_eq!(out.len(), 3, "{:?}", out.iter().map(|s| s.len()).collect::<Vec<_>>());
    let mut seen = Seen::default();
    for t in &out {
        seen.note(t);
    }
    assert_eq!(seen.enrolled.as_deref(), Some("dnew"));
    assert_eq!(seen.open_challenges(), 2);
    // garbage that is not a server frame stops the reading and nothing else
    let mut dead = FrameScan::default();
    let mut o = Vec::new();
    dead.push(&[0x81, 0x85, 1, 2, 3, 4, 0, 0, 0, 0, 0], &mut o);
    dead.push(&text_frame("{}"), &mut o);
    assert!(o.is_empty());
}

#[test]
fn the_signing_path_is_the_instances() {
    assert_eq!(WS_PATH, "/client/ws");
}

// ------------------------------------------------------------------------------------------------------------------ a fake instance

#[derive(Default)]
struct Fake {
    /// Request heads the instance received (lower-case header lines), in order.
    heads: Mutex<Vec<String>>,
    /// Text messages it received over the socket.
    texts: Mutex<Vec<String>>,
}

fn tls_config() -> (Vec<u8>, Arc<rustls::ServerConfig>) {
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let c = params.self_signed(&key).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![c.der().clone()], rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap())
        .unwrap();
    (c.der().to_vec(), Arc::new(cfg))
}

const PAGE_CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'";

fn start_instance() -> (Target, Arc<Fake>) {
    let (der, cfg) = tls_config();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    let fake = Arc::new(Fake::default());
    let f2 = fake.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (cfg, fake) = (cfg.clone(), f2.clone());
            std::thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
                let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(cfg).unwrap(), s);
                instance_one(&mut tls, &fake);
            });
        }
    });
    (Target { addr, pin: pin_of(&der) }, fake)
}

type SrvTls = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

fn instance_one(tls: &mut SrvTls, fake: &Fake) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let at = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..at]).to_string();
    let rest = buf[at + 4..].to_vec();
    fake.heads.lock().unwrap().push(head.to_ascii_lowercase());
    let line = head.lines().next().unwrap_or("").to_string();
    let header = |n: &str| head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|(_, v)| v.trim().to_string()));
    let ok = |tls: &mut SrvTls, ctype: &str, body: &[u8]| {
        let _ = tls.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Security-Policy: {PAGE_CSP}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
        let _ = tls.write_all(body);
        let _ = tls.flush();
    };
    match line.as_str() {
        "GET /connect HTTP/1.1" => ok(tls, "text/html; charset=utf-8", b"<p>INSTANCE PAGE</p>"),
        "GET /connect/page.js HTTP/1.1" => ok(tls, "text/javascript; charset=utf-8", b"/* instance page.js */"),
        "GET /connect/keys.js HTTP/1.1" => ok(tls, "text/javascript; charset=utf-8", b"/* instance keys.js: must never be served by the window */"),
        "GET /connect/media.js HTTP/1.1" => ok(tls, "text/javascript; charset=utf-8", b"/* instance media.js */"),
        "GET /connect/audio-worklet.js HTTP/1.1" => ok(tls, "text/javascript; charset=utf-8", b"/* instance audio-worklet.js */"),
        "GET /connect/engine.wasm HTTP/1.1" => ok(tls, "application/wasm", &[0, b'a', b's', b'm', 1, 0, 0, 0]),
        "GET /client/ws HTTP/1.1" => instance_socket(tls, &head, &header, rest, fake),
        _ => {
            let _ = tls.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = tls.flush();
        }
    }
}

fn instance_socket(tls: &mut SrvTls, _head: &str, header: &dyn Fn(&str) -> Option<String>, mut pending: Vec<u8>, fake: &Fake) {
    let key = header("sec-websocket-key").unwrap();
    let _ = tls.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(&key)).as_bytes());
    let _ = tls.write_all(&text_frame(&json!({"t": "hello_ok", "v": 1, "caps": [], "challenge": "chal-one-0123456789", "tile": 64, "max_message": 100000}).to_string()));
    let _ = tls.flush();
    let mut parser = iyakki_link::wsproto::Parser::new(1 << 20);
    let mut chunk = [0u8; 4096];
    loop {
        parser.push(&pending);
        pending.clear();
        while let Ok(Some(m)) = parser.next() {
            if let iyakki_link::wsproto::Message::Text(t) = m {
                fake.texts.lock().unwrap().push(t.clone());
                let reply = if t.contains("\"enrol\"") { json!({"t": "enrolled", "device": "dnewdevice01", "challenge": "chal-two-0123456789"}).to_string() } else { format!("echo:{t}") };
                let _ = tls.write_all(&text_frame(&reply));
                let _ = tls.flush();
            }
        }
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ a fake browser

struct Reply {
    code: u16,
    head: String,
    body: String,
}

fn raw(port: u16, request: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Reply { code: head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), head: head.to_string(), body: body.to_string() }
}

fn get(port: u16, path: &str, cookie: Option<&str>, extra: &str) -> Reply {
    let c = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
    raw(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{c}{extra}Connection: close\r\n\r\n"))
}

fn post(port: u16, path: &str, cookie: &str, csrf: Option<&str>, origin: Option<&str>, body: &str) -> Reply {
    let c = csrf.map(|c| format!("X-Csrf: {c}\r\n")).unwrap_or_default();
    let o = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
    raw(port, &format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\n{c}{o}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()))
}

/// Use the launch address; returns the cookie and the Location the browser is sent to.
fn launch(w: &Window) -> (String, String) {
    let path = w.launch_url().split("127.0.0.1:").nth(1).unwrap().split_once('/').unwrap().1;
    let r = get(w.port(), &format!("/{path}"), None, "");
    assert_eq!(r.code, 303, "{}", r.head);
    let cookie = r.head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).expect("a cookie").split(';').next().unwrap().to_string();
    assert!(r.head.contains("HttpOnly") && r.head.contains("SameSite=Lax"));
    let loc = r.head.lines().find_map(|l| l.strip_prefix("Location: ")).unwrap().to_string();
    (cookie, loc)
}

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-window-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn window(target: Target, token: Option<&str>, dir: &std::path::Path, key: DeviceKey, device: Option<&str>) -> Window {
    Window::start(Options {
        target,
        link_token: token.map(String::from),
        identity: Identity { key, device: device.map(String::from), saved: device.is_some() },
        persist: Some(Persist { instances: Instances::new(dir), name: "fake".into() }),
        port: 0,
    })
    .unwrap()
}

fn csrf_of(port: u16, cookie: &str) -> String {
    let r = get(port, "/local/device", Some(cookie), "");
    assert_eq!(r.code, 200, "{}", r.head);
    serde_json::from_str::<Value>(&r.body).unwrap()["csrf"].as_str().unwrap().to_string()
}

// ------------------------------------------------------------------------------------------------------------------ refusals

#[test]
fn the_loopback_server_refuses_what_it_should_before_it_asks_the_instance_anything() {
    let (target, fake) = start_instance();
    let dir = temp("refuse");
    let w = window(target, Some("0123456789abcdef0123456789abcdef"), &dir, DeviceKey::generate().unwrap(), None);
    let port = w.port();
    // nothing without the launch address' cookie
    for p in ["/connect", "/connect/page.js", "/connect/keys.js", "/local/device", "/client/ws"] {
        assert_eq!(get(port, p, None, "").code, 403, "{p}");
    }
    // a wrong launch token is refused and does not spend the real one
    assert_eq!(get(port, "/launch?t=nope", None, "").code, 403);
    assert_eq!(get(port, "/launch", None, "").code, 403);
    // wrong Host (DNS rebinding sends its own name), a doubled Host
    assert_eq!(raw(port, "GET /launch HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n").code, 403);
    assert_eq!(raw(port, &format!("GET /launch HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: evil.example\r\nConnection: close\r\n\r\n")).code, 403);
    assert_eq!(get(port, "/launch?t=x", None, "Origin: http://evil.example\r\n").code, 403);
    // the real one, once
    let (cookie, loc) = launch(&w);
    assert_eq!(loc, "/connect#0123456789abcdef0123456789abcdef", "the instance's token travels in the fragment of the redirect only");
    let again = format!("/{}", w.launch_url().split_once(&format!("{port}/")).unwrap().1);
    assert_eq!(get(port, &again, None, "").code, 403, "a second use of the launch address");
    assert_eq!(get(port, "/connect", Some("iyakki_window=wrong"), "").code, 403, "a wrong cookie");
    // with the cookie: wrong Origin, unknown paths, methods
    assert_eq!(get(port, "/connect", Some(&cookie), "Origin: http://evil.example\r\n").code, 403);
    assert_eq!(get(port, "/etc/passwd", Some(&cookie), "").code, 404);
    assert_eq!(get(port, "/connect/../local/device", Some(&cookie), "").code, 404);
    assert_eq!(get(port, "/connect/other.js", Some(&cookie), "").code, 404);
    assert_eq!(get(port, "/local/sign", Some(&cookie), "").code, 405);
    let csrf = csrf_of(port, &cookie);
    let origin = format!("http://127.0.0.1:{port}");
    let body = json!({"text": "x"}).to_string();
    assert_eq!(post(port, "/local/sign", &cookie, None, Some(&origin), &body).code, 403, "no CSRF value");
    assert_eq!(post(port, "/local/sign", &cookie, Some("wrong"), Some(&origin), &body).code, 403, "wrong CSRF value");
    assert_eq!(post(port, "/local/sign", &cookie, Some(&csrf), None, &body).code, 403, "no Origin");
    assert_eq!(post(port, "/local/sign", &cookie, Some(&csrf), Some("http://evil.example"), &body).code, 403, "another Origin");
    assert_eq!(post(port, "/local/sign", &cookie, Some(&csrf), Some(&origin), "not json").code, 400);
    assert_eq!(post(port, "/local/sign", &cookie, Some(&csrf), Some(&origin), &body).code, 403, "a text that is no sign-in");
    assert_eq!(post(port, "/local/device", &cookie, Some(&csrf), Some(&origin), &json!({"device": "dmade-up"}).to_string()).code, 400, "a device id the instance never sent");
    // a socket needs the upgrade and this window's own Origin
    assert_eq!(get(port, "/client/ws", Some(&cookie), "").code, 426);
    assert_eq!(get(port, "/client/ws", Some(&cookie), "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n").code, 403, "no Origin on a socket");
    // none of this reached the instance
    assert!(fake.heads.lock().unwrap().is_empty(), "{:?}", fake.heads.lock().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------------------------ the page and the socket

fn upgrade(port: u16, cookie: &str) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    s.write_all(format!("GET /client/ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(s.read(&mut b).unwrap(), 1);
        head.push(b[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.contains(&accept_key(key)));
    s
}

fn send_text(s: &mut TcpStream, t: &str) {
    let p = t.as_bytes();
    assert!(p.len() < 126);
    let mut f = vec![0x81, 0x80 | p.len() as u8, 9, 8, 7, 6];
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ [9u8, 8, 7, 6][i % 4]));
    s.write_all(&f).unwrap();
}

fn read_text(s: &mut TcpStream) -> String {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).unwrap();
    assert_eq!(h[0], 0x81);
    let mut p = vec![0u8; (h[1] & 0x7f) as usize];
    s.read_exact(&mut p).unwrap();
    String::from_utf8(p).unwrap()
}

#[test]
fn the_instances_page_passes_through_keys_js_is_ours_and_the_socket_is_relayed_and_enrolment_is_saved() {
    let (target, fake) = start_instance();
    let dir = temp("flow");
    let key = DeviceKey::generate().unwrap();
    let public = key.public_hex();
    let w = window(target.clone(), Some("0123456789abcdef0123456789abcdef"), &dir, key, None);
    let port = w.port();
    let (cookie, _) = launch(&w);

    // the instance's own page, script and engine, with the instance's policy
    let page = get(port, "/connect", Some(&cookie), "");
    assert_eq!((page.code, page.body.as_str()), (200, "<p>INSTANCE PAGE</p>"));
    assert!(page.head.contains(&format!("Content-Security-Policy: {PAGE_CSP}")), "{}", page.head);
    assert!(page.head.contains("text/html"));
    assert_eq!(get(port, "/connect/page.js", Some(&cookie), "").body, "/* instance page.js */");
    // the media half of the page (EXPERIMENTAL) comes from the instance too, so sound, clipboard and cameras work in a window
    let media = get(port, "/connect/media.js", Some(&cookie), "");
    assert_eq!((media.code, media.body.as_str()), (200, "/* instance media.js */"));
    assert!(media.head.contains("text/javascript"));
    assert_eq!(get(port, "/connect/audio-worklet.js", Some(&cookie), "").body, "/* instance audio-worklet.js */");
    assert_eq!(get(port, "/connect/media.js", None, "").code, 403, "and, like the rest of the page, only with this window's cookie");
    let wasm = get(port, "/connect/engine.wasm", None, "");
    assert!(wasm.head.contains("application/wasm") && wasm.body.starts_with('\0'), "the engine is fetched without credentials, like the page does it: {}", wasm.head);
    assert_eq!(get(port, "/connect/engine.wasm", None, "Origin: http://evil.example\r\n").code, 403, "but not for another site");
    // the key module is ours: the page asks this program to sign
    let keys = get(port, "/connect/keys.js", Some(&cookie), "");
    assert!(keys.body.contains("/local/sign") && !keys.body.contains("instance keys.js"), "{}", keys.body);
    assert!(!fake.heads.lock().unwrap().iter().any(|h| h.contains("keys.js")), "keys.js was never asked of the instance");
    // the program holds the key; the page is told only the public half
    let dev = get(port, "/local/device", Some(&cookie), "");
    let dev: Value = serde_json::from_str(&dev.body).unwrap();
    assert_eq!((dev["public"].as_str().unwrap(), dev["device"].is_null()), (public.as_str(), true));
    assert!(!dev.to_string().contains("seed"));

    // the instance saw its own name as Host and Origin, and nothing of this window's cookie
    let heads = fake.heads.lock().unwrap().clone();
    assert!(heads.iter().all(|h| h.contains(&format!("host: {}", target.addr)) && !h.contains("cookie") && !h.contains("iyakki_window")), "{heads:?}");

    // the socket: bytes both ways, the instance's Host and Origin on it
    let mut ws = upgrade(port, &cookie);
    let hello: Value = serde_json::from_str(&read_text(&mut ws)).unwrap();
    assert_eq!(hello["t"], "hello_ok");
    send_text(&mut ws, "{\"t\":\"hello\"}");
    assert_eq!(read_text(&mut ws), "echo:{\"t\":\"hello\"}");
    let ws_head = fake.heads.lock().unwrap().iter().find(|h| h.starts_with("get /client/ws")).cloned().unwrap();
    assert!(ws_head.contains(&format!("origin: https://{}", target.addr)) && !ws_head.contains("cookie"), "{ws_head}");

    // before the instance enrolled this device, a device id the page made up is not believed and nothing is written
    let origin = format!("http://127.0.0.1:{port}");
    let csrf = csrf_of(port, &cookie);
    assert_eq!(post(port, "/local/device", &cookie, Some(&csrf), Some(&origin), &json!({"device": "dnewdevice01"}).to_string()).code, 400);
    assert!(Instances::new(&dir).list().unwrap().is_empty());
    // enrolment: the instance answers `enrolled` with the device id and a challenge
    send_text(&mut ws, "{\"t\":\"enrol\",\"token\":\"x\"}");
    let enrolled: Value = serde_json::from_str(&read_text(&mut ws)).unwrap();
    assert_eq!(enrolled["device"], "dnewdevice01");
    // now the page may sign for that device over the challenge the program saw, and report the id
    let text = format!("iyakki-vault-v2\ndnewdevice01\nGET\n/client/ws\n{}\n{}\n00112233445566778899aabbccddeeff", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), iyakki_link::sigscheme::hex(&sha2::Sha256::digest(b"chal-two-0123456789")));
    let signed = post(port, "/local/sign", &cookie, Some(&csrf), Some(&origin), &json!({"text": text}).to_string());
    assert_eq!(signed.code, 200, "{} {}", signed.head, signed.body);
    let sig = serde_json::from_str::<Value>(&signed.body).unwrap()["sig"].as_str().unwrap().to_string();
    assert_eq!(sig.len(), 128);
    // the first challenge (hello_ok) is still open and signs for the same device too, once
    assert_eq!(post(port, "/local/sign", &cookie, Some(&csrf), Some(&origin), &json!({"text": text}).to_string()).code, 403, "the same challenge twice");
    assert_eq!(post(port, "/local/device", &cookie, Some(&csrf), Some(&origin), &json!({"device": "dnewdevice01"}).to_string()).code, 200);
    assert_eq!(w.identity(), (Some("dnewdevice01".into()), true));
    // written down: a record without a secret, a key file only this user can read
    let store = Instances::new(&dir);
    let rec = store.find("fake").unwrap().unwrap();
    assert_eq!((rec.addr.as_str(), rec.device.as_deref(), rec.public.as_str()), (target.addr.as_str(), Some("dnewdevice01"), public.as_str()));
    assert_eq!(store.key(&target.addr).unwrap().unwrap().public_hex(), public);
    assert!(!std::fs::read_to_string(dir.join("instances.json")).unwrap().contains(&store.key(&target.addr).unwrap().unwrap().seed_hex().unwrap()));
    for e in std::fs::read_dir(dir.join("device-keys")).unwrap() {
        assert_eq!(e.unwrap().metadata().unwrap().permissions().mode() & 0o077, 0);
    }
    // forgetting (the page does this when the instance says the device is gone) deletes both
    assert_eq!(post(port, "/local/forget", &cookie, Some(&csrf), Some(&origin), "{}").code, 200);
    assert!(store.list().unwrap().is_empty() && store.key(&target.addr).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_certificate_that_is_not_the_pinned_one_stops_the_window_and_nothing_is_passed_on() {
    let (target, fake) = start_instance();
    let dir = temp("pin");
    let wrong = Target { addr: target.addr.clone(), pin: Pin([9u8; 32]) };
    let w = window(wrong, None, &dir, DeviceKey::generate().unwrap(), Some("dabc"));
    let (cookie, _) = launch(&w);
    let r = get(w.port(), "/connect", Some(&cookie), "");
    assert_eq!(r.code, 502, "{}", r.head);
    assert!(!r.body.contains("INSTANCE PAGE"));
    assert!(w.stopped(), "a changed certificate ends the window");
    assert!(w.failure().unwrap().contains("not the one that was trusted"), "{:?}", w.failure());
    assert!(fake.heads.lock().unwrap().is_empty(), "not one request reached the instance");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------------------------ the commands

fn ctx(dir: &std::path::Path) -> iyakki_connect::Ctx {
    iyakki_connect::Ctx { store: iyakki_connect::pins::Store::new(dir), timeout: Duration::from_secs(8), interactive: false }
}

fn prepare(dir: &std::path::Path, args: &[&str]) -> Result<iyakki_connect::windowcmd::Prepared, iyakki_connect::Fail> {
    let args: Vec<String> = std::iter::once("window").chain(args.iter().copied()).map(String::from).collect();
    iyakki_connect::windowcmd::prepare(&args, &ctx(dir), &mut std::io::empty(), &mut Vec::new())
}

#[test]
fn window_pins_from_the_link_refuses_a_changed_or_disagreeing_certificate_and_needs_a_link_the_first_time() {
    let (target, _fake) = start_instance();
    let dir = temp("cmd");
    let token = "0123456789abcdef0123456789abcdef";
    let link = |pin: &str| format!("https://{}/connect{pin}#{token}", target.addr);

    // a link without a pin, not pinned yet, nobody to ask: nothing starts, nothing is remembered
    let e = prepare(&dir, &[&link(""), "--no-browser"]).err().expect("refused");
    assert!(e.text().contains("--yes"), "{}", e.text());
    assert!(ctx(&dir).store.get(&target.addr).unwrap().is_none());
    // a link whose pin is not what the instance shows: hard stop (status 2), nothing remembered
    let e = prepare(&dir, &[&link(&format!("?pin={}", Pin([3u8; 32]).hex())), "--no-browser"]).err().expect("refused");
    assert_eq!(e.status(), 2, "{}", e.text());
    assert!(ctx(&dir).store.get(&target.addr).unwrap().is_none());
    // the link names the right certificate: remembered at once, a window starts
    let p = prepare(&dir, &[&link(&format!("?pin={}", target.pin.display().replace(':', "%3A"))), "--no-browser"]).unwrap();
    assert!(p.print_only && p.window.launch_url().starts_with("http://127.0.0.1:"));
    assert_eq!(ctx(&dir).store.get(&target.addr).unwrap(), Some(target.pin));
    drop(p);
    // pinned differently: a hard stop whatever the link says
    ctx(&dir).store.set(&target.addr, Pin([4u8; 32])).unwrap();
    for l in [link(""), link(&format!("?pin={}", target.pin.hex()))] {
        let e = prepare(&dir, &[&l]).err().expect("refused");
        assert_eq!(e.status(), 2, "{}", e.text());
    }
    assert_eq!(ctx(&dir).store.get(&target.addr).unwrap(), Some(Pin([4u8; 32])), "the pin is never replaced silently");
    ctx(&dir).store.set(&target.addr, target.pin).unwrap();

    // pinned, but no device here and no link: the person is told what to do
    let e = prepare(&dir, &[&target.addr]).err().expect("needs a link");
    assert!(e.text().contains("one-time link"), "{}", e.text());
    // a bare http link is refused outright
    assert!(prepare(&dir, &[&format!("http://{}/connect#{token}", target.addr)]).is_err());

    // a saved instance opens by its name, with its own key, and `devices` lists and forgets it
    let store = Instances::new(&dir);
    let key = DeviceKey::generate().unwrap();
    store.save(&iyakki_connect::instances::Record { name: "home".into(), addr: target.addr.clone(), device: Some("dsaved000001".into()), public: key.public_hex() }, &key).unwrap();
    let p = prepare(&dir, &["home", "--no-browser"]).unwrap();
    assert_eq!(p.name, "home");
    assert_eq!(p.window.identity(), (Some("dsaved000001".into()), true));
    drop(p);
    let mut out = Vec::new();
    iyakki_connect::run(&["devices".into()], &ctx(&dir), &mut std::io::empty(), &mut out).unwrap();
    let listed = String::from_utf8(out).unwrap();
    assert!(listed.contains("home") && listed.contains("dsaved000001") && listed.contains(&iyakki_link::sigscheme::fingerprint(&key.public())), "{listed}");
    assert!(!listed.contains(&key.seed_hex().unwrap()));
    iyakki_connect::run(&["devices".into(), "forget".into(), "home".into()], &ctx(&dir), &mut std::io::empty(), &mut Vec::new()).unwrap();
    assert!(store.list().unwrap().is_empty() && store.key(&target.addr).unwrap().is_none());
    assert!(iyakki_connect::run(&["devices".into(), "forget".into(), "home".into()], &ctx(&dir), &mut std::io::empty(), &mut Vec::new()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
