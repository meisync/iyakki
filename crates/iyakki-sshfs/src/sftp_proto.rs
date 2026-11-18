//! The SFTP packet codec, protocol version 3 (draft-ietf-secsh-filexfer-02, the version every client speaks). Pure: bytes in, typed
//! requests out, and typed answers to bytes. No file, socket or clock is touched here (see `sftp_fs.rs` and `sftp_session.rs`).
//!
//! Hostile input is expected. Every length is checked against what is really there before anything is allocated; a packet is never
//! longer than [`MAX_PACKET`]; counts (attribute extensions, names) are bounded; a request that cannot be parsed is a
//! [`ProtoError`], never a panic. Text fields are bytes on the wire; paths are turned into `String` only when they are valid UTF-8
//! (anything else is refused, so a path can never be smuggled in as raw bytes).

/// The largest packet (type byte plus body) either side may send: 256 KiB, the size OpenSSH uses.
pub const MAX_PACKET: usize = 256 * 1024;
/// The largest data block one READ may return.
pub const MAX_READ: u32 = 64 * 1024;
/// The most extended attribute pairs read from one ATTRS (they are ignored).
const MAX_EXTENDED: u32 = 16;
/// The longest path or name accepted.
pub const MAX_PATH: usize = 4096;

pub const FXP_INIT: u8 = 1;
pub const FXP_VERSION: u8 = 2;
pub const FXP_OPEN: u8 = 3;
pub const FXP_CLOSE: u8 = 4;
pub const FXP_READ: u8 = 5;
pub const FXP_WRITE: u8 = 6;
pub const FXP_LSTAT: u8 = 7;
pub const FXP_FSTAT: u8 = 8;
pub const FXP_SETSTAT: u8 = 9;
pub const FXP_FSETSTAT: u8 = 10;
pub const FXP_OPENDIR: u8 = 11;
pub const FXP_READDIR: u8 = 12;
pub const FXP_REMOVE: u8 = 13;
pub const FXP_MKDIR: u8 = 14;
pub const FXP_RMDIR: u8 = 15;
pub const FXP_REALPATH: u8 = 16;
pub const FXP_STAT: u8 = 17;
pub const FXP_RENAME: u8 = 18;
pub const FXP_READLINK: u8 = 19;
pub const FXP_SYMLINK: u8 = 20;
pub const FXP_STATUS: u8 = 101;
pub const FXP_HANDLE: u8 = 102;
pub const FXP_DATA: u8 = 103;
pub const FXP_NAME: u8 = 104;
pub const FXP_ATTRS: u8 = 105;
pub const FXP_EXTENDED: u8 = 200;

pub const FX_OK: u32 = 0;
pub const FX_EOF: u32 = 1;
pub const FX_NO_SUCH_FILE: u32 = 2;
pub const FX_PERMISSION_DENIED: u32 = 3;
pub const FX_FAILURE: u32 = 4;
pub const FX_BAD_MESSAGE: u32 = 5;
pub const FX_OP_UNSUPPORTED: u32 = 8;

pub const OPEN_READ: u32 = 0x01;
pub const OPEN_WRITE: u32 = 0x02;
pub const OPEN_APPEND: u32 = 0x04;
pub const OPEN_CREAT: u32 = 0x08;
pub const OPEN_TRUNC: u32 = 0x10;
pub const OPEN_EXCL: u32 = 0x20;

const ATTR_SIZE: u32 = 1;
const ATTR_UIDGID: u32 = 2;
const ATTR_PERMS: u32 = 4;
const ATTR_TIMES: u32 = 8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoError {
    /// The packet is cut short or a length points past its end.
    Truncated,
    /// A length (packet, string, count) is over its limit.
    TooLong,
    /// A text field is not valid UTF-8, or has a NUL.
    BadText,
    /// A packet type that does not exist in version 3.
    UnknownType(u8),
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Truncated => write!(f, "packet cut short"),
            ProtoError::TooLong => write!(f, "length over the limit"),
            ProtoError::BadText => write!(f, "text is not valid"),
            ProtoError::UnknownType(t) => write!(f, "unknown packet type {t}"),
        }
    }
}

/// File attributes (the version 3 set). A field is `None` when the sender did not give it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid_gid: Option<(u32, u32)>,
    pub perms: Option<u32>,
    pub times: Option<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    Init { version: u32 },
    Open { id: u32, path: String, flags: u32, attrs: Attrs },
    Close { id: u32, handle: Vec<u8> },
    Read { id: u32, handle: Vec<u8>, offset: u64, len: u32 },
    Write { id: u32, handle: Vec<u8>, offset: u64, data: Vec<u8> },
    Lstat { id: u32, path: String },
    Fstat { id: u32, handle: Vec<u8> },
    Setstat { id: u32, path: String, attrs: Attrs },
    Fsetstat { id: u32, handle: Vec<u8>, attrs: Attrs },
    Opendir { id: u32, path: String },
    Readdir { id: u32, handle: Vec<u8> },
    Remove { id: u32, path: String },
    Mkdir { id: u32, path: String, attrs: Attrs },
    Rmdir { id: u32, path: String },
    Realpath { id: u32, path: String },
    Stat { id: u32, path: String },
    Rename { id: u32, from: String, to: String },
    Readlink { id: u32, path: String },
    Symlink { id: u32 },
    Extended { id: u32, name: String },
}

impl Req {
    /// The request id an answer must carry (`None` for INIT).
    pub fn id(&self) -> Option<u32> {
        match self {
            Req::Init { .. } => None,
            Req::Open { id, .. } | Req::Close { id, .. } | Req::Read { id, .. } | Req::Write { id, .. } | Req::Lstat { id, .. } | Req::Fstat { id, .. } | Req::Setstat { id, .. } | Req::Fsetstat { id, .. } | Req::Opendir { id, .. } | Req::Readdir { id, .. } | Req::Remove { id, .. } | Req::Mkdir { id, .. } | Req::Rmdir { id, .. } | Req::Realpath { id, .. } | Req::Stat { id, .. } | Req::Rename { id, .. } | Req::Readlink { id, .. } | Req::Symlink { id } | Req::Extended { id, .. } => Some(*id),
        }
    }
}

struct Rd<'a> {
    b: &'a [u8],
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if n > self.b.len() {
            return Err(ProtoError::Truncated);
        }
        let (h, t) = self.b.split_at(n);
        self.b = t;
        Ok(h)
    }
    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| ProtoError::Truncated)?))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| ProtoError::Truncated)?))
    }
    /// A length-prefixed byte string; the length is checked against what is left before anything is copied.
    fn bytes(&mut self) -> Result<&'a [u8], ProtoError> {
        let n = self.u32()? as usize;
        if n > MAX_PACKET {
            return Err(ProtoError::TooLong);
        }
        self.take(n)
    }
    fn text(&mut self) -> Result<String, ProtoError> {
        let b = self.bytes()?;
        if b.len() > MAX_PATH {
            return Err(ProtoError::TooLong);
        }
        let s = std::str::from_utf8(b).map_err(|_| ProtoError::BadText)?;
        if s.contains('\0') {
            return Err(ProtoError::BadText);
        }
        Ok(s.to_string())
    }
    fn handle(&mut self) -> Result<Vec<u8>, ProtoError> {
        let b = self.bytes()?;
        if b.len() > 256 {
            return Err(ProtoError::TooLong);
        }
        Ok(b.to_vec())
    }
    fn attrs(&mut self) -> Result<Attrs, ProtoError> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            a.uid_gid = Some((self.u32()?, self.u32()?));
        }
        if flags & ATTR_PERMS != 0 {
            a.perms = Some(self.u32()?);
        }
        if flags & ATTR_TIMES != 0 {
            a.times = Some((self.u32()?, self.u32()?));
        }
        if flags & ATTR_EXTENDED != 0 {
            let n = self.u32()?;
            if n > MAX_EXTENDED {
                return Err(ProtoError::TooLong);
            }
            for _ in 0..n {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(a)
    }
}

/// The length a frame announces (its first four bytes); `Err(TooLong)` when it is zero or over [`MAX_PACKET`], so a hostile length never
/// makes the reader wait for or allocate gigabytes.
pub fn frame_len(head: [u8; 4]) -> Result<usize, ProtoError> {
    let n = u32::from_be_bytes(head) as usize;
    if n == 0 || n > MAX_PACKET {
        Err(ProtoError::TooLong)
    } else {
        Ok(n)
    }
}

/// Parse one packet's payload (the type byte and the body, without the 4-byte length).
pub fn parse_request(payload: &[u8]) -> Result<Req, ProtoError> {
    if payload.len() > MAX_PACKET {
        return Err(ProtoError::TooLong);
    }
    let mut r = Rd { b: payload };
    let t = r.u8()?;
    if t == FXP_INIT {
        return Ok(Req::Init { version: r.u32()? });
    }
    let id = r.u32()?;
    let req = match t {
        FXP_OPEN => Req::Open { id, path: r.text()?, flags: r.u32()?, attrs: r.attrs()? },
        FXP_CLOSE => Req::Close { id, handle: r.handle()? },
        FXP_READ => Req::Read { id, handle: r.handle()?, offset: r.u64()?, len: r.u32()? },
        FXP_WRITE => Req::Write { id, handle: r.handle()?, offset: r.u64()?, data: r.bytes()?.to_vec() },
        FXP_LSTAT => Req::Lstat { id, path: r.text()? },
        FXP_FSTAT => Req::Fstat { id, handle: r.handle()? },
        FXP_SETSTAT => Req::Setstat { id, path: r.text()?, attrs: r.attrs()? },
        FXP_FSETSTAT => Req::Fsetstat { id, handle: r.handle()?, attrs: r.attrs()? },
        FXP_OPENDIR => Req::Opendir { id, path: r.text()? },
        FXP_READDIR => Req::Readdir { id, handle: r.handle()? },
        FXP_REMOVE => Req::Remove { id, path: r.text()? },
        FXP_MKDIR => Req::Mkdir { id, path: r.text()?, attrs: r.attrs()? },
        FXP_RMDIR => Req::Rmdir { id, path: r.text()? },
        FXP_REALPATH => Req::Realpath { id, path: r.text()? },
        FXP_STAT => Req::Stat { id, path: r.text()? },
        FXP_RENAME => Req::Rename { id, from: r.text()?, to: r.text()? },
        FXP_READLINK => Req::Readlink { id, path: r.text()? },
        FXP_SYMLINK => Req::Symlink { id },
        FXP_EXTENDED => Req::Extended { id, name: r.text()? },
        other => return Err(ProtoError::UnknownType(other)),
    };
    Ok(req)
}

// ------------------------------------------------------------------ answers

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}
fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_be_bytes());
}
fn put_bytes(o: &mut Vec<u8>, b: &[u8]) {
    put_u32(o, b.len() as u32);
    o.extend_from_slice(b);
}

pub fn encode_attrs(o: &mut Vec<u8>, a: &Attrs) {
    let mut f = 0;
    if a.size.is_some() {
        f |= ATTR_SIZE;
    }
    if a.uid_gid.is_some() {
        f |= ATTR_UIDGID;
    }
    if a.perms.is_some() {
        f |= ATTR_PERMS;
    }
    if a.times.is_some() {
        f |= ATTR_TIMES;
    }
    put_u32(o, f);
    if let Some(s) = a.size {
        put_u64(o, s);
    }
    if let Some((u, g)) = a.uid_gid {
        put_u32(o, u);
        put_u32(o, g);
    }
    if let Some(p) = a.perms {
        put_u32(o, p);
    }
    if let Some((x, m)) = a.times {
        put_u32(o, x);
        put_u32(o, m);
    }
}

pub fn version_packet() -> Vec<u8> {
    let mut o = vec![FXP_VERSION];
    put_u32(&mut o, 3);
    o
}

pub fn status_packet(id: u32, code: u32, msg: &str) -> Vec<u8> {
    let mut o = vec![FXP_STATUS];
    put_u32(&mut o, id);
    put_u32(&mut o, code);
    put_bytes(&mut o, msg.as_bytes());
    put_bytes(&mut o, b"");
    o
}

pub fn handle_packet(id: u32, handle: &[u8]) -> Vec<u8> {
    let mut o = vec![FXP_HANDLE];
    put_u32(&mut o, id);
    put_bytes(&mut o, handle);
    o
}

pub fn data_packet(id: u32, data: &[u8]) -> Vec<u8> {
    let mut o = vec![FXP_DATA];
    put_u32(&mut o, id);
    put_bytes(&mut o, data);
    o
}

pub fn attrs_packet(id: u32, a: &Attrs) -> Vec<u8> {
    let mut o = vec![FXP_ATTRS];
    put_u32(&mut o, id);
    encode_attrs(&mut o, a);
    o
}

/// A NAME answer: each entry is a file name, its `ls -l` style long name and its attributes.
pub fn name_packet(id: u32, entries: &[(String, String, Attrs)]) -> Vec<u8> {
    let mut o = vec![FXP_NAME];
    put_u32(&mut o, id);
    put_u32(&mut o, entries.len() as u32);
    for (n, long, a) in entries {
        put_bytes(&mut o, n.as_bytes());
        put_bytes(&mut o, long.as_bytes());
        encode_attrs(&mut o, a);
    }
    o
}

/// Wrap a payload in its length prefix for the wire.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(payload.len() + 4);
    put_u32(&mut o, payload.len() as u32);
    o.extend_from_slice(payload);
    o
}

/// Cuts a byte stream into packets. Feed it whatever arrives; it hands back whole payloads and refuses a hostile length at once.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
    /// The next whole payload, `Ok(None)` when more bytes are needed. An error means the stream is not SFTP: close it.
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, ProtoError> {
        let Some(head) = self.buf.first_chunk::<4>() else { return Ok(None) };
        let n = frame_len(*head)?;
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let payload = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Ok(Some(payload))
    }
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}
