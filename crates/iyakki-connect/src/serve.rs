//! `iyakki-connect serve`: a page server on the loopback address that shows a tenant's desktop in the browser already installed.
//!
//! What is refused, and where (every one has a test in `tests/serve.rs`):
//! * the server listens on `127.0.0.1` only, on a random port;
//! * `Host` must be exactly `127.0.0.1:PORT` (no DNS rebinding: a rebinding name sends its own name);
//! * an `Origin`, when present, must be exactly `http://127.0.0.1:PORT`; the WebSocket requires one (browsers always send it);
//! * a one-time token (printed in the link) is exchanged once for a cookie (`HttpOnly`, `SameSite=Strict`); without the cookie every
//!   path, the WebSocket and the scripts are refused; a second use of the token is refused;
//! * only the paths below exist; one WebSocket at a time; a message over the limit ends the connection;
//! * the page is served with a strict Content-Security-Policy (no inline script, no outside request) and the page carries no secret.
//!
//! The password or key stays in this program ([`iyakki_link::rawlink::Auth`]); the page never sees it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_link::cloudclient::Target;
use iyakki_link::rawlink::{Auth, FilesApi, Frame, LinkError, RawWs};
use iyakki_link::wsproto::{binary_frame, frame, text_frame, Message, Parser, WsError};
use serde_json::{json, Value};

use crate::share::{split_rel, Share, ShareError};

pub const INDEX_HTML: &str = include_str!("../web/index.html");
pub const APP_JS: &str = include_str!("../web/app.js");
pub const STYLE_CSS: &str = include_str!("../web/style.css");
/// The tenant's own picture decoder and device bridge, byte for byte: the same files the tenant's page serves (not forked).
pub const DECODER_JS: &str = include_str!("../../iyakki-remote/src/decoder.js");
pub const MEDIA_JS: &str = include_str!("../../iyakki-remote/src/media.js");

/// The biggest message accepted from the page (a clipboard image is the largest thing it sends).
pub const MAX_PAGE_MESSAGE: usize = 4 * 1024 * 1024 + 64 * 1024;
/// The biggest text message accepted from the page (a paste of 200,000 characters is up to 800 kB).
pub const MAX_PAGE_TEXT: usize = 1024 * 1024;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 32;
const COOKIE: &str = "iyakki_connect";

pub struct Options {
    pub target: Target,
    pub auth: Auth,
    pub share: Option<Share>,
    /// The bearer-token client for the tenant's files (needs a password sign-in).
    pub files: Option<FilesApi>,
    /// Port on 127.0.0.1; 0 chooses one at random.
    pub port: u16,
}

struct Shared {
    host: String,
    origin: String,
    token: Mutex<Option<String>>,
    secret: String,
    csrf: String,
    target: Target,
    auth: Auth,
    share: Option<Share>,
    files: Option<FilesApi>,
    ws_active: AtomicBool,
    open: AtomicUsize,
    stop: AtomicBool,
}

pub struct Server {
    shared: Arc<Shared>,
    addr: SocketAddr,
    token: String,
}

fn rand_hex() -> Result<String, String> {
    let mut b = [0u8; 32];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no random numbers: {e:?}"))?;
    Ok(iyakki_link::sigscheme::hex(&b))
}

impl Server {
    pub fn start(o: Options) -> Result<Server, String> {
        let l = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, o.port)).map_err(|e| format!("cannot listen on 127.0.0.1: {e}"))?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let addr = l.local_addr().map_err(|e| e.to_string())?;
        let token = rand_hex()?;
        let shared = Arc::new(Shared {
            host: format!("127.0.0.1:{}", addr.port()),
            origin: format!("http://127.0.0.1:{}", addr.port()),
            token: Mutex::new(Some(token.clone())),
            secret: rand_hex()?,
            csrf: rand_hex()?,
            target: o.target,
            auth: o.auth,
            share: o.share,
            files: o.files,
            ws_active: AtomicBool::new(false),
            open: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        std::thread::Builder::new().name("iyakki-connect-accept".into()).spawn(move || accept_loop(l, s)).map_err(|e| e.to_string())?;
        Ok(Server { shared, addr, token })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The link to open: the token is in the query and is spent by its first use.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?t={}", self.addr.port(), self.token)
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }
}

impl Drop for Server {
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
                let spawned = std::thread::Builder::new().name("iyakki-connect-conn".into()).stack_size(2 * 1024 * 1024).spawn(move || {
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

// ------------------------------------------------------------------------------------------------------------------- HTTP

pub struct Req {
    pub method: String,
    /// The path as sent (not decoded): only exact paths match.
    pub path: String,
    pub query: HashMap<String, String>,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
    fn count(&self, n: &str) -> usize {
        self.headers.iter().filter(|(k, _)| k == n).count()
    }
}

struct Resp {
    status: u16,
    ctype: &'static str,
    body: Vec<u8>,
    extra: Vec<(String, String)>,
}

fn reason(s: u16) -> &'static str {
    match s {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        426 => "Upgrade Required",
        _ => "Error",
    }
}

impl Resp {
    fn new(status: u16, ctype: &'static str, body: impl Into<Vec<u8>>) -> Resp {
        Resp { status, ctype, body: body.into(), extra: vec![] }
    }
    fn err(status: u16, msg: &str) -> Resp {
        Resp::new(status, "application/json", json!({"error": msg}).to_string())
    }
    fn json(v: Value) -> Resp {
        Resp::new(200, "application/json", v.to_string())
    }
    fn header(mut self, k: &str, v: &str) -> Resp {
        self.extra.push((k.into(), v.into()));
        self
    }
    fn bytes(&self, csp: &str) -> Vec<u8> {
        let mut h = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n", self.status, reason(self.status), self.ctype, self.body.len());
        // on every answer, errors included
        h.push_str(&format!("Content-Security-Policy: {csp}\r\n"));
        h.push_str("Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Frame-Options: DENY\r\n");
        for (k, v) in &self.extra {
            h.push_str(&format!("{k}: {v}\r\n"));
        }
        h.push_str("\r\n");
        let mut out = h.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn csp(port: u16) -> String {
    // no inline script or style, nothing from outside; the worker is the decoder file of this origin; the WebSocket is this origin's
    format!("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self' ws://127.0.0.1:{port}; worker-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
}

/// Percent-decoding of a query value; invalid UTF-8 or a bad escape is an error (no lenient reading: it is a security boundary).
fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn read_request(s: &mut TcpStream) -> Result<Req, Resp> {
    let started = Instant::now();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(Resp::err(400, "request too large"));
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err(Resp::err(400, "too slow"));
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Resp::err(400, "no request")),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| Resp::err(400, "bad request"))?;
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or("");
    let mut p = line.split(' ');
    let (method, target, version) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
    if p.next().is_some() || version != "HTTP/1.1" || method.is_empty() || !target.starts_with('/') {
        return Err(Resp::err(400, "bad request"));
    }
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let mut query = HashMap::new();
    for kv in qs.split('&').filter(|x| !x.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let (Some(k), Some(v)) = (pct_decode(k), pct_decode(v)) else { return Err(Resp::err(400, "bad query")) };
        query.insert(k, v);
    }
    let mut headers = Vec::new();
    for l in lines {
        let (k, v) = l.split_once(':').ok_or_else(|| Resp::err(400, "bad header"))?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let mut req = Req { method: method.into(), path: path.into(), query, headers, body: vec![] };
    if req.header("transfer-encoding").is_some() {
        return Err(Resp::err(400, "bad request"));
    }
    let len = match req.header("content-length") {
        None => 0,
        Some(v) => v.parse::<usize>().map_err(|_| Resp::err(400, "bad length"))?,
    };
    if len > MAX_BODY {
        return Err(Resp::err(413, "body too large"));
    }
    let mut body = buf[end + 4..].to_vec();
    while body.len() < len {
        if started.elapsed() > Duration::from_secs(10) {
            return Err(Resp::err(400, "too slow"));
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Resp::err(400, "short body")),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(len);
    req.body = body;
    Ok(req)
}

fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn cookie<'a>(req: &'a Req, name: &str) -> Option<&'a str> {
    req.header("cookie")?.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

fn connection(mut s: TcpStream, sh: &Shared) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let port = sh.host.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let csp = csp(port);
    let req = match read_request(&mut s) {
        Ok(r) => r,
        Err(r) => {
            let _ = s.write_all(&r.bytes(&csp));
            return;
        }
    };
    match gate(&req, sh) {
        Err(r) => {
            let _ = s.write_all(&r.bytes(&csp));
        }
        Ok(Pass::Normal) => {
            let r = route(&req, sh);
            let _ = s.write_all(&r.bytes(&csp));
        }
        Ok(Pass::Socket) => websocket(s, &req, sh, &csp),
    }
}

enum Pass {
    Normal,
    Socket,
}

/// The checks every request goes through, in this order. `Err` is the refusal.
fn gate(req: &Req, sh: &Shared) -> Result<Pass, Resp> {
    // 1. the name the browser used: DNS rebinding sends its own name, and a missing or doubled Host is not a browser's
    if req.count("host") != 1 || req.header("host") != Some(sh.host.as_str()) {
        return Err(Resp::err(403, "wrong host"));
    }
    // 2. another site asking from the person's browser carries its own Origin
    if req.count("origin") > 1 || req.header("origin").is_some_and(|o| o != sh.origin) {
        return Err(Resp::err(403, "wrong origin"));
    }
    // 3. the one-time token, exchanged for a cookie: the only request that needs no cookie
    if req.method == "GET" && req.path == "/" && req.query.contains_key("t") {
        let given = req.query.get("t").map(String::as_str).unwrap_or("");
        let mut t = sh.token.lock().map_err(|_| Resp::err(403, "refused"))?;
        return match t.as_deref() {
            Some(want) if same(want, given) => {
                *t = None; // spent
                Err(Resp::new(303, "text/plain", "").header("Location", "/").header("Set-Cookie", &format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict", sh.secret)))
            }
            _ => Err(Resp::err(403, "this link is not valid (or was already used): start the program again for a new one")),
        };
    }
    // 4. everything else needs the cookie
    if !cookie(req, COOKIE).is_some_and(|c| same(c, &sh.secret)) {
        return Err(Resp::err(403, "open the link the program printed"));
    }
    // 5. a change needs this origin's own Origin header and the CSRF value only this origin could fetch
    if req.method != "GET" {
        if req.header("origin") != Some(sh.origin.as_str()) {
            return Err(Resp::err(403, "wrong origin"));
        }
        if !req.header("x-csrf").is_some_and(|c| same(c, &sh.csrf)) {
            return Err(Resp::err(403, "missing token"));
        }
    }
    if req.method == "GET" && req.path == "/ws" {
        let up = req.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
        if !up {
            return Err(Resp::err(426, "WebSocket only"));
        }
        // browsers always send Origin on a WebSocket; a missing one is a script
        if req.header("origin") != Some(sh.origin.as_str()) {
            return Err(Resp::err(403, "wrong origin"));
        }
        return Ok(Pass::Socket);
    }
    Ok(Pass::Normal)
}

fn route(req: &Req, sh: &Shared) -> Resp {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => Resp::new(200, "text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/app.js") => Resp::new(200, "text/javascript; charset=utf-8", APP_JS),
        ("GET", "/style.css") => Resp::new(200, "text/css; charset=utf-8", STYLE_CSS),
        ("GET", "/decoder.js") => Resp::new(200, "text/javascript; charset=utf-8", DECODER_JS),
        ("GET", "/media.js") => Resp::new(200, "text/javascript; charset=utf-8", MEDIA_JS),
        ("GET", "/api/config") => Resp::json(json!({"csrf": sh.csrf, "share": sh.share.is_some(), "write": sh.share.as_ref().is_some_and(|s| s.writable()), "user": sh.auth.user()})),
        ("GET", "/fs/local/list") => files_list(req, sh, true),
        ("GET", "/fs/remote/list") => files_list(req, sh, false),
        ("POST", "/fs/send") => files_send(req, sh),
        ("POST", "/fs/fetch") => files_fetch(req, sh),
        (_, "/" | "/app.js" | "/style.css" | "/decoder.js" | "/media.js" | "/api/config" | "/fs/local/list" | "/fs/remote/list" | "/fs/send" | "/fs/fetch") => Resp::err(405, "method not allowed"),
        _ => Resp::err(404, "not found"),
    }
}

// ------------------------------------------------------------------------------------------------------------------- files

fn share_err(e: ShareError) -> Resp {
    let status = match e {
        ShareError::NotFound => 404,
        ShareError::Exists => 409,
        ShareError::TooLarge => 413,
        ShareError::ReadOnly | ShareError::Refused(_) => 403,
        ShareError::Io(_) => 400,
    };
    let mut v = json!({"error": e.text()});
    if e == ShareError::Exists {
        v["exists"] = json!(true);
    }
    Resp::new(status, "application/json", v.to_string())
}

fn link_err(e: LinkError) -> Resp {
    Resp::err(400, e.text())
}

fn files_list(req: &Req, sh: &Shared, local: bool) -> Resp {
    let Some(share) = &sh.share else { return Resp::err(404, "no folder is shared (start the program with --share DIR)") };
    let path = req.query.get("path").map(String::as_str).unwrap_or("");
    if local {
        return match share.list(path) {
            Ok(v) => Resp::json(json!({"path": path, "entries": v.iter().map(|e| json!({"name": e.name, "kind": e.kind, "size": e.size})).collect::<Vec<_>>()})),
            Err(e) => share_err(e),
        };
    }
    if let Err(e) = split_rel(path) {
        return share_err(e);
    }
    let Some(api) = &sh.files else { return Resp::err(404, "the tenant's files need a password sign-in (not available with a key)") };
    match api.list(path) {
        Ok(v) => Resp::json(json!({"path": path, "entries": v.iter().map(|e| json!({"name": e.name, "kind": e.kind, "size": e.size})).collect::<Vec<_>>()})),
        Err(e) => link_err(e),
    }
}

fn body_json(req: &Req) -> Result<Value, Resp> {
    serde_json::from_slice(&req.body).map_err(|_| Resp::err(400, "bad request"))
}

fn files_send(req: &Req, sh: &Shared) -> Resp {
    let (Some(share), Some(api)) = (&sh.share, &sh.files) else { return Resp::err(404, "no folder is shared, or no password sign-in") };
    let v = match body_json(req) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (Some(local), Some(remote)) = (v["local"].as_str(), v["remote"].as_str()) else { return Resp::err(400, "local and remote are needed") };
    if remote.is_empty() {
        return Resp::err(400, "name the file on the tenant");
    }
    if let Err(e) = split_rel(remote) {
        return share_err(e);
    }
    let bytes = match share.read(local) {
        Ok(b) => b,
        Err(e) => return share_err(e),
    };
    if !v["replace"].as_bool().unwrap_or(false) {
        let (dir, name) = remote.rsplit_once('/').unwrap_or(("", remote));
        // the tenant's list is the only way to ask whether a name is taken
        if let Ok(list) = api.list(dir) {
            if list.iter().any(|e| e.name == name) {
                return share_err(ShareError::Exists).header("X-Note", "remote");
            }
        }
    }
    match api.upload(remote, &bytes) {
        Ok(()) => Resp::json(json!({"ok": true, "bytes": bytes.len()})),
        Err(e) => link_err(e),
    }
}

fn files_fetch(req: &Req, sh: &Shared) -> Resp {
    let (Some(share), Some(api)) = (&sh.share, &sh.files) else { return Resp::err(404, "no folder is shared, or no password sign-in") };
    if !share.writable() {
        return share_err(ShareError::ReadOnly);
    }
    let v = match body_json(req) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (Some(local), Some(remote)) = (v["local"].as_str(), v["remote"].as_str()) else { return Resp::err(400, "local and remote are needed") };
    let replace = v["replace"].as_bool().unwrap_or(false);
    if let Err(e) = split_rel(remote).and_then(|_| split_rel(local)) {
        return share_err(e);
    }
    // ask first: do not fetch a big file only to find the name is taken
    if !replace {
        match share.exists(local) {
            Ok(true) => return share_err(ShareError::Exists),
            Ok(false) => {}
            Err(e) => return share_err(e),
        }
    }
    let bytes = match api.download(remote, crate::share::MAX_FILE as usize) {
        Ok(b) => b,
        Err(e) => return link_err(e),
    };
    match share.write(local, &bytes, replace) {
        Ok(()) => Resp::json(json!({"ok": true, "bytes": bytes.len()})),
        Err(e) => share_err(e),
    }
}

// ------------------------------------------------------------------------------------------------------------------- WebSocket

/// What the page may say to the tenant, in the tenant's own JSON. Everything else is dropped.
const ALLOWED: &[&str] = &["size", "hello", "ack", "full", "quality", "ping", "paste", "media", "audio", "shift", "move", "down", "up", "right", "wheel", "key"];

/// A text message of the page, when it is an object whose `t` is one of the allowed ones. Forwarded as it came.
pub fn check_text(t: &str) -> Option<&str> {
    if t.len() > MAX_PAGE_TEXT {
        return None;
    }
    let v: Value = serde_json::from_str(t).ok()?;
    let kind = v.get("t")?.as_str()?;
    ALLOWED.contains(&kind).then_some(t)
}

pub enum Up {
    Text(String),
    Binary(Vec<u8>),
}

/// A binary message of the page: `0x01..=0x04` is a microphone, camera or clipboard item and goes on as it is (the tenant checks, limits and
/// audits it); `0x20..` is input in a compact form and becomes the tenant's JSON. Anything else is dropped.
pub fn translate_binary(b: &[u8]) -> Option<Up> {
    let (&t, rest) = b.split_first()?;
    let xy = |r: &[u8]| -> Option<(u16, u16)> { Some((u16::from_le_bytes(r.get(0..2)?.try_into().ok()?), u16::from_le_bytes(r.get(2..4)?.try_into().ok()?))) };
    let j = |v: Value| Some(Up::Text(v.to_string()));
    match t {
        0x01..=0x04 => Some(Up::Binary(b.to_vec())),
        0x20 | 0x21 | 0x22 | 0x23 if rest.len() == 4 => {
            let (x, y) = xy(rest)?;
            let kind = ["move", "down", "up", "right"][(t - 0x20) as usize];
            j(json!({"t": kind, "x": x, "y": y}))
        }
        0x24 if rest.len() == 1 => {
            let d = rest[0] as i8;
            if d == 0 {
                return None;
            }
            j(json!({"t": "wheel", "d": d}))
        }
        0x25 if rest.len() >= 2 => {
            let (flags, kl) = (rest[0], rest[1] as usize);
            let k = std::str::from_utf8(rest.get(2..2 + kl)?).ok()?;
            let code = std::str::from_utf8(rest.get(2 + kl..)?).ok()?;
            if k.is_empty() || k.len() > 32 || code.len() > 32 || k.chars().any(|c| c.is_control()) || code.chars().any(|c| c.is_control()) {
                return None;
            }
            let mut v = json!({"t": "key", "key": k, "ctrl": flags & 1 != 0, "shift": flags & 2 != 0, "alt": flags & 4 != 0});
            if !code.is_empty() {
                v["code"] = json!(code);
            }
            j(v)
        }
        0x26 if rest.len() == 1 => j(json!({"t": "shift", "on": rest[0] != 0})),
        _ => None,
    }
}

fn close_with(s: &mut TcpStream, code: u16) {
    let _ = s.write_all(&frame(0x8, &code.to_be_bytes()));
}

fn websocket(mut s: TcpStream, req: &Req, sh: &Shared, csp: &str) {
    let Some(key) = req.header("sec-websocket-key") else {
        let _ = s.write_all(&Resp::err(400, "no key").bytes(csp));
        return;
    };
    // one window at a time: a second one is refused before it is upgraded
    if sh.ws_active.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        let _ = s.write_all(&Resp::err(409, "this desktop is already open in another window").bytes(csp));
        return;
    }
    struct Release<'a>(&'a AtomicBool);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _release = Release(&sh.ws_active);
    let hello = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", iyakki_link::wsproto::accept_key(key));
    if s.write_all(hello.as_bytes()).is_err() {
        return;
    }
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let said = |s: &mut TcpStream, fin: bool, m: &str| {
        let _ = s.write_all(&text_frame(&json!({"t": "connect_error", "msg": m, "final": fin}).to_string()));
    };
    // the tenant: sign in and open the WebSocket (the pinned certificate is checked first; nothing is sent to another one)
    let mut up = match RawWs::open(&sh.target, &sh.auth) {
        Ok(u) => u,
        Err(e) => {
            let fin = matches!(e, LinkError::Refused(_) | LinkError::PinChanged(_));
            said(&mut s, fin, e.text());
            let _ = s.write_all(&frame(0x8, &1000u16.to_be_bytes()));
            return;
        }
    };
    // the page's messages arrive on a thread of their own and are handed over here
    let (tx, rx) = mpsc::channel::<Up>();
    let gone = Arc::new(AtomicBool::new(false));
    let (rd, g2) = (s.try_clone(), gone.clone());
    let Ok(mut rd) = rd else { return };
    let mut pong_out = match s.try_clone() {
        Ok(p) => p,
        Err(_) => return,
    };
    let reader = std::thread::Builder::new().name("iyakki-connect-page".into()).spawn(move || {
        let _ = rd.set_read_timeout(Some(Duration::from_millis(200)));
        let mut parser = Parser::new(MAX_PAGE_MESSAGE);
        let mut buf = vec![0u8; 32 * 1024];
        'outer: while !g2.load(Ordering::Relaxed) {
            match rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => parser.push(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(_) => break,
            }
            loop {
                match parser.next() {
                    Ok(Some(Message::Text(t))) => {
                        if let Some(t) = check_text(&t) {
                            if tx.send(Up::Text(t.to_string())).is_err() {
                                break 'outer;
                            }
                        }
                    }
                    Ok(Some(Message::Binary(b))) => {
                        if let Some(u) = translate_binary(&b) {
                            if tx.send(u).is_err() {
                                break 'outer;
                            }
                        }
                    }
                    Ok(Some(Message::Ping(p))) => {
                        let _ = pong_out.write_all(&frame(0xA, &p));
                    }
                    Ok(Some(Message::Pong)) => {}
                    Ok(Some(Message::Close)) => break 'outer,
                    Ok(None) => break,
                    Err(WsError::Protocol(why)) => {
                        // a message over the limit (or a broken frame) ends the connection with the proper code
                        close_with(&mut pong_out, if why.contains("too large") { 1009 } else { 1002 });
                        break 'outer;
                    }
                }
            }
        }
        g2.store(true, Ordering::Relaxed);
    });
    if reader.is_err() {
        return;
    }
    // a short wait while things are moving (an acknowledgement or a key is not held back by the poll), a longer one when idle
    let mut last_busy = Instant::now();
    let mut waiting_short = false;
    while !gone.load(Ordering::Relaxed) && !sh.stop.load(Ordering::Relaxed) {
        let busy = last_busy.elapsed() < Duration::from_millis(1500);
        if busy != waiting_short {
            waiting_short = busy;
            up.set_poll_wait(Duration::from_millis(if busy { 1 } else { 4 }));
        }
        match up.poll() {
            Ok(Some(Frame::Text(t))) => {
                last_busy = Instant::now();
                if s.write_all(&text_frame(&t)).is_err() {
                    break;
                }
            }
            Ok(Some(Frame::Binary(b))) => {
                last_busy = Instant::now();
                if s.write_all(&binary_frame(&b)).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                said(&mut s, false, &e);
                let _ = s.write_all(&frame(0x8, &1001u16.to_be_bytes()));
                break;
            }
        }
        while let Ok(m) = rx.try_recv() {
            last_busy = Instant::now();
            let r = match m {
                Up::Text(t) => up.send_text(&t),
                Up::Binary(b) => up.send_binary(&b),
            };
            if r.is_err() {
                gone.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    gone.store(true, Ordering::Relaxed);
    let _ = s.shutdown(std::net::Shutdown::Both);
    up.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_is_forwarded_only_when_it_is_an_allowed_message() {
        assert!(check_text(r#"{"t":"size","w":800,"h":600}"#).is_some());
        assert!(check_text(r#"{"t":"media","kind":"mic","on":true}"#).is_some());
        for bad in [r#"{"t":"sp_voices"}"#, r#"{"t":"speak"}"#, r#"{"x":1}"#, r#"[1]"#, "not json", r#"{"t":5}"#, r#"{"t":"logout"}"#] {
            assert!(check_text(bad).is_none(), "{bad}");
        }
        assert!(check_text(&format!(r#"{{"t":"paste","s":"{}"}}"#, "a".repeat(MAX_PAGE_TEXT))).is_none(), "over the text limit");
    }

    #[test]
    fn compact_input_becomes_the_tenants_messages() {
        let t = |b: &[u8]| match translate_binary(b) {
            Some(Up::Text(s)) => serde_json::from_str::<Value>(&s).unwrap(),
            _ => Value::Null,
        };
        assert_eq!(t(&[0x20, 10, 0, 20, 0]), json!({"t": "move", "x": 10, "y": 20}));
        assert_eq!(t(&[0x21, 0xff, 0xff, 0, 1]), json!({"t": "down", "x": 65535, "y": 256}));
        assert_eq!(t(&[0x22, 1, 0, 2, 0]), json!({"t": "up", "x": 1, "y": 2}));
        assert_eq!(t(&[0x23, 1, 0, 2, 0]), json!({"t": "right", "x": 1, "y": 2}));
        assert_eq!(t(&[0x24, 0xfd]), json!({"t": "wheel", "d": -3}));
        assert_eq!(t(&[0x26, 1]), json!({"t": "shift", "on": true}));
        assert_eq!(t(&[0x25, 3, 5, b'E', b'n', b't', b'e', b'r', b'K']), json!({"t": "key", "key": "Enter", "ctrl": true, "shift": true, "alt": false, "code": "K"}));
        // wrong length, empty or odd keys, unknown kinds: dropped
        for bad in [&[0x20, 1, 0][..], &[0x24][..], &[0x24, 0][..], &[0x25, 0, 0][..], &[0x25, 0, 3, b'a'][..], &[0x25, 0, 1, 7][..], &[0x30][..], &[][..], &[0x05, 1][..]] {
            assert!(translate_binary(bad).is_none(), "{bad:?}");
        }
        // media passes untouched
        assert!(matches!(translate_binary(&[0x01, 9, 9]), Some(Up::Binary(b)) if b == [0x01, 9, 9]));
    }

    #[test]
    fn queries_decode_strictly() {
        assert_eq!(pct_decode("a%2Fb%20c"), Some("a/b c".into()));
        assert_eq!(pct_decode("%2e%2e"), Some("..".into()));
        assert_eq!(pct_decode("%zz"), None);
        assert_eq!(pct_decode("%c0%ae"), None, "an overlong encoding is invalid UTF-8");
        assert_eq!(pct_decode("%"), None);
    }
}
