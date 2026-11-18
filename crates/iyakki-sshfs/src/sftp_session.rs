//! One SFTP session: requests in (payloads from `sftp_proto`), answers out, files through the person's [`Jail`]. No sockets here: the
//! SSH channel feeds bytes to [`Session::feed`] and sends back what it returns, so the whole conversation is testable in memory.
//!
//! Limits: at most [`MAX_HANDLES`] open handles, a folder listing is handed out in pages, a READ returns at most 64 KiB, a bad stream
//! (oversize length, unparseable packet, a request before INIT) ends the session. Every change to files writes one audit line with the
//! virtual path (control characters removed); contents, passwords and keys never appear.

use std::collections::HashMap;
use std::fs::File;

use crate::sftp_fs::{self, FsErr, Jail};
use crate::sftp_proto::*;

pub const MAX_HANDLES: usize = 64;
const PAGE: usize = 100;

enum Handle {
    File { file: File, writable: bool },
    Dir { entries: Vec<(String, String, Attrs)>, pos: usize },
}

pub struct Session {
    jail: Jail,
    handles: HashMap<u32, Handle>,
    next: u32,
    started: bool,
    framer: Framer,
    audit: Box<dyn Fn(&str, &str) + Send>,
    pub closed: bool,
}

/// Path text safe for a log line.
pub fn clean(s: &str) -> String {
    s.chars().take(160).map(|c| if c.is_control() { '?' } else { c }).collect()
}

impl Session {
    pub fn new(jail: Jail, audit: Box<dyn Fn(&str, &str) + Send>) -> Session {
        Session { jail, handles: HashMap::new(), next: 1, started: false, framer: Framer::default(), audit, closed: false }
    }

    /// Bytes from the client; the framed answers to send back. After a hostile stream `closed` is set and nothing more is answered.
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if self.closed {
            return out;
        }
        self.framer.push(data);
        loop {
            match self.framer.next() {
                Ok(Some(p)) => {
                    for r in self.handle(&p) {
                        out.extend_from_slice(&frame(&r));
                    }
                    if self.closed {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    (self.audit)("sftp.bad_stream", &e.to_string());
                    self.closed = true;
                    break;
                }
            }
        }
        out
    }

    fn status(id: u32, code: u32, msg: &str) -> Vec<Vec<u8>> {
        vec![status_packet(id, code, msg)]
    }

    fn fail(id: u32, e: FsErr) -> Vec<Vec<u8>> {
        Self::status(id, e.code(), &e.text())
    }

    fn put(&mut self, h: Handle) -> Result<Vec<u8>, FsErr> {
        if self.handles.len() >= MAX_HANDLES {
            return Err(FsErr::Failure("Too many open files".into()));
        }
        let n = self.next;
        self.next = self.next.wrapping_add(1);
        self.handles.insert(n, h);
        Ok(n.to_be_bytes().to_vec())
    }

    fn get(&mut self, h: &[u8]) -> Option<&mut Handle> {
        let n = u32::from_be_bytes(h.try_into().ok()?);
        self.handles.get_mut(&n)
    }

    /// Answer one request payload.
    pub fn handle(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        let req = match parse_request(payload) {
            Ok(r) => r,
            Err(ProtoError::UnknownType(_)) => {
                // answer with the id when the packet has one, so the client does not wait
                let id = payload.get(1..5).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
                return match id {
                    Some(id) => Self::status(id, FX_OP_UNSUPPORTED, "Unsupported request"),
                    None => {
                        self.closed = true;
                        Vec::new()
                    }
                };
            }
            Err(e) => {
                (self.audit)("sftp.bad_packet", &e.to_string());
                self.closed = true;
                return Vec::new();
            }
        };
        if !self.started {
            return match req {
                Req::Init { .. } => {
                    self.started = true;
                    vec![version_packet()]
                }
                _ => {
                    self.closed = true;
                    Vec::new()
                }
            };
        }
        match req {
            Req::Init { .. } => {
                self.closed = true;
                Vec::new()
            }
            Req::Open { id, path, flags, attrs } => match self.jail.open(&path, flags, &attrs) {
                Ok((file, virt)) => {
                    let writable = flags & (OPEN_WRITE | OPEN_APPEND) != 0;
                    match self.put(Handle::File { file, writable }) {
                        Ok(h) => {
                            let what = if flags & OPEN_CREAT != 0 { "create" } else if writable { "write" } else { "read" };
                            (self.audit)("sftp.open", &format!("{what} {}", clean(&virt)));
                            vec![handle_packet(id, &h)]
                        }
                        Err(e) => Self::fail(id, e),
                    }
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Close { id, handle } => {
                let ok = <[u8; 4]>::try_from(handle.as_slice()).ok().and_then(|b| self.handles.remove(&u32::from_be_bytes(b))).is_some();
                if ok { Self::status(id, FX_OK, "") } else { Self::status(id, FX_FAILURE, "Bad handle") }
            }
            Req::Read { id, handle, offset, len } => match self.get(&handle) {
                Some(Handle::File { file, .. }) => match sftp_fs::read_at(file, offset, len) {
                    Ok(d) if d.is_empty() && len > 0 => Self::status(id, FX_EOF, "End of file"),
                    Ok(d) => vec![data_packet(id, &d)],
                    Err(e) => Self::fail(id, e),
                },
                _ => Self::status(id, FX_FAILURE, "Bad handle"),
            },
            Req::Write { id, handle, offset, data } => match self.get(&handle) {
                Some(Handle::File { file, writable: true }) => match sftp_fs::write_at(file, offset, &data) {
                    Ok(()) => Self::status(id, FX_OK, ""),
                    Err(e) => Self::fail(id, e),
                },
                Some(Handle::File { .. }) => Self::status(id, FX_PERMISSION_DENIED, "Opened for reading only"),
                _ => Self::status(id, FX_FAILURE, "Bad handle"),
            },
            Req::Lstat { id, path } => self.stat(id, &path, false),
            Req::Stat { id, path } => self.stat(id, &path, true),
            Req::Fstat { id, handle } => match self.get(&handle) {
                Some(Handle::File { file, .. }) => match file.metadata() {
                    Ok(m) => vec![attrs_packet(id, &Jail::attrs_of(&m))],
                    Err(_) => Self::status(id, FX_FAILURE, "Operation failed"),
                },
                _ => Self::status(id, FX_FAILURE, "Bad handle"),
            },
            Req::Setstat { id, path, attrs } => match self.jail.setstat(&path, &attrs) {
                Ok(v) => {
                    (self.audit)("sftp.setstat", &clean(&v));
                    Self::status(id, FX_OK, "")
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Fsetstat { id, handle, attrs } => match self.get(&handle) {
                Some(Handle::File { file, writable }) => match sftp_fs::fset(file, *writable, &attrs) {
                    Ok(()) => Self::status(id, FX_OK, ""),
                    Err(e) => Self::fail(id, e),
                },
                _ => Self::status(id, FX_FAILURE, "Bad handle"),
            },
            Req::Opendir { id, path } => match self.jail.list(&path) {
                Ok(entries) => match self.put(Handle::Dir { entries, pos: 0 }) {
                    Ok(h) => vec![handle_packet(id, &h)],
                    Err(e) => Self::fail(id, e),
                },
                Err(e) => Self::fail(id, e),
            },
            Req::Readdir { id, handle } => match self.get(&handle) {
                Some(Handle::Dir { entries, pos }) => {
                    if *pos >= entries.len() {
                        return Self::status(id, FX_EOF, "End of folder");
                    }
                    let end = (*pos + PAGE).min(entries.len());
                    let page = &entries[*pos..end];
                    *pos = end;
                    vec![name_packet(id, page)]
                }
                _ => Self::status(id, FX_FAILURE, "Bad handle"),
            },
            Req::Remove { id, path } => match self.jail.remove(&path) {
                Ok(v) => {
                    (self.audit)("sftp.remove", &clean(&v));
                    Self::status(id, FX_OK, "")
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Mkdir { id, path, attrs } => match self.jail.mkdir(&path, &attrs) {
                Ok(v) => {
                    (self.audit)("sftp.mkdir", &clean(&v));
                    Self::status(id, FX_OK, "")
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Rmdir { id, path } => match self.jail.rmdir(&path) {
                Ok(v) => {
                    (self.audit)("sftp.rmdir", &clean(&v));
                    Self::status(id, FX_OK, "")
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Rename { id, from, to } => match self.jail.rename(&from, &to) {
                Ok((a, b)) => {
                    (self.audit)("sftp.rename", &format!("{} -> {}", clean(&a), clean(&b)));
                    Self::status(id, FX_OK, "")
                }
                Err(e) => Self::fail(id, e),
            },
            Req::Realpath { id, path } => match self.jail.realpath(if path.is_empty() { "/" } else { &path }) {
                Ok(v) => vec![name_packet(id, &[(v.clone(), v, Attrs::default())])],
                Err(e) => Self::fail(id, e),
            },
            // links are not offered: a link is the easiest way to point outside, and the scope rules decide what exists
            Req::Readlink { id, .. } | Req::Symlink { id } => Self::status(id, FX_OP_UNSUPPORTED, "Links are not supported"),
            Req::Extended { id, .. } => Self::status(id, FX_OP_UNSUPPORTED, "Unsupported request"),
        }
    }

    fn stat(&mut self, id: u32, path: &str, follow: bool) -> Vec<Vec<u8>> {
        match self.jail.stat(path, follow) {
            Ok(a) => vec![attrs_packet(id, &a)],
            Err(e) => Self::fail(id, e),
        }
    }
}
