//! An FTP / FTPS (explicit TLS, `AUTH TLS`) CLIENT for the remote files client (`remotefiles.rs`). In-house; TLS is rustls, which the
//! agent already carries (`gitnet.rs`).
//!
//! * FTPS: the control connection is upgraded before the user name and password are sent, and the data connections are encrypted too
//!   (`PROT P`). The server's certificate is pinned on first use, exactly like an SSH host key: `cert_ok(fingerprint)` says whether it is
//!   the pinned one, and when it is not the connection ends before anything secret is sent ([`FtpErr::Cert`]). A data connection must
//!   show the same certificate as the control connection. A server that insists on TLS session reuse for data connections may refuse
//!   (rustls resumes sessions when the server issued a ticket; not proven against every server).
//! * Plain FTP is the caller's decision (the explicit warning toggle); nothing is encrypted.
//! * The address in a `PASV` answer is never used: data connections go to the address the control connection already reached, so a
//!   hostile server cannot make this computer connect to some other host. Ports of 0 are refused.
//! * Hostile input: reply lines are capped, replies are capped, listings are capped, names that are paths are dropped, commands never
//!   carry a CR, LF or NUL from a path (no command injection).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::remotefs::{clean_path, Entry, RemoteFs};
use crate::rf_sftp::plain_name;

const MAX_LINE: usize = 4096;
const MAX_REPLY_LINES: usize = 2000;
const MAX_LISTING: usize = 32 * 1024 * 1024;
const IDLE: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum FtpErr {
    /// The certificate is not the pinned one (or nothing is pinned yet). Nothing secret was sent.
    Cert { fingerprint: String },
    Auth(String),
    Other(String),
}

impl std::fmt::Display for FtpErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FtpErr::Cert { fingerprint } => write!(f, "the server's certificate {fingerprint} is not trusted"),
            FtpErr::Auth(s) | FtpErr::Other(s) => f.write_str(s),
        }
    }
}

impl From<&str> for FtpErr {
    fn from(s: &str) -> FtpErr {
        FtpErr::Other(s.into())
    }
}

impl From<String> for FtpErr {
    fn from(s: String) -> FtpErr {
        FtpErr::Other(s)
    }
}

/// `SHA256:` + base64 of the certificate, the same look as an SSH fingerprint.
pub fn cert_fingerprint(der: &[u8]) -> String {
    format!("SHA256:{}", crate::b64::encode(&Sha256::digest(der)).trim_end_matches('='))
}

#[derive(Debug)]
struct Recorder {
    seen: Arc<Mutex<Option<Vec<u8>>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(end.as_ref().to_vec());
        }
        // judged by the caller's pin right after the handshake, before anything secret is sent
        Ok(ServerCertVerified::assertion())
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

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(b),
            Stream::Tls(s) => s.read(b),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(b),
            Stream::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

impl Stream {
    fn finish(&mut self) {
        if let Stream::Tls(s) = self {
            s.conn.send_close_notify();
            let _ = s.flush();
        }
    }
}

pub struct FtpFs {
    ctl: Stream,
    peer: IpAddr,
    buf: Vec<u8>,
    tls: Option<(Arc<rustls::ClientConfig>, ServerName<'static>, Vec<u8>)>,
    stop: Arc<AtomicBool>,
    no_mlsd: bool,
    binary: bool,
}

fn tls_config(seen: Arc<Mutex<Option<Vec<u8>>>>) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS is not available: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Recorder { seen, provider }))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

pub fn connect_tcp(host: &str, port: u16, stop: &AtomicBool) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(|_| format!("could not find {host}"))?.collect();
    let mut last = format!("could not reach {host}:{port}");
    for a in addrs {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = format!("could not reach {host}:{port} ({})", e.kind()),
        }
    }
    Err(last)
}

fn handshake(conn: &mut rustls::ClientConnection, tcp: &mut TcpStream, stop: &AtomicBool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while conn.is_handshaking() {
        if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            return Err("the TLS handshake did not finish".into());
        }
        match conn.complete_io(tcp) {
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("the TLS handshake failed ({e})")),
        }
    }
    Ok(())
}

/// Connect, secure (FTPS) and log in. `cert_ok` is asked with the certificate fingerprint before any secret is sent.
pub fn connect(host: &str, port: u16, user: &str, password: &str, tls: bool, cert_ok: &dyn Fn(&str) -> bool, stop: Arc<AtomicBool>) -> Result<FtpFs, FtpErr> {
    let tcp = connect_tcp(host, port, &stop)?;
    let peer = tcp.peer_addr().map_err(|e| e.to_string())?.ip();
    let mut f = FtpFs { ctl: Stream::Plain(tcp), peer, buf: Vec::new(), tls: None, stop: stop.clone(), no_mlsd: false, binary: false };
    let (code, text) = f.reply()?;
    if code != 220 {
        return Err(FtpErr::Other(format!("the server did not greet us ({code} {text})")));
    }
    if tls {
        let (code, text) = f.cmd("AUTH TLS")?;
        if code != 234 {
            return Err(FtpErr::Other(format!("the server does not offer FTPS (explicit TLS): {code} {text}")));
        }
        let seen = Arc::new(Mutex::new(None));
        let cfg = tls_config(seen.clone())?;
        let name = ServerName::try_from(host.to_string()).map_err(|_| format!("{host} is not a valid server name"))?;
        let mut conn = rustls::ClientConnection::new(cfg.clone(), name.clone()).map_err(|e| format!("TLS: {e}"))?;
        let spare = f.dup()?;
        let Stream::Plain(mut tcp) = std::mem::replace(&mut f.ctl, Stream::Plain(spare)) else { return Err("internal: no plain stream".to_string().into()) };
        handshake(&mut conn, &mut tcp, &stop)?;
        let leaf = seen.lock().ok().and_then(|s| s.clone()).ok_or("the server showed no certificate")?;
        let fp = cert_fingerprint(&leaf);
        if !cert_ok(&fp) {
            return Err(FtpErr::Cert { fingerprint: fp });
        }
        f.ctl = Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)));
        f.buf.clear();
        f.tls = Some((cfg, name, leaf));
        for c in ["PBSZ 0", "PROT P"] {
            let (code, text) = f.cmd(c)?;
            if code != 200 {
                return Err(FtpErr::Other(format!("the server refuses to encrypt the file data ({code} {text})")));
            }
        }
    }
    let (code, text) = f.cmd(&format!("USER {}", check_arg(user)?))?;
    let (code, text) = match code {
        331 | 332 => f.secret_cmd(password)?,
        _ => (code, text),
    };
    match code {
        230 | 202 => {}
        530 | 331 | 332 | 421 => return Err(FtpErr::Auth(format!("the server did not accept the login ({code} {text})"))),
        _ => return Err(FtpErr::Other(format!("unexpected answer to the login: {code} {text}"))),
    }
    let _ = f.cmd("OPTS UTF8 ON");
    Ok(f)
}

fn check_arg(s: &str) -> Result<&str, String> {
    if s.contains(['\r', '\n', '\0']) {
        Err("a name with a line break or NUL cannot be sent to an FTP server".into())
    } else {
        Ok(s)
    }
}

impl FtpFs {
    fn dup(&self) -> Result<TcpStream, String> {
        match &self.ctl {
            Stream::Plain(s) => s.try_clone().map_err(|e| e.to_string()),
            Stream::Tls(s) => s.sock.try_clone().map_err(|e| e.to_string()),
        }
    }

    fn read_line(&mut self) -> Result<String, String> {
        let deadline = Instant::now() + IDLE;
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let s = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
                return Ok(s);
            }
            if self.buf.len() > MAX_LINE {
                return Err("the server sent a line that is too long".into());
            }
            if self.stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if Instant::now() > deadline {
                return Err("the server did not answer in time".into());
            }
            let mut chunk = [0u8; 1024];
            match self.ctl.read(&mut chunk) {
                Ok(0) => return Err("the connection was closed by the server".into()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(format!("network error: {}", e.kind())),
            }
        }
    }

    /// One reply: the code and the text of the last line.
    fn reply(&mut self) -> Result<(u16, String), String> {
        let first = self.read_line()?;
        let code: u16 = first.get(..3).and_then(|c| c.parse().ok()).ok_or("the server's answer does not start with a code")?;
        if first.as_bytes().get(3) != Some(&b'-') {
            return Ok((code, first.get(4..).unwrap_or("").to_string()));
        }
        for _ in 0..MAX_REPLY_LINES {
            let l = self.read_line()?;
            if l.len() >= 4 && l.get(..3).and_then(|c| c.parse::<u16>().ok()) == Some(code) && l.as_bytes()[3] == b' ' {
                return Ok((code, l[4..].to_string()));
            }
        }
        Err("the server's answer has too many lines".into())
    }

    fn cmd(&mut self, c: &str) -> Result<(u16, String), String> {
        self.ctl.write_all(format!("{c}\r\n").as_bytes()).and_then(|_| self.ctl.flush()).map_err(|e| format!("network error: {}", e.kind()))?;
        self.reply()
    }

    /// `PASS`, with the password only in a buffer that is wiped.
    fn secret_cmd(&mut self, password: &str) -> Result<(u16, String), String> {
        let line = Zeroizing::new(format!("PASS {}\r\n", check_arg(password)?));
        self.ctl.write_all(line.as_bytes()).and_then(|_| self.ctl.flush()).map_err(|e| format!("network error: {}", e.kind()))?;
        self.reply()
    }

    fn expect(&mut self, c: &str, ok: &[u16]) -> Result<String, String> {
        let (code, text) = self.cmd(c)?;
        if ok.contains(&code) { Ok(text) } else { Err(failure(code, &text)) }
    }

    fn type_i(&mut self) -> Result<(), String> {
        if !self.binary {
            self.expect("TYPE I", &[200])?;
            self.binary = true;
        }
        Ok(())
    }

    /// Open a passive data connection (and secure it when the control connection is).
    fn data(&mut self) -> Result<Stream, String> {
        let port = match self.cmd("EPSV")? {
            (229, t) => {
                let inner = t.split_once('(').and_then(|(_, r)| r.split_once(')')).map(|(i, _)| i).unwrap_or("");
                let parts: Vec<&str> = inner.split('|').collect();
                parts.get(3).and_then(|p| p.parse::<u16>().ok())
            }
            _ => None,
        };
        let port = match port {
            Some(p) => p,
            None => {
                let t = self.expect("PASV", &[227])?;
                let inner = t.split_once('(').and_then(|(_, r)| r.split_once(')')).map(|(i, _)| i).unwrap_or("");
                let n: Vec<u16> = inner.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if n.len() != 6 || n.iter().any(|x| *x > 255) {
                    return Err("the server's passive-mode answer is not understood".into());
                }
                n[4] * 256 + n[5]
            }
        };
        if port == 0 {
            return Err("the server offered port 0 for the data connection".into());
        }
        let tcp = TcpStream::connect_timeout(&SocketAddr::new(self.peer, port), Duration::from_secs(10)).map_err(|e| format!("could not open the data connection ({})", e.kind()))?;
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
        let _ = tcp.set_nodelay(true);
        Ok(Stream::Plain(tcp))
    }

    /// The data stream after the transfer command was accepted: wrapped in TLS when needed, with the same certificate as the control.
    fn secure_data(&mut self, s: Stream) -> Result<Stream, String> {
        let Some((cfg, name, leaf)) = self.tls.clone() else { return Ok(s) };
        let Stream::Plain(mut tcp) = s else { return Ok(s) };
        let mut conn = rustls::ClientConnection::new(cfg, name).map_err(|e| format!("TLS: {e}"))?;
        handshake(&mut conn, &mut tcp, &self.stop)?;
        let shown = conn.peer_certificates().and_then(|c| c.first().map(|d| d.as_ref().to_vec()));
        if shown.as_deref() != Some(leaf.as_slice()) {
            return Err("the data connection showed a different certificate than the control connection".into());
        }
        Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
    }

    fn read_all_data(&mut self, s: &mut Stream) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        let mut deadline = Instant::now() + IDLE;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if Instant::now() > deadline {
                return Err("the server stopped sending".into());
            }
            match s.read(&mut chunk) {
                Ok(0) => return Ok(out),
                Ok(n) => {
                    out.extend_from_slice(&chunk[..n]);
                    deadline = Instant::now() + IDLE;
                    if out.len() > MAX_LISTING {
                        return Err("the listing is larger than 32 MB".into());
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                // some servers close the TLS data connection without a close_notify
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(out),
                Err(e) => return Err(format!("network error: {}", e.kind())),
            }
        }
    }

    fn listing(&mut self, c: &str) -> Result<Vec<u8>, (u16, String)> {
        let data = self.data().map_err(|e| (0, e))?;
        let (code, text) = self.cmd(c).map_err(|e| (0, e))?;
        if code != 125 && code != 150 {
            return Err((code, text));
        }
        let mut s = self.secure_data(data).map_err(|e| (0, e))?;
        let bytes = self.read_all_data(&mut s).map_err(|e| (0, e))?;
        s.finish();
        drop(s);
        let (code, text) = self.reply().map_err(|e| (0, e))?;
        if code != 226 && code != 250 {
            return Err((code, text));
        }
        Ok(bytes)
    }
}

fn failure(code: u16, text: &str) -> String {
    let base = match code {
        530 | 532 => "not logged in",
        550 => "the file or folder is not there, or you may not do that",
        553 => "that name is not allowed",
        552 => "the server is out of space",
        421 => "the server is closing the connection",
        450 | 451 | 452 => "the server cannot do that right now",
        500..=504 => "the server does not support that",
        _ => "the server refused",
    };
    let t: String = text.chars().filter(|c| !c.is_control()).take(120).collect();
    if t.is_empty() { format!("{base} ({code})") } else { format!("{base} ({code} {t})") }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn year_of(unix: i64) -> i64 {
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

/// One line of a Unix style `LIST`. Lines it cannot read are skipped by the caller.
pub fn parse_list_line(line: &str, now: i64) -> Option<Entry> {
    let l = line.trim_end();
    let kind = l.chars().next()?;
    if !matches!(kind, 'd' | '-' | 'l') || l.len() < 12 {
        return None;
    }
    // eight whitespace separated fields, then the name (which may hold spaces)
    let mut rest = l;
    let mut f: Vec<&str> = Vec::with_capacity(8);
    for _ in 0..8 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        f.push(&rest[..end]);
        rest = &rest[end..];
    }
    let name = rest.strip_prefix(' ').unwrap_or(rest.trim_start());
    let name = if kind == 'l' { name.split(" -> ").next().unwrap_or(name) } else { name };
    if !plain_name(name) {
        return None;
    }
    let size: u64 = f[4].parse().ok()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| f[5].to_ascii_lowercase().starts_with(m))? as i64 + 1;
    let day: i64 = f[6].parse().ok()?;
    let modified = if let Some((h, m)) = f[7].split_once(':') {
        let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
        let mut y = year_of(now);
        let mut t = days_from_civil(y, month, day) * 86_400 + h * 3600 + m * 60;
        if t > now + 86_400 {
            y -= 1;
            t = days_from_civil(y, month, day) * 86_400 + h * 3600 + m * 60;
        }
        t
    } else {
        days_from_civil(f[7].parse().ok()?, month, day) * 86_400
    };
    Some(Entry { name: name.to_string(), is_dir: kind == 'd', is_link: kind == 'l', size: if kind == 'd' { 0 } else { size }, modified, perms: f[0].get(1..10).unwrap_or("").to_string() })
}

/// One line of an `MLSD` listing.
pub fn parse_mlsd_line(line: &str) -> Option<Entry> {
    let (facts, name) = line.split_once(' ')?;
    if !plain_name(name) {
        return None;
    }
    let (mut is_dir, mut is_link, mut size, mut modified, mut perms) = (false, false, 0u64, 0i64, String::new());
    for f in facts.split(';').filter(|f| !f.is_empty()) {
        let (k, v) = f.split_once('=')?;
        match k.to_ascii_lowercase().as_str() {
            "type" => match v.to_ascii_lowercase().as_str() {
                "file" => {}
                "dir" => is_dir = true,
                t if t.starts_with("os.unix=slink") => is_link = true,
                _ => return None,
            },
            "size" => size = v.parse().ok()?,
            "modify" => {
                let d = v.split('.').next().unwrap_or("");
                if d.len() == 14 && d.bytes().all(|b| b.is_ascii_digit()) {
                    let n = |a: usize, b: usize| d[a..b].parse::<i64>().unwrap_or(0);
                    modified = days_from_civil(n(0, 4), n(4, 6), n(6, 8)) * 86_400 + n(8, 10) * 3600 + n(10, 12) * 60 + n(12, 14);
                }
            }
            "unix.mode" => {
                if let Ok(m) = u32::from_str_radix(v.trim_start_matches('0'), 8) {
                    for sh in [6, 3, 0] {
                        let b = (m >> sh) & 7;
                        perms.push(if b & 4 != 0 { 'r' } else { '-' });
                        perms.push(if b & 2 != 0 { 'w' } else { '-' });
                        perms.push(if b & 1 != 0 { 'x' } else { '-' });
                    }
                }
            }
            _ => {}
        }
    }
    Some(Entry { name: name.to_string(), is_dir, is_link, size: if is_dir { 0 } else { size }, modified, perms })
}

impl RemoteFs for FtpFs {
    fn list(&mut self, path: &str) -> Result<(String, Vec<Entry>), String> {
        self.expect(&format!("CWD {}", check_arg(path)?), &[250])?;
        let canon = match self.cmd("PWD")? {
            (257, t) => t.strip_prefix('"').and_then(|r| {
                let mut out = String::new();
                let mut it = r.chars().peekable();
                while let Some(c) = it.next() {
                    if c == '"' {
                        if it.peek() == Some(&'"') {
                            it.next();
                            out.push('"');
                        } else {
                            return Some(out);
                        }
                    } else {
                        out.push(c);
                    }
                }
                None
            }),
            _ => None,
        }
        .filter(|p| p.starts_with('/') && !p.contains('\0'))
        .map_or_else(|| clean_path(path), |p| clean_path(&p));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut entries = Vec::new();
        let mut used_mlsd = false;
        if !self.no_mlsd {
            match self.listing("MLSD") {
                Ok(b) => {
                    used_mlsd = true;
                    entries = String::from_utf8_lossy(&b).lines().filter_map(parse_mlsd_line).collect();
                }
                Err((c, _)) if (500..=504).contains(&c) || c == 550 => self.no_mlsd = true,
                Err((_, e)) if !e.is_empty() && !e.starts_with(char::is_numeric) => return Err(if e.is_empty() { "listing failed".into() } else { failure(0, &e) }),
                Err((c, e)) => return Err(failure(c, &e)),
            }
        }
        if !used_mlsd {
            let b = self.listing("LIST").map_err(|(c, e)| if c == 0 { e } else { failure(c, &e) })?;
            entries = String::from_utf8_lossy(&b).lines().filter_map(|l| parse_list_line(l, now)).collect();
        }
        if entries.len() > 200_000 {
            return Err("the folder has more than 200000 entries".into());
        }
        Ok((canon, entries))
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        self.expect(&format!("MKD {}", check_arg(path)?), &[257, 250]).map(|_| ())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        self.expect(&format!("RNFR {}", check_arg(from)?), &[350])?;
        self.expect(&format!("RNTO {}", check_arg(to)?), &[250]).map(|_| ())
    }
    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        self.expect(&format!("DELE {}", check_arg(path)?), &[250]).map(|_| ())
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        self.expect(&format!("RMD {}", check_arg(path)?), &[250]).map(|_| ())
    }
    fn size(&mut self, path: &str) -> Result<Option<u64>, String> {
        self.type_i()?;
        match self.cmd(&format!("SIZE {}", check_arg(path)?))? {
            (213, t) => Ok(Some(t.trim().parse().map_err(|_| "the server's SIZE answer is not a number")?)),
            (550, _) => Ok(None),
            (c, t) => Err(failure(c, &t)),
        }
    }

    fn download(&mut self, remote: &str, offset: u64, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        self.type_i()?;
        let data = self.data()?;
        if offset > 0 {
            self.expect(&format!("REST {offset}"), &[350]).map_err(|e| format!("the server cannot resume a download: {e}"))?;
        }
        let (code, text) = self.cmd(&format!("RETR {}", check_arg(remote)?))?;
        if code != 125 && code != 150 {
            return Err(failure(code, &text));
        }
        let mut s = self.secure_data(data)?;
        let mut chunk = vec![0u8; 32 * 1024];
        let mut deadline = Instant::now() + IDLE;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if Instant::now() > deadline {
                return Err("the server stopped sending".into());
            }
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    deadline = Instant::now() + IDLE;
                    out.write_all(&chunk[..n]).map_err(|e| format!("could not write the local file: {}", e.kind()))?;
                    if !progress(n as u64) {
                        return Err("cancelled".into());
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("network error: {}", e.kind())),
            }
        }
        drop(s);
        match self.reply()? {
            (226 | 250, _) => Ok(()),
            (c, t) => Err(failure(c, &t)),
        }
    }

    fn upload(&mut self, remote: &str, offset: u64, src: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String> {
        self.type_i()?;
        let data = self.data()?;
        let c = if offset > 0 { format!("APPE {}", check_arg(remote)?) } else { format!("STOR {}", check_arg(remote)?) };
        let (code, text) = self.cmd(&c)?;
        if code != 125 && code != 150 {
            return Err(failure(code, &text));
        }
        let mut s = self.secure_data(data)?;
        let mut chunk = vec![0u8; 32 * 1024];
        loop {
            let n = src.read(&mut chunk).map_err(|e| format!("could not read the local file: {}", e.kind()))?;
            if n == 0 {
                break;
            }
            let mut sent = 0;
            let deadline = Instant::now() + IDLE;
            while sent < n {
                if self.stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                if Instant::now() > deadline {
                    return Err("the server stopped receiving".into());
                }
                match s.write(&chunk[sent..n]) {
                    Ok(k) => sent += k,
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(format!("network error: {}", e.kind())),
                }
            }
            if !progress(n as u64) {
                return Err("cancelled".into());
            }
        }
        s.finish();
        drop(s);
        match self.reply()? {
            (226 | 250, _) => Ok(()),
            (c, t) => Err(failure(c, &t)),
        }
    }
}
