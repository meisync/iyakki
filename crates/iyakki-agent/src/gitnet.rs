//! The Git connections' own HTTP(S) client: no `curl` and no other program. One request per connection (`Connection: close`),
//! HTTP/1.1 with `Content-Length` or chunked answers, TLS by rustls.
//!
//! * The token goes into the request header in memory only: never a file, never an argument, never a log line.
//! * A certificate the person chose to trust (`Req::cert`, a PEM file) is pinned: the server's certificate must be exactly that one.
//!   Otherwise the system's CA bundle decides (`/etc/ssl/certs/ca-certificates.crt` and the usual other places).
//! * `Req::timeout_s` (0: 20) bounds the whole request, `Req::max_bytes` (0: 512 KB) the answer; an answer over the limit is
//!   [`TOO_BIG`], as it was with curl. Redirects are not followed.
//! * [`NetGit::peer_cert`] reads the certificate a server presents without trusting it (for the person to compare).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::gitconn::{cert_info, scrub, CertInfo, GitHttp, NetErr, Req, Resp, Style, TOO_BIG};

/// Where the system keeps its CA bundle (first that exists and holds certificates).
const CA_FILES: [&str; 4] = ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/cert.pem", "/etc/ssl/ca-bundle.pem"];

pub struct NetGit {
    /// Extra CA bundle files to try first (tests, odd images).
    pub ca_files: Vec<std::path::PathBuf>,
    /// Accept any certificate (only for reachability probes of a registry on the LAN, where no secret is sent).
    pub insecure: bool,
}

impl Default for NetGit {
    fn default() -> Self {
        NetGit { ca_files: Vec::new(), insecure: false }
    }
}

#[derive(Debug)]
struct Url {
    https: bool,
    host: String,
    port: u16,
    target: String,
}

fn parse_url(u: &str) -> Result<Url, String> {
    let (https, rest) = if let Some(r) = u.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = u.strip_prefix("http://") {
        (false, r)
    } else {
        return Err("only http and https addresses are used".into());
    };
    let (auth, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if auth.contains('@') || auth.is_empty() {
        return Err("the address must not carry a user name or password".into());
    }
    let target = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    if target.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err("the address holds a space or control character".into());
    }
    let (host, port) = if let Some(r) = auth.strip_prefix('[') {
        let (h, p) = r.split_once(']').ok_or("bad IPv6 address")?;
        (h.to_string(), p.strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| "bad port")?)
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p.parse::<u16>().map_err(|_| "bad port")?)),
            None => (auth.to_string(), None),
        }
    };
    Ok(Url { https, host, port: port.unwrap_or(if https { 443 } else { 80 }), target })
}

fn pem_certs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(s) = rest.find("-----BEGIN CERTIFICATE-----") {
        let after = &rest[s + 27..];
        let Some(e) = after.find("-----END CERTIFICATE-----") else { break };
        let b64: String = after[..e].chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(der) = crate::sshkey::unb64(&b64) {
            out.push(der);
        }
        rest = &after[e + 25..];
    }
    out
}

fn roots(extra: &[std::path::PathBuf]) -> Result<rustls::RootCertStore, String> {
    let mut store = rustls::RootCertStore::empty();
    for f in extra.iter().map(|p| p.display().to_string()).chain(CA_FILES.iter().map(|s| s.to_string())) {
        let Ok(t) = std::fs::read_to_string(&f) else { continue };
        let (added, _) = store.add_parsable_certificates(pem_certs(&t).into_iter().map(CertificateDer::from));
        if added > 0 {
            return Ok(store);
        }
    }
    Err("this system has no list of trusted certificate authorities (no ca-certificates.crt), so an https address cannot be checked".into())
}

/// How many trusted certificate authorities the system's bundle holds (the list every https call here checks against), or why it has
/// none. The boot self-test calls this so an image without `ca-certificates.crt` fails there and not on the person's first https address.
pub fn ca_bundle_certs() -> Result<usize, String> {
    roots(&[]).map(|s| s.len())
}

#[derive(Debug)]
struct Seen {
    /// Only this certificate is accepted (the one the person trusted); None: any (reading what a server shows).
    expect: Option<Vec<u8>>,
    seen: Arc<Mutex<Option<Vec<u8>>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Seen {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(end.as_ref().to_vec());
        }
        match &self.expect {
            Some(e) if e.as_slice() != end.as_ref() => Err(rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn connect(u: &Url, deadline: Instant) -> Result<TcpStream, NetErr> {
    let left = |d: Instant| d.saturating_duration_since(Instant::now()).max(Duration::from_millis(200));
    let addrs: Vec<_> = (u.host.as_str(), u.port).to_socket_addrs().map_err(|e| NetErr::Unreachable(format!("{}: {e}", u.host)))?.collect();
    let mut last = format!("{}: no address", u.host);
    for a in addrs {
        match TcpStream::connect_timeout(&a, left(deadline).min(Duration::from_secs(10))) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("cannot connect to {}:{}: {e}", u.host, u.port),
        }
    }
    Err(NetErr::Unreachable(last))
}

fn tls_err(e: &std::io::Error) -> Option<String> {
    let inner = e.get_ref()?.downcast_ref::<rustls::Error>()?;
    Some(match inner {
        rustls::Error::InvalidCertificate(c) => format!("the server's certificate is not trusted ({c:?})"),
        o => format!("TLS: {o}"),
    })
}

/// A readable and writable stream with the request's deadline on every call.
trait Wire: Read + Write {
    fn limit(&mut self, left: Duration);
}
struct Plain(TcpStream);
impl Read for Plain {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for Plain {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Wire for Plain {
    fn limit(&mut self, left: Duration) {
        let _ = self.0.set_read_timeout(Some(left));
        let _ = self.0.set_write_timeout(Some(left));
    }
}
struct Secure(rustls::StreamOwned<rustls::ClientConnection, TcpStream>);
impl Read for Secure {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for Secure {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Wire for Secure {
    fn limit(&mut self, left: Duration) {
        let _ = self.0.sock.set_read_timeout(Some(left));
        let _ = self.0.sock.set_write_timeout(Some(left));
    }
}

fn header_end(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

fn dechunk(mut b: &[u8], max: usize) -> Result<Vec<u8>, NetErr> {
    let mut out = Vec::new();
    loop {
        let Some(i) = b.windows(2).position(|w| w == b"\r\n") else { return Ok(out) };
        let size = usize::from_str_radix(std::str::from_utf8(&b[..i]).unwrap_or("").split(';').next().unwrap_or("").trim(), 16).map_err(|_| NetErr::Other("the answer is not valid HTTP (chunk size)".into()))?;
        b = &b[i + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            out.extend_from_slice(b);
            return Ok(out);
        }
        out.extend_from_slice(&b[..size]);
        if out.len() > max {
            return Err(NetErr::Other(TOO_BIG.into()));
        }
        b = b[size..].strip_prefix(b"\r\n").unwrap_or(&b[size..]);
    }
}

fn exchange(w: &mut dyn Wire, request: &[u8], deadline: Instant, max: usize, token: &str) -> Result<Resp, NetErr> {
    let left = || deadline.saturating_duration_since(Instant::now());
    let io = |e: std::io::Error, token: &str| -> NetErr {
        match tls_err(&e) {
            Some(t) => NetErr::Tls(scrub(&t, token)),
            None => NetErr::Unreachable(scrub(&format!("{e}"), token)),
        }
    };
    w.limit(left().max(Duration::from_millis(200)));
    w.write_all(request).and_then(|_| w.flush()).map_err(|e| io(e, token))?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut head_len = None;
    loop {
        if left().is_zero() {
            return Err(NetErr::Unreachable("no answer in time".into()));
        }
        w.limit(left().max(Duration::from_millis(200)));
        match w.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            // a TLS server that closes without close_notify after its answer is common; what was read is the answer
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && head_len.is_some() => break,
            Err(e) if head_len.is_some() && matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(NetErr::Unreachable("the answer stopped half way".into())),
            Err(e) => return Err(io(e, token)),
        }
        if head_len.is_none() {
            head_len = header_end(&buf);
        }
        if buf.len() > max + 64 * 1024 {
            return Err(NetErr::Other(TOO_BIG.into()));
        }
        if let Some(h) = head_len {
            let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
            if let Some(cl) = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()) {
                if cl > max {
                    return Err(NetErr::Other(TOO_BIG.into()));
                }
                if buf.len() >= h + 4 + cl {
                    break;
                }
            }
            if head.contains("transfer-encoding: chunked") && buf.ends_with(b"0\r\n\r\n") {
                break;
            }
        }
    }
    let h = header_end(&buf).ok_or_else(|| NetErr::Other("the answer is not valid HTTP".into()))?;
    let head = String::from_utf8_lossy(&buf[..h]).to_string();
    let status: u16 = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).ok_or_else(|| NetErr::Other("the answer is not valid HTTP (status line)".into()))?;
    let headers: Vec<(String, String)> = head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let raw = &buf[h + 4..];
    let body = if headers.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked")) { dechunk(raw, max)? } else { raw.to_vec() };
    if body.len() > max {
        return Err(NetErr::Other(TOO_BIG.into()));
    }
    Ok(Resp { status, headers, body: String::from_utf8_lossy(&body).to_string() })
}

impl NetGit {
    fn tls(&self, u: &Url, expect: Option<Vec<u8>>, any: bool, seen: Arc<Mutex<Option<Vec<u8>>>>) -> Result<rustls::ClientConnection, NetErr> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| NetErr::Other(e.to_string()))?;
        let cfg = if expect.is_some() || any {
            builder.dangerous().with_custom_certificate_verifier(Arc::new(Seen { expect, seen, provider })).with_no_client_auth()
        } else {
            builder.with_root_certificates(roots(&self.ca_files).map_err(NetErr::Tls)?).with_no_client_auth()
        };
        let name = ServerName::try_from(u.host.clone()).map_err(|_| NetErr::Other(format!("{} is not a valid host name", u.host)))?;
        rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| NetErr::Other(e.to_string()))
    }
}

impl GitHttp for NetGit {
    fn send(&self, r: &Req) -> Result<Resp, NetErr> {
        let u = parse_url(&r.url).map_err(NetErr::Other)?;
        let deadline = Instant::now() + Duration::from_secs(if r.timeout_s == 0 { 20 } else { r.timeout_s });
        let max = if r.max_bytes == 0 { 512 * 1024 } else { r.max_bytes };
        let token = r.token.clone().unwrap_or_default();
        let accept = r.accept.as_deref().filter(|a| a.chars().all(|c| c.is_ascii_graphic() || c == ' ')).unwrap_or("application/json");
        if !r.method.chars().all(|c| c.is_ascii_uppercase()) || r.method.is_empty() {
            return Err(NetErr::Other("bad request method".into()));
        }
        let hostline = if u.port == if u.https { 443 } else { 80 } { u.host.clone() } else { format!("{}:{}", u.host, u.port) };
        let mut req = format!("{} {} HTTP/1.1\r\nHost: {hostline}\r\nConnection: close\r\nUser-Agent: iyakki-git\r\nAccept: {accept}\r\n", r.method, u.target);
        if let Some(t) = &r.token {
            if t.chars().any(|c| c.is_control()) {
                return Err(NetErr::Other("the token holds a control character".into()));
            }
            req.push_str(&format!("Authorization: {} {t}\r\n", if r.style == Style::Token { "token" } else { "Bearer" }));
        }
        match &r.body {
            Some(b) => req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len())),
            None => req.push_str(&format!("{}\r\n", if matches!(r.method.as_str(), "POST" | "PUT" | "PATCH") { "Content-Length: 0\r\n" } else { "" })),
        }
        let sock = connect(&u, deadline)?;
        let request = req.into_bytes();
        let out = if u.https {
            let expect = match &r.cert {
                Some(p) => Some(pem_certs(&std::fs::read_to_string(p).map_err(|e| NetErr::Other(format!("{}: {e}", p.display())))?).into_iter().next().ok_or_else(|| NetErr::Other("the trusted certificate file holds no certificate".into()))?),
                None => None,
            };
            let conn = self.tls(&u, expect, self.insecure, Arc::new(Mutex::new(None)))?;
            let mut w = Secure(rustls::StreamOwned::new(conn, sock));
            exchange(&mut w, &request, deadline, max, &token)
        } else {
            exchange(&mut Plain(sock), &request, deadline, max, &token)
        };
        out.map_err(|e| match e {
            NetErr::Unreachable(m) => NetErr::Unreachable(scrub(&m, &token)),
            o => o,
        })
    }

    fn peer_cert(&self, url: &str) -> Result<CertInfo, String> {
        if !url.starts_with("https://") {
            return Err("only https servers have a certificate".into());
        }
        let u = parse_url(url)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let sock = connect(&u, deadline).map_err(|e| format!("{e:?}"))?;
        let _ = sock.set_read_timeout(Some(Duration::from_secs(15)));
        let _ = sock.set_write_timeout(Some(Duration::from_secs(15)));
        let seen = Arc::new(Mutex::new(None));
        let mut conn = self.tls(&u, None, true, seen.clone()).map_err(|e| format!("{e:?}"))?;
        let mut sock = sock;
        // the handshake only: nothing is sent to the server but the hello and the finish
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(|e| format!("the TLS handshake failed: {e}"))?;
        }
        let der = seen.lock().ok().and_then(|s| s.clone()).ok_or("the server did not show a certificate")?;
        let b64 = crate::wsproto::base64(&der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for l in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(l).unwrap_or(""));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        cert_info(&pem)
    }
}

#[cfg(test)]
#[path = "gitnet_tests.rs"]
mod tests;
