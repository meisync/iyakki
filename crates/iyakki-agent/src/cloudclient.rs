//! The desktop connector's client (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3.4): the local IYAKKI signs in to a tenant's web
//! desktop (`iyakki serve --web`, `iyakki-remote`) over HTTPS and shows its picture inside the local shell, sending keys and mouse back.
//! It speaks the pixel-stream protocol of `docs/REMOTE-PROTOCOL.md`: the same one the browser page speaks, so the server needs no change.
//!
//! THE API BELOW IS THE CONTRACT between the client (this file's body) and the shell (`iyakki-shell` cloud sessions). The signatures may be
//! extended; do not remove or rename without telling the other side.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::pixelcodec::{decode_frame, Canvas};

/// The SHA-256 of a server's certificate (the whole DER): what a tenant is pinned by, like a paired device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin(pub [u8; 32]);

impl Pin {
    /// Lower-case hex, 64 characters.
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
    /// Groups for a person to compare by eye: `AB:CD:...`.
    pub fn display(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
    }
    pub fn parse(s: &str) -> Option<Pin> {
        let h: String = s.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect();
        if h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Pin(out))
    }
}

/// Where a tenant is and what its certificate must be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// `host:port` (the port defaults to 443 when the person gives none; the caller normalises).
    pub addr: String,
    pub pin: Pin,
}

/// What a server showed when first asked: the person decides whether to trust it (trust on first use), like pairing a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presented {
    pub pin: Pin,
    /// The certificate's first name, for the dialog.
    pub subject: String,
    /// Unix seconds.
    pub not_after: i64,
}

/// Ask `addr` (`host:port`) for its certificate without trusting it and without sending anything secret: the TLS handshake is made
/// (with every signature checked, but any certificate accepted and remembered) and the connection is ended before a byte of
/// application data goes out.
pub fn probe(addr: &str, timeout: Duration) -> Result<Presented, String> {
    let (mut tls, der) = tls_open(addr, None, timeout).map_err(|e| e.text())?;
    tls.conn.send_close_notify();
    let _ = tls.flush();
    let der = der.ok_or("the server showed no certificate")?;
    let info = crate::cloudroute::read_cert(&der);
    let subject = info.as_ref().and_then(|i| i.names.first().cloned()).unwrap_or_else(|| split_addr(addr).map(|(h, _)| h).unwrap_or_default());
    Ok(Presented { pin: Pin(Sha256::digest(&der).into()), subject, not_after: info.map_or(0, |i| i.not_after) })
}

/// The state of the link. `Refused` and `Closed` are final (the threads have ended); the others change on their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    /// Signed in and receiving pictures.
    Live,
    /// The link dropped; the client is trying again (the session on the server is kept for a while). After 2 minutes without success
    /// it becomes `Closed`.
    Reconnecting,
    /// The server refused the account or password. Final.
    Refused(String),
    /// Ended, with the reason (the person closed it, the server went away for good, the person signed out inside the tenant's desktop,
    /// the pin did not match: its text then says the certificate changed).
    Closed(String),
}

/// What the person does in the tenant's window; the client turns it into the protocol's messages (`iyakki-remote/src/input.rs` is the
/// reader on the other side).
///
/// Coordinates are in the tenant's picture, in pixels (the picture the shell draws is exactly that size, so the shell scales its
/// window position into it). `button` follows the browser: 0 left, 1 middle, 2 right. The server knows a left press and release and a
/// right press (which opens the context menu, there is no right release); the middle button is ignored. `Wheel`'s `dy` is the browser's
/// `deltaY` (positive scrolls down, about 40 per notch; the server scrolls 1 to 10 steps) and `dx` is not used (the server has no
/// sideways scroll); the pointer is moved to `x`,`y` first. `Key` names are the browser's `key` values (`Enter`, `Backspace`,
/// `ArrowUp`, `F5`, a single character such as `a`); `Text` is typed one character at a time (control characters are dropped, at most
/// 4000 characters); `Paste` is at most 200,000 characters. Input sent while the link is down is dropped, not replayed late.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    MouseMove { x: i32, y: i32 },
    MouseDown { x: i32, y: i32, button: u8 },
    MouseUp { x: i32, y: i32, button: u8 },
    Wheel { x: i32, y: i32, dx: i32, dy: i32 },
    /// A named key (`Enter`, `Backspace`, `ArrowUp`, `F5`, ...) with its modifiers; printable text goes as `Text`.
    Key { name: String, ctrl: bool, shift: bool, alt: bool },
    Text(String),
    Paste(String),
    /// The size of the window the picture is shown in (the server keeps it between 640x400 and 3840x2160 and draws its picture at
    /// that size, so the picture's size follows a moment later).
    Size { w: u32, h: u32 },
    Shift(bool),
}

/// The whole picture as RGBA, with a number that changes whenever any part of it does. The size is the tenant's screen size (a picture
/// the server halved to save bandwidth is stretched back here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub generation: u64,
}

// ------------------------------------------------------------------------------------------------------------------ the client

/// The sign-in cookie's name (`iyakki_session`, as `iyakki-remote` sets it).
const COOKIE: &str = "iyakki_session";
/// The biggest WebSocket message accepted from the server (it sends at most 512 KiB itself).
const MAX_MESSAGE: usize = 8 << 20;
/// A server picture bigger than this many pixels is refused (the server's own limit is 3840x2160).
const MAX_PIXELS: usize = 4096 * 4096;
const MAX_QUEUE: usize = 20_000;
const IO_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the link may be down (reconnecting) before the client gives up.
const GIVE_UP: Duration = Duration::from_secs(120);
/// No message at all (the server sends statistics every second) for this long means the link is dead.
const SILENT: Duration = Duration::from_secs(20);

type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// A live connection to one tenant. Cheap to poll from the shell's frame loop: nothing here blocks the caller.
///
/// `connect` signs in on the caller's thread and then one thread of its own carries the WebSocket, decodes pictures, sends the queued
/// input and reconnects (with a growing wait of 0.5 s to 10 s) when the link drops. It ends on `close()`, on drop, when the server says
/// the person signed out, when the pin does not match, when the password is refused, or after 2 minutes of failed reconnects.
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    target: Target,
    user: String,
    password: Zeroizing<String>,
    status: Mutex<Status>,
    shown: Mutex<Option<Shown>>,
    generation: AtomicU64,
    out: Mutex<VecDeque<String>>,
    size: Mutex<(u32, u32)>,
    cursor: Mutex<Option<String>>,
    copied: Mutex<VecDeque<String>>,
    clip: AtomicBool,
    stop: AtomicBool,
    finished: AtomicBool,
}

/// The decoded picture, at the size the server coded it (a halved picture is stretched when it is asked for).
struct Shown {
    canvas: Canvas,
    screen_w: usize,
    screen_h: usize,
}

impl Inner {
    /// Final states stay: nothing overrides `Refused` or `Closed`.
    fn set_status(&self, s: Status) {
        if let Ok(mut g) = self.status.lock() {
            if !matches!(*g, Status::Refused(_) | Status::Closed(_)) {
                log::debug!("desktop connector: {s:?}");
                *g = s;
            }
        }
    }

    fn push_out(&self, msgs: Vec<String>) {
        let Ok(mut q) = self.out.lock() else { return };
        for m in msgs {
            let is_move = m.starts_with(r#"{"t":"move""#);
            if is_move && q.back().is_some_and(|l| l.starts_with(r#"{"t":"move""#)) {
                q.pop_back(); // only the newest pointer position matters
            }
            if q.len() < MAX_QUEUE {
                q.push_back(m);
            }
        }
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

impl Client {
    /// Sign in as `user` and start receiving pictures for a window of `size`. Blocks for the sign-in only (a few seconds at most), then the
    /// connection runs in its own threads.
    ///
    /// Returns `Err` when the tenant cannot be reached or does not take the sign-in for another reason (the text says why: no route,
    /// timeout, too many tries, too many people signed in). It returns `Ok` with a client that is already final when the answer is
    /// a definite no: `Status::Refused` for a wrong account or password, `Status::Closed` (no retry) for a certificate that is not the
    /// pinned one (the password was not sent) or a server too old to speak protocol 2. `size` is clamped by the server (see
    /// `Input::Size`).
    pub fn connect(t: &Target, user: &str, password: &str, size: (u32, u32)) -> Result<Client, String> {
        let inner = Arc::new(Inner {
            target: t.clone(),
            user: user.trim().to_string(),
            password: Zeroizing::new(password.to_string()),
            status: Mutex::new(Status::Connecting),
            shown: Mutex::new(None),
            generation: AtomicU64::new(0),
            out: Mutex::new(VecDeque::new()),
            size: Mutex::new(size),
            cursor: Mutex::new(None),
            copied: Mutex::new(VecDeque::new()),
            clip: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        });
        let mut session = None;
        match establish(&inner, &mut session) {
            Ok(first) => {
                let i = inner.clone();
                let spawned = std::thread::Builder::new().name("iyakki-cloud-client".into()).spawn(move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| supervise(&i, session, Some(first))));
                    if r.is_err() {
                        i.set_status(Status::Closed("the desktop connector failed inside".into()));
                    }
                    i.finished.store(true, Ordering::Relaxed);
                });
                if let Err(e) = spawned {
                    return Err(format!("cannot start the connection: {e}"));
                }
            }
            Err(ConnErr::Refused(m)) => {
                inner.set_status(Status::Refused(m));
                inner.finished.store(true, Ordering::Relaxed);
            }
            Err(e @ (ConnErr::Mismatch(_) | ConnErr::Fatal(_))) => {
                inner.set_status(Status::Closed(e.text()));
                inner.finished.store(true, Ordering::Relaxed);
            }
            Err(e) => return Err(e.text()),
        }
        Ok(Client { inner })
    }

    pub fn status(&self) -> Status {
        self.inner.status.lock().map(|s| s.clone()).unwrap_or(Status::Closed("internal error".into()))
    }

    /// The number of the latest picture: compare with the last one drawn to know whether to redraw.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Relaxed)
    }

    /// The latest picture, if any has arrived (it stays while the link is down and reconnecting).
    pub fn picture(&self) -> Option<Picture> {
        let g = self.inner.shown.lock().ok()?;
        let s = g.as_ref()?;
        let generation = self.inner.generation.load(Ordering::Relaxed);
        let (w, h) = (s.screen_w, s.screen_h);
        let rgba = if (s.canvas.w, s.canvas.h) == (w, h) { s.canvas.rgba.clone() } else { stretch(&s.canvas, w, h) };
        Some(Picture { w: w as u32, h: h as u32, rgba, generation })
    }

    pub fn send(&self, i: Input) {
        if let Input::Size { w, h } = &i {
            if let Ok(mut s) = self.inner.size.lock() {
                *s = (*w, *h);
            }
        }
        if self.inner.finished.load(Ordering::Relaxed) {
            return;
        }
        let m = messages(&i);
        self.inner.push_out(m);
    }

    /// The pointer shape the tenant's desktop wants, as a CSS cursor name (`default`, `pointer`, `text`, ...), if it said.
    pub fn cursor(&self) -> Option<String> {
        self.inner.cursor.lock().ok()?.clone()
    }

    /// Text the person copied inside the tenant's desktop, oldest first: for the local clipboard.
    pub fn take_copy(&self) -> Option<String> {
        self.inner.copied.lock().ok()?.pop_front()
    }

    /// True once when the tenant's desktop wants the local clipboard's text: answer with `Input::Paste`.
    pub fn take_clip_request(&self) -> bool {
        self.inner.clip.swap(false, Ordering::Relaxed)
    }

    /// True once the connection's thread has ended (after `close()`, or when the status became final).
    pub fn finished(&self) -> bool {
        self.inner.finished.load(Ordering::Relaxed)
    }

    /// Sign out of the tenant and end the connection. Returns at once; the thread signs out and ends within a few seconds
    /// (`finished()` tells when).
    pub fn close(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.set_status(Status::Closed("closed".into()));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------------------------- messages

/// The JSON messages the page would send for one input (`iyakki-remote/src/input.rs` reads them).
pub fn messages(i: &Input) -> Vec<String> {
    let m = |v: Value| v.to_string();
    match i {
        Input::MouseMove { x, y } => vec![m(json!({"t": "move", "x": x, "y": y}))],
        Input::MouseDown { x, y, button } => match button {
            0 => vec![m(json!({"t": "down", "x": x, "y": y}))],
            2 => vec![m(json!({"t": "right", "x": x, "y": y}))],
            _ => vec![],
        },
        Input::MouseUp { x, y, button } => match button {
            0 => vec![m(json!({"t": "up", "x": x, "y": y}))],
            _ => vec![],
        },
        Input::Wheel { x, y, dy, .. } => {
            if *dy == 0 {
                return vec![];
            }
            let steps = ((dy.unsigned_abs() as f64 / 40.0).round() as i32).clamp(1, 10);
            let d = if *dy > 0 { -steps } else { steps };
            vec![m(json!({"t": "move", "x": x, "y": y})), m(json!({"t": "wheel", "d": d}))]
        }
        Input::Key { name, ctrl, shift, alt } => {
            if name.is_empty() || name.len() > 32 {
                return vec![];
            }
            vec![m(json!({"t": "key", "key": name, "ctrl": ctrl, "shift": shift, "alt": alt}))]
        }
        Input::Text(s) => s.chars().filter(|c| !c.is_control()).take(4000).map(|c| m(json!({"t": "key", "key": c.to_string()}))).collect(),
        Input::Paste(s) => {
            let t: String = s.chars().take(200_000).collect();
            vec![m(json!({"t": "paste", "s": t}))]
        }
        Input::Size { w, h } => vec![m(json!({"t": "size", "w": w, "h": h}))],
        Input::Shift(on) => vec![m(json!({"t": "shift", "on": on}))],
    }
}

/// Stretch a picture the server halved (or quartered) back to the screen's size.
fn stretch(c: &Canvas, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    if c.w == 0 || c.h == 0 {
        return out;
    }
    for y in 0..h {
        let sy = (y * c.h / h).min(c.h - 1);
        for x in 0..w {
            let sx = (x * c.w / w).min(c.w - 1);
            let (s, d) = ((sy * c.w + sx) * 4, (y * w + x) * 4);
            out[d..d + 4].copy_from_slice(&c.rgba[s..s + 4]);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------------------------- TLS

#[derive(Debug)]
enum ConnErr {
    /// The certificate is not the pinned one (the pin it showed).
    Mismatch(String),
    /// The account or password was refused.
    Refused(String),
    /// Will not work again however often it is tried.
    Fatal(String),
    /// Worth another try.
    Transient(String),
    /// The server answered with this status.
    Http(u16, String),
}

impl ConnErr {
    fn text(&self) -> String {
        match self {
            ConnErr::Mismatch(m) | ConnErr::Refused(m) | ConnErr::Fatal(m) | ConnErr::Transient(m) | ConnErr::Http(_, m) => m.clone(),
        }
    }
}

/// Accepts the certificate whose SHA-256 is `pin` (or any, when there is no pin, to look at it) and remembers what it was shown. The
/// signatures of the handshake are still checked with the crypto provider: a pinned certificate is not enough without its key.
#[derive(Debug)]
struct Verifier {
    pin: Option<Pin>,
    seen: Mutex<Option<Vec<u8>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(end_entity.as_ref().to_vec());
        }
        match self.pin {
            Some(p) if Pin(Sha256::digest(end_entity.as_ref()).into()) != p => Err(rustls::Error::General("certificate does not match the pin".into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// `host` and port of `host:port`, `[v6]:port` or a bare host (port 443).
fn split_addr(addr: &str) -> Result<(String, u16), String> {
    let a = addr.trim();
    if a.is_empty() || a.len() > 300 || a.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
        return Err(format!("{a:?} is not an address"));
    }
    if let Some(rest) = a.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or("an address in [ ] needs a closing ]")?;
        return match tail.strip_prefix(':') {
            Some(p) => Ok((h.to_string(), p.parse().map_err(|_| format!("{p:?} is not a port"))?)),
            None if tail.is_empty() => Ok((h.to_string(), 443)),
            None => Err(format!("{a:?} is not an address")),
        };
    }
    match a.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Ok((h.to_string(), p.parse().map_err(|_| format!("{p:?} is not a port"))?)),
        _ => Ok((a.to_string(), 443)),
    }
}

/// TCP and the TLS handshake to `addr`; nothing but the handshake is sent. With a pin, any other certificate ends in
/// `ConnErr::Mismatch` before a byte of application data. Also returns the certificate that was shown.
fn tls_open(addr: &str, pin: Option<Pin>, timeout: Duration) -> Result<(Tls, Option<Vec<u8>>), ConnErr> {
    let (host, port) = split_addr(addr).map_err(ConnErr::Fatal)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Verifier { pin, seen: Mutex::new(None), provider: provider.clone() });
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnErr::Fatal(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).or_else(|_| ServerName::try_from("iyakki-tenant".to_string())).map_err(|e| ConnErr::Fatal(e.to_string()))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| ConnErr::Fatal(e.to_string()))?;
    let addrs = (host.as_str(), port).to_socket_addrs().map_err(|e| ConnErr::Transient(format!("{host}: {e}")))?;
    let mut sock = None;
    let mut last = format!("{host}: no address");
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = format!("cannot reach {host}:{port}: {e}"),
        }
    }
    let mut sock = sock.ok_or(ConnErr::Transient(last))?;
    let _ = sock.set_nodelay(true);
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let mut result = Ok(());
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut sock) {
            result = Err(e);
            break;
        }
    }
    let seen = verifier.seen.lock().ok().and_then(|s| s.clone());
    if let Err(e) = result {
        if let (Some(p), Some(der)) = (pin, &seen) {
            let shown = Pin(Sha256::digest(der).into());
            if shown != p {
                return Err(ConnErr::Mismatch(format!("the certificate of {host}:{port} is not the one that was trusted (it shows {}, the trusted one is {}): the tenant's certificate changed, or this is not the tenant", shown.display(), p.display())));
            }
        }
        return Err(ConnErr::Transient(format!("secure connection to {host}:{port} failed: {e}")));
    }
    Ok((rustls::StreamOwned::new(conn, sock), seen))
}

// ------------------------------------------------------------------------------------------------------------------- HTTP

/// A server's sentence made safe to show: printable, short.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(160).collect()
}

/// Read the status line and headers (names lower-cased); returns what came after them too.
fn read_head(tls: &mut Tls) -> Result<(u16, Vec<(String, String)>, Vec<u8>), ConnErr> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let at = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 16 * 1024 {
            return Err(ConnErr::Transient("the server's answer is malformed".into()));
        }
        match tls.read(&mut chunk) {
            Ok(0) => return Err(ConnErr::Transient("the server closed the connection".into())),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(ConnErr::Transient(format!("reading from the server: {e}"))),
        }
    };
    let head = String::from_utf8_lossy(&buf[..at]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse::<u16>().ok()).ok_or(ConnErr::Transient("the server's answer is malformed".into()))?;
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Ok((status, headers, buf[at + 4..].to_vec()))
}

/// One HTTP request on a fresh connection; returns status, headers and body.
fn http(tls: &mut Tls, host: &str, method: &str, path: &str, extra: &[(&str, &str)], body: &[u8]) -> Result<(u16, Vec<(String, String)>, Vec<u8>), ConnErr> {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut out = req.into_bytes();
    out.extend_from_slice(body);
    tls.write_all(&out).and_then(|_| tls.flush()).map_err(|e| ConnErr::Transient(format!("sending to the server: {e}")))?;
    let (status, headers, mut body) = read_head(tls)?;
    let want = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0).min(1 << 20);
    let mut chunk = [0u8; 4096];
    while body.len() < want {
        match tls.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    Ok((status, headers, body))
}

fn error_text(body: &[u8], status: u16) -> String {
    serde_json::from_slice::<Value>(body).ok().and_then(|v| v["error"].as_str().map(clean)).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("the server answered {status}"))
}

/// The cookie and the CSRF token of a signed-in session. Secret: never logged, never in an error text.
struct Session {
    cookie: String,
    csrf: String,
}

fn login(inner: &Inner) -> Result<Session, ConnErr> {
    let (mut tls, _) = tls_open(&inner.target.addr, Some(inner.target.pin), IO_TIMEOUT)?;
    let body = json!({"user": inner.user, "password": inner.password.as_str()}).to_string();
    let (status, headers, body) = http(&mut tls, &inner.target.addr, "POST", "/api/login", &[("Content-Type", "application/json")], body.as_bytes())?;
    match status {
        200 => {
            let cookie = headers
                .iter()
                .filter(|(k, _)| k == "set-cookie")
                .find_map(|(_, v)| v.split(';').next().filter(|c| c.starts_with(&format!("{COOKIE}="))).map(str::to_string))
                .ok_or(ConnErr::Transient("the server gave no session".into()))?;
            let csrf = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["csrf"].as_str().map(str::to_string)).ok_or(ConnErr::Transient("the server gave no session".into()))?;
            Ok(Session { cookie, csrf })
        }
        401 => Err(ConnErr::Refused(error_text(&body, status))),
        s => Err(ConnErr::Http(s, error_text(&body, s))),
    }
}

/// Open the WebSocket with a session; returns the stream and what already arrived after the handshake.
fn ws_open(inner: &Inner, s: &Session) -> Result<(Tls, Vec<u8>), ConnErr> {
    let (mut tls, _) = tls_open(&inner.target.addr, Some(inner.target.pin), IO_TIMEOUT)?;
    let mut key = [0u8; 16];
    iyakki_sec::random_fill(&mut key).map_err(|_| ConnErr::Fatal("no randomness".into()))?;
    let key = crate::wsproto::base64(&key);
    let req = format!(
        "GET /ws?csrf={} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nCookie: {}\r\n\r\n",
        s.csrf, inner.target.addr, s.cookie
    );
    tls.write_all(req.as_bytes()).and_then(|_| tls.flush()).map_err(|e| ConnErr::Transient(format!("sending to the server: {e}")))?;
    let (status, headers, rest) = read_head(&mut tls)?;
    if status != 101 {
        let mut body = rest;
        let mut chunk = [0u8; 1024];
        if let Ok(n) = tls.read(&mut chunk) {
            body.extend_from_slice(&chunk[..n]);
        }
        let msg = error_text(&body, status);
        return Err(ConnErr::Http(status, msg));
    }
    let ok = headers.iter().any(|(k, v)| k == "sec-websocket-accept" && *v == crate::wsproto::accept_key(&key));
    if !ok {
        return Err(ConnErr::Transient("the server's WebSocket answer is wrong".into()));
    }
    Ok((tls, rest))
}

/// Sign in if there is no session yet, then open the WebSocket; a session the server no longer knows is replaced by a new sign-in once.
fn establish(inner: &Inner, session: &mut Option<Session>) -> Result<(Tls, Vec<u8>), ConnErr> {
    if session.is_none() {
        *session = Some(login(inner)?);
    }
    let first = ws_open(inner, session.as_ref().ok_or(ConnErr::Fatal("no session".into()))?);
    match first {
        Err(ConnErr::Http(401 | 403, _)) => {
            *session = Some(login(inner)?);
            ws_open(inner, session.as_ref().ok_or(ConnErr::Fatal("no session".into()))?)
        }
        other => other,
    }
}

fn logout(inner: &Inner, s: &Session) {
    if let Ok((mut tls, _)) = tls_open(&inner.target.addr, Some(inner.target.pin), Duration::from_secs(3)) {
        let _ = http(&mut tls, &inner.target.addr, "POST", "/api/logout", &[("Cookie", &s.cookie), ("X-Csrf", &s.csrf)], b"");
        tls.conn.send_close_notify();
        let _ = tls.flush();
    }
}

// ------------------------------------------------------------------------------------------------------------- the connection

/// Wait `d`, or less when the client is stopping.
fn nap(inner: &Inner, d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d && !inner.stopping() {
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn supervise(inner: &Inner, mut session: Option<Session>, mut next: Option<(Tls, Vec<u8>)>) {
    let mut backoff = Duration::from_millis(500);
    let mut down_since: Option<Instant> = None;
    let mut reconnect = false;
    while !inner.stopping() {
        let conn = match next.take() {
            Some(c) => Ok(c),
            None => {
                nap(inner, backoff);
                if inner.stopping() {
                    break;
                }
                establish(inner, &mut session)
            }
        };
        match conn {
            Ok((tls, left)) => {
                let started = Instant::now();
                let end = run_ws(inner, tls, left, reconnect);
                reconnect = true;
                match end {
                    End::Stop => break,
                    End::Bye => {
                        inner.set_status(Status::Closed("you signed out of the tenant's desktop".into()));
                        session = None;
                        break;
                    }
                    End::Fatal(m) => {
                        inner.set_status(Status::Closed(m));
                        break;
                    }
                    End::Dropped(why) => {
                        log::debug!("desktop connector: link dropped: {why}");
                        inner.set_status(Status::Reconnecting);
                        down_since = Some(Instant::now());
                        backoff = if started.elapsed() > Duration::from_secs(5) { Duration::from_millis(500) } else { (backoff * 2).min(Duration::from_secs(10)) };
                    }
                }
            }
            Err(e) => {
                match e {
                    ConnErr::Mismatch(_) | ConnErr::Fatal(_) => {
                        inner.set_status(Status::Closed(e.text()));
                        break;
                    }
                    ConnErr::Refused(m) => {
                        inner.set_status(Status::Refused(m));
                        break;
                    }
                    ConnErr::Transient(_) | ConnErr::Http(..) => {
                        log::debug!("desktop connector: reconnect failed: {}", e.text());
                        inner.set_status(Status::Reconnecting);
                    }
                }
                let since = *down_since.get_or_insert_with(Instant::now);
                if since.elapsed() > GIVE_UP {
                    inner.set_status(Status::Closed(format!("the tenant did not answer for {} seconds ({})", GIVE_UP.as_secs(), e.text())));
                    break;
                }
                backoff = (backoff * 2).min(Duration::from_secs(10));
            }
        }
    }
    if inner.stopping() {
        if let Some(s) = &session {
            logout(inner, s);
        }
    }
}

enum End {
    Stop,
    Bye,
    Dropped(String),
    Fatal(String),
}

/// A masked client frame.
fn masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut mask = [0u8; 4];
    if iyakki_sec::random_fill(&mut mask).is_err() {
        mask = [0x37, 0xfa, 0x21, 0x3d];
    }
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

fn text_msg(s: &str) -> Vec<u8> {
    masked_frame(0x1, s.as_bytes())
}

enum Msg {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// Reads the server's (unmasked) WebSocket frames; sizes are bounded before anything is kept.
struct FrameReader {
    buf: Vec<u8>,
    partial: Option<(bool, Vec<u8>)>,
    max: usize,
}

impl FrameReader {
    fn new(max: usize) -> FrameReader {
        FrameReader { buf: Vec::new(), partial: None, max }
    }

    fn push(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    fn next(&mut self) -> Result<Option<Msg>, &'static str> {
        loop {
            let b = &self.buf;
            if b.len() < 2 {
                return Ok(None);
            }
            let (fin, rsv, op) = (b[0] & 0x80 != 0, b[0] & 0x70, b[0] & 0x0f);
            if rsv != 0 {
                return Err("reserved bits set");
            }
            if b[1] & 0x80 != 0 {
                return Err("server frames must not be masked");
            }
            let (len, at) = match b[1] & 0x7f {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().map_err(|_| "bad length")?), 10),
                126 | 127 => return Ok(None),
                n => (n as u64, 2),
            };
            if len > self.max as u64 {
                return Err("message too large");
            }
            let len = len as usize;
            if b.len() < at + len {
                return Ok(None);
            }
            let payload = b[at..at + len].to_vec();
            self.buf.drain(..at + len);
            match op {
                0x8 => return Ok(Some(Msg::Close)),
                0x9 => return Ok(Some(Msg::Ping(payload))),
                0xA => return Ok(Some(Msg::Pong)),
                0x0..=0x2 => {
                    let (text, mut data) = match (op, self.partial.take()) {
                        (0x0, Some(p)) => p,
                        (0x0, None) => return Err("continuation without a start"),
                        (_, Some(_)) => return Err("new message inside a fragmented one"),
                        (o, None) => (o == 0x1, Vec::new()),
                    };
                    data.extend_from_slice(&payload);
                    if data.len() > self.max {
                        return Err("message too large");
                    }
                    if !fin {
                        self.partial = Some((text, data));
                        continue;
                    }
                    return Ok(Some(if text { Msg::Text(String::from_utf8(data).map_err(|_| "text is not UTF-8")?) } else { Msg::Binary(data) }));
                }
                _ => return Err("unknown opcode"),
            }
        }
    }
}

fn run_ws(inner: &Inner, mut tls: Tls, leftover: Vec<u8>, reconnect: bool) -> End {
    let _ = tls.sock.set_read_timeout(Some(Duration::from_millis(10)));
    let _ = tls.sock.set_write_timeout(Some(Duration::from_secs(5)));
    if let Ok(mut q) = inner.out.lock() {
        q.clear(); // what was typed while the link was down is stale
    }
    let mut reader = FrameReader::new(MAX_MESSAGE);
    reader.push(&leftover);
    let t0 = Instant::now();
    let ms = || t0.elapsed().as_millis() as u64;
    let mut canvas = Canvas::default();
    let mut meta = (0usize, 0usize);
    let mut wire: Vec<u8> = Vec::new();
    wire.extend(text_msg(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#));
    let (w, h) = inner.size.lock().map(|s| *s).unwrap_or((1280, 720));
    wire.extend(text_msg(&json!({"t": "size", "w": w, "h": h}).to_string()));
    if reconnect {
        wire.extend(text_msg(r#"{"t":"full"}"#));
    }
    let mut buf = vec![0u8; 32 * 1024];
    let (mut last_rx, mut last_ping, mut rtt) = (Instant::now(), 0u64, None::<f64>);
    loop {
        if inner.stopping() {
            let _ = tls.write_all(&masked_frame(0x8, &[])).and_then(|_| tls.flush());
            return End::Stop;
        }
        if let Ok(mut q) = inner.out.lock() {
            while let Some(m) = q.pop_front() {
                wire.extend(text_msg(&m));
            }
        }
        if ms().saturating_sub(last_ping) >= 2000 {
            last_ping = ms();
            let mut p = json!({"t": "ping", "ts": last_ping});
            if let Some(r) = rtt {
                p["rtt"] = json!(r);
            }
            wire.extend(text_msg(&p.to_string()));
        }
        if !wire.is_empty() {
            let r = tls.write_all(&wire).and_then(|_| tls.flush());
            wire.clear();
            if let Err(e) = r {
                return End::Dropped(format!("sending failed: {e}"));
            }
        }
        match tls.read(&mut buf) {
            Ok(0) => return End::Dropped("the server closed the connection".into()),
            Ok(n) => {
                reader.push(&buf[..n]);
                last_rx = Instant::now();
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return End::Dropped(format!("reading failed: {e}")),
        }
        let (mut changed, mut bye, mut closed) = (false, false, false);
        loop {
            match reader.next() {
                Ok(Some(Msg::Text(t))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    match v["t"].as_str().unwrap_or("") {
                        "hello_ok" => {
                            if v["v"].as_u64().unwrap_or(1) < 2 {
                                return End::Fatal("the tenant's server is too old for the desktop connector (protocol 1)".into());
                            }
                            inner.set_status(Status::Live);
                        }
                        "bye" => bye = true,
                        "pong" => {
                            if let Some(ts) = v["ts"].as_u64() {
                                rtt = Some(ms().saturating_sub(ts) as f64).filter(|r| *r <= 60_000.0);
                            }
                        }
                        "cursor" => {
                            let c = v["c"].as_str().unwrap_or("");
                            if !c.is_empty() && c.len() <= 24 && c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-') {
                                if let Ok(mut g) = inner.cursor.lock() {
                                    *g = Some(c.to_string());
                                }
                            }
                        }
                        "copy" => {
                            if let (Some(s), Ok(mut q)) = (v["s"].as_str(), inner.copied.lock()) {
                                if q.len() < 8 {
                                    q.push_back(s.chars().take(200_000).collect());
                                }
                            }
                        }
                        "readclip" => inner.clip.store(true, Ordering::Relaxed),
                        _ => {} // stats, speech and whatever a newer server adds
                    }
                }
                Ok(Some(Msg::Binary(m))) => {
                    let hd = match decode_frame(&m, &mut canvas) {
                        Ok(h) => h,
                        Err(e) => return End::Dropped(format!("a picture could not be decoded: {e:?}")),
                    };
                    meta = (hd.screen_w as usize, hd.screen_h as usize);
                    if meta.0 * meta.1 > MAX_PIXELS {
                        return End::Dropped("the picture is too big".into());
                    }
                    wire.extend(text_msg(&json!({"t": "ack", "n": hd.seq}).to_string()));
                    changed = true;
                }
                Ok(Some(Msg::Ping(p))) => wire.extend(masked_frame(0xA, &p)),
                Ok(Some(Msg::Pong)) => {}
                Ok(Some(Msg::Close)) => {
                    closed = true;
                    break;
                }
                Ok(None) => break,
                Err(e) => return End::Dropped(format!("bad data from the server: {e}")),
            }
        }
        if changed {
            if let Ok(mut g) = inner.shown.lock() {
                *g = Some(Shown { canvas: canvas.clone(), screen_w: meta.0, screen_h: meta.1 });
            }
            inner.generation.fetch_add(1, Ordering::Relaxed);
            inner.set_status(Status::Live);
        }
        if bye {
            return End::Bye;
        }
        if closed {
            let _ = tls.write_all(&masked_frame(0x8, &[])).and_then(|_| tls.flush());
            return End::Dropped("the server closed the connection".into());
        }
        if last_rx.elapsed() > SILENT {
            return End::Dropped("the server went silent".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_round_trips_through_hex_and_the_display_form() {
        let p = Pin([0xab; 32]);
        assert_eq!(Pin::parse(&p.hex()), Some(p));
        assert_eq!(Pin::parse(&p.display()), Some(p));
        assert!(p.display().starts_with("AB:AB:"));
        assert_eq!(Pin::parse("abc"), None);
        assert_eq!(Pin::parse(&"zz".repeat(32)), None);
    }

    #[test]
    fn addresses_split_into_host_and_port() {
        assert_eq!(split_addr("acme.example.org:8443"), Ok(("acme.example.org".into(), 8443)));
        assert_eq!(split_addr("acme.example.org"), Ok(("acme.example.org".into(), 443)));
        assert_eq!(split_addr("[::1]:9000"), Ok(("::1".into(), 9000)));
        assert_eq!(split_addr("[::1]"), Ok(("::1".into(), 443)));
        assert_eq!(split_addr("127.0.0.1:1"), Ok(("127.0.0.1".into(), 1)));
        assert!(split_addr("host:notaport").is_err());
        assert!(split_addr("").is_err());
        assert!(split_addr("a b:1").is_err());
        assert!(split_addr("https://x").is_err());
    }

    fn one(i: Input) -> Value {
        let m = messages(&i);
        assert_eq!(m.len(), 1, "{m:?}");
        serde_json::from_str(&m[0]).unwrap()
    }

    #[test]
    fn inputs_become_the_messages_the_page_sends() {
        assert_eq!(one(Input::MouseMove { x: 3, y: 4 }), json!({"t": "move", "x": 3, "y": 4}));
        assert_eq!(one(Input::MouseDown { x: 1, y: 2, button: 0 }), json!({"t": "down", "x": 1, "y": 2}));
        assert_eq!(one(Input::MouseDown { x: 1, y: 2, button: 2 }), json!({"t": "right", "x": 1, "y": 2}));
        assert_eq!(one(Input::MouseUp { x: 5, y: 6, button: 0 }), json!({"t": "up", "x": 5, "y": 6}));
        assert!(messages(&Input::MouseUp { x: 5, y: 6, button: 2 }).is_empty(), "the server has no right release");
        assert!(messages(&Input::MouseDown { x: 1, y: 1, button: 1 }).is_empty(), "the middle button is not used");
        assert_eq!(one(Input::Key { name: "Enter".into(), ctrl: false, shift: true, alt: false }), json!({"t": "key", "key": "Enter", "ctrl": false, "shift": true, "alt": false}));
        assert_eq!(one(Input::Size { w: 1280, h: 720 }), json!({"t": "size", "w": 1280, "h": 720}));
        assert_eq!(one(Input::Shift(true)), json!({"t": "shift", "on": true}));
        assert_eq!(one(Input::Paste("héllo".into())), json!({"t": "paste", "s": "héllo"}));
    }

    #[test]
    fn wheel_text_and_paste_follow_the_pages_rules() {
        let w = messages(&Input::Wheel { x: 9, y: 8, dx: 0, dy: 120 });
        assert_eq!(w.iter().map(|m| serde_json::from_str::<Value>(m).unwrap()).collect::<Vec<_>>(), vec![json!({"t": "move", "x": 9, "y": 8}), json!({"t": "wheel", "d": -3})], "down is negative, 40 per notch");
        let up = messages(&Input::Wheel { x: 0, y: 0, dx: 0, dy: -1 });
        assert_eq!(serde_json::from_str::<Value>(&up[1]).unwrap(), json!({"t": "wheel", "d": 1}), "a small scroll is one step");
        let big = messages(&Input::Wheel { x: 0, y: 0, dx: 0, dy: 100_000 });
        assert_eq!(serde_json::from_str::<Value>(&big[1]).unwrap(), json!({"t": "wheel", "d": -10}), "capped at 10");
        assert!(messages(&Input::Wheel { x: 0, y: 0, dx: 5, dy: 0 }).is_empty());
        let t = messages(&Input::Text("a\u{7}é".into()));
        assert_eq!(t.len(), 2, "control characters are dropped");
        assert_eq!(serde_json::from_str::<Value>(&t[1]).unwrap(), json!({"t": "key", "key": "é"}));
        assert_eq!(messages(&Input::Text("x".repeat(9000))).len(), 4000);
        let p = messages(&Input::Paste("y".repeat(300_000)));
        assert_eq!(serde_json::from_str::<Value>(&p[0]).unwrap()["s"].as_str().unwrap().len(), 200_000);
        assert!(messages(&Input::Key { name: String::new(), ctrl: false, shift: false, alt: false }).is_empty());
    }

    #[test]
    fn the_queue_keeps_only_the_newest_pointer_position_and_is_bounded() {
        let q = |m: Vec<String>| {
            let i = Inner {
                target: Target { addr: "x:1".into(), pin: Pin([0; 32]) },
                user: String::new(),
                password: Zeroizing::new(String::new()),
                status: Mutex::new(Status::Connecting),
                shown: Mutex::new(None),
                generation: AtomicU64::new(0),
                out: Mutex::new(VecDeque::new()),
                size: Mutex::new((1, 1)),
                cursor: Mutex::new(None),
                copied: Mutex::new(VecDeque::new()),
                clip: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                finished: AtomicBool::new(false),
            };
            i.push_out(m);
            let n = i.out.lock().unwrap().len();
            n
        };
        let moves: Vec<String> = (0..100).flat_map(|i| messages(&Input::MouseMove { x: i, y: i })).collect();
        assert_eq!(q(moves), 1);
        let keys: Vec<String> = (0..MAX_QUEUE + 50).flat_map(|_| messages(&Input::Shift(true))).collect();
        assert_eq!(q(keys), MAX_QUEUE);
    }

    fn frame(op: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mut f = crate::wsproto::frame(op, payload);
        if !fin {
            f[0] &= 0x7f;
        }
        f
    }

    #[test]
    fn server_frames_are_read_in_pieces_and_hostile_ones_are_refused() {
        let whole = [crate::wsproto::text_frame("hi"), crate::wsproto::binary_frame(&[1, 2, 3])].concat();
        let mut r = FrameReader::new(1000);
        r.push(&whole[..3]);
        assert!(matches!(r.next(), Ok(None)));
        r.push(&whole[3..]);
        assert!(matches!(r.next(), Ok(Some(Msg::Text(t))) if t == "hi"));
        assert!(matches!(r.next(), Ok(Some(Msg::Binary(b))) if b == [1, 2, 3]));
        assert!(matches!(r.next(), Ok(None)));
        // fragmented
        r.push(&frame(0x1, false, b"ab"));
        r.push(&frame(0x0, true, b"cd"));
        assert!(matches!(r.next(), Ok(Some(Msg::Text(t))) if t == "abcd"));
        // too large is refused before it is read
        let mut r = FrameReader::new(10);
        r.push(&[0x82, 127, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        assert!(r.next().is_err());
        // masked server frames, reserved bits, unknown opcodes, a lone continuation, bad UTF-8
        for bad in [vec![0x81, 0x81, 0, 0, 0, 0, b'a'], vec![0xc1, 0], vec![0x83, 0], vec![0x80, 0], frame(0x1, true, &[0xff, 0xfe])] {
            let mut r = FrameReader::new(100);
            r.push(&bad);
            assert!(r.next().is_err(), "{bad:?}");
        }
        let mut r = FrameReader::new(100);
        r.push(&crate::wsproto::close_frame());
        assert!(matches!(r.next(), Ok(Some(Msg::Close))));
    }

    #[test]
    fn client_frames_are_masked_the_way_the_server_reads_them() {
        for len in [0usize, 5, 125, 126, 70_000] {
            let payload = vec![b'x'; len];
            let f = masked_frame(0x1, &payload);
            let mut p = crate::wsproto::Parser::new(100_000);
            p.push(&f);
            assert!(matches!(p.next(), Ok(Some(crate::wsproto::Message::Text(t))) if t.len() == len), "{len}");
        }
    }

    #[test]
    fn a_halved_picture_is_stretched_back_to_the_screen_size() {
        let c = Canvas { w: 2, h: 1, rgba: vec![1, 1, 1, 255, 2, 2, 2, 255] };
        let out = stretch(&c, 4, 2);
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(&out[0..4], &[1, 1, 1, 255]);
        assert_eq!(&out[4..8], &[1, 1, 1, 255]);
        assert_eq!(&out[8..12], &[2, 2, 2, 255]);
        assert_eq!(&out[12..16], &[2, 2, 2, 255]);
        assert_eq!(stretch(&Canvas::default(), 2, 2), vec![0u8; 16], "an empty canvas gives black, not a panic");
    }

    #[test]
    fn server_sentences_are_cleaned_before_they_are_shown() {
        assert_eq!(clean("a\r\nb\u{1b}[31m"), "ab[31m");
        assert_eq!(clean(&"x".repeat(1000)).len(), 160);
        assert_eq!(error_text(br#"{"error":"wrong name or password"}"#, 401), "wrong name or password");
        assert_eq!(error_text(b"<html>", 502), "the server answered 502");
    }

    #[test]
    fn nothing_listens_means_an_error_not_a_hang() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        drop(l);
        assert!(probe(&addr, Duration::from_secs(2)).is_err());
        assert!(Client::connect(&Target { addr, pin: Pin([1; 32]) }, "alice", "pw", (800, 600)).is_err());
    }
}
