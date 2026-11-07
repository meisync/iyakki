//! The real network for the channels. Every connection first passes the tenant's egress allow-list (a tenant with no egress policy
//! sends nothing out: `egress_allows` must say `Some(true)`), refuses addresses that are not public (so a name cannot be pointed at
//! the cluster), checks the server's certificate against the system's authorities, and has a time limit. Not exercised against real
//! servers by the tests (they use fakes and loopback only).

use super::smtp::{self, Auth, Mail, MailStream, TlsMode};
use super::webhook::HttpReq;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct MailJob {
    pub host: String,
    pub port: u16,
    pub mode: TlsMode,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub now: u64,
    pub id: String,
}

/// What the hub needs from the network; tests supply a fake.
pub trait Net: Send + Sync {
    /// The status code of the answer; the answer's body is never read further.
    fn post(&self, team: Option<&str>, req: &HttpReq) -> Result<u16, String>;
    fn send_mail(&self, team: Option<&str>, job: &MailJob) -> Result<(), String>;
}

/// Not bound: nothing leaves.
pub struct NoNet;
impl Net for NoNet {
    fn post(&self, _: Option<&str>, _: &HttpReq) -> Result<u16, String> {
        Err("no network is bound for notification channels".into())
    }
    fn send_mail(&self, _: Option<&str>, _: &MailJob) -> Result<(), String> {
        Err("no network is bound for notification channels".into())
    }
}

pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_broadcast() || a.is_multicast() || (o[0] == 100 && (64..128).contains(&o[1])) || o[0] == 0)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            if let Some(v4) = a.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

fn connect(host: &str, port: u16, deadline: Instant) -> Result<TcpStream, String> {
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|_| "the host name did not resolve".to_string())?.collect();
    let addr = addrs.iter().find(|a| public_ip(a.ip())).ok_or("the host is not on a public address")?;
    let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(1));
    let s = TcpStream::connect_timeout(addr, left).map_err(|e| format!("cannot connect: {}", e.kind()))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(15)));
    Ok(s)
}

fn tls_client(host: &str, ca: &[PathBuf]) -> Result<rustls::ClientConnection, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(crate::gitnet::roots(ca)?)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| "not a valid host name".to_string())?;
    rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| e.to_string())
}

type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

enum Conn {
    Plain(TcpStream),
    Secure(Box<Tls>),
    Gone,
}

struct RealMail {
    conn: Conn,
    host: String,
    ca: Vec<PathBuf>,
    buf: Vec<u8>,
}

impl MailStream for RealMail {
    fn read_line(&mut self) -> Result<String, String> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return Ok(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string());
            }
            if self.buf.len() > 4096 {
                return Err("the mail server sent a line that is too long".into());
            }
            let mut chunk = [0u8; 1024];
            let n = match &mut self.conn {
                Conn::Plain(s) => s.read(&mut chunk),
                Conn::Secure(s) => s.read(&mut chunk),
                Conn::Gone => return Err("closed".into()),
            }
            .map_err(|e| format!("reading from the mail server: {}", e.kind()))?;
            if n == 0 {
                return Err("the mail server closed the connection".into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
    fn write_all(&mut self, b: &[u8]) -> Result<(), String> {
        match &mut self.conn {
            Conn::Plain(s) => s.write_all(b).and_then(|_| s.flush()),
            Conn::Secure(s) => s.write_all(b).and_then(|_| s.flush()),
            Conn::Gone => return Err("closed".into()),
        }
        .map_err(|e| format!("writing to the mail server: {}", e.kind()))
    }
    fn start_tls(&mut self) -> Result<(), String> {
        match std::mem::replace(&mut self.conn, Conn::Gone) {
            Conn::Plain(s) => {
                self.buf.clear();
                self.conn = Conn::Secure(Box::new(rustls::StreamOwned::new(tls_client(&self.host, &self.ca)?, s)));
                Ok(())
            }
            other => {
                self.conn = other;
                Err("already encrypted".into())
            }
        }
    }
    fn is_tls(&self) -> bool {
        matches!(self.conn, Conn::Secure(_))
    }
}

/// The real thing, for the tenant on `dir`.
pub struct RealNet {
    pub dir: PathBuf,
    pub ca_files: Vec<PathBuf>,
}

impl RealNet {
    fn egress(&self, team: Option<&str>, host: &str) -> Result<(), String> {
        match crate::tenantconfig::enforce::egress_allows(&self.dir, team, host) {
            Some(true) => Ok(()),
            Some(false) => Err(format!("the tenant's egress policy does not allow {host}")),
            None => Err(format!("the tenant has no egress policy, so nothing may be sent to {host}")),
        }
    }
}

impl Net for RealNet {
    fn post(&self, team: Option<&str>, req: &HttpReq) -> Result<u16, String> {
        let host = super::webhook::https_host(&req.url)?;
        self.egress(team, &host)?;
        let rest = &req.url["https://".len()..];
        let (authority, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(a, p)| (a, format!("/{p}")));
        let port = authority.rsplit_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(443);
        let deadline = Instant::now() + Duration::from_secs(req.timeout_secs.max(1));
        let sock = connect(&host, port, deadline)?;
        let mut tls = rustls::StreamOwned::new(tls_client(&host, &self.ca_files)?, sock);
        let mut head = format!("POST {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nUser-Agent: iyakki-notify\r\nContent-Length: {}\r\n", req.body.len());
        for (k, v) in &req.headers {
            if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
                return Err("a header holds a line break".into());
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        tls.write_all(head.as_bytes()).and_then(|_| tls.write_all(&req.body)).and_then(|_| tls.flush()).map_err(|e| format!("sending: {}", e.kind()))?;
        // only the status line is read; the rest of the answer is never kept
        let mut first = [0u8; 64];
        let (mut n, t0) = (0, Instant::now());
        while n < 12 && t0.elapsed() < Duration::from_secs(req.timeout_secs.max(1)) {
            match tls.read(&mut first[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(_) => break,
            }
        }
        let line = String::from_utf8_lossy(&first[..n]).to_string();
        line.split(' ').nth(1).and_then(|c| c.get(..3)).and_then(|c| c.parse().ok()).ok_or_else(|| "no HTTP answer".to_string())
    }

    fn send_mail(&self, team: Option<&str>, j: &MailJob) -> Result<(), String> {
        self.egress(team, &j.host)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let sock = connect(&j.host, j.port, deadline)?;
        let conn = match j.mode {
            TlsMode::Implicit => Conn::Secure(Box::new(rustls::StreamOwned::new(tls_client(&j.host, &self.ca_files)?, sock))),
            TlsMode::Starttls => Conn::Plain(sock),
        };
        let mut m = RealMail { conn, host: j.host.clone(), ca: self.ca_files.clone(), buf: Vec::new() };
        let auth = match (&j.user, &j.pass) {
            (Some(u), Some(p)) => Some(Auth { user: u, pass: p }),
            _ => None,
        };
        smtp::send(&mut m, j.mode, auth.as_ref(), &Mail { from: &j.from, to: &j.to, subject: &j.subject, body: &j.body, ehlo_name: "iyakki.invalid", now: j.now, id: &j.id })
    }
}
