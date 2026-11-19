//! One place where the app-layer clients open their TCP connections, so that something above this crate (the VPN manager) can decide
//! per destination how the socket is made: with a mark that policy routing sends through the tunnel (F-VPN-08), or plain. This crate
//! knows nothing of VPNs: it holds an optional hook and asks it before every connection.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The answer of a hook: `None` means "connect the plain way"; `Some` is the finished result (a refusal is an error and the caller must
/// not try another way, so nothing a hook sent into a tunnel ever leaves outside it).
pub type Hook = Arc<dyn Fn(&str, SocketAddr, Duration) -> Option<io::Result<TcpStream>> + Send + Sync>;

static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

/// Install or (with `None`) remove the hook.
pub fn set_hook(h: Option<Hook>) {
    *HOOK.lock().unwrap_or_else(PoisonError::into_inner) = h;
}

pub fn hook_installed() -> bool {
    HOOK.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}

/// Connect to `addr`, which is one of the addresses of `host`.
pub fn connect(host: &str, addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    let hook = HOOK.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(h) = hook {
        if let Some(r) = h(host, addr, timeout) {
            return r;
        }
    }
    TcpStream::connect_timeout(&addr, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The hook is global: one test at a time touches it.
    static ONE: Mutex<()> = Mutex::new(());

    #[test]
    fn without_a_hook_it_connects_plainly_and_a_hook_can_take_over_or_refuse() {
        let _g = ONE.lock().unwrap_or_else(PoisonError::into_inner);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        set_hook(None);
        assert!(connect("example.test", addr, Duration::from_secs(5)).is_ok());
        set_hook(Some(Arc::new(|host, _, _| (host == "corp.example.test").then(|| Err(io::Error::other("meant for the tunnel"))))));
        assert!(hook_installed());
        assert!(connect("corp.example.test", addr, Duration::from_secs(5)).is_err(), "refused, never plain");
        assert!(connect("other.example.test", addr, Duration::from_secs(5)).is_ok(), "None means plain");
        set_hook(None);
    }
}
