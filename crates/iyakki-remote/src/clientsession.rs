//! The session endpoint for clients (docs/development/CLIENT-SPLIT-DESIGN.md sections 5 to 7, slice 3). EXPERIMENTAL, off by default.
//!
//! One WebSocket at `/client/ws` per client, speaking the protocol of `iyakki_core_api::client`. The order is fixed and enforced here:
//!
//! 1. `hello`: version and capabilities settled; the instance sends a random challenge for this connection.
//! 2. `enrol` (a one-time token, once; the device key and name are stored) or `auth` (the device signs the challenge with the `IYAKKI-SIG`
//!    scheme). A device key alone proves which client it is, nothing more.
//! 3. `sign_in`: the person's own password through the same gate as the browser sign-in. Only now does the connection have a person; a
//!    device key never opens a vault or gives person-level data.
//! 4. Calls go to the broker (`iyakki_core_api::broker`) with `Caller` kind `Client`, so the access log shows device name and person;
//!    input and the one picture stream (the existing tile stream, tagged as stream 1) reach and leave the person's screen.
//!
//! Safety rules, each with a test: the whole endpoint is a plain 404 while the setting is off; a token works once and expires; a
//! challenge works once on one connection; a revoked device is refused and an open session of it is closed within a second; failed
//! tokens, signatures and passwords from one address are rate limited; nothing secret is logged or audited (no token, password or signature).

use crate::adapt::Controller;
use crate::codec::{Caps, Opts, Streamer};
use crate::input::{self, Input};
use crate::screen::{Gate, LoginError, Screen};
use crate::ws::{self, Message, Parser};
use iyakki_agent::clientlink::{self as link, AuthError, Device, TokenError};
use iyakki_core_api::broker::{client_key, Broker, Call, Caller, CallerKind};
use iyakki_core_api::client::{self as proto, ClientMsg, ErrorCode, EventRing, Resumed, ServerMsg};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod media;

/// Failed tokens, signatures and passwords from one address in a window before it is refused.
pub const MAX_FAILS: usize = 8;
pub const FAIL_WINDOW: Duration = Duration::from_secs(60);
/// Open client sessions at once.
pub const MAX_SESSIONS: usize = 8;
/// Wrong passwords on one connection before it is closed.
const MAX_SIGN_IN_FAILS: u32 = 5;
const EVENT_RING: usize = 256;

/// What a call does after the broker said yes, and whether the person's role allows it at all. The shell supplies the real one.
pub trait CallHandler: Send + Sync {
    /// The instance enforces the role of the signed-in person, never the client.
    fn role_allows(&self, person: &str, capability: &str) -> bool;
    fn perform(&self, person: &str, capability: &str, unit: Option<&str>) -> Result<String, String>;
}

/// Failures per address in a window; success costs nothing.
pub struct FailLimit {
    max: usize,
    window: Duration,
    seen: Mutex<HashMap<String, Vec<Instant>>>,
}

impl FailLimit {
    pub fn new(max: usize, window: Duration) -> FailLimit {
        FailLimit { max, window, seen: Mutex::new(HashMap::new()) }
    }

    fn prune(m: &mut HashMap<String, Vec<Instant>>, window: Duration) {
        let now = Instant::now();
        m.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < window);
            !v.is_empty()
        });
    }

    /// True when `who` has failed too often lately.
    pub fn blocked(&self, who: &str) -> bool {
        let Ok(mut m) = self.seen.lock() else { return true };
        Self::prune(&mut m, self.window);
        m.get(who).is_some_and(|v| v.len() >= self.max)
    }

    pub fn fail(&self, who: &str) {
        let Ok(mut m) = self.seen.lock() else { return };
        Self::prune(&mut m, self.window);
        if m.len() < 10_000 || m.contains_key(who) {
            m.entry(who.to_string()).or_default().push(Instant::now());
        }
    }
}

struct Open {
    /// Listed in the compat report while the session is open.
    _live: iyakki_agent::compatreport::LiveGuard,
    device: String,
    kill: Arc<AtomicBool>,
}

pub struct ClientEndpoint {
    data: PathBuf,
    broker: Arc<Broker>,
    calls: Arc<dyn CallHandler>,
    audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>,
    fails: FailLimit,
    ring: Mutex<EventRing>,
    open: Mutex<HashMap<u64, Open>>,
    next_open: AtomicU64,
    /// The media half (sound, clipboard, camera frames, the client's own devices); none: those messages answer `unsupported`.
    media: Option<Arc<iyakki_agent::clientmedia::MediaHost>>,
}

fn now_secs() -> i64 {
    iyakki_agent::clock::now_unix()
}

impl ClientEndpoint {
    /// `audit(person, action, detail)` is the audit log of the instance. Nothing secret is ever passed to it.
    pub fn new(data: &Path, broker: Arc<Broker>, calls: Arc<dyn CallHandler>, audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) -> ClientEndpoint {
        ClientEndpoint::with_limit(data, broker, calls, audit, FailLimit::new(MAX_FAILS, FAIL_WINDOW))
    }

    pub fn with_limit(data: &Path, broker: Arc<Broker>, calls: Arc<dyn CallHandler>, audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>, fails: FailLimit) -> ClientEndpoint {
        ClientEndpoint { data: data.to_path_buf(), broker, calls, audit, fails, ring: Mutex::new(EventRing::new(EVENT_RING)), open: Mutex::new(HashMap::new()), next_open: AtomicU64::new(1), media: None }
    }

    /// Serve the media half too (EXPERIMENTAL): the hello then offers `audio.out`, `clipboard.text`, `camera.frames`, `client.camera` and
    /// `client.microphone`. The broker of this endpoint should ask its hard asks through `media.card()`.
    pub fn with_media(mut self, media: Arc<iyakki_agent::clientmedia::MediaHost>) -> ClientEndpoint {
        self.media = Some(media);
        self
    }

    /// The experimental switch (Settings, Remote access). Off: every route of the endpoint answers 404.
    pub fn enabled(&self) -> bool {
        link::enabled(&self.data)
    }

    /// An event for `audience` (a person's name, or empty for every signed-in person), kept for resuming. Returns its sequence number.
    pub fn publish(&self, audience: &str, topic: &str, body: &str) -> u64 {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).push(audience, topic, body)
    }

    /// Sessions open now.
    pub fn open_sessions(&self) -> usize {
        self.open.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Revoke a device and close its open sessions now (a revoke made by another program is seen within a second).
    pub fn revoke(&self, id: &str, by: &str) -> Result<bool, String> {
        let done = link::revoke_device(&self.data, id, now_secs())?;
        if done {
            (self.audit)(by, "client.revoke", id);
        }
        if let Ok(m) = self.open.lock() {
            for o in m.values().filter(|o| o.device == id) {
                o.kill.store(true, Ordering::Relaxed);
            }
        }
        Ok(done)
    }

    fn register(&self, device: &str) -> Option<(u64, Arc<AtomicBool>)> {
        let mut m = self.open.lock().ok()?;
        if m.len() >= MAX_SESSIONS {
            return None;
        }
        let id = self.next_open.fetch_add(1, Ordering::Relaxed);
        let kill = Arc::new(AtomicBool::new(false));
        m.insert(id, Open { _live: iyakki_agent::compatreport::note_live("session", &format!("device {}", device.chars().take(8).collect::<String>()), &format!("client protocol {}", iyakki_core_api::client::PROTOCOL_VERSION)), device: device.to_string(), kill: kill.clone() });
        Some((id, kill))
    }

    fn unregister(&self, id: u64) {
        if let Ok(mut m) = self.open.lock() {
            m.remove(&id);
        }
    }

    /// Carry one client over `conn` (a WebSocket already upgraded; reads must time out after a few milliseconds instead of blocking).
    /// `peer` is the address for rate limiting. Returns when either side ends the session.
    pub fn serve<S: Read + Write>(&self, conn: &mut S, gate: &dyn Gate, peer: &str, stop: &dyn Fn() -> bool) {
        let mut s = Session::new(self, peer);
        s.run(conn, gate, stop);
        // the machine's speakers come back, the leases go and the client's devices vanish, before the session number is given up
        s.media_teardown();
        if let Some((id, _)) = s.open.take() {
            self.unregister(id);
        }
        if let Some(screen) = s.screen.as_mut() {
            screen.viewing(false);
        }
        if let Some(d) = &s.device {
            (self.audit)(s.person.as_deref().unwrap_or(""), "client.session.end", &format!("{} {}", d.name, d.id));
        }
    }
}

struct Pump {
    ctl: Controller,
    st: Streamer,
    caps: Caps,
    armed: bool,
}

struct Session<'a> {
    ep: &'a ClientEndpoint,
    peer: &'a str,
    negotiated: bool,
    challenge: Option<String>,
    device: Option<Device>,
    open: Option<(u64, Arc<AtomicBool>)>,
    person: Option<String>,
    screen: Option<Box<dyn Screen>>,
    pump: Option<Pump>,
    sent_seq: u64,
    sign_in_fails: u32,
    out: Vec<u8>,
    close: bool,
    /// What `hello` agreed on.
    agreed: Vec<String>,
    media: media::MediaState,
}

fn say(out: &mut Vec<u8>, m: &ServerMsg) {
    out.extend(ws::text_frame(&proto::encode_server(m)));
}

impl<'a> Session<'a> {
    fn new(ep: &'a ClientEndpoint, peer: &'a str) -> Session<'a> {
        Session { ep, peer, negotiated: false, challenge: None, device: None, open: None, person: None, screen: None, pump: None, sent_seq: 0, sign_in_fails: 0, out: Vec::new(), close: false, agreed: Vec::new(), media: media::MediaState::default() }
    }

    /// Say why and end the connection.
    fn refuse(&mut self, code: ErrorCode, message: &str) {
        say(&mut self.out, &ServerMsg::Error { code, message: message.into() });
        self.close = true;
    }

    /// Say why, and carry on.
    fn complain(&mut self, code: ErrorCode, message: &str) {
        say(&mut self.out, &ServerMsg::Error { code, message: message.into() });
    }

    fn run<S: Read + Write>(&mut self, conn: &mut S, gate: &dyn Gate, stop: &dyn Fn() -> bool) {
        // text messages are held to MAX_TEXT (a clipboard to MAX_CLIPBOARD_MESSAGE) by `decode_client`; the bound here is for the largest binary message, one camera frame
        let mut parser = Parser::new(proto::MAX_CLIPBOARD_MESSAGE.max(proto::media::MAX_CAMERA_FRAME) + 1024);
        let mut buf = vec![0u8; 16 * 1024];
        let t0 = Instant::now();
        let mut last_check = Instant::now();
        let hello_deadline = Duration::from_secs(20);
        loop {
            if stop() {
                return;
            }
            let now = t0.elapsed().as_millis() as u64;
            match conn.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => parser.push(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(_) => return,
            }
            loop {
                match parser.next() {
                    Ok(Some(Message::Text(t))) => match proto::decode_client(&t) {
                        Ok(m) => self.on_message(m, gate, now),
                        Err(e) => self.refuse(e.code, &e.message),
                    },
                    Ok(Some(Message::Ping(p))) => self.out.extend(ws::pong_frame(&p)),
                    Ok(Some(Message::Close)) => {
                        let _ = flush(conn, &mut self.out);
                        let _ = conn.write_all(&ws::close_frame());
                        return;
                    }
                    // a client's own microphone and camera (only after an accepted offer); anything else is not used
                    Ok(Some(Message::Binary(b))) => self.client_binary(&b),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        self.refuse(ErrorCode::BadMessage, "that is not a valid WebSocket message");
                        break;
                    }
                }
                if self.close {
                    break;
                }
            }
            if !self.close && self.device.is_none() && t0.elapsed() > hello_deadline {
                self.refuse(ErrorCode::Closed, "too slow");
            }
            // a revoke (from Settings, another program, or this endpoint) or switching the setting off ends the session within a second
            if !self.close && (self.open.as_ref().is_some_and(|(_, k)| k.load(Ordering::Relaxed)) || last_check.elapsed() >= Duration::from_secs(1)) {
                last_check = Instant::now();
                if let Some(d) = &self.device {
                    let gone = self.open.as_ref().is_some_and(|(_, k)| k.load(Ordering::Relaxed)) || link::get_device(&self.ep.data, &d.id).is_none_or(|x| !x.active());
                    if gone {
                        say(&mut self.out, &ServerMsg::Bye { reason: "revoked".into() });
                        self.close = true;
                    } else {
                        link::touch_device(&self.ep.data, &d.id, now_secs());
                    }
                }
                if !self.close && !self.ep.enabled() {
                    say(&mut self.out, &ServerMsg::Bye { reason: "disabled".into() });
                    self.close = true;
                }
            }
            if !self.close {
                self.pump_screen(now);
                self.pump_events();
                self.pump_media();
            }
            if self.close {
                self.out.extend(ws::close_frame());
                let _ = flush(conn, &mut self.out);
                return;
            }
            if !flush(conn, &mut self.out) {
                return;
            }
        }
    }

    fn on_message(&mut self, m: ClientMsg, gate: &dyn Gate, now_ms: u64) {
        if !self.negotiated && !matches!(m, ClientMsg::Hello { .. } | ClientMsg::Bye) {
            return self.refuse(ErrorCode::BadSequence, "say hello first");
        }
        match m {
            ClientMsg::Hello { v, min_v, offered, needed, .. } => {
                if self.negotiated {
                    return self.refuse(ErrorCode::BadSequence, "hello was already said");
                }
                let mut server: Vec<&str> = proto::cap::SERVER.to_vec();
                if self.ep.media.is_some() {
                    server.extend_from_slice(proto::cap::SERVER_MEDIA);
                }
                match proto::negotiate(v, min_v, &offered, &needed, &server) {
                    Ok(caps) => match link::new_challenge() {
                        Ok(ch) => {
                            self.agreed = caps.clone();
                            self.negotiated = true;
                            self.challenge = Some(ch.clone());
                            say(&mut self.out, &ServerMsg::HelloOk { v: proto::PROTOCOL_VERSION.min(v), caps, challenge: ch, tile: crate::codec::TILE as u32, max_message: proto::MAX_TEXT, deprecations: iyakki_core_api::compat::deprecations().into_iter().map(|d| d.name).collect() });
                        }
                        Err(_) => self.refuse(ErrorCode::Internal, "the instance has no random numbers"),
                    },
                    Err(e) => self.refuse(e.code, &e.message),
                }
            }
            ClientMsg::Enrol { token, public, name } => self.enrol(&token, &public, &name),
            ClientMsg::Auth { authorization } => self.auth(&authorization),
            ClientMsg::SignIn { user, password } => self.sign_in(gate, &user, &password),
            ClientMsg::Call { id, capability, unit } => self.call(id, &capability, unit.as_deref()),
            ClientMsg::Input { msg } => self.input(&msg),
            ClientMsg::PictureOpen { deflate } => {
                let Some(screen) = self.screen.as_mut() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
                screen.set_stream(true);
                screen.viewing(true);
                self.pump = Some(Pump { ctl: Controller::new(now_ms), st: Streamer::new(), caps: Caps { deflate }, armed: false });
                say(&mut self.out, &ServerMsg::PictureOk);
            }
            ClientMsg::Ack { seq } => {
                if let Some(p) = self.pump.as_mut() {
                    p.ctl.on_ack(seq, now_ms);
                }
            }
            ClientMsg::Resume { last_seq } => {
                let Some(person) = self.person.clone() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
                let r = self.ep.ring.lock().unwrap_or_else(|e| e.into_inner());
                match r.since(last_seq, &person) {
                    Resumed::Events(v) => {
                        say(&mut self.out, &ServerMsg::ResumeOk { from: last_seq, count: v.len() as u64 });
                        self.sent_seq = last_seq;
                    }
                    Resumed::Gap { oldest } => {
                        say(&mut self.out, &ServerMsg::ResumeGap { oldest });
                        self.sent_seq = r.latest();
                    }
                }
            }
            ClientMsg::Ping { ts } => say(&mut self.out, &ServerMsg::Pong { ts }),
            ClientMsg::Bye => self.close = true,
            ClientMsg::AudioOpen { side } => self.audio_open(side),
            ClientMsg::AudioClose => self.audio_close(),
            ClientMsg::ClipboardPut { text, confirmed } => self.clipboard_put(&text, confirmed),
            ClientMsg::ClipboardGet { confirmed } => self.clipboard_get(confirmed),
            ClientMsg::CameraList => self.camera_list(),
            ClientMsg::CameraOpen { unit } => self.camera_open(&unit),
            ClientMsg::CameraClose { unit } => self.camera_close(&unit),
            ClientMsg::OfferOpen { kind } => self.offer_open(&kind),
            ClientMsg::OfferClose { kind } => self.offer_close(&kind),
        }
    }

    fn enrol(&mut self, token: &str, public: &str, name: &str) {
        if self.device.is_some() {
            return self.refuse(ErrorCode::BadSequence, "this connection is already a known device");
        }
        if self.ep.fails.blocked(self.peer) {
            return self.refuse(ErrorCode::RateLimited, "too many failed tries, wait a minute");
        }
        let now = now_secs();
        let by = match link::redeem_token(&self.ep.data, token, now) {
            Ok(by) => by,
            Err(e) => {
                self.ep.fails.fail(self.peer);
                let (code, words) = match e {
                    TokenError::Unknown => (ErrorCode::TokenUnknown, "that link is not known"),
                    TokenError::Expired => (ErrorCode::TokenExpired, "that link has expired: make a new one in Settings"),
                    TokenError::Used => (ErrorCode::TokenUsed, "that link was used already: make a new one in Settings"),
                };
                (self.ep.audit)("", "client.enrol.refused", code.as_str());
                return self.refuse(code, words);
            }
        };
        match link::enrol_device(&self.ep.data, name, public, &by, now) {
            Ok(dev) => {
                (self.ep.audit)(&by, "client.enrol", &format!("{} {}", dev.name, dev.id));
                match link::new_challenge() {
                    Ok(ch) => {
                        self.challenge = Some(ch.clone());
                        say(&mut self.out, &ServerMsg::Enrolled { device: dev.id, challenge: ch });
                    }
                    Err(_) => self.refuse(ErrorCode::Internal, "the instance has no random numbers"),
                }
            }
            // the token is spent; make a new link
            Err(e) => self.refuse(ErrorCode::BadEnrolment, &e),
        }
    }

    fn auth(&mut self, authorization: &str) {
        // a challenge is used once, whatever came of it
        let Some(challenge) = self.challenge.take() else { return self.refuse(ErrorCode::Replayed, "that challenge was used already") };
        if self.device.is_some() {
            return self.refuse(ErrorCode::BadSequence, "this connection is already a known device");
        }
        if self.ep.fails.blocked(self.peer) {
            return self.refuse(ErrorCode::RateLimited, "too many failed tries, wait a minute");
        }
        let Some(parsed) = iyakki_link::sigscheme::parse_authorization(authorization) else {
            self.ep.fails.fail(self.peer);
            return self.refuse(ErrorCode::BadMessage, "that answer is not understood");
        };
        let Some(dev) = link::get_device(&self.ep.data, &parsed.id) else {
            self.ep.fails.fail(self.peer);
            return self.refuse(ErrorCode::DeviceUnknown, "that device is not known: connect with a new link");
        };
        if !dev.active() {
            self.ep.fails.fail(self.peer);
            (self.ep.audit)("", "client.auth.refused", &format!("revoked {}", dev.id));
            return self.refuse(ErrorCode::DeviceRevoked, "that device was revoked");
        }
        let now = now_secs();
        if let Err(e) = link::verify_answer(&dev, authorization, &challenge, now.max(0) as u64) {
            self.ep.fails.fail(self.peer);
            (self.ep.audit)("", "client.auth.refused", &format!("signature {}", dev.id));
            let (code, words) = match e {
                AuthError::Malformed => (ErrorCode::BadMessage, "that answer is not understood"),
                AuthError::BadSignature => (ErrorCode::BadSignature, "that signature does not match"),
            };
            return self.refuse(code, words);
        }
        let Some(reg) = self.ep.register(&dev.id) else { return self.refuse(ErrorCode::RateLimited, "too many clients are connected") };
        link::touch_device(&self.ep.data, &dev.id, now);
        self.open = Some(reg);
        say(&mut self.out, &ServerMsg::Authed { device: dev.id.clone(), name: dev.name.clone() });
        self.device = Some(dev);
    }

    fn sign_in(&mut self, gate: &dyn Gate, user: &str, password: &str) {
        let Some(dev) = self.device.clone() else { return self.refuse(ErrorCode::NotAuthenticated, "prove the device first") };
        if self.person.is_some() {
            return self.complain(ErrorCode::BadSequence, "already signed in");
        }
        if self.ep.fails.blocked(self.peer) {
            return self.refuse(ErrorCode::RateLimited, "too many failed tries, wait a minute");
        }
        match gate.login(user, password) {
            Ok(screen) => {
                let person = user.trim().to_string();
                (self.ep.audit)(&person, "client.signin", &format!("{} {}", dev.name, dev.id));
                self.sent_seq = self.ep.ring.lock().unwrap_or_else(|e| e.into_inner()).latest();
                self.screen = Some(screen);
                say(&mut self.out, &ServerMsg::SignedIn { person: person.clone() });
                self.person = Some(person);
            }
            Err(LoginError::Invalid) => {
                self.ep.fails.fail(self.peer);
                self.sign_in_fails += 1;
                (self.ep.audit)("", "client.signin.refused", &dev.id);
                if self.sign_in_fails >= MAX_SIGN_IN_FAILS {
                    self.refuse(ErrorCode::SignInRefused, "wrong name or password");
                } else {
                    self.complain(ErrorCode::SignInRefused, "wrong name or password");
                }
            }
            Err(LoginError::Throttled(s)) => self.complain(ErrorCode::RateLimited, &format!("too many wrong passwords, try again in {s} s")),
            Err(LoginError::Other(_)) => self.complain(ErrorCode::Internal, "sign-in is not available now"),
        }
    }

    fn call(&mut self, id: u64, capability: &str, unit: Option<&str>) {
        let (Some(person), Some(dev)) = (self.person.clone(), self.device.clone()) else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        if capability.is_empty() || capability.len() > 64 || unit.is_some_and(|u| u.len() > 200) {
            return self.complain(ErrorCode::BadMessage, "that call is not understood");
        }
        let err = |code: &str, message: String| ServerMsg::CallErr { id, code: code.into(), message };
        if !self.ep.calls.role_allows(&person, capability) {
            (self.ep.audit)(&person, "client.call.refused", &format!("{capability} role {}", dev.id));
            return say(&mut self.out, &err(ErrorCode::RoleDenied.as_str(), "your role does not allow that".into()));
        }
        let caller = Caller { app: "client".into(), kind: CallerKind::Client { device_id: dev.id.clone() }, person: person.clone(), device: dev.name.clone() };
        let r = self.ep.broker.decide(&Call { caller: &caller, capability, unit, label: unit.unwrap_or("") });
        match r {
            Ok(()) => match self.ep.calls.perform(&person, capability, unit) {
                Ok(detail) => say(&mut self.out, &ServerMsg::CallOk { id, detail }),
                Err(e) => say(&mut self.out, &err("failed", e)),
            },
            Err(d) => say(&mut self.out, &err(d.code(), d.to_string())),
        }
    }

    fn input(&mut self, msg: &str) {
        let Some(screen) = self.screen.as_mut() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        match input::parse(msg) {
            // only keys, mouse, size and "send the whole picture again"; a client cannot switch on the microphone or camera of the screen
            Some(i @ (Input::Event(_) | Input::Size(..) | Input::Shift(_))) => screen.input(i),
            Some(Input::Full) => {
                if let Some(p) = self.pump.as_mut() {
                    p.st.reset();
                }
                screen.input(Input::Full);
            }
            Some(_) => self.complain(ErrorCode::Unsupported, "that input is not available to a client"),
            None => self.complain(ErrorCode::BadMessage, "that input is not understood"),
        }
    }

    fn pump_screen(&mut self, now: u64) {
        let Some(screen) = self.screen.as_mut() else { return };
        let _ = screen.step();
        if screen.finished() {
            say(&mut self.out, &ServerMsg::Bye { reason: "signed_out".into() });
            self.close = true;
            return;
        }
        let Some(p) = self.pump.as_mut() else { return };
        p.ctl.tick(now);
        if let Some(px) = screen.take_pixels() {
            p.armed = false;
            p.st.set_source(px.w as usize, px.h as usize, px.rgba);
        }
        if p.ctl.can_send(now) && p.st.wants_encode(now) {
            let o = Opts { level: p.ctl.level(), caps: p.caps, budget: p.ctl.budget(), now_ms: now };
            if let Some(e) = p.st.encode(&o) {
                p.ctl.on_send(e.seq, e.msg.len(), now);
                self.out.extend(ws::binary_frame(&proto::binary(proto::STREAM_PICTURE, &e.msg)));
            }
        }
        if !p.armed && p.ctl.can_send(now) {
            p.armed = true;
            screen.arm();
        }
    }

    fn pump_events(&mut self) {
        let Some(person) = self.person.clone() else { return };
        let r = self.ep.ring.lock().unwrap_or_else(|e| e.into_inner());
        if r.latest() == self.sent_seq {
            return;
        }
        match r.since(self.sent_seq, &person) {
            Resumed::Events(v) => {
                for k in v {
                    say(&mut self.out, &ServerMsg::Event { seq: k.seq, topic: k.topic, body: k.body });
                }
            }
            Resumed::Gap { oldest } => say(&mut self.out, &ServerMsg::ResumeGap { oldest }),
        }
        self.sent_seq = r.latest();
    }
}

fn flush<S: Write>(conn: &mut S, out: &mut Vec<u8>) -> bool {
    if out.is_empty() {
        return true;
    }
    let r = conn.write_all(out).and_then(|_| conn.flush()).is_ok();
    out.clear();
    r
}

/// The approval-book key of a device, for tools that grant it capabilities.
pub fn approval_key(device_id: &str) -> String {
    client_key(device_id)
}
