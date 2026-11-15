//! A pinned TLS connection as plain bytes, for programs that relay or fetch over it (`iyakki-connect window`): the certificate must be
//! the pinned one, checked before a byte of application data goes out (the same check as every other connection of this crate).

use std::io::{Read, Write};
use std::time::Duration;

use crate::cloudclient::{read_head, tls_open, ConnErr, Target, Tls};
use crate::rawlink::LinkError;

fn from_conn(e: ConnErr) -> LinkError {
    match e {
        ConnErr::Refused(m) => LinkError::Refused(m),
        ConnErr::Mismatch(m) => LinkError::PinChanged(m),
        other => LinkError::Other(other.text()),
    }
}

/// What the instance answered: status, header names in lower case, body (at most 1 MiB).
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// An open connection to the pinned instance.
pub struct Pinned {
    tls: Tls,
}

impl Pinned {
    /// Connect and finish the handshake; a certificate other than the pinned one is `LinkError::PinChanged`.
    pub fn open(t: &Target, timeout: Duration) -> Result<Pinned, LinkError> {
        let (tls, _) = tls_open(&t.addr, Some(t.pin), timeout).map_err(from_conn)?;
        Ok(Pinned { tls })
    }

    /// Read timeouts for polling loops (`None` blocks).
    pub fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) {
        let _ = self.tls.sock.set_read_timeout(read);
        let _ = self.tls.sock.set_write_timeout(write);
    }

    /// Status line and headers of the answer, and the bytes that came after them.
    pub fn read_head(&mut self) -> Result<(u16, Vec<(String, String)>, Vec<u8>), LinkError> {
        read_head(&mut self.tls).map_err(from_conn)
    }
}

impl Read for Pinned {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.tls.read(b)
    }
}

impl Write for Pinned {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.tls.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tls.flush()
    }
}

/// One GET on a fresh pinned connection (no cookie, nothing secret is sent).
pub fn get(t: &Target, path: &str, timeout: Duration) -> Result<Reply, LinkError> {
    let (mut tls, _) = tls_open(&t.addr, Some(t.pin), timeout).map_err(from_conn)?;
    let (status, headers, body) = crate::cloudclient::http(&mut tls, &t.addr, "GET", path, &[], b"").map_err(from_conn)?;
    Ok(Reply { status, headers, body })
}
