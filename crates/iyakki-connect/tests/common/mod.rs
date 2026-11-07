//! A fake tenant over real TLS on the loopback, for the connector's tests: password sign-in, the WebSocket with the version 2 handshake
//! (a real tile frame built by the picture codec, one more for each `ack` up to a limit), the files routes of the tenant. It is a FAKE:
//! it proves the connector's code and protocol handling, not a real desktop.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_link::cloudclient::{pin_of, Target};
use iyakki_link::pixelcodec::{Caps, Level, Opts, Streamer};
use iyakki_link::wsproto::{accept_key, binary_frame, text_frame, Message, Parser};

pub const PASSWORD: &str = "right";
pub const USER: &str = "alice";

pub struct Remote {
    pub target: Target,
    /// Text messages the tenant received over its WebSocket, in order.
    pub texts: Arc<Mutex<Vec<String>>>,
    /// Binary messages it received.
    pub bins: Arc<Mutex<Vec<Vec<u8>>>>,
    /// The first frame it sent (the bytes the page must receive unchanged).
    pub first_frame: Arc<Mutex<Vec<u8>>>,
    pub files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub logins: Arc<AtomicUsize>,
    pub ws_open: Arc<AtomicUsize>,
    /// Frames to send after the first one: one per `ack` until this many have gone out.
    pub frames_on_ack: Arc<AtomicUsize>,
}

fn tls_config() -> (Vec<u8>, Arc<rustls::ServerConfig>) {
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let c = params.self_signed(&key).unwrap();
    let der = c.der().to_vec();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![c.der().clone()], rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap())
        .unwrap();
    (der, Arc::new(cfg))
}

/// A picture frame message for the fake screen: `n` moves a bright box so every frame has a changed region.
pub fn frame_for(st: &mut Streamer, w: usize, h: usize, n: usize, ms: u64) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let v = ((x * 255 / w) as u8, (y * 255 / h) as u8, 90u8);
            rgba[i..i + 4].copy_from_slice(&[v.0, v.1, v.2, 255]);
        }
    }
    let bx = (n * 16) % (w - 64);
    for y in 100..164 {
        for x in bx..bx + 64 {
            let i = (y * w + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[250, 250, 20, 255]);
        }
    }
    st.set_source(w, h, rgba);
    st.encode(&Opts { level: Level::Lossless, caps: Caps { deflate: true }, budget: 1 << 20, now_ms: ms }).expect("a frame").msg
}

pub fn start_remote() -> Remote {
    let (der, cfg) = tls_config();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    let r = Remote {
        target: Target { addr: addr.clone(), pin: pin_of(&der) },
        texts: Default::default(),
        bins: Default::default(),
        first_frame: Default::default(),
        files: Default::default(),
        logins: Default::default(),
        ws_open: Default::default(),
        frames_on_ack: Default::default(),
    };
    let (texts, bins, first, files, logins, ws_open, fo) = (r.texts.clone(), r.bins.clone(), r.first_frame.clone(), r.files.clone(), r.logins.clone(), r.ws_open.clone(), r.frames_on_ack.clone());
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (cfg, texts, bins, first, files, logins, ws_open, fo) = (cfg.clone(), texts.clone(), bins.clone(), first.clone(), files.clone(), logins.clone(), ws_open.clone(), fo.clone());
            std::thread::spawn(move || {
                let _ = s.set_nodelay(true);
                let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
                let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(cfg).unwrap(), s);
                serve_one(&mut tls, &texts, &bins, &first, &files, &logins, &ws_open, &fo);
            });
        }
    });
    r
}

type Tls = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

fn read_head(tls: &mut Tls) -> Option<(String, Vec<(String, String)>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let at = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..at]).into_owned();
    let mut lines = head.lines();
    let first = lines.next()?.to_string();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Some((first, headers, buf[at + 4..].to_vec()))
}

fn answer(tls: &mut Tls, code: u16, extra: &str, body: &[u8]) {
    let _ = tls.write_all(format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
    let _ = tls.write_all(body);
    let _ = tls.flush();
}

fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let (mut o, mut i) = (Vec::new(), 0);
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            o.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
            i += 3;
        } else {
            o.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&o).into_owned()
}

#[allow(clippy::too_many_arguments)]
fn serve_one(tls: &mut Tls, texts: &Mutex<Vec<String>>, bins: &Mutex<Vec<Vec<u8>>>, first: &Mutex<Vec<u8>>, files: &Mutex<BTreeMap<String, Vec<u8>>>, logins: &AtomicUsize, ws_open: &AtomicUsize, frames_on_ack: &AtomicUsize) {
    let Some((line, headers, mut rest)) = read_head(tls) else { return };
    let h = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(unpct));
    let want = h("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let mut chunk = vec![0u8; 65536];
    while rest.len() < want {
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => rest.extend_from_slice(&chunk[..n]),
        }
    }
    let body = String::from_utf8_lossy(&rest).into_owned();
    match (method, path) {
        ("POST", "/api/login") => {
            logins.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if v["user"] == USER && v["password"] == PASSWORD {
                answer(tls, 200, "Set-Cookie: iyakki_session=sess; Path=/\r\n", br#"{"user":"alice","csrf":"csrf1"}"#);
            } else {
                answer(tls, 401, "", br#"{"error":"wrong name or password"}"#);
            }
        }
        ("POST", "/api/logout") => answer(tls, 200, "", br#"{"ok":true}"#),
        ("POST", "/api/v1/login") => {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if v["name"] == USER && v["password"] == PASSWORD {
                answer(tls, 200, "", br#"{"token":"tok","expires_in":3600,"user":"alice","role":"user"}"#);
            } else {
                answer(tls, 401, "", br#"{"error":"wrong name or password"}"#);
            }
        }
        ("GET", "/api/v1/tenants") => {
            if h("authorization").as_deref() != Some("Bearer tok") {
                return answer(tls, 401, "", br#"{"error":"sign in"}"#);
            }
            answer(tls, 200, "", br#"{"tenants":[{"id":"acme","name":"Acme Ltd","role":"member","current":true},{"id":"beta","name":"Beta\u0007 GmbH","role":"owner","current":false}],"current":"acme","can_switch":true,"scope":"own"}"#);
        }
        ("GET", "/api/v1/files") | ("GET", "/api/v1/files/download") | ("PUT", "/api/v1/files/upload") => {
            if h("authorization").as_deref() != Some("Bearer tok") {
                return answer(tls, 401, "", br#"{"error":"sign in"}"#);
            }
            let p = q("path").unwrap_or_default();
            let mut fl = files.lock().unwrap();
            match (method, path) {
                ("GET", "/api/v1/files") => {
                    let prefix = if p.is_empty() { String::new() } else { format!("{p}/") };
                    let mut seen = BTreeMap::new();
                    for (k, v) in fl.iter().filter(|(k, _)| k.starts_with(&prefix)) {
                        let tail = &k[prefix.len()..];
                        match tail.split_once('/') {
                            Some((d, _)) => {
                                seen.insert(d.to_string(), ("dir", 0));
                            }
                            None => {
                                seen.insert(tail.to_string(), ("file", v.len()));
                            }
                        }
                    }
                    let entries: Vec<_> = seen.iter().map(|(n, (k, s))| serde_json::json!({"name": n, "kind": k, "size": s})).collect();
                    answer(tls, 200, "", serde_json::json!({"path": p, "entries": entries}).to_string().as_bytes());
                }
                ("GET", _) => match fl.get(&p) {
                    Some(b) => answer(tls, 200, "", b),
                    None => answer(tls, 404, "", br#"{"error":"no such file"}"#),
                },
                _ => {
                    fl.insert(p, rest.clone());
                    answer(tls, 200, "", br#"{"ok":true}"#);
                }
            }
        }
        ("GET", "/ws") => {
            if !h("cookie").is_some_and(|c| c.contains("iyakki_session=sess")) || !q("csrf").is_some_and(|c| c == "csrf1") {
                return answer(tls, 401, "", br#"{"error":"not signed in"}"#);
            }
            let key = h("sec-websocket-key").unwrap_or_default();
            let _ = tls.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(&key)).as_bytes());
            let _ = tls.flush();
            ws_open.fetch_add(1, Ordering::SeqCst);
            let _ = tls.sock.set_read_timeout(Some(Duration::from_millis(20)));
            let mut parser = Parser::new(16 << 20);
            let mut streamer = Streamer::new();
            let t0 = Instant::now();
            let (mut sent, mut n) = (0usize, 0usize);
            let mut pre: Vec<Vec<u8>> = Vec::new();
            let mut buf = vec![0u8; 65536];
            'conn: loop {
                match tls.read(&mut buf) {
                    Ok(0) => break,
                    Ok(k) => parser.push(&buf[..k]),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
                loop {
                    match parser.next() {
                        Ok(Some(Message::Text(t))) => {
                            texts.lock().unwrap().push(t.clone());
                            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                            match v["t"].as_str() {
                                Some("hello") => {
                                    let _ = tls.write_all(&text_frame(r#"{"t":"hello_ok","v":2,"tile":64,"levels":["lossless"],"max_message":524288}"#));
                                    let f = frame_for(&mut streamer, 640, 400, 0, t0.elapsed().as_millis() as u64);
                                    // the later frames are built now, so that a measurement times the relay and the page, not this fake's encoder
                                    for i in 1..=frames_on_ack.load(Ordering::SeqCst) {
                                        pre.push(frame_for(&mut streamer, 640, 400, i, t0.elapsed().as_millis() as u64));
                                    }
                                    *first.lock().unwrap() = f.clone();
                                    let _ = tls.write_all(&binary_frame(&f));
                                    let _ = tls.flush();
                                }
                                Some("ack") if sent < frames_on_ack.load(Ordering::SeqCst) => {
                                    sent += 1;
                                    let f = std::mem::take(&mut pre[n]);
                                    n += 1;
                                    let _ = tls.write_all(&binary_frame(&f));
                                    let _ = tls.flush();
                                }
                                Some("ping") => {
                                    let _ = tls.write_all(&text_frame(&format!(r#"{{"t":"pong","ts":{}}}"#, v["ts"])));
                                    let _ = tls.flush();
                                }
                                _ => {}
                            }
                        }
                        Ok(Some(Message::Binary(b))) => bins.lock().unwrap().push(b),
                        Ok(Some(Message::Close)) | Err(_) => break 'conn,
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                    }
                }
            }
            ws_open.fetch_sub(1, Ordering::SeqCst);
        }
        _ => answer(tls, 404, "", br#"{"error":"not found"}"#),
    }
}

// ------------------------------------------------------------------------------------------------------- a plain client of the page server

pub fn http(port: u16, raw: &str) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").map_or(out.len(), |p| p + 4);
    let head = String::from_utf8_lossy(&out[..at]).into_owned();
    let code = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, head, out[at..].to_vec())
}

/// A GET with the given extra header lines (each ends with `\r\n`) and the correct Host unless `host` is given.
pub fn get(port: u16, path: &str, host: Option<&str>, extra: &str) -> (u16, String, Vec<u8>) {
    let host = host.map(str::to_string).unwrap_or_else(|| format!("127.0.0.1:{port}"));
    http(port, &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n"))
}

pub fn masked(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x11, 0x22, 0x33, 0x44];
    let mut f = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n <= 0xffff => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// What a WebSocket client in the page sees.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Got {
    Text(String),
    Binary(Vec<u8>),
    Close(u16),
    Eof,
}

pub struct Ws {
    pub s: TcpStream,
    buf: Vec<u8>,
}

impl Ws {
    /// Open a WebSocket on the page server with the given cookie and Origin; the answer's status code if it is not 101.
    pub fn open(port: u16, cookie: &str, origin: Option<&str>) -> Result<Ws, u16> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        let o = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
        s.write_all(format!("GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{o}Cookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let at = loop {
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p;
            }
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => return Err(0),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..at]).into_owned();
        let code: u16 = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        if code != 101 {
            return Err(code);
        }
        Ok(Ws { s, buf: buf[at + 4..].to_vec() })
    }

    pub fn send_text(&mut self, t: &str) {
        self.s.write_all(&masked(0x1, t.as_bytes())).unwrap();
    }

    pub fn send_binary(&mut self, b: &[u8]) {
        let _ = self.s.write_all(&masked(0x2, b));
    }

    /// The next message, or `Eof` after `secs` of silence or when the connection ended.
    pub fn next(&mut self, secs: u64) -> Got {
        let t0 = Instant::now();
        let mut chunk = vec![0u8; 65536];
        loop {
            if self.buf.len() >= 2 {
                let b = &self.buf;
                let (op, l7) = (b[0] & 0x0f, (b[1] & 0x7f) as usize);
                let (len, at) = match l7 {
                    126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
                    127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()) as usize, 10),
                    126 | 127 => (usize::MAX, 0),
                    n => (n, 2),
                };
                if len != usize::MAX && b.len() >= at + len {
                    let payload = b[at..at + len].to_vec();
                    self.buf.drain(..at + len);
                    return match op {
                        0x1 => Got::Text(String::from_utf8_lossy(&payload).into_owned()),
                        0x2 => Got::Binary(payload),
                        0x8 => Got::Close(if payload.len() >= 2 { u16::from_be_bytes([payload[0], payload[1]]) } else { 1005 }),
                        _ => continue,
                    };
                }
            }
            if t0.elapsed() > Duration::from_secs(secs) {
                return Got::Eof;
            }
            let _ = self.s.set_read_timeout(Some(Duration::from_millis(200)));
            match self.s.read(&mut chunk) {
                Ok(0) => return Got::Eof,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return Got::Eof,
            }
        }
    }

    /// The next text message that contains `needle` (others are skipped).
    pub fn text_with(&mut self, needle: &str, secs: u64) -> Option<String> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Got::Text(t) = self.next(1) {
                if t.contains(needle) {
                    return Some(t);
                }
            }
        }
        None
    }
}

pub fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}
