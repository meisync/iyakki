//! Protocol version 2 over a real socket: negotiation (new and old pages), tiles that decode to the screen, backpressure with a
//! slow page, level changes, no extensions in the handshake, hostile messages.

use iyakki_remote::codec::{decode_frame, Canvas, Level};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The picture a fake screen shows; the test changes it and counts how many pictures were asked for.
struct Pic {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    ver: u64,
    delivered: u64,
    pulls: u64,
    legacy_frames: u64,
    inputs: Vec<String>,
    stream: bool,
}

type Shared = Arc<Mutex<Pic>>;

fn solid(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect()
}

fn paint(p: &Shared, c: [u8; 3]) {
    let mut g = p.lock().unwrap();
    g.rgba = solid(g.w, g.h, c);
    g.ver += 1;
}

struct Fake {
    pic: Shared,
    armed: bool,
    sent_legacy: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.pic.lock().unwrap().inputs.push(format!("{i:?}"));
    }
    fn viewing(&mut self, _: bool) {
        self.sent_legacy = false;
    }
    fn step(&mut self) -> Option<Frame> {
        std::thread::sleep(Duration::from_millis(3));
        let mut p = self.pic.lock().unwrap();
        if !p.stream && !self.sent_legacy {
            self.sent_legacy = true;
            p.legacy_frames += 1;
            return Some(Frame { x: 1, y: 2, w: 3, h: 4, png: vec![9, 9, 9] });
        }
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn set_stream(&mut self, on: bool) {
        self.pic.lock().unwrap().stream = on;
    }
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        let mut p = self.pic.lock().unwrap();
        if self.armed && p.ver != p.delivered {
            self.armed = false;
            p.delivered = p.ver;
            p.pulls += 1;
            return Some(Pixels { w: p.w, h: p.h, rgba: p.rgba.clone() });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate(Shared);
impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { pic: self.0.clone(), armed: false, sent_legacy: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

fn start(w: u32, h: u32) -> (Server, Shared) {
    let pic = Arc::new(Mutex::new(Pic { w, h, rgba: solid(w, h, [10, 20, 30]), ver: 1, delivered: 0, pulls: 0, legacy_frames: 0, inputs: vec![], stream: false }));
    let s = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate(pic.clone()))).unwrap();
    (s, pic)
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

fn sign_in(addr: std::net::SocketAddr) -> (String, String) {
    let body = r#"{"user":"ann","password":"right"}"#;
    let (code, head, body) = http(addr, &format!("POST /api/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert_eq!(code, 200, "{body}");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(&body).unwrap()["csrf"].as_str().unwrap().to_string();
    (cookie, csrf)
}

fn masked(op: u8, p: &[u8]) -> Vec<u8> {
    let mask = [3u8, 1, 4, 1];
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

struct Page {
    s: TcpStream,
    head: String,
}

impl Page {
    fn open(addr: std::net::SocketAddr, cookie: &str, csrf: &str, extra: &str) -> Page {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}Cookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        Page { s, head }
    }

    fn text(&mut self, t: &str) {
        self.s.write_all(&masked(1, t.as_bytes())).unwrap();
    }

    /// The next frame, or `None` when nothing arrives within `wait`.
    fn read(&mut self, wait: Duration) -> Option<(u8, Vec<u8>)> {
        self.s.set_read_timeout(Some(wait)).unwrap();
        let mut h = [0u8; 2];
        match self.s.read_exact(&mut h) {
            Ok(()) => {}
            Err(_) => return None,
        }
        self.s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let len = match h[1] & 0x7f {
            126 => {
                let mut l = [0u8; 2];
                self.s.read_exact(&mut l).unwrap();
                u16::from_be_bytes(l) as usize
            }
            127 => {
                let mut l = [0u8; 8];
                self.s.read_exact(&mut l).unwrap();
                u64::from_be_bytes(l) as usize
            }
            n => n as usize,
        };
        let mut p = vec![0u8; len];
        self.s.read_exact(&mut p).unwrap();
        Some((h[0] & 0x0f, p))
    }

    /// Read until a text message with `"t":kind` arrives (other messages are collected in `skipped`).
    fn until_text(&mut self, kind: &str, skipped: &mut Vec<(u8, Vec<u8>)>) -> serde_json::Value {
        let t0 = Instant::now();
        loop {
            assert!(t0.elapsed() < Duration::from_secs(8), "no {kind} message");
            if let Some((op, p)) = self.read(Duration::from_millis(200)) {
                if op == 1 {
                    if let Ok(j) = serde_json::from_slice::<serde_json::Value>(&p) {
                        if j["t"] == kind {
                            return j;
                        }
                    }
                }
                skipped.push((op, p));
            }
        }
    }

    fn next_binary(&mut self, wait: Duration) -> Option<Vec<u8>> {
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            if let Some((op, p)) = self.read(Duration::from_millis(100)) {
                if op == 2 {
                    return Some(p);
                }
            }
        }
        None
    }
}

fn hello(deflate: bool) -> String {
    format!(r#"{{"t":"hello","v":2,"caps":{{"deflate":{deflate}}}}}"#)
}

#[test]
fn a_new_page_says_hello_and_gets_tiles_that_decode_to_the_screen() {
    let (srv, pic) = start(200, 130);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(&hello(true));
    let mut skipped = Vec::new();
    let ok = p.until_text("hello_ok", &mut skipped);
    assert_eq!(ok["v"], 2);
    assert_eq!(ok["tile"], 64);
    assert!(skipped.iter().all(|(op, _)| *op != 2), "no legacy picture before the hello was answered");
    let msg = p.next_binary(Duration::from_secs(5)).expect("a picture");
    let mut canvas = Canvas::default();
    let h = decode_frame(&msg, &mut canvas).unwrap();
    assert_eq!((h.tiles, canvas.w, canvas.h), (4 * 3, 200, 130));
    assert!(canvas.rgba.chunks(4).all(|px| px == [10, 20, 30, 255]));
    assert_eq!(pic.lock().unwrap().legacy_frames, 0);
    // the page acknowledges; a change on the screen arrives as one tile
    p.text(&format!(r#"{{"t":"ack","n":{}}}"#, h.seq));
    {
        let mut g = pic.lock().unwrap();
        let w = g.w as usize;
        g.rgba[(70 * w + 70) * 4] = 255;
        g.ver += 1;
    }
    let msg = p.next_binary(Duration::from_secs(5)).expect("the change");
    let h2 = decode_frame(&msg, &mut canvas).unwrap();
    assert_eq!(h2.tiles, 1);
    assert_eq!(canvas.rgba[(70 * 200 + 70) * 4], 255);
    // the same picture again costs nothing
    p.text(&format!(r#"{{"t":"ack","n":{}}}"#, h2.seq));
    pic.lock().unwrap().ver += 1;
    assert!(p.next_binary(Duration::from_millis(900)).is_none(), "an unchanged screen sends nothing");
    // statistics arrive about once a second
    let mut skipped = Vec::new();
    let st = p.until_text("stats", &mut skipped);
    assert_eq!(st["level"], "balanced");
}

#[test]
fn an_old_page_without_hello_gets_the_png_pieces_as_before() {
    let (srv, pic) = start(100, 100);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    let (op, payload) = p.read(Duration::from_secs(5)).expect("a frame");
    assert_eq!(op, 2);
    assert_eq!(payload, vec![1, 0, 2, 0, 3, 0, 4, 0, 9, 9, 9], "the old eight-byte header and a PNG");
    // a hello that asks for an older version is answered with version 1 and the pictures stay as they were
    drop(p);
    std::thread::sleep(Duration::from_millis(200));
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(r#"{"t":"hello","v":1}"#);
    let mut skipped = Vec::new();
    let ok = p.until_text("hello_ok", &mut skipped);
    assert_eq!(ok["v"], 1);
    assert!(p.next_binary(Duration::from_secs(2)).is_some());
    assert_eq!(pic.lock().unwrap().inputs.len(), 0, "hello never reaches the shell");
}

#[test]
fn the_handshake_never_offers_extensions_so_tiles_are_not_compressed_twice() {
    let (srv, _) = start(100, 100);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let p = Page::open(srv.local_addr(), &cookie, &csrf, "Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n");
    assert!(!p.head.to_ascii_lowercase().contains("sec-websocket-extensions"), "{}", p.head);
}

#[test]
fn a_page_that_does_not_acknowledge_gets_at_most_two_pictures_and_then_the_latest() {
    let (srv, pic) = start(128, 128);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(&hello(true));
    let mut skipped = Vec::new();
    p.until_text("hello_ok", &mut skipped);
    // change the screen steadily for a second and a half while the page acknowledges nothing
    let t0 = Instant::now();
    let mut n = 0u8;
    while t0.elapsed() < Duration::from_millis(1500) {
        n = n.wrapping_add(7);
        paint(&pic, [n, 255 - n, n / 2]);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut got = Vec::new();
    while let Some(m) = p.next_binary(Duration::from_millis(300)) {
        got.push(m);
    }
    assert_eq!(got.len(), 2, "the window is two: nothing is queued behind a page that is not answering");
    let pictures_made = pic.lock().unwrap().pulls;
    assert!(pictures_made <= 4, "the screen was asked for {pictures_made} pictures, not one per change (about 150 changes)");
    // now the page answers: the next picture is the newest state, and nothing old is replayed
    let mut canvas = Canvas::default();
    let mut last = 0;
    for m in &got {
        last = decode_frame(m, &mut canvas).unwrap().seq;
    }
    paint(&pic, [1, 2, 3]);
    p.text(&format!(r#"{{"t":"ack","n":{last}}}"#));
    let mut ok = false;
    for _ in 0..10 {
        match p.next_binary(Duration::from_millis(600)) {
            Some(m) => {
                let h = decode_frame(&m, &mut canvas).unwrap();
                p.text(&format!(r#"{{"t":"ack","n":{}}}"#, h.seq));
                if canvas.rgba.chunks(4).all(|px| px == [1, 2, 3, 255]) {
                    ok = true;
                    break;
                }
            }
            None => break,
        }
    }
    assert!(ok, "the page ends on the newest picture");
}

#[test]
fn the_person_can_fix_the_quality_and_turn_on_the_data_saver() {
    let (srv, _) = start(100, 100);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(&hello(true));
    let mut skipped = Vec::new();
    p.until_text("hello_ok", &mut skipped);
    p.text(r#"{"t":"quality","level":"lossless","saver":false}"#);
    let t0 = Instant::now();
    loop {
        let st = p.until_text("stats", &mut skipped);
        if st["level"] == "lossless" && st["auto"] == false {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(6), "{st}");
    }
    p.text(r#"{"t":"quality","level":"lossless","saver":true}"#);
    let t0 = Instant::now();
    loop {
        let st = p.until_text("stats", &mut skipped);
        if st["level"] == "low" && st["saver"] == true {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(6), "{st}");
    }
    p.text(r#"{"t":"quality","level":"verylow"}"#);
    loop {
        let st = p.until_text("stats", &mut skipped);
        if st["level"] == "verylow" {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "{st}");
    }
    // at the lowest level the picture is half size
    p.text(r#"{"t":"full"}"#);
    let mut canvas = Canvas::default();
    let mut halved = false;
    for _ in 0..10 {
        if let Some(m) = p.next_binary(Duration::from_millis(500)) {
            let h = decode_frame(&m, &mut canvas).unwrap();
            p.text(&format!(r#"{{"t":"ack","n":{}}}"#, h.seq));
            if h.scale == 2 {
                halved = true;
                assert_eq!((canvas.w, canvas.h), (50, 50));
                break;
            }
        }
    }
    assert!(halved);
}

#[test]
fn a_page_without_inflate_gets_png_tiles_it_can_decode() {
    let (srv, _) = start(100, 70);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(&hello(false));
    let msg = p.next_binary(Duration::from_secs(5)).expect("a picture");
    assert_eq!(msg[1] & 1, 0, "nothing is deflated for a page that cannot inflate");
    let mut canvas = Canvas::default();
    decode_frame(&msg, &mut canvas).unwrap();
    assert!(canvas.rgba.chunks(4).all(|px| px == [10, 20, 30, 255]));
}

#[test]
fn hostile_messages_from_a_page_are_ignored_and_oversized_ones_end_the_connection() {
    let (srv, pic) = start(100, 100);
    let (cookie, csrf) = sign_in(srv.local_addr());
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    p.text(&hello(true));
    let mut rng = 12345u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let samples = [
        r#"{"t":"ack","n":99999999999}"#, r#"{"t":"ack","n":"x"}"#, r#"{"t":"hello","v":-5}"#, r#"{"t":"hello","v":"2"}"#, r#"{"t":"quality","level":null}"#,
        r#"{"t":"ping","ts":1e999}"#, r#"{"t":"ping","ts":-1}"#, r#"{"t":"size","w":1e99,"h":0}"#, r#"[1,2,3]"#, r#"null"#, "\u{0}\u{1}", r#"{"t":{"t":1}}"#,
    ];
    for s in samples {
        p.text(s);
    }
    for _ in 0..300 {
        let len = (next() % 80) as usize;
        let junk: String = (0..len).map(|_| (b' ' + (next() % 90) as u8) as char).collect();
        p.text(&junk);
        // plausible but wrong
        p.text(&format!(r#"{{"t":"ack","n":{}}}"#, next() % 1000));
        p.text(&format!(r#"{{"t":"ping","ts":{}}}"#, next() % 100));
    }
    // binary from the page is not something we read; it is ignored
    p.s.write_all(&masked(2, &[1, 2, 3])).unwrap();
    // the connection still works
    let mut sk = Vec::new();
    p.until_text("hello_ok", &mut sk);
    assert!(p.next_binary(Duration::from_secs(5)).is_some());
    // an unmasked frame is a protocol error: the server closes
    p.s.write_all(&[0x81, 0x02, b'h', b'i']).unwrap();
    let t0 = Instant::now();
    let mut closed = false;
    while t0.elapsed() < Duration::from_secs(5) {
        match p.read(Duration::from_millis(200)) {
            Some((8, _)) => {
                closed = true;
                break;
            }
            Some(_) => {}
            None => {}
        }
    }
    assert!(closed, "the server closes on a protocol error");
    // a message over the 512 KiB limit also ends it, on a fresh connection
    std::thread::sleep(Duration::from_millis(300));
    let mut p = Page::open(srv.local_addr(), &cookie, &csrf, "");
    let big = vec![b'a'; 600 * 1024];
    let _ = p.s.write_all(&masked(1, &big));
    let t0 = Instant::now();
    let mut closed = false;
    while t0.elapsed() < Duration::from_secs(5) {
        match p.s.read(&mut [0u8; 4096]) {
            Ok(0) | Err(_) => {
                closed = true;
                break;
            }
            Ok(_) => {}
        }
    }
    assert!(closed);
    assert!(pic.lock().unwrap().inputs.iter().all(|i| !i.contains("Hello")));
    // and the server still serves
    assert_eq!(http(srv.local_addr(), "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").0, 200);
}

#[test]
fn the_decoder_script_is_served_without_signing_in() {
    let (srv, _) = start(100, 100);
    let (code, head, body) = http(srv.local_addr(), "GET /decoder.js HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(code, 200);
    assert!(head.to_ascii_lowercase().contains("content-type: text/javascript"));
    assert!(body.contains("DecompressionStream") && body.contains("class Painter"));
    let (_, _, page) = http(srv.local_addr(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    for needed in ["/decoder.js", "hello", "transferControlToOffscreen", "pointerdown", "requestFullscreen", "id=\"stats\"", "Data saver", "beforeinput"] {
        assert!(page.contains(needed), "the page lacks {needed}");
    }
}
