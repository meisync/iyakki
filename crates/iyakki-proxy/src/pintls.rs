//! HTTPS GET with the certificate check changed for ONE connection, used only where a person approved it or where a missing
//! issuer could be fetched and verified:
//!
//! * [`TlsMode::Pin`]: the server must present exactly the leaf certificate with this SHA-256 fingerprint; its name must
//!   match the host asked for (subject alternative names), and it must be within its validity period. The chain is not
//!   checked, which is what lets a server that forgot to send its intermediate certificate work. A different certificate is
//!   refused ([`CHANGED`]) and never replaces the pin.
//! * [`TlsMode::ExtraChain`]: the normal check against the built-in roots, with the given intermediate certificates added
//!   to the ones the server sent. Nothing is weakened: a wrong certificate still fails.
//!
//! Also [`peer_chain`], which only completes a handshake to read what the server shows (nothing is sent or trusted).

use super::{StreamHead, TlsMode};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CHANGED: &str = "the certificate changed since you trusted it";
pub const EXPIRED: &str = "the certificate has expired";
pub const NOT_YET: &str = "the certificate is not valid yet";
pub const WRONG_NAME: &str = "the certificate is not for this host name";

/// Does a connection error say the issuer of the certificate is unknown (the chain is incomplete or the CA is not trusted)?
pub fn is_unknown_issuer(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    e.contains("unknownissuer") || e.contains("unknown issuer") || e.contains("unable to get local issuer") || e.contains("unknown ca")
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots() -> rustls::RootCertStore {
    rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() }
}

#[derive(Debug)]
struct Recorder {
    chain: Mutex<Vec<Vec<u8>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

#[derive(Debug)]
struct PinVerifier {
    fp: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
    why: Arc<Mutex<Option<String>>>,
}

#[derive(Debug)]
struct ExtraVerifier {
    inner: Arc<WebPkiServerVerifier>,
    extra: Vec<CertificateDer<'static>>,
}

macro_rules! sig_methods {
    ($provider:expr) => {
        fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &$provider(self).signature_verification_algorithms)
        }
        fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &$provider(self).signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            $provider(self).signature_verification_algorithms.supported_schemes()
        }
    };
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, rest: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let mut v = vec![end.as_ref().to_vec()];
        v.extend(rest.iter().map(|c| c.as_ref().to_vec()));
        *self.chain.lock().unwrap() = v;
        Ok(ServerCertVerified::assertion())
    }
    sig_methods!(|s: &Self| s.provider.clone());
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, _: &[CertificateDer<'_>], name: &ServerName<'_>, _: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let fail = |m: String| {
            *self.why.lock().unwrap() = Some(m.clone());
            Err(rustls::Error::General(m))
        };
        let seen = crate::certinfo::fingerprint(end.as_ref());
        if crate::certinfo::norm_fingerprint(&seen) != self.fp {
            return fail(format!("{CHANGED} (it now shows {seen})"));
        }
        let parsed = rustls::server::ParsedCertificate::try_from(end)?;
        if rustls::client::verify_server_name(&parsed, name).is_err() {
            return fail(WRONG_NAME.to_string());
        }
        let info = crate::certinfo::parse(end.as_ref()).map_err(rustls::Error::General)?;
        let t = now.as_secs() as i64;
        if t > info.not_after {
            return fail(format!("{EXPIRED} (it ended on {})", crate::certinfo::date_text(info.not_after)));
        }
        if t < info.not_before {
            return fail(NOT_YET.to_string());
        }
        Ok(ServerCertVerified::assertion())
    }
    sig_methods!(|s: &Self| s.provider.clone());
}

impl ServerCertVerifier for ExtraVerifier {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, rest: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let mut all: Vec<CertificateDer<'_>> = rest.to_vec();
        all.extend(self.extra.iter().cloned());
        self.inner.verify_server_cert(end, &all, name, ocsp, now)
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(m, c, d)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// `https://host[:port]/path?query` -> (host, port, request target). Only https.
pub fn split_https(url: &str) -> Result<(String, u16, String), String> {
    let rest = url.strip_prefix("https://").ok_or("a pinned connection needs an https:// address")?;
    let (authority, target) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    let target = target.split('#').next().unwrap_or("/").to_string();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, p.parse::<u16>().map_err(|_| "bad port")?),
        _ => (authority, 443),
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host.is_empty() {
        return Err("no host in the address".into());
    }
    Ok((host, port, target))
}

fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.collect();
    let mut last = format!("{host}: no address");
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(15)));
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = format!("{host}:{port}: {e}"),
        }
    }
    Err(last)
}

fn config(v: Arc<dyn ServerCertVerifier>) -> Result<Arc<rustls::ClientConfig>, String> {
    let cfg = rustls::ClientConfig::builder_with_provider(provider()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.dangerous().with_custom_certificate_verifier(v).with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Is the certificate (DER) valid for this host name (subject alternative names)?
pub fn names_match(der: &[u8], host: &str) -> bool {
    let (Ok(name), cert) = (ServerName::try_from(host.to_string()), CertificateDer::from(der.to_vec())) else { return false };
    match rustls::server::ParsedCertificate::try_from(&cert) {
        Ok(p) => rustls::client::verify_server_name(&p, &name).is_ok(),
        Err(_) => false,
    }
}

/// What the server at `host:port` shows (leaf first, then what it sent after it). The certificates are not trusted by this call
/// and nothing but the handshake is sent.
pub fn peer_chain(host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
    let rec = Arc::new(Recorder { chain: Mutex::new(Vec::new()), provider: provider() });
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let mut conn = rustls::ClientConnection::new(config(rec.clone())?, name).map_err(|e| e.to_string())?;
    let mut sock = connect(host, port)?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| format!("{host}:{port}: {e}"))?;
        if !rec.chain.lock().unwrap().is_empty() && !conn.is_handshaking() {
            break;
        }
    }
    let chain = rec.chain.lock().unwrap().clone();
    if chain.is_empty() {
        return Err(format!("{host}:{port} showed no certificate"));
    }
    Ok(chain)
}

struct Chunked<R> {
    inner: R,
    left: usize,
    done: bool,
}

impl<R: Read> Chunked<R> {
    fn line(&mut self) -> std::io::Result<String> {
        let mut s = Vec::new();
        let mut b = [0u8; 1];
        loop {
            if self.inner.read(&mut b)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            if b[0] == b'\n' {
                return Ok(String::from_utf8_lossy(&s).trim().to_string());
            }
            s.push(b[0]);
            if s.len() > 200 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk header"));
            }
        }
    }
}

impl<R: Read> Read for Chunked<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.done || out.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let l = self.line()?;
            let l = if l.is_empty() { self.line()? } else { l };
            let n = usize::from_str_radix(l.split(';').next().unwrap_or("").trim(), 16).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk size"))?;
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.left = n;
        }
        let want = out.len().min(self.left);
        let n = self.inner.read(&mut out[..want])?;
        self.left -= n;
        Ok(n)
    }
}

/// GET `url` over TLS checked the way `mode` says. Does not follow redirects (the caller does, host by host).
pub fn open_tls(url: &str, headers: &[(String, String)], mode: &TlsMode) -> Result<StreamHead, String> {
    open_with_roots(url, headers, mode, roots())
}

fn open_with_roots(url: &str, headers: &[(String, String)], mode: &TlsMode, trusted: rustls::RootCertStore) -> Result<StreamHead, String> {
    let (host, port, target) = split_https(url)?;
    let why = Arc::new(Mutex::new(None));
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
        TlsMode::Pin(fp) => Arc::new(PinVerifier { fp: crate::certinfo::norm_fingerprint(fp), provider: provider(), why: why.clone() }),
        TlsMode::ExtraChain(extra) => {
            let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(trusted), provider()).build().map_err(|e| e.to_string())?;
            Arc::new(ExtraVerifier { inner, extra: extra.iter().map(|d| CertificateDer::from(d.clone())).collect() })
        }
    };
    let name = ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(config(verifier)?, name).map_err(|e| e.to_string())?;
    let sock = connect(&host, port)?;
    let mut tls = rustls::StreamOwned::new(conn, sock);
    let host_header = if port == 443 { host.clone() } else { format!("{host}:{port}") };
    let mut req = format!("GET {target} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: Mozilla/5.0 (X11; Linux x86_64) Iyakki/0.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !v.contains(['\r', '\n']) {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    req.push_str("\r\n");
    let explain = |e: std::io::Error| why.lock().unwrap().clone().unwrap_or_else(|| e.to_string());
    tls.write_all(req.as_bytes()).and_then(|_| tls.flush()).map_err(explain)?;
    // the answer's head, a byte at a time so that nothing of the body is taken
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match tls.read(&mut b) {
            Ok(0) => return Err(why.lock().unwrap().clone().unwrap_or_else(|| "the server closed the connection".into())),
            Ok(_) => head.push(b[0]),
            Err(e) => return Err(explain(e)),
        }
        if head.len() > 32 * 1024 {
            return Err("the server's answer has too long a header".into());
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let status: u16 = lines.next().unwrap_or("").split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("the server did not answer with HTTP")?;
    let hs: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let chunked = hs.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let body: Box<dyn Read + Send> = if chunked { Box::new(Chunked { inner: tls, left: 0, done: false }) } else { Box::new(tls) };
    Ok(StreamHead { status, headers: hs, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TlsPlan;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use std::net::TcpListener;

    struct Made {
        der: Vec<u8>,
        cert: rcgen::Certificate,
        key: KeyPair,
    }

    fn ca(name: &str) -> Made {
        let mut p = CertificateParams::new(vec![]).unwrap();
        p.distinguished_name.push(DnType::CommonName, name);
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let key = KeyPair::generate().unwrap();
        let cert = p.self_signed(&key).unwrap();
        Made { der: cert.der().to_vec(), cert, key }
    }

    fn leaf(names: &[&str], by: &Made, years: Option<(i32, i32)>, aia: Option<&str>) -> Made {
        let mut p = CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        p.distinguished_name.push(DnType::CommonName, names[0]);
        if let Some((a, b)) = years {
            p.not_before = rcgen::date_time_ymd(a, 1, 1);
            p.not_after = rcgen::date_time_ymd(b, 1, 1);
        }
        if let Some(u) = aia {
            let uri = u.as_bytes();
            let mut ad = vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x02, 0x86, uri.len() as u8];
            ad.extend_from_slice(uri);
            let mut seq = vec![0x30, ad.len() as u8];
            seq.extend(ad);
            let mut outer = vec![0x30, seq.len() as u8];
            outer.extend(seq);
            p.custom_extensions.push(rcgen::CustomExtension::from_oid_content(&[1, 3, 6, 1, 5, 5, 7, 1, 1], outer));
        }
        let key = KeyPair::generate().unwrap();
        let cert = p.signed_by(&key, &by.cert, &by.key).unwrap();
        Made { der: cert.der().to_vec(), cert, key }
    }

    /// A TLS server on localhost that sends exactly `chain` (leaf first) and answers every request with a small body.
    fn serve(chain: Vec<Vec<u8>>, key: &KeyPair, chunked: bool) -> u16 {
        let cfg = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain.into_iter().map(CertificateDer::from).collect(), rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap())
            .unwrap();
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut c = rustls::ServerConnection::new(cfg).unwrap();
                    let mut t = rustls::Stream::new(&mut c, &mut s);
                    let mut seen = Vec::new();
                    let mut b = [0u8; 1];
                    while !seen.ends_with(b"\r\n\r\n") {
                        match t.read(&mut b) {
                            Ok(1) => seen.push(b[0]),
                            _ => return,
                        }
                    }
                    let resp: &[u8] = if chunked { b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n" } else { b"ICY 200 OK\r\nContent-Type: audio/mpeg\r\n\r\nhello" };
                    let _ = t.write_all(resp);
                    let _ = t.flush();
                    c.send_close_notify();
                    let _ = c.complete_io(&mut s);
                });
            }
        });
        port
    }

    fn body(h: StreamHead) -> String {
        let mut s = String::new();
        let mut b = h.body;
        let _ = b.read_to_string(&mut s);
        s
    }

    fn url(port: u16) -> String {
        format!("https://localhost:{port}/stream?x=1")
    }

    fn fp(m: &Made) -> String {
        crate::certinfo::fingerprint(&m.der)
    }

    #[test]
    fn a_leaf_alone_is_read_described_and_refused_by_the_normal_check() {
        let root = ca("Test Root");
        let l = leaf(&["localhost"], &root, None, Some("http://ca.example/int.der"));
        let port = serve(vec![l.der.clone()], &l.key, false);
        let chain = peer_chain("localhost", port).unwrap();
        assert_eq!(chain.len(), 1);
        let info = crate::certinfo::parse(&chain[0]).unwrap();
        assert_eq!(info.sha256, fp(&l));
        assert_eq!(info.subject, "localhost");
        assert_eq!(info.issuer, "Test Root");
        assert!(info.not_after > info.not_before && !info.self_issued);
        assert_eq!(info.ca_issuers, vec!["http://ca.example/int.der"]);
        // the usual check, with the built-in roots, says the issuer is unknown
        let e = match open_tls(&url(port), &[], &TlsMode::ExtraChain(vec![])) {
            Err(e) => e,
            Ok(_) => panic!("must be refused"),
        };
        assert!(is_unknown_issuer(&e), "{e}");
    }

    #[test]
    fn a_pinned_certificate_is_accepted_for_its_host_only_and_a_changed_one_is_refused() {
        let root = ca("Test Root");
        let l = leaf(&["localhost"], &root, None, None);
        let port = serve(vec![l.der.clone()], &l.key, false);
        let h = open_tls(&url(port), &[("Icy-MetaData".into(), "1".into())], &TlsMode::Pin(fp(&l))).unwrap();
        assert_eq!((h.status, h.header("content-type")), (200, Some("audio/mpeg")));
        assert_eq!(body(h), "hello");
        // typed in another style it is the same pin
        assert!(open_tls(&url(port), &[], &TlsMode::Pin(fp(&l).to_ascii_lowercase().replace(':', " "))).is_ok());
        // a different certificate on the same address: refused, and it says so
        let other = leaf(&["localhost"], &root, None, None);
        let port2 = serve(vec![other.der.clone()], &other.key, false);
        let e = open_tls(&url(port2), &[], &TlsMode::Pin(fp(&l))).err().unwrap();
        assert!(e.contains(CHANGED) && e.contains(&fp(&other)), "{e}");
    }

    #[test]
    fn the_name_and_the_dates_are_still_checked_under_a_pin() {
        let root = ca("Test Root");
        let wrong = leaf(&["other.example"], &root, None, None);
        let port = serve(vec![wrong.der.clone()], &wrong.key, false);
        let e = open_tls(&url(port), &[], &TlsMode::Pin(fp(&wrong))).err().unwrap();
        assert!(e.contains(WRONG_NAME), "{e}");
        let old = leaf(&["localhost"], &root, Some((2019, 2020)), None);
        let port = serve(vec![old.der.clone()], &old.key, false);
        let e = open_tls(&url(port), &[], &TlsMode::Pin(fp(&old))).err().unwrap();
        assert!(e.contains(EXPIRED) && e.contains("2020-01-01"), "{e}");
        let future = leaf(&["localhost"], &root, Some((2090, 2091)), None);
        let port = serve(vec![future.der.clone()], &future.key, false);
        assert!(open_tls(&url(port), &[], &TlsMode::Pin(fp(&future))).err().unwrap().contains(NOT_YET));
    }

    #[test]
    fn a_missing_intermediate_can_be_added_but_it_must_really_lead_to_a_trusted_root() {
        let root = ca("Test Root");
        let mut ip = CertificateParams::new(vec![]).unwrap();
        ip.distinguished_name.push(DnType::CommonName, "Test Intermediate");
        ip.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ikey = KeyPair::generate().unwrap();
        let icert = ip.signed_by(&ikey, &root.cert, &root.key).unwrap();
        let inter = Made { der: icert.der().to_vec(), cert: icert, key: ikey };
        let l = leaf(&["localhost"], &inter, None, None);
        let port = serve(vec![l.der.clone()], &l.key, true);
        let trust = |ders: &[&Made]| {
            let mut s = rustls::RootCertStore::empty();
            for m in ders {
                s.add(CertificateDer::from(m.der.clone())).unwrap();
            }
            s
        };
        // the server sent the leaf only: not enough
        let e = open_with_roots(&url(port), &[], &TlsMode::ExtraChain(vec![]), trust(&[&root])).err().unwrap();
        assert!(is_unknown_issuer(&e), "{e}");
        // with the intermediate added it verifies against the (test) root, over a chunked answer
        let h = open_with_roots(&url(port), &[], &TlsMode::ExtraChain(vec![inter.der.clone()]), trust(&[&root])).unwrap();
        assert_eq!(body(h), "hello");
        // an intermediate of some other authority does not help
        let stranger = ca("Stranger");
        let e = open_with_roots(&url(port), &[], &TlsMode::ExtraChain(vec![stranger.der.clone()]), trust(&[&root])).err().unwrap();
        assert!(is_unknown_issuer(&e), "{e}");
    }

    #[test]
    fn addresses_are_split_for_the_pin() {
        assert_eq!(split_https("https://Radios.Example.org:8002/3?a=b").unwrap(), ("radios.example.org".into(), 8002, "/3?a=b".into()));
        assert_eq!(split_https("https://x.org").unwrap(), ("x.org".into(), 443, "/".into()));
        assert_eq!(split_https("https://x.org?q=1").unwrap().2, "/?q=1");
        assert!(split_https("http://x.org/").is_err());
        let plan = TlsPlan::new(Some(("Radios.Example.org", 8002, "AB:CD")));
        assert_eq!(plan.mode_for("https://radios.example.org:8002/x"), Some(TlsMode::Pin("AB:CD".into())));
        assert_eq!(plan.mode_for("https://radios.example.org/x"), None);
        assert_eq!(plan.mode_for("https://elsewhere.example.org:8002/x"), None);
        assert_eq!(plan.mode_for("http://radios.example.org:8002/x"), None);
    }

    /// Needs the internet: `IYAKKI_TLS_URL=https://host:port/path cargo test -p iyakki-proxy --lib live_ -- --ignored --nocapture`
    /// Shows what the ordinary transport says, what the server presents, and whether a pin to that certificate plays.
    #[test]
    #[ignore]
    fn live_server_without_its_intermediate() {
        let u = std::env::var("IYAKKI_TLS_URL").unwrap_or_else(|_| "https://radios.crabdance.com:8002/3".into());
        let ordinary = crate::https::UreqStreamer::default();
        match crate::Streamer::open(&ordinary, &u, &[]) {
            Ok(h) => println!("ordinary check: connected, status {}", h.status),
            Err(e) => println!("ordinary check: {e}  (unknown issuer: {})", is_unknown_issuer(&e)),
        }
        let (host, port, _) = split_https(&u).unwrap();
        let chain = peer_chain(&host, port).unwrap();
        for c in &chain {
            let i = crate::certinfo::parse(c).unwrap();
            println!("{} | {} <- {} | until {} | aia {:?}", i.sha256, i.subject, i.issuer, crate::certinfo::date_text(i.not_after), i.ca_issuers);
        }
        let fp = crate::certinfo::fingerprint(&chain[0]);
        let h = open_tls(&u, &[("Icy-MetaData".into(), "1".into())], &TlsMode::Pin(fp)).unwrap();
        println!("pinned: status {} content-type {:?}", h.status, h.header("content-type"));
        let mut buf = [0u8; 4096];
        let n = h.body.take(4096).read(&mut buf).unwrap();
        println!("read {n} bytes");
    }
}
