//! The web server behind Settings -> Shared folders: one small HTTP/1.1 server per share, on its own port (so its own browser
//! origin: a shared *website* can run its scripts without ever touching another share, the portal or the shell's pages).
//! `docs/SHARED-FOLDERS.md` explains the model in plain words.
//!
//! * `Site::respond` is the whole request logic (pure in what it needs: the peer address and the clock are handed in), the
//!   connection code at the bottom only reads a request with hard limits and writes the reply.
//! * Limits: 32 connections, 16 KB of headers (`httpmini::MAX_HEAD`), 15 s to send the headers, 15 s without a byte while
//!   an upload arrives, 2 uploads at a time, no keep-alive (`Connection: close`).
//! * Nothing is served from outside the folder: the path is split, every part is checked, the result is canonicalised and must
//!   still be inside the folder (so a symlink pointing out is a 404), dotfiles are hidden, the deny list (the vault, the
//!   settings, the share list itself) is never served.
//! * No cookies. The password mode uses HTTP Basic (weak on plain HTTP: the UI says so); the token mode puts a 128-bit secret
//!   in the address.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::httpmini::MAX_HEAD;
use crate::sharedfolder::{Access, Mode, ShareCfg};

pub const MAX_CONNECTIONS: usize = 32;
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(15);
pub const BODY_IDLE: Duration = Duration::from_secs(15);
const MAX_UPLOADS_AT_ONCE: usize = 2;
const MAX_LIST: usize = 5000;
const RATE_WINDOW: i64 = 10;
const RATE_MAX: u32 = 300;
const FAIL_WINDOW: i64 = 60;
const FAIL_MAX: u32 = 10;
/// Free space that an upload may never use up.
pub const DISK_RESERVE: u64 = 16 * 1024 * 1024;

/// What the server needs from outside (all injectable for tests).
#[derive(Clone)]
pub struct Env {
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// (event, detail): the audit log. Never gets a password or a token.
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// Real paths that are never served (the vault, the settings, the share list).
    pub deny: Vec<PathBuf>,
    pub free_space: Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>,
}

impl Env {
    pub fn real(deny: Vec<PathBuf>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Env {
        Env { clock: Arc::new(crate::clock::now_unix), audit, deny, free_space: Arc::new(statvfs_free) }
    }
}

/// Free bytes on the volume of `p`, for a normal user.
pub fn statvfs_free(p: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` is plain data that statvfs fills in.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
}

// ------------------------------------------------------------------ the request

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    /// The path exactly as sent (before the `?`).
    pub raw_path: String,
    pub query: String,
    /// Names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, n: &str) -> Option<&str> {
        self.headers.get(n).map(String::as_str)
    }
    /// A plain request for tests.
    pub fn get(path: &str) -> Req {
        Req { method: "GET".into(), raw_path: path.into(), query: String::new(), headers: HashMap::new(), body: vec![] }
    }
    pub fn with(mut self, k: &str, v: &str) -> Req {
        self.headers.insert(k.to_ascii_lowercase(), v.into());
        self
    }
    pub fn method(mut self, m: &str) -> Req {
        self.method = m.into();
        self
    }
    pub fn body(mut self, b: &[u8]) -> Req {
        self.headers.insert("content-length".into(), b.len().to_string());
        self.body = b.to_vec();
        self
    }
}

pub enum Head {
    Incomplete,
    Done(Req, usize),
    Bad(u16, &'static str),
}

/// Read the request line and headers out of what has arrived. The body (if any) starts at the returned offset.
pub fn parse_head(buf: &[u8]) -> Head {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > MAX_HEAD { Head::Bad(431, "request headers too large") } else { Head::Incomplete };
    };
    if end > MAX_HEAD {
        return Head::Bad(431, "request headers too large");
    }
    let Ok(text) = std::str::from_utf8(&buf[..end]) else { return Head::Bad(400, "bad request") };
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version), None) = (first.next(), first.next(), first.next(), first.next()) else {
        return Head::Bad(400, "bad request line");
    };
    if !version.starts_with("HTTP/1.") {
        return Head::Bad(505, "HTTP version not supported");
    }
    if !target.starts_with('/') || target.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'\\') {
        return Head::Bad(400, "bad path");
    }
    let mut headers = HashMap::new();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { return Head::Bad(400, "bad header") };
        if headers.len() >= 64 {
            return Head::Bad(431, "too many headers");
        }
        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
    }
    if headers.contains_key("transfer-encoding") {
        return Head::Bad(501, "chunked requests are not supported");
    }
    let (p, q) = target.split_once('?').unwrap_or((target, ""));
    Head::Done(Req { method: method.to_ascii_uppercase(), raw_path: p.into(), query: q.into(), headers, body: vec![] }, end + 4)
}

fn hexv(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Decode one path segment (`%xx` only; a `+` stays a plus).
fn decode_segment(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let (h, l) = (b.get(i + 1).and_then(|c| hexv(*c))?, b.get(i + 2).and_then(|c| hexv(*c))?);
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn pct_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// A name that is safe to use as one part of a path (no separators, no control characters, not `.` or `..`).
fn clean_part(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && s != "." && s != ".." && !s.chars().any(|c| c.is_control() || c == '/' || c == '\\')
}

// ------------------------------------------------------------------ the reply

pub enum Body {
    Bytes(Vec<u8>),
    File { file: File, start: u64, len: u64 },
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

fn reason(s: u16) -> &'static str {
    match s {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        206 => "Partial Content",
        301 => "Moved Permanently",
        303 => "See Other",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

impl Reply {
    fn bytes(status: u16, ctype: &str, body: Vec<u8>) -> Reply {
        Reply { status, headers: vec![("Content-Type".into(), ctype.into())], body: Body::Bytes(body) }
    }
    fn h(mut self, k: &str, v: &str) -> Reply {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    pub fn len(&self) -> u64 {
        match &self.body {
            Body::Bytes(b) => b.len() as u64,
            Body::File { len, .. } => *len,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Everything of the reply as bytes (tests; a file body is read whole).
    pub fn to_vec(&self, head_only: bool) -> Vec<u8> {
        let mut v = Vec::new();
        let _ = self.write_to(&mut v, head_only, &AtomicBool::new(false));
        v
    }
    pub fn write_to(&self, w: &mut dyn Write, head_only: bool, stop: &AtomicBool) -> std::io::Result<()> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        let nobody = self.status == 304 || self.status == 100;
        if !nobody {
            head.push_str(&format!("Content-Length: {}\r\n", self.len()));
        }
        head.push_str("Connection: close\r\n");
        for (k, v) in &self.headers {
            let v: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        w.write_all(head.as_bytes())?;
        if head_only || nobody {
            return Ok(());
        }
        match &self.body {
            Body::Bytes(b) => w.write_all(b),
            Body::File { file, start, len } => {
                let mut pos = *start;
                let end = start + len;
                let mut buf = vec![0u8; 64 * 1024];
                while pos < end {
                    if stop.load(Ordering::Relaxed) {
                        return Err(std::io::Error::other("stopped"));
                    }
                    let want = ((end - pos) as usize).min(buf.len());
                    let n = file.read_at(&mut buf[..want], pos)?;
                    if n == 0 {
                        break;
                    }
                    w.write_all(&buf[..n])?;
                    pos += n as u64;
                }
                Ok(())
            }
        }
    }
}

// ------------------------------------------------------------------ content types

/// Content type by extension (a small table; anything else is a download).
pub fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "xhtml" => "application/xhtml+xml; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" | "log" | "conf" | "ini" | "toml" | "yaml" | "yml" | "rs" | "py" | "c" | "h" | "sh" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" | "xsl" => "application/xml; charset=utf-8",
        "svg" | "svgz" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" | "aac" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        _ => "application/octet-stream",
    }
}

/// Types a browser would run or interpret as a page: in a *listing* share they are downloads, never shown.
pub fn is_active_content(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "html" | "htm" | "xhtml" | "shtml" | "svg" | "svgz" | "js" | "mjs" | "xml" | "xsl" | "xslt" | "wasm" | "hta")
}

// ------------------------------------------------------------------ dates

const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn http_date(t: i64) -> String {
    let days = t.div_euclid(86400);
    let s = t.rem_euclid(86400);
    let (y, m, d) = crate::clock::civil(days);
    format!("{}, {:02} {} {} {:02}:{:02}:{:02} GMT", DAYS[days.rem_euclid(7) as usize], d, MONTHS[(m - 1) as usize], y, s / 3600, s % 3600 / 60, s % 60)
}

fn list_date(t: i64) -> String {
    let (y, m, d) = crate::clock::civil(t.div_euclid(86400));
    let s = t.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", s / 3600, s % 3600 / 60)
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() => o.push('\u{FFFD}'),
            c => o.push(c),
        }
    }
    o
}

// ------------------------------------------------------------------ who may connect

fn v4_ok(a: Ipv4Addr) -> bool {
    a.is_loopback() || a.is_private() || a.is_link_local() || (a.octets()[0] == 100 && (a.octets()[1] & 0xC0) == 64)
}

/// Is this address on a local network (private, link-local, shared-address space of VPNs, loopback)? Public addresses are
/// refused in every mode, so a forwarded router port does not put a share on the internet.
pub fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => v4_ok(a),
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v) => v4_ok(v),
            None => a.is_loopback() || (a.segments()[0] & 0xffc0) == 0xfe80 || (a.segments()[0] & 0xfe00) == 0xfc00,
        },
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v| v.is_loopback()),
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ the site

struct Window {
    start: i64,
    count: u32,
}

pub struct Gate {
    pub ip: IpAddr,
    /// The path below the share's root (the token prefix removed).
    pub rel: Vec<String>,
    /// The part of the address before it (`/s/<token>` or empty).
    prefix: String,
}

pub struct Site {
    pub cfg: ShareCfg,
    root: PathBuf,
    env: Env,
    pub stop: AtomicBool,
    pub expires_at: AtomicI64,
    active: AtomicUsize,
    uploads: AtomicUsize,
    pub requests: AtomicU64,
    pub clients: Mutex<HashSet<IpAddr>>,
    rate: Mutex<HashMap<IpAddr, Window>>,
    fails: Mutex<HashMap<IpAddr, Window>>,
    auth_ok: Mutex<HashSet<[u8; 32]>>,
}

fn simple(status: u16, msg: &str) -> Reply {
    let page = format!("<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>{status}</title><body style=\"font:16px system-ui,sans-serif;margin:2rem\"><h1>{status} {}</h1><p>{}</p>", reason(status), esc(msg));
    Reply::bytes(status, "text/html; charset=utf-8", page.into_bytes()).h("Cache-Control", "no-store")
}

impl Site {
    pub fn new(cfg: ShareCfg, env: Env) -> Result<Site, String> {
        let root = std::fs::canonicalize(&cfg.folder).map_err(|e| format!("the folder cannot be opened: {e}"))?;
        if !root.is_dir() {
            return Err("the shared path is not a folder".into());
        }
        let exp = cfg.expires_at;
        Ok(Site { cfg, root, env, stop: AtomicBool::new(false), expires_at: AtomicI64::new(exp), active: AtomicUsize::new(0), uploads: AtomicUsize::new(0), requests: AtomicU64::new(0), clients: Mutex::new(HashSet::new()), rate: Mutex::new(HashMap::new()), fails: Mutex::new(HashMap::new()), auth_ok: Mutex::new(HashSet::new()) })
    }

    pub fn clients_served(&self) -> usize {
        self.clients.lock().map(|c| c.len()).unwrap_or(0)
    }

    fn now(&self) -> i64 {
        (self.env.clock)()
    }

    fn website(&self) -> bool {
        self.cfg.mode == Mode::Website
    }

    fn max_upload(&self) -> u64 {
        self.cfg.max_upload_mb.clamp(1, 512) * 1024 * 1024
    }

    fn log(&self, ip: IpAddr, method: &str, path: &str, status: u16) {
        let p: String = path.chars().map(|c| if c.is_control() { '?' } else { c }).take(200).collect();
        (self.env.audit)("share.request", &format!("share={} ip={ip} {method} {p} -> {status}", self.cfg.name));
    }

    fn log_event(&self, ev: &str) {
        (self.env.audit)(ev, &format!("share={}", self.cfg.name));
    }

    fn rate_hit(map: &Mutex<HashMap<IpAddr, Window>>, ip: IpAddr, now: i64, span: i64, max: u32, count: bool) -> bool {
        let Ok(mut m) = map.lock() else { return false };
        if m.len() > 4096 {
            m.retain(|_, w| now - w.start < span);
        }
        let w = m.entry(ip).or_insert(Window { start: now, count: 0 });
        if now - w.start >= span {
            *w = Window { start: now, count: 0 };
        }
        if count {
            w.count += 1;
        }
        w.count > max
    }

    /// The checks that need only the head: who, how often, whether the share still lives, the address and the password.
    pub fn gate(&self, req: &Req, ip: IpAddr) -> Result<Gate, Reply> {
        let deny = |status: u16, msg: &str, why: &str| -> Reply {
            self.log(ip, &req.method, why, status);
            simple(status, msg)
        };
        let local_only = self.cfg.access == Access::Local;
        if (local_only && !is_loopback(ip)) || !is_local_network(ip) {
            return Err(deny(403, "This share is not available from this address.", "-"));
        }
        if local_only {
            let host = req.header("host").unwrap_or("");
            let h = host.rsplit_once(':').map_or(host, |(a, p)| if p.chars().all(|c| c.is_ascii_digit()) && !host.ends_with(']') { a } else { host });
            if !matches!(h, "127.0.0.1" | "localhost" | "[::1]") {
                return Err(deny(403, "This share answers only to its local address.", "-"));
            }
        }
        let now = self.now();
        if Self::rate_hit(&self.fails, ip, now, FAIL_WINDOW, FAIL_MAX, false) || Self::rate_hit(&self.rate, ip, now, RATE_WINDOW, RATE_MAX, true) {
            return Err(deny(429, "Too many requests. Wait a little and try again.", "-").h("Retry-After", "10"));
        }
        let exp = self.expires_at.load(Ordering::Relaxed);
        if exp > 0 && now >= exp {
            return Err(deny(410, "This share has ended.", "-"));
        }
        if !matches!(req.method.as_str(), "GET" | "HEAD" | "PUT" | "POST") {
            return Err(deny(405, "Not allowed.", "-").h("Allow", "GET, HEAD, PUT, POST"));
        }
        let mut segs = Vec::new();
        for raw in req.raw_path.split('/') {
            if raw.is_empty() {
                continue;
            }
            let Some(s) = decode_segment(raw) else { return Err(deny(400, "Bad address.", "-")) };
            segs.push(s);
        }
        if req.raw_path.len() > 2048 || segs.len() > 64 {
            return Err(deny(400, "Bad address.", "-"));
        }
        let mut prefix = String::new();
        if self.cfg.access == Access::Token {
            let ok = segs.len() >= 2 && segs[0] == "s" && ct_eq(segs[1].as_bytes(), self.cfg.token.as_bytes());
            if !ok {
                Self::rate_hit(&self.fails, ip, now, FAIL_WINDOW, FAIL_MAX, true);
                return Err(deny(404, "Not found.", "- (wrong link)"));
            }
            prefix = format!("/s/{}", self.cfg.token);
            segs.drain(..2);
        }
        if self.cfg.access == Access::Password {
            let given = req.header("authorization").unwrap_or("");
            let key: [u8; 32] = Sha256::digest(given.as_bytes()).into();
            let known = self.auth_ok.lock().map(|s| s.contains(&key)).unwrap_or(false);
            let good = known || {
                let pw = given.strip_prefix("Basic ").and_then(|b| b64_decode(b.trim())).and_then(|v| String::from_utf8(v).ok()).and_then(|s| s.split_once(':').map(|(_, p)| p.to_string()));
                pw.is_some_and(|p| crate::sharedfolder::verify_password(&p, &self.cfg.pass_hash))
            };
            if !good {
                Self::rate_hit(&self.fails, ip, now, FAIL_WINDOW, FAIL_MAX, true);
                return Err(deny(401, "A password is needed.", "- (password)").h("WWW-Authenticate", "Basic realm=\"shared folder\", charset=\"UTF-8\""));
            }
            if !known {
                if let Ok(mut s) = self.auth_ok.lock() {
                    if s.len() > 256 {
                        s.clear();
                    }
                    s.insert(key);
                }
            }
        }
        Ok(Gate { ip, rel: segs, prefix })
    }

    /// One whole request (the body already read): gate, serve, log. The peer address is given, never looked up here.
    pub fn respond(&self, req: &Req, ip: IpAddr) -> Reply {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let g = match self.gate(req, ip) {
            Ok(g) => g,
            Err(r) => return r,
        };
        if let Ok(mut c) = self.clients.lock() {
            if c.len() < 10_000 {
                c.insert(ip);
            }
        }
        let r = self.serve(req, &g);
        self.log(ip, &req.method, &format!("/{}", g.rel.join("/")), r.status);
        r
    }

    pub fn serve(&self, req: &Req, g: &Gate) -> Reply {
        let r = match req.method.as_str() {
            "GET" | "HEAD" => self.get(req, g),
            "PUT" => self.put(req, g),
            _ => self.post(req, g),
        };
        self.finish(r)
    }

    fn finish(&self, mut r: Reply) -> Reply {
        let has = |r: &Reply, k: &str| r.header(k).is_some();
        let hdrs: &[(&str, &str)] = &[("X-Content-Type-Options", "nosniff"), ("Referrer-Policy", "no-referrer")];
        for (k, v) in hdrs {
            if !has(&r, k) {
                r.headers.push(((*k).into(), (*v).into()));
            }
        }
        if !self.website() && !has(&r, "X-Frame-Options") {
            r.headers.push(("X-Frame-Options".into(), "DENY".into()));
        }
        r
    }

    // -------------------------------------------------------------- paths

    fn hidden(&self, name: &str) -> bool {
        name.starts_with('.') && (self.website() || !self.cfg.show_hidden)
    }

    /// The real path of `rel` inside the folder, or the status to answer.
    fn resolve(&self, rel: &[String]) -> Result<(PathBuf, std::fs::Metadata), u16> {
        let mut p = self.root.clone();
        let mut total = 0;
        for s in rel {
            if s == "." {
                continue;
            }
            total += s.len() + 1;
            if s == ".." || !clean_part(s) || total > 2048 {
                return Err(400);
            }
            if self.hidden(s) {
                return Err(404);
            }
            p.push(s);
        }
        let real = std::fs::canonicalize(&p).map_err(|_| 404u16)?;
        let inside = real.strip_prefix(&self.root).map_err(|_| 404u16)?;
        if inside.components().any(|c| self.hidden(&c.as_os_str().to_string_lossy())) || self.env.deny.iter().any(|d| real.starts_with(d)) {
            return Err(404);
        }
        let md = std::fs::metadata(&real).map_err(|_| 404u16)?;
        if !(md.is_dir() || md.is_file()) {
            return Err(404);
        }
        Ok((real, md))
    }

    fn href(&self, g: &Gate, rel: &[String], dir: bool) -> String {
        let mut h = g.prefix.clone();
        for s in rel {
            h.push('/');
            h.push_str(&pct_encode(s));
        }
        if dir || rel.is_empty() {
            h.push('/');
        }
        h
    }

    // -------------------------------------------------------------- GET / HEAD

    fn get(&self, req: &Req, g: &Gate) -> Reply {
        let (path, md) = match self.resolve(&g.rel) {
            Ok(x) => x,
            Err(400) => return simple(400, "Bad address."),
            Err(_) => return simple(404, "Not found."),
        };
        if md.is_dir() {
            if !req.raw_path.ends_with('/') && !g.rel.is_empty() {
                return Reply { status: 301, headers: vec![("Location".into(), self.href(g, &g.rel, true)), ("Cache-Control".into(), "no-store".into())], body: Body::Bytes(vec![]) };
            }
            if self.website() {
                let index = path.join("index.html");
                return match std::fs::canonicalize(&index).ok().filter(|r| r.starts_with(&self.root)).and_then(|r| std::fs::metadata(&r).ok().map(|m| (r, m))) {
                    Some((r, m)) if m.is_file() => self.send_file(req, &r, &m, "index.html"),
                    _ => simple(404, "Not found."),
                };
            }
            return self.listing(g, &path);
        }
        let name = g.rel.last().map(String::as_str).unwrap_or("");
        self.send_file(req, &path, &md, name)
    }

    fn send_file(&self, req: &Req, path: &Path, md: &std::fs::Metadata, name: &str) -> Reply {
        let size = md.len();
        let mtime = md.mtime();
        let etag = format!("\"{:x}-{:x}-{:x}\"", size, mtime, md.mtime_nsec());
        let lm = http_date(mtime);
        let active = !self.website() && is_active_content(name);
        let ctype = if active { "application/octet-stream" } else { mime_for(name) };
        let mut hs: Vec<(String, String)> = vec![("Content-Type".into(), ctype.into()), ("Accept-Ranges".into(), "bytes".into()), ("ETag".into(), etag.clone()), ("Last-Modified".into(), lm.clone()), ("Cache-Control".into(), "no-cache".into())];
        if !self.website() {
            let csp = if ctype == "application/pdf" { "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'" } else { "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; sandbox" };
            hs.push(("Content-Security-Policy".into(), csp.into()));
            if active {
                hs.push(("Content-Disposition".into(), disposition(name)));
            }
        }
        // conditional requests
        let inm = req.header("if-none-match");
        let fresh = match inm {
            Some(v) => v.trim() == "*" || v.split(',').any(|t| t.trim().trim_start_matches("W/") == etag),
            None => req.header("if-modified-since").is_some_and(|v| v.trim() == lm),
        };
        if fresh {
            return Reply { status: 304, headers: hs, body: Body::Bytes(vec![]) };
        }
        let file = match File::open(path) {
            Ok(f) if f.metadata().map(|m| m.is_file()).unwrap_or(false) => f,
            _ => return simple(404, "Not found."),
        };
        let range_ok = req.header("if-range").is_none_or(|v| v.trim() == etag || v.trim() == lm);
        if let (Some(r), true) = (req.header("range"), range_ok) {
            if r.trim().starts_with("bytes=") && !r.contains(',') {
                return match crate::mediaserve::parse_range(r, size) {
                    Some((a, b)) => {
                        hs.push(("Content-Range".into(), format!("bytes {a}-{b}/{size}")));
                        Reply { status: 206, headers: hs, body: Body::File { file, start: a, len: b - a + 1 } }
                    }
                    None => simple(416, "That part of the file does not exist.").h("Content-Range", &format!("bytes */{size}")),
                };
            }
        }
        Reply { status: 200, headers: hs, body: Body::File { file, start: 0, len: size } }
    }

    fn listing(&self, g: &Gate, dir: &Path) -> Reply {
        struct E {
            name: String,
            dir: bool,
            size: u64,
            mtime: i64,
        }
        let mut es: Vec<E> = Vec::new();
        let mut truncated = false;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().into_owned();
                if self.hidden(&name) {
                    continue;
                }
                let Ok(real) = std::fs::canonicalize(ent.path()) else { continue };
                if !real.starts_with(&self.root) || self.env.deny.iter().any(|d| real.starts_with(d)) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&real) else { continue };
                if !(md.is_dir() || md.is_file()) {
                    continue;
                }
                if es.len() >= MAX_LIST {
                    truncated = true;
                    break;
                }
                es.push(E { name, dir: md.is_dir(), size: md.len(), mtime: md.mtime() });
            }
        }
        es.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name))));
        let title = format!("/{}", g.rel.join("/"));
        let mut h = String::new();
        h.push_str("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">");
        h.push_str(&format!("<title>{} - {}</title>", esc(&self.cfg.name), esc(&title)));
        h.push_str("<style>:root{color-scheme:light dark}body{font:16px system-ui,sans-serif;margin:0 auto;max-width:56rem;padding:1rem}h1{font-size:1.2rem;word-break:break-all}table{width:100%;border-collapse:collapse}td,th{padding:.55rem .4rem;border-bottom:1px solid #8884;text-align:left}td.n,th.n{text-align:right;white-space:nowrap}a{text-decoration:none;word-break:break-all}a.d{font-weight:600}.m{opacity:.7;font-size:.85rem}form{margin:1rem 0;padding:.6rem;border:1px solid #8886;border-radius:8px}input,button{font:inherit;margin:.2rem 0}@media(max-width:520px){.hide{display:none}}</style>");
        h.push_str(&format!("<h1>{} <span class=m>{}</span></h1>", esc(&self.cfg.name), esc(&title)));
        if !g.rel.is_empty() {
            let mut up = g.rel.clone();
            up.pop();
            h.push_str(&format!("<p><a class=d href=\"{}\">&larr; parent folder</a></p>", esc(&self.href(g, &up, true))));
        }
        h.push_str("<table><tr><th>Name<th class=n>Size<th class=\"n hide\">Modified");
        for e in &es {
            let mut rel = g.rel.clone();
            rel.push(e.name.clone());
            let href = self.href(g, &rel, e.dir);
            h.push_str(&format!("<tr><td><a class={} href=\"{}\"><bdi>{}{}</bdi></a><td class=n>{}<td class=\"n hide\">{}", if e.dir { "d" } else { "f" }, esc(&href), esc(&e.name), if e.dir { "/" } else { "" }, if e.dir { "-".to_string() } else { human_size(e.size) }, list_date(e.mtime)));
        }
        h.push_str("</table>");
        if es.is_empty() {
            h.push_str("<p class=m>This folder is empty.</p>");
        }
        if truncated {
            h.push_str("<p class=m>Only the first 5000 entries are listed.</p>");
        }
        let here = esc(&self.href(g, &g.rel, true));
        if self.cfg.upload {
            h.push_str(&format!("<form method=post action=\"{here}\" enctype=multipart/form-data><b>Upload files</b><br><input type=file name=f multiple required> <button>Upload</button><div class=m>Up to {} MB each. Files with the same name are {}.</div></form>", self.cfg.max_upload_mb, if self.cfg.overwrite { "replaced" } else { "refused" }));
        }
        if self.cfg.upload && self.cfg.mkdir {
            h.push_str(&format!("<form method=post action=\"{here}\"><b>New folder</b><br><input name=mkdir maxlength=100 required> <button>Create</button></form>"));
        }
        let mut r = Reply::bytes(200, "text/html; charset=utf-8", h.into_bytes());
        r.headers.push(("Cache-Control".into(), "no-store".into()));
        r.headers.push(("Content-Security-Policy".into(), "default-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'".into()));
        r
    }

    // -------------------------------------------------------------- upload

    fn can_upload(&self) -> Option<Reply> {
        if self.website() || !self.cfg.upload {
            return Some(simple(405, "Uploading is not switched on for this share.").h("Allow", "GET, HEAD"));
        }
        None
    }

    /// `from_form`: the name came from a browser form, which may send a path; only its last part counts.
    fn upload_name(&self, name: &str, from_form: bool) -> Result<String, Reply> {
        let base = if from_form { name.rsplit(['/', '\\']).next().unwrap_or("").trim() } else { name };
        if !clean_part(base) || base.starts_with('.') || base.len() > 200 {
            return Err(simple(400, "That file name cannot be used."));
        }
        Ok(base.to_string())
    }

    fn store(&self, dir: &Path, name: &str, data: &[u8]) -> Result<(), Reply> {
        if data.len() as u64 > self.max_upload() {
            return Err(simple(413, "The file is larger than this share accepts."));
        }
        match (self.env.free_space)(dir) {
            Some(free) if free >= data.len() as u64 + DISK_RESERVE => {}
            _ => return Err(simple(507, "There is not enough free space.")),
        }
        let target = dir.join(name);
        match std::fs::symlink_metadata(&target) {
            Ok(m) => {
                if !self.cfg.overwrite || !m.is_file() {
                    return Err(simple(409, "A file or folder with that name exists already."));
                }
                let tmp = dir.join(format!(".upload-{}", crate::sharedfolder::random_hex(6)));
                let res = (|| -> std::io::Result<()> {
                    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
                    f.write_all(data)?;
                    f.sync_all()?;
                    std::fs::rename(&tmp, &target)
                })();
                if res.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(simple(500, "The file could not be written."));
                }
                Ok(())
            }
            Err(_) => {
                let res = (|| -> std::io::Result<()> {
                    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(&target)?;
                    f.write_all(data)
                })();
                match res {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(simple(409, "A file or folder with that name exists already.")),
                    Err(_) => {
                        let _ = std::fs::remove_file(&target);
                        Err(simple(500, "The file could not be written."))
                    }
                }
            }
        }
    }

    fn put(&self, req: &Req, g: &Gate) -> Reply {
        if let Some(r) = self.can_upload() {
            return r;
        }
        let Some((name, parent)) = g.rel.split_last() else { return simple(400, "Give a file name.") };
        let name = match self.upload_name(name, false) {
            Ok(n) => n,
            Err(r) => return r,
        };
        let (dir, md) = match self.resolve(parent) {
            Ok(x) => x,
            Err(400) => return simple(400, "Bad address."),
            Err(_) => return simple(404, "Not found."),
        };
        if !md.is_dir() {
            return simple(404, "Not found.");
        }
        match self.store(&dir, &name, &req.body) {
            Ok(()) => Reply::bytes(201, "text/plain; charset=utf-8", b"created\n".to_vec()),
            Err(r) => r,
        }
    }

    fn post(&self, req: &Req, g: &Gate) -> Reply {
        if let Some(r) = self.can_upload() {
            return r;
        }
        let (dir, md) = match self.resolve(&g.rel) {
            Ok(x) => x,
            Err(400) => return simple(400, "Bad address."),
            Err(_) => return simple(404, "Not found."),
        };
        if !md.is_dir() {
            return simple(404, "Not found.");
        }
        let ctype = req.header("content-type").unwrap_or("");
        let back = Reply { status: 303, headers: vec![("Location".into(), self.href(g, &g.rel, true)), ("Cache-Control".into(), "no-store".into())], body: Body::Bytes(vec![]) };
        if let Some(b) = ctype.strip_prefix("multipart/form-data").and_then(|_| ctype.split(';').filter_map(|p| p.trim().strip_prefix("boundary=")).next()) {
            let boundary = b.trim_matches('"');
            let parts = match multipart(&req.body, boundary) {
                Some(p) => p,
                None => return simple(400, "The upload could not be read."),
            };
            let mut done = 0;
            for (fname, data) in parts {
                let name = match self.upload_name(&fname, true) {
                    Ok(n) => n,
                    Err(r) => return r,
                };
                if let Err(r) = self.store(&dir, &name, data) {
                    return r;
                }
                done += 1;
            }
            return if done == 0 { simple(400, "No file was sent.") } else { back };
        }
        if ctype.starts_with("application/x-www-form-urlencoded") {
            if !self.cfg.mkdir {
                return simple(405, "Making folders is not switched on for this share.");
            }
            let body = String::from_utf8_lossy(&req.body);
            let name = body.split('&').find_map(|kv| kv.strip_prefix("mkdir=")).and_then(|v| decode_segment(&v.replace('+', " ")));
            let Some(name) = name else { return simple(400, "Give a folder name.") };
            let name = name.trim().to_string();
            if !clean_part(&name) || name.starts_with('.') || name.len() > 100 {
                return simple(400, "That folder name cannot be used.");
            }
            return match std::fs::create_dir(dir.join(&name)) {
                Ok(()) => back,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => simple(409, "A file or folder with that name exists already."),
                Err(_) => simple(500, "The folder could not be made."),
            };
        }
        simple(400, "Unsupported upload.")
    }
}

fn disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && !matches!(c, '"' | '\\' | ';' | '%') { c } else { '_' }).collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", pct_encode(name))
}

/// The parts of a `multipart/form-data` body that carry a file: (file name, content).
fn multipart<'a>(body: &'a [u8], boundary: &str) -> Option<Vec<(String, &'a [u8])>> {
    if boundary.is_empty() || boundary.len() > 70 || boundary.bytes().any(|b| b < 0x20 || b == b'"') {
        return None;
    }
    let delim = format!("--{boundary}").into_bytes();
    let find = |hay: &[u8], needle: &[u8], from: usize| -> Option<usize> { hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from) };
    let mut out = Vec::new();
    let mut pos = find(body, &delim, 0)? + delim.len();
    loop {
        if body.get(pos..pos + 2) == Some(b"--") {
            return Some(out);
        }
        if body.get(pos..pos + 2) != Some(b"\r\n") {
            return None;
        }
        pos += 2;
        let hend = find(body, b"\r\n\r\n", pos)?;
        let head = std::str::from_utf8(&body[pos..hend]).ok()?;
        let start = hend + 4;
        let next = find(body, &[b"\r\n".as_slice(), &delim].concat(), start)?;
        for l in head.split("\r\n") {
            if l.to_ascii_lowercase().starts_with("content-disposition:") {
                if let Some(f) = disposition_param(l, "filename") {
                    if out.len() >= 50 {
                        return None;
                    }
                    out.push((f, &body[start..next]));
                }
            }
        }
        pos = next + 2 + delim.len();
    }
}

fn disposition_param(line: &str, key: &str) -> Option<String> {
    let (mut cur, mut inq, mut esc_next) = (String::new(), false, false);
    let mut parts = Vec::new();
    for c in line.chars() {
        if esc_next {
            cur.push(c);
            esc_next = false;
        } else if inq && c == '\\' {
            esc_next = true;
        } else if c == '"' {
            inq = !inq;
            cur.push(c);
        } else if c == ';' && !inq {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts.iter().find_map(|p| p.trim().strip_prefix(key).and_then(|r| r.trim_start().strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string()))
}

// ------------------------------------------------------------------ the sockets

pub struct Running {
    pub site: Arc<Site>,
    pub addr: SocketAddr,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Is the listener still up? (A share whose time ran out closes its own port.)
    pub fn alive(&self) -> bool {
        self.join.as_ref().is_some_and(|j| !j.is_finished())
    }

    /// Stop accepting, and let running transfers end. The port is free again when this returns.
    pub fn stop(&mut self) {
        self.site.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Where a share listens: this machine only for the local mode, else every interface or the one address chosen.
pub fn bind_addr(cfg: &ShareCfg) -> Result<SocketAddr, String> {
    let ip: IpAddr = if cfg.access == Access::Local {
        Ipv4Addr::LOCALHOST.into()
    } else if cfg.bind.is_empty() || cfg.bind == "all" {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        cfg.bind.parse::<IpAddr>().map_err(|_| "the address to listen on is not valid".to_string())?
    };
    if let IpAddr::V6(a) = ip {
        if a == Ipv6Addr::UNSPECIFIED {
            return Err("IPv6 is not supported".into());
        }
    }
    Ok(SocketAddr::new(ip, cfg.port))
}

pub fn start(cfg: ShareCfg, env: Env) -> Result<Running, String> {
    let addr = bind_addr(&cfg)?;
    let listener = bind_retry(addr).map_err(|e| if e.kind() == std::io::ErrorKind::AddrInUse { format!("port {} is in use by something else; edit the share and choose another port", cfg.port) } else { format!("could not listen on {addr}: {e}") })?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    let site = Arc::new(Site::new(cfg, env)?);
    let s2 = site.clone();
    let join = std::thread::Builder::new().name("share".into()).spawn(move || accept_loop(listener, s2)).map_err(|e| e.to_string())?;
    Ok(Running { site, addr: local, join: Some(join) })
}

fn accept_loop(l: TcpListener, site: Arc<Site>) {
    while !site.stop.load(Ordering::Relaxed) {
        let exp = site.expires_at.load(Ordering::Relaxed);
        if exp > 0 && site.now() >= exp {
            site.log_event("share.expired");
            break;
        }
        match l.accept() {
            Ok((mut s, _)) => {
                let _ = s.set_nonblocking(false);
                if site.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    site.active.fetch_sub(1, Ordering::SeqCst);
                    let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
                    let _ = s.write_all(&simple(503, "Too many connections.").to_vec(false));
                    continue;
                }
                let site2 = site.clone();
                let ok = std::thread::Builder::new().name("share-conn".into()).spawn(move || {
                    let _ = serve_conn(&site2, s);
                    site2.active.fetch_sub(1, Ordering::SeqCst);
                });
                if ok.is_err() {
                    site.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn reply_and_close(site: &Site, s: &mut TcpStream, r: Reply, head_only: bool) {
    let r = site.finish(r);
    let _ = r.write_to(s, head_only, &site.stop);
    let _ = s.flush();
    let _ = s.shutdown(Shutdown::Both);
}

fn serve_conn(site: &Arc<Site>, mut s: TcpStream) -> std::io::Result<()> {
    let ip = s.peer_addr()?.ip();
    s.set_write_timeout(Some(Duration::from_secs(15)))?;
    let started = Instant::now();
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let (mut req, body_at) = loop {
        match parse_head(&buf) {
            Head::Done(r, at) => break (r, at),
            Head::Bad(c, m) => {
                reply_and_close(site, &mut s, simple(c, m), false);
                return Ok(());
            }
            Head::Incomplete => {}
        }
        let left = HEAD_TIMEOUT.saturating_sub(started.elapsed());
        if left.is_zero() {
            reply_and_close(site, &mut s, simple(408, "The request took too long."), false);
            return Ok(());
        }
        s.set_read_timeout(Some(left))?;
        match s.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => {
                reply_and_close(site, &mut s, simple(408, "The request took too long."), false);
                return Ok(());
            }
        }
    };
    let head_only = req.method == "HEAD";
    let writes = matches!(req.method.as_str(), "PUT" | "POST");
    if writes {
        // who is allowed is decided before a byte of the body is read
        site.requests.fetch_add(1, Ordering::Relaxed);
        let gate = site.gate(&req, ip);
        let g = match gate {
            Ok(g) => g,
            Err(r) => {
                reply_and_close(site, &mut s, r, false);
                return Ok(());
            }
        };
        if let Some(r) = site.can_upload() {
            reply_and_close(site, &mut s, r, false);
            return Ok(());
        }
        let len = match req.header("content-length").map(|v| v.parse::<u64>()) {
            Some(Ok(n)) => n,
            Some(Err(_)) => {
                reply_and_close(site, &mut s, simple(400, "Bad content length."), false);
                return Ok(());
            }
            None => {
                reply_and_close(site, &mut s, simple(411, "A content length is needed."), false);
                return Ok(());
            }
        };
        if len > site.max_upload() + 64 * 1024 {
            reply_and_close(site, &mut s, simple(413, "The upload is larger than this share accepts."), false);
            return Ok(());
        }
        if site.uploads.fetch_add(1, Ordering::SeqCst) >= MAX_UPLOADS_AT_ONCE {
            site.uploads.fetch_sub(1, Ordering::SeqCst);
            reply_and_close(site, &mut s, simple(503, "Other uploads are running. Try again in a moment."), false);
            return Ok(());
        }
        if req.header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
            let _ = s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        let mut body: Vec<u8> = buf[body_at..].to_vec();
        let mut ok = true;
        while (body.len() as u64) < len {
            if site.stop.load(Ordering::Relaxed) {
                ok = false;
                break;
            }
            s.set_read_timeout(Some(BODY_IDLE))?;
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => {
                    ok = false;
                    break;
                }
                Ok(n) => body.extend_from_slice(&chunk[..n]),
            }
        }
        if ok {
            body.truncate(len as usize);
            req.body = body;
            if let Ok(mut c) = site.clients.lock() {
                c.insert(ip);
            }
            let r = site.serve(&req, &g);
            site.log(ip, &req.method, &format!("/{}", g.rel.join("/")), r.status);
            reply_and_close(site, &mut s, r, false);
        }
        site.uploads.fetch_sub(1, Ordering::SeqCst);
        return Ok(());
    }
    let r = site.respond(&req, ip);
    reply_and_close(site, &mut s, r, head_only);
    Ok(())
}

/// Bind a listener. A port that was given up a moment ago (a share stopped and started again, a new secret link) can still be
/// held for a few milliseconds by a child process that was forked while the old listener was open and has not run its `exec`
/// yet; "address in use" for that long is not a real conflict, so try again briefly before reporting it.
pub(crate) fn bind_retry(addr: impl std::net::ToSocketAddrs + Copy) -> std::io::Result<TcpListener> {
    let mut tries = 0;
    loop {
        match TcpListener::bind(addr) {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            r => return r,
        }
    }
}
