//! The sign-in relay page (`/login/<token>`, see `iyakki_agent::extagent::relay`): the answer to one request, and a small HTTPS
//! listener started only for the length of a login when the web front end is not running (when it is, its own listener answers).
//! Only local-network addresses are answered; the page is not forwarded anywhere.

use crate::http::{Parse, Request, Response};
use iyakki_agent::extagent::relay;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The answer for `req` when it is a relay request (`None`: not ours).
pub fn respond(req: &Request, peer: IpAddr) -> Option<Response> {
    if !req.path.starts_with("/login/") {
        return None;
    }
    // loopback and public addresses get the same answer as a wrong token
    let reply = if relay::lan_peer(peer) { relay::handle(&req.method, &req.path, req.header("content-type"), &req.body, &peer.to_string()) } else { relay::handle("GET", "/login/-", None, b"", "refused") };
    reply.map(|(c, ct, b)| Response::new(c, ct, b).header("Cache-Control", "no-store").header("Referrer-Policy", "no-referrer"))
}

struct Running {
    stop: Arc<AtomicBool>,
    port: u16,
}

/// Starts the relay's own HTTPS listener on demand and stops it with the login.
pub struct OwnListener {
    tls: Arc<rustls::ServerConfig>,
    running: Mutex<Option<Running>>,
}

impl OwnListener {
    pub fn new(tls: Arc<rustls::ServerConfig>) -> OwnListener {
        OwnListener { tls, running: Mutex::new(None) }
    }
}

impl relay::Listener for OwnListener {
    fn start(&self) -> Option<(u16, bool)> {
        let mut g = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.as_ref() {
            return Some((r.port, true));
        }
        let l = TcpListener::bind("0.0.0.0:0").ok()?;
        l.set_nonblocking(true).ok()?;
        let port = l.local_addr().ok()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (s, tls) = (stop.clone(), self.tls.clone());
        std::thread::Builder::new().name("iyakki-login-relay".into()).spawn(move || accept(l, s, tls)).ok()?;
        *g = Some(Running { stop, port });
        Some((port, true))
    }

    fn stop(&self) {
        if let Some(r) = self.running.lock().unwrap_or_else(|e| e.into_inner()).take() {
            r.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// The listener the device installs at start: the certificate (the web front end's, in `<data>/tls`, made when missing) is only loaded
/// when a login first needs the relay.
pub struct LazyListener {
    dir: std::path::PathBuf,
    inner: Mutex<Option<Arc<OwnListener>>>,
}

impl LazyListener {
    pub fn new(tls_dir: &std::path::Path) -> LazyListener {
        LazyListener { dir: tls_dir.to_path_buf(), inner: Mutex::new(None) }
    }
}

impl relay::Listener for LazyListener {
    fn start(&self) -> Option<(u16, bool)> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
            let names: Vec<String> = [host].into_iter().filter(|h| !h.is_empty()).collect();
            match crate::tls::identity(&self.dir, &names) {
                Ok(id) => *g = Some(Arc::new(OwnListener::new(id.config))),
                Err(e) => {
                    log::warn!("login relay: no certificate ({e})");
                    return None;
                }
            }
        }
        g.as_ref().and_then(|l| l.start())
    }

    fn stop(&self) {
        if let Some(l) = self.inner.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            l.stop();
        }
    }
}

fn accept(l: TcpListener, stop: Arc<AtomicBool>, tls: Arc<rustls::ServerConfig>) {
    let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match l.accept() {
            Ok((s, peer)) => {
                if open.load(Ordering::Relaxed) >= 8 {
                    continue;
                }
                open.fetch_add(1, Ordering::Relaxed);
                let (tls, open) = (tls.clone(), open.clone());
                let _ = std::thread::Builder::new().name("iyakki-login-relay-conn".into()).spawn(move || {
                    connection(s, peer, tls);
                    open.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn connection(stream: TcpStream, peer: SocketAddr, tls: Arc<rustls::ServerConfig>) {
    if !relay::lan_peer(peer.ip()) {
        return; // not even a TLS handshake for a stranger
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(c) = rustls::ServerConnection::new(tls) else { return };
    let mut conn = rustls::StreamOwned::new(c, stream);
    let (mut buf, mut chunk, t0) = (Vec::new(), [0u8; 4096], Instant::now());
    let resp = loop {
        match crate::http::parse_limited(&buf, relay::MAX_BODY + 4096) {
            Parse::Done(r) => break respond(&r, peer.ip()).unwrap_or_else(|| Response::error(404, "not found")),
            Parse::Bad(code, msg) => break Response::error(code, msg),
            Parse::Incomplete => {}
        }
        if t0.elapsed() > Duration::from_secs(15) {
            return;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let _ = conn.write_all(&resp.to_bytes());
    let _ = conn.flush();
    conn.conn.send_close_notify();
    let _ = conn.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn identity() -> crate::tls::Identity {
        let d = std::env::temp_dir().join(format!("iyakki-relay-tls-{}", std::process::id()));
        let id = crate::tls::identity(&d, &["localhost".into()]).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        id
    }

    fn get(port: u16, path: &str) -> String {
        // a client that trusts anything: the certificate is the device's own
        #[derive(Debug)]
        struct Any;
        impl rustls::client::danger::ServerCertVerifier for Any {
            fn verify_server_cert(&self, _: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>], _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
            }
        }
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().dangerous().with_custom_certificate_verifier(Arc::new(Any)).with_no_client_auth();
        let c = rustls::ClientConnection::new(Arc::new(cfg), "localhost".try_into().unwrap()).unwrap();
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut t = rustls::StreamOwned::new(c, s);
        let _ = t.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes());
        let mut out = Vec::new();
        let _ = t.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn requests_from_loopback_and_strangers_get_the_wrong_token_answer_and_lan_requests_are_served() {
        use iyakki_agent::extagent::relay::Listener;
        let token = relay::open("Claude Code");
        let req = |m: &str, p: &str| Request { method: m.into(), path: p.into(), query: Default::default(), headers: Default::default(), body: vec![] };
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        let r = respond(&req("GET", &format!("/login/{token}")), lan).unwrap();
        assert_eq!(r.status, 200);
        assert!(r.headers.iter().any(|(k, v)| k == "Cache-Control" && v == "no-store"));
        for ip in ["127.0.0.1", "8.8.8.8"] {
            assert_eq!(respond(&req("GET", &format!("/login/{token}")), ip.parse().unwrap()).unwrap().status, 404, "{ip}");
        }
        assert!(respond(&req("GET", "/healthz"), lan).is_none());
        // the real listener: loopback is refused before the handshake finishes, and stop closes it
        let l = OwnListener::new(identity().config);
        let (port, https) = l.start().unwrap();
        assert!(https && l.start().unwrap().0 == port, "started once");
        assert!(!get(port, &format!("/login/{token}")).contains("<textarea"), "a loopback client never gets the page");
        l.stop();
        relay::finish();
    }
}
