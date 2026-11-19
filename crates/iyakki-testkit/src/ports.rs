//! Reserved ports. Two tests never get the same number (not even from two test processes of this user), and a "nothing listens" port is
//! one that no other [`port`] or [`closed_port`] call hands out. Numbers come from 20000..28000, below the ephemeral range of Linux and
//! macOS, so the kernel's own choices for outgoing connections never meet them. Every number is claimed with an `flock` on a file in
//! `/tmp/iyakki-test-ports-<uid>/` that this process keeps until it ends.

use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

static HELD: Mutex<Vec<std::fs::File>> = Mutex::new(Vec::new());
static NEXT: AtomicU32 = AtomicU32::new(0);

fn claim(port: u16) -> bool {
    // SAFETY: getuid has no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-test-ports-{uid}"));
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join(port.to_string())) else { return true };
    // SAFETY: the descriptor is open and owned by `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return false;
    }
    HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(file);
    true
}

/// A reserved port: the number is claimed and the sockets are held, so nothing else can take it until [`Port::release`] or
/// [`Port::into_listener`].
pub struct Port {
    number: u16,
    tcp: Option<TcpListener>,
    _udp: Option<UdpSocket>,
}

impl Port {
    /// The port number.
    pub fn number(&self) -> u16 {
        self.number
    }

    /// Gives the number to a server that binds it itself: the held sockets are closed first (the claim stays for this process, so no
    /// other testkit call hands the number out again).
    pub fn release(mut self) -> u16 {
        self.tcp = None;
        self.number
    }

    /// The already bound listener (no gap between reservation and use).
    pub fn into_listener(mut self) -> TcpListener {
        self.tcp.take().expect("listener present")
    }
}

/// Reserves a free TCP and UDP port on all addresses.
pub fn port() -> Port {
    for _ in 0..16_000 {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let port = (20_000 + ((std::process::id() % 8_000) * 37 + n) % 8_000) as u16;
        if !claim(port) {
            continue;
        }
        let (Ok(tcp), Ok(udp)) = (TcpListener::bind(("0.0.0.0", port)), UdpSocket::bind(("0.0.0.0", port))) else { continue };
        return Port { number: port, tcp: Some(tcp), _udp: Some(udp) };
    }
    panic!("testkit: no free port in 20000..28000");
}

/// A port on which nothing listens and that no other testkit call will hand out for the rest of this process (a connection to it is
/// refused). For tests of "the server is not there".
pub fn closed_port() -> u16 {
    for _ in 0..16_000 {
        let p = port();
        let n = p.release();
        // the bind succeeded a moment ago, so nothing listened; make sure a connect is refused now
        if TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], n)), std::time::Duration::from_millis(500)).is_err() {
            return n;
        }
    }
    panic!("testkit: no closed port found");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_unique_closed_ports_refuse_and_a_released_number_binds() {
        let a = port();
        let b = port();
        assert_ne!(a.number(), b.number());
        let c = closed_port();
        assert!(TcpStream::connect(("127.0.0.1", c)).is_err());
        let n = a.release();
        let l = TcpListener::bind(("127.0.0.1", n)).unwrap();
        assert_eq!(l.local_addr().unwrap().port(), n);
        assert_ne!(b.number(), c);
    }
}
