//! The SFTP CLIENT side, protocol version 3 (draft-ietf-secsh-filexfer-02) for the remote files client (`remotefiles.rs`).
//!
//! * [`encode`] and [`parse_response`] are pure: bytes in, typed answers out. They are the mirror of `sftp_proto.rs` (the server's codec:
//!   it parses requests and writes answers; this one writes requests and parses answers). They share only the packet numbers by value.
//!   Folding the two into one module that the server can use too is a later cleanup; the server agent's files were not touched.
//! * Hostile servers are expected: every length is checked against what is there before anything is allocated, a packet is never over
//!   [`MAX_PACKET`], counts are bounded, a data block is never longer than what was asked, and names that are paths (`a/b`, `..`, empty)
//!   are dropped from listings ([`plain_name`]).
//! * [`SftpFs`] speaks over any stream (the SSH channel, or a loopback socket in tests) and implements [`crate::remotefs::RemoteFs`].

use std::collections::VecDeque;
use std::io::{Read, Write};

use crate::remotefs::{Entry, RemoteFs};

pub const MAX_PACKET: usize = 256 * 1024;
/// The data block of one READ or WRITE.
pub const BLOCK: u32 = 32 * 1024;
/// Requests in flight at once while moving a file.
const DEPTH: usize = 8;
const MAX_NAMES: usize = 50_000;

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const FSTAT: u8 = 8;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const REMOVE: u8 = 13;
const MKDIR: u8 = 14;
const RMDIR: u8 = 15;
const REALPATH: u8 = 16;
const STAT: u8 = 17;
const RENAME: u8 = 18;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;

pub const OK: u32 = 0;
pub const EOF: u32 = 1;
pub const NO_SUCH_FILE: u32 = 2;
pub const PERMISSION_DENIED: u32 = 3;

pub const O_READ: u32 = 1;
pub const O_WRITE: u32 = 2;
pub const O_CREAT: u32 = 8;
pub const O_TRUNC: u32 = 0x10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoError {
    Truncated,
    TooLong,
    Unknown(u8),
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Truncated => f.write_str("the server's answer was cut short"),
            ProtoError::TooLong => f.write_str("the server sent a length that is too large"),
            ProtoError::Unknown(t) => write!(f, "the server sent an unknown packet type {t}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub perms: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.perms.is_some_and(|p| p & 0o170000 == 0o040000)
    }
    pub fn is_link(&self) -> bool {
        self.perms.is_some_and(|p| p & 0o170000 == 0o120000)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resp {
    Version(u32),
    Status { id: u32, code: u32, msg: String },
    Handle { id: u32, handle: Vec<u8> },
    Data { id: u32, data: Vec<u8> },
    Name { id: u32, names: Vec<(String, Attrs)> },
    Attrs { id: u32, attrs: Attrs },
}

impl Resp {
    pub fn id(&self) -> Option<u32> {
        match self {
            Resp::Version(_) => None,
            Resp::Status { id, .. } | Resp::Handle { id, .. } | Resp::Data { id, .. } | Resp::Name { id, .. } | Resp::Attrs { id, .. } => Some(*id),
        }
    }
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, ProtoError> {
        let (h, t) = self.0.split_first().ok_or(ProtoError::Truncated)?;
        self.0 = t;
        Ok(*h)
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        let (h, t) = self.0.split_first_chunk::<4>().ok_or(ProtoError::Truncated)?;
        self.0 = t;
        Ok(u32::from_be_bytes(*h))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        let (h, t) = self.0.split_first_chunk::<8>().ok_or(ProtoError::Truncated)?;
        self.0 = t;
        Ok(u64::from_be_bytes(*h))
    }
    fn bytes(&mut self) -> Result<&'a [u8], ProtoError> {
        let n = self.u32()? as usize;
        if n > MAX_PACKET {
            return Err(ProtoError::TooLong);
        }
        if n > self.0.len() {
            return Err(ProtoError::Truncated);
        }
        let (h, t) = self.0.split_at(n);
        self.0 = t;
        Ok(h)
    }
}

fn attrs(r: &mut Rd) -> Result<Attrs, ProtoError> {
    let flags = r.u32()?;
    let mut a = Attrs::default();
    if flags & 1 != 0 {
        a.size = Some(r.u64()?);
    }
    if flags & 2 != 0 {
        r.u32()?;
        r.u32()?;
    }
    if flags & 4 != 0 {
        a.perms = Some(r.u32()?);
    }
    if flags & 8 != 0 {
        r.u32()?;
        a.mtime = Some(r.u32()?);
    }
    if flags & 0x8000_0000 != 0 {
        let n = r.u32()?;
        if n > 64 {
            return Err(ProtoError::TooLong);
        }
        for _ in 0..n {
            r.bytes()?;
            r.bytes()?;
        }
    }
    Ok(a)
}

/// A name from a listing is a plain name: no folders in it, not `.` or `..`, not empty, no NUL.
pub fn plain_name(n: &str) -> bool {
    !n.is_empty() && n != "." && n != ".." && !n.contains(['/', '\0']) && n.len() <= 1024
}

/// Parse one packet's payload (the type byte first, no length prefix).
pub fn parse_response(payload: &[u8]) -> Result<Resp, ProtoError> {
    if payload.len() > MAX_PACKET {
        return Err(ProtoError::TooLong);
    }
    let mut r = Rd(payload);
    let t = r.u8()?;
    if t == VERSION {
        let v = r.u32()?;
        return Ok(Resp::Version(v));
    }
    let id = r.u32()?;
    match t {
        STATUS => {
            let code = r.u32()?;
            let msg = r.bytes().map(|b| String::from_utf8_lossy(b).chars().filter(|c| !c.is_control()).take(200).collect()).unwrap_or_default();
            Ok(Resp::Status { id, code, msg })
        }
        HANDLE => {
            let h = r.bytes()?;
            if h.is_empty() || h.len() > 256 {
                return Err(ProtoError::TooLong);
            }
            Ok(Resp::Handle { id, handle: h.to_vec() })
        }
        DATA => Ok(Resp::Data { id, data: r.bytes()?.to_vec() }),
        NAME => {
            let count = r.u32()? as usize;
            // each name needs at least 12 bytes (two empty strings and empty attributes), so a count beyond that is a lie
            if count > MAX_NAMES || count > r.0.len() / 12 + 1 {
                return Err(ProtoError::TooLong);
            }
            let mut names = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                let name = r.bytes()?;
                let _long = r.bytes()?;
                let a = attrs(&mut r)?;
                names.push((String::from_utf8_lossy(name).into_owned(), a));
            }
            Ok(Resp::Name { id, names })
        }
        ATTRS => Ok(Resp::Attrs { id, attrs: attrs(&mut r)? }),
        t => Err(ProtoError::Unknown(t)),
    }
}

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}
fn put_str(o: &mut Vec<u8>, s: &[u8]) {
    put_u32(o, s.len() as u32);
    o.extend_from_slice(s);
}

/// A request: type, id, and the fields. Returns the framed packet (length prefix included).
pub enum Req<'a> {
    Open(&'a str, u32),
    Close(&'a [u8]),
    Read(&'a [u8], u64, u32),
    Write(&'a [u8], u64, &'a [u8]),
    Fstat(&'a [u8]),
    OpenDir(&'a str),
    ReadDir(&'a [u8]),
    Remove(&'a str),
    Mkdir(&'a str),
    Rmdir(&'a str),
    RealPath(&'a str),
    Stat(&'a str),
    Rename(&'a str, &'a str),
}

pub fn encode(id: u32, r: &Req) -> Vec<u8> {
    let mut b = Vec::new();
    let no_attrs = |b: &mut Vec<u8>| put_u32(b, 0);
    match r {
        Req::Open(p, flags) => {
            b.push(OPEN);
            put_u32(&mut b, id);
            put_str(&mut b, p.as_bytes());
            put_u32(&mut b, *flags);
            no_attrs(&mut b);
        }
        Req::Close(h) => {
            b.push(CLOSE);
            put_u32(&mut b, id);
            put_str(&mut b, h);
        }
        Req::Read(h, off, len) => {
            b.push(READ);
            put_u32(&mut b, id);
            put_str(&mut b, h);
            b.extend_from_slice(&off.to_be_bytes());
            put_u32(&mut b, *len);
        }
        Req::Write(h, off, data) => {
            b.push(WRITE);
            put_u32(&mut b, id);
            put_str(&mut b, h);
            b.extend_from_slice(&off.to_be_bytes());
            put_str(&mut b, data);
        }
        Req::Fstat(h) => {
            b.push(FSTAT);
            put_u32(&mut b, id);
            put_str(&mut b, h);
        }
        Req::OpenDir(p) | Req::Remove(p) | Req::Rmdir(p) | Req::RealPath(p) | Req::Stat(p) => {
            b.push(match r {
                Req::OpenDir(_) => OPENDIR,
                Req::Remove(_) => REMOVE,
                Req::Rmdir(_) => RMDIR,
                Req::RealPath(_) => REALPATH,
                _ => STAT,
            });
            put_u32(&mut b, id);
            put_str(&mut b, p.as_bytes());
        }
        Req::ReadDir(h) => {
            b.push(READDIR);
            put_u32(&mut b, id);
            put_str(&mut b, h);
        }
        Req::Mkdir(p) => {
            b.push(MKDIR);
            put_u32(&mut b, id);
            put_str(&mut b, p.as_bytes());
            no_attrs(&mut b);
        }
        Req::Rename(a, c) => {
            b.push(RENAME);
            put_u32(&mut b, id);
            put_str(&mut b, a.as_bytes());
            put_str(&mut b, c.as_bytes());
        }
    }
    frame(&b)
}

pub fn init_packet() -> Vec<u8> {
    let mut b = vec![INIT];
    put_u32(&mut b, 3);
    frame(&b)
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(payload.len() + 4);
    put_u32(&mut o, payload.len() as u32);
    o.extend_from_slice(payload);
    o
}

fn status_text(code: u32, msg: &str) -> String {
    let base = match code {
        NO_SUCH_FILE => "no such file or folder",
        PERMISSION_DENIED => "permission denied",
        EOF => "end of file",
        4 => "the server reported a failure",
        5 => "the server did not understand the request",
        6 => "no connection",
        7 => "the connection was lost",
        8 => "the server does not support that",
        _ => "the server refused",
    };
    if msg.is_empty() { base.into() } else { format!("{base} ({msg})") }
}

/// An SFTP session over a stream.
pub struct SftpFs<S: Read + Write> {
    s: S,
    next: u32,
    rest: VecDeque<u8>,
}

impl<S: Read + Write + Send> SftpFs<S> {
    /// Does the INIT/VERSION handshake.
    pub fn start(mut s: S) -> Result<SftpFs<S>, String> {
        s.write_all(&init_packet()).map_err(io)?;
        let mut c = SftpFs { s, next: 1, rest: VecDeque::new() };
        match c.recv()? {
            Resp::Version(v) if v >= 3 => Ok(c),
            Resp::Version(v) => Err(format!("the server speaks SFTP version {v}; version 3 or newer is needed")),
            _ => Err("the server did not answer the SFTP hello".into()),
        }
    }

    fn fill(&mut self, n: usize) -> Result<(), String> {
        let mut buf = [0u8; 8192];
        while self.rest.len() < n {
            let want = (n - self.rest.len()).min(buf.len());
            let got = self.s.read(&mut buf[..want]).map_err(io)?;
            if got == 0 {
                return Err("the connection was closed by the server".into());
            }
            self.rest.extend(&buf[..got]);
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Resp, String> {
        self.fill(4)?;
        let head: Vec<u8> = self.rest.drain(..4).collect();
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if len == 0 || len > MAX_PACKET {
            return Err(format!("the server sent a packet of {len} bytes, which is not allowed"));
        }
        self.fill(len)?;
        let body: Vec<u8> = self.rest.drain(..len).collect();
        parse_response(&body).map_err(|e| e.to_string())
    }

    fn send(&mut self, id: u32, r: &Req) -> Result<(), String> {
        self.s.write_all(&encode(id, r)).map_err(io)
    }

    fn id(&mut self) -> u32 {
        self.next = self.next.wrapping_add(1);
        self.next
    }

    /// One request and its answer.
    fn call(&mut self, r: &Req) -> Result<Resp, String> {
        let id = self.id();
        self.send(id, r)?;
        let a = self.recv()?;
        if a.id() != Some(id) {
            return Err("the server answered a different request".into());
        }
        Ok(a)
    }

    fn expect_ok(&mut self, r: &Req) -> Result<(), String> {
        match self.call(r)? {
            Resp::Status { code: OK, .. } => Ok(()),
            Resp::Status { code, msg, .. } => Err(status_text(code, &msg)),
            _ => Err("the server answered with the wrong kind of packet".into()),
        }
    }

    fn handle_of(&mut self, r: &Req) -> Result<Vec<u8>, String> {
        match self.call(r)? {
            Resp::Handle { handle, .. } => Ok(handle),
            Resp::Status { code, msg, .. } => Err(status_text(code, &msg)),
            _ => Err("the server answered with the wrong kind of packet".into()),
        }
    }

    fn close(&mut self, h: &[u8]) {
        let _ = self.expect_ok(&Req::Close(h));
    }

    fn real(&mut self, p: &str) -> Result<String, String> {
        match self.call(&Req::RealPath(p))? {
            Resp::Name { names, .. } if names.first().is_some_and(|n| n.0.starts_with('/') && !n.0.contains('\0')) => Ok(crate::remotefs::clean_path(&names[0].0)),
            Resp::Name { .. } => Ok(crate::remotefs::clean_path(p)),
            Resp::Status { code, msg, .. } => Err(status_text(code, &msg)),
            _ => Err("the server answered with the wrong kind of packet".into()),
        }
    }
}

fn io(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => "the connection was lost".into(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "the server did not answer in time".into(),
        _ => format!("network error: {}", e.kind()),
    }
}

fn entry_of(name: String, a: &Attrs) -> Entry {
    let perms = a.perms.map(|p| {
        let mut s = String::new();
        for sh in [6, 3, 0] {
            let b = (p >> sh) & 7;
            s.push(if b & 4 != 0 { 'r' } else { '-' });
            s.push(if b & 2 != 0 { 'w' } else { '-' });
            s.push(if b & 1 != 0 { 'x' } else { '-' });
        }
        s
    });
    Entry { name, is_dir: a.is_dir(), is_link: a.is_link(), size: a.size.unwrap_or(0), modified: a.mtime.map_or(0, i64::from), perms: perms.unwrap_or_default() }
}

impl<S: Read + Write + Send> RemoteFs for SftpFs<S> {
    fn list(&mut self, path: &str) -> Result<(String, Vec<Entry>), String> {
        let canon = self.real(path)?;
        let h = self.handle_of(&Req::OpenDir(&canon))?;
        let mut out = Vec::new();
        let r = (|| loop {
            match self.call(&Req::ReadDir(&h))? {
                Resp::Name { names, .. } => {
                    for (n, a) in names {
                        if plain_name(&n) {
                            out.push(entry_of(n, &a));
                        }
                    }
                    if out.len() > 200_000 {
                        return Err("the folder has more than 200000 entries".to_string());
                    }
                }
                Resp::Status { code: EOF, .. } => return Ok(()),
                Resp::Status { code, msg, .. } => return Err(status_text(code, &msg)),
                _ => return Err("the server answered with the wrong kind of packet".to_string()),
            }
        })();
        self.close(&h);
        r.map(|_| (canon, out))
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        self.expect_ok(&Req::Mkdir(path))
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        self.expect_ok(&Req::Rename(from, to))
    }
    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        self.expect_ok(&Req::Remove(path))
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        self.expect_ok(&Req::Rmdir(path))
    }
    fn size(&mut self, path: &str) -> Result<Option<u64>, String> {
        match self.call(&Req::Stat(path))? {
            Resp::Attrs { attrs, .. } => Ok(Some(attrs.size.unwrap_or(0))),
            Resp::Status { code: NO_SUCH_FILE, .. } => Ok(None),
            Resp::Status { code, msg, .. } => Err(status_text(code, &msg)),
            _ => Err("the server answered with the wrong kind of packet".into()),
        }
    }

    fn download(&mut self, remote: &str, offset: u64, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        let h = self.handle_of(&Req::Open(remote, O_READ))?;
        let r = (|| -> Result<(), String> {
            let mut pos = offset;
            let mut eof = false;
            let mut inflight: VecDeque<(u32, u64)> = VecDeque::new();
            let mut next_off = pos;
            loop {
                while !eof && inflight.len() < DEPTH {
                    let id = self.id();
                    self.send(id, &Req::Read(&h, next_off, BLOCK))?;
                    inflight.push_back((id, next_off));
                    next_off += BLOCK as u64;
                }
                let Some((id, off)) = inflight.pop_front() else { return Ok(()) };
                let a = self.recv()?;
                if a.id() != Some(id) {
                    return Err("the server answered out of order".into());
                }
                match a {
                    Resp::Data { data, .. } => {
                        if data.len() > BLOCK as usize {
                            return Err("the server sent more data than was asked for".into());
                        }
                        if off != pos {
                            // an earlier block came back short: this one is at the wrong place; drop it
                            continue;
                        }
                        if data.is_empty() {
                            return Err("the server sent an empty block".into());
                        }
                        out.write_all(&data).map_err(|e| format!("could not write the local file: {}", e.kind()))?;
                        pos += data.len() as u64;
                        if !progress(data.len() as u64) {
                            return Err("cancelled".into());
                        }
                        if data.len() < BLOCK as usize {
                            // short block: everything requested after it is at the wrong offset; ask again from here
                            while let Some((i2, _)) = inflight.pop_front() {
                                let _ = (i2, self.recv()?);
                            }
                            next_off = pos;
                        }
                    }
                    Resp::Status { code: EOF, .. } => {
                        if off == pos {
                            eof = true;
                        }
                    }
                    Resp::Status { code, msg, .. } => return Err(status_text(code, &msg)),
                    _ => return Err("the server answered with the wrong kind of packet".into()),
                }
            }
        })();
        self.close(&h);
        r
    }

    fn upload(&mut self, remote: &str, offset: u64, src: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        let flags = O_WRITE | O_CREAT | if offset == 0 { O_TRUNC } else { 0 };
        let h = self.handle_of(&Req::Open(remote, flags))?;
        let r = (|| -> Result<(), String> {
            let mut pos = offset;
            let mut inflight: VecDeque<(u32, u64)> = VecDeque::new();
            let mut buf = vec![0u8; BLOCK as usize];
            let mut done = false;
            loop {
                while !done && inflight.len() < DEPTH {
                    let mut n = 0;
                    while n < buf.len() {
                        let k = src.read(&mut buf[n..]).map_err(|e| format!("could not read the local file: {}", e.kind()))?;
                        if k == 0 {
                            done = true;
                            break;
                        }
                        n += k;
                    }
                    if n == 0 {
                        break;
                    }
                    let id = self.id();
                    self.send(id, &Req::Write(&h, pos, &buf[..n]))?;
                    inflight.push_back((id, n as u64));
                    pos += n as u64;
                }
                let Some((id, n)) = inflight.pop_front() else { return Ok(()) };
                let a = self.recv()?;
                if a.id() != Some(id) {
                    return Err("the server answered out of order".into());
                }
                match a {
                    Resp::Status { code: OK, .. } => {
                        if !progress(n) {
                            // finish nothing more: the answers still in flight are read and dropped by the closing of the connection
                            return Err("cancelled".into());
                        }
                    }
                    Resp::Status { code, msg, .. } => return Err(status_text(code, &msg)),
                    _ => return Err("the server answered with the wrong kind of packet".into()),
                }
            }
        })();
        if r.is_ok() {
            // a server may report a late write error only at close
            let id = self.id();
            self.send(id, &Req::Close(&h))?;
            return match self.recv()? {
                Resp::Status { code: OK, .. } => Ok(()),
                Resp::Status { code, msg, .. } => Err(status_text(code, &msg)),
                _ => Err("the server answered with the wrong kind of packet".into()),
            };
        }
        self.close(&h);
        r
    }
}
