//! The wire of the Gemini live session (EXPERIMENTAL): a WebSocket client (RFC 6455) over TLS, written here (no new dependency).
//! The server side, SHA-1/Base64 and the masked client frame are `iyakki_link::wsproto`; the TLS is the workspace's rustls with the
//! system's certificate authorities (`gitnet::roots`), the same setup the git client uses. The key travels in the address
//! (`?key=`, as Google documents) and is removed from every error text here.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_link::wsproto;

use crate::geminilive::{ConnectErr, LiveConnector, LiveLink, Recv, MAX_MESSAGE};

/// A stream with a read timeout that can be changed.
pub trait Duplex: Read + Write + Send {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()>;
}

impl Duplex for TcpStream {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()> {
        TcpStream::set_read_timeout(self, d)
    }
}

struct Tls(rustls::StreamOwned<rustls::ClientConnection, TcpStream>);

impl Read for Tls {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for Tls {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Duplex for Tls {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()> {
        self.0.sock.set_read_timeout(d)
    }
}

/// The text before the key in an address, for messages.
fn without_key(s: &str, url: &str) -> String {
    let key = url.split("key=").nth(1).unwrap_or("");
    crate::gemini::redact(s, key.split('&').next().unwrap_or(""))
}

/// `wss://host[:port]/path?query` or `ws://...` (plain: tests on this computer only).
struct Parts {
    tls: bool,
    host: String,
    port: u16,
    target: String,
}

fn parse(url: &str) -> Result<Parts, String> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") { (true, r) } else if let Some(r) = url.strip_prefix("ws://") { (false, r) } else { return Err("not a WebSocket address".into()) };
    let (hostport, target) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "bad port".to_string())?),
        None => (hostport.to_string(), if tls { 443 } else { 80 }),
    };
    if host.is_empty() || target.chars().any(|c| c.is_control() || c == ' ') {
        return Err("bad address".into());
    }
    Ok(Parts { tls, host, port, target })
}

/// The real connector: DNS, TCP, TLS (for `wss`), the upgrade request, the check of the answer.
pub struct WsConnector {
    pub timeout: Duration,
}

impl Default for WsConnector {
    fn default() -> WsConnector {
        WsConnector { timeout: Duration::from_secs(10) }
    }
}

impl LiveConnector for WsConnector {
    fn connect(&self, url: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
        let p = parse(url).map_err(ConnectErr::Other)?;
        let other = |e: String| ConnectErr::Other(without_key(&e, url));
        let deadline = Instant::now() + self.timeout;
        let addrs: Vec<_> = (p.host.as_str(), p.port).to_socket_addrs().map_err(|e| other(format!("{}: {e}", p.host)))?.collect();
        let mut sock = None;
        let mut last = format!("{}: no address", p.host);
        for a in addrs {
            match TcpStream::connect_timeout(&a, deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(200))) {
                Ok(s) => {
                    sock = Some(s);
                    break;
                }
                Err(e) => last = format!("cannot connect to {}:{}: {e}", p.host, p.port),
            }
        }
        let sock = sock.ok_or_else(|| other(last))?;
        let _ = sock.set_nodelay(true);
        let _ = sock.set_write_timeout(Some(self.timeout));
        let io: Box<dyn Duplex> = if p.tls {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let cfg = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| other(e.to_string()))?
                .with_root_certificates(crate::gitnet::roots(&[]).map_err(other)?)
                .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(p.host.clone()).map_err(|_| other(format!("{} is not a valid host name", p.host)))?;
            let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| other(e.to_string()))?;
            Box::new(Tls(rustls::StreamOwned::new(conn, sock)))
        } else {
            Box::new(sock)
        };
        WsLink::handshake(io, &p.host, &p.target, self.timeout).map(|l| Box::new(l) as Box<dyn LiveLink>).map_err(|e| match e {
            ConnectErr::Other(t) => other(t),
            h => h,
        })
    }
}

/// An open WebSocket (client side).
pub struct WsLink {
    io: Box<dyn Duplex>,
    buf: Vec<u8>,
    frag: Option<(u8, Vec<u8>)>,
    ready: VecDeque<Recv>,
    closed: bool,
}

fn mask() -> [u8; 4] {
    let mut m = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut m);
    m
}

impl WsLink {
    /// Sends the upgrade request on `io` and checks the answer. A refusal (for example 429) comes back with its status and body.
    pub fn handshake(mut io: Box<dyn Duplex>, host: &str, target: &str, timeout: Duration) -> Result<WsLink, ConnectErr> {
        let mut k = [0u8; 16];
        iyakki_sec::random_fill(&mut k).map_err(|_| ConnectErr::Other("no random numbers".into()))?;
        let key = wsproto::base64(&k);
        let req = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: iyakki-live\r\n\r\n");
        io.write_all(req.as_bytes()).and_then(|()| io.flush()).map_err(|e| ConnectErr::Other(format!("cannot send the request: {e}")))?;
        let deadline = Instant::now() + timeout;
        let mut head = Vec::new();
        let mut chunk = [0u8; 2048];
        let end = loop {
            if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            if head.len() > 16 * 1024 || Instant::now() > deadline {
                return Err(ConnectErr::Other("the answer to the upgrade request was too long or too slow".into()));
            }
            let _ = io.set_read_timeout(Some(Duration::from_millis(500)));
            match io.read(&mut chunk) {
                Ok(0) => return Err(ConnectErr::Other("the connection closed before the answer".into())),
                Ok(n) => head.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
                Err(e) => return Err(ConnectErr::Other(format!("cannot read the answer: {e}"))),
            }
        };
        let text = String::from_utf8_lossy(&head[..end]).to_string();
        let mut lines = text.split("\r\n");
        let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        if status != 101 {
            // the rest of what was read, then a little more, is the error body
            let mut body = head[end..].to_vec();
            let _ = io.set_read_timeout(Some(Duration::from_millis(300)));
            while body.len() < 4096 {
                match io.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&chunk[..n]),
                }
            }
            return Err(ConnectErr::Http(status, String::from_utf8_lossy(&body[..body.len().min(4096)]).to_string()));
        }
        let want = wsproto::accept_key(&key);
        let ok = lines.any(|l| l.split_once(':').is_some_and(|(n, v)| n.trim().eq_ignore_ascii_case("sec-websocket-accept") && v.trim() == want));
        if !ok {
            return Err(ConnectErr::Other("the server did not accept the WebSocket upgrade correctly".into()));
        }
        Ok(WsLink { io, buf: head[end..].to_vec(), frag: None, ready: VecDeque::new(), closed: false })
    }

    fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let f = wsproto::client_frame(opcode, payload, mask());
        self.io.write_all(&f).and_then(|()| self.io.flush()).map_err(|e| format!("cannot send: {e}"))
    }

    /// Takes complete frames out of the buffer. Server frames are never masked.
    fn parse(&mut self) -> Result<(), String> {
        loop {
            let b = &self.buf;
            if b.len() < 2 {
                return Ok(());
            }
            let (fin, op, masked, l7) = (b[0] & 0x80 != 0, b[0] & 0x0f, b[1] & 0x80 != 0, (b[1] & 0x7f) as usize);
            if b[0] & 0x70 != 0 || masked {
                return Err("the server broke the WebSocket rules".into());
            }
            let (len, at) = match l7 {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
                127 if b.len() >= 10 => (u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]) as usize, 10),
                126 | 127 => return Ok(()),
                n => (n, 2),
            };
            if len > MAX_MESSAGE {
                return Err("a message from the server is too big".into());
            }
            if b.len() < at + len {
                return Ok(());
            }
            let payload = b[at..at + len].to_vec();
            self.buf.drain(..at + len);
            match op {
                0x8 => {
                    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
                    let reason = String::from_utf8_lossy(payload.get(2..).unwrap_or(&[])).chars().take(300).collect();
                    self.ready.push_back(Recv::Closed { code, reason });
                    self.closed = true;
                    return Ok(());
                }
                0x9 => self.write_frame(0xA, &payload)?,
                0xA => {}
                0x0..=0x2 => {
                    let (first, mut data) = match (op, self.frag.take()) {
                        (0x0, Some((o, d))) => (o, d),
                        (0x0, None) => return Err("a continuation without a start".into()),
                        (o, None) => (o, Vec::new()),
                        (_, Some(_)) => return Err("a new message before the last one ended".into()),
                    };
                    data.extend_from_slice(&payload);
                    if data.len() > MAX_MESSAGE {
                        return Err("a message from the server is too big".into());
                    }
                    if fin {
                        // text and binary frames both carry JSON in this API
                        self.ready.push_back(Recv::Text(String::from_utf8(data).map_err(|_| "a message was not text".to_string())?));
                    } else {
                        self.frag = Some((first, data));
                    }
                }
                _ => return Err("an unknown WebSocket frame".into()),
            }
        }
    }
}

impl LiveLink for WsLink {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        if self.closed {
            return Err("the connection is closed".into());
        }
        self.write_frame(0x1, text.as_bytes())
    }

    fn recv(&mut self, wait: Duration) -> Result<Recv, String> {
        let until = Instant::now() + wait;
        loop {
            self.parse()?;
            if let Some(r) = self.ready.pop_front() {
                return Ok(r);
            }
            if self.closed {
                return Ok(Recv::Closed { code: None, reason: String::new() });
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Recv::Timeout);
            }
            let _ = self.io.set_read_timeout(Some(left.max(Duration::from_millis(1))));
            let mut chunk = [0u8; 16 * 1024];
            match self.io.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    return Ok(Recv::Closed { code: None, reason: "the server ended the connection".into() });
                }
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
                Err(e) => return Err(format!("cannot read: {e}")),
            }
        }
    }

    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.write_frame(0x8, &1000u16.to_be_bytes());
        }
    }
}
