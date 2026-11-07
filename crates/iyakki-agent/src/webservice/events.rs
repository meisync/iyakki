//! `GET /api/v1/events`: the live event stream, one WebSocket per browser tab (plan: `docs/development/NOTIFICATIONS-PLAN.md`). The
//! token is NOT in the address (it would land in logs): the first message is `{"type":"auth","token":"..."}` and must come within 10
//! seconds. After that the server sends only this person's events: `ready`, `notification`, `count`, `approval`. A ping every 25
//! seconds is the heartbeat; a client that sends nothing for 90 seconds is dropped. A slow client loses its OLDEST queued events (the
//! hub never waits for it). Connections are capped per person and per tenant (`notifyhub::events`).
//!
//! [`Session`] is the whole protocol as a value with no socket in it; [`serve`] is the thin loop that feeds it.

use crate::httpmini::parse_head;
use crate::notifyhub::events::{event, Bus};
use crate::wsproto::{self as ws, Message, Parser};
use serde_json::json;
use std::io::{ErrorKind, Read, Write};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

pub const AUTH_WITHIN_SECS: u64 = 10;
pub const HEARTBEAT_SECS: u64 = 25;
pub const IDLE_SECS: u64 = 90;
const MAX_MESSAGE: usize = 4096;

/// A stream whose reads can be given a short time limit.
pub trait WsIo: Read + Write {
    fn set_read_timeout(&mut self, d: Duration);
}

/// Who signed in, and what their stream needs.
pub struct Authed {
    pub user: String,
    pub bus: Arc<Bus>,
}

pub trait Backing {
    /// The account behind a token, or `None`.
    fn auth(&self, token: &str) -> Option<Authed>;
    /// Approvals waiting for this person (tool calls and tenant requests).
    fn approvals(&self, who: &Authed) -> usize;
}

pub struct Session {
    started: u64,
    last_rx: u64,
    last_ping: u64,
    who: Option<Authed>,
    sub: Option<u64>,
    last_approvals: Option<usize>,
    closed: bool,
}

fn text(v: &serde_json::Value) -> Vec<u8> {
    ws::text_frame(&v.to_string())
}

impl Session {
    pub fn new(now: u64) -> Session {
        Session { started: now, last_rx: now, last_ping: now, who: None, sub: None, last_approvals: None, closed: false }
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

    /// A text message from the client. Frames to send back.
    pub fn on_text(&mut self, msg: &str, now: u64, b: &dyn Backing) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.last_rx = now;
        if self.who.is_some() {
            // nothing else is accepted from the client: it cannot ask for another person's events
            if serde_json::from_str::<serde_json::Value>(msg).ok().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from)).as_deref() != Some("ping") {
                self.close(&mut out, "unexpected message");
            } else {
                out.push(text(&json!({"type": "pong"})));
            }
            return out;
        }
        let tok = serde_json::from_str::<serde_json::Value>(msg).ok().filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("auth")).and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from));
        let Some(who) = tok.and_then(|t| b.auth(&t)) else {
            self.close(&mut out, "sign in again");
            return out;
        };
        match who.bus.subscribe(&who.user) {
            Ok(id) => self.sub = Some(id),
            Err(m) => {
                self.close(&mut out, m);
                return out;
            }
        }
        out.push(text(&json!({"type": "ready", "user": who.user, "heartbeat_secs": HEARTBEAT_SECS})));
        self.who = Some(who);
        out.extend(self.tick(now, b));
        out
    }

    /// Time passes: queued events, the approvals count when it changed, the heartbeat, the limits.
    pub fn tick(&mut self, now: u64, b: &dyn Backing) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if self.closed {
            return out;
        }
        let Some(who) = &self.who else {
            if now.saturating_sub(self.started) > AUTH_WITHIN_SECS {
                self.close(&mut out, "no sign-in received");
            }
            return out;
        };
        if now.saturating_sub(self.last_rx) > IDLE_SECS {
            self.close(&mut out, "");
            return out;
        }
        match self.sub.and_then(|id| who.bus.drain(id, Duration::ZERO)) {
            Some(evs) => out.extend(evs.iter().map(|e| ws::text_frame(e))),
            None => {
                self.close(&mut out, "");
                return out;
            }
        }
        let a = b.approvals(who);
        if self.last_approvals != Some(a) {
            self.last_approvals = Some(a);
            out.push(ws::text_frame(&event("approval", json!({"count": a}))));
        }
        if now.saturating_sub(self.last_ping) >= HEARTBEAT_SECS {
            self.last_ping = now;
            out.push(ws::frame(0x9, b""));
        }
        out
    }

    pub fn pong(&mut self, now: u64) {
        self.last_rx = now;
    }

    pub fn end(&mut self) {
        if let (Some(w), Some(id)) = (&self.who, self.sub.take()) {
            w.bus.unsubscribe(id);
        }
        self.closed = true;
    }
}

/// Is this head a request to open the event stream?
pub(super) fn is_events_head(head: &[u8]) -> bool {
    let p = b"GET /api/v1/events";
    head.starts_with(p) && matches!(head.get(p.len()), Some(b' ' | b'?')) && String::from_utf8_lossy(head).to_ascii_lowercase().contains("upgrade: websocket")
}

struct RealBacking<'a> {
    sh: &'a super::server::Shared,
}

impl Backing for RealBacking<'_> {
    fn auth(&self, token: &str) -> Option<Authed> {
        let user = self.sh.tokens.find(token)?;
        let p = self.sh.deps.auth.lookup(&user)?;
        let dir = super::tenants::data_dir_for(self.sh, Some(token), &p.user);
        Some(Authed { user: p.user, bus: crate::notifyhub::events::bus_for(&dir) })
    }
    fn approvals(&self, who: &Authed) -> usize {
        // tool-call approvals are in the run store; tenant requests are counted when the page asks (the stream only says "changed")
        self.sh.deps.store.pending_approvals(Some(&who.user)).len()
    }
}

fn refuse<S: Write>(io: &mut S, status: u16, msg: &str) {
    let r = super::server::error_answer(status, "invalid_input", msg);
    let _ = io.write_all(&r.to_bytes());
    let _ = io.flush();
}

/// Serve the stream on `io`. Returns false when this is not the events route of the running profile (the ordinary pipeline answers).
pub(super) fn serve<S: WsIo>(sh: &super::server::Shared, io: &mut S, buf: &[u8], peer: IpAddr) -> bool {
    use super::routes::{self, Found};
    let head = match parse_head(buf) {
        Ok(Some(h)) => h,
        _ => return false,
    };
    if !matches!(routes::find(sh.profile, &head.req.method, &head.req.path), Found::Route(r, _) if r.id == "events") {
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
    let key = head.req.header("sec-websocket-key").unwrap_or("");
    if key.is_empty() || head.req.header("sec-websocket-version") != Some("13") {
        refuse(io, 400, "not a WebSocket request");
        return true;
    }
    let resp = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nCache-Control: no-store\r\n\r\n", ws::accept_key(key));
    if io.write_all(resp.as_bytes()).and_then(|_| io.flush()).is_err() {
        return true;
    }
    run(io, &RealBacking { sh }, &head_rest(buf, head.body_at));
    true
}

fn head_rest(buf: &[u8], at: usize) -> Vec<u8> {
    buf.get(at..).unwrap_or(&[]).to_vec()
}

/// The loop: read what arrives (waiting at most 300 ms), feed the session, write what it says.
pub(super) fn run<S: WsIo>(io: &mut S, b: &dyn Backing, first: &[u8]) {
    io.set_read_timeout(Duration::from_millis(300));
    let now = || iyakki_schema::now_secs();
    let mut s = Session::new(now());
    let mut parser = Parser::new(MAX_MESSAGE);
    parser.push(first);
    let mut chunk = [0u8; 2048];
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
                Ok(Some(Message::Close)) | Err(_) => {
                    let _ = io.write_all(&ws::close_frame());
                    break 'outer;
                }
                Ok(Some(Message::Binary(_))) => {
                    let _ = io.write_all(&ws::close_frame());
                    break 'outer;
                }
                Ok(None) => break,
            }
        }
        for f in s.tick(now(), b) {
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
    s.end();
}
