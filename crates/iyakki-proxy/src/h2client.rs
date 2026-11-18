//! HTTP/2 connected to the browser's HTTP client (F-BROX-10, RFC 9113 over RFC 7301 ALPN): one connection per host that many requests
//! share, with a fall back to HTTP/1.1 for hosts that do not speak `h2`.
//!
//! The protocol itself is [`crate::h2::Conn`] (no sockets). Here a [`H2Connection`] gives it a [`Transport`] and lets any number of
//! threads send requests on it at the same time: whoever holds the lock reads a little (a few milliseconds at most) and hands what
//! arrives to the stream it belongs to, so the answer to one request is never stuck behind another's. An [`H2Pool`] keeps the
//! connections per host and asks a dialer for a new one when none is usable; the dialer says whether the host chose `h2`.
//!
//! A host that answers `http/1.1` is remembered for ten minutes so the browser does not try again on every file. When a request
//! cannot be sent on HTTP/2 (the connection was closed by the server, or it will take no more streams) and nothing of the answer has
//! arrived yet, a GET or HEAD is given back to the caller to be sent the old way.

use crate::h2::{Conn, Event, H2Error};
use crate::hpack::Header;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long one pump of the connection waits for bytes.
const POLL: Duration = Duration::from_millis(5);
/// A connection that has carried nothing for this long is asked with a PING before it is used again (a NAT or a server may have dropped it).
const IDLE_PING: Duration = Duration::from_secs(30);
/// How long the answer to such a PING is waited for.
const PING_WAIT: Duration = Duration::from_secs(2);
/// A host that answered `http/1.1` is not asked for `h2` again for this long.
pub const NO_H2_FOR: Duration = Duration::from_secs(600);
/// Connections to one host (more than one only when the server's stream limit is reached).
pub const MAX_CONNS_PER_HOST: usize = 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the connection needs from a socket (plain or encrypted).
pub trait Transport: Send {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Reads what is there, waiting at most `wait`: `Ok(None)` when nothing came, `Ok(Some(0))` at the end of the stream.
    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<Option<usize>>;
}

impl Transport for std::net::TcpStream {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.write_all(bytes)?;
        self.flush()
    }
    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<Option<usize>> {
        use std::io::Read;
        self.set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        match self.read(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "https")]
impl Transport for rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream> {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.write_all(bytes)?;
        self.flush()
    }
    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<Option<usize>> {
        use std::io::Read;
        self.sock.set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        match self.read(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            // a server that closes without the TLS goodbye after its last answer
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(Some(0)),
            Err(e) => Err(e),
        }
    }
}

/// One request to send.
#[derive(Clone, Debug, Default)]
pub struct H2Request {
    pub method: String,
    pub authority: String,
    /// Path and query.
    pub path: String,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    /// The answer is refused above this many bytes.
    pub max_body: usize,
    /// A video or song (by its content type) is cut at this many bytes and the rest of the stream cancelled; 0 reads it all.
    pub media_cap: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct H2Response {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum H2Fail {
    /// This connection cannot carry the request (closed, full, said goodbye); nothing of an answer arrived, so a GET or HEAD may go
    /// another way.
    Unusable(String),
    /// The request failed after it was sent (a reset, a timeout, an answer too large).
    Failed(String),
}

impl std::fmt::Display for H2Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H2Fail::Unusable(s) | H2Fail::Failed(s) => write!(f, "{s}"),
        }
    }
}

struct Part {
    status: u16,
    headers: Vec<Header>,
    body: Vec<u8>,
    cap: usize,
    cut: bool,
}

struct Inner {
    io: Box<dyn Transport>,
    conn: Conn,
    parts: HashMap<u32, Part>,
    done: HashMap<u32, Result<H2Response, H2Fail>>,
    caps: HashMap<u32, (usize, usize)>,
    broken: Option<String>,
    /// The last time a request was sent or something came in.
    last_active: Instant,
    /// Answers to our PINGs that arrived.
    acked: Vec<[u8; 8]>,
    pings: u64,
}

/// One HTTP/2 connection, shared by every thread that has a request for its host.
pub struct H2Connection {
    inner: Mutex<Inner>,
}

impl H2Connection {
    /// Starts the connection: the preface and settings are sent now.
    pub fn open(mut io: Box<dyn Transport>) -> io::Result<H2Connection> {
        let mut conn = Conn::new();
        io.send(&conn.take_output())?;
        Ok(H2Connection { inner: Mutex::new(Inner { io, conn, parts: HashMap::new(), done: HashMap::new(), caps: HashMap::new(), broken: None, last_active: Instant::now(), acked: Vec::new(), pings: 0 }) })
    }

    /// Whether another request may be started on it now.
    pub fn usable(&self) -> bool {
        let g = lock(&self.inner);
        g.broken.is_none() && g.conn.can_start()
    }

    pub fn is_broken(&self) -> bool {
        lock(&self.inner).broken.is_some()
    }

    pub fn open_streams(&self) -> usize {
        lock(&self.inner).conn.open_streams()
    }

    /// How long nothing has been sent or received on it.
    pub fn idle_for(&self) -> Duration {
        lock(&self.inner).last_active.elapsed()
    }

    /// Keep-alive: send a PING and wait up to `wait` for its answer. A connection that does not answer (or is found closed) is marked
    /// broken and false is returned, so the caller dials a new one before a request is lost on a dead socket.
    pub fn ping_check(&self, wait: Duration) -> bool {
        let (token, end) = {
            let mut g = lock(&self.inner);
            if g.broken.is_some() {
                return false;
            }
            g.pings += 1;
            let token = g.pings.to_be_bytes();
            g.conn.ping(token);
            if let Err(e) = g.flush() {
                g.break_with(&format!("the connection failed while checking it ({e})"));
                return false;
            }
            (token, Instant::now() + wait)
        };
        loop {
            let mut g = lock(&self.inner);
            if g.broken.is_some() {
                return false;
            }
            if let Some(i) = g.acked.iter().position(|a| *a == token) {
                g.acked.remove(i);
                g.last_active = Instant::now();
                return true;
            }
            if Instant::now() > end {
                g.break_with("the server did not answer a PING");
                return false;
            }
            g.pump();
            drop(g);
            std::thread::yield_now();
        }
    }

    /// Sends one request and waits for its answer (other threads' requests go on in the meantime).
    pub fn fetch(&self, req: &H2Request, deadline: Instant) -> Result<H2Response, H2Fail> {
        let id = {
            let mut g = lock(&self.inner);
            if let Some(why) = &g.broken {
                return Err(H2Fail::Unusable(why.clone()));
            }
            g.last_active = Instant::now();
            let body = (!req.body.is_empty()).then(|| req.body.clone());
            let id = match g.conn.request(&req.method, "https", &req.authority, &req.path, &req.headers, body) {
                Ok(id) => id,
                Err(H2Error::Busy(w)) => return Err(H2Fail::Unusable(w)),
                Err(H2Error::Protocol(w)) => {
                    g.break_with(&w);
                    return Err(H2Fail::Unusable(w));
                }
            };
            g.caps.insert(id, (req.max_body, if req.method.eq_ignore_ascii_case("GET") { req.media_cap } else { 0 }));
            if let Err(e) = g.flush() {
                let w = format!("the connection failed while sending ({e})");
                g.break_with(&w);
                return Err(H2Fail::Unusable(w));
            }
            id
        };
        loop {
            let mut g = lock(&self.inner);
            if let Some(r) = g.done.remove(&id) {
                return r;
            }
            if let Some(why) = g.broken.clone() {
                let seen = g.parts.remove(&id).is_some();
                g.caps.remove(&id);
                return Err(if seen { H2Fail::Failed(why) } else { H2Fail::Unusable(why) });
            }
            if Instant::now() > deadline {
                g.conn.cancel(id);
                let _ = g.flush();
                g.parts.remove(&id);
                g.caps.remove(&id);
                return Err(H2Fail::Failed("the server did not answer in time".into()));
            }
            g.pump();
            drop(g);
            std::thread::yield_now();
        }
    }
}

impl Inner {
    fn flush(&mut self) -> io::Result<()> {
        let out = self.conn.take_output();
        if out.is_empty() {
            return Ok(());
        }
        self.io.send(&out)
    }

    fn break_with(&mut self, why: &str) {
        if self.broken.is_none() {
            self.broken = Some(why.to_string());
        }
        // the requests that had not started to answer may be sent another way; the others failed
        let ids: Vec<u32> = self.caps.keys().copied().collect();
        for id in ids {
            if self.done.contains_key(&id) {
                continue;
            }
            let r = if self.parts.contains_key(&id) { H2Fail::Failed(why.to_string()) } else { H2Fail::Unusable(why.to_string()) };
            self.parts.remove(&id);
            self.done.insert(id, Err(r));
        }
        self.caps.clear();
    }

    /// Reads once (a few milliseconds at most) and gives what came to the streams.
    fn pump(&mut self) {
        let mut buf = [0u8; 16 * 1024];
        match self.io.recv(&mut buf, POLL) {
            Ok(None) => {}
            Ok(Some(0)) => self.break_with("the server closed the connection"),
            Ok(Some(n)) => match { self.last_active = Instant::now(); self.conn.feed(&buf[..n]) } {
                Ok(events) => {
                    for e in events {
                        self.event(e);
                    }
                    if let Err(e) = self.flush() {
                        self.break_with(&format!("the connection failed ({e})"));
                    }
                }
                Err(e) => {
                    let _ = self.flush();
                    self.break_with(&format!("the server broke the HTTP/2 rules ({e})"));
                }
            },
            Err(e) => self.break_with(&format!("the connection failed ({e})")),
        }
    }

    fn finish(&mut self, id: u32) {
        if let Some(p) = self.parts.remove(&id) {
            self.caps.remove(&id);
            self.done.insert(id, Ok(H2Response { status: p.status, headers: p.headers, body: p.body }));
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Headers { stream, status, headers, end } => {
                let Some(&(max, media)) = self.caps.get(&stream) else { return };
                let ct = headers.iter().find(|(n, _)| n == "content-type").map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_default();
                let is_media = ct.starts_with("video/") || ct.starts_with("audio/") || ct.starts_with("application/ogg");
                let (cap, cut) = if is_media && media > 0 { (media, true) } else { (max, false) };
                self.parts.insert(stream, Part { status, headers, body: Vec::new(), cap, cut });
                if end {
                    self.finish(stream);
                }
            }
            Event::Data { stream, data, end } => {
                let Some(p) = self.parts.get_mut(&stream) else { return };
                p.body.extend_from_slice(&data);
                if p.body.len() > p.cap {
                    if p.cut {
                        p.body.truncate(p.cap);
                        self.conn.cancel(stream);
                        self.finish(stream);
                    } else {
                        let cap = p.cap;
                        self.parts.remove(&stream);
                        self.caps.remove(&stream);
                        self.conn.cancel(stream);
                        self.done.insert(stream, Err(H2Fail::Failed(format!("the answer is larger than {} MB", cap >> 20))));
                    }
                    return;
                }
                if end {
                    self.finish(stream);
                }
            }
            Event::Reset { stream, code } => {
                if self.caps.remove(&stream).is_some() {
                    let seen = self.parts.remove(&stream).is_some();
                    let why = format!("the server reset the request (code {code})");
                    // REFUSED_STREAM means it was not processed at all
                    let r = if !seen && code == crate::h2::code::REFUSED_STREAM { H2Fail::Unusable(why) } else { H2Fail::Failed(why) };
                    self.done.insert(stream, Err(r));
                }
            }
            Event::PingAck { data } => {
                if self.acked.len() < 16 {
                    self.acked.push(data);
                }
            }
            Event::GoAway { last, code } => {
                let why = format!("the server said goodbye (code {code})");
                let ids: Vec<u32> = self.caps.keys().copied().filter(|&i| i > last).collect();
                for id in ids {
                    self.caps.remove(&id);
                    self.parts.remove(&id);
                    self.done.insert(id, Err(H2Fail::Unusable(why.clone())));
                }
            }
        }
    }
}

/// What a dialer found at the other end.
pub enum Dialed {
    /// The host chose `h2`.
    H2(Box<dyn Transport>),
    /// The host does not speak `h2` (or it answered with another protocol).
    NotH2,
}

pub type Dialer = Arc<dyn Fn(&str, u16) -> Result<Dialed, String> + Send + Sync>;

#[derive(Default)]
struct Slot {
    conns: Vec<Arc<H2Connection>>,
    no_h2_until: Option<Instant>,
}

/// The connections of one browser, by host and port.
pub struct H2Pool {
    dial: Dialer,
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
}

/// What the pool made of a request.
pub enum Pooled {
    /// Answered over HTTP/2.
    Done(Result<H2Response, String>),
    /// Not sent over HTTP/2 (the host does not speak it, could not be reached this way, or the connection gave out first): send it
    /// the HTTP/1.1 way.
    Http1,
}

impl H2Pool {
    pub fn new(dial: Dialer) -> H2Pool {
        H2Pool { dial, slots: Mutex::new(HashMap::new()) }
    }

    pub fn connections(&self) -> usize {
        lock(&self.slots).values().map(|s| lock(s).conns.len()).sum()
    }

    pub fn fetch(&self, host: &str, port: u16, req: &H2Request, timeout: Duration) -> Pooled {
        let key = format!("{host}:{port}");
        let slot = {
            let mut m = lock(&self.slots);
            if m.len() > 256 && !m.contains_key(&key) {
                m.clear();
            }
            m.entry(key).or_default().clone()
        };
        let replay_ok = matches!(req.method.to_ascii_uppercase().as_str(), "GET" | "HEAD");
        let deadline = Instant::now() + timeout;
        for _ in 0..2 {
            let conn = {
                let mut s = lock(&slot);
                if s.no_h2_until.is_some_and(|t| Instant::now() < t) {
                    return Pooled::Http1;
                }
                s.conns.retain(|c| !c.is_broken());
                let found = s.conns.iter().find(|c| c.usable()).cloned();
                match found {
                    Some(c) => c,
                    None => {
                        if s.conns.len() >= MAX_CONNS_PER_HOST {
                            return Pooled::Http1;
                        }
                        match (self.dial)(host, port) {
                            Ok(Dialed::H2(t)) => match H2Connection::open(t) {
                                Ok(c) => {
                                    let c = Arc::new(c);
                                    s.conns.push(c.clone());
                                    c
                                }
                                Err(_) => return Pooled::Http1,
                            },
                            Ok(Dialed::NotH2) => {
                                s.no_h2_until = Some(Instant::now() + NO_H2_FOR);
                                return Pooled::Http1;
                            }
                            Err(_) => return Pooled::Http1,
                        }
                    }
                }
            };
            // a connection that sat idle may have been dropped by the other end or on the way: ask it before trusting it with a request
            if conn.open_streams() == 0 && conn.idle_for() > IDLE_PING && !conn.ping_check(PING_WAIT) {
                continue;
            }
            match conn.fetch(req, deadline) {
                Ok(r) => return Pooled::Done(Ok(r)),
                Err(H2Fail::Failed(w)) => return Pooled::Done(Err(w)),
                // a request that may be sent again goes to another connection, then the old way; one that may not is reported
                Err(H2Fail::Unusable(w)) if !replay_ok => return Pooled::Done(Err(w)),
                Err(H2Fail::Unusable(_)) => continue,
            }
        }
        Pooled::Http1
    }
}

/// `https://host:port/path?query` taken apart: (host, port, authority, path and query). Addresses with a user name, a fragment-only
/// path or another scheme are not handled here.
pub fn split_https(url: &str) -> Option<(String, u16, String, String)> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    if auth.is_empty() || auth.contains('@') {
        return None;
    }
    let (host, port) = if let Some(r) = auth.strip_prefix('[') {
        let close = r.find(']')?;
        let port = r[close + 1..].strip_prefix(':').map(|p| p.parse::<u16>()).transpose().ok()?;
        (r[..close].to_string(), port)
    } else if let Some((h, p)) = auth.rsplit_once(':') {
        (h.to_string(), Some(p.parse::<u16>().ok()?))
    } else {
        (auth.to_string(), None)
    };
    if host.is_empty() {
        return None;
    }
    let tail = tail.split('#').next().unwrap_or("");
    let path = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Some((host, port.unwrap_or(443), auth.to_string(), path))
}

/// The dialer of the real browser: connect (the address that answers first, F-BROX-11), TLS with `h2` and `http/1.1` offered, and
/// see which one the host chose.
#[cfg(feature = "https")]
pub fn tls_dialer(roots: rustls::RootCertStore, connect_timeout: Duration) -> Dialer {
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("the default protocol versions are valid")
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = alpn_offer();
    let cfg = Arc::new(cfg);
    Arc::new(move |host: &str, port: u16| {
        use std::net::ToSocketAddrs;
        let addrs: Vec<std::net::SocketAddr> = (host, port).to_socket_addrs().map_err(|e| e.to_string())?.collect();
        let order = if addrs.len() > 1 { crate::happy::fastest_first(&addrs) } else { addrs };
        let mut last = String::from("no address");
        let mut sock = None;
        for a in order.iter().take(4) {
            match std::net::TcpStream::connect_timeout(a, connect_timeout) {
                Ok(s) => {
                    sock = Some(s);
                    break;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let sock = sock.ok_or(last)?;
        let _ = sock.set_nodelay(true);
        let _ = sock.set_write_timeout(Some(Duration::from_secs(20)));
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let conn = rustls::ClientConnection::new(cfg.clone(), name).map_err(|e| e.to_string())?;
        let mut st = rustls::StreamOwned::new(conn, sock);
        let _ = st.sock.set_read_timeout(Some(connect_timeout));
        while st.conn.is_handshaking() {
            st.conn.complete_io(&mut st.sock).map_err(|e| e.to_string())?;
        }
        if st.conn.alpn_protocol() == Some(b"h2".as_slice()) {
            Ok(Dialed::H2(Box::new(st)))
        } else {
            Ok(Dialed::NotH2)
        }
    })
}

/// The protocols offered in the TLS hello, best first.
pub fn alpn_offer() -> Vec<Vec<u8>> {
    vec![b"h2".to_vec(), b"http/1.1".to_vec()]
}

#[cfg(test)]
#[path = "h2client_tests.rs"]
mod tests;
