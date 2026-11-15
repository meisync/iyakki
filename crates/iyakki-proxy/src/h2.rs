//! The HTTP/2 client protocol (RFC 9113) as a state machine with no sockets: bytes from the server go in with [`Conn::feed`], bytes for the
//! server come out of [`Conn::take_output`], and what the server answered comes back as [`Event`]s. Many requests share one
//! connection (streams) with flow control in both directions; the headers are compressed by [`crate::hpack`]. It is connected to the
//! browser's HTTP client by [`crate::h2client`] (ALPN `h2`, a connection per host).
//!
//! The server's bytes are untrusted: frame sizes, header lists, concurrent streams, window arithmetic and every state are checked, and an
//! error closes the connection with a GOAWAY instead of going on.

use crate::hpack::{Decoder, Encoder, Header, HpackError};
use std::collections::HashMap;

pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
pub const DEFAULT_WINDOW: i64 = 65_535;
pub const DEFAULT_FRAME: usize = 16_384;
/// The window this side gives the server for what it sends (per stream, and for the connection).
pub const LOCAL_WINDOW: i64 = 1 << 20;
/// At most this many bytes of one response body are taken in (the rest of the stream is cancelled).
pub const MAX_BODY: usize = 64 << 20;
pub const MAX_STREAMS_OPEN: usize = 100;

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const PRIORITY: u8 = 2;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PUSH_PROMISE: u8 = 5;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;

const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const PRIORITY_FLAG: u8 = 0x20;

/// Error codes (RFC 9113 section 7).
pub mod code {
    pub const NO_ERROR: u32 = 0;
    pub const PROTOCOL_ERROR: u32 = 1;
    pub const FLOW_CONTROL_ERROR: u32 = 3;
    pub const FRAME_SIZE_ERROR: u32 = 6;
    pub const REFUSED_STREAM: u32 = 7;
    pub const CANCEL: u32 = 8;
    pub const COMPRESSION_ERROR: u32 = 9;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H2Error {
    /// The server broke the protocol (the words say how); the connection was told GOAWAY.
    Protocol(String),
    /// The connection cannot take another request now (the server's concurrency limit, or it said GOAWAY).
    Busy(String),
}

impl std::fmt::Display for H2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H2Error::Protocol(s) | H2Error::Busy(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for H2Error {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The response's status line and headers (`end` when there is no body).
    Headers { stream: u32, status: u16, headers: Vec<Header>, end: bool },
    Data { stream: u32, data: Vec<u8>, end: bool },
    /// The server (or this side, for a body over the limit) ended the stream with an error.
    Reset { stream: u32, code: u32 },
    /// The server will take no streams above `last`; `code` is why.
    GoAway { last: u32, code: u32 },
}

#[derive(Default)]
struct Stream {
    send_window: i64,
    recv_window: i64,
    /// The rest of the request body, and whether the stream ends with it.
    body: Vec<u8>,
    sent: usize,
    ends_with_body: bool,
    got_headers: bool,
    got: usize,
}

struct Frame<'a> {
    kind: u8,
    flags: u8,
    stream: u32,
    payload: &'a [u8],
}

pub fn frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    let n = payload.len();
    out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags]);
    out.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    out.extend_from_slice(payload);
}

pub struct Conn {
    out: Vec<u8>,
    input: Vec<u8>,
    enc: Encoder,
    dec: Decoder,
    peer_max_frame: usize,
    peer_max_streams: u32,
    peer_initial_window: i64,
    send_window: i64,
    recv_window: i64,
    streams: HashMap<u32, Stream>,
    next: u32,
    /// A header block that continues in CONTINUATION frames: (stream, bytes so far, ends the stream).
    open_block: Option<(u32, Vec<u8>, bool)>,
    goaway: Option<u32>,
    closed: bool,
}

impl Default for Conn {
    fn default() -> Self {
        Conn::new()
    }
}

impl Conn {
    /// A new connection: the preface and this side's SETTINGS (no server push, a big window) are ready to send.
    pub fn new() -> Conn {
        let mut c = Conn { out: Vec::new(), input: Vec::new(), enc: Encoder::default(), dec: Decoder::default(), peer_max_frame: DEFAULT_FRAME, peer_max_streams: 100, peer_initial_window: DEFAULT_WINDOW, send_window: DEFAULT_WINDOW, recv_window: LOCAL_WINDOW, streams: HashMap::new(), next: 1, open_block: None, goaway: None, closed: false };
        c.out.extend_from_slice(PREFACE);
        let mut s = Vec::new();
        for (id, v) in [(2u16, 0u32), (4, LOCAL_WINDOW as u32), (1, crate::hpack::DEFAULT_TABLE as u32)] {
            s.extend_from_slice(&id.to_be_bytes());
            s.extend_from_slice(&v.to_be_bytes());
        }
        frame(&mut c.out, SETTINGS, 0, 0, &s);
        frame(&mut c.out, WINDOW_UPDATE, 0, 0, &((LOCAL_WINDOW - DEFAULT_WINDOW) as u32).to_be_bytes());
        c
    }

    /// The bytes to send to the server now.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub fn open_streams(&self) -> usize {
        self.streams.len()
    }

    /// The connection is finished (the server said GOAWAY and nothing is left, or it broke the protocol).
    pub fn is_closed(&self) -> bool {
        self.closed || (self.goaway.is_some() && self.streams.is_empty())
    }

    /// Whether another request may be started now.
    pub fn can_start(&self) -> bool {
        !self.closed && self.goaway.is_none() && (self.streams.len() as u32) < self.peer_max_streams.min(MAX_STREAMS_OPEN as u32) && self.next < 0x7fff_fff0
    }

    /// Start a request: the pseudo-headers, the headers, and an optional body. The stream number comes back.
    pub fn request(&mut self, method: &str, scheme: &str, authority: &str, path: &str, headers: &[Header], body: Option<Vec<u8>>) -> Result<u32, H2Error> {
        if !self.can_start() {
            return Err(H2Error::Busy("the connection cannot take another request now".into()));
        }
        let id = self.next;
        self.next += 2;
        let mut list: Vec<Header> = vec![(":method".into(), method.into()), (":scheme".into(), scheme.into()), (":authority".into(), authority.into()), (":path".into(), path.into())];
        for (n, v) in headers {
            let n = n.to_ascii_lowercase();
            // the headers of the connection itself do not exist here; `host` is the authority
            if matches!(n.as_str(), "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade" | "host") || n.starts_with(':') {
                continue;
            }
            list.push((n, v.clone()));
        }
        let block = self.enc.encode(&list);
        let end = body.as_ref().is_none_or(|b| b.is_empty());
        self.write_block(id, &block, end);
        let mut st = Stream { send_window: self.peer_initial_window, recv_window: LOCAL_WINDOW, ..Default::default() };
        if let Some(b) = body.filter(|b| !b.is_empty()) {
            st.body = b;
            st.ends_with_body = true;
        }
        if end {
            // a stream that ends with its headers still waits for the answer
        }
        self.streams.insert(id, st);
        self.pump_bodies();
        Ok(id)
    }

    fn write_block(&mut self, id: u32, block: &[u8], end_stream: bool) {
        let max = self.peer_max_frame;
        let mut chunks = block.chunks(max.max(1));
        let first = chunks.next().unwrap_or(&[]);
        let more: Vec<&[u8]> = chunks.collect();
        let mut flags = if end_stream { END_STREAM } else { 0 };
        if more.is_empty() {
            flags |= END_HEADERS;
        }
        frame(&mut self.out, HEADERS, flags, id, first);
        for (i, c) in more.iter().enumerate() {
            frame(&mut self.out, CONTINUATION, if i + 1 == more.len() { END_HEADERS } else { 0 }, id, c);
        }
    }

    /// Send as much of the waiting request bodies as the windows and the frame size allow.
    fn pump_bodies(&mut self) {
        let ids: Vec<u32> = self.streams.iter().filter(|(_, s)| s.sent < s.body.len()).map(|(k, _)| *k).collect();
        let mut ids = ids;
        ids.sort_unstable();
        for id in ids {
            loop {
                let Some(st) = self.streams.get_mut(&id) else { break };
                let left = st.body.len() - st.sent;
                let n = left.min(self.peer_max_frame).min(st.send_window.max(0) as usize).min(self.send_window.max(0) as usize);
                if n == 0 {
                    break;
                }
                let last = n == left && st.ends_with_body;
                let chunk = st.body[st.sent..st.sent + n].to_vec();
                st.sent += n;
                st.send_window -= n as i64;
                self.send_window -= n as i64;
                if st.sent == st.body.len() {
                    st.body = Vec::new();
                    st.sent = 0;
                }
                frame(&mut self.out, DATA, if last { END_STREAM } else { 0 }, id, &chunk);
                if last {
                    break;
                }
            }
        }
    }

    fn fail(&mut self, code: u32, why: &str) -> H2Error {
        let last = self.streams.keys().copied().max().unwrap_or(0);
        let mut p = last.to_be_bytes().to_vec();
        p.extend_from_slice(&code.to_be_bytes());
        p.extend_from_slice(why.as_bytes());
        frame(&mut self.out, GOAWAY, 0, 0, &p);
        self.closed = true;
        H2Error::Protocol(why.to_string())
    }

    /// Take in what the server sent (any amount, cut anywhere) and return what happened.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Event>, H2Error> {
        if self.closed {
            return Err(H2Error::Protocol("the connection is closed".into()));
        }
        self.input.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut at = 0usize;
        loop {
            let rest = &self.input[at..];
            if rest.len() < 9 {
                break;
            }
            let len = ((rest[0] as usize) << 16) | ((rest[1] as usize) << 8) | rest[2] as usize;
            if len > DEFAULT_FRAME {
                // this side never raised its SETTINGS_MAX_FRAME_SIZE
                self.input.drain(..at);
                return Err(self.fail(code::FRAME_SIZE_ERROR, "the server sent a frame bigger than the limit"));
            }
            if rest.len() < 9 + len {
                break;
            }
            let f = Frame { kind: rest[3], flags: rest[4], stream: u32::from_be_bytes([rest[5], rest[6], rest[7], rest[8]]) & 0x7fff_ffff, payload: &rest[9..9 + len] };
            let owned = (f.kind, f.flags, f.stream, f.payload.to_vec());
            at += 9 + len;
            let mut ev = Vec::new();
            if let Err(e) = self.on_frame(owned.0, owned.1, owned.2, &owned.3, &mut ev) {
                self.input.drain(..at);
                return Err(e);
            }
            events.extend(ev);
        }
        self.input.drain(..at);
        self.pump_bodies();
        Ok(events)
    }

    fn on_frame(&mut self, kind: u8, flags: u8, id: u32, p: &[u8], ev: &mut Vec<Event>) -> Result<(), H2Error> {
        if let Some((s, _, _)) = &self.open_block {
            if kind != CONTINUATION || id != *s {
                return Err(self.fail(code::PROTOCOL_ERROR, "a header block was interrupted by another frame"));
            }
        }
        match kind {
            SETTINGS => {
                if id != 0 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "SETTINGS on a stream"));
                }
                if flags & ACK != 0 {
                    return Ok(());
                }
                if p.len() % 6 != 0 {
                    return Err(self.fail(code::FRAME_SIZE_ERROR, "SETTINGS of a wrong size"));
                }
                for c in p.chunks(6) {
                    let (sid, v) = (u16::from_be_bytes([c[0], c[1]]), u32::from_be_bytes([c[2], c[3], c[4], c[5]]));
                    match sid {
                        1 => self.enc.set_peer_limit(v as usize),
                        3 => self.peer_max_streams = v,
                        4 => {
                            if v > 0x7fff_ffff {
                                return Err(self.fail(code::FLOW_CONTROL_ERROR, "a window above its limit"));
                            }
                            let d = v as i64 - self.peer_initial_window;
                            self.peer_initial_window = v as i64;
                            for s in self.streams.values_mut() {
                                s.send_window += d;
                            }
                        }
                        5 => {
                            if !(16_384..=16_777_215).contains(&v) {
                                return Err(self.fail(code::PROTOCOL_ERROR, "a frame size outside the allowed range"));
                            }
                            self.peer_max_frame = v as usize;
                        }
                        _ => {}
                    }
                }
                frame(&mut self.out, SETTINGS, ACK, 0, &[]);
            }
            PING => {
                if id != 0 || p.len() != 8 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "a PING that is not one"));
                }
                if flags & ACK == 0 {
                    frame(&mut self.out, PING, ACK, 0, p);
                }
            }
            WINDOW_UPDATE => {
                if p.len() != 4 {
                    return Err(self.fail(code::FRAME_SIZE_ERROR, "a WINDOW_UPDATE of a wrong size"));
                }
                let inc = (u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff) as i64;
                if inc == 0 {
                    if id == 0 {
                        return Err(self.fail(code::PROTOCOL_ERROR, "a window change of zero"));
                    }
                    self.reset(id, code::PROTOCOL_ERROR, ev);
                    return Ok(());
                }
                if id == 0 {
                    self.send_window += inc;
                    if self.send_window > 0x7fff_ffff {
                        return Err(self.fail(code::FLOW_CONTROL_ERROR, "the connection window overflowed"));
                    }
                } else if let Some(s) = self.streams.get_mut(&id) {
                    s.send_window += inc;
                    if s.send_window > 0x7fff_ffff {
                        self.reset(id, code::FLOW_CONTROL_ERROR, ev);
                    }
                }
            }
            GOAWAY => {
                if id != 0 || p.len() < 8 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "a GOAWAY that is not one"));
                }
                let last = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff;
                let c = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
                self.goaway = Some(last);
                // the streams the server never saw are refused; they can be asked again on another connection
                let refused: Vec<u32> = self.streams.keys().copied().filter(|s| *s > last).collect();
                for s in refused {
                    self.streams.remove(&s);
                    ev.push(Event::Reset { stream: s, code: code::REFUSED_STREAM });
                }
                ev.push(Event::GoAway { last, code: c });
            }
            RST_STREAM => {
                if id == 0 || p.len() != 4 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "a RST_STREAM that is not one"));
                }
                if self.streams.remove(&id).is_some() {
                    ev.push(Event::Reset { stream: id, code: u32::from_be_bytes([p[0], p[1], p[2], p[3]]) });
                }
            }
            PRIORITY => {}
            PUSH_PROMISE => return Err(self.fail(code::PROTOCOL_ERROR, "the server pushed although push was switched off")),
            HEADERS => {
                if id == 0 || id % 2 == 0 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "HEADERS on a stream this side did not open"));
                }
                let mut body = p;
                if flags & PADDED != 0 {
                    let pad = *body.first().ok_or_else(|| self.fail(code::FRAME_SIZE_ERROR, "padding that is not there"))? as usize;
                    if pad + 1 > body.len() {
                        return Err(self.fail(code::PROTOCOL_ERROR, "more padding than frame"));
                    }
                    body = &body[1..body.len() - pad];
                }
                if flags & PRIORITY_FLAG != 0 {
                    if body.len() < 5 {
                        return Err(self.fail(code::FRAME_SIZE_ERROR, "a priority that is not there"));
                    }
                    body = &body[5..];
                }
                if flags & END_HEADERS != 0 {
                    self.headers_done(id, body, flags & END_STREAM != 0, ev)?;
                } else {
                    self.open_block = Some((id, body.to_vec(), flags & END_STREAM != 0));
                }
            }
            CONTINUATION => {
                let Some((s, mut block, end)) = self.open_block.take() else { return Err(self.fail(code::PROTOCOL_ERROR, "CONTINUATION without a header block")) };
                if s != id {
                    return Err(self.fail(code::PROTOCOL_ERROR, "CONTINUATION on another stream"));
                }
                block.extend_from_slice(p);
                if block.len() > crate::hpack::MAX_LIST {
                    return Err(self.fail(code::COMPRESSION_ERROR, "the header block is too large"));
                }
                if flags & END_HEADERS != 0 {
                    self.headers_done(id, &block, end, ev)?;
                } else {
                    self.open_block = Some((id, block, end));
                }
            }
            DATA => {
                if id == 0 {
                    return Err(self.fail(code::PROTOCOL_ERROR, "DATA on stream 0"));
                }
                let mut data = p;
                if flags & PADDED != 0 {
                    let pad = *data.first().ok_or_else(|| self.fail(code::FRAME_SIZE_ERROR, "padding that is not there"))? as usize;
                    if pad + 1 > data.len() {
                        return Err(self.fail(code::PROTOCOL_ERROR, "more padding than frame"));
                    }
                    data = &data[1..data.len() - pad];
                }
                // the whole frame counts against the windows, padding too
                self.recv_window -= p.len() as i64;
                if self.recv_window < 0 {
                    return Err(self.fail(code::FLOW_CONTROL_ERROR, "the server sent more than the connection window"));
                }
                let end = flags & END_STREAM != 0;
                let known = self.streams.contains_key(&id);
                if known {
                    let st = self.streams.get_mut(&id).expect("checked");
                    st.recv_window -= p.len() as i64;
                    st.got += data.len();
                    if st.recv_window < 0 {
                        self.reset(id, code::FLOW_CONTROL_ERROR, ev);
                    } else if st.got > MAX_BODY {
                        self.reset(id, code::CANCEL, ev);
                    } else if !st.got_headers {
                        return Err(self.fail(code::PROTOCOL_ERROR, "DATA before the headers"));
                    } else {
                        ev.push(Event::Data { stream: id, data: data.to_vec(), end });
                        if end {
                            self.streams.remove(&id);
                        }
                    }
                }
                // give the window back as it is used: the connection's and a live stream's when half of it is gone
                if self.recv_window < LOCAL_WINDOW / 2 {
                    let add = LOCAL_WINDOW - self.recv_window;
                    frame(&mut self.out, WINDOW_UPDATE, 0, 0, &(add as u32).to_be_bytes());
                    self.recv_window = LOCAL_WINDOW;
                }
                if let Some(st) = self.streams.get_mut(&id) {
                    if st.recv_window < LOCAL_WINDOW / 2 {
                        let add = LOCAL_WINDOW - st.recv_window;
                        st.recv_window = LOCAL_WINDOW;
                        frame(&mut self.out, WINDOW_UPDATE, 0, id, &(add as u32).to_be_bytes());
                    }
                }
            }
            _ => {} // frames of kinds this side does not know are ignored, as the standard says
        }
        Ok(())
    }

    fn reset(&mut self, id: u32, c: u32, ev: &mut Vec<Event>) {
        frame(&mut self.out, RST_STREAM, 0, id, &c.to_be_bytes());
        self.streams.remove(&id);
        ev.push(Event::Reset { stream: id, code: c });
    }

    fn headers_done(&mut self, id: u32, block: &[u8], end: bool, ev: &mut Vec<Event>) -> Result<(), H2Error> {
        // the block must be decoded even for a stream that is gone: the table is shared
        let headers = match self.dec.decode(block) {
            Ok(h) => h,
            Err(e @ (HpackError::TooLarge | HpackError::BadSizeUpdate)) => return Err(self.fail(code::COMPRESSION_ERROR, &e.to_string())),
            Err(e) => return Err(self.fail(code::COMPRESSION_ERROR, &e.to_string())),
        };
        let Some(st) = self.streams.get_mut(&id) else { return Ok(()) };
        if st.got_headers {
            // trailers: the end of the stream, nothing else is kept
            if !end {
                return Err(self.fail(code::PROTOCOL_ERROR, "trailers that do not end the stream"));
            }
            self.streams.remove(&id);
            ev.push(Event::Data { stream: id, data: Vec::new(), end: true });
            return Ok(());
        }
        let status = headers.iter().find(|(n, _)| n == ":status").and_then(|(_, v)| v.parse::<u16>().ok());
        let Some(status) = status else {
            self.reset(id, code::PROTOCOL_ERROR, ev);
            return Ok(());
        };
        if (100..200).contains(&status) {
            return Ok(()); // an interim answer (100 Continue): the real one follows
        }
        if headers.iter().any(|(n, _)| n.starts_with(':') && n != ":status") {
            self.reset(id, code::PROTOCOL_ERROR, ev);
            return Ok(());
        }
        st.got_headers = true;
        let headers: Vec<Header> = headers.into_iter().filter(|(n, _)| !n.starts_with(':')).collect();
        ev.push(Event::Headers { stream: id, status, headers, end });
        if end {
            self.streams.remove(&id);
        }
        Ok(())
    }

    /// Give up on a request: the server is told to stop.
    pub fn cancel(&mut self, id: u32) {
        if self.streams.remove(&id).is_some() {
            frame(&mut self.out, RST_STREAM, 0, id, &code::CANCEL.to_be_bytes());
        }
    }

    /// Say goodbye (a clean end of the connection).
    pub fn close(&mut self) {
        if !self.closed {
            let last = 0u32;
            let mut p = last.to_be_bytes().to_vec();
            p.extend_from_slice(&code::NO_ERROR.to_be_bytes());
            frame(&mut self.out, GOAWAY, 0, 0, &p);
            self.closed = true;
        }
    }
}

#[cfg(test)]
#[path = "h2_tests.rs"]
mod tests;
