//! The `window.iyakki` bridge of a web-bundle tile over a WebSocket (F-WEBSVC-12, KI-821): `GET /tiles/{id}/bridge` upgrades to a socket where the
//! page, from its own origin, makes the service calls its bundle's manifest lists, and nothing else. The token is NOT in the address (it would
//! land in logs): the first message is `{"type":"auth","token":"..."}` and must come within 10 seconds. After that:
//!
//! * `{"type":"call","id":7,"call":"clipboard.paste","args":{}}` is answered by `{"type":"result","id":7,"ok":true,"value":{...}}` or
//!   `{"type":"result","id":7,"ok":false,"error":"..."}`. A call the manifest does not list is refused here, before anything runs.
//! * `{"type":"ping"}` is answered by `{"type":"pong"}`.
//!
//! Each call goes through the SAME pipeline as `POST /api/v1/tile-bundles/{id}/call` (the person's own token, the origin and rate checks, the
//! tile's service backend with its approvals and audit lines): the bridge adds a transport and takes nothing away. A message is at most 64 KiB, a
//! socket makes at most 60 calls in 10 seconds, and one that says nothing for 5 minutes is dropped.
//!
//! [`Session`] is the whole protocol as a value with no socket in it; [`serve`] is the thin loop that feeds it.

use super::events::WsIo;
use crate::httpmini::parse_head;
use crate::wsproto::{self as ws, Message, Parser};
use serde_json::{json, Value};
use std::io::{ErrorKind, Write};
use std::net::IpAddr;
use std::time::Duration;

pub const AUTH_WITHIN_SECS: u64 = 10;
pub const IDLE_SECS: u64 = 300;
pub const MAX_MESSAGE: usize = 64 * 1024;
pub const CALLS_PER_WINDOW: usize = 60;
pub const WINDOW_SECS: u64 = 10;

pub trait Backing {
    /// May this token open the bridge of this tile? (the tile must be an installed bundle, the account must be allowed to use tiles)
    fn open(&self, tile: &str, token: &str) -> Option<Vec<String>>;
    /// Make one call as the person behind the token. `Ok(value)` or an error sentence.
    fn call(&self, tile: &str, token: &str, call: &str, args: &Value) -> Result<Value, String>;
}

pub struct Session {
    tile: String,
    started: u64,
    last_rx: u64,
    token: Option<String>,
    caps: Vec<String>,
    window: Vec<u64>,
    closed: bool,
}

fn text(v: &Value) -> Vec<u8> {
    ws::text_frame(&v.to_string())
}

/// A call name is `domain.method`: letters, digits and `_` in each part.
pub fn valid_call(c: &str) -> bool {
    let mut it = c.split('.');
    let (Some(d), Some(m), None) = (it.next(), it.next(), it.next()) else { return false };
    let ok = |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    ok(d) && ok(m)
}

impl Session {
    pub fn new(tile: &str, now: u64) -> Session {
        Session { tile: tile.to_string(), started: now, last_rx: now, token: None, caps: Vec::new(), window: Vec::new(), closed: false }
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    fn close(&mut self, out: &mut Vec<Vec<u8>>, why: &str) {
        if !why.is_empty() {
            out.push(text(&json!({"type": "error", "message": why})));
        }
        out.push(ws::close_frame());
        self.closed = true;
    }

    /// A text message from the page. Frames to send back.
    pub fn on_text(&mut self, msg: &str, now: u64, b: &dyn Backing) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.last_rx = now;
        let Ok(v) = serde_json::from_str::<Value>(msg) else {
            self.close(&mut out, "not JSON");
            return out;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if self.token.is_none() {
            let tok = v.get("token").and_then(Value::as_str).filter(|_| kind == "auth");
            let Some(tok) = tok else {
                self.close(&mut out, "sign in first");
                return out;
            };
            let Some(caps) = b.open(&self.tile, tok) else {
                self.close(&mut out, "sign in again");
                return out;
            };
            self.token = Some(tok.to_string());
            self.caps = caps;
            out.push(text(&json!({"type": "ready", "tile": self.tile, "caps": self.caps})));
            return out;
        }
        match kind {
            "ping" => out.push(text(&json!({"type": "pong"}))),
            "call" => {
                let id = v.get("id").cloned().filter(|i| i.is_u64()).unwrap_or(Value::Null);
                let reply = |ok: bool, key: &str, val: Value| text(&json!({"type": "result", "id": id, "ok": ok, key: val}));
                let call = v.get("call").and_then(Value::as_str).unwrap_or("");
                self.window.retain(|t| now.saturating_sub(*t) < WINDOW_SECS);
                if id.is_null() || !valid_call(call) {
                    out.push(reply(false, "error", json!("a call has a whole number \"id\" and a \"call\" like clipboard.paste")));
                } else if self.window.len() >= CALLS_PER_WINDOW {
                    out.push(reply(false, "error", json!("too many calls; wait a few seconds")));
                } else if !self.caps.iter().any(|c| c == call) {
                    out.push(reply(false, "error", json!(format!("this tile did not ask for {call}"))));
                } else {
                    self.window.push(now);
                    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
                    match b.call(&self.tile, self.token.as_deref().unwrap_or(""), call, &args) {
                        Ok(val) => out.push(reply(true, "value", val)),
                        Err(e) => out.push(reply(false, "error", json!(e))),
                    }
                }
            }
            _ => self.close(&mut out, "unexpected message"),
        }
        out
    }

    /// Time passes: the sign-in window and the idle limit.
    pub fn tick(&mut self, now: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if self.closed {
            return out;
        }
        if self.token.is_none() && now.saturating_sub(self.started) > AUTH_WITHIN_SECS {
            self.close(&mut out, "no sign-in received");
        } else if now.saturating_sub(self.last_rx) > IDLE_SECS {
            self.close(&mut out, "");
        }
        out
    }

    pub fn pong(&mut self, now: u64) {
        self.last_rx = now;
    }
}

/// Is this head a request to open a tile's bridge? Returns the tile id.
pub(super) fn bridge_head(head: &[u8]) -> Option<String> {
    let line_end = head.iter().position(|b| *b == b'\r' || *b == b'\n').unwrap_or(head.len());
    let line = std::str::from_utf8(&head[..line_end]).ok()?;
    let mut it = line.split(' ');
    let (Some("GET"), Some(target), Some(_)) = (it.next(), it.next(), it.next()) else { return None };
    let id = target.strip_prefix("/tiles/")?.strip_suffix("/bridge")?;
    if id.is_empty() || id.contains('/') || !String::from_utf8_lossy(head).to_ascii_lowercase().contains("upgrade: websocket") {
        return None;
    }
    Some(id.to_string())
}

struct RealBacking<'a> {
    sh: &'a super::server::Shared,
    peer: IpAddr,
}

impl Backing for RealBacking<'_> {
    fn open(&self, tile: &str, token: &str) -> Option<Vec<String>> {
        let user = self.sh.tokens.find(token)?;
        self.sh.deps.auth.lookup(&user)?;
        // a program's token (a paired developer key) may not open a bridge unless it holds the tiles scope, like every other tile route
        if let Some(route) = super::routes::ROUTES.iter().find(|r| r.id == "tile_bundle_call") {
            if super::server::program_refusal(self.sh, token, route).is_some() {
                return None;
            }
        }
        self.sh.tiles.bundles.get(tile).map(|b| b.caps.clone())
    }

    fn call(&self, tile: &str, token: &str, call: &str, args: &Value) -> Result<Value, String> {
        let body = json!({"call": call, "args": args}).to_string().into_bytes();
        let mut headers = std::collections::HashMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        headers.insert("content-type".to_string(), "application/json".to_string());
        let req = crate::httpmini::Request { method: "POST".into(), path: format!("/api/v1/tile-bundles/{tile}/call"), query: Default::default(), headers, body };
        let resp = super::server::handle(self.sh, &req, self.peer);
        let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        if resp.status == 200 {
            Ok(v.get("value").cloned().unwrap_or(Value::Null))
        } else {
            Err(v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the call was refused").to_string())
        }
    }
}

fn refuse<S: Write>(io: &mut S, status: u16, msg: &str) {
    let r = super::server::error_answer(status, "invalid_input", msg);
    let _ = io.write_all(&r.to_bytes());
    let _ = io.flush();
}

/// Serve the bridge on `io`. Returns false when this is not a bridge route of the running profile (the ordinary pipeline answers).
pub(super) fn serve<S: WsIo>(sh: &super::server::Shared, io: &mut S, buf: &[u8], peer: IpAddr) -> bool {
    use super::routes::{self, Found};
    let Some(tile) = bridge_head(buf) else { return false };
    let head = match parse_head(buf) {
        Ok(Some(h)) => h,
        _ => return false,
    };
    if !matches!(routes::find(sh.profile, &head.req.method, &head.req.path), Found::Route(r, _) if r.id == "tile_bridge") {
        return false;
    }
    if !super::server::origin_allowed(sh, &head.req) {
        refuse(io, 403, "origin not allowed");
        return true;
    }
    if !sh.addr_limit.allow(&peer.to_string()).0 {
        refuse(io, 429, "too many requests from this address");
        return true;
    }
    if sh.tiles.bundles.get(&tile).is_none() {
        refuse(io, 404, "no such tile");
        return true;
    }
    let key = head.req.header("sec-websocket-key").unwrap_or("");
    if key.is_empty() || head.req.header("sec-websocket-version") != Some("13") {
        refuse(io, 400, "not a WebSocket request");
        return true;
    }
    let resp = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nCache-Control: no-store\r\n\r\n", ws::accept_key(key));
    if io.write_all(resp.as_bytes()).and_then(|_| io.flush()).is_err() {
        return true;
    }
    run(io, &RealBacking { sh, peer }, &tile, buf.get(head.body_at..).unwrap_or(&[]));
    true
}

/// The loop: read what arrives (waiting at most 300 ms), feed the session, write what it says.
pub(super) fn run<S: WsIo>(io: &mut S, b: &dyn Backing, tile: &str, first: &[u8]) {
    io.set_read_timeout(Duration::from_millis(300));
    let now = || iyakki_schema::now_secs();
    let mut s = Session::new(tile, now());
    let mut parser = Parser::new(MAX_MESSAGE);
    parser.push(first);
    let mut chunk = [0u8; 4096];
    'outer: loop {
        loop {
            match parser.next() {
                Ok(Some(Message::Text(t))) => {
                    for f in s.on_text(&t, now(), b) {
                        if io.write_all(&f).is_err() {
                            break 'outer;
                        }
                    }
                }
                Ok(Some(Message::Ping(p))) => {
                    s.pong(now());
                    if io.write_all(&ws::pong_frame(&p)).is_err() {
                        break 'outer;
                    }
                }
                Ok(Some(Message::Pong)) => s.pong(now()),
                Ok(Some(Message::Close)) | Err(_) | Ok(Some(Message::Binary(_))) => {
                    let _ = io.write_all(&ws::close_frame());
                    break 'outer;
                }
                Ok(None) => break,
            }
        }
        for f in s.tick(now()) {
            if io.write_all(&f).is_err() {
                break 'outer;
            }
        }
        let _ = io.flush();
        if s.closed() {
            break;
        }
        match io.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => parser.push(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<(String, String, Value)>>,
    }

    impl Backing for Fake {
        fn open(&self, tile: &str, token: &str) -> Option<Vec<String>> {
            (tile == "notes" && token == "good").then(|| vec!["clipboard.paste".to_string(), "settings.get".to_string()])
        }
        fn call(&self, tile: &str, token: &str, call: &str, args: &Value) -> Result<Value, String> {
            self.calls.borrow_mut().push((tile.into(), token.into(), args.clone()));
            if call == "settings.get" { Err("your account may not use the chat".into()) } else { Ok(json!({"text": "hi"})) }
        }
    }

    fn texts(frames: &[Vec<u8>]) -> Vec<Value> {
        // server frames are unmasked text frames with a short length here
        frames.iter().filter(|f| f[0] == 0x81).map(|f| serde_json::from_slice(&f[2..]).unwrap()).collect()
    }

    fn fake() -> Fake {
        Fake { calls: RefCell::new(vec![]) }
    }

    #[test]
    fn the_page_must_sign_in_first_and_only_a_good_token_opens_the_bridge() {
        let b = fake();
        let mut s = Session::new("notes", 100);
        let out = s.on_text(r#"{"type":"call","id":1,"call":"clipboard.paste"}"#, 100, &b);
        assert!(s.closed() && texts(&out)[0]["message"] == "sign in first");
        assert!(b.calls.borrow().is_empty());
        let mut s = Session::new("notes", 100);
        let out = s.on_text(r#"{"type":"auth","token":"bad"}"#, 100, &b);
        assert!(s.closed() && texts(&out)[0]["message"] == "sign in again");
        let mut s = Session::new("other-tile", 100);
        s.on_text(r#"{"type":"auth","token":"good"}"#, 100, &b);
        assert!(s.closed(), "a tile that is not installed has no bridge");
        let mut s = Session::new("notes", 100);
        let out = texts(&s.on_text(r#"{"type":"auth","token":"good"}"#, 100, &b));
        assert_eq!(out[0]["type"], "ready");
        assert_eq!(out[0]["caps"], json!(["clipboard.paste", "settings.get"]));
        assert!(!s.closed());
        let mut late = Session::new("notes", 100);
        assert!(late.tick(105).is_empty());
        assert!(!late.tick(111).is_empty() && late.closed(), "no sign-in within 10 seconds closes the socket");
        let mut junk = Session::new("notes", 100);
        junk.on_text("not json", 100, &b);
        assert!(junk.closed());
    }

    #[test]
    fn a_call_is_made_only_for_a_listed_capability_and_answers_carry_the_id() {
        let b = fake();
        let mut s = Session::new("notes", 0);
        s.on_text(r#"{"type":"auth","token":"good"}"#, 0, &b);
        let r = texts(&s.on_text(r#"{"type":"call","id":7,"call":"clipboard.paste","args":{"x":1}}"#, 1, &b));
        assert_eq!(r[0], json!({"type": "result", "id": 7, "ok": true, "value": {"text": "hi"}}));
        assert_eq!(b.calls.borrow()[0], ("notes".to_string(), "good".to_string(), json!({"x": 1})));
        // an error from the service is an error result, not a closed socket
        let r = texts(&s.on_text(r#"{"type":"call","id":8,"call":"settings.get"}"#, 1, &b));
        assert_eq!(r[0]["ok"], false);
        assert!(r[0]["error"].as_str().unwrap().contains("chat"));
        // not in the manifest: refused before the backing is reached
        let before = b.calls.borrow().len();
        for call in ["files.write", "tools.shell", "clipboard.copy"] {
            let r = texts(&s.on_text(&format!(r#"{{"type":"call","id":9,"call":"{call}"}}"#), 1, &b));
            assert_eq!(r[0]["ok"], false, "{call}");
            assert!(r[0]["error"].as_str().unwrap().contains("did not ask"));
        }
        assert_eq!(b.calls.borrow().len(), before);
        // malformed calls say what a call looks like
        for bad in [r#"{"type":"call","call":"clipboard.paste"}"#, r#"{"type":"call","id":"x","call":"clipboard.paste"}"#, r#"{"type":"call","id":1,"call":"a.b.c"}"#, r#"{"type":"call","id":1,"call":""}"#, r#"{"type":"call","id":1,"call":"../x"}"#] {
            let r = texts(&s.on_text(bad, 1, &b));
            assert_eq!(r[0]["ok"], false, "{bad}");
            assert!(!s.closed());
        }
        assert_eq!(texts(&s.on_text(r#"{"type":"ping"}"#, 1, &b))[0]["type"], "pong");
        s.on_text(r#"{"type":"something"}"#, 2, &b);
        assert!(s.closed(), "an unknown message type ends the session");
    }

    #[test]
    fn calls_are_rationed_over_a_window_and_the_window_slides() {
        let b = fake();
        let mut s = Session::new("notes", 0);
        s.on_text(r#"{"type":"auth","token":"good"}"#, 0, &b);
        for i in 0..CALLS_PER_WINDOW {
            let r = texts(&s.on_text(&format!(r#"{{"type":"call","id":{i},"call":"clipboard.paste"}}"#), 5, &b));
            assert_eq!(r[0]["ok"], true, "{i}");
        }
        let r = texts(&s.on_text(r#"{"type":"call","id":99,"call":"clipboard.paste"}"#, 6, &b));
        assert_eq!(r[0]["ok"], false);
        assert!(r[0]["error"].as_str().unwrap().contains("too many"));
        assert_eq!(b.calls.borrow().len(), CALLS_PER_WINDOW);
        let r = texts(&s.on_text(r#"{"type":"call","id":100,"call":"clipboard.paste"}"#, 5 + WINDOW_SECS, &b));
        assert_eq!(r[0]["ok"], true, "after the window the calls flow again");
    }

    #[test]
    fn an_idle_socket_is_dropped_and_the_upgrade_request_is_told_from_other_requests() {
        let b = fake();
        let mut s = Session::new("notes", 0);
        s.on_text(r#"{"type":"auth","token":"good"}"#, 0, &b);
        assert!(s.tick(IDLE_SECS).is_empty());
        assert!(!s.tick(IDLE_SECS + 1).is_empty() && s.closed());
        let up = b"GET /tiles/notes/bridge HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
        assert_eq!(bridge_head(up).as_deref(), Some("notes"));
        assert!(bridge_head(b"GET /tiles/notes/bridge HTTP/1.1\r\nHost: x\r\n\r\n").is_none(), "no upgrade header: the ordinary pipeline answers");
        assert!(bridge_head(b"GET /tiles/a/b/bridge HTTP/1.1\r\nUpgrade: websocket\r\n\r\n").is_none());
        assert!(bridge_head(b"POST /tiles/notes/bridge HTTP/1.1\r\nUpgrade: websocket\r\n\r\n").is_none());
        assert!(bridge_head(b"GET /api/v1/events HTTP/1.1\r\nUpgrade: websocket\r\n\r\n").is_none());
        assert!(valid_call("clipboard.paste") && valid_call("a_1.b_2") && !valid_call("x") && !valid_call("a.b.c") && !valid_call("a. b"));
    }

    #[test]
    fn the_loop_speaks_the_protocol_over_a_pipe() {
        // a client's frames are masked; the server's are not. Run the loop on a canned stream and read what it wrote.
        struct Pipe {
            input: std::io::Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Pipe {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                self.input.read(b)
            }
        }
        impl Write for Pipe {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.output.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl WsIo for Pipe {
            fn set_read_timeout(&mut self, _: Duration) {}
        }
        let mut wire = Vec::new();
        for m in [r#"{"type":"auth","token":"good"}"#, r#"{"type":"call","id":1,"call":"clipboard.paste"}"#] {
            wire.extend(ws::client_frame(0x1, m.as_bytes(), [1, 2, 3, 4]));
        }
        let mut p = Pipe { input: std::io::Cursor::new(wire), output: Vec::new() };
        run(&mut p, &fake(), "notes", &[]);
        let mut parser = Parser::for_client(MAX_MESSAGE);
        parser.push(&p.output);
        let mut got = vec![];
        while let Ok(Some(Message::Text(t))) = parser.next() {
            got.push(serde_json::from_str::<Value>(&t).unwrap());
        }
        assert_eq!(got[0]["type"], "ready");
        assert_eq!(got[1], json!({"type": "result", "id": 1, "ok": true, "value": {"text": "hi"}}));
    }
}
