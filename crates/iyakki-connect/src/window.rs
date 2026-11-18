//! `iyakki-connect window`: a client window for an IYAKKI instance without a browser tab (docs/development/CLIENT-SPLIT-DESIGN.md step 5,
//! docs/development/CONNECTOR-APP-PLAN.md). EXPERIMENTAL.
//!
//! The program starts a small server on `127.0.0.1` (random port) and opens the installed browser in application mode on it. The server
//! * serves the instance's own `/connect` page and engine, fetched over the pinned TLS connection (nothing is bundled here, so the page
//!   and the engine are always the instance's own version);
//! * replaces `/connect/keys.js` with `web/window-keys.js`, so the **device key stays in this program**: the page asks `/local/sign` and
//!   gets a signature only for a challenge this program itself saw come from the instance;
//! * relays the session WebSocket (`/client/ws`) byte for byte to the instance over the pinned connection, so the browser needs no public
//!   certificate authority and never sees the password's route or the key.
//!
//! What is refused (every one has a test in `tests/window.rs`): a `Host` other than `127.0.0.1:PORT`; an `Origin` other than this one (the
//! WebSocket needs one); anything without the cookie the one-time launch address sets (a second use of it is refused); a change without
//! the CSRF value; any path not listed; a signing request that is not exactly a sign-in over a fresh challenge from the instance for this
//! device within a minute of now; a certificate that is not the pinned one (a hard stop, nothing sent).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iyakki_link::cloudclient::Target;
use iyakki_link::pinned::{self, Pinned};
use iyakki_link::rawlink::LinkError;
use iyakki_link::sigscheme::{hex, DeviceKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::instances::{Instances, Record};
use crate::serve::{cookie, rand_hex, read_request, same, Req, Resp};

/// The replacement for the instance's `/connect/keys.js`.
pub const WINDOW_KEYS_JS: &str = include_str!("../web/window-keys.js");
/// The path a sign-in signature covers (the instance's `iyakki_core_api::client::WS_PATH`; a test keeps them equal).
pub const WS_PATH: &str = "/client/ws";
/// How far the signed time may be from this computer's clock (the instance allows the same).
pub const SIG_WINDOW_SECS: u64 = 60;
const COOKIE: &str = "iyakki_window";
const MAX_CONNECTIONS: usize = 32;
const MAX_SOCKETS: usize = 2;
/// Files of the instance's page that are passed through (the media half's `media.js` and `audio-worklet.js` too). `/connect/keys.js` is ours.
const PAGE_PATHS: [&str; 7] = ["/connect", "/connect/page.js", "/connect/page.css", "/connect/engine.js", "/connect/engine.wasm", "/connect/media.js", "/connect/audio-worklet.js"];
const OUR_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

// ------------------------------------------------------------------------------------------------------------------ what was seen

/// What this program saw come from the instance on the relayed socket: the challenges not yet signed for, and the device id of a fresh enrolment.
#[derive(Default, Debug)]
pub struct Seen {
    challenges: Vec<String>,
    pub enrolled: Option<String>,
}

impl Seen {
    /// Learn from one text message of the instance.
    pub fn note(&mut self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return };
        let t = v["t"].as_str().unwrap_or("");
        if t == "enrolled" {
            if let Some(d) = v["device"].as_str().filter(|d| d.len() <= 64 && d.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')) {
                self.enrolled = Some(d.to_string());
            }
        }
        if matches!(t, "hello_ok" | "enrolled") {
            if let Some(c) = v["challenge"].as_str().filter(|c| !c.is_empty() && c.len() <= 256) {
                self.challenges.push(c.to_string());
                if self.challenges.len() > 8 {
                    self.challenges.remove(0);
                }
            }
        }
    }

    pub fn open_challenges(&self) -> usize {
        self.challenges.len()
    }
}

/// Finds the small text messages in the instance-to-browser byte stream (unmasked WebSocket frames). Nothing is changed or held back: the
/// bytes go on to the browser as they are; this only reads along. Frames that are not whole small text messages are skipped by length.
#[derive(Default)]
pub struct FrameScan {
    buf: Vec<u8>,
    skip: u64,
    dead: bool,
}

const MAX_SCANNED: u64 = 16 * 1024;

impl FrameScan {
    pub fn push(&mut self, mut bytes: &[u8], out: &mut Vec<String>) {
        if self.dead {
            return;
        }
        if self.skip > 0 {
            let n = (self.skip as usize).min(bytes.len());
            self.skip -= n as u64;
            bytes = &bytes[n..];
        }
        self.buf.extend_from_slice(bytes);
        loop {
            let b = &self.buf;
            if b.len() < 2 {
                return;
            }
            let (fin, op, masked, l7) = (b[0] & 0x80 != 0, b[0] & 0x0f, b[1] & 0x80 != 0, (b[1] & 0x7f) as u64);
            if masked || b[0] & 0x70 != 0 {
                self.dead = true; // not what an instance sends: stop reading along (the bytes still go through)
                self.buf.clear();
                return;
            }
            let (len, head) = match l7 {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4usize),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().expect("8 bytes")), 10usize),
                126 | 127 => return,
                n => (n, 2usize),
            };
            if op == 1 && fin && len <= MAX_SCANNED {
                if (b.len() as u64) < head as u64 + len {
                    return;
                }
                if let Ok(s) = std::str::from_utf8(&b[head..head + len as usize]) {
                    out.push(s.to_string());
                }
                self.buf.drain(..head + len as usize);
            } else {
                // not a small text message: drop the header, skip the payload as it passes
                let have = (b.len() - head).min(len as usize);
                self.skip = len - have as u64;
                self.buf.drain(..head + have);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ signing

/// The signature (128 hex digits) of a sign-in text, if and only if it is exactly the sign-in the engine builds: method `GET`, path
/// [`WS_PATH`], for one of `devices`, a time within a minute of `now`, a nonce of 16 to 64 hex digits, over a challenge in `seen` (which is
/// then spent). The text is rebuilt from its parts and signed through the scheme the instance verifies; nothing else can be signed.
pub fn sign_relay(text: &str, key: &DeviceKey, devices: &[&str], seen: &mut Seen, now: u64) -> Result<String, &'static str> {
    let parts: Vec<&str> = text.split('\n').collect();
    let [magic, device, method, path, ts, digest, nonce] = parts.as_slice() else { return Err("that is not a sign-in text") };
    if *magic != "iyakki-vault-v2" || *method != "GET" || *path != WS_PATH {
        return Err("only a sign-in for the client session can be signed");
    }
    if !devices.contains(device) {
        return Err("that is not this device");
    }
    let ts_n: u64 = ts.parse().map_err(|_| "the time is not a number")?;
    if ts_n.to_string() != *ts || ts_n.abs_diff(now) > SIG_WINDOW_SECS {
        return Err("the time is not within a minute of this computer's clock");
    }
    if !(16..=64).contains(&nonce.len()) || !nonce.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("the nonce is not 16 to 64 hex digits");
    }
    let at = seen.challenges.iter().position(|c| hex(&Sha256::digest(c.as_bytes())) == *digest).ok_or("no such challenge came from the instance (or it was signed already)")?;
    let challenge = seen.challenges.remove(at);
    let header = key.sign_request_with_nonce(device, "GET", WS_PATH, ts_n, challenge.as_bytes(), nonce).map_err(|_| "the key could not sign (is the ssh-agent still running and holding it?)")?;
    header.rsplit_once("sig=").map(|(_, s)| s.to_string()).ok_or("signing failed")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ------------------------------------------------------------------------------------------------------------------ the server

/// The key this program holds for the instance. `saved` is false for a new key that waits for the instance to enrol it.
pub struct Identity {
    pub key: DeviceKey,
    pub device: Option<String>,
    pub saved: bool,
}

/// Where the identity is written once the instance has enrolled it.
pub struct Persist {
    pub instances: Instances,
    pub name: String,
}

pub struct Options {
    pub target: Target,
    /// The one-time token of the link, handed to the page once (in the fragment of its first address).
    pub link_token: Option<String>,
    pub identity: Identity,
    pub persist: Option<Persist>,
    /// Port on 127.0.0.1; 0 chooses one at random.
    pub port: u16,
}

struct Shared {
    host: String,
    origin: String,
    launch: Mutex<Option<String>>,
    link_token: Mutex<Option<String>>,
    secret: String,
    csrf: String,
    target: Target,
    ident: Mutex<Identity>,
    seen: Mutex<Seen>,
    persist: Option<Persist>,
    failure: Mutex<Option<String>>,
    sockets: AtomicUsize,
    open: AtomicUsize,
    stop: AtomicBool,
}

pub struct Window {
    shared: Arc<Shared>,
    port: u16,
    launch: String,
}

impl Window {
    pub fn start(o: Options) -> Result<Window, String> {
        let l = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, o.port)).map_err(|e| format!("cannot listen on 127.0.0.1: {e}"))?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        let launch = rand_hex()?;
        let shared = Arc::new(Shared {
            host: format!("127.0.0.1:{port}"),
            origin: format!("http://127.0.0.1:{port}"),
            launch: Mutex::new(Some(launch.clone())),
            link_token: Mutex::new(o.link_token),
            secret: rand_hex()?,
            csrf: rand_hex()?,
            target: o.target,
            ident: Mutex::new(o.identity),
            seen: Mutex::new(Seen::default()),
            persist: o.persist,
            failure: Mutex::new(None),
            sockets: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        std::thread::Builder::new().name("iyakki-window-accept".into()).spawn(move || accept_loop(l, s)).map_err(|e| e.to_string())?;
        Ok(Window { shared, port, launch: format!("http://127.0.0.1:{port}/launch?t={launch}") })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The one-time address the browser opens first. It sets the cookie and works once.
    pub fn launch_url(&self) -> &str {
        &self.launch
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }

    /// Why the window stopped by itself (a certificate that changed), if it did.
    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().ok().and_then(|f| f.clone())
    }

    /// The device id of the instance this program holds, and whether the identity is written down.
    pub fn identity(&self) -> (Option<String>, bool) {
        let i = self.shared.ident.lock().unwrap_or_else(|e| e.into_inner());
        (i.device.clone(), i.saved)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(l: TcpListener, sh: Arc<Shared>) {
    while !sh.stop.load(Ordering::Relaxed) {
        match l.accept() {
            Ok((s, peer)) => {
                if !peer.ip().is_loopback() || sh.open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                sh.open.fetch_add(1, Ordering::Relaxed);
                let sh2 = sh.clone();
                let spawned = std::thread::Builder::new().name("iyakki-window-conn".into()).stack_size(2 * 1024 * 1024).spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(s, &sh2)));
                    sh2.open.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    sh.open.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

enum Pass {
    Normal,
    Socket,
}

fn connection(mut s: TcpStream, sh: &Shared) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let req = match read_request(&mut s) {
        Ok(r) => r,
        Err(r) => {
            let _ = s.write_all(&r.bytes(OUR_CSP));
            return;
        }
    };
    match gate(&req, sh) {
        Err(r) => {
            let _ = s.write_all(&r.bytes(OUR_CSP));
        }
        Ok(Pass::Normal) => {
            let (r, csp) = route(&req, sh);
            let _ = s.write_all(&r.bytes(&csp));
        }
        Ok(Pass::Socket) => relay_socket(s, &req, sh),
    }
}

/// The checks every request goes through, in this order. `Err` is the refusal.
fn gate(req: &Req, sh: &Shared) -> Result<Pass, Resp> {
    if req.count("host") != 1 || req.header("host") != Some(sh.host.as_str()) {
        return Err(Resp::err(403, "wrong host"));
    }
    if req.count("origin") > 1 || req.header("origin").is_some_and(|o| o != sh.origin) {
        return Err(Resp::err(403, "wrong origin"));
    }
    // the one-time launch address: the only request that needs no cookie. It sends the browser on to the page, with the instance's one-time
    // token in the fragment of the address it is sent to (so the token is never in an argument list, a log or a request).
    // (SameSite=Lax, not Strict: the browser arrives from the local start page, and a Strict cookie is not sent on a redirect chain that began
    // on another origin. A change still needs this origin's own Origin header and the CSRF value, so Lax costs nothing here.)
    if req.method == "GET" && req.path == "/launch" {
        let given = req.query.get("t").map(String::as_str).unwrap_or("");
        let mut t = sh.launch.lock().map_err(|_| Resp::err(403, "refused"))?;
        return match t.as_deref() {
            Some(want) if same(want, given) => {
                *t = None;
                let to = match sh.link_token.lock().ok().and_then(|mut x| x.take()) {
                    Some(tok) => format!("/connect#{tok}"),
                    None => "/connect".to_string(),
                };
                Err(Resp::new(303, "text/plain", "").header("Location", &to).header("Set-Cookie", &format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax", sh.secret)))
            }
            _ => Err(Resp::err(403, "this address is not valid (or was already used): start the program again")),
        };
    }
    // The page fetches its engine with `credentials: "omit"`, so no cookie comes with it. The engine is a public file of the instance (the
    // instance serves it to anyone), so this one path needs only the Host and Origin checks above.
    let public_file = req.method == "GET" && req.path == "/connect/engine.wasm";
    if !public_file && !cookie(req, COOKIE).is_some_and(|c| same(c, &sh.secret)) {
        return Err(Resp::err(403, "this window was not opened by the program"));
    }
    if req.method != "GET" {
        if req.header("origin") != Some(sh.origin.as_str()) {
            return Err(Resp::err(403, "wrong origin"));
        }
        if !req.header("x-csrf").is_some_and(|c| same(c, &sh.csrf)) {
            return Err(Resp::err(403, "missing token"));
        }
    }
    if req.method == "GET" && req.path == WS_PATH {
        if !req.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket")) {
            return Err(Resp::err(426, "WebSocket only"));
        }
        // a browser always sends its Origin on a WebSocket; a missing one is a script
        if req.header("origin") != Some(sh.origin.as_str()) {
            return Err(Resp::err(403, "wrong origin"));
        }
        return Ok(Pass::Socket);
    }
    Ok(Pass::Normal)
}

fn route(req: &Req, sh: &Shared) -> (Resp, String) {
    let our = |r: Resp| (r, OUR_CSP.to_string());
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/connect/keys.js") => our(Resp::new(200, "text/javascript; charset=utf-8", WINDOW_KEYS_JS)),
        ("GET", p) if PAGE_PATHS.contains(&p) => page_file(p, sh),
        ("GET", "/local/device") => our(local_device(sh)),
        ("POST", "/local/device") => our(local_device_saved(req, sh)),
        ("POST", "/local/sign") => our(local_sign(req, sh)),
        ("POST", "/local/forget") => our(local_forget(sh)),
        (_, "/connect" | "/connect/keys.js" | "/local/device" | "/local/sign" | "/local/forget") => our(Resp::err(405, "method not allowed")),
        (_, p) if PAGE_PATHS.contains(&p) => our(Resp::err(405, "method not allowed")),
        _ => our(Resp::err(404, "not found")),
    }
}

fn link_failure(sh: &Shared, e: LinkError) -> Resp {
    match e {
        LinkError::PinChanged(m) => {
            // a hard stop: nothing was sent, and the program ends
            if let Ok(mut f) = sh.failure.lock() {
                f.get_or_insert(m.clone());
            }
            sh.stop.store(true, Ordering::Relaxed);
            Resp::err(502, "the instance's certificate is not the pinned one: the window was stopped")
        }
        other => Resp::err(502, &format!("the instance did not answer: {}", other.text())),
    }
}

/// A page file of the instance, fetched over the pinned connection and passed on with the instance's own content type and policy.
fn page_file(path: &str, sh: &Shared) -> (Resp, String) {
    match pinned::get(&sh.target, path, Duration::from_secs(8)) {
        Ok(r) => {
            let ctype: &'static str = match r.header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim() {
                "text/html" => "text/html; charset=utf-8",
                "text/javascript" | "application/javascript" => "text/javascript; charset=utf-8",
                "text/css" => "text/css; charset=utf-8",
                "application/wasm" => "application/wasm",
                "application/json" => "application/json",
                _ => "text/plain; charset=utf-8",
            };
            let status = if matches!(r.status, 200 | 404) { r.status } else { 502 };
            // the instance's policy applies (its page needs `wasm-unsafe-eval`); with none, ours (which allows nothing) does
            let csp = r.header("content-security-policy").filter(|c| !c.contains('\r') && !c.contains('\n')).unwrap_or(OUR_CSP).to_string();
            (Resp::new(status, ctype, r.body), csp)
        }
        Err(e) => (link_failure(sh, e), OUR_CSP.to_string()),
    }
}

fn local_device(sh: &Shared) -> Resp {
    let i = sh.ident.lock().unwrap_or_else(|e| e.into_inner());
    Resp::json(json!({"public": i.key.public_hex(), "device": i.device, "saved": i.saved, "csrf": sh.csrf}))
}

fn body_json(req: &Req) -> Result<Value, Resp> {
    serde_json::from_slice(&req.body).map_err(|_| Resp::err(400, "bad request"))
}

/// The page reports the device id it got. The program believes it only when it is the id the instance itself sent on the relayed socket.
fn local_device_saved(req: &Req, sh: &Shared) -> Resp {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(device) = v["device"].as_str() else { return Resp::err(400, "a device id is needed") };
    let enrolled = sh.seen.lock().unwrap_or_else(|e| e.into_inner()).enrolled.clone();
    let mut i = sh.ident.lock().unwrap_or_else(|e| e.into_inner());
    if i.device.as_deref() == Some(device) {
        return Resp::json(json!({"ok": true}));
    }
    if enrolled.as_deref() != Some(device) {
        return Resp::err(400, "that is not the device id the instance gave");
    }
    i.device = Some(device.to_string());
    if let Some(p) = &sh.persist {
        let rec = Record { name: p.name.clone(), addr: sh.target.addr.clone(), device: i.device.clone(), public: i.key.public_hex() };
        match p.instances.save(&rec, &i.key) {
            Ok(()) => i.saved = true,
            Err(e) => return Resp::err(500, &format!("the device key could not be saved: {e}")),
        }
    }
    Resp::json(json!({"ok": true}))
}

fn local_sign(req: &Req, sh: &Shared) -> Resp {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(text) = v["text"].as_str().filter(|t| t.len() <= 1024) else { return Resp::err(400, "a text is needed") };
    let i = sh.ident.lock().unwrap_or_else(|e| e.into_inner());
    let mut seen = sh.seen.lock().unwrap_or_else(|e| e.into_inner());
    let enrolled = seen.enrolled.clone();
    let mut ok: Vec<&str> = Vec::new();
    if let Some(d) = i.device.as_deref() {
        ok.push(d);
    }
    if let Some(d) = enrolled.as_deref() {
        ok.push(d);
    }
    match sign_relay(text, &i.key, &ok, &mut seen, now_secs()) {
        Ok(sig) => Resp::json(json!({"sig": sig})),
        Err(why) => Resp::err(403, why),
    }
}

fn local_forget(sh: &Shared) -> Resp {
    let mut i = sh.ident.lock().unwrap_or_else(|e| e.into_inner());
    i.device = None;
    i.saved = false;
    if let Some(p) = &sh.persist {
        if let Err(e) = p.instances.forget(&sh.target.addr) {
            return Resp::err(500, &format!("could not forget: {e}"));
        }
    }
    Resp::json(json!({"ok": true}))
}

// ------------------------------------------------------------------------------------------------------------------ the socket

/// Write all of `data` to a socket that is in non-blocking mode.
fn write_nb(s: &mut TcpStream, mut data: &[u8], limit: Duration) -> bool {
    let t0 = Instant::now();
    while !data.is_empty() {
        match s.write(data) {
            Ok(0) => return false,
            Ok(n) => data = &data[n..],
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {
                if t0.elapsed() > limit {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
    true
}

fn relay_socket(mut b: TcpStream, req: &Req, sh: &Shared) {
    if sh.sockets.fetch_add(1, Ordering::Relaxed) >= MAX_SOCKETS {
        sh.sockets.fetch_sub(1, Ordering::Relaxed);
        let _ = b.write_all(&Resp::err(409, "this window has a connection already").bytes(OUR_CSP));
        return;
    }
    relay_socket_inner(&mut b, req, sh);
    sh.sockets.fetch_sub(1, Ordering::Relaxed);
}

fn relay_socket_inner(b: &mut TcpStream, req: &Req, sh: &Shared) {
    let refuse = |b: &mut TcpStream, r: Resp| {
        let _ = b.write_all(&r.bytes(OUR_CSP));
    };
    let key = req.header("sec-websocket-key").filter(|k| k.len() == 24 && k.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='))).map(str::to_string);
    let (Some(key), Some("13")) = (key, req.header("sec-websocket-version")) else { return refuse(b, Resp::err(400, "bad WebSocket request")) };
    let mut up = match Pinned::open(&sh.target, Duration::from_secs(8)) {
        Ok(u) => u,
        Err(e) => return refuse(b, link_failure(sh, e)),
    };
    let addr = &sh.target.addr;
    // the instance sees its own address as Host and Origin, like a browser tab on it, and no cookie of this window
    let ask = format!("GET {WS_PATH} HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://{addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n");
    up.set_timeouts(Some(Duration::from_secs(8)), Some(Duration::from_secs(10)));
    if up.write_all(ask.as_bytes()).and_then(|_| up.flush()).is_err() {
        return refuse(b, Resp::err(502, "the instance did not answer"));
    }
    let (status, headers, rest) = match up.read_head() {
        Ok(h) => h,
        Err(e) => return refuse(b, link_failure(sh, e)),
    };
    let accept = headers.iter().find(|(k, _)| k == "sec-websocket-accept").map(|(_, v)| v.as_str());
    if status != 101 || accept != Some(iyakki_link::wsproto::accept_key(&key).as_str()) {
        let code = if status == 404 { 404 } else { 502 };
        return refuse(b, Resp::err(code, if status == 404 { "the instance does not answer client connections (Connect a client is off)" } else { "the instance did not open the connection" }));
    }
    let head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", iyakki_link::wsproto::accept_key(&key));
    if b.write_all(head.as_bytes()).is_err() {
        return;
    }
    let _ = b.set_nonblocking(true);
    up.set_timeouts(Some(Duration::from_millis(3)), Some(Duration::from_secs(10)));
    let mut scan = FrameScan::default();
    let mut texts = Vec::new();
    let mut note = |scan: &mut FrameScan, bytes: &[u8]| {
        texts.clear();
        scan.push(bytes, &mut texts);
        if !texts.is_empty() {
            let mut seen = sh.seen.lock().unwrap_or_else(|e| e.into_inner());
            for t in &texts {
                seen.note(t);
            }
        }
    };
    if !rest.is_empty() {
        note(&mut scan, &rest);
        if !write_nb(b, &rest, Duration::from_secs(10)) {
            return;
        }
    }
    let mut buf = vec![0u8; 64 * 1024];
    while !sh.stop.load(Ordering::Relaxed) {
        // the browser's messages go to the instance unchanged
        match b.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if up.write_all(&buf[..n]).and_then(|_| up.flush()).is_err() {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
        // the instance's go to the browser unchanged (read along for the challenge)
        match up.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                note(&mut scan, &buf[..n]);
                if !write_nb(b, &buf[..n], Duration::from_secs(10)) {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
    }
}
