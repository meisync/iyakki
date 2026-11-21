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

/// The same hook for the ureq-based clients (F-VPN-08): a connector that asks [`connect`]'s hook for the TCP socket of each resolved
/// address; when the hook has no opinion, ureq's own TCP connector runs as before. TLS goes on top of whichever socket results.
#[cfg(feature = "https")]
pub mod ureq_hook {
    use super::{Hook, HOOK};
    use std::sync::PoisonError;
    use ureq::unversioned::transport::{
        Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, RustlsConnector, TcpConnector, Transport,
    };
    use ureq::Error;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    /// A hook-made socket as a ureq transport (ureq's own `TcpTransport` is not public).
    #[derive(Debug)]
    pub struct HookedTransport {
        stream: TcpStream,
        buffers: LazyBuffers,
    }

    impl Transport for HookedTransport {
        fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
            &mut self.buffers
        }
        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
            self.stream.set_write_timeout(timeout.not_zero().map(|t| *t))?;
            let out = &self.buffers.output()[..amount];
            match self.stream.write_all(out) {
                Ok(()) => Ok(()),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => Err(Error::Timeout(timeout.reason)),
                Err(e) => Err(e.into()),
            }
        }
        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
            self.stream.set_read_timeout(timeout.not_zero().map(|t| *t))?;
            let buf = self.buffers.input_append_buf();
            let n = match self.stream.read(buf) {
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => return Err(Error::Timeout(timeout.reason)),
                Err(e) => return Err(e.into()),
            };
            self.buffers.input_appended(n);
            Ok(n > 0)
        }
        fn is_open(&mut self) -> bool {
            if self.stream.set_nonblocking(true).is_err() {
                return false;
            }
            let mut b = [0u8];
            let idle = matches!(self.stream.read(&mut b), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock);
            idle && self.stream.set_nonblocking(false).is_ok()
        }
    }

    #[derive(Debug, Default)]
    pub struct HookTcp;

    fn current() -> Option<Hook> {
        HOOK.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    impl<In: Transport> Connector<In> for HookTcp {
        type Out = Either<In, HookedTransport>;

        fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, Error> {
            if chained.is_some() {
                return Ok(chained.map(Either::A));
            }
            let Some(hook) = current() else { return Ok(None) };
            let host = details.uri.host().unwrap_or("").to_string();
            let mut last: Option<std::io::Error> = None;
            for addr in details.addrs.iter() {
                let t = details.timeout.not_zero().map(|t| std::time::Duration::from_millis((t.as_millis() as u64).max(10))).unwrap_or(std::time::Duration::from_secs(30));
                match hook(&host, *addr, t) {
                    // The hook has no opinion about this destination: the plain connector takes over.
                    None => return Ok(None),
                    Some(Ok(stream)) => {
                        let _ = stream.set_nodelay(details.config.no_delay());
                        let buffers = LazyBuffers::new(details.config.input_buffer_size(), details.config.output_buffer_size());
                        return Ok(Some(Either::B(HookedTransport { stream, buffers })));
                    }
                    // A refusal ends the request: the caller must not try another way.
                    Some(Err(e)) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut) => last = Some(e),
                    Some(Err(e)) => return Err(Error::Io(e)),
                }
            }
            Err(Error::Io(last.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::ConnectionRefused))))
        }
    }

    /// The connector chain for a client without an upstream proxy: the hook, ureq's TCP, then rustls.
    pub fn connector() -> impl Connector<(), Out = impl Transport> {
        ().chain(HookTcp).chain(TcpConnector::default()).chain(RustlsConnector::default())
    }
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

    #[cfg(feature = "https")]
    #[test]
    fn the_ureq_clients_open_their_sockets_through_the_hook_and_a_refusal_is_final() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _g = ONE.lock().unwrap_or_else(PoisonError::into_inner);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            for _ in 0..1 {
                let (mut c, _) = l.accept().unwrap();
                let mut b = [0u8; 1024];
                let _ = c.read(&mut b).unwrap();
                c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            }
        });
        let seen = Arc::new(AtomicUsize::new(0));
        let s2 = seen.clone();
        set_hook(Some(Arc::new(move |host, addr, t| {
            // The hook is process-wide, so every test of the crate that connects while it is installed passes through it: it may only
            // have an opinion about THIS test's own server (its port is held by `l` above); every other connection is made plainly.
            if host == "localhost" && addr.port() == port {
                return Some(Err(io::Error::other("meant for the tunnel")));
            }
            if addr.port() == port {
                s2.fetch_add(1, Ordering::SeqCst);
            }
            Some(TcpStream::connect_timeout(&addr, t))
        })));
        let cfg = ureq::Agent::config_builder().http_status_as_error(false).build();
        let agent = ureq::Agent::with_parts(cfg, ureq_hook::connector(), ureq::unversioned::resolver::DefaultResolver::default());
        let mut r = agent.get(format!("http://127.0.0.1:{port}/")).call().expect("the hooked socket carries the request");
        assert_eq!(r.body_mut().read_to_string().unwrap(), "ok");
        assert_eq!(seen.load(Ordering::SeqCst), 1, "the hook made the socket");
        assert!(agent.get(format!("http://localhost:{port}/")).call().is_err(), "a refusal never falls back to a plain connection");
        set_hook(None);
        srv.join().unwrap();
    }
}
