//! The session protocol between an instance and a client (docs/development/CLIENT-SPLIT-DESIGN.md sections 2, 5 and 6; slice 3).
//!
//! One WebSocket per client. Text messages are the JSON of [`ClientMsg`] (client to instance) and [`ServerMsg`] (instance to client);
//! binary messages carry a stream: one byte naming the stream ([`STREAM_PICTURE`]) and then the bytes of that stream. The picture stream
//! is the existing tile codec (`iyakki_link::pixelcodec`) unchanged, so a client decodes it with the code the browser page already uses.
//!
//! Pure types and rules, no OS code and no sockets, so the same file builds for the browser engine (wasm32) and for the instance.
//!
//! Order of a connection: `hello`, then either `enrol` (a one-time token, once) or `auth` (a signed challenge), then `sign_in` (the
//! person's password; a device key alone gives no person-level data), then calls, input, pictures and events. Every refusal is an
//! [`ServerMsg::Error`] or [`ServerMsg::CallErr`] with a stable [`ErrorCode`].

use std::collections::VecDeque;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The version this build speaks. A client says the range it speaks; no overlap is `version_mismatch`.
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest version this build still answers.
pub const MIN_PROTOCOL_VERSION: u32 = 1;
/// The most bytes of one text message in either direction.
pub const MAX_TEXT: usize = 64 * 1024;
/// The most bytes of the one text message that may be longer than [`MAX_TEXT`]: `clipboard_put` and `clipboard_text` carry up to
/// [`media::MAX_CLIPBOARD`] bytes of text, which JSON may escape to six bytes a character.
pub const MAX_CLIPBOARD_MESSAGE: usize = 6 * media::MAX_CLIPBOARD + 256;
/// The most bytes of one `app_list` message (a display list of an SDK app, F-SDK-29): larger than [`MAX_TEXT`], never larger than this.
pub const MAX_APP_LIST: usize = 512 * 1024;
/// The path of the WebSocket, and what a signed challenge covers.
pub const WS_PATH: &str = "/client/ws";

/// Capability names a hello offers or needs.
pub mod cap {
    pub const PICTURE: &str = "picture";
    pub const INPUT: &str = "input";
    pub const EVENTS: &str = "events";
    pub const CALLS: &str = "calls";
    /// What this build of the instance can serve.
    pub const SERVER: &[&str] = &[PICTURE, INPUT, EVENTS, CALLS];
    // The media half (EXPERIMENTAL, `media` module). An instance offers these only when its media host is wired.
    /// Sound from the instance to the client, 16-bit PCM in 20 ms chunks.
    pub const AUDIO_OUT: &str = "audio.out";
    /// Text clipboard in both directions, only on a press of the client's own button.
    pub const CLIPBOARD_TEXT: &str = "clipboard.text";
    /// JPEG frames of an instance camera (5 per second at most).
    pub const CAMERA_FRAMES: &str = "camera.frames";
    /// The client's own camera offered to the instance as a virtual source.
    pub const CLIENT_CAMERA: &str = "client.camera";
    /// The client's own microphone offered to the instance as a virtual source.
    pub const CLIENT_MICROPHONE: &str = "client.microphone";
    /// Display-list mode (ADDITIVE, D-CLIENT-PROTO-DISPLAYLIST): the client asks for an SDK app by name and gets its display list as text
    /// (`app_open`, `app_input`, `app_close`; answers `app_list`, `app_closed`, `app_err`). The picture stream is unchanged and stays the default.
    pub const DISPLAY_LIST: &str = "display.list";
    /// What an instance offers besides [`SERVER`] and [`SERVER_MEDIA`]: the display-list mode.
    pub const SERVER_UI: &[&str] = &[DISPLAY_LIST];
    /// File transfer through the page (F-CLIENT-01, ADDITIVE): list the person's home folder, download one file, upload one file, each file with a card
    /// on the machine. Offered only by an instance whose media half is wired and whose screen has files.
    pub const FILE_TRANSFER: &str = "files.transfer";
    pub const SERVER_FILES: &[&str] = &[FILE_TRANSFER];
    /// What an instance with media wired serves, besides [`SERVER`].
    pub const SERVER_MEDIA: &[&str] = &[AUDIO_OUT, CLIPBOARD_TEXT, CAMERA_FRAMES, CLIENT_CAMERA, CLIENT_MICROPHONE];
}

pub mod media;

/// The binary stream tags (first byte of a binary message).
pub const STREAM_PICTURE: u8 = 1;
/// Instance to client: one 20 ms chunk of sound (see [`media`]).
pub const STREAM_AUDIO: u8 = 2;
/// Instance to client: one JPEG frame of a camera of the instance.
pub const STREAM_CAMERA: u8 = 3;
/// Client to instance: microphone sound of the client's own microphone (16 kHz mono 16-bit).
pub const STREAM_MIC_IN: u8 = 4;
/// Client to instance: one JPEG frame of the client's own camera.
pub const STREAM_CAMERA_IN: u8 = 5;
/// Instance to client: a chunk of a file being downloaded, `[id: u32 big endian][bytes]` (see [`FileMsg`]).
pub const STREAM_FILE_OUT: u8 = 6;
/// Client to instance: a chunk of a file being uploaded, `[id: u32 big endian][bytes]`.
pub const STREAM_FILE_IN: u8 = 7;
/// The most bytes of one file chunk, and of one file in either direction.
pub const FILE_CHUNK: usize = 32 * 1024;
pub const MAX_FILE: usize = crate::files::MAX_UPLOAD;

/// A binary message of `stream` carrying `payload`.
pub fn binary(stream: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 1);
    v.push(stream);
    v.extend_from_slice(payload);
    v
}

/// The stream tag and the payload of a binary message (`None` for an empty one).
pub fn split_binary(msg: &[u8]) -> Option<(u8, &[u8])> {
    msg.split_first().map(|(s, p)| (*s, p))
}

/// Why something was refused. The strings never change; clients and tests compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Not JSON, not a known message, or a field out of range.
    BadMessage,
    TooBig,
    /// A message that is not allowed at this point of the connection.
    BadSequence,
    VersionMismatch,
    CapabilityMissing,
    TokenUnknown,
    TokenExpired,
    TokenUsed,
    /// The key or the device name was not accepted at enrolment.
    BadEnrolment,
    /// The signature does not belong to the challenge or the device.
    BadSignature,
    /// The challenge was already used on this connection.
    Replayed,
    DeviceUnknown,
    DeviceRevoked,
    RateLimited,
    NotAuthenticated,
    /// Wrong name or password (one answer for both).
    SignInRefused,
    NotSignedIn,
    /// The person's role does not allow it.
    RoleDenied,
    Unsupported,
    Closed,
    Internal,
    /// A code a newer instance or client sent that this release does not know (the compatibility charter: enums are open). Never sent by this
    /// release.
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadMessage => "bad_message",
            ErrorCode::TooBig => "too_big",
            ErrorCode::BadSequence => "bad_sequence",
            ErrorCode::VersionMismatch => "version_mismatch",
            ErrorCode::CapabilityMissing => "capability_missing",
            ErrorCode::TokenUnknown => "token_unknown",
            ErrorCode::TokenExpired => "token_expired",
            ErrorCode::TokenUsed => "token_used",
            ErrorCode::BadEnrolment => "bad_enrolment",
            ErrorCode::BadSignature => "bad_signature",
            ErrorCode::Replayed => "replayed",
            ErrorCode::DeviceUnknown => "device_unknown",
            ErrorCode::DeviceRevoked => "device_revoked",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::NotAuthenticated => "not_authenticated",
            ErrorCode::SignInRefused => "sign_in_refused",
            ErrorCode::NotSignedIn => "not_signed_in",
            ErrorCode::RoleDenied => "role_denied",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::Closed => "closed",
            ErrorCode::Internal => "internal",
            ErrorCode::Unknown => "unknown",
        }
    }
}

/// A refusal with its stable code and words for a person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtoError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ProtoError {
        ProtoError { code, message: message.into() }
    }
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code.as_str())
    }
}

impl std::error::Error for ProtoError {}

fn one() -> u32 {
    1
}

/// Client to instance.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// First message. `v` is the newest version the client speaks, `min_v` the oldest. `offered` is what it can use, `needed` what it cannot
    /// work without.
    Hello {
        v: u32,
        #[serde(default = "one")]
        min_v: u32,
        #[serde(default)]
        offered: Vec<String>,
        #[serde(default)]
        needed: Vec<String>,
        #[serde(default)]
        client: String,
    },
    /// Redeem a one-time token with the device's Ed25519 public key (64 hex digits) and a name for the device list.
    Enrol { token: String, public: String, name: String },
    /// Answer the challenge: an `IYAKKI-SIG` header value signed over the challenge (method GET, path [`WS_PATH`], body the challenge text).
    Auth { authorization: String },
    /// The person's own sign-in. Needed for anything that belongs to a person.
    SignIn { user: String, password: String },
    /// Use a capability of the catalogue; the broker decides and the access log shows device and person.
    Call {
        id: u64,
        capability: String,
        #[serde(default)]
        unit: Option<String>,
    },
    /// One input message of the screen stream (the JSON the browser page sends: key, mouse, size ...), passed through as text.
    Input { msg: String },
    /// Open the picture stream; `deflate` says the client can inflate deflate-raw tiles.
    PictureOpen {
        #[serde(default)]
        deflate: bool,
    },
    /// The client has drawn the picture message with this number.
    Ack { seq: u32 },
    /// Continue the event stream after this sequence number.
    Resume { last_seq: u64 },
    Ping { ts: u64 },
    Bye,
    /// Ask for the person's sound on this client. `side` is `client` (the default; the machine's speakers stay quiet) or `both`.
    AudioOpen {
        #[serde(default)]
        side: Option<String>,
    },
    /// Stop the sound on this client; the machine's speakers come back.
    AudioClose,
    /// Put text on the machine's clipboard. `confirmed` must be true: it says the person pressed the button for this one time.
    ClipboardPut {
        text: String,
        #[serde(default)]
        confirmed: bool,
    },
    /// Ask for the machine's clipboard text. `confirmed` as above.
    ClipboardGet {
        #[serde(default)]
        confirmed: bool,
    },
    /// Which cameras of the instance this person may watch (answered with `camera_list`, names only, never device paths).
    CameraList,
    /// Watch a camera of the instance (`unit` is its id, for example `usb:video0`); frames come as the camera stream.
    CameraOpen { unit: String },
    CameraClose { unit: String },
    /// Offer this client's own `camera` or `microphone` to the instance as a virtual source named after the device.
    OfferOpen { kind: String },
    OfferClose { kind: String },
    /// Open an SDK app of the instance in display-list mode (needs `display.list`): `app` is its id (`sample`, `markdown`), `w` and `h` the
    /// size of the client's pane in logical units. Answered with `app_list` (the first frame) or `app_err`. One app per session.
    AppOpen { app: String, w: u32, h: u32 },
    /// Input lines for the open app (the SDK's text input lines, one per line: pointer, key, text, action, wheel, resize, tick). Answered with
    /// the next `app_list`. A bad batch applies nothing and is answered with `app_err`.
    AppInput { lines: String },
    /// Close the app; the picture of the machine's screen carries on.
    AppClose,
    /// List a folder of the person's home (needs `files.transfer`; the machine asks on its screen). Answered with `file_list` ([`FileMsg`]).
    FileList { dir: String },
    /// Download one file of the person's home. The machine asks on its screen; then `file_get_ok`, chunks on [`STREAM_FILE_OUT`], `file_done`.
    FileGet { id: u32, path: String },
    /// Upload one file of `size` bytes into the person's home. The machine asks on its screen; `file_put_ok` says to send the chunks on
    /// [`STREAM_FILE_IN`]; `file_end` closes it and the file is saved (`file_saved`).
    FilePut { id: u32, path: String, size: u64 },
    /// All the chunks of upload `id` were sent.
    FileEnd { id: u32 },
    /// Stop transfer `id` in either direction; what was received is thrown away.
    FileCancel { id: u32 },
    /// A message of a type this release does not know (a newer client). The instance answers `unsupported` and goes on; it is never sent.
    #[serde(other)]
    Unknown,
}

impl fmt::Debug for ClientMsg {
    // a debug print never shows a token, a password or a signature
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientMsg::Hello { v, .. } => write!(f, "Hello(v{v})"),
            ClientMsg::Enrol { name, .. } => write!(f, "Enrol({name}, <token>)"),
            ClientMsg::Auth { .. } => write!(f, "Auth(<signature>)"),
            ClientMsg::SignIn { user, .. } => write!(f, "SignIn({user}, <password>)"),
            ClientMsg::Call { id, capability, .. } => write!(f, "Call({id}, {capability})"),
            ClientMsg::Input { .. } => write!(f, "Input"),
            ClientMsg::PictureOpen { .. } => write!(f, "PictureOpen"),
            ClientMsg::Ack { seq } => write!(f, "Ack({seq})"),
            ClientMsg::Resume { last_seq } => write!(f, "Resume({last_seq})"),
            ClientMsg::Ping { ts } => write!(f, "Ping({ts})"),
            ClientMsg::Bye => write!(f, "Bye"),
            ClientMsg::AudioOpen { .. } => write!(f, "AudioOpen"),
            ClientMsg::AudioClose => write!(f, "AudioClose"),
            // a clipboard may hold a password: never printed
            ClientMsg::ClipboardPut { text, .. } => write!(f, "ClipboardPut({} bytes)", text.len()),
            ClientMsg::ClipboardGet { .. } => write!(f, "ClipboardGet"),
            ClientMsg::CameraList => write!(f, "CameraList"),
            ClientMsg::CameraOpen { .. } => write!(f, "CameraOpen"),
            ClientMsg::CameraClose { .. } => write!(f, "CameraClose"),
            ClientMsg::OfferOpen { kind } => write!(f, "OfferOpen({kind})"),
            ClientMsg::OfferClose { kind } => write!(f, "OfferClose({kind})"),
            // typed text may be a password: the lines are never printed
            ClientMsg::AppOpen { app, w, h } => write!(f, "AppOpen({app}, {w}x{h})"),
            ClientMsg::AppInput { lines } => write!(f, "AppInput({} bytes)", lines.len()),
            ClientMsg::AppClose => write!(f, "AppClose"),
            // a path may name a person's private file: only its length is printed
            ClientMsg::FileList { dir } => write!(f, "FileList({} chars)", dir.len()),
            ClientMsg::FileGet { id, .. } => write!(f, "FileGet({id})"),
            ClientMsg::FilePut { id, size, .. } => write!(f, "FilePut({id}, {size} bytes)"),
            ClientMsg::FileEnd { id } => write!(f, "FileEnd({id})"),
            ClientMsg::FileCancel { id } => write!(f, "FileCancel({id})"),
            ClientMsg::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Instance to client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    HelloOk {
        v: u32,
        caps: Vec<String>,
        challenge: String,
        tile: u32,
        max_message: usize,
        /// Names this instance has deprecated (charter section 6); absent from older instances and from instances with none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        deprecations: Vec<String>,
    },
    /// The token was redeemed: the device id to sign with from now on, and a fresh challenge.
    Enrolled { device: String, challenge: String },
    Authed { device: String, name: String },
    SignedIn { person: String },
    /// The call was allowed; `detail` is what the capability returned (may be empty).
    CallOk { id: u64, detail: String },
    CallErr { id: u64, code: String, message: String },
    /// Server events with increasing sequence numbers (not necessarily contiguous: other people's events are not shown to this one).
    Event { seq: u64, topic: String, body: String },
    ResumeOk { from: u64, count: u64 },
    /// The wanted events are no longer kept: start again from the current state. `oldest` is the first sequence still kept.
    ResumeGap { oldest: u64 },
    PictureOk,
    Pong { ts: u64 },
    Error { code: ErrorCode, message: String },
    /// The instance ends the session (for example `revoked`).
    Bye { reason: String },
    /// Sound will come as the audio stream: `rate` Hz, `channels`, `chunk_ms` per chunk, `side` as granted.
    AudioOk { rate: u32, channels: u8, chunk_ms: u32, side: String },
    /// The sound stream ended (`closed`, `side_changed`, `machine_stopped`). Nothing is queued after this.
    AudioClosed { reason: String },
    /// Throw away the sound you hold: the sound stopped or paused on the instance, so what is queued must not play on.
    AudioFlush,
    /// The machine's clipboard text.
    ClipboardText { text: String },
    ClipboardOk,
    /// The cameras this person may watch from here: `unit` is what `camera_open` takes, `name` is what to show.
    CameraList { cameras: Vec<CameraEntry> },
    /// Frames of `unit` will come as the camera stream.
    CameraOk { unit: String, fps: u32 },
    CameraClosed { unit: String, reason: String },
    /// The offer was accepted: the instance calls it `source`. Send the stream of that kind now.
    OfferOk { kind: String, source: String },
    OfferClosed { kind: String, reason: String },
    /// A media request was refused. `what` is `audio`, `clipboard`, `camera` or `offer`; `code` is stable (the broker's denial codes,
    /// `in_use`, `consent_required`, `too_big`, `empty`, `unsupported`, `not_open`, `bad_request`).
    MediaErr { what: String, code: String, message: String },
    /// A message of a type this release does not know (a newer instance). The client ignores it and goes on; it is never sent.
    #[serde(other)]
    Unknown,
}

/// One camera in the list a client is offered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraEntry {
    pub unit: String,
    pub name: String,
}

/// Instance to client, display-list mode (D-CLIENT-PROTO-DISPLAYLIST, ADDITIVE). A text message like a [`ServerMsg`] (`{"t":"app_list",...}`), kept in
/// its own type so that [`ServerMsg`] and the client engine's reader of it are unchanged: the engine reads these by hand ([`decode_app`]), which
/// keeps its WebAssembly module inside its size limit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AppMsg {
    /// A frame of the open app as display-list text (the first after `app_open`, then one after every `app_input`).
    AppList { app: String, text: String },
    /// The app ended: `closed` (the client asked), `left` (the person left it with Back), `too_big`, `signed_out`.
    AppClosed { app: String, reason: String },
    /// An app request was refused. `code` is stable: `unsupported`, `not_found`, `denied`, `busy`, `bad_request`, `not_open`, `too_big`, `failed`.
    AppErr { code: String, message: String },
}

/// Instance to client, file transfer (F-CLIENT-01, ADDITIVE; like [`AppMsg`] it is not a [`ServerMsg`]: the page reads the JSON itself and the
/// engine only passes `{"t":"file_..."}` texts on).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FileMsg {
    /// The folder `dir` (relative to the home): `(name, is_dir, size)`.
    FileList { dir: String, entries: Vec<FileRow>, truncated: bool },
    /// The download `id` starts: `size` bytes follow on [`STREAM_FILE_OUT`].
    FileGetOk { id: u32, size: u64 },
    /// The download `id` is complete.
    FileDone { id: u32 },
    /// Send the chunks of upload `id` now.
    FilePutOk { id: u32 },
    /// The upload `id` is saved as `path`.
    FileSaved { id: u32, path: String },
    /// A file request was refused or failed. `id` is 0 for a listing. `code` is stable: `unsupported`, `capability_missing`, `role_denied`, the broker's denial
    /// codes, `bad_request`, `too_big`, `not_found`, `busy`, `failed`.
    FileErr { id: u32, code: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileRow {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

pub fn encode_file(m: &FileMsg) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

/// A binary file chunk: the id and the bytes.
pub fn file_chunk(stream: u8, id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes.len() + 5);
    v.push(stream);
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(bytes);
    v
}

/// The id and the bytes of a file chunk's payload (what follows the stream tag); `None` when it is shorter than the id.
pub fn split_file_chunk(payload: &[u8]) -> Option<(u32, &[u8])> {
    let (id, bytes) = payload.split_at_checked(4)?;
    Some((u32::from_be_bytes([id[0], id[1], id[2], id[3]]), bytes))
}

pub fn encode_app(m: &AppMsg) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

/// Is this text one of the display-list messages, and if so is it within its bound? `Ok(false)` for a text that is not one (it is for
/// [`decode_server`]); an `app_list` may be as long as [`MAX_APP_LIST`], the others are held to [`MAX_TEXT`].
pub fn app_text(text: &str) -> Result<bool, ProtoError> {
    let list = text.starts_with(r#"{"t":"app_list""#);
    if text.starts_with(r#"{"t":"file_"#) {
        // a file message: the page reads it; here it is only held to the size of an ordinary message
        return if text.len() > MAX_TEXT { Err(ProtoError::new(ErrorCode::TooBig, "that message is too long")) } else { Ok(true) };
    }
    if !list && !text.starts_with(r#"{"t":"app_"#) {
        return Ok(false);
    }
    if text.len() > if list { MAX_APP_LIST + 256 } else { MAX_TEXT } {
        return Err(ProtoError::new(ErrorCode::TooBig, "that message is too long"));
    }
    Ok(true)
}

/// The display-list message in an already parsed JSON value. The client engine parses with its own copy of the JSON reader and calls this
/// (reading it with a derived `Deserialize` here would add a second copy of the reader to its module, which has a size limit).
pub fn app_from_value(v: &serde_json::Value) -> Result<AppMsg, ProtoError> {
    // a file message is the page's to read: it is checked to be a message with a tag, nothing more
    if v.get("t").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with("file_")) {
        return Ok(AppMsg::AppClosed { app: String::new(), reason: String::new() });
    }
    let bad = || ProtoError::new(ErrorCode::BadMessage, "that message is not understood");
    let f = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    match v.get("t").and_then(|t| t.as_str()) {
        Some("app_list") => Ok(AppMsg::AppList { app: f("app").ok_or_else(bad)?, text: f("text").ok_or_else(bad)? }),
        Some("app_closed") => Ok(AppMsg::AppClosed { app: f("app").ok_or_else(bad)?, reason: f("reason").ok_or_else(bad)? }),
        Some("app_err") => Ok(AppMsg::AppErr { code: f("code").ok_or_else(bad)?, message: f("message").ok_or_else(bad)? }),
        _ => Err(bad()),
    }
}

/// Read one display-list message of the instance (for a client that is not the engine, and for tests): `Ok(None)` for a text that is not one.
pub fn decode_app(text: &str) -> Result<Option<AppMsg>, ProtoError> {
    if !app_text(text)? {
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| ProtoError::new(ErrorCode::BadMessage, "that message is not understood"))?;
    app_from_value(&v).map(Some)
}

impl From<ProtoError> for ServerMsg {
    fn from(e: ProtoError) -> ServerMsg {
        ServerMsg::Error { code: e.code, message: e.message }
    }
}

/// Read one text message from the client.
pub fn decode_client(text: &str) -> Result<ClientMsg, ProtoError> {
    if text.len() > MAX_TEXT && !(text.len() <= MAX_CLIPBOARD_MESSAGE && text.starts_with(r#"{"t":"clipboard_put""#)) {
        return Err(ProtoError::new(ErrorCode::TooBig, "that message is too long"));
    }
    serde_json::from_str(text).map_err(|_| ProtoError::new(ErrorCode::BadMessage, "that message is not understood"))
}

pub fn encode_client(m: &ClientMsg) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

pub fn encode_server(m: &ServerMsg) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

/// Read one text message from the instance (the client side).
pub fn decode_server(text: &str) -> Result<ServerMsg, ProtoError> {
    if text.len() > MAX_TEXT && !(text.len() <= MAX_CLIPBOARD_MESSAGE && text.starts_with(r#"{"t":"clipboard_text""#)) {
        return Err(ProtoError::new(ErrorCode::TooBig, "that message is too long"));
    }
    serde_json::from_str(text).map_err(|_| ProtoError::new(ErrorCode::BadMessage, "that message is not understood"))
}

/// Settle version and capabilities. Returns the capabilities both sides use (the client's `offered` that the instance has).
pub fn negotiate(v: u32, min_v: u32, offered: &[String], needed: &[String], server: &[&str]) -> Result<Vec<String>, ProtoError> {
    if min_v > v || min_v > PROTOCOL_VERSION || v < MIN_PROTOCOL_VERSION {
        // the words say which side is older and must be updated (charter section 5); the code stays `version_mismatch`
        let who = if min_v > v { "the client sent a version range that makes no sense" } else if min_v > PROTOCOL_VERSION { "update this IYAKKI instance" } else { "update the client" };
        return Err(ProtoError::new(ErrorCode::VersionMismatch, format!("this instance speaks protocol {MIN_PROTOCOL_VERSION} to {PROTOCOL_VERSION}; the client speaks {min_v} to {v}: {who}")));
    }
    let missing: Vec<&str> = needed.iter().map(String::as_str).filter(|n| !server.contains(n)).collect();
    if !missing.is_empty() {
        return Err(ProtoError::new(ErrorCode::CapabilityMissing, format!("this instance cannot offer: {}", missing.join(", "))));
    }
    let mut agreed: Vec<String> = Vec::new();
    for c in offered.iter().chain(needed.iter()) {
        if server.contains(&c.as_str()) && !agreed.contains(c) {
            agreed.push(c.clone());
        }
    }
    Ok(agreed)
}

/// One event kept for resuming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub seq: u64,
    /// Who may see it: a person's name, or empty for every signed-in person.
    pub audience: String,
    pub topic: String,
    pub body: String,
}

/// What a resume found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumed {
    Events(Vec<Kept>),
    Gap { oldest: u64 },
}

/// The latest events, bounded, with increasing sequence numbers starting at 1.
pub struct EventRing {
    cap: usize,
    next: u64,
    items: VecDeque<Kept>,
}

impl EventRing {
    pub fn new(cap: usize) -> EventRing {
        EventRing { cap: cap.max(1), next: 1, items: VecDeque::new() }
    }

    /// The sequence number of the newest event (0: none yet).
    pub fn latest(&self) -> u64 {
        self.next - 1
    }

    pub fn push(&mut self, audience: &str, topic: &str, body: &str) -> u64 {
        let seq = self.next;
        self.next += 1;
        self.items.push_back(Kept { seq, audience: audience.into(), topic: topic.into(), body: body.into() });
        while self.items.len() > self.cap {
            self.items.pop_front();
        }
        seq
    }

    /// The events after `last_seq` that `person` may see. A client that is ahead of this ring (the instance restarted) or behind what
    /// is kept gets a gap, never a silent hole.
    pub fn since(&self, last_seq: u64, person: &str) -> Resumed {
        let oldest = self.items.front().map_or(self.next, |k| k.seq);
        if last_seq > self.latest() || last_seq.saturating_add(1) < oldest {
            return Resumed::Gap { oldest };
        }
        Resumed::Events(self.items.iter().filter(|k| k.seq > last_seq && (k.audience.is_empty() || k.audience == person)).cloned().collect())
    }
}

#[cfg(test)]
mod tests;
