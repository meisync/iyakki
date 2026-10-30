//! The developer computer's side: its identity (an ed25519 key made once, kept private), the devices it paired with (address and
//! the certificate fingerprint it pinned), and a small HTTPS client that signs every request. Used by `iyakki dev` and by the IDE's
//! "paired device" deploy target.
//!
//! TLS is rustls (already in the tree for the web front end). The device's certificate is self-signed, so instead of a certificate
//! authority the client pins the certificate's SHA-256: at pairing the fingerprint seen in the handshake goes into the pairing proof
//! (a machine in the middle cannot pair), and from then on any other certificate at that address is refused.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth;

// -------------------------------------------------------------------------------------------- identity and devices

/// Where the computer keeps its key and its devices: `$IYAKKI_DEV_HOME`, else `$XDG_CONFIG_HOME/iyakki/dev`, else
/// `~/.config/iyakki/dev`.
pub fn default_home() -> PathBuf {
    if let Some(h) = std::env::var_os("IYAKKI_DEV_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(h);
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x).join("iyakki").join("dev");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".config").join("iyakki").join("dev")
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// A short name of your choice (the device's host name by default).
    pub name: String,
    /// The address or the name (`iyakki-0a1b2c.local`) the device was paired at. A name is looked up on the local network (mDNS) when
    /// the saved address does not answer; the pinned fingerprint decides whether it is the same device.
    pub host: String,
    pub port: u16,
    /// The last address a name was found at (empty for a device paired by address).
    #[serde(default)]
    pub addr: String,
    /// The SHA-256 of the device's certificate, pinned at pairing.
    pub fingerprint: String,
    /// The device account this computer acts as.
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub paired_at: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct DevicesFile {
    /// Layout of `devices.json` (see `iyakki-schema`): 0 = alpha.1.
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    default: String,
    #[serde(default)]
    devices: Vec<Device>,
    /// Fields a newer release wrote: kept, and written back.
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

/// The paired devices of this computer (`devices.json`). Schema 1 stamps the alpha.1 layout.
pub const DEVICES_SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "paired devices", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the alpha.1 layout", run: |_| {} }] };

/// The computer's folder of developer mode.
#[derive(Clone, Debug)]
pub struct Home {
    dir: PathBuf,
}

impl Home {
    pub fn at(dir: &Path) -> Home {
        Home { dir: dir.to_path_buf() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The computer's key, made the first time (mode 0600).
    pub fn key(&self) -> Result<SigningKey, String> {
        let p = self.dir.join("id.key");
        if let Ok(t) = std::fs::read_to_string(&p) {
            return crate::pkg::signing_key_from_hex(t.trim()).map_err(|e| format!("{}: {e}", p.display()));
        }
        let k = crate::pkg::new_signing_key().map_err(|e| e.to_string())?;
        super::write_private(&p, crate::pkg::signing_key_hex(&k).as_bytes())?;
        super::write_private(&self.dir.join("id.pub"), format!("{}\n", crate::pkg::public_key_hex(&k)).as_bytes())?;
        Ok(k)
    }

    /// The computer's public key, if it has one already.
    pub fn public_key(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("id.key")).ok().and_then(|t| crate::pkg::signing_key_from_hex(t.trim()).ok()).map(|k| crate::pkg::public_key_hex(&k))
    }

    fn file(&self) -> DevicesFile {
        iyakki_schema::load_typed::<DevicesFile>(&DEVICES_SCHEMA, &self.dir.join("devices.json"), iyakki_schema::now_secs()).1
    }

    fn save(&self, f: &DevicesFile) -> Result<(), String> {
        iyakki_schema::save_typed(&DEVICES_SCHEMA, &self.dir.join("devices.json"), f, 0o600)
    }

    pub fn devices(&self) -> Vec<Device> {
        self.file().devices
    }

    pub fn default_name(&self) -> String {
        let f = self.file();
        if f.default.is_empty() { f.devices.first().map(|d| d.name.clone()).unwrap_or_default() } else { f.default }
    }

    /// Keep a paired device (replacing one of the same name); the first one becomes the default.
    pub fn add(&self, d: Device) -> Result<(), String> {
        let mut f = self.file();
        f.devices.retain(|x| x.name != d.name);
        if f.default.is_empty() {
            f.default = d.name.clone();
        }
        f.devices.push(d);
        self.save(&f)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut f = self.file();
        let n = f.devices.len();
        f.devices.retain(|d| d.name != name);
        if f.devices.len() == n {
            return Err(format!("no paired device called {name}"));
        }
        if f.default == name {
            f.default = f.devices.first().map(|d| d.name.clone()).unwrap_or_default();
        }
        self.save(&f)
    }

    /// Remember the address a named device was last reached at.
    pub fn set_addr(&self, name: &str, addr: &str) -> Result<(), String> {
        let mut f = self.file();
        let d = f.devices.iter_mut().find(|d| d.name == name).ok_or_else(|| format!("no paired device called {name}"))?;
        d.addr = addr.to_string();
        self.save(&f)
    }

    pub fn set_default(&self, name: &str) -> Result<(), String> {
        let mut f = self.file();
        if !f.devices.iter().any(|d| d.name == name) {
            return Err(format!("no paired device called {name}"));
        }
        f.default = name.into();
        self.save(&f)
    }

    /// The device called `name`, or the default one.
    pub fn device(&self, name: Option<&str>) -> Result<Device, String> {
        let want = name.map(str::to_string).unwrap_or_else(|| self.default_name());
        if want.is_empty() {
            return Err("no device is paired yet: run `iyakki dev pair <address>` first".into());
        }
        self.devices().into_iter().find(|d| d.name == want).ok_or_else(|| format!("no paired device called {want} (see `iyakki dev devices`)"))
    }
}

/// `host`, `host:port` or `[v6]:port`.
pub fn split_host(s: &str, default_port: u16) -> Result<(String, u16), String> {
    let s = s.trim().trim_start_matches("https://").trim_end_matches('/');
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
        return Err("give the device's address, for example 192.168.1.20 or iyakki.local:7979".into());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (h, p) = rest.split_once(']').ok_or("bad IPv6 address")?;
        let port = match p.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| "bad port")?,
            None => default_port,
        };
        return Ok((h.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Ok((h.to_string(), p.parse().map_err(|_| format!("bad port {p:?}"))?)),
        Some(_) => Ok((s.to_string(), default_port)),
        None => Ok((s.to_string(), default_port)),
    }
}

// -------------------------------------------------------------------------------------------- finding a device by its name

/// How long the name lookup waits for an answer.
pub const RESOLVE_WAIT: Duration = Duration::from_secs(2);

/// The addresses of `name.local` asked on the local network by our own resolver (no system resolver is involved).
pub fn resolve_local(name: &str) -> Vec<String> {
    iyakki_netscan::mdns::net::resolve(name, &iyakki_netscan::mdns::net::Ask::new(RESOLVE_WAIT)).iter().map(|a| a.to_string()).collect()
}

/// Do `attempt` against the device's address. A device paired by name is tried at its saved address first; when that fails (no answer,
/// refused, or another device's certificate) the name is looked up again, every address found is tried, and the one that passes the
/// pinned-certificate check becomes the saved address (`note` says `device moved: old -> new`). The fingerprint stays the anchor: an
/// answer that points to the wrong machine is refused by `attempt` (the TLS check) and changes nothing.
pub fn reach<T>(d: &Device, home: Option<&Home>, resolver: &dyn Fn(&str) -> Vec<String>, attempt: &mut dyn FnMut(&str) -> Result<T, String>, note: &mut dyn FnMut(&str)) -> Result<T, String> {
    if !iyakki_netscan::mdns::is_local_name(&d.host) {
        return attempt(&d.host);
    }
    let first = if d.addr.is_empty() { None } else { Some(d.addr.clone()) };
    let mut last = match &first {
        Some(a) => match attempt(a) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        },
        None => String::new(),
    };
    let found = resolver(&d.host);
    if found.is_empty() {
        return Err(if last.is_empty() { format!("cannot find {} on the local network (is the device on, on the same network, and announcing its name?)", d.host) } else { format!("{last}; and {} did not answer on the local network", d.host) });
    }
    for ip in found.iter().filter(|ip| Some(*ip) != first.as_ref()) {
        match attempt(ip) {
            Ok(v) => {
                if let Some(old) = &first {
                    note(&format!("device moved: {old} -> {ip}"));
                } else {
                    note(&format!("{} is at {ip}", d.host));
                }
                if let Some(h) = home {
                    let _ = h.set_addr(&d.name, ip);
                }
                return Ok(v);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

// -------------------------------------------------------------------------------------------- TLS with a pinned certificate

#[derive(Debug)]
struct Pin {
    expect: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let fp = auth::fingerprint(end_entity.as_ref());
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(fp.clone());
        }
        match &self.expect {
            Some(e) if auth::norm_fp(e) != auth::norm_fp(&fp) => Err(rustls::Error::General(format!("the device's certificate changed (it is {fp}, the one paired was {e}): if the device was reinstalled, pair again"))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// An answer from the device.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// The fingerprint of the certificate the device showed.
    pub fingerprint: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The JSON body on success, the device's error sentence otherwise.
    pub fn ok_json(&self) -> Result<Value, String> {
        let v = self.json();
        if self.status == 200 {
            Ok(v)
        } else {
            Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the device answered {}", self.status)))
        }
    }
}

const MAX_REPLY: usize = 16 << 20;

/// One HTTPS request to `host:port` (the certificate pinned to `expect`, or anything when None, and the one seen is returned).
pub fn https(host: &str, port: u16, expect: Option<&str>, method: &str, target: &str, headers: &[(String, String)], body: &[u8], timeout: Duration) -> Result<Reply, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let seen = Arc::new(Mutex::new(None));
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin { expect: expect.map(str::to_string), seen: seen.clone(), provider }))
        .with_no_client_auth();
    let name = ServerName::try_from("iyakki-device").map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| e.to_string())?;
    let addr = (host, port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.next().ok_or_else(|| format!("{host}: no address"))?;
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| format!("cannot reach {host}:{port}: {e} (is developer mode on, and the port open?)"))?;
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let mut tls = rustls::StreamOwned::new(conn, sock);
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut out = req.into_bytes();
    out.extend_from_slice(body);
    let fp = || seen.lock().ok().and_then(|s| s.clone()).unwrap_or_default();
    tls.write_all(&out).and_then(|_| tls.flush()).map_err(|e| format!("sending to {host}:{port}: {e}"))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match tls.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_REPLY {
                    return Err("the device's answer is too large".into());
                }
                if complete(&buf) {
                    break;
                }
            }
            // a peer that closes without close_notify: fine once the whole answer is here
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("reading from {host}:{port}: {e}")),
        }
    }
    let (status, ct, body) = split_reply(&buf).ok_or("the device's answer was cut off")?;
    Ok(Reply { status, content_type: ct, body, fingerprint: fp() })
}

fn head_end(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse().ok()))
}

fn complete(b: &[u8]) -> bool {
    head_end(b).is_some_and(|h| content_length(&String::from_utf8_lossy(&b[..h])).is_some_and(|n| b.len() >= h + 4 + n))
}

fn split_reply(b: &[u8]) -> Option<(u16, String, Vec<u8>)> {
    let h = head_end(b)?;
    let head = String::from_utf8_lossy(&b[..h]).into_owned();
    let status = head.split(' ').nth(1)?.parse().ok()?;
    let ct = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
    let n = content_length(&head).unwrap_or(b.len() - h - 4);
    let body = b.get(h + 4..h + 4 + n)?.to_vec();
    Some((status, ct, body))
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// `path?k=v&...` with the values percent-encoded.
pub fn target(path: &str, query: &[(String, String)]) -> String {
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&"))
    }
}

// -------------------------------------------------------------------------------------------- pairing and signed requests

/// What a device says before pairing: its name, version, fingerprint and whether a pairing is open.
pub fn hello(host: &str, port: u16) -> Result<(String, Value), String> {
    let r = https(host, port, None, "GET", "/v1/dev/hello", &[], b"", Duration::from_secs(15))?;
    let v = r.ok_json()?;
    Ok((r.fingerprint, v))
}

/// Pair with the device at `host:port` using the code on its screen. `expect_fp`, if given, must be the fingerprint the device shows.
/// The device is kept in `home` (pinned) and returned.
pub fn pair(home: &Home, host: &str, port: u16, name: &str, code: &str, expect_fp: Option<&str>, device_name: Option<&str>) -> Result<Device, String> {
    if !super::valid_name(name) {
        return Err("the computer's name: letters, digits, - _ . (at most 40)".into());
    }
    let key = home.key()?;
    let pubhex = crate::pkg::public_key_hex(&key);
    // a name (`iyakki-0a1b2c.local`) is looked up now; the device keeps the name and the address it was found at
    let by_name = iyakki_netscan::mdns::is_local_name(host);
    let (name_host, host) = if by_name {
        let ip = resolve_local(host).into_iter().next().ok_or_else(|| format!("cannot find {host} on the local network (is the device on, on the same network, and announcing its name?)"))?;
        (Some(host.to_string()), ip)
    } else {
        (None, host.to_string())
    };
    let host = host.as_str();
    let (fp, info) = hello(host, port)?;
    if let Some(e) = expect_fp {
        if auth::norm_fp(e) != auth::norm_fp(&fp) {
            return Err(format!("the device at {host}:{port} shows the certificate {fp}, not {e}: nothing was paired"));
        }
    }
    let pk = auth::pair_key(code, &fp)?;
    let body = serde_json::json!({ "name": name, "key": pubhex, "proof": auth::pair_proof(&pk, &fp, &pubhex, name) }).to_string();
    let r = https(host, port, Some(&fp), "POST", "/v1/dev/pair", &[("Content-Type".into(), "application/json".into())], body.as_bytes(), Duration::from_secs(30))?;
    let v = r.ok_json()?;
    let answer = v["answer"].as_str().unwrap_or("");
    if !auth::same(answer.as_bytes(), auth::pair_answer(&pk, &fp, &pubhex).as_bytes()) {
        return Err("the device did not prove that it knows the code: nothing was saved (something may be in the middle)".into());
    }
    let dname = device_name.map(str::to_string).or_else(|| info["name"].as_str().map(str::to_string)).filter(|n| super::valid_name(n)).unwrap_or_else(|| name_host.clone().unwrap_or_else(|| host.to_string()).replace(':', "-"));
    let d = Device { name: dname, host: name_host.clone().unwrap_or_else(|| host.to_string()), addr: if by_name { host.to_string() } else { String::new() }, port, fingerprint: fp, account: v["account"].as_str().unwrap_or("").to_string(), paired_at: crate::clock::now_unix() };
    home.add(d.clone())?;
    Ok(d)
}

/// A paired device and the key to sign with.
#[derive(Clone)]
pub struct DevClient {
    pub device: Device,
    key: SigningKey,
    pub timeout: Duration,
    /// Where a moved device's new address is saved.
    home: Option<Home>,
}

impl DevClient {
    pub fn new(home: &Home, device: Device) -> Result<DevClient, String> {
        Ok(DevClient { device, key: home.key()?, timeout: Duration::from_secs(120), home: Some(home.clone()) })
    }

    pub fn with_key(device: Device, key: SigningKey) -> DevClient {
        DevClient { device, key, timeout: Duration::from_secs(120), home: None }
    }

    /// A signed request.
    pub fn request(&self, method: &str, path: &str, query: &[(String, String)], body: &[u8]) -> Result<Reply, String> {
        let d = &self.device;
        let mut h = auth::sign_headers(&self.key, method, path, query, body, &d.fingerprint, crate::clock::now_unix())?;
        h.push(("Content-Type".into(), "application/octet-stream".into()));
        reach(d, self.home.as_ref(), &resolve_local, &mut |host| https(host, d.port, Some(&d.fingerprint), method, &target(path, query), &h, body, self.timeout), &mut |n| eprintln!("{n}"))
    }

    /// A signed request whose answer is JSON (the device's error sentence on failure).
    pub fn call(&self, method: &str, path: &str, query: &[(&str, &str)], body: &[u8]) -> Result<Value, String> {
        let q: Vec<(String, String)> = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.request(method, path, &q, body)?.ok_json()
    }

    /// A signed request whose answer is raw bytes (file chunks, pictures).
    pub fn call_bytes(&self, method: &str, path: &str, query: &[(&str, &str)], body: &[u8]) -> Result<Vec<u8>, String> {
        let q: Vec<(String, String)> = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = self.request(method, path, &q, body)?;
        if r.status == 200 {
            Ok(r.body)
        } else {
            r.ok_json().and_then(|_| Err("unexpected answer".to_string()))
        }
    }

    /// Like [`DevClient::call`] but keeps the status and the JSON of a refusal (for the `have` of a resumable push).
    pub fn call_status(&self, method: &str, path: &str, query: &[(&str, &str)], body: &[u8]) -> Result<(u16, Value), String> {
        let q: Vec<(String, String)> = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = self.request(method, path, &q, body)?;
        Ok((r.status, r.json()))
    }

    pub fn whoami(&self) -> Result<Value, String> {
        self.call("GET", "/v1/dev/whoami", &[], b"")
    }

    pub fn install(&self, package: &[u8], shared: bool) -> Result<Value, String> {
        self.call("POST", "/v1/dev/install", if shared { &[("scope", "shared")] } else { &[] }, package)
    }

    pub fn uninstall(&self, id: &str) -> Result<Value, String> {
        self.call("POST", "/v1/dev/uninstall", &[("id", id)], b"")
    }

    pub fn list(&self, all: bool) -> Result<Value, String> {
        self.call("GET", "/v1/dev/list", if all { &[("all", "1")] } else { &[] }, b"")
    }

    pub fn logs(&self, id: &str, since: u64) -> Result<Value, String> {
        self.call("GET", "/v1/dev/logs", &[("id", id), ("since", &since.to_string())], b"")
    }

    pub fn open(&self, id: &str) -> Result<Value, String> {
        self.call("POST", "/v1/dev/open", &[("id", id)], b"")
    }

    /// A PNG of the device's screen.
    pub fn screenshot(&self) -> Result<Vec<u8>, String> {
        let r = self.request("GET", "/v1/dev/screenshot", &[], b"")?;
        if r.status == 200 && r.content_type.starts_with("image/png") {
            Ok(r.body)
        } else {
            r.ok_json().and_then(|_| Err("the device did not send a picture".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_split_into_host_and_port() {
        assert_eq!(split_host("192.168.1.20", 7979), Ok(("192.168.1.20".into(), 7979)));
        assert_eq!(split_host("box.local:8000", 7979), Ok(("box.local".into(), 8000)));
        assert_eq!(split_host("https://box.local:8000/", 7979), Ok(("box.local".into(), 8000)));
        assert_eq!(split_host("[::1]:9000", 7979), Ok(("::1".into(), 9000)));
        assert_eq!(split_host("::1", 7979), Ok(("::1".into(), 7979)));
        assert!(split_host("", 1).is_err() && split_host("a b", 1).is_err() && split_host("h:x", 1).is_err());
    }

    #[test]
    fn the_home_keeps_one_private_key_and_the_paired_devices_with_a_default() {
        let d = std::env::temp_dir().join(format!("iyakki-devhome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let h = Home::at(&d);
        assert!(h.public_key().is_none());
        let k = h.key().unwrap();
        assert_eq!(crate::pkg::public_key_hex(&h.key().unwrap()), crate::pkg::public_key_hex(&k), "made once");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("id.key")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(h.device(None).is_err());
        let dev = |n: &str| Device { name: n.into(), host: "127.0.0.1".into(), addr: String::new(), port: 1, fingerprint: "AA".into(), account: "root".into(), paired_at: 0 };
        h.add(dev("a")).unwrap();
        h.add(dev("b")).unwrap();
        assert_eq!(h.device(None).unwrap().name, "a", "the first is the default");
        h.set_default("b").unwrap();
        assert_eq!(h.device(None).unwrap().name, "b");
        h.remove("b").unwrap();
        assert_eq!(h.device(None).unwrap().name, "a");
        assert!(h.remove("zz").is_err() && h.set_default("zz").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn named(addr: &str) -> Device {
        Device { name: "mac".into(), host: "iyakki-0a1b2c.local".into(), addr: addr.into(), port: 7979, fingerprint: "AA:BB".into(), account: "admin".into(), paired_at: 0 }
    }

    /// The stand-in for TLS with a pin: only `good` shows the pinned certificate; `other` is another device that took the old address.
    fn tls(good: &'static str, other: &'static str) -> impl FnMut(&str) -> Result<String, String> {
        move |ip: &str| {
            if ip == good {
                Ok(format!("answered by {ip}"))
            } else if ip == other {
                Err("the device's certificate changed (it is 00:11, the one paired was AA:BB): if the device was reinstalled, pair again".into())
            } else {
                Err(format!("cannot reach {ip}:7979: timed out"))
            }
        }
    }

    #[test]
    fn a_device_that_moved_is_found_again_by_its_name_and_the_new_address_is_saved_and_said() {
        let dir = std::env::temp_dir().join(format!("iyakki-devmoved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::at(&dir);
        home.add(named("192.168.50.67")).unwrap();
        let asked = std::cell::Cell::new(0);
        let resolver = |n: &str| {
            asked.set(asked.get() + 1);
            assert_eq!(n, "iyakki-0a1b2c.local");
            vec!["192.168.50.66".to_string()]
        };
        let mut notes = Vec::new();
        let r = reach(&named("192.168.50.67"), Some(&home), &resolver, &mut tls("192.168.50.66", "none"), &mut |n| notes.push(n.to_string())).unwrap();
        assert_eq!(r, "answered by 192.168.50.66");
        assert_eq!(notes, vec!["device moved: 192.168.50.67 -> 192.168.50.66".to_string()]);
        assert_eq!(home.device(Some("mac")).unwrap().addr, "192.168.50.66", "saved");
        assert_eq!(home.device(Some("mac")).unwrap().host, "iyakki-0a1b2c.local", "the name stays");
        // the saved address works: no lookup at all, nothing said
        let mut notes = Vec::new();
        reach(&home.device(Some("mac")).unwrap(), Some(&home), &resolver, &mut tls("192.168.50.66", "none"), &mut |n| notes.push(n.to_string())).unwrap();
        assert_eq!((asked.get(), notes.len()), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_device_at_the_old_address_or_a_forged_answer_does_not_pass_the_pinned_certificate() {
        // the old address now belongs to another IYAKKI (its certificate differs): the name is looked up, and the real one is used
        let mut notes = Vec::new();
        let ok = reach(&named("192.168.50.67"), None, &|_| vec!["192.168.50.67".into(), "192.168.50.66".into()], &mut tls("192.168.50.66", "192.168.50.67"), &mut |n| notes.push(n.to_string())).unwrap();
        assert_eq!(ok, "answered by 192.168.50.66");
        assert_eq!(notes, vec!["device moved: 192.168.50.67 -> 192.168.50.66".to_string()]);
        // a spoofed answer that points only to a machine with another certificate: refused, nothing saved, the error says why
        let dir = std::env::temp_dir().join(format!("iyakki-devspoof-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::at(&dir);
        home.add(named("192.168.50.67")).unwrap();
        let mut notes = Vec::new();
        let e = reach(&named("192.168.50.67"), Some(&home), &|_| vec!["192.168.50.99".to_string()], &mut tls("192.168.50.66", "192.168.50.99"), &mut |n| notes.push(n.to_string())).unwrap_err();
        assert!(e.contains("certificate changed"), "{e}");
        assert!(notes.is_empty() && home.device(Some("mac")).unwrap().addr == "192.168.50.67", "the address is not replaced by a forged one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nobody_answering_and_devices_paired_by_address_are_handled() {
        let e = reach(&named("192.168.50.67"), None, &|_| vec![], &mut tls("x", "y"), &mut |_| {}).unwrap_err();
        assert!(e.contains("timed out") && e.contains("iyakki-0a1b2c.local did not answer"), "{e}");
        let e = reach(&named(""), None, &|_| vec![], &mut tls("x", "y"), &mut |_| {}).unwrap_err();
        assert!(e.contains("cannot find iyakki-0a1b2c.local on the local network"), "{e}");
        // a name that was never looked up gets its address from the first answer
        let mut notes = Vec::new();
        assert!(reach(&named(""), None, &|_| vec!["192.168.50.66".into()], &mut tls("192.168.50.66", "y"), &mut |n| notes.push(n.to_string())).is_ok());
        assert_eq!(notes, vec!["iyakki-0a1b2c.local is at 192.168.50.66".to_string()]);
        // an address is used as it is: no lookup, ever
        let mut d = named("");
        d.host = "192.168.50.66".into();
        assert!(reach(&d, None, &|_| panic!("no lookup for an address"), &mut tls("192.168.50.66", "y"), &mut |_| {}).is_ok());
        let mut d = named("");
        d.host = "box.example.com".into();
        assert!(reach(&d, None, &|_| panic!("no lookup for a DNS name"), &mut |h| Ok::<_, String>(h.to_string()), &mut |_| {}).is_ok());
    }

    #[test]
    fn an_old_devices_file_without_the_address_still_loads_and_the_name_is_kept() {
        let dir = std::env::temp_dir().join(format!("iyakki-devold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("devices.json"), r#"{"schema":1,"default":"a","devices":[{"name":"a","host":"192.168.1.2","port":7979,"fingerprint":"AA","account":"root","paired_at":1}]}"#).unwrap();
        let home = Home::at(&dir);
        assert_eq!(home.device(None).unwrap().addr, "");
        home.add(named("192.168.50.66")).unwrap();
        assert_eq!(Home::at(&dir).device(Some("mac")).unwrap().addr, "192.168.50.66");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reply_is_read_by_its_length_and_targets_are_encoded() {
        let b = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        assert!(complete(b) && !complete(&b[..b.len() - 1]));
        assert_eq!(split_reply(b), Some((200, "application/json".into(), b"{}".to_vec())));
        assert_eq!(target("/v1/dev/logs", &[("id".into(), "my app".into()), ("since".into(), "3".into())]), "/v1/dev/logs?id=my%20app&since=3");
    }
}
