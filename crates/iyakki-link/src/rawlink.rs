//! A raw link to a tenant's web desktop for `iyakki-connect serve`: sign in (password or device key), open the WebSocket, and hand the
//! messages over exactly as they arrive. Nothing is decoded or re-encoded here: the picture stream goes to the local page as it is.
//!
//! The certificate is pinned like every other connection of this crate (`cloudclient::tls_open`): a different one is a hard stop before
//! a byte of application data, so a password or a signature is never sent to the wrong server.
//!
//! Also the tenant's file routes (`/api/v1/files`, `iyakki-agent/src/webservice/files.rs`) for `--share`: a bearer token from
//! `/api/v1/login` (a password; the key sign-in is for the web desktop only).

use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::cloudclient::{clean, error_text, http, masked_frame, read_head, tls_open, ConnErr, FrameReader, Msg, Target, Tls, COOKIE, IO_TIMEOUT, MAX_MESSAGE};
use crate::sigscheme::DeviceKey;

/// How a person proves who they are. Held by the program only; the page never sees it.
pub enum Auth {
    Password { user: String, password: Zeroizing<String> },
    Key { user: String, key: DeviceKey },
}

impl Auth {
    pub fn user(&self) -> &str {
        match self {
            Auth::Password { user, .. } | Auth::Key { user, .. } => user,
        }
    }
}

/// Why a sign-in or an open did not work.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The server said no to the account, password or key.
    Refused(String),
    /// The certificate is not the pinned one. Nothing was sent.
    PinChanged(String),
    Other(String),
}

impl LinkError {
    pub fn text(&self) -> &str {
        match self {
            LinkError::Refused(m) | LinkError::PinChanged(m) | LinkError::Other(m) => m,
        }
    }
}

fn from_conn(e: ConnErr) -> LinkError {
    match e {
        ConnErr::Refused(m) => LinkError::Refused(m),
        ConnErr::Mismatch(m) => LinkError::PinChanged(m),
        other => LinkError::Other(other.text()),
    }
}

struct Session {
    cookie: String,
    csrf: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn sign_in(t: &Target, auth: &Auth) -> Result<Session, ConnErr> {
    let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT)?;
    let (path, extra, body): (&str, Vec<(String, String)>, Vec<u8>) = match auth {
        Auth::Password { user, password } => ("/api/login", vec![("Content-Type".into(), "application/json".into())], json!({"user": user, "password": password.as_str()}).to_string().into_bytes()),
        Auth::Key { user, key } => {
            let h = key.sign_request(user, "POST", "/api/login-key", now_secs(), b"").map_err(ConnErr::Fatal)?;
            ("/api/login-key", vec![("Authorization".into(), h)], Vec::new())
        }
    };
    let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (status, headers, body) = http(&mut tls, &t.addr, "POST", path, &extra, &body)?;
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
        401 | 403 => Err(ConnErr::Refused(error_text(&body, status))),
        s => Err(ConnErr::Http(s, error_text(&body, s))),
    }
}

/// One message from the tenant, untouched.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// An open WebSocket to the tenant's desktop.
pub struct RawWs {
    tls: Tls,
    reader: FrameReader,
    session: Session,
    target: Target,
    closed: bool,
}

impl RawWs {
    /// Sign in and open the WebSocket. Blocks for a few seconds at most.
    pub fn open(t: &Target, auth: &Auth) -> Result<RawWs, LinkError> {
        let session = sign_in(t, auth).map_err(from_conn)?;
        let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT).map_err(from_conn)?;
        let mut key = [0u8; 16];
        iyakki_sec::random_fill(&mut key).map_err(|_| LinkError::Other("no randomness".into()))?;
        let key = crate::wsproto::base64(&key);
        let req = format!(
            "GET /ws?csrf={} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nCookie: {}\r\n\r\n",
            session.csrf, t.addr, session.cookie
        );
        tls.write_all(req.as_bytes()).and_then(|_| tls.flush()).map_err(|e| LinkError::Other(format!("sending to the server: {e}")))?;
        let (status, headers, rest) = read_head(&mut tls).map_err(from_conn)?;
        if status != 101 {
            let mut body = rest;
            let mut chunk = [0u8; 1024];
            if let Ok(n) = tls.read(&mut chunk) {
                body.extend_from_slice(&chunk[..n]);
            }
            return Err(LinkError::Other(error_text(&body, status)));
        }
        if !headers.iter().any(|(k, v)| k == "sec-websocket-accept" && *v == crate::wsproto::accept_key(&key)) {
            return Err(LinkError::Other("the server's WebSocket answer is wrong".into()));
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_millis(5)));
        let _ = tls.sock.set_write_timeout(Some(Duration::from_secs(5)));
        let mut reader = FrameReader::new(MAX_MESSAGE);
        reader.push(&rest);
        Ok(RawWs { tls, reader, session, target: t.clone(), closed: false })
    }

    /// How long `poll` may wait for bytes (1 ms to 50 ms): short while the link is busy so that input sent meanwhile is not held back.
    pub fn set_poll_wait(&mut self, d: Duration) {
        let _ = self.tls.sock.set_read_timeout(Some(d.clamp(Duration::from_millis(1), Duration::from_millis(50))));
    }

    pub fn send_text(&mut self, s: &str) -> Result<(), String> {
        self.write(&masked_frame(0x1, s.as_bytes()))
    }

    pub fn send_binary(&mut self, b: &[u8]) -> Result<(), String> {
        self.write(&masked_frame(0x2, b))
    }

    fn write(&mut self, f: &[u8]) -> Result<(), String> {
        self.tls.write_all(f).and_then(|_| self.tls.flush()).map_err(|e| format!("sending failed: {e}"))
    }

    /// The next message, waiting up to a few milliseconds for bytes. `Ok(None)`: nothing yet. `Err`: the link ended (the text says why).
    pub fn poll(&mut self) -> Result<Option<Frame>, String> {
        loop {
            match self.reader.next() {
                Ok(Some(Msg::Text(t))) => return Ok(Some(Frame::Text(t))),
                Ok(Some(Msg::Binary(b))) => return Ok(Some(Frame::Binary(b))),
                Ok(Some(Msg::Ping(p))) => self.write(&masked_frame(0xA, &p))?,
                Ok(Some(Msg::Pong)) => {}
                Ok(Some(Msg::Close)) => {
                    self.closed = true;
                    return Err("the server closed the connection".into());
                }
                Ok(None) => break,
                Err(e) => return Err(format!("bad data from the server: {e}")),
            }
        }
        let mut buf = vec![0u8; 64 * 1024];
        match self.tls.read(&mut buf) {
            Ok(0) => Err("the server closed the connection".into()),
            Ok(n) => {
                self.reader.push(&buf[..n]);
                Ok(None)
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => Ok(None),
            Err(e) => Err(format!("reading failed: {e}")),
        }
    }

    /// Close the WebSocket and sign out of the tenant (best effort, a few seconds at most).
    pub fn close(mut self) {
        if !self.closed {
            let _ = self.write(&masked_frame(0x8, &[]));
        }
        if let Ok((mut tls, _)) = tls_open(&self.target.addr, Some(self.target.pin), Duration::from_secs(3)) {
            let _ = http(&mut tls, &self.target.addr, "POST", "/api/logout", &[("Cookie", &self.session.cookie), ("X-Csrf", &self.session.csrf)], b"");
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    }
}

/// Sign in with a password and enrol `public` (an `ssh-ed25519` line) as a device key of that account
/// (`POST /api/device-keys`, `iyakki-remote/src/devicekeys.rs`). Returns the key's fingerprint as the tenant stored it. The session
/// is ended again afterwards.
pub fn enroll_key(t: &Target, user: &str, password: &str, public: &str) -> Result<String, LinkError> {
    let auth = Auth::Password { user: user.to_string(), password: Zeroizing::new(password.to_string()) };
    let session = sign_in(t, &auth).map_err(from_conn)?;
    let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT).map_err(from_conn)?;
    let body = json!({"key": public}).to_string();
    let (status, _, resp) = http(&mut tls, &t.addr, "POST", "/api/device-keys", &[("Content-Type", "application/json"), ("Cookie", &session.cookie), ("X-Csrf", &session.csrf)], body.as_bytes()).map_err(from_conn)?;
    if let Ok((mut out, _)) = tls_open(&t.addr, Some(t.pin), Duration::from_secs(3)) {
        let _ = http(&mut out, &t.addr, "POST", "/api/logout", &[("Cookie", &session.cookie), ("X-Csrf", &session.csrf)], b"");
    }
    match status {
        200 => serde_json::from_slice::<Value>(&resp).ok().and_then(|v| v["fingerprint"].as_str().map(str::to_string)).ok_or_else(|| LinkError::Other("the server gave no fingerprint".into())),
        401 | 403 => Err(LinkError::Refused(error_text(&resp, status))),
        s => Err(LinkError::Other(error_text(&resp, s))),
    }
}

// ------------------------------------------------------------------------------------------------------------------ files

/// One entry of a remote folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    /// `file`, `dir` or `link`.
    pub kind: String,
    pub size: u64,
}

/// One tenant of the signed-in person, as the server listed it (`GET /api/v1/tenants`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantInfo {
    pub id: String,
    pub name: String,
    /// The person's role in it, as the server names it.
    pub role: String,
    pub current: bool,
}

/// The tenant's `/api/v1/files` routes with a bearer token. Every call is its own pinned connection.
pub struct FilesApi {
    target: Target,
    token: Zeroizing<String>,
}

/// Percent-encode a query value (everything but unreserved characters).
pub fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl FilesApi {
    pub fn login(t: &Target, user: &str, password: &str) -> Result<FilesApi, LinkError> {
        let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT).map_err(from_conn)?;
        let body = json!({"name": user, "password": password}).to_string();
        let (status, _, body) = http(&mut tls, &t.addr, "POST", "/api/v1/login", &[("Content-Type", "application/json")], body.as_bytes()).map_err(from_conn)?;
        match status {
            200 => {
                let token = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["token"].as_str().map(str::to_string)).ok_or_else(|| LinkError::Other("the server gave no token".into()))?;
                Ok(FilesApi { target: t.clone(), token: Zeroizing::new(token) })
            }
            401 | 403 => Err(LinkError::Refused(error_text(&body, status))),
            s => Err(LinkError::Other(error_text(&body, s))),
        }
    }

    fn call(&self, method: &str, path_query: &str, extra: &[(&str, &str)], body: &[u8], cap: usize) -> Result<(u16, Vec<u8>), LinkError> {
        let (mut tls, _) = tls_open(&self.target.addr, Some(self.target.pin), IO_TIMEOUT).map_err(from_conn)?;
        let auth = format!("Bearer {}", self.token.as_str());
        let mut hdrs: Vec<(&str, &str)> = vec![("Authorization", &auth)];
        hdrs.extend_from_slice(extra);
        // the shared `http` caps bodies at 1 MiB; downloads read the rest here
        let mut req = format!("{method} {path_query} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n", self.target.addr, body.len());
        for (k, v) in &hdrs {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        let mut out = req.into_bytes();
        out.extend_from_slice(body);
        tls.write_all(&out).and_then(|_| tls.flush()).map_err(|e| LinkError::Other(format!("sending to the server: {e}")))?;
        let (status, headers, mut got) = read_head(&mut tls).map_err(from_conn)?;
        let want = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
        if want > cap {
            return Err(LinkError::Other(format!("the file is larger than the {} MiB limit", cap >> 20)));
        }
        let mut chunk = vec![0u8; 64 * 1024];
        while got.len() < want {
            match tls.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(LinkError::Other(format!("reading from the server: {e}"))),
            }
        }
        if got.len() < want {
            return Err(LinkError::Other("the server ended the file early".into()));
        }
        got.truncate(want);
        Ok((status, got))
    }

    fn fail(status: u16, body: &[u8]) -> LinkError {
        LinkError::Other(format!("{} ({status})", error_text(body, status)))
    }

    pub fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, LinkError> {
        let (s, b) = self.call("GET", &format!("/api/v1/files?path={}", pct(path)), &[], b"", 4 << 20)?;
        if s != 200 {
            return Err(Self::fail(s, &b));
        }
        let v: Value = serde_json::from_slice(&b).map_err(|_| LinkError::Other("the list is not understood".into()))?;
        Ok(v["entries"]
            .as_array()
            .map(|a| a.iter().map(|e| RemoteEntry { name: clean(e["name"].as_str().unwrap_or("")), kind: clean(e["kind"].as_str().unwrap_or("file")), size: e["size"].as_u64().unwrap_or(0) }).collect())
            .unwrap_or_default())
    }

    /// The tenants this signed-in person may use, and which one the session is in (the server decides; nothing is asked of other tenants).
    pub fn tenants(&self) -> Result<Vec<TenantInfo>, LinkError> {
        let (s, b) = self.call("GET", "/api/v1/tenants", &[], b"", 1 << 20)?;
        if s == 404 {
            return Err(LinkError::Other("this server has no tenant list (it is older than the tenant switcher)".into()));
        }
        if s != 200 {
            return Err(Self::fail(s, &b));
        }
        let v: Value = serde_json::from_slice(&b).map_err(|_| LinkError::Other("the tenant list is not understood".into()))?;
        let cur = v["current"].as_str().unwrap_or("");
        Ok(v["tenants"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let id = clean(e["id"].as_str()?);
                        Some(TenantInfo { name: clean(e["name"].as_str().unwrap_or("")), role: clean(e["role"].as_str().unwrap_or("")), current: e["current"].as_bool().unwrap_or(false) || id == cur, id })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn download(&self, path: &str, max: usize) -> Result<Vec<u8>, LinkError> {
        let (s, b) = self.call("GET", &format!("/api/v1/files/download?path={}", pct(path)), &[], b"", max)?;
        if s != 200 {
            return Err(Self::fail(s, &b));
        }
        Ok(b)
    }

    pub fn upload(&self, path: &str, bytes: &[u8]) -> Result<(), LinkError> {
        let sum = {
            use sha2::{Digest, Sha256};
            crate::sigscheme::hex(&Sha256::digest(bytes))
        };
        let (s, b) = self.call("PUT", &format!("/api/v1/files/upload?path={}", pct(path)), &[("X-Content-SHA256", &sum), ("Content-Type", "application/octet-stream")], bytes, 1 << 20)?;
        if s != 200 {
            return Err(Self::fail(s, &b));
        }
        Ok(())
    }
}
