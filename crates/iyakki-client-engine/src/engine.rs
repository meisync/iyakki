//! The client engine's state machine, picture mode. Pure Rust, no sockets and no clock: the host (the browser page, a test, later the window
//! program) owns the connection and feeds it text and binary messages; the engine answers with [`Out`] records to act on.
//!
//! Order of a connection (docs/development/CLIENT-SPLIT-DESIGN.md section 5, `iyakki_core_api::client`):
//! `connected` sends `hello`; `hello_ok` leads to `enrol` (a link token, once) or to the signed challenge; `authed` asks the host for the
//! person's password; `signed_in` resumes events and opens the picture; binary stream 1 is decoded into the framebuffer and acknowledged.
//! A dropped connection keeps the device, the last event number and the picture, so `connected` again resumes where it stopped.
//!
//! The engine never stores or logs a token or a password: the token is taken out of the configuration when it is sent, and the password
//! passes through [`Engine::sign_in`] into the one message that carries it.

use crate::picture::{self, Canvas};
use crate::sign;
use iyakki_core_api::client::{self as proto, ClientMsg, ErrorCode, ServerMsg};
use serde::Serialize;
use std::collections::VecDeque;

/// What the host tells the engine once, when it starts.
#[derive(Clone, Default)]
pub struct Config {
    /// The one-time link token from the URL fragment (enrolment), or none for a device that is enrolled already.
    pub token: Option<String>,
    /// The device's Ed25519 public key, 64 hex digits.
    pub public: String,
    /// The name for the device list.
    pub name: String,
    /// The device id from an earlier enrolment, if there is one.
    pub device: Option<String>,
    /// Shown to the instance in `hello` (a product name and version, nothing about the person).
    pub client: String,
}

impl std::fmt::Debug for Config {
    // never the token
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config({}, device {:?}, token {})", self.name, self.device, if self.token.is_some() { "<set>" } else { "none" })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No connection yet.
    Idle,
    /// Hello sent.
    Connecting,
    /// A link token was sent with the device key.
    Enrolling,
    /// Waiting for the signature of the challenge.
    Proving,
    /// The device is known; the person has not signed in.
    SignIn,
    /// Signed in; the picture is being opened.
    Opening,
    /// The picture stream runs.
    Live,
    /// The connection ended; `wants_reconnect` says whether trying again can help.
    Closed,
    /// Stopped by a refusal that trying again cannot fix.
    Failed,
}

/// What the person (or the page) can do about a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Nothing to do but read it.
    None,
    /// Make a new link in Settings, Remote access, on the IYAKKI machine.
    NewLink,
    /// This browser's device record is no longer valid: delete it, then use a new link.
    ForgetDevice,
    /// Type the name and password again.
    SignIn,
    /// Wait and the page tries again by itself.
    Wait,
    /// Reload the page; if it comes back, it is a bug.
    Reload,
}

/// What a refusal means for a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    pub title: &'static str,
    pub message: &'static str,
    /// Trying again by itself cannot help (the page stops reconnecting).
    pub fatal: bool,
    pub action: Action,
    /// Seconds to wait before the page reconnects (0: the usual back-off).
    pub retry_s: u32,
}

/// The words for each stable error code. Exhaustive on purpose: a new code in the protocol does not compile until it has words here.
pub fn describe(code: ErrorCode) -> Described {
    let d = |title, message, fatal, action, retry_s| Described { title, message, fatal, action, retry_s };
    match code {
        ErrorCode::BadMessage => d("Message not understood", "The machine did not understand a message from this page, or this page did not understand the machine. Reload the page; if it happens again, the page and the machine are different versions.", true, Action::Reload, 0),
        ErrorCode::TooBig => d("Message too long", "A message was longer than the machine accepts. Reload the page.", true, Action::Reload, 0),
        ErrorCode::BadSequence => d("Out of order", "The page and the machine got out of step. Reload the page.", true, Action::Reload, 0),
        ErrorCode::VersionMismatch => d("Versions do not match", "This page and the IYAKKI machine speak different protocol versions. Update the machine, or reload this page to get the matching one.", true, Action::Reload, 0),
        ErrorCode::CapabilityMissing => d("Not supported by this machine", "The machine cannot offer something this page needs (the picture or input). Update the machine.", true, Action::None, 0),
        ErrorCode::TokenUnknown => d("This link is not known", "The machine does not know this link. Make a new one in Settings, Remote access, Connect a client.", true, Action::NewLink, 0),
        ErrorCode::TokenExpired => d("This link has expired", "Links work for ten minutes. Make a new one in Settings, Remote access, Connect a client.", true, Action::NewLink, 0),
        ErrorCode::TokenUsed => d("This link was already used", "A link works once. Make a new one in Settings, Remote access, Connect a client.", true, Action::NewLink, 0),
        ErrorCode::BadEnrolment => d("The device could not be added", "The machine did not accept this device's key or name (the list may be full: 16 devices). The link is spent; make a new one after removing a device you do not use.", true, Action::NewLink, 0),
        ErrorCode::BadSignature => d("The device proof was refused", "The machine did not accept this device's signature. The most common cause is a clock that is more than a minute wrong on this device; set the clock to automatic and reload. If the key changed, forget this device and use a new link.", true, Action::Reload, 0),
        ErrorCode::Replayed => d("Proof used twice", "The machine saw the same proof twice. Reload the page.", false, Action::Reload, 0),
        ErrorCode::DeviceUnknown => d("The machine does not know this device", "This device was removed, or this is a different machine. Forget this device here and connect with a new link.", true, Action::ForgetDevice, 0),
        ErrorCode::DeviceRevoked => d("This device was revoked", "Someone removed this device in Settings on the machine, so it cannot connect any more. To use it again, make a new link and connect again.", true, Action::ForgetDevice, 0),
        ErrorCode::RateLimited => d("Too many tries", "Too many wrong tries from this address. The page waits a minute and tries again by itself.", false, Action::Wait, 60),
        ErrorCode::NotAuthenticated => d("Device not proven", "The device has to be proven before anyone can sign in. Reload the page.", true, Action::Reload, 0),
        ErrorCode::SignInRefused => d("Wrong name or password", "The name or the password is wrong. Try again.", false, Action::SignIn, 0),
        ErrorCode::NotSignedIn => d("Sign in first", "Sign in with your name and password.", false, Action::SignIn, 0),
        ErrorCode::RoleDenied => d("Not allowed", "Your role on this machine does not allow that.", false, Action::None, 0),
        ErrorCode::Unsupported => d("Not available to a client", "That is not available from a client.", false, Action::None, 0),
        ErrorCode::Closed => d("Too slow", "The machine closed the connection because nothing was said for a while. The page connects again by itself.", false, Action::Wait, 0),
        ErrorCode::Internal => d("The machine had a problem", "The machine could not do that right now. The page tries again in a moment.", false, Action::Wait, 5),
    }
}

/// A record for the host to act on.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Out {
    /// Send this text message on the socket.
    Send { text: String },
    /// Sign the challenge: call `begin_sign`, sign the text it returns, then `finish_sign` (or `sign_with_seed`).
    Sign { device: String, challenge: String },
    /// Keep this device id (the machine named the device at enrolment); it is needed on every later connection.
    Device { id: String },
    Phase { phase: Phase, text: String },
    /// The picture changed. `w`, `h` are the framebuffer; `screen_w`, `screen_h` the machine's screen (input coordinates).
    Frame { w: usize, h: usize, screen_w: usize, screen_h: usize, seq: u32 },
    Event { seq: u64, topic: String, body: String },
    CallOk { id: u64, detail: String },
    CallErr { id: u64, code: String, message: String },
    Pong { ts: u64 },
    /// Something worth a line in the page that is not a failure.
    Notice { text: String },
    /// A refusal. `code` is the stable name of the protocol.
    Fail { code: String, title: String, message: String, fatal: bool, action: Action, retry_s: u32 },
    /// The machine ended the session (`revoked`, `disabled`, `signed_out`).
    Bye { reason: String, title: String, message: String, action: Action },
    /// What `hello` agreed on (so the page shows only the buttons this machine can serve).
    Caps { caps: Vec<String> },
    /// A message of the media half: `ev` is its tag (`audio_ok`, `audio_closed`, `audio_flush`, `clipboard_text`, `clipboard_ok`, `camera_ok`,
    /// `camera_closed`, `offer_ok`, `offer_closed`, `media_err`) and `body` the whole message.
    Media { ev: String, body: serde_json::Value },
}

struct Pending {
    device: String,
    challenge: String,
    ts: u64,
    nonce: String,
}

pub struct Engine {
    cfg: Config,
    phase: Phase,
    device: Option<String>,
    challenge: Option<String>,
    pending: Option<Pending>,
    last_seq: u64,
    canvas: Canvas,
    screen: (usize, usize),
    viewport: Option<(u32, u32)>,
    version: u64,
    frame_errors: u32,
    reconnect: bool,
    out: VecDeque<Out>,
}

fn msg_text(m: &ClientMsg) -> Out {
    Out::Send { text: proto::encode_client(m) }
}

impl Engine {
    pub fn new(cfg: Config) -> Engine {
        let device = cfg.device.clone().filter(|d| !d.is_empty());
        Engine { cfg, phase: Phase::Idle, device, challenge: None, pending: None, last_seq: 0, canvas: Canvas::default(), screen: (0, 0), viewport: None, version: 0, frame_errors: 0, reconnect: true, out: VecDeque::new() }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Should the host open a new connection after this one ended?
    pub fn wants_reconnect(&self) -> bool {
        self.reconnect
    }

    pub fn pop_out(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    pub fn take_out(&mut self) -> Vec<Out> {
        self.out.drain(..).collect()
    }

    /// The framebuffer (RGBA, `w * h * 4` bytes) and a number that changes whenever it does.
    pub fn framebuffer(&self) -> (&[u8], usize, usize, u64) {
        (&self.canvas.rgba, self.canvas.w, self.canvas.h, self.version)
    }

    /// The machine's screen size in its own pixels (what input coordinates mean); 0, 0 before the first picture.
    pub fn screen(&self) -> (usize, usize) {
        self.screen
    }

    fn say(&mut self, m: &ClientMsg) {
        self.out.push_back(msg_text(m));
    }

    fn phase_to(&mut self, p: Phase, text: &str) {
        self.phase = p;
        self.out.push_back(Out::Phase { phase: p, text: text.into() });
    }

    fn fail(&mut self, code: ErrorCode, server_words: Option<&str>) {
        let d = describe(code);
        let _ = server_words; // the stable text of the page is shown, not the machine's free text (it is logged by neither side)
        if d.fatal {
            self.reconnect = false;
            self.phase = Phase::Failed;
        }
        if code == ErrorCode::SignInRefused || code == ErrorCode::NotSignedIn {
            self.phase = Phase::SignIn;
        }
        self.out.push_back(Out::Fail { code: code.as_str().into(), title: d.title.into(), message: d.message.into(), fatal: d.fatal, action: d.action, retry_s: d.retry_s });
    }

    // ------------------------------------------------------------------------------------------ connection

    /// The socket is open: say hello. May be called again after a drop; the device, the event number and the picture are kept.
    pub fn connected(&mut self) {
        self.challenge = None;
        self.pending = None;
        self.frame_errors = 0;
        self.reconnect = true;
        self.phase_to(Phase::Connecting, "Connecting");
        let hello = ClientMsg::Hello {
            v: proto::PROTOCOL_VERSION,
            min_v: proto::MIN_PROTOCOL_VERSION,
            offered: [proto::cap::PICTURE, proto::cap::INPUT, proto::cap::EVENTS, proto::cap::CALLS].into_iter().chain(proto::cap::SERVER_MEDIA.iter().copied()).map(String::from).collect(),
            needed: vec![proto::cap::PICTURE.into(), proto::cap::INPUT.into()],
            client: self.cfg.client.clone(),
        };
        self.say(&hello);
    }

    /// The socket ended (either side).
    pub fn closed(&mut self) {
        if !matches!(self.phase, Phase::Failed | Phase::Closed) {
            self.phase_to(Phase::Closed, "Disconnected");
        }
    }

    /// The person ends the session.
    pub fn bye(&mut self) {
        self.reconnect = false;
        self.say(&ClientMsg::Bye);
    }

    pub fn on_text(&mut self, text: &str) {
        let m = match proto::decode_server(text) {
            Ok(m) => m,
            Err(e) => {
                self.out.push_back(Out::Notice { text: format!("a message from the machine was not understood ({})", e.code.as_str()) });
                return;
            }
        };
        match m {
            ServerMsg::HelloOk { caps, challenge, .. } => {
                if self.phase != Phase::Connecting {
                    return self.fail(ErrorCode::BadSequence, None);
                }
                if !caps.iter().any(|c| c == proto::cap::PICTURE) || !caps.iter().any(|c| c == proto::cap::INPUT) {
                    return self.fail(ErrorCode::CapabilityMissing, None);
                }
                self.challenge = Some(challenge);
                if self.device.is_some() {
                    self.ask_signature();
                } else if let Some(token) = self.cfg.token.take() {
                    self.phase_to(Phase::Enrolling, "Adding this device");
                    self.say(&ClientMsg::Enrol { token, public: self.cfg.public.clone(), name: self.cfg.name.clone() });
                } else {
                    self.reconnect = false;
                    self.phase = Phase::Failed;
                    self.out.push_back(Out::Fail {
                        code: "no_link".into(),
                        title: "No link".into(),
                        message: "This browser has no saved device and the address has no link. Open the link shown in Settings, Remote access, Connect a client on the IYAKKI machine.".into(),
                        fatal: true,
                        action: Action::NewLink,
                        retry_s: 0,
                    });
                }
                self.out.push_back(Out::Caps { caps });
            }
            ServerMsg::Enrolled { device, challenge } => {
                self.device = Some(device.clone());
                self.challenge = Some(challenge);
                self.out.push_back(Out::Device { id: device });
                self.ask_signature();
            }
            ServerMsg::Authed { name, .. } => self.phase_to(Phase::SignIn, &format!("This device is known as {name}. Sign in with your name and password.")),
            ServerMsg::SignedIn { person } => {
                if self.last_seq > 0 {
                    let last_seq = self.last_seq;
                    self.say(&ClientMsg::Resume { last_seq });
                }
                self.say(&ClientMsg::PictureOpen { deflate: true });
                self.phase_to(Phase::Opening, &format!("Signed in as {person}"));
                if let Some((w, h)) = self.viewport {
                    self.size(w, h);
                }
            }
            ServerMsg::PictureOk => self.phase_to(Phase::Live, "Connected"),
            ServerMsg::Event { seq, topic, body } => {
                self.last_seq = self.last_seq.max(seq);
                self.out.push_back(Out::Event { seq, topic, body });
            }
            ServerMsg::ResumeOk { count, .. } => {
                if count > 0 {
                    self.out.push_back(Out::Notice { text: format!("{count} event(s) came in while this page was away") });
                }
            }
            ServerMsg::ResumeGap { oldest } => {
                self.last_seq = oldest.saturating_sub(1);
                self.out.push_back(Out::Notice { text: "some events were missed while this page was away".into() });
            }
            ServerMsg::CallOk { id, detail } => self.out.push_back(Out::CallOk { id, detail }),
            ServerMsg::CallErr { id, code, message } => self.out.push_back(Out::CallErr { id, code, message }),
            ServerMsg::Pong { ts } => self.out.push_back(Out::Pong { ts }),
            ServerMsg::Error { code, message } => self.fail(code, Some(&message)),
            ServerMsg::Bye { reason } => self.on_bye(&reason),
            m @ (ServerMsg::AudioOk { .. }
            | ServerMsg::AudioClosed { .. }
            | ServerMsg::AudioFlush
            | ServerMsg::ClipboardText { .. }
            | ServerMsg::ClipboardOk
            | ServerMsg::CameraList { .. }
            | ServerMsg::CameraOk { .. }
            | ServerMsg::CameraClosed { .. }
            | ServerMsg::OfferOk { .. }
            | ServerMsg::OfferClosed { .. }
            | ServerMsg::MediaErr { .. }) => {
                let body = serde_json::to_value(&m).unwrap_or_default();
                let ev = body.get("t").and_then(|t| t.as_str()).unwrap_or("").to_string();
                self.out.push_back(Out::Media { ev, body });
            }
        }
    }

    fn on_bye(&mut self, reason: &str) {
        let (title, message, action, reconnect) = match reason {
            "revoked" => ("This device was revoked", "Someone removed this device in Settings on the machine. To use it again, make a new link and connect again.", Action::ForgetDevice, false),
            "disabled" => ("Connecting a client was switched off", "Connect a client was switched off in Settings on the machine. Switch it on there and reload.", Action::None, false),
            "signed_out" => ("Signed out", "You signed out on the machine.", Action::SignIn, false),
            _ => ("The machine ended the session", "The machine ended the session.", Action::None, true),
        };
        self.reconnect = reconnect;
        if !reconnect {
            self.phase = Phase::Failed;
        }
        self.out.push_back(Out::Bye { reason: reason.into(), title: title.into(), message: message.into(), action });
    }

    // ------------------------------------------------------------------------------------------ device proof

    fn ask_signature(&mut self) {
        let (Some(device), Some(challenge)) = (self.device.clone(), self.challenge.clone()) else { return };
        self.phase_to(Phase::Proving, "Proving this device");
        self.out.push_back(Out::Sign { device, challenge });
    }

    /// The text to sign for the open challenge (the browser signs it with WebCrypto). `ts` is the clock of this device in seconds; the
    /// instance accepts a minute either way. `nonce` is 16 to 64 hex digits, fresh each time.
    pub fn begin_sign(&mut self, ts: u64, nonce: &str) -> Option<String> {
        if !sign::nonce_ok(nonce) {
            return None;
        }
        let (device, challenge) = (self.device.clone()?, self.challenge.clone()?);
        let text = sign::request_text(&device, &challenge, ts, nonce);
        self.pending = Some(Pending { device, challenge, ts, nonce: nonce.into() });
        Some(text)
    }

    /// Send the answer with the 64-byte signature (hex) of the text from [`Engine::begin_sign`].
    pub fn finish_sign(&mut self, sig_hex: &str) -> bool {
        let Some(p) = self.pending.take() else { return false };
        let Some(sig) = sign::unhex(sig_hex).filter(|s| s.len() == 64) else { return false };
        self.challenge = None; // a challenge is used once
        let _ = &p.challenge;
        self.say(&ClientMsg::Auth { authorization: sign::header(&p.device, p.ts, &p.nonce, &sig) });
        true
    }

    /// Sign with a seed the page holds (browsers without Ed25519 in WebCrypto).
    pub fn sign_with_seed(&mut self, seed_hex: &str, ts: u64, nonce: &str) -> bool {
        let Some(seed) = sign::unhex(seed_hex).and_then(|b| <[u8; 32]>::try_from(b).ok()) else { return false };
        if !sign::nonce_ok(nonce) {
            return false;
        }
        let (Some(device), Some(challenge)) = (self.device.clone(), self.challenge.take()) else { return false };
        self.say(&ClientMsg::Auth { authorization: sign::sign_with_seed(&seed, &device, &challenge, ts, nonce) });
        true
    }

    /// The person's sign-in. The password goes into the one message that carries it and is not kept.
    pub fn sign_in(&mut self, user: &str, password: &str) -> bool {
        if self.phase != Phase::SignIn {
            return false;
        }
        self.say(&ClientMsg::SignIn { user: user.into(), password: password.into() });
        true
    }

    // ------------------------------------------------------------------------------------------ calls

    pub fn call(&mut self, id: u64, capability: &str, unit: Option<&str>) {
        self.say(&ClientMsg::Call { id, capability: capability.into(), unit: unit.map(str::to_string) });
    }

    pub fn ping(&mut self, ts: u64) {
        self.say(&ClientMsg::Ping { ts });
    }

    // ------------------------------------------------------------------------------------------ the media half

    fn signed_in(&self) -> bool {
        matches!(self.phase, Phase::Opening | Phase::Live)
    }

    /// Ask for the person's sound here. `side` is `client` (the machine's speakers stay quiet) or `both`.
    pub fn audio_open(&mut self, side: &str) -> bool {
        if !self.signed_in() || !matches!(side, "client" | "both") {
            return false;
        }
        self.say(&ClientMsg::AudioOpen { side: Some(side.into()) });
        true
    }

    pub fn audio_close(&mut self) -> bool {
        self.signed_in() && {
            self.say(&ClientMsg::AudioClose);
            true
        }
    }

    /// Send text to the machine. `confirmed` must be true: the page sets it only inside the click of its own button.
    pub fn clip_put(&mut self, text: &str, confirmed: bool) -> bool {
        if !self.signed_in() || !confirmed || text.is_empty() || text.len() > proto::media::MAX_CLIPBOARD {
            return false;
        }
        self.say(&ClientMsg::ClipboardPut { text: text.into(), confirmed: true });
        true
    }

    pub fn clip_get(&mut self, confirmed: bool) -> bool {
        if !self.signed_in() || !confirmed {
            return false;
        }
        self.say(&ClientMsg::ClipboardGet { confirmed: true });
        true
    }

    /// Ask which cameras of the machine this person may watch (answered as a media event `camera_list`).
    pub fn camera_list(&mut self) -> bool {
        self.signed_in() && {
            self.say(&ClientMsg::CameraList);
            true
        }
    }

    pub fn camera_open(&mut self, unit: &str) -> bool {
        self.signed_in() && !unit.is_empty() && {
            self.say(&ClientMsg::CameraOpen { unit: unit.into() });
            true
        }
    }

    pub fn camera_close(&mut self, unit: &str) -> bool {
        self.signed_in() && {
            self.say(&ClientMsg::CameraClose { unit: unit.into() });
            true
        }
    }

    /// Offer this device's `camera` or `microphone` to the machine (the page asks the browser's own permission first).
    pub fn offer_open(&mut self, kind: &str) -> bool {
        self.signed_in() && matches!(kind, "camera" | "microphone") && {
            self.say(&ClientMsg::OfferOpen { kind: kind.into() });
            true
        }
    }

    pub fn offer_close(&mut self, kind: &str) -> bool {
        self.signed_in() && matches!(kind, "camera" | "microphone") && {
            self.say(&ClientMsg::OfferClose { kind: kind.into() });
            true
        }
    }

    // ------------------------------------------------------------------------------------------ the picture

    pub fn on_binary(&mut self, msg: &[u8]) {
        if !matches!(self.phase, Phase::Opening | Phase::Live) {
            return;
        }
        let Some((stream, payload)) = proto::split_binary(msg) else { return };
        if stream != proto::STREAM_PICTURE {
            return; // a stream this engine does not know (sound, later): ignored, never an error
        }
        match picture::decode_frame(payload, &mut self.canvas) {
            Ok(h) => {
                self.frame_errors = 0;
                self.screen = (h.screen_w as usize, h.screen_h as usize);
                self.version += 1;
                self.say(&ClientMsg::Ack { seq: h.seq });
                self.out.push_back(Out::Frame { w: self.canvas.w, h: self.canvas.h, screen_w: self.screen.0, screen_h: self.screen.1, seq: h.seq });
            }
            Err(e) => {
                self.frame_errors += 1;
                self.out.push_back(Out::Notice { text: format!("a picture could not be drawn: {}", e.words()) });
                // ask for the whole picture again, but do not loop forever on a broken stream
                if self.frame_errors <= 3 {
                    self.send_input(serde_json::json!({ "t": "full" }));
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------ input

    fn send_input(&mut self, v: serde_json::Value) {
        if matches!(self.phase, Phase::Opening | Phase::Live) {
            self.say(&ClientMsg::Input { msg: v.to_string() });
        }
    }

    /// The size of the page's viewport in CSS pixels; the machine sizes its screen to it (within its own limits).
    pub fn size(&mut self, w: u32, h: u32) {
        self.viewport = Some((w, h));
        self.send_input(serde_json::json!({ "t": "size", "w": w, "h": h }));
    }

    /// A point of the drawn picture, in CSS pixels from its top left corner, in the machine's own pixels. `None` before the first picture.
    pub fn map_point(&self, rel_x: f64, rel_y: f64, view_w: f64, view_h: f64) -> Option<(i32, i32)> {
        let (sw, sh) = self.screen;
        if sw == 0 || sh == 0 || !(view_w > 0.0 && view_h > 0.0) || !rel_x.is_finite() || !rel_y.is_finite() {
            return None;
        }
        let x = (rel_x * sw as f64 / view_w).round().clamp(0.0, sw as f64 - 1.0);
        let y = (rel_y * sh as f64 / view_h).round().clamp(0.0, sh as f64 - 1.0);
        Some((x as i32, y as i32))
    }

    /// `kind` is `move`, `down`, `right` or `up`.
    pub fn pointer(&mut self, kind: &str, x: i32, y: i32) {
        if matches!(kind, "move" | "down" | "right" | "up") {
            self.send_input(serde_json::json!({ "t": kind, "x": x, "y": y }));
        }
    }

    pub fn wheel(&mut self, d: i32) {
        self.send_input(serde_json::json!({ "t": "wheel", "d": d.clamp(-20, 20) }));
    }

    pub fn shift(&mut self, on: bool) {
        self.send_input(serde_json::json!({ "t": "shift", "on": on }));
    }

    /// A key as the browser names it (`KeyboardEvent.key`), with `code` for chords.
    pub fn key(&mut self, key: &str, code: &str, ctrl: bool, alt: bool, shift: bool) {
        if key.is_empty() || key.len() > 32 {
            return;
        }
        self.send_input(serde_json::json!({ "t": "key", "key": key, "code": code, "ctrl": ctrl, "alt": alt, "shift": shift }));
    }

    /// Typed text (the on-screen keyboard of a phone): one key press per character.
    pub fn text(&mut self, s: &str) {
        for c in s.chars().take(2000) {
            self.send_input(serde_json::json!({ "t": "key", "key": c.to_string() }));
        }
    }

    /// Ask for the whole picture again.
    pub fn full(&mut self) {
        self.send_input(serde_json::json!({ "t": "full" }));
    }
}
