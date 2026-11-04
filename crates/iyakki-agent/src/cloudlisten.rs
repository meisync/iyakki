//! The local domain listener: "listen and port-forward by domain" (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3c).
//!
//! An in-house reverse proxy that applies a [`RouteTable`] on this machine. Plain `std` sockets and threads, no async runtime, no proxy
//! library; TLS is rustls (already a dependency), the HTTP/1.1 and the TLS ClientHello readers are our own.
//!
//! **What it does**
//! * HTTP/1.1 only. Every request head is read (limit 16 KB, a deadline against slow clients, an idle limit between requests), the route is
//!   chosen with [`RouteTable::pick`] from the `Host` header, the path and the port the connection came in on, and the request goes to the
//!   target: `Target::Local { port }` is 127.0.0.1:port. Each request on a keep-alive connection is routed on its own. Bodies (Content-Length
//!   and chunked) are streamed, never held whole. Each request uses its own upstream connection (`Connection: close` towards the target).
//! * Headers: hop-by-hop headers (and the names a `Connection` header lists) are dropped, `X-Forwarded-For/-Proto/-Host` are set by this
//!   listener (what the client sent under those names is discarded, as is `Forwarded`), `Expect: 100-continue` is answered here.
//! * `Upgrade` (WebSocket and any other): when the route allows it (`websocket`), the request is forwarded and, after the target's `101`,
//!   the connection becomes a plain two-way byte tunnel. A route with `websocket: false` answers 403 and the target is not contacted.
//! * HTTPS: TLS ends here with the certificate of the route chosen by SNI (rustls `ResolvesServerCert`), TLS 1.2 or 1.3. A certificate comes
//!   from a [`CertSource`] ([`FileCerts`]: PEM files), is checked with [`check_certificate`] when loaded, and a host whose certificate is
//!   invalid, for another host, or expired is not served (the handshake fails; [`Listener::refused`] says why). Expiry is also checked
//!   at every handshake. [`Listener::reload_certs`] replaces certificates without a restart (new connections use the new one); a
//!   replacement that fails the check does not remove a still valid certificate. A `Host` that matches no route, or differs from the SNI
//!   of the connection, is answered 421.
//! * `Scheme::Passthrough`: the ClientHello is read (its SNI parsed by [`client_hello_sni`]), and the connection is tunnelled untouched to the
//!   target. A port may carry both https and passthrough routes: the SNI decides.
//! * `redirect_http`: plain HTTP on [`Options::redirect_port`] (80) answers hosts of such routes with `308` to `https://`.
//! * [`PortGate`]: called with `open(port)` once after a port is bound and `close(port)` once when its last route is gone (or the
//!   listener stops), so the caller can drive a firewall. A port that cannot be bound is an error that says so; no other port is ever used.
//! * Mutual TLS (`mtls`): a host named in [`Options::client_ca`] needs a client certificate signed by that CA (403/503 otherwise).
//!   Certificates and CAs are reloaded every [`Options::cert_reload`] when set. `gate::FirewallGate` is a [`PortGate`] that drives the firewall.
//! * Limits: a cap on concurrent connections (extra ones are closed at once), timeouts on connecting to and reading from the target.
//!
//! **What it does not do**
//! * `Access::Members` is **not enforced**. A plain proxy cannot know who a person is; the listener serves a `members` route to anyone who
//!   can reach the port, exactly like a `public` one. Every log line shows the route's access (`members (not enforced by this listener)`),
//!   so nobody is led to think otherwise. Put a sign-in in front (the IYAKKI web service) or restrict the port with the firewall.
//! * `Target::Service` (a Kubernetes Service) is reachable only through a [`ServiceDialer`] in [`Options::dialer`] (`pod::PortForward` or
//!   `pod::ClusterDns`); without one requests to such a route get a 502 that says so.
//! * Not supported: HTTP/1.0 (505), HTTP/2 (ALPN offers only `http/1.1`), trailers (dropped), a ClientHello split over
//!   several TLS records, `CONNECT`, absolute-form request targets, and paths with `..` segments (400). Routes are matched on the raw
//!   path, not a decoded one.
//! * WebSocket and other tunnels over TLS are polled every 10 ms (one thread per connection), which is simple and costs a little latency
//!   and idle CPU; a tunnel idle for an hour is closed.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::cloudroute::{check_certificate, load_certified, name_covers, Access, CertRef, Route, RouteTable, Scheme, Target};

/// The longest request head accepted.
pub const HEAD_LIMIT: usize = 16 * 1024;
const RESPONSE_HEAD_LIMIT: usize = 64 * 1024;

// ------------------------------------------------------------------------------------------------------------ public pieces

/// Where certificates come from: [`FileCerts`] (a folder) or [`vault::VaultCerts`] (the vault).
pub trait CertSource: Send + Sync {
    /// The PEM chain and PEM private key for the route of `host` that names `cert`.
    fn load(&self, host: &str, cert: &CertRef) -> Result<(String, String), String>;
    /// The PEM of the CA (one or more certificates) that signs the client certificates a mutual-TLS route accepts, by the name given in
    /// [`Options::client_ca`]. A source that has none says so, and such a route then refuses every client.
    fn load_client_ca(&self, name: &str) -> Result<String, String> {
        Err(format!("this certificate source has no client CA \"{name}\""))
    }
}

/// Certificates in a folder: `NAME.crt` and `NAME.key` (NAME is the route's certificate name), or else `HOST.crt` and `HOST.key`.
/// The files are read on every load, so replacing them and calling [`Listener::reload_certs`] is enough.
pub struct FileCerts {
    dir: PathBuf,
}

impl FileCerts {
    pub fn new(dir: impl Into<PathBuf>) -> FileCerts {
        FileCerts { dir: dir.into() }
    }
}

impl CertSource for FileCerts {
    fn load(&self, host: &str, cert: &CertRef) -> Result<(String, String), String> {
        let name = match cert {
            CertRef::Vault(n) | CertRef::Secret(n) => n.as_str(),
        };
        for stem in [name, host] {
            let (c, k) = (self.dir.join(format!("{stem}.crt")), self.dir.join(format!("{stem}.key")));
            if c.is_file() {
                let chain = std::fs::read_to_string(&c).map_err(|e| format!("{}: {e}", c.display()))?;
                let key = std::fs::read_to_string(&k).map_err(|e| format!("{}: {e}", k.display()))?;
                return Ok((chain, key));
            }
        }
        Err(format!("no {name}.crt and {name}.key (or {host}.crt and {host}.key) in {}", self.dir.display()))
    }

    /// `NAME.ca` in the folder.
    fn load_client_ca(&self, name: &str) -> Result<String, String> {
        if !crate::cloudstorage::plain_name(name) {
            return Err(format!("\"{name}\" is not a CA name"));
        }
        let f = self.dir.join(format!("{name}.ca"));
        std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))
    }
}

/// How a route whose target is a Kubernetes Service (a pod and its port) is reached from this machine: a connected socket, or the reason
/// it could not be had. Implementations in `cloudlisten_pod.rs`; the listener applies its own connect timeout through `timeout`.
pub trait ServiceDialer: Send + Sync {
    fn dial(&self, name: &str, port: u16, timeout: Duration) -> io::Result<TcpStream>;
}

fn dest_text(t: &Target) -> String {
    match t {
        Target::Local { port } => format!("127.0.0.1:{port}"),
        Target::Service { name, port } => format!("service {name}:{port}"),
    }
}

/// The firewall seen from the listener: a port gains its first route (`open`) or loses its last one (`close`).
pub trait PortGate: Send + Sync {
    fn open(&self, port: u16) -> Result<(), String>;
    fn close(&self, port: u16);
}

/// A gate that does nothing, for a caller that manages the firewall itself.
pub struct NoGate;

impl PortGate for NoGate {
    fn open(&self, _port: u16) -> Result<(), String> {
        Ok(())
    }
    fn close(&self, _port: u16) {}
}

#[derive(Clone)]
pub struct Options {
    /// The address to listen on (all interfaces by default).
    pub bind: IpAddr,
    /// Where plain HTTP is answered with the redirect of `redirect_http` routes.
    pub redirect_port: u16,
    pub max_conns: usize,
    /// How long a client may take to send a request head.
    pub head_timeout: Duration,
    /// How long a keep-alive connection may wait for its next request.
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    /// How long the target may take for each read (response head and body).
    pub upstream_timeout: Duration,
    /// One line per event. The default writes to standard error.
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
    /// How `Target::Service` routes are reached (a pod through the cluster, see `cloudlisten_pod.rs`). Without one they answer 502.
    pub dialer: Option<Arc<dyn ServiceDialer>>,
    /// Mutual TLS: route host (as written in the route, lower case) -> the name of the CA (in the [`CertSource`]) that signs the client
    /// certificates that route accepts. A request without a certificate signed by that CA is answered 403 and the target is not contacted.
    pub client_ca: BTreeMap<String, String>,
    /// Re-read the certificates (and client CAs) from the source this often, so a rotation takes effect by itself. `None`: only on
    /// [`Listener::reload_certs`].
    pub cert_reload: Option<Duration>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            redirect_port: 80,
            max_conns: 256,
            head_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(3),
            upstream_timeout: Duration::from_secs(60),
            log: Arc::new(|l| eprintln!("cloud listen: {l}")),
            dialer: None,
            client_ca: BTreeMap::new(),
            cert_reload: None,
        }
    }
}

struct CertEntry {
    key: Arc<CertifiedKey>,
    not_after: i64,
}

struct Inner {
    table: RwLock<Arc<RouteTable>>,
    certs: RwLock<HashMap<String, CertEntry>>,
    refused: Mutex<Vec<(String, String)>>,
    source: Arc<dyn CertSource>,
    gate: Arc<dyn PortGate>,
    opts: Options,
    tls: Arc<rustls::ServerConfig>,
    active: AtomicUsize,
    auth: Arc<mtls::AuthState>,
    closed: AtomicBool,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Inner {
    fn log(&self, line: &str) {
        (self.opts.log)(line)
    }
    fn table(&self) -> Arc<RouteTable> {
        self.table.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// A connection to a route's target: a local port, or a service through the configured [`ServiceDialer`].
    fn dial(&self, t: &Target) -> io::Result<TcpStream> {
        match t {
            Target::Local { port } => connect_local(*port, self.opts.connect_timeout),
            Target::Service { name, port } => match &self.opts.dialer {
                Some(d) => d.dial(name, *port, self.opts.connect_timeout),
                None => Err(io::Error::new(io::ErrorKind::Unsupported, "a Kubernetes Service is not reachable from a local listener")),
            },
        }
    }
}

struct Resolver(Arc<Inner>);

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Resolver")
    }
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?.to_ascii_lowercase();
        let certs = self.0.certs.read().unwrap_or_else(|e| e.into_inner());
        let found = certs.get(&name).or_else(|| name.split_once('.').and_then(|(_, rest)| certs.get(&format!("*.{rest}"))))?;
        (now() < found.not_after).then(|| found.key.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Plain,
    Tls,
}

struct PortHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    addr: SocketAddr,
}

/// A running listener. Dropping it (or [`Listener::stop`]) closes the ports; connections already open finish on their own.
pub struct Listener {
    inner: Arc<Inner>,
    ports: Mutex<BTreeMap<u16, PortHandle>>,
}

/// The ports a table needs and what each carries. Mixed plain and TLS routes on one port cannot work and are refused.
fn wanted_ports(t: &RouteTable, o: &Options) -> Result<BTreeMap<u16, Kind>, String> {
    let mut m: BTreeMap<u16, Kind> = BTreeMap::new();
    for r in &t.routes {
        let k = if r.scheme == Scheme::Http { Kind::Plain } else { Kind::Tls };
        if let Some(old) = m.insert(r.listen_port(), k) {
            if old != k {
                return Err(format!("port {} carries both plain http and tls routes (route {}); give them different listen ports", r.listen_port(), r.host));
            }
        }
    }
    if t.routes.iter().any(|r| r.redirect_http) && m.insert(o.redirect_port, Kind::Plain) == Some(Kind::Tls) {
        return Err(format!("the redirect port {} is also a tls port; choose another redirect port", o.redirect_port));
    }
    Ok(m)
}

fn bind(ip: IpAddr, port: u16) -> Result<TcpListener, String> {
    TcpListener::bind((ip, port)).map_err(|e| {
        let hint = if port < 1024 && e.kind() == io::ErrorKind::PermissionDenied { " (ports below 1024 need the right to bind them, for example CAP_NET_BIND_SERVICE, or choose a listen port of 1024 or more)" } else { "" };
        format!("cannot listen on port {port}: {e}{hint}; no other port is used instead")
    })
}

impl Listener {
    /// Bind every port the table needs, open them in the gate, load the certificates, and start serving.
    pub fn start(table: RouteTable, source: Arc<dyn CertSource>, gate: Arc<dyn PortGate>, opts: Options) -> Result<Listener, String> {
        table.check(&[])?;
        let wanted = wanted_ports(&table, &opts)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let auth = Arc::new(mtls::AuthState::default());
        let auth2 = auth.clone();
        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<Inner>| {
            let resolver = LazyResolver(weak.clone());
            let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
                .expect("ring supports TLS 1.2 and 1.3")
                .with_client_cert_verifier(Arc::new(mtls::DynVerifier::new(auth.clone())))
                .with_cert_resolver(Arc::new(resolver));
            cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
            Inner {
                table: RwLock::new(Arc::new(table)),
                certs: RwLock::new(HashMap::new()),
                refused: Mutex::new(Vec::new()),
                source,
                gate,
                opts,
                tls: Arc::new(cfg),
                active: AtomicUsize::new(0),
                auth: auth2,
                closed: AtomicBool::new(false),
            }
        });
        let l = Listener { inner, ports: Mutex::new(BTreeMap::new()) };
        for line in l.reload_certs() {
            l.inner.log(&line);
        }
        l.bind_ports(&wanted)?;
        l.announce();
        if let Some(every) = l.inner.opts.cert_reload {
            spawn_reloader(&l.inner, every);
        }
        Ok(l)
    }

    fn announce(&self) {
        let t = self.inner.table();
        for r in &t.routes {
            let dest = match &r.target {
                Target::Local { port } => format!("127.0.0.1:{port}"),
                Target::Service { name, port } if self.inner.opts.dialer.is_some() => format!("service {name}:{port} (through the cluster)"),
                Target::Service { name, port } => format!("service {name}:{port} (NOT reachable from a local listener: requests get 502)"),
            };
            self.inner.log(&format!("route {}{} on port {} ({:?}) -> {dest}, access {}", r.host, r.path_prefix, r.listen_port(), r.scheme, access_text(r.access)));
        }
    }

    /// Bind the ports not yet bound and open them in the gate; on any failure the ports of this call are released again.
    fn bind_ports(&self, wanted: &BTreeMap<u16, Kind>) -> Result<(), String> {
        let mut ports = self.ports.lock().unwrap_or_else(|e| e.into_inner());
        let mut added: Vec<u16> = Vec::new();
        let mut fail: Option<String> = None;
        let missing: Vec<u16> = wanted.keys().filter(|p| !ports.contains_key(p)).copied().collect();
        for p in missing {
            let step = bind(self.inner.opts.bind, p).and_then(|lis| {
                self.inner.gate.open(p).map_err(|e| format!("port {p}: the firewall could not be opened: {e}"))?;
                Ok(lis)
            });
            match step {
                Ok(lis) => {
                    let h = spawn_port(self.inner.clone(), p, lis);
                    ports.insert(p, h);
                    added.push(p);
                }
                Err(e) => {
                    fail = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = fail {
            for p in added {
                if let Some(h) = ports.remove(&p) {
                    stop_port(&self.inner, p, h);
                }
            }
            return Err(e);
        }
        Ok(())
    }

    /// Apply a new table: ports of new routes are bound and opened, ports that lost their last route are closed. Nothing changes when the
    /// new table is refused or a port cannot be bound.
    pub fn update(&self, table: RouteTable) -> Result<(), String> {
        table.check(&[])?;
        let wanted = wanted_ports(&table, &self.inner.opts)?;
        self.bind_ports(&wanted)?;
        *self.inner.table.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(table);
        for line in self.reload_certs() {
            self.inner.log(&line);
        }
        let gone: Vec<(u16, PortHandle)> = {
            let mut ports = self.ports.lock().unwrap_or_else(|e| e.into_inner());
            let drop_these: Vec<u16> = ports.keys().filter(|p| !wanted.contains_key(p)).copied().collect();
            drop_these.into_iter().filter_map(|p| ports.remove(&p).map(|h| (p, h))).collect()
        };
        for (p, h) in gone {
            stop_port(&self.inner, p, h);
        }
        self.announce();
        Ok(())
    }

    /// Load the certificates of all https routes again (from the [`CertSource`]). New connections use the new ones. Returns one line per
    /// host. A certificate that fails the check is not served; one that was valid before and still is stays in place.
    pub fn reload_certs(&self) -> Vec<String> {
        reload(&self.inner)
    }

    /// Hosts that are not served over https, and why.
    pub fn refused(&self) -> Vec<(String, String)> {
        self.inner.refused.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The address a port is really bound to.
    pub fn local_addr(&self, port: u16) -> Option<SocketAddr> {
        self.ports.lock().unwrap_or_else(|e| e.into_inner()).get(&port).map(|h| h.addr)
    }

    /// The ports in use.
    pub fn ports(&self) -> Vec<u16> {
        self.ports.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect()
    }

    pub fn stop(self) {
        drop(self);
    }
}

fn reload(inner: &Inner) -> Vec<String> {
    let t = inner.table();
    let at = now();
    let mut fresh: HashMap<String, CertEntry> = HashMap::new();
    let mut refused: Vec<(String, String)> = Vec::new();
    let mut lines = Vec::new();
    let mut tried: Vec<String> = Vec::new();
    let old = inner.certs.read().unwrap_or_else(|e| e.into_inner());
    for r in t.routes.iter().filter(|r| r.scheme == Scheme::Https) {
        let Some(cref) = &r.cert else { continue };
        let host = r.host.to_ascii_lowercase();
        if tried.contains(&host) {
            continue;
        }
        tried.push(host.clone());
        match inner.source.load(&host, cref).and_then(|(chain, key)| {
            let (info, line) = check_certificate(&chain, &key, &host, at)?;
            Ok((CertEntry { key: Arc::new(load_certified(&chain, &key)?), not_after: info.not_after }, line))
        }) {
            Ok((entry, line)) => {
                lines.push(format!("certificate {host}: {line}"));
                fresh.insert(host, entry);
            }
            Err(e) => match old.get(&host).filter(|o| o.not_after > at) {
                Some(o) => {
                    lines.push(format!("certificate {host}: new certificate refused ({e}); the previous one stays"));
                    fresh.insert(host, CertEntry { key: o.key.clone(), not_after: o.not_after });
                }
                None => {
                    lines.push(format!("certificate {host}: refused, the host is not served over https ({e})"));
                    refused.push((host, e));
                }
            },
        }
    }
    drop(old);
    *inner.certs.write().unwrap_or_else(|e| e.into_inner()) = fresh;
    *inner.refused.lock().unwrap_or_else(|e| e.into_inner()) = refused;
    lines.extend(mtls::reload_cas(inner, &t));
    lines
}

/// Re-read the certificates every `every` until the listener is gone. Only changed results are logged.
fn spawn_reloader(inner: &Arc<Inner>, every: Duration) {
    let weak = Arc::downgrade(inner);
    let _ = std::thread::Builder::new().name("cloud-cert-reload".into()).spawn(move || {
        let mut last: Vec<String> = Vec::new();
        'outer: loop {
            let mut waited = Duration::ZERO;
            while waited < every {
                std::thread::sleep(Duration::from_millis(20));
                waited += Duration::from_millis(20);
                match weak.upgrade() {
                    Some(i) if !i.closed.load(Ordering::Relaxed) => {}
                    _ => break 'outer,
                }
            }
            let Some(i) = weak.upgrade() else { break };
            let lines = reload(&i);
            if lines != last {
                for l in &lines {
                    i.log(l);
                }
                last = lines;
            }
        }
    });
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Relaxed);
        let all: Vec<(u16, PortHandle)> = std::mem::take(&mut *self.ports.lock().unwrap_or_else(|e| e.into_inner())).into_iter().collect();
        for (p, h) in all {
            stop_port(&self.inner, p, h);
        }
    }
}

/// The resolver is made before the `Inner` that owns the configuration that owns it.
struct LazyResolver(std::sync::Weak<Inner>);

impl std::fmt::Debug for LazyResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LazyResolver")
    }
}

impl ResolvesServerCert for LazyResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Resolver(self.0.upgrade()?).resolve(hello)
    }
}

fn stop_port(inner: &Inner, port: u16, mut h: PortHandle) {
    h.stop.store(true, Ordering::Relaxed);
    if let Some(t) = h.thread.take() {
        let _ = t.join();
    }
    inner.gate.close(port);
    inner.log(&format!("port {port} closed"));
}

fn spawn_port(inner: Arc<Inner>, port: u16, lis: TcpListener) -> PortHandle {
    let addr = lis.local_addr().unwrap_or_else(|_| SocketAddr::new(inner.opts.bind, port));
    let _ = lis.set_nonblocking(true);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::Builder::new().name(format!("cloud-listen-{port}")).spawn(move || accept_loop(inner, port, lis, flag)).ok();
    PortHandle { stop, thread, addr }
}

struct Guard(Arc<Inner>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

fn accept_loop(inner: Arc<Inner>, port: u16, lis: TcpListener, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match lis.accept() {
            Ok((tcp, peer)) => {
                if inner.active.fetch_add(1, Ordering::Relaxed) >= inner.opts.max_conns {
                    inner.active.fetch_sub(1, Ordering::Relaxed);
                    inner.log(&format!("{peer}: too many connections ({}), closed", inner.opts.max_conns));
                    continue;
                }
                let guard = Guard(inner.clone());
                let i = inner.clone();
                let spawned = std::thread::Builder::new().name("cloud-conn".into()).stack_size(512 * 1024).spawn(move || {
                    let _g = guard;
                    let _ = tcp.set_nonblocking(false);
                    let _ = tcp.set_nodelay(true);
                    serve_connection(&i, tcp, peer, port);
                });
                if spawned.is_err() {
                    inner.log("could not start a thread for a connection");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn access_text(a: Access) -> &'static str {
    match a {
        Access::Public => "public",
        Access::Members => "members (not enforced by this listener)",
    }
}

// ------------------------------------------------------------------------------------------------------------ sockets

trait Sock: Read + Write + Send {
    fn set_timeout(&self, d: Option<Duration>);
    /// The SNI of the TLS connection, lower case.
    fn sni(&self) -> Option<String> {
        None
    }
    /// The client certificate chain (DER, end entity first) the peer presented, empty when none or not TLS.
    fn peer_certs(&self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}

impl Sock for TcpStream {
    fn set_timeout(&self, d: Option<Duration>) {
        let _ = self.set_read_timeout(d);
    }
}

/// A socket whose first bytes were already read (the ClientHello): they are given back first.
struct Replay {
    prefix: Vec<u8>,
    pos: usize,
    tcp: TcpStream,
}

impl Read for Replay {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.prefix.len() {
            let n = buf.len().min(self.prefix.len() - self.pos);
            buf[..n].copy_from_slice(&self.prefix[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.tcp.read(buf)
    }
}

impl Write for Replay {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tcp.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

impl Sock for rustls::StreamOwned<rustls::ServerConnection, Replay> {
    fn set_timeout(&self, d: Option<Duration>) {
        let _ = self.sock.tcp.set_read_timeout(d);
    }
    fn sni(&self) -> Option<String> {
        self.conn.server_name().map(|s| s.to_ascii_lowercase())
    }
    fn peer_certs(&self) -> Vec<Vec<u8>> {
        self.conn.peer_certificates().map(|c| c.iter().map(|d| d.as_ref().to_vec()).collect()).unwrap_or_default()
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

// ------------------------------------------------------------------------------------------------------------ TLS ClientHello

/// What the first TLS record of a connection says.
#[derive(Debug, PartialEq, Eq)]
pub enum Hello {
    /// More bytes are needed.
    Incomplete,
    /// A complete ClientHello; the server name when it carries one.
    Sni(Option<String>),
}

struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| b[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2).map(|b| (b[0] as usize) << 8 | b[1] as usize)
    }
}

/// Read the server name out of the start of a TLS connection (our own reader, so a passthrough route can be chosen without terminating
/// TLS). The ClientHello has to fit in one TLS record.
pub fn client_hello_sni(b: &[u8]) -> Result<Hello, &'static str> {
    if b.first().is_some_and(|t| *t != 0x16) || b.get(1).is_some_and(|v| *v != 3) {
        return Err("not a TLS handshake");
    }
    if b.len() < 5 {
        return Ok(Hello::Incomplete);
    }
    let rec = (b[3] as usize) << 8 | b[4] as usize;
    if rec > 16384 {
        return Err("TLS record too large");
    }
    if b.len() < 5 + rec {
        return Ok(Hello::Incomplete);
    }
    let hs = &b[5..5 + rec];
    if hs.len() < 4 || hs[0] != 1 {
        return Err("not a ClientHello");
    }
    let hl = (hs[1] as usize) << 16 | (hs[2] as usize) << 8 | hs[3] as usize;
    if hl + 4 > hs.len() {
        return Err("a ClientHello over several TLS records is not supported");
    }
    let bad = "malformed ClientHello";
    let mut c = Cur(&hs[4..4 + hl]);
    c.take(2 + 32).ok_or(bad)?;
    let n = c.u8().ok_or(bad)?;
    c.take(n).ok_or(bad)?;
    let n = c.u16().ok_or(bad)?;
    c.take(n).ok_or(bad)?;
    let n = c.u8().ok_or(bad)?;
    c.take(n).ok_or(bad)?;
    if c.0.is_empty() {
        return Ok(Hello::Sni(None));
    }
    let n = c.u16().ok_or(bad)?;
    let mut ext = Cur(c.take(n).ok_or(bad)?);
    while !ext.0.is_empty() {
        let (ty, n) = (ext.u16().ok_or(bad)?, ext.u16().ok_or(bad)?);
        let body = ext.take(n).ok_or(bad)?;
        if ty != 0 {
            continue;
        }
        let mut list = Cur(body);
        let n = list.u16().ok_or(bad)?;
        let mut list = Cur(list.take(n).ok_or(bad)?);
        while !list.0.is_empty() {
            let kind = list.u8().ok_or(bad)?;
            let n = list.u16().ok_or(bad)?;
            let name = list.take(n).ok_or(bad)?;
            if kind == 0 {
                let s = std::str::from_utf8(name).map_err(|_| bad)?;
                if s.is_empty() || !s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) {
                    return Err("the server name is not a host name");
                }
                return Ok(Hello::Sni(Some(s.trim_end_matches('.').to_ascii_lowercase())));
            }
        }
    }
    Ok(Hello::Sni(None))
}

/// The passthrough route for a server name on a port: an exact host before a wildcard.
fn passthrough_route<'a>(t: &'a RouteTable, sni: &str, port: u16) -> Option<&'a Route> {
    let all = || t.routes.iter().filter(|r| r.scheme == Scheme::Passthrough && r.listen_port() == port && name_covers(&r.host, sni));
    all().find(|r| !r.host.starts_with("*.")).or_else(|| all().next())
}

// ------------------------------------------------------------------------------------------------------------ connections

fn serve_connection(inner: &Arc<Inner>, tcp: TcpStream, peer: SocketAddr, port: u16) {
    let tls = inner.table().routes.iter().any(|r| r.listen_port() == port && r.scheme != Scheme::Http);
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
    if !tls {
        serve_http(inner, Box::new(tcp), peer, port, false);
        return;
    }
    // read the ClientHello: it decides between tunnelling untouched and terminating TLS
    let _ = tcp.set_read_timeout(Some(inner.opts.head_timeout));
    let deadline = Instant::now() + inner.opts.head_timeout;
    let mut hello: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let sni = loop {
        match client_hello_sni(&hello) {
            Ok(Hello::Sni(s)) => break s,
            Ok(Hello::Incomplete) => {}
            Err(why) => {
                inner.log(&format!("{peer}: refused on port {port}: {why}"));
                return;
            }
        }
        if Instant::now() > deadline {
            inner.log(&format!("{peer}: no ClientHello in time on port {port}"));
            return;
        }
        match (&tcp).read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => hello.extend_from_slice(&chunk[..n]),
        }
    };
    let table = inner.table();
    if let Some(r) = sni.as_deref().and_then(|s| passthrough_route(&table, s, port)) {
        let dest = dest_text(&r.target);
        if matches!(r.target, Target::Service { .. }) && inner.opts.dialer.is_none() {
            inner.log(&format!("{peer}: {} is a service of a cluster, not reachable from a local listener; closed", r.host));
            return;
        }
        match inner.dial(&r.target) {
            Ok(up) => {
                inner.log(&format!("{peer} tls passthrough {} -> {dest} [access {}]", sni.unwrap_or_default(), access_text(r.access)));
                tunnel_raw(tcp, up, &hello);
            }
            Err(e) => inner.log(&format!("{peer} tls passthrough {} -> {dest} failed: {e}", r.host)),
        }
        return;
    }
    let conn = match rustls::ServerConnection::new(inner.tls.clone()) {
        Ok(c) => c,
        Err(e) => {
            inner.log(&format!("{peer}: TLS setup failed: {e}"));
            return;
        }
    };
    let stream = rustls::StreamOwned::new(conn, Replay { prefix: hello, pos: 0, tcp });
    serve_http(inner, Box::new(stream), peer, port, true);
}

fn connect_local(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let s = TcpStream::connect_timeout(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port), timeout)?;
    let _ = s.set_nodelay(true);
    Ok(s)
}

/// Two threads copy in each direction until either side ends.
fn tunnel_raw(client: TcpStream, up: TcpStream, first: &[u8]) {
    let _ = client.set_read_timeout(None);
    let _ = up.set_read_timeout(None);
    let mut up_w = match up.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    if up_w.write_all(first).is_err() {
        return;
    }
    let (mut c_r, c_w, mut u_r) = match (client.try_clone(), client.try_clone(), up.try_clone()) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => return,
    };
    let back = std::thread::spawn(move || {
        let mut c_w = c_w;
        let _ = io::copy(&mut u_r, &mut c_w);
        let _ = c_w.shutdown(Shutdown::Both);
    });
    let _ = io::copy(&mut c_r, &mut up_w);
    let _ = up_w.shutdown(Shutdown::Both);
    let _ = back.join();
    let _ = client.shutdown(Shutdown::Both);
}

// ------------------------------------------------------------------------------------------------------------ HTTP

enum HeadErr {
    Eof,
    TooLarge,
    Timeout,
    Io,
}

/// Read up to and including the blank line that ends a head. Reads no further, so a pipelined next request stays in the reader.
fn read_head<R: BufRead>(r: &mut R, limit: usize, deadline: Instant) -> Result<Vec<u8>, HeadErr> {
    let mut head: Vec<u8> = Vec::with_capacity(512);
    loop {
        if Instant::now() > deadline {
            return Err(HeadErr::Timeout);
        }
        let (used, done) = {
            let buf = match r.fill_buf() {
                Ok(b) => b,
                Err(e) if is_timeout(&e) => return Err(HeadErr::Timeout),
                Err(_) => return Err(HeadErr::Io),
            };
            if buf.is_empty() {
                return Err(if head.is_empty() { HeadErr::Eof } else { HeadErr::Io });
            }
            let (mut used, mut done) = (0, false);
            for &b in buf {
                head.push(b);
                used += 1;
                if head.ends_with(b"\r\n\r\n") {
                    done = true;
                    break;
                }
                if head.len() >= limit {
                    return Err(HeadErr::TooLarge);
                }
            }
            (used, done)
        };
        r.consume(used);
        if done {
            return Ok(head);
        }
    }
}

type Headers = Vec<(String, String)>;

fn token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
}

/// First line and header fields of a head (the blank line included). Strict: CRLF only, no folded lines, no control characters.
fn parse_head(raw: &[u8]) -> Result<(String, Headers), &'static str> {
    let text = std::str::from_utf8(raw).map_err(|_| "the head is not valid text")?;
    let text = text.strip_suffix("\r\n\r\n").ok_or("the head does not end properly")?;
    let mut lines = text.split("\r\n");
    let start = lines.next().unwrap_or("").to_string();
    if start.is_empty() {
        return Err("empty request line");
    }
    let mut headers = Vec::new();
    for l in lines {
        if l.contains('\r') || l.contains('\n') {
            return Err("a bare CR or LF in the head");
        }
        if l.starts_with(' ') || l.starts_with('\t') {
            return Err("folded header lines are not accepted");
        }
        let (n, v) = l.split_once(':').ok_or("a header line without a colon")?;
        if !token(n) {
            return Err("a header name is not valid");
        }
        let v = v.trim_matches(|c| c == ' ' || c == '\t');
        if v.bytes().any(|c| (c < 0x20 && c != b'\t') || c == 0x7f) {
            return Err("a control character in a header value");
        }
        headers.push((n.to_string(), v.to_string()));
    }
    if start.bytes().any(|c| c < 0x20 || c == 0x7f) {
        return Err("a control character in the first line");
    }
    Ok((start, headers))
}

fn values<'a>(h: &'a Headers, name: &str) -> Vec<&'a str> {
    h.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
}

fn tokens_of(h: &Headers, name: &str) -> Vec<String> {
    values(h, name).iter().flat_map(|v| v.split(',')).map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()).collect()
}

const HOP: [&str; 9] = ["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"];

fn hop_by_hop(h: &Headers) -> Vec<String> {
    let mut v: Vec<String> = HOP.iter().map(|s| s.to_string()).collect();
    v.extend(tokens_of(h, "connection"));
    v
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Body {
    None,
    Len(u64),
    Chunked,
    UntilClose,
}

/// How the body of a message is framed; an error for what could be used to smuggle a request.
fn framing(h: &Headers) -> Result<Body, (u16, &'static str)> {
    let te = tokens_of(h, "transfer-encoding");
    let cl = values(h, "content-length");
    if !te.is_empty() {
        if !cl.is_empty() {
            return Err((400, "both Content-Length and Transfer-Encoding"));
        }
        return if te == ["chunked"] { Ok(Body::Chunked) } else { Err((501, "only Transfer-Encoding: chunked is supported")) };
    }
    let mut len: Option<u64> = None;
    for v in cl.iter().flat_map(|v| v.split(',')) {
        let v = v.trim();
        let n: u64 = if !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()) { v.parse().map_err(|_| (400, "Content-Length is not a number"))? } else { return Err((400, "Content-Length is not a number")) };
        if len.is_some_and(|l| l != n) {
            return Err((400, "different Content-Length values"));
        }
        len = Some(n);
    }
    Ok(len.map_or(Body::None, Body::Len))
}

fn reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

fn respond(w: &mut dyn Write, code: u16, extra: &[(&str, &str)], text: &str, keep: bool) -> bool {
    let mut s = format!("HTTP/1.1 {code} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n", reason(code), text.len());
    for (n, v) in extra {
        s.push_str(&format!("{n}: {v}\r\n"));
    }
    if !keep {
        s.push_str("Connection: close\r\n");
    }
    s.push_str("\r\n");
    s.push_str(text);
    w.write_all(s.as_bytes()).and_then(|_| w.flush()).is_ok() && keep
}

fn bad_data(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

fn copy_len<R: BufRead, W: Write>(r: &mut R, w: &mut W, mut n: u64) -> io::Result<()> {
    while n > 0 {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let k = (buf.len() as u64).min(n) as usize;
        w.write_all(&buf[..k])?;
        w.flush()?;
        r.consume(k);
        n -= k as u64;
    }
    Ok(())
}

fn copy_to_end<R: BufRead, W: Write>(r: &mut R, w: &mut W) -> io::Result<()> {
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return w.flush();
        }
        let k = buf.len();
        w.write_all(buf)?;
        w.flush()?;
        r.consume(k);
    }
}

/// One CRLF-ended line of at most `max` bytes, without the CRLF.
fn crlf_line<R: BufRead>(r: &mut R, max: usize) -> io::Result<String> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let (mut used, mut done) = (0, false);
        for &b in buf {
            used += 1;
            if b == b'\n' {
                done = true;
                break;
            }
            line.push(b);
            if line.len() > max {
                return Err(bad_data("line too long"));
            }
        }
        r.consume(used);
        if done {
            if line.pop() != Some(b'\r') {
                return Err(bad_data("a line without CR"));
            }
            return String::from_utf8(line).map_err(|_| bad_data("not text"));
        }
    }
}

/// A chunked body, chunk by chunk, forwarded as it arrives. Chunk extensions and trailers are dropped.
fn copy_chunked<R: BufRead, W: Write>(r: &mut R, w: &mut W) -> io::Result<()> {
    loop {
        let line = crlf_line(r, 1024)?;
        let hex = line.split(';').next().unwrap_or("").trim();
        if hex.is_empty() || hex.len() > 16 {
            return Err(bad_data("bad chunk size"));
        }
        let size = u64::from_str_radix(hex, 16).map_err(|_| bad_data("bad chunk size"))?;
        if size == 0 {
            for _ in 0..100 {
                if crlf_line(r, 8192)?.is_empty() {
                    w.write_all(b"0\r\n\r\n")?;
                    return w.flush();
                }
            }
            return Err(bad_data("too many trailer lines"));
        }
        w.write_all(format!("{size:x}\r\n").as_bytes())?;
        copy_len(r, w, size)?;
        if !crlf_line(r, 2)?.is_empty() {
            return Err(bad_data("chunk not ended by CRLF"));
        }
        w.write_all(b"\r\n")?;
        w.flush()?;
    }
}

fn copy_body<R: BufRead, W: Write>(r: &mut R, w: &mut W, b: Body) -> io::Result<()> {
    match b {
        Body::None => Ok(()),
        Body::Len(n) => copy_len(r, w, n),
        Body::Chunked => copy_chunked(r, w),
        Body::UntilClose => copy_to_end(r, w),
    }
}

/// The host of a Host header: no port, lower case.
fn host_only(h: &str) -> String {
    h.rsplit_once(':').filter(|(_, p)| p.bytes().all(|c| c.is_ascii_digit())).map_or(h, |(h, _)| h).trim_end_matches('.').to_ascii_lowercase()
}

fn serve_http(inner: &Arc<Inner>, sock: Box<dyn Sock>, peer: SocketAddr, port: u16, tls: bool) {
    let mut rd = BufReader::with_capacity(16 * 1024, sock);
    loop {
        rd.get_ref().set_timeout(Some(inner.opts.idle_timeout));
        match rd.fill_buf() {
            Ok(b) if !b.is_empty() => {}
            _ => return,
        }
        rd.get_ref().set_timeout(Some(inner.opts.head_timeout));
        if !one_request(inner, &mut rd, peer, port, tls) {
            let _ = rd.get_mut().flush();
            return;
        }
    }
}

/// One request and its response. `true` when the connection can carry another request.
fn one_request(inner: &Arc<Inner>, rd: &mut BufReader<Box<dyn Sock>>, peer: SocketAddr, port: u16, tls: bool) -> bool {
    let deadline = Instant::now() + inner.opts.head_timeout;
    let raw = match read_head(rd, HEAD_LIMIT, deadline) {
        Ok(r) => r,
        Err(HeadErr::TooLarge) => {
            inner.log(&format!("{peer}: request head over {HEAD_LIMIT} bytes, refused"));
            return respond(rd.get_mut(), 431, &[], "request head too large (limit 16 KB)\n", false);
        }
        Err(HeadErr::Timeout) => return respond(rd.get_mut(), 408, &[], "request head too slow\n", false),
        Err(HeadErr::Eof) | Err(HeadErr::Io) => return false,
    };
    let (start, headers) = match parse_head(&raw) {
        Ok(p) => p,
        Err(m) => return respond(rd.get_mut(), 400, &[], &format!("{m}\n"), false),
    };
    let mut parts = start.split(' ');
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if parts.next().is_some() || !token(method) || target.is_empty() {
        return respond(rd.get_mut(), 400, &[], "bad request line\n", false);
    }
    if version != "HTTP/1.1" {
        return respond(rd.get_mut(), 505, &[], "only HTTP/1.1 is supported\n", false);
    }
    if method == "CONNECT" {
        return respond(rd.get_mut(), 405, &[], "CONNECT is not supported\n", false);
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    if !target.starts_with('/') || target.contains('\\') || path.split('/').any(|s| s == ".." || s.eq_ignore_ascii_case("%2e%2e") || s.eq_ignore_ascii_case(".%2e") || s.eq_ignore_ascii_case("%2e.")) {
        return respond(rd.get_mut(), 400, &[], "the request target is not accepted\n", false);
    }
    let hosts = values(&headers, "host");
    if hosts.len() != 1 || hosts[0].is_empty() {
        return respond(rd.get_mut(), 400, &[], "exactly one Host header is needed\n", false);
    }
    let host_header = hosts[0].to_string();
    let host = host_only(&host_header);
    let body = match framing(&headers) {
        Ok(b) => b,
        Err((c, m)) => return respond(rd.get_mut(), c, &[], &format!("{m}\n"), false),
    };
    // a body not yet read means the connection cannot be reused after an early answer
    let reusable = body == Body::None;
    let client_close = tokens_of(&headers, "connection").iter().any(|t| t == "close");
    let keep = reusable && !client_close;
    let table = inner.table();

    if tls {
        let sni = rd.get_ref().sni().unwrap_or_default();
        if sni != host {
            inner.log(&format!("{peer} {method} {host_header}{path}: Host differs from the SNI \"{sni}\", 421"));
            return respond(rd.get_mut(), 421, &[], "the Host does not match the name of this TLS connection\n", keep);
        }
    } else {
        // plain http: an https route that asks for a redirect
        let mut https_ports: Vec<u16> = table.routes.iter().filter(|r| r.redirect_http).map(|r| r.listen_port()).collect();
        https_ports.sort_unstable();
        https_ports.dedup();
        for hp in https_ports {
            if let Some(r) = table.pick(&host, path, hp).filter(|r| r.redirect_http) {
                let safe = host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-');
                if !safe {
                    break;
                }
                let loc = format!("https://{host}{}{target}", if hp == 443 { String::new() } else { format!(":{hp}") });
                inner.log(&format!("{peer} {method} {host_header}{path} -> redirect to https [route {}{}, access {}] 308", r.host, r.path_prefix, access_text(r.access)));
                return respond(rd.get_mut(), 308, &[("Location", &loc)], "", keep);
            }
        }
    }

    let Some(route) = table.pick(&host, path, port).filter(|r| r.scheme != Scheme::Passthrough) else {
        inner.log(&format!("{peer} {method} {host_header}{path} on port {port}: no route, {}", if tls { 421 } else { 404 }));
        return if tls { respond(rd.get_mut(), 421, &[], "no route for this host on this port\n", keep) } else { respond(rd.get_mut(), 404, &[], "no route for this host and path\n", keep) };
    };
    let label = format!("[route {}{}, access {}]", route.host, route.path_prefix, access_text(route.access));
    let dest = dest_text(&route.target);
    if matches!(route.target, Target::Service { .. }) && inner.opts.dialer.is_none() {
        inner.log(&format!("{peer} {method} {host_header}{path} {label}: the target is a cluster service, 502"));
        return respond(rd.get_mut(), 502, &[], "this route points at a Kubernetes Service; a local listener can only reach local ports\n", keep);
    }
    // client certificate (mutual TLS): checked before the target is contacted or an Upgrade is looked at
    if tls {
        if let Some((code, why)) = mtls::verdict(inner, route, &**rd.get_ref()) {
            inner.log(&format!("{peer} {method} {host_header}{path} {label}: client certificate refused ({why}), {code}"));
            return respond(rd.get_mut(), code, &[], &format!("{why}\n"), keep);
        }
    }
    let upgrade = values(&headers, "upgrade").first().map(|v| v.to_string());
    if upgrade.is_some() && !route.websocket {
        inner.log(&format!("{peer} {method} {host_header}{path} {label}: Upgrade refused, 403"));
        return respond(rd.get_mut(), 403, &[], "this route does not allow Upgrade (websocket is off)\n", keep);
    }
    let up = match inner.dial(&route.target) {
        Ok(s) => s,
        Err(e) => {
            inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest}: {e}, 502"));
            return respond(rd.get_mut(), 502, &[], &format!("the target {dest} did not answer\n"), keep);
        }
    };
    let _ = up.set_read_timeout(Some(inner.opts.upstream_timeout));
    let _ = up.set_write_timeout(Some(Duration::from_secs(30)));

    // the request towards the target
    let hop = hop_by_hop(&headers);
    let mut head = format!("{method} {target} HTTP/1.1\r\n");
    for (n, v) in &headers {
        let l = n.to_ascii_lowercase();
        if hop.contains(&l) || l.starts_with("x-forwarded-") || matches!(l.as_str(), "forwarded" | "expect" | "content-length") {
            continue;
        }
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    match body {
        Body::Len(n) => head.push_str(&format!("Content-Length: {n}\r\n")),
        Body::Chunked => head.push_str("Transfer-Encoding: chunked\r\n"),
        _ => {}
    }
    head.push_str(&format!("X-Forwarded-For: {}\r\nX-Forwarded-Proto: {}\r\nX-Forwarded-Host: {host_header}\r\n", peer.ip(), if tls { "https" } else { "http" }));
    match &upgrade {
        Some(u) => head.push_str(&format!("Connection: Upgrade\r\nUpgrade: {u}\r\n")),
        None => head.push_str("Connection: close\r\n"),
    }
    head.push_str("\r\n");
    let sent = (|| -> io::Result<()> {
        let mut w = BufWriter::with_capacity(8192, up.try_clone()?);
        w.write_all(head.as_bytes())?;
        w.flush()?;
        if values(&headers, "expect").iter().any(|v| v.eq_ignore_ascii_case("100-continue")) && body != Body::None {
            let c = rd.get_mut();
            c.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            c.flush()?;
        }
        copy_body(rd, &mut w, body)?;
        w.flush()
    })();
    if let Err(e) = sent {
        inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest}: request not delivered: {e}, 502"));
        respond(rd.get_mut(), 502, &[], "the request could not be passed to the target\n", false);
        return false;
    }

    // the response
    let mut ur = BufReader::with_capacity(16 * 1024, match up.try_clone() {
        Ok(s) => s,
        Err(_) => return false,
    });
    let resp_deadline = Instant::now() + inner.opts.upstream_timeout;
    let (status_line, rheaders, code) = loop {
        let raw = match read_head(&mut ur, RESPONSE_HEAD_LIMIT, resp_deadline) {
            Ok(r) => r,
            Err(_) => {
                inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest}: no valid response, 502"));
                respond(rd.get_mut(), 502, &[], "the target sent no valid response\n", false);
                return false;
            }
        };
        let parsed = parse_head(&raw).ok().and_then(|(s, h)| {
            let code: u16 = s.split(' ').nth(1)?.parse().ok()?;
            s.starts_with("HTTP/1.").then_some((s, h, code))
        });
        let Some((s, h, code)) = parsed else {
            inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest}: unreadable response, 502"));
            respond(rd.get_mut(), 502, &[], "the target sent an unreadable response\n", false);
            return false;
        };
        if (100..200).contains(&code) && code != 101 {
            let c = rd.get_mut();
            if c.write_all(&raw).and_then(|_| c.flush()).is_err() {
                return false;
            }
            continue;
        }
        break (s, h, code);
    };
    if code == 101 {
        if upgrade.is_none() {
            respond(rd.get_mut(), 502, &[], "the target switched protocols without being asked\n", false);
            return false;
        }
        let mut out = format!("{status_line}\r\n");
        for (n, v) in &rheaders {
            out.push_str(&format!("{n}: {v}\r\n"));
        }
        out.push_str("\r\n");
        let c = rd.get_mut();
        if c.write_all(out.as_bytes()).and_then(|_| c.flush()).is_err() {
            return false;
        }
        inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest} 101 (tunnel)"));
        tunnel_poll(rd, &mut ur);
        return false;
    }
    let rbody = if method == "HEAD" || code == 204 || code == 304 {
        Body::None
    } else {
        match framing(&rheaders) {
            Ok(Body::None) | Err(_) => Body::UntilClose,
            Ok(b) => b,
        }
    };
    let keep_after = keep && rbody != Body::UntilClose;
    let rhop = hop_by_hop(&rheaders);
    let mut out = format!("{status_line}\r\n");
    for (n, v) in &rheaders {
        let l = n.to_ascii_lowercase();
        if rhop.contains(&l) {
            continue;
        }
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    if rbody == Body::Chunked {
        out.push_str("Transfer-Encoding: chunked\r\n");
    }
    if !keep_after {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    inner.log(&format!("{peer} {method} {host_header}{path} {label} -> {dest} {code}"));
    let c = rd.get_mut();
    if c.write_all(out.as_bytes()).and_then(|_| c.flush()).is_err() {
        return false;
    }
    if copy_body(&mut ur, c, rbody).is_err() {
        return false;
    }
    keep_after
}

/// After a `101`: bytes go both ways until either side ends. The client side may be TLS (no way to split it), so both sides are polled
/// with a short read timeout.
fn tunnel_poll(client: &mut BufReader<Box<dyn Sock>>, up: &mut BufReader<TcpStream>) {
    let poll = Some(Duration::from_millis(10));
    // bytes already read ahead on either side go first
    let ahead = client.buffer().to_vec();
    client.consume(ahead.len());
    if up.get_mut().write_all(&ahead).is_err() {
        return;
    }
    let ahead = up.buffer().to_vec();
    up.consume(ahead.len());
    if !ahead.is_empty() && client.get_mut().write_all(&ahead).and_then(|_| client.get_mut().flush()).is_err() {
        return;
    }
    client.get_ref().set_timeout(poll);
    let _ = up.get_ref().set_read_timeout(poll);
    let mut buf = vec![0u8; 16 * 1024];
    let mut last = Instant::now();
    while last.elapsed() < Duration::from_secs(3600) {
        let mut moved = false;
        match client.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if up.get_mut().write_all(&buf[..n]).is_err() {
                    break;
                }
                moved = true;
            }
            Err(e) if is_timeout(&e) => {}
            Err(_) => break,
        }
        match up.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let c = client.get_mut();
                if c.write_all(&buf[..n]).and_then(|_| c.flush()).is_err() {
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
    let _ = up.get_ref().shutdown(Shutdown::Both);
}

#[path = "cloudlisten_gate.rs"]
pub mod gate;
#[path = "cloudlisten_mtls.rs"]
mod mtls;
#[path = "cloudlisten_pod.rs"]
pub mod pod;
#[path = "cloudlisten_vault.rs"]
pub mod vault;

#[cfg(test)]
#[path = "cloudlisten_tests.rs"]
mod tests;
