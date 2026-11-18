//! HTTP/2 for the domain listener (RFC 9113 and RFC 7541), a submodule of `cloudlisten`.
//!
//! One connection carries many requests. This module owns the connection (preface, SETTINGS, flow control, HPACK, stream state) and
//! nothing else: every request stream is turned into one HTTP/1.1 request (`Connection: close`) and handed to the existing
//! [`serve_http`](super::serve_http) over an in-memory pipe, and the HTTP/1.1 answer is turned back into HEADERS and DATA frames. So routing
//! by Host and path, the members gate, mutual TLS, tenants, forwarding headers, upstream dialing and every error answer are the ones of
//! HTTP/1.1, and cannot differ between the two protocols.
//!
//! Entered by prior knowledge on a plain port (the client sends the preface first, so `curl --http2-prior-knowledge` works) and by ALPN `h2`
//! on a TLS port. Limits: 32 concurrent streams, a request body of at most 16 MiB, a header block of at most 64 KiB, at most 400 stream
//! resets per connection (a rapid-reset flood ends in GOAWAY ENHANCE_YOUR_CALM). Not served: server push (never sent), priorities (read and
//! ignored), trailers of a request or a response (dropped), `CONNECT` and Upgrade (refused as on HTTP/1.1), extended CONNECT.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use iyakki_proxy::hpack::{Decoder, Encoder, Header};

use super::{serve_http, Inner, Sock};
use crate::cloudlisten_h2::{self as f, ErrorCode, FrameType};

const MAX_STREAMS: usize = 32;
const MAX_BODY: usize = 16 << 20;
const MAX_HEADER_BLOCK: usize = 64 * 1024;
const MAX_HEADER_LIST: usize = 32 * 1024;
const MAX_RESETS: u32 = 400;
const QUEUE_CAP: usize = 256 * 1024;
const TICK: Duration = Duration::from_millis(4);

const END_STREAM: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PRIORITY: u8 = 0x20;
const ACK: u8 = 0x1;

// ---------------------------------------------------------------------------------------------------------------- the pipe

struct Pipe {
    q: Mutex<(VecDeque<u8>, bool)>,
    cv: Condvar,
}

impl Pipe {
    fn new() -> Arc<Pipe> {
        Arc::new(Pipe { q: Mutex::new((VecDeque::new(), false)), cv: Condvar::new() })
    }
    fn close(&self) {
        self.q.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.cv.notify_all();
    }
}

/// One end of an in-memory duplex connection: what it writes the other end reads.
struct End {
    rx: Arc<Pipe>,
    tx: Arc<Pipe>,
    timeout: Mutex<Option<Duration>>,
    sni: Option<String>,
    certs: Vec<Vec<u8>>,
}

fn pair(sni: Option<String>, certs: Vec<Vec<u8>>) -> (End, End) {
    let (a, b) = (Pipe::new(), Pipe::new());
    (
        End { rx: a.clone(), tx: b.clone(), timeout: Mutex::new(None), sni: sni.clone(), certs: certs.clone() },
        End { rx: b, tx: a, timeout: Mutex::new(None), sni, certs },
    )
}

impl Read for End {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let limit = *self.timeout.lock().unwrap_or_else(|e| e.into_inner());
        let until = limit.map(|d| Instant::now() + d);
        let mut g = self.rx.q.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if !g.0.is_empty() {
                let n = buf.len().min(g.0.len());
                for (i, b) in g.0.drain(..n).enumerate() {
                    buf[i] = b;
                }
                return Ok(n);
            }
            if g.1 {
                return Ok(0);
            }
            match until {
                None => g = self.rx.cv.wait(g).unwrap_or_else(|e| e.into_inner()),
                Some(t) => {
                    let now = Instant::now();
                    if now >= t {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    g = self.rx.cv.wait_timeout(g, t - now).unwrap_or_else(|e| e.into_inner()).0;
                }
            }
        }
    }
}

impl Write for End {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut g = self.tx.q.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        g.0.extend(buf.iter().copied());
        drop(g);
        self.tx.cv.notify_all();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.tx.close();
        self.rx.close();
    }
}

impl Sock for End {
    fn set_timeout(&self, d: Option<Duration>) {
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner()) = d;
    }
    fn sni(&self) -> Option<String> {
        self.sni.clone()
    }
    fn peer_certs(&self) -> Vec<Vec<u8>> {
        self.certs.clone()
    }
}

// ---------------------------------------------------------------------------------------------------------------- one request

enum Out {
    Head(u16, Vec<Header>, bool),
    Data(Vec<u8>),
    End,
}

struct Stream {
    block: Vec<u8>,
    headers: Vec<Header>,
    body: Vec<u8>,
    req_done: bool,
    started: bool,
    send_window: i64,
    queue: VecDeque<u8>,
    ending: bool,
    head_sent: bool,
    rx: Option<Receiver<Out>>,
    cancel: Arc<AtomicBool>,
}

fn connection_specific(name: &str) -> bool {
    matches!(name, "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade")
}

/// The HTTP/1.1 request one stream stands for, or why it is not acceptable (a stream error: `RST_STREAM`/400).
fn to_http1(headers: &[Header], body: &[u8]) -> Result<(Vec<u8>, String), String> {
    let (mut method, mut path, mut authority, mut scheme) = (None, None, None, None);
    let mut regular: Vec<&Header> = Vec::new();
    let mut seen_regular = false;
    for h in headers {
        let (n, v) = (h.0.as_str(), h.1.as_str());
        if n.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err("a header name is not lower case".into());
        }
        if v.bytes().any(|b| b == 0 || b == b'\r' || b == b'\n') {
            return Err("a header value holds a control character".into());
        }
        if let Some(p) = n.strip_prefix(':') {
            if seen_regular {
                return Err("a pseudo-header came after a header".into());
            }
            let slot = match p {
                "method" => &mut method,
                "path" => &mut path,
                "authority" => &mut authority,
                "scheme" => &mut scheme,
                _ => return Err("an unknown pseudo-header".into()),
            };
            if slot.replace(v.to_string()).is_some() {
                return Err("a pseudo-header twice".into());
            }
        } else {
            seen_regular = true;
            if connection_specific(n) {
                return Err("a connection-specific header".into());
            }
            if n == "te" && v != "trailers" {
                return Err("TE other than trailers".into());
            }
            regular.push(h);
        }
    }
    let method = method.ok_or("no :method")?;
    let path = path.ok_or("no :path")?;
    scheme.ok_or("no :scheme")?;
    if method == "CONNECT" {
        return Err("CONNECT is not supported".into());
    }
    if method.is_empty() || !method.bytes().all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)) {
        return Err("a bad :method".into());
    }
    if path.is_empty() || path.contains(' ') {
        return Err("a bad :path".into());
    }
    let host_header = regular.iter().find(|h| h.0 == "host").map(|h| h.1.clone());
    let host = authority.or(host_header).filter(|h| !h.is_empty()).ok_or("no :authority or host")?;
    let mut out = Vec::with_capacity(256 + body.len());
    out.extend_from_slice(format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n").as_bytes());
    let mut cookies: Vec<&str> = Vec::new();
    for (n, v) in regular {
        match n.as_str() {
            "host" | "content-length" | "te" => {}
            "cookie" => cookies.push(v),
            _ => out.extend_from_slice(format!("{n}: {v}\r\n").as_bytes()),
        }
    }
    if !cookies.is_empty() {
        out.extend_from_slice(format!("cookie: {}\r\n", cookies.join("; ")).as_bytes());
    }
    if !body.is_empty() || matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    Ok((out, method))
}

/// Read the HTTP/1.1 answer from `end` and send it as messages. Returns when the answer ended, the connection went, or the stream was cancelled.
fn relay_answer(end: &mut End, head_only: bool, tx: &SyncSender<Out>, cancel: &AtomicBool) {
    end.set_timeout(Some(Duration::from_millis(200)));
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut closed = false;
    // 1. the head (skipping interim 1xx answers)
    let (status, headers, rest) = loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).into_owned();
            let rest = buf[i + 4..].to_vec();
            let mut lines = head.split("\r\n");
            let status: u16 = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).unwrap_or(502);
            let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
            if (100..200).contains(&status) {
                buf = rest;
                continue;
            }
            break (status, headers, rest);
        }
        match end.read(&mut chunk) {
            Ok(0) => {
                let _ = tx.send(Out::Head(502, vec![("content-type".into(), "text/plain".into())], false));
                let _ = tx.send(Out::Data(b"the target closed before it answered\n".to_vec()));
                let _ = tx.send(Out::End);
                return;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {}
            Err(_) => return,
        }
    };
    let chunked = headers.iter().any(|(n, v)| n == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let length: Option<usize> = headers.iter().find(|(n, _)| n == "content-length").and_then(|(_, v)| v.parse().ok());
    let no_body = head_only || matches!(status, 204 | 304) || length == Some(0);
    let mut out_headers: Vec<Header> = headers.into_iter().filter(|(n, _)| !connection_specific(n) && n != "trailer" && !(chunked && n == "content-length")).collect();
    out_headers.retain(|(n, _)| n != "te");
    if tx.send(Out::Head(status, out_headers, no_body)).is_err() {
        return;
    }
    if no_body {
        return;
    }
    // 2. the body
    let mut dec = Chunked::default();
    let mut left = length;
    let mut pending = rest;
    loop {
        if !pending.is_empty() {
            let data = std::mem::take(&mut pending);
            let body: Vec<u8> = if chunked {
                match dec.feed(&data) {
                    Ok(b) => b,
                    Err(()) => return,
                }
            } else if let Some(l) = left.as_mut() {
                let take = data.len().min(*l);
                *l -= take;
                data[..take].to_vec()
            } else {
                data
            };
            for piece in body.chunks(16 * 1024) {
                if cancel.load(Ordering::Relaxed) || tx.send(Out::Data(piece.to_vec())).is_err() {
                    return;
                }
            }
            if (chunked && dec.done) || left == Some(0) {
                let _ = tx.send(Out::End);
                return;
            }
        }
        if closed {
            let _ = tx.send(Out::End);
            return;
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match end.read(&mut chunk) {
            Ok(0) => closed = true,
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {}
            Err(_) => return,
        }
    }
}

/// An incremental decoder of the chunked transfer coding (trailers are read and dropped).
#[derive(Default)]
struct Chunked {
    buf: Vec<u8>,
    left: usize,
    in_data: bool,
    done: bool,
    trailers: bool,
}

impl Chunked {
    fn feed(&mut self, data: &[u8]) -> Result<Vec<u8>, ()> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.done {
                return Ok(out);
            }
            if self.in_data {
                if self.left > 0 {
                    let n = self.left.min(self.buf.len());
                    if n == 0 {
                        return Ok(out);
                    }
                    out.extend(self.buf.drain(..n));
                    self.left -= n;
                    continue;
                }
                if self.buf.len() < 2 {
                    return Ok(out);
                }
                if &self.buf[..2] != b"\r\n" {
                    return Err(());
                }
                self.buf.drain(..2);
                self.in_data = false;
                continue;
            }
            let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") else {
                if self.buf.len() > 4096 {
                    return Err(());
                }
                return Ok(out);
            };
            let line = String::from_utf8_lossy(&self.buf[..i]).into_owned();
            self.buf.drain(..i + 2);
            if self.trailers {
                if line.is_empty() {
                    self.done = true;
                }
                continue;
            }
            let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| ())?;
            if size == 0 {
                self.trailers = true;
            } else {
                self.left = size;
                self.in_data = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the connection

struct Conn {
    sock: Box<dyn Sock>,
    inbuf: Vec<u8>,
    enc: Encoder,
    dec: Decoder,
    peer: f::Settings,
    conn_window: i64,
    streams: HashMap<u32, Stream>,
    last_client_stream: u32,
    continuing: Option<(u32, bool)>,
    resets: u32,
    goaway: bool,
}

fn put(sock: &mut dyn Sock, ty: FrameType, flags: u8, stream: u32, payload: &[u8]) -> io::Result<()> {
    let frame = f::encode_frame(ty, flags, stream, payload).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let (mut off, give_up) = (0, Instant::now() + Duration::from_secs(30));
    while off < frame.len() {
        match sock.write(&frame[off..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => off += n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) && Instant::now() < give_up => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => return Err(e),
        }
    }
    sock.flush()
}

/// A connection error: say why with GOAWAY and end.
struct Fatal(ErrorCode);

fn goaway(sock: &mut dyn Sock, last: u32, code: ErrorCode) {
    let mut p = (last & 0x7fff_ffff).to_be_bytes().to_vec();
    p.extend_from_slice(&code.to_u32().to_be_bytes());
    let _ = put(sock, FrameType::GoAway, 0, 0, &p);
}

/// Serve one HTTP/2 connection until it ends. `first` holds bytes already read from the client (at most the preface).
pub(super) fn serve(inner: &Arc<Inner>, sock: Box<dyn Sock>, first: Vec<u8>, peer: std::net::SocketAddr, port: u16, tls: bool) {
    inner.log(&format!("{peer}: HTTP/2 connection on port {port}{}", if tls { " (TLS, ALPN h2)" } else { " (prior knowledge)" }));
    let (sni, certs) = (sock.sni(), sock.peer_certs());
    sock.set_timeout(Some(TICK));
    let mut c = Conn { sock, inbuf: first, enc: Encoder::default(), dec: Decoder::new(4096), peer: f::Settings::default(), conn_window: 65_535, streams: HashMap::new(), last_client_stream: 0, continuing: None, resets: 0, goaway: false };
    let mine = f::Settings { max_concurrent_streams: Some(MAX_STREAMS as u32), max_header_list_size: Some(MAX_HEADER_LIST as u32), enable_push: false, ..Default::default() };
    if put(&mut *c.sock, FrameType::Settings, 0, 0, &mine.encode_changes()).is_err() {
        return;
    }
    // the preface comes first
    let started = Instant::now();
    while c.inbuf.len() < f::PREFACE.len() {
        if !f::PREFACE.starts_with(&c.inbuf[..c.inbuf.len().min(f::PREFACE.len())]) || started.elapsed() > inner.opts.head_timeout {
            return;
        }
        if !c.fill() {
            return;
        }
    }
    if &c.inbuf[..f::PREFACE.len()] != f::PREFACE {
        return;
    }
    c.inbuf.drain(..f::PREFACE.len());
    let mut last_active = Instant::now();
    loop {
        // 1. frames from the client
        if c.fill_nonblocking() == Fill::Closed {
            c.cancel_all();
            return;
        }
        match c.frames(inner, &sni, &certs, peer, port, tls) {
            Ok(n) => {
                if n > 0 {
                    last_active = Instant::now();
                }
            }
            Err(Fatal(code)) => {
                inner.log(&format!("{peer}: HTTP/2 connection ended: {code:?}"));
                goaway(&mut *c.sock, c.last_client_stream, code);
                c.cancel_all();
                return;
            }
        }
        // 2. answers to the client
        if c.pump().is_err() {
            c.cancel_all();
            return;
        }
        if c.goaway && c.streams.is_empty() {
            return;
        }
        if c.streams.is_empty() && last_active.elapsed() > inner.opts.idle_timeout {
            goaway(&mut *c.sock, c.last_client_stream, ErrorCode::NoError);
            return;
        }
        if !c.streams.is_empty() {
            last_active = Instant::now();
        }
    }
}

#[derive(PartialEq, Eq)]
enum Fill {
    Some,
    None,
    Closed,
}

impl Conn {
    fn fill(&mut self) -> bool {
        self.fill_nonblocking() != Fill::Closed
    }

    fn fill_nonblocking(&mut self) -> Fill {
        let mut tmp = [0u8; 32 * 1024];
        match self.sock.read(&mut tmp) {
            Ok(0) => Fill::Closed,
            Ok(n) => {
                self.inbuf.extend_from_slice(&tmp[..n]);
                Fill::Some
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => Fill::None,
            Err(_) => Fill::Closed,
        }
    }

    fn cancel_all(&mut self) {
        for s in self.streams.values() {
            s.cancel.store(true, Ordering::Relaxed);
        }
        self.streams.clear();
    }

    fn reset(&mut self, id: u32, code: ErrorCode) {
        let _ = put(&mut *self.sock, FrameType::RstStream, 0, id, &code.to_u32().to_be_bytes());
        if let Some(s) = self.streams.remove(&id) {
            s.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Handle every complete frame in the input. Returns how many there were.
    fn frames(&mut self, inner: &Arc<Inner>, sni: &Option<String>, certs: &[Vec<u8>], peer: std::net::SocketAddr, port: u16, tls: bool) -> Result<usize, Fatal> {
        let mut count = 0;
        loop {
            let Some(h) = f::parse_header(&self.inbuf) else { return Ok(count) };
            if let Err(e) = f::check_header(&h, 16_384) {
                if e.conn {
                    return Err(Fatal(e.code));
                }
            }
            let total = f::HEADER_LEN + h.len as usize;
            if self.inbuf.len() < total {
                return Ok(count);
            }
            let payload: Vec<u8> = self.inbuf[f::HEADER_LEN..total].to_vec();
            self.inbuf.drain(..total);
            count += 1;
            if let Some((id, _)) = self.continuing {
                if h.ty != FrameType::Continuation || h.stream != id {
                    return Err(Fatal(ErrorCode::ProtocolError));
                }
            }
            match h.ty {
                FrameType::Settings => {
                    if h.flags & ACK != 0 {
                        continue;
                    }
                    let old = self.peer.initial_window_size;
                    self.peer = self.peer.apply(&payload).map_err(|e| Fatal(e.code))?;
                    let delta = i64::from(self.peer.initial_window_size) - i64::from(old);
                    for s in self.streams.values_mut() {
                        s.send_window += delta;
                    }
                    self.enc.set_peer_limit(self.peer.header_table_size as usize);
                    put(&mut *self.sock, FrameType::Settings, ACK, 0, &[]).map_err(|_| Fatal(ErrorCode::InternalError))?;
                }
                FrameType::Ping => {
                    if h.flags & ACK == 0 {
                        put(&mut *self.sock, FrameType::Ping, ACK, 0, &payload).map_err(|_| Fatal(ErrorCode::InternalError))?;
                    }
                }
                FrameType::WindowUpdate => match f::window_increment(&payload, h.stream) {
                    Ok(n) => {
                        if h.stream == 0 {
                            self.conn_window += i64::from(n);
                            if self.conn_window > i64::from(f::MAX_WINDOW) {
                                return Err(Fatal(ErrorCode::FlowControlError));
                            }
                        } else if let Some(s) = self.streams.get_mut(&h.stream) {
                            s.send_window += i64::from(n);
                            if s.send_window > i64::from(f::MAX_WINDOW) {
                                self.reset(h.stream, ErrorCode::FlowControlError);
                            }
                        }
                    }
                    Err(e) if e.conn => return Err(Fatal(e.code)),
                    Err(e) => self.reset(h.stream, e.code),
                },
                FrameType::GoAway => {
                    self.goaway = true;
                }
                FrameType::RstStream => {
                    self.resets += 1;
                    if self.resets > MAX_RESETS {
                        return Err(Fatal(ErrorCode::EnhanceYourCalm));
                    }
                    if let Some(s) = self.streams.remove(&h.stream) {
                        s.cancel.store(true, Ordering::Relaxed);
                    }
                }
                FrameType::Priority => {}
                FrameType::PushPromise => return Err(Fatal(ErrorCode::ProtocolError)),
                FrameType::Headers => {
                    let body = f::strip_padding(h.flags, &payload).map_err(|e| Fatal(e.code))?;
                    let body = if h.flags & PRIORITY != 0 {
                        body.get(5..).ok_or(Fatal(ErrorCode::FrameSizeError))?
                    } else {
                        body
                    };
                    if h.stream % 2 == 0 {
                        return Err(Fatal(ErrorCode::ProtocolError));
                    }
                    let end_stream = h.flags & END_STREAM != 0;
                    if self.streams.contains_key(&h.stream) {
                        // trailers of a request: read, not used
                        let s = self.streams.get_mut(&h.stream).unwrap();
                        if s.req_done || !end_stream {
                            return Err(Fatal(ErrorCode::ProtocolError));
                        }
                        s.block = body.to_vec();
                        self.continuing = if h.flags & END_HEADERS == 0 { Some((h.stream, true)) } else { None };
                        if h.flags & END_HEADERS != 0 {
                            let blk = std::mem::take(&mut self.streams.get_mut(&h.stream).unwrap().block);
                            self.dec.decode(&blk).map_err(|_| Fatal(ErrorCode::CompressionError))?;
                            self.finish_request(inner, h.stream, sni, certs, peer, port, tls);
                        }
                        continue;
                    }
                    if h.stream <= self.last_client_stream {
                        return Err(Fatal(ErrorCode::ProtocolError));
                    }
                    self.last_client_stream = h.stream;
                    if self.goaway {
                        // the client said it is leaving: still decode (the table must stay in step), answer nothing
                    }
                    let s = Stream { block: body.to_vec(), headers: Vec::new(), body: Vec::new(), req_done: false, started: false, send_window: i64::from(self.peer.initial_window_size), queue: VecDeque::new(), ending: false, head_sent: false, rx: None, cancel: Arc::new(AtomicBool::new(false)) };
                    if self.streams.len() >= MAX_STREAMS {
                        // decode and drop so the table stays in step, then refuse
                        if h.flags & END_HEADERS != 0 {
                            self.dec.decode(&s.block).map_err(|_| Fatal(ErrorCode::CompressionError))?;
                            let _ = put(&mut *self.sock, FrameType::RstStream, 0, h.stream, &ErrorCode::RefusedStream.to_u32().to_be_bytes());
                        } else {
                            return Err(Fatal(ErrorCode::EnhanceYourCalm));
                        }
                        continue;
                    }
                    self.streams.insert(h.stream, s);
                    if h.flags & END_HEADERS == 0 {
                        self.continuing = Some((h.stream, end_stream));
                        if body.len() > MAX_HEADER_BLOCK {
                            return Err(Fatal(ErrorCode::EnhanceYourCalm));
                        }
                    } else {
                        self.headers_done(inner, h.stream, end_stream, sni, certs, peer, port, tls)?;
                    }
                }
                FrameType::Continuation => {
                    let Some((id, end_stream)) = self.continuing else { return Err(Fatal(ErrorCode::ProtocolError)) };
                    let Some(s) = self.streams.get_mut(&id) else { return Err(Fatal(ErrorCode::ProtocolError)) };
                    s.block.extend_from_slice(&payload);
                    if s.block.len() > MAX_HEADER_BLOCK {
                        return Err(Fatal(ErrorCode::EnhanceYourCalm));
                    }
                    if h.flags & END_HEADERS != 0 {
                        self.continuing = None;
                        if self.streams.get(&id).is_some_and(|s| s.started || !s.headers.is_empty()) {
                            // trailers
                            let blk = std::mem::take(&mut self.streams.get_mut(&id).unwrap().block);
                            self.dec.decode(&blk).map_err(|_| Fatal(ErrorCode::CompressionError))?;
                            self.finish_request(inner, id, sni, certs, peer, port, tls);
                        } else {
                            self.headers_done(inner, id, end_stream, sni, certs, peer, port, tls)?;
                        }
                    }
                }
                FrameType::Data => {
                    let data = f::strip_padding(h.flags, &payload).map_err(|e| Fatal(e.code))?.to_vec();
                    // the connection's receive window is given back at once: the data is held, not processed
                    if !payload.is_empty() {
                        let inc = (payload.len() as u32).to_be_bytes();
                        put(&mut *self.sock, FrameType::WindowUpdate, 0, 0, &inc).map_err(|_| Fatal(ErrorCode::InternalError))?;
                    }
                    let Some(s) = self.streams.get_mut(&h.stream) else {
                        if h.stream > self.last_client_stream {
                            return Err(Fatal(ErrorCode::ProtocolError));
                        }
                        continue;
                    };
                    if s.req_done {
                        self.reset(h.stream, ErrorCode::StreamClosed);
                        continue;
                    }
                    if s.body.len() + data.len() > MAX_BODY {
                        self.reset(h.stream, ErrorCode::EnhanceYourCalm);
                        continue;
                    }
                    s.body.extend_from_slice(&data);
                    if !payload.is_empty() && h.flags & END_STREAM == 0 {
                        let inc = (payload.len() as u32).to_be_bytes();
                        put(&mut *self.sock, FrameType::WindowUpdate, 0, h.stream, &inc).map_err(|_| Fatal(ErrorCode::InternalError))?;
                    }
                    if h.flags & END_STREAM != 0 {
                        self.finish_request(inner, h.stream, sni, certs, peer, port, tls);
                    }
                }
                FrameType::Unknown(_) => {}
            }
        }
    }

    /// A complete header block arrived for a new stream.
    #[allow(clippy::too_many_arguments)]
    fn headers_done(&mut self, inner: &Arc<Inner>, id: u32, end_stream: bool, sni: &Option<String>, certs: &[Vec<u8>], peer: std::net::SocketAddr, port: u16, tls: bool) -> Result<(), Fatal> {
        let blk = std::mem::take(&mut self.streams.get_mut(&id).unwrap().block);
        let headers = self.dec.decode(&blk).map_err(|_| Fatal(ErrorCode::CompressionError))?;
        if self.dec.take_uppercase_name() {
            self.reset(id, ErrorCode::ProtocolError);
            return Ok(());
        }
        let size: usize = headers.iter().map(|(n, v)| n.len() + v.len() + 32).sum();
        if size > MAX_HEADER_LIST {
            self.reset(id, ErrorCode::EnhanceYourCalm);
            return Ok(());
        }
        self.streams.get_mut(&id).unwrap().headers = headers;
        if end_stream {
            self.finish_request(inner, id, sni, certs, peer, port, tls);
        }
        Ok(())
    }

    /// The request is complete: start the worker that runs it through the HTTP/1.1 code.
    #[allow(clippy::too_many_arguments)]
    fn finish_request(&mut self, inner: &Arc<Inner>, id: u32, sni: &Option<String>, certs: &[Vec<u8>], peer: std::net::SocketAddr, port: u16, tls: bool) {
        let Some(s) = self.streams.get_mut(&id) else { return };
        s.req_done = true;
        if s.started {
            return;
        }
        s.started = true;
        let (request, method) = match to_http1(&s.headers, &s.body) {
            Ok(r) => r,
            Err(why) => {
                inner.log(&format!("{peer}: HTTP/2 request refused: {why}"));
                let _ = put(&mut *self.sock, FrameType::RstStream, 0, id, &ErrorCode::ProtocolError.to_u32().to_be_bytes());
                self.streams.remove(&id);
                return;
            }
        };
        let (tx, rx) = sync_channel::<Out>(8);
        s.rx = Some(rx);
        let cancel = s.cancel.clone();
        let (mut client_end, server_end) = pair(sni.clone(), certs.to_vec());
        let inner2 = inner.clone();
        std::thread::spawn(move || serve_http(&inner2, Box::new(server_end), peer, port, tls));
        std::thread::spawn(move || {
            if client_end.write_all(&request).is_err() {
                return;
            }
            relay_answer(&mut client_end, method == "HEAD", &tx, &cancel);
        });
    }

    /// Move answers from the workers to the client, as the windows allow. Ends streams that are finished.
    fn pump(&mut self) -> io::Result<()> {
        let ids: Vec<u32> = self.streams.keys().copied().collect();
        for id in ids {
            // take what the worker has produced, while the stream's queue is not full
            loop {
                let Some(s) = self.streams.get_mut(&id) else { break };
                if s.queue.len() >= QUEUE_CAP {
                    break;
                }
                let Some(rx) = s.rx.as_ref() else { break };
                match rx.try_recv() {
                    Ok(Out::Head(status, headers, end)) => {
                        let mut list: Vec<Header> = vec![(":status".into(), status.to_string())];
                        list.extend(headers);
                        let block = self.enc.encode(&list);
                        self.send_block(id, &block, end)?;
                        let s = self.streams.get_mut(&id).unwrap();
                        s.head_sent = true;
                        if end {
                            s.ending = false;
                            self.streams.remove(&id);
                            break;
                        }
                    }
                    Ok(Out::Data(d)) => s.queue.extend(d),
                    Ok(Out::End) => {
                        s.ending = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // the worker ended without finishing: reset what is open
                        if s.head_sent {
                            s.ending = true;
                        } else {
                            self.reset(id, ErrorCode::InternalError);
                        }
                        break;
                    }
                }
            }
            // send data as far as the windows allow
            let Some(s) = self.streams.get_mut(&id) else { continue };
            while !s.queue.is_empty() && self.conn_window > 0 && s.send_window > 0 {
                let n = s.queue.len().min(self.peer.max_frame_size as usize).min(self.conn_window as usize).min(s.send_window as usize);
                let part: Vec<u8> = s.queue.drain(..n).collect();
                let last = s.ending && s.queue.is_empty();
                put(&mut *self.sock, FrameType::Data, if last { END_STREAM } else { 0 }, id, &part)?;
                self.conn_window -= n as i64;
                s.send_window -= n as i64;
                if last {
                    s.ending = false;
                    self.streams.remove(&id);
                    break;
                }
            }
            if let Some(s) = self.streams.get_mut(&id) {
                if s.ending && s.queue.is_empty() {
                    put(&mut *self.sock, FrameType::Data, END_STREAM, id, &[])?;
                    self.streams.remove(&id);
                }
            }
        }
        Ok(())
    }

    /// HEADERS (and CONTINUATION frames when the block is larger than one frame).
    fn send_block(&mut self, id: u32, block: &[u8], end_stream: bool) -> io::Result<()> {
        let max = self.peer.max_frame_size as usize;
        let mut chunks = block.chunks(max.max(1));
        let first = chunks.next().unwrap_or(&[]);
        let more: Vec<&[u8]> = chunks.collect();
        let mut flags = if end_stream { END_STREAM } else { 0 };
        if more.is_empty() {
            flags |= END_HEADERS;
        }
        put(&mut *self.sock, FrameType::Headers, flags, id, first)?;
        for (i, c) in more.iter().enumerate() {
            put(&mut *self.sock, FrameType::Continuation, if i + 1 == more.len() { END_HEADERS } else { 0 }, id, c)?;
        }
        Ok(())
    }
}
