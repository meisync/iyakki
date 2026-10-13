//! Capability-based syscall handles.
//!
//! A guest can only touch a resource by presenting a [`SyscallToken`] that the
//! supervisor issued for it. Tokens are 128-bit random values, unguessable and
//! revocable. Everything else is denied by default.

pub mod policy;
pub mod wpa;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// What a token permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Read files at or below this absolute VFS path.
    FsRead(String),
    /// Write files at or below this absolute VFS path.
    FsWrite(String),
    /// Open outbound connections to this host.
    NetConnect(String),
}

/// The kind of access being requested; checked against a [`Capability`].
#[derive(Clone, Debug)]
pub enum Access<'a> {
    FsRead(&'a str),
    FsWrite(&'a str),
    NetConnect(&'a str),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecError {
    #[error("unknown or revoked token")]
    UnknownToken,
    #[error("capability does not permit this access")]
    Denied,
    #[error("entropy source unavailable")]
    Entropy,
}

/// Extract the lowercase host from an `http(s)://` URL. Rejects userinfo (`user@host`),
/// empty hosts and other schemes so a host allow-list cannot be spoofed.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        return None;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    if host.is_empty() { None } else { Some(host.to_ascii_lowercase()) }
}

/// Opaque, unforgeable handle (128 random bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyscallToken([u8; 16]);

impl SyscallToken {
    pub const LEN: usize = 16;

    pub fn from_bytes(b: [u8; 16]) -> Self {
        Self(b)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

// Never print token material in logs.
impl fmt::Debug for SyscallToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyscallToken(..)")
    }
}

fn path_within(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return path.starts_with('/');
    }
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl Capability {
    pub fn permits(&self, access: &Access<'_>) -> bool {
        match (self, access) {
            (Capability::FsRead(p), Access::FsRead(q)) => path_within(p, q),
            (Capability::FsWrite(p), Access::FsWrite(q)) => path_within(p, q),
            (Capability::NetConnect(h), Access::NetConnect(g)) => h == g,
            _ => false,
        }
    }
}

/// Issues, checks and revokes tokens. Cheap to clone (shared state).
#[derive(Clone, Default)]
pub struct CapabilityStore {
    inner: Arc<Mutex<HashMap<SyscallToken, Capability>>>,
}

impl CapabilityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue(&self, cap: Capability) -> Result<SyscallToken, SecError> {
        let mut b = [0u8; 16];
        random_fill(&mut b)?;
        let t = SyscallToken(b);
        self.inner.lock().unwrap().insert(t, cap);
        Ok(t)
    }

    pub fn check(&self, token: &SyscallToken, access: &Access<'_>) -> Result<(), SecError> {
        let g = self.inner.lock().unwrap();
        let cap = g.get(token).ok_or(SecError::UnknownToken)?;
        if cap.permits(access) {
            Ok(())
        } else {
            Err(SecError::Denied)
        }
    }

    pub fn revoke(&self, token: &SyscallToken) -> bool {
        self.inner.lock().unwrap().remove(token).is_some()
    }
}

/// Fill `buf` with cryptographically secure random bytes from the operating system
/// (`getrandom(2)`, falling back to `/dev/urandom` where the syscall is unavailable).
pub fn random_fill(buf: &mut [u8]) -> Result<(), SecError> {
    let mut off = 0;
    while off < buf.len() {
        // SAFETY: the pointer and length describe the unfilled tail of `buf`, which we own mutably.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let r = unsafe { libc::getrandom(buf[off..].as_mut_ptr().cast(), buf.len() - off, 0) };
        // other systems (macOS): the random device is read instead
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let r: isize = 0;
        if r > 0 {
            off += r as usize;
            continue;
        }
        let err = std::io::Error::last_os_error();
        if r < 0 && err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        // old kernel or a sandbox that blocks the syscall: read the device instead
        use std::io::Read;
        return std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf[off..])).map_err(|_| SecError::Entropy);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_read() {
        let s = CapabilityStore::new();
        let t = s.issue(Capability::FsRead("/work".into())).unwrap();
        assert!(s.check(&t, &Access::FsRead("/work/a.txt")).is_ok());
        assert!(s.check(&t, &Access::FsRead("/work")).is_ok());
        assert_eq!(s.check(&t, &Access::FsRead("/workshop")), Err(SecError::Denied));
        assert_eq!(s.check(&t, &Access::FsRead("/etc/passwd")), Err(SecError::Denied));
    }

    #[test]
    fn read_cap_does_not_allow_write() {
        let s = CapabilityStore::new();
        let t = s.issue(Capability::FsRead("/".into())).unwrap();
        assert_eq!(s.check(&t, &Access::FsWrite("/x")), Err(SecError::Denied));
    }

    #[test]
    fn forged_and_revoked_tokens_fail() {
        let s = CapabilityStore::new();
        let forged = SyscallToken::from_bytes([7; 16]);
        assert_eq!(s.check(&forged, &Access::FsRead("/")), Err(SecError::UnknownToken));
        let t = s.issue(Capability::NetConnect("api.example.com".into())).unwrap();
        assert!(s.check(&t, &Access::NetConnect("api.example.com")).is_ok());
        assert_eq!(s.check(&t, &Access::NetConnect("evil.com")), Err(SecError::Denied));
        assert!(s.revoke(&t));
        assert_eq!(s.check(&t, &Access::NetConnect("api.example.com")), Err(SecError::UnknownToken));
    }

    #[test]
    fn url_host_parsing() {
        assert_eq!(url_host("https://API.Example.com/v1?x=1").as_deref(), Some("api.example.com"));
        assert_eq!(url_host("http://a.com:8080/x").as_deref(), Some("a.com"));
        assert_eq!(url_host("https://[::1]:443/").as_deref(), Some("::1"));
        assert_eq!(url_host("https://good.com@evil.com/"), None);
        assert_eq!(url_host("ftp://a.com"), None);
        assert_eq!(url_host("https:///x"), None);
    }

    #[test]
    fn tokens_are_unique() {
        let s = CapabilityStore::new();
        let a = s.issue(Capability::FsRead("/".into())).unwrap();
        let b = s.issue(Capability::FsRead("/".into())).unwrap();
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod random_tests {
    use super::random_fill;

    #[test]
    fn random_bytes_fill_the_buffer_and_differ() {
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        random_fill(&mut a).unwrap();
        random_fill(&mut b).unwrap();
        assert_ne!(a, b);
        assert!(a.iter().any(|x| *x != 0));
        let mut big = vec![0u8; 100_000]; // larger than one getrandom call may return
        random_fill(&mut big).unwrap();
        assert!(big.iter().filter(|x| **x == 0).count() < 2000, "looks random");
        random_fill(&mut []).unwrap();
    }
}
