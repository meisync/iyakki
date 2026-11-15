//! Mutual TLS between the domain listener and the pod behind a route (F-CLOUD-49; docs/development/CLOUD-MULTITENANT-PLAN.md, 3c).
//!
//! The link from a client to the listener is `cloudlisten_mtls.rs`. This file is the other link: listener to pod. Both ends change.
//!
//! **One CA per route.** The control side ([`provision`]) makes a CA for a route name and two leaf certificates from it: the pod's
//! (`server`, valid for the one name [`server_name`], not for any DNS name of the cluster) and the listener's (`client`). They live in the
//! vault as the secrets of [`SecretNames`]:
//!
//! | secret | holds | read by |
//! |---|---|---|
//! | `NAME-pod-ca` | the CA certificate(s) (PEM; two while a CA rotation overlaps) | listener and pod |
//! | `NAME-pod-client-chain`, `NAME-pod-client-key` | the listener's certificate and key | listener |
//! | `NAME-pod-server-chain`, `NAME-pod-server-key` | the pod's certificate and key | the pod (sidecar) |
//! | `NAME-pod-ca-key` | the CA's private key | the control side only; never the listener or a pod |
//!
//! **The listener.** A host in [`super::Options::pod_tls`] is dialled with TLS 1.2/1.3: the pod's certificate must chain to the route's CA
//! and be valid for [`server_name`] (not expired), and the listener presents its client certificate. There is NO fallback to plain HTTP:
//! a handshake that fails, a certificate that is not loaded, or an own certificate that has expired ends in 502 and not one byte of the
//! request is sent. The certificates are re-read with the other certificates (`Options::cert_reload`); a replacement that does not check
//! out leaves the last good one.
//!
//! **The pod.** [`PodServer`] is the pod side as a sidecar (`iyakki cloud pod-tls serve`): it speaks TLS only, REQUIRES a client
//! certificate of the route's CA, and only after the handshake opens the pod's own plain port on 127.0.0.1 and passes the bytes. A plain
//! HTTP client, a client with no certificate, a certificate of another CA or an expired one never reaches the application.
//!
//! **Limits.** A TLS-1.3 client learns that its certificate was refused only on its first read (that is TLS), so the listener sees the
//! refusal as "no valid response" and answers 502. A pod that closes without a TLS close_notify is an error to the listener (a cut-off
//! answer is not mistaken for a finished one). Revocation is rotation: there is no CRL; leaf certificates are short-lived (default 90 days).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned};

use super::vault::{Reader, SecretReader};
use super::{is_timeout, wait_readable, Inner};
use crate::cloudroute::{read_cert, RouteTable, Scheme};
use crate::cloudstorage::plain_name;

// ------------------------------------------------------------------------------------------------------------ names and material

/// The one DNS name the pod's certificate is valid for and the listener checks, whatever address the pod is reached on.
pub fn server_name(name: &str) -> String {
    format!("{name}.pod.iyakki.internal")
}

/// The vault secrets of one route's pod link.
pub struct SecretNames {
    pub ca: String,
    pub ca_key: String,
    pub client_chain: String,
    pub client_key: String,
    pub server_chain: String,
    pub server_key: String,
}

impl SecretNames {
    pub fn of(name: &str) -> SecretNames {
        let n = |s: &str| format!("{name}-pod-{s}");
        SecretNames { ca: n("ca"), ca_key: n("ca-key"), client_chain: n("client-chain"), client_key: n("client-key"), server_chain: n("server-chain"), server_key: n("server-key") }
    }
}

/// What one end needs: the CA(s) it trusts and its own certificate chain and key (PEM). Never printed.
#[derive(Clone)]
pub struct Material {
    pub ca: String,
    pub chain: String,
    pub key: String,
}

impl std::fmt::Debug for Material {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Material(..)")
    }
}

/// Reads the pod link's material for one end from a vault reader (`which`: `client` or `server`).
pub fn read_material(r: &dyn SecretReader, name: &str, which: &str) -> Result<Material, String> {
    if !plain_name(name) {
        return Err(format!("\"{name}\" is not a vault name"));
    }
    let n = SecretNames::of(name);
    let (chain, key) = if which == "client" { (&n.client_chain, &n.client_key) } else { (&n.server_chain, &n.server_key) };
    let get = |s: &str| -> Result<String, String> {
        let b = r.read(s).map_err(|e| format!("vault secret \"{s}\": {e}"))?;
        String::from_utf8(b).map_err(|_| format!("vault secret \"{s}\" is not text (PEM expected)"))
    };
    Ok(Material { ca: get(&n.ca)?, chain: get(chain)?, key: get(key)? })
}

// ------------------------------------------------------------------------------------------------------------ the control side

/// A route's CA: the certificate and its private key (PEM).
#[derive(Clone)]
pub struct RouteCa {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leaf {
    Server,
    Client,
}

/// Year, month, day.
pub type Ymd = (i32, u8, u8);

/// Calendar date of a unix time (civil from days).
pub fn ymd(secs: i64) -> Ymd {
    let z = secs.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((yoe + era * 400 + i64::from(m <= 2)) as i32, m as u8, d as u8)
}

fn ca_params(name: &str, window: (Ymd, Ymd)) -> Result<rcgen::CertificateParams, String> {
    let mut p = rcgen::CertificateParams::new(vec![]).map_err(|e| e.to_string())?;
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    p.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    p.distinguished_name.push(rcgen::DnType::CommonName, format!("IYAKKI pod CA {name}"));
    p.not_before = rcgen::date_time_ymd(window.0 .0, window.0 .1, window.0 .2);
    p.not_after = rcgen::date_time_ymd(window.1 .0, window.1 .1, window.1 .2);
    Ok(p)
}

fn window(now: i64, days: u32) -> (Ymd, Ymd) {
    (ymd(now - 86400), ymd(now + i64::from(days.max(1)) * 86400))
}

/// A new CA for the route `name`, valid for `days`.
pub fn create_ca(name: &str, now: i64, days: u32) -> Result<RouteCa, String> {
    if !plain_name(name) {
        return Err(format!("\"{name}\" is not a name"));
    }
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = ca_params(name, window(now, days))?.self_signed(&key).map_err(|e| e.to_string())?;
    Ok(RouteCa { cert_pem: cert.pem(), key_pem: key.serialize_pem() })
}

/// A leaf certificate and key (PEM) from the CA, valid between two dates, for the DNS name `dns` (a server leaf). The dates are what the
/// certificate format can say; the tests use this to make an expired one and a pod for another name.
pub fn issue_for(ca: &RouteCa, name: &str, leaf: Leaf, dns: &str, from: Ymd, to: Ymd) -> Result<(String, String), String> {
    let ca_key = rcgen::KeyPair::from_pem(&ca.key_pem).map_err(|_| "the CA key is not usable".to_string())?;
    // the same subject and key give the same issuer name and signature check as the stored CA certificate
    let issuer = ca_params(name, window(0, 1))?.self_signed(&ca_key).map_err(|e| e.to_string())?;
    let mut p = rcgen::CertificateParams::new(if leaf == Leaf::Server { vec![dns.to_string()] } else { vec![] }).map_err(|e| e.to_string())?;
    p.distinguished_name.push(rcgen::DnType::CommonName, if leaf == Leaf::Server { dns.to_string() } else { format!("IYAKKI listener {name}") });
    p.extended_key_usages = vec![if leaf == Leaf::Server { rcgen::ExtendedKeyUsagePurpose::ServerAuth } else { rcgen::ExtendedKeyUsagePurpose::ClientAuth }];
    p.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    p.not_before = rcgen::date_time_ymd(from.0, from.1, from.2);
    p.not_after = rcgen::date_time_ymd(to.0, to.1, to.2);
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = p.signed_by(&key, &issuer, &ca_key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// A leaf certificate and key (PEM) valid for `days` from `now`.
pub fn issue(ca: &RouteCa, name: &str, leaf: Leaf, now: i64, days: u32) -> Result<(String, String), String> {
    let w = window(now, days);
    issue_for(ca, name, leaf, &server_name(name), w.0, w.1)
}

/// Where the control side puts secrets (the vault; tests use memory). The error never holds the value.
pub trait SecretWriter {
    fn write(&self, name: &str, value: &[u8]) -> Result<(), String>;
}

impl SecretWriter for Reader {
    fn write(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.client.put(&self.scope, name, value)
    }
}

pub const CA_DAYS: u32 = 3650;
pub const LEAF_DAYS: u32 = 90;

fn put(w: &dyn SecretWriter, name: &str, v: &str) -> Result<(), String> {
    w.write(name, v.as_bytes()).map_err(|e| format!("could not write the secret \"{name}\": {e}"))
}

fn leaves(w: &dyn SecretWriter, ca: &RouteCa, name: &str, now: i64, days: u32) -> Result<(), String> {
    let n = SecretNames::of(name);
    let (c, k) = issue(ca, name, Leaf::Client, now, days)?;
    put(w, &n.client_chain, &c)?;
    put(w, &n.client_key, &k)?;
    let (c, k) = issue(ca, name, Leaf::Server, now, days)?;
    put(w, &n.server_chain, &c)?;
    put(w, &n.server_key, &k)
}

/// Make the pod link of the route `name`: a CA and both leaves, written to the vault. Returns lines for the person (never a secret).
pub fn provision(w: &dyn SecretWriter, name: &str, now: i64, leaf_days: u32) -> Result<Vec<String>, String> {
    let ca = create_ca(name, now, CA_DAYS)?;
    let n = SecretNames::of(name);
    put(w, &n.ca_key, &ca.key_pem)?;
    leaves(w, &ca, name, now, leaf_days)?;
    put(w, &n.ca, &ca.cert_pem)?;
    Ok(vec![format!("pod link {name}: CA made ({CA_DAYS} days), listener and pod certificates issued ({leaf_days} days); the pod is named {}", server_name(name))])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rotate {
    /// New listener and pod certificates from the same CA.
    Leaves,
    /// A new CA and new leaves; the old CA stays trusted (both in `NAME-pod-ca`) until [`Rotate::DropOldCa`], so ends that still hold an
    /// old certificate keep working while every end reloads.
    CaOverlap,
    /// Trust only the current CA again (after every end holds a certificate of the new one).
    DropOldCa,
}

/// Rotate the pod link. Needs the CA key (from `r`), which only the control side can read.
pub fn rotate(w: &dyn SecretWriter, r: &dyn SecretReader, name: &str, what: Rotate, now: i64, leaf_days: u32) -> Result<Vec<String>, String> {
    if !plain_name(name) {
        return Err(format!("\"{name}\" is not a name"));
    }
    let n = SecretNames::of(name);
    let text = |s: &str| -> Result<String, String> {
        String::from_utf8(r.read(s).map_err(|e| format!("vault secret \"{s}\": {e}"))?).map_err(|_| format!("vault secret \"{s}\" is not text"))
    };
    match what {
        Rotate::Leaves => {
            let ca = RouteCa { cert_pem: text(&n.ca)?, key_pem: text(&n.ca_key)? };
            leaves(w, &ca, name, now, leaf_days)?;
            Ok(vec![format!("pod link {name}: new listener and pod certificates ({leaf_days} days); the CA is unchanged")])
        }
        Rotate::CaOverlap => {
            let old = text(&n.ca)?;
            let ca = create_ca(name, now, CA_DAYS)?;
            put(w, &n.ca, &format!("{}{old}", ca.cert_pem))?;
            put(w, &n.ca_key, &ca.key_pem)?;
            leaves(w, &ca, name, now, leaf_days)?;
            Ok(vec![format!("pod link {name}: new CA and certificates; the old CA is still trusted. When the listener and the pod have reloaded, run the rotation again with drop-old to stop trusting it")])
        }
        Rotate::DropOldCa => {
            // the current CA is the first certificate of the bundle (CaOverlap puts it first)
            const END: &str = "-----END CERTIFICATE-----";
            let bundle = text(&n.ca)?;
            let cut = bundle.find(END).ok_or("no CA certificate in the bundle")? + END.len();
            put(w, &n.ca, &format!("{}\n", bundle[..cut].trim_start()))?;
            Ok(vec![format!("pod link {name}: only the current CA is trusted now")])
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ checks shared by both ends

fn parse_roots(ca: &str) -> Result<RootCertStore, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(ca.as_bytes()).collect::<Result<_, _>>().map_err(|_| "the pod CA is not PEM".to_string())?;
    if certs.is_empty() {
        return Err("no certificate in the pod CA".into());
    }
    let mut store = RootCertStore::empty();
    for c in certs {
        store.add(c).map_err(|_| "a certificate of the pod CA cannot be used as a trust anchor".to_string())?;
    }
    Ok(store)
}

/// The leaf chain and key, checked: they belong together and the leaf is valid at `now`. Returns the chain, key and the leaf's end time.
fn parse_own(chain: &str, key: &str, now: i64) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>, i64), String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain.as_bytes()).collect::<Result<_, _>>().map_err(|_| "the certificate is not PEM".to_string())?;
    let first = certs.first().ok_or("no certificate found (expected -----BEGIN CERTIFICATE-----)")?;
    let k = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|_| "the private key is not PEM".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&k).map_err(|_| "the private key type is not supported".to_string())?;
    rustls::sign::CertifiedKey::new(certs.clone(), signer).keys_match().map_err(|_| "the private key does not belong to this certificate".to_string())?;
    let info = read_cert(first.as_ref()).ok_or("the certificate could not be read")?;
    if now < info.not_before {
        return Err("the certificate is not valid yet".into());
    }
    if now >= info.not_after {
        return Err("the certificate has expired".into());
    }
    let end = info.not_after;
    Ok((certs, k, end))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn other(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, m.into())
}

// ------------------------------------------------------------------------------------------------------------ the listener's end

pub(super) struct PodClient {
    cfg: Arc<ClientConfig>,
    name: ServerName<'static>,
    not_after: i64,
}

#[derive(Default)]
pub(super) struct PodState(RwLock<HashMap<String, Arc<PodClient>>>);

fn client_for(m: &Material, route_name: &str, now: i64) -> Result<PodClient, String> {
    let roots = parse_roots(&m.ca)?;
    let (chain, key, not_after) = parse_own(&m.chain, &m.key, now)?;
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_client_auth_cert(chain, key)
        .map_err(|_| "the listener's certificate and key cannot be used".to_string())?;
    let name = ServerName::try_from(server_name(route_name)).map_err(|_| "the pod name is not a DNS name".to_string())?;
    Ok(PodClient { cfg: Arc::new(cfg), name, not_after })
}

/// The hosts that need TLS to the pod and the certificate name of each: `Options::pod_tls`, for hosts the table still has as a plain or
/// TLS-ending route.
pub(super) fn wanted(inner: &Inner, t: &RouteTable) -> BTreeMap<String, String> {
    inner.opts.pod_tls.iter().filter(|(h, _)| t.routes.iter().any(|r| r.scheme != Scheme::Passthrough && r.host.eq_ignore_ascii_case(h))).map(|(h, n)| (h.to_ascii_lowercase(), n.clone())).collect()
}

/// Load the listener's certificates for all pod-TLS hosts again. A failure keeps the previous good one (while it is valid); a host that
/// never had one is not served (502). One line per host; no line holds a secret.
pub(super) fn reload(inner: &Inner, t: &RouteTable) -> Vec<String> {
    let at = super::now();
    let old = inner.pod.0.read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut fresh: HashMap<String, Arc<PodClient>> = HashMap::new();
    let mut lines = Vec::new();
    for (host, name) in wanted(inner, t) {
        match inner.source.load_pod_client(&name).and_then(|m| client_for(&m, &name, at)) {
            Ok(c) => {
                lines.push(format!("pod TLS {host}: client certificate valid for {} more day(s)", (c.not_after - at) / 86400));
                fresh.insert(host, Arc::new(c));
            }
            Err(e) => match old.get(&host).filter(|o| o.not_after > at) {
                Some(o) => {
                    lines.push(format!("pod TLS {host}: new certificate refused ({e}); the previous one stays"));
                    fresh.insert(host, o.clone());
                }
                None => lines.push(format!("pod TLS {host}: no usable certificate ({e}); requests to this route get 502 and are not sent in plain")),
            },
        }
    }
    *inner.pod.0.write().unwrap_or_else(|e| e.into_inner()) = fresh;
    lines
}

/// The connection to a pod: plain TCP, or TLS over it. Clones share one TLS session (the request is written before the response is read).
pub(super) struct Upstream {
    tcp: TcpStream,
    tls: Option<Arc<Mutex<StreamOwned<ClientConnection, TcpStream>>>>,
}

impl Upstream {
    pub(super) fn plain(tcp: TcpStream) -> Upstream {
        Upstream { tcp, tls: None }
    }
    pub(super) fn try_clone(&self) -> io::Result<Upstream> {
        Ok(Upstream { tcp: self.tcp.try_clone()?, tls: self.tls.clone() })
    }
    pub(super) fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.tcp.set_read_timeout(d)
    }
    pub(super) fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.tcp.set_write_timeout(d)
    }
    pub(super) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.tcp.shutdown(how)
    }
    pub(super) fn raw_fd(&self) -> std::os::fd::RawFd {
        self.tcp.as_raw_fd()
    }
    pub(super) fn is_tls(&self) -> bool {
        self.tls.is_some()
    }
}

impl Read for Upstream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &self.tls {
            Some(t) => t.lock().unwrap_or_else(|e| e.into_inner()).read(buf),
            None => (&self.tcp).read(buf),
        }
    }
}

impl Write for Upstream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &self.tls {
            Some(t) => t.lock().unwrap_or_else(|e| e.into_inner()).write(buf),
            None => (&self.tcp).write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match &self.tls {
            Some(t) => t.lock().unwrap_or_else(|e| e.into_inner()).flush(),
            None => (&self.tcp).flush(),
        }
    }
}

/// Start TLS to the pod on a connected socket and finish the handshake. Nothing of the request has been written.
pub(super) fn connect(inner: &Inner, host: &str, mut tcp: TcpStream) -> io::Result<Upstream> {
    let c = inner.pod.0.read().unwrap_or_else(|e| e.into_inner()).get(&host.to_ascii_lowercase()).cloned();
    let Some(c) = c else { return Err(other("no certificate for the link to the pod is loaded; the request is not sent in plain")) };
    if super::now() >= c.not_after {
        return Err(other("the listener's certificate for the pod has expired; the request is not sent"));
    }
    let mut conn = ClientConnection::new(c.cfg.clone(), c.name.clone()).map_err(|e| other(format!("TLS to the pod: {e}")))?;
    tcp.set_read_timeout(Some(inner.opts.connect_timeout))?;
    tcp.set_write_timeout(Some(inner.opts.connect_timeout))?;
    let started = Instant::now();
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(|e| other(format!("TLS to the pod failed: {e}")))?;
        if started.elapsed() > inner.opts.connect_timeout * 2 {
            return Err(other("TLS to the pod: the handshake took too long"));
        }
    }
    let twin = tcp.try_clone()?;
    Ok(Upstream { tcp, tls: Some(Arc::new(Mutex::new(StreamOwned::new(conn, twin)))) })
}

// ------------------------------------------------------------------------------------------------------------ the pod's end

/// The pod side's TLS configuration: its certificate, and a client certificate of the route's CA REQUIRED.
pub fn server_config(m: &Material, now: i64) -> Result<Arc<ServerConfig>, String> {
    let roots = parse_roots(&m.ca)?;
    let (chain, key, _) = parse_own(&m.chain, &m.key, now)?;
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider()).build().map_err(|e| format!("the pod CA is not usable: {e}"))?;
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .map_err(|_| "the pod's certificate and key cannot be used".to_string())?;
    Ok(Arc::new(cfg))
}

pub struct PodOptions {
    /// The application's plain port on 127.0.0.1.
    pub backend_port: u16,
    pub max_conns: usize,
    pub handshake_timeout: Duration,
    /// A connection that moves no byte for this long is closed.
    pub idle_timeout: Duration,
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
}

impl PodOptions {
    pub fn new(backend_port: u16) -> PodOptions {
        PodOptions { backend_port, max_conns: 128, handshake_timeout: Duration::from_secs(10), idle_timeout: Duration::from_secs(300), log: Arc::new(|l| crate::qeprintln!("pod tls: {l}")) }
    }
}

/// The pod side: accepts TLS with a required client certificate and passes the bytes to the application's plain port.
pub struct PodServer {
    addr: SocketAddr,
    cfg: Arc<RwLock<Arc<ServerConfig>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PodServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PodServer({})", self.addr)
    }
}

impl PodServer {
    pub fn start(listen: SocketAddr, m: &Material, o: PodOptions) -> Result<PodServer, String> {
        let cfg = Arc::new(RwLock::new(server_config(m, super::now())?));
        let lis = TcpListener::bind(listen).map_err(|e| format!("cannot listen on {listen}: {e}"))?;
        let addr = lis.local_addr().map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (cfg.clone(), stop.clone());
        let o = Arc::new(o);
        let active = Arc::new(AtomicUsize::new(0));
        let thread = std::thread::Builder::new()
            .name("pod-tls-accept".into())
            .spawn(move || {
                for c in lis.incoming() {
                    if s2.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(tcp) = c else { continue };
                    // a socket from a listener may inherit its mode on some systems: ours blocks
                    let _ = tcp.set_nonblocking(false);
                    if active.fetch_add(1, Ordering::AcqRel) >= o.max_conns {
                        active.fetch_sub(1, Ordering::AcqRel);
                        (o.log)("too many open connections; one closed");
                        continue;
                    }
                    let (cfg, o2, a2) = (c2.read().unwrap_or_else(|e| e.into_inner()).clone(), o.clone(), active.clone());
                    let spawned = std::thread::Builder::new().name("pod-tls-conn".into()).spawn(move || {
                        serve_conn(tcp, cfg, &o2);
                        a2.fetch_sub(1, Ordering::AcqRel);
                    });
                    if spawned.is_err() {
                        active.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(PodServer { addr, cfg, stop, thread: Some(thread) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// New certificate or CA for new connections. A replacement that does not check out changes nothing.
    pub fn replace(&self, m: &Material) -> Result<(), String> {
        let c = server_config(m, super::now())?;
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = c;
        Ok(())
    }
}

impl Drop for PodServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve_conn(mut tcp: TcpStream, cfg: Arc<ServerConfig>, o: &PodOptions) {
    let peer = tcp.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(o.handshake_timeout));
    let _ = tcp.set_write_timeout(Some(o.handshake_timeout));
    let mut conn = match ServerConnection::new(cfg) {
        Ok(c) => c,
        Err(e) => return (o.log)(&format!("{peer}: TLS setup failed: {e}")),
    };
    let started = Instant::now();
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut tcp) {
            return (o.log)(&format!("{peer}: refused, nothing passed on: {e}"));
        }
        if started.elapsed() > o.handshake_timeout {
            return (o.log)(&format!("{peer}: refused, the handshake took too long"));
        }
    }
    if conn.peer_certificates().map_or(true, |c| c.is_empty()) {
        return (o.log)(&format!("{peer}: refused, no client certificate"));
    }
    let back = match TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], o.backend_port)), Duration::from_secs(3)) {
        Ok(b) => b,
        Err(e) => return (o.log)(&format!("{peer}: the application on port {} did not answer: {e}", o.backend_port)),
    };
    let twin = match tcp.try_clone() {
        Ok(t) => t,
        Err(_) => return,
    };
    bridge(StreamOwned::new(conn, twin), tcp, back, o.idle_timeout);
}

/// Bytes both ways until either side ends (one thread: the TLS side cannot be split; both sockets are read with a 1 ms timeout and the
/// thread sleeps in poll(2) between passes).
fn bridge(mut tls: StreamOwned<ServerConnection, TcpStream>, tcp: TcpStream, mut back: TcpStream, idle: Duration) {
    let _ = tcp.set_read_timeout(Some(Duration::from_millis(1)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
    let _ = back.set_read_timeout(Some(Duration::from_millis(1)));
    let _ = back.set_write_timeout(Some(Duration::from_secs(30)));
    let mut buf = vec![0u8; 16 * 1024];
    let (mut last, mut moved) = (Instant::now(), true);
    while last.elapsed() < idle {
        if !moved && !wait_readable(&[tcp.as_raw_fd(), back.as_raw_fd()], 1000) {
            std::thread::sleep(Duration::from_millis(10));
        }
        moved = false;
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if back.write_all(&buf[..n]).is_err() {
                    break;
                }
                moved = true;
            }
            Err(e) if is_timeout(&e) => {}
            Err(_) => break,
        }
        match back.read(&mut buf) {
            Ok(0) => {
                tls.conn.send_close_notify();
                let _ = tls.flush();
                break;
            }
            Ok(n) => {
                if tls.write_all(&buf[..n]).and_then(|_| tls.flush()).is_err() {
                    break;
                }
                moved = true;
            }
            Err(e) if is_timeout(&e) => {}
            Err(_) => break,
        }
        if moved {
            last = Instant::now();
        }
    }
    let _ = tcp.shutdown(Shutdown::Both);
    let _ = back.shutdown(Shutdown::Both);
}

// ------------------------------------------------------------------------------------------------------------ the command

pub const USAGE: &str = "iyakki cloud pod-tls create --name NAME [--leaf-days N]   (a CA for the route's pod link and both certificates, into the vault)\n  \
    iyakki cloud pod-tls rotate --name NAME [--what leaves|ca|drop-old] [--leaf-days N]   (leaves: new certificates; ca: new CA, the old one still trusted; drop-old: trust only the new one)\n  \
    iyakki cloud pod-tls spec --name NAME --routes FILE --host HOST --image REF [--deployment D] [--tls-port P]   (the patches that add the sidecar to the Deployment and point the Service at it)\n  \
    iyakki cloud pod-tls secret --name NAME --namespace NS [--apply] [--kubeconfig F] [--context C]   (the pod's CA, certificate and key as a Secret, sent only to kubectl)\n  \
    iyakki cloud pod-tls serve --name NAME --listen ADDR:PORT --backend-port PORT   (the pod's end: TLS with a required client certificate in front of the plain port; run it beside the application)\n  \
    all three take the vault flags of `iyakki cloud listen` (--vault, --vault-fingerprint, --tenant, --vault-user, --vault-key, --vault-scope); `serve` also --cert-source file --cert-dir DIR (NAME.pod-ca, NAME.pod-server.crt, NAME.pod-server.key)";

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

/// `iyakki cloud pod-tls ...` (see [`USAGE`]).
pub fn cli(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    use super::setup::{self, Deps};
    let sub = a.first().map(String::as_str).unwrap_or("");
    let rest = if a.is_empty() { a } else { &a[1..] };
    let name = opt(rest, "--name").ok_or_else(|| format!("usage: {USAGE}"))?;
    let days = match opt(rest, "--leaf-days") {
        Some(d) => d.parse::<u32>().ok().filter(|d| (1..=825).contains(d)).ok_or("--leaf-days is a number from 1 to 825")?,
        None => LEAF_DAYS,
    };
    let now = super::now();
    let deps = Deps::real();
    let mut notes = Vec::new();
    match sub {
        "create" | "rotate" => {
            let (client, scope) = setup::vault_client(rest, &deps, &mut notes)?;
            let v = Reader { client, scope };
            let what = match (sub, opt(rest, "--what").as_deref()) {
                ("create", _) => None,
                (_, None | Some("leaves")) => Some(Rotate::Leaves),
                (_, Some("ca")) => Some(Rotate::CaOverlap),
                (_, Some("drop-old")) => Some(Rotate::DropOldCa),
                (_, Some(o)) => return Err(format!("--what \"{o}\" is not leaves, ca or drop-old")),
            };
            for n in notes {
                out(&n);
            }
            let lines = match what {
                None => provision(&v, &name, now, days)?,
                Some(w) => rotate(&v, &v, &name, w, now, days)?,
            };
            lines.iter().for_each(|l| out(l));
            Ok(())
        }
        "spec" => {
            use super::podspec;
            let file = opt(rest, "--routes").ok_or("--routes FILE is needed")?;
            let host = opt(rest, "--host").ok_or("--host HOST is needed (the route that has --pod-tls)")?;
            let image = opt(rest, "--image").ok_or("--image REF is needed (the IYAKKI image that runs the sidecar)")?;
            let table = crate::cloudroute::RouteTable::parse(&std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?)?;
            let (service, port) = podspec::service_of(&table, &host)?;
            let tls_port = match opt(rest, "--tls-port") {
                Some(p) => p.parse().ok().filter(|p| *p != 0).ok_or("--tls-port is a port")?,
                None => podspec::DEFAULT_TLS_PORT,
            };
            let o = podspec::SpecOpts { name: &name, image: &image, tls_port, backend_port: port };
            out(&podspec::deployment_patch(&opt(rest, "--deployment").unwrap_or_else(|| service.clone()), &o)?);
            out("---");
            out(&podspec::service_patch(&service, port, &o)?);
            out(&format!("# The application must listen on 127.0.0.1:{port} only. The Secret {} comes from `iyakki cloud pod-tls secret --name {name} --namespace NS --apply`.", podspec::secret_name(&name)));
            Ok(())
        }
        "secret" => {
            use super::podspec;
            let ns = opt(rest, "--namespace").ok_or("--namespace NS is needed")?;
            let (client, scope) = setup::vault_client(rest, &deps, &mut notes)?;
            let m = read_material(&Reader { client, scope }, &name, "server")?;
            let yaml = podspec::secret_manifest(&ns, &name, &m)?;
            if !rest.iter().any(|x| x == "--apply") {
                out(&format!("The Secret {} in {ns} would hold the pod's CA, certificate and key for the link {name}. Nothing was applied (the key is never printed); run again with --apply to send it to kubectl.", podspec::secret_name(&name)));
                return Ok(());
            }
            let mut args: Vec<String> = Vec::new();
            for (flag, key) in [("--kubeconfig", "--kubeconfig"), ("--context", "--context")] {
                if let Some(v) = opt(rest, flag) {
                    args.push(format!("{key}={v}"));
                }
            }
            args.extend(["apply".into(), "-f".into(), "-".into()]);
            out(crate::cloudcli::kubectl_apply(&args, yaml.as_bytes())?.trim_end());
            Ok(())
        }
        "serve" => {
            let listen: SocketAddr = opt(rest, "--listen").and_then(|l| l.parse().ok()).ok_or("--listen ADDR:PORT is needed")?;
            let port: u16 = opt(rest, "--backend-port").and_then(|p| p.parse().ok()).filter(|p| *p != 0).ok_or("--backend-port PORT is needed")?;
            let source = setup::source(rest, &deps, &mut notes)?;
            notes.iter().for_each(|n| out(n));
            let server = PodServer::start(listen, &source.load_pod_server(&name)?, PodOptions::new(port))?;
            out(&format!("pod TLS on {} -> 127.0.0.1:{port}; a client certificate of the pod link {name} is required. Ctrl-C stops it.", server.addr()));
            loop {
                std::thread::sleep(Duration::from_secs(60));
                match source.load_pod_server(&name).and_then(|m| server.replace(&m)) {
                    Ok(()) => {}
                    Err(e) => out(&format!("new pod certificate not applied: {e}; the previous one stays")),
                }
            }
        }
        _ => Err(format!("usage: {USAGE}")),
    }
}

// ------------------------------------------------------------------------------------------------------------ for the chat tools

/// The tenant vault as the pod-link code needs it: read, write and list the names of one scope. Names only for `names`; never a value.
pub trait PodVault: SecretReader + SecretWriter {
    fn names(&self) -> Result<Vec<String>, String>;
}

impl PodVault for Reader {
    fn names(&self) -> Result<Vec<String>, String> {
        self.client.list(&self.scope)
    }
}

/// The tenant-scope vault of the signed-in member `user` of the tenant whose volume is `dir` (the member's key from
/// `<dir>/cloud/vault-keys/`, the vault service from the pod's environment). `Err` reads as "the vault is locked".
pub fn tenant_vault(dir: &std::path::Path, user: &str) -> Result<Reader, String> {
    let crate::cloudvaultlink::Link::Target(t) = crate::cloudvaultlink::Link::from_env() else { return Err("the tenant vault is not linked to this computer".into()) };
    let keys = crate::cloudrotate::actor_keys(dir, user).ok_or("the tenant vault is locked: this person has no vault key here")?;
    Ok(Reader { client: t.client(user, keys)?, scope: crate::cloudvaultd::Scope::Tenant })
}

fn days_left(pem: &str, now: i64) -> String {
    match CertificateDer::pem_slice_iter(pem.as_bytes()).next().and_then(|r| r.ok()).and_then(|d| read_cert(d.as_ref())) {
        Some(i) if i.not_after > now => format!("valid for {} more day(s)", (i.not_after - now) / 86400),
        Some(_) => "EXPIRED".to_string(),
        None => "unreadable".to_string(),
    }
}

/// What is there for the pod link `name` (or for every link found when `name` is `None`): which secrets exist and how long the
/// certificates last. Reads only the certificates, never a key.
pub fn status(v: &dyn PodVault, name: Option<&str>, now: i64) -> Result<String, String> {
    let have = v.names()?;
    let links: Vec<String> = match name {
        Some(n) if plain_name(n) => vec![n.to_string()],
        Some(n) => return Err(format!("\"{n}\" is not a name")),
        None => have.iter().filter_map(|n| n.strip_suffix("-pod-ca")).map(String::from).collect(),
    };
    if links.is_empty() {
        return Ok("No pod link has been made yet (pod_tls_create makes one for a route).".into());
    }
    let mut out = Vec::new();
    for l in links {
        let n = SecretNames::of(&l);
        let has = |s: &String| have.contains(s);
        let missing: Vec<&str> = [(&n.ca, "CA"), (&n.ca_key, "CA key"), (&n.client_chain, "listener certificate"), (&n.client_key, "listener key"), (&n.server_chain, "pod certificate"), (&n.server_key, "pod key")].iter().filter(|(s, _)| !has(s)).map(|(_, w)| *w).collect();
        let cert = |s: &String| if has(s) { v.read(s).ok().and_then(|b| String::from_utf8(b).ok()).map_or("unreadable".to_string(), |p| days_left(&p, now)) } else { "missing".to_string() };
        out.push(format!("{l}: CA {}; listener certificate {}; pod certificate {}; the pod must be named {}{}", cert(&n.ca), cert(&n.client_chain), cert(&n.server_chain), server_name(&l), if missing.is_empty() { String::new() } else { format!("; missing: {}", missing.join(", ")) }));
    }
    Ok(out.join("\n"))
}
