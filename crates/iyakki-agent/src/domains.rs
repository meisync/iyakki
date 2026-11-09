//! Domain names for services on this device (Settings -> Domains, the chat tools `domains_list`, `domain_route_add`,
//! `domain_route_remove`, `domains_listener_set`; `docs/DOMAINS.md`).
//!
//! The engine is `cloudroute.rs` (the route table and its checker) and `cloudlisten.rs` (the reverse proxy with WebSocket and SNI TLS).
//! This file manages them as one service for a signed-in person:
//!
//! * **The table and the listener's settings** are `<data>/domains.json`. It never holds a certificate or a key.
//! * **Certificates** are in the person's vault (`domain.<name>-chain` and `domain.<name>-key`), read into memory by the listener on every
//!   reload. A trial self-signed certificate is made on request; the person's own files can be imported. Expiry within 30 days (or
//!   already) leaves a warning in the inbox.
//! * **The listener** starts on 127.0.0.1 only, on 8080 (http) and 8443 (https). Ports 80 and 443 are used only when the caller says they
//!   were approved. It is reachable from the network only after the firewall opening, which is a step of its own with a local-network
//!   scope and an optional end time; disabling the listener removes the opening, and an end time that passes stops the listener.
//! * **Local names**: a route host ending in `.local` is written to `<data>/domains-names.json` while the listener runs, and the device
//!   name announcer (`iyakki_remote::devname`) announces it as one more name for this device's addresses.
//! * **Discovery**: [`parse_services`] reads `kubectl get svc -A -o json` (through the [`Kube`] trait, the no-shell helper of the managed
//!   cluster) so a route can be filled in from a service found there, and [`own_services`] lists this device's own.
//!
//! Everything the world is asked for comes in through [`Deps`], so the tests use fakes (kubectl output, firewall, announcer, vault).
//!
//! Not enforced: `Access::Members` (the listener cannot know who a person is; see `cloudlisten.rs`). The page and the tools say so.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cloudlisten::gate::{FirewallGate, GateReach};
use crate::cloudlisten::vault::{SecretReader, VaultCerts};
use crate::cloudlisten::{Listener, NoGate, Options, PortGate, ServiceDialer};
use crate::cloudroute::{check_certificate, Access, CertRef, Route, RouteTable, Scheme, Target, WARN_DAYS};
use crate::gitkeys::{AuditFn, SecretStore};

pub const FILE: &str = "domains.json";
pub const NAMES_FILE: &str = "domains-names.json";
pub const DEFAULT_HTTP: u16 = 8080;
pub const DEFAULT_HTTPS: u16 = 8443;
pub const STANDARD_HTTP: u16 = 80;
pub const STANDARD_HTTPS: u16 = 443;
/// The NodePort of the private store (`dist/store-k3s/40-service.yaml`), the worked example of `docs/DOMAINS.md`.
pub const STORE_NODEPORT: u16 = 30580;
const VAULT_PREFIX: &str = "domain.";
const MAX_ERRORS: usize = 30;
const CERT_CHECK_EVERY: i64 = 6 * 3600;

// ------------------------------------------------------------------------------------------------------------ the saved file

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_http")]
    pub http_port: u16,
    #[serde(default = "d_https")]
    pub https_port: u16,
    /// The ports are open to the local network in the firewall (and the listener is bound to all addresses). Off: 127.0.0.1 only.
    #[serde(default)]
    pub firewall: bool,
    /// Unix seconds when the opening ends (0: no end time). Only with `firewall`.
    #[serde(default)]
    pub firewall_until: i64,
}

fn d_http() -> u16 {
    DEFAULT_HTTP
}
fn d_https() -> u16 {
    DEFAULT_HTTPS
}

impl Default for ListenerCfg {
    fn default() -> ListenerCfg {
        ListenerCfg { enabled: false, http_port: DEFAULT_HTTP, https_port: DEFAULT_HTTPS, firewall: false, firewall_until: 0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainsFile {
    #[serde(default)]
    pub routes: Vec<Route>,
    #[serde(default)]
    pub listener: ListenerCfg,
    /// The namespace of each cluster Service a route points at (a `Target::Service` carries only the name).
    #[serde(default)]
    pub service_ns: BTreeMap<String, String>,
}

pub fn load(dir: &Path) -> DomainsFile {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(dir: &Path, f: &DomainsFile) -> Result<(), String> {
    let text = serde_json::to_string_pretty(f).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| format!("{}: {e}", dir.join(FILE).display()))
}

/// Does this route's key (host and path) match?
fn same(r: &Route, host: &str, path: &str) -> bool {
    r.host.eq_ignore_ascii_case(host) && r.path_prefix == path
}

/// The `.local` hosts of the table (no wildcards): the extra names this device answers for.
pub fn local_names(routes: &[Route]) -> Vec<String> {
    let mut v: Vec<String> = routes.iter().map(|r| r.host.to_ascii_lowercase()).filter(|h| h.ends_with(".local") && !h.starts_with('*')).collect();
    v.sort();
    v.dedup();
    v
}

/// What the announcer is to announce. Valid only for the process that wrote it (a name left over from before a restart is not announced).
pub fn read_names(dir: &Path) -> Vec<String> {
    let Some(v) = std::fs::read_to_string(dir.join(NAMES_FILE)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return vec![] };
    if v["pid"].as_u64() != Some(std::process::id() as u64) {
        return vec![];
    }
    v["names"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn write_names(dir: &Path, names: &[String]) -> Result<(), String> {
    let text = serde_json::json!({"pid": std::process::id(), "names": names}).to_string();
    std::fs::write(dir.join(NAMES_FILE), text).map_err(|e| format!("{NAMES_FILE}: {e}"))
}

// ------------------------------------------------------------------------------------------------------------ discovery

/// One TCP port of a cluster Service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundPort {
    pub name: String,
    pub port: u16,
    pub node_port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub namespace: String,
    pub name: String,
    /// `ClusterIP`, `NodePort` or `LoadBalancer`.
    pub kind: String,
    pub ports: Vec<FoundPort>,
}

impl Found {
    /// The target a route to this port gets: a NodePort is reached on this machine at 127.0.0.1 (kube-proxy answers it there), anything
    /// else through the Service (a `kubectl port-forward` the listener runs).
    pub fn target(&self, p: &FoundPort) -> Target {
        match p.node_port {
            Some(n) => Target::Local { port: n },
            None => Target::Service { name: self.name.clone(), port: p.port },
        }
    }

    pub fn label(&self, p: &FoundPort) -> String {
        match p.node_port {
            Some(n) => format!("{}/{} port {} (NodePort {n}, reached on this device)", self.namespace, self.name, p.port),
            None => format!("{}/{} port {} (inside the cluster, reached through kubectl)", self.namespace, self.name, p.port),
        }
    }
}

/// The TCP Services in the `kubectl get svc -A -o json` text: no headless or ExternalName Services, no UDP ports. NodePort and
/// ClusterIP first, the `kube-system` ones last.
pub fn parse_services(json: &str) -> Result<Vec<Found>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("the Service list was not readable: {e}"))?;
    let items = v["items"].as_array().ok_or("the Service list has no items")?;
    let mut out = Vec::new();
    for it in items {
        let spec = &it["spec"];
        let kind = spec["type"].as_str().unwrap_or("ClusterIP");
        if kind == "ExternalName" || spec["clusterIP"].as_str() == Some("None") {
            continue;
        }
        let ports: Vec<FoundPort> = spec["ports"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|p| p["protocol"].as_str().unwrap_or("TCP") == "TCP")
                    .filter_map(|p| {
                        Some(FoundPort {
                            name: p["name"].as_str().unwrap_or("").to_string(),
                            port: u16::try_from(p["port"].as_u64()?).ok().filter(|n| *n > 0)?,
                            node_port: p["nodePort"].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (namespace, name) = (it["metadata"]["namespace"].as_str().unwrap_or("").to_string(), it["metadata"]["name"].as_str().unwrap_or("").to_string());
        if ports.is_empty() || namespace.is_empty() || name.is_empty() {
            continue;
        }
        out.push(Found { namespace, name, kind: kind.to_string(), ports });
    }
    out.sort_by(|a, b| ((a.namespace == "kube-system"), &a.namespace, &a.name).cmp(&((b.namespace == "kube-system"), &b.namespace, &b.name)));
    Ok(out)
}

/// A service of this device itself that a route can point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Own {
    pub label: String,
    pub target: Target,
    pub scheme: Scheme,
    pub note: String,
}

/// This device's own services: the private store's NodePort, and the web desktop (HTTPS on its own port, so a passthrough route: the
/// listener cannot speak TLS to a target, and the browser sees the web desktop's own certificate).
pub fn own_services(web_port: Option<u16>) -> Vec<Own> {
    let mut v = vec![Own { label: format!("Private store (NodePort {STORE_NODEPORT})"), target: Target::Local { port: STORE_NODEPORT }, scheme: Scheme::Http, note: "plain HTTP; only when the private store is installed in the cluster".into() }];
    if let Some(p) = web_port {
        v.push(Own { label: format!("This device's web desktop (HTTPS, port {p})"), target: Target::Local { port: p }, scheme: Scheme::Passthrough, note: "TLS passes through untouched, so the browser shows the web desktop's own certificate and its warning".into() });
    }
    v
}

// ------------------------------------------------------------------------------------------------------------ the world

/// `kubectl get svc -A -o json` on the managed cluster, as text.
pub trait Kube: Send + Sync {
    fn services(&self) -> Result<String, String>;
}

/// The real one: the no-shell kubectl helper of the managed cluster (`K3sDev::kubectl_cap`, which falls back to `k3s kubectl`).
pub struct K3sKube(pub Arc<crate::k3sdev::K3sDev>);

impl Kube for K3sKube {
    fn services(&self) -> Result<String, String> {
        let o = self.0.kubectl_cap(&["get", "svc", "-A", "-o", "json"], 4_000_000);
        if o.ok() {
            Ok(o.stdout)
        } else {
            Err(crate::localreg::explain_kubectl(&o.combined()))
        }
    }
}

/// Makes the firewall gate: the ports it opens, who may reach them, and when the opening ends (0: never).
pub trait GateMaker: Send + Sync {
    fn make(&self, reach: GateReach, until: i64) -> Result<Arc<dyn PortGate>, String>;
}

pub struct RealGates;

impl GateMaker for RealGates {
    fn make(&self, reach: GateReach, until: i64) -> Result<Arc<dyn PortGate>, String> {
        Ok(Arc::new(FirewallGate::real_until(reach, until)))
    }
}

/// Where the extra `.local` names go (the device name announcer reads them).
pub trait NameSink: Send + Sync {
    fn set(&self, names: &[String]) -> Result<(), String>;
}

pub struct FileNames(pub PathBuf);

impl NameSink for FileNames {
    fn set(&self, names: &[String]) -> Result<(), String> {
        write_names(&self.0, names)
    }
}

pub struct Deps {
    pub kube: Arc<dyn Kube>,
    pub secrets: Arc<dyn SecretStore>,
    pub gates: Arc<dyn GateMaker>,
    pub names: Arc<dyn NameSink>,
    /// A warning for the person's inbox: (title, text). Called again for the same title only when the old one was read.
    pub notify: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// A dialer for the Services of a namespace (a `kubectl port-forward`).
    pub dialer: Arc<dyn Fn(&str) -> Result<Arc<dyn ServiceDialer>, String> + Send + Sync>,
    /// Ports other services of this device listen on (the web desktop, developer mode).
    pub held: Arc<dyn Fn() -> Vec<u16> + Send + Sync>,
    /// The port of the web desktop when it is on (for the list of this device's own services).
    pub web_port: Arc<dyn Fn() -> Option<u16> + Send + Sync>,
    pub audit: AuditFn,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

/// Reads the vault with the `domain.` prefix, as the listener's [`VaultCerts`] wants it.
struct Reader(Arc<dyn SecretStore>);

impl SecretReader for Reader {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.get(&format!("{VAULT_PREFIX}{name}")).map(|z| z.as_bytes().to_vec())
    }
}

// ------------------------------------------------------------------------------------------------------------ certificates

/// A vault name for the certificate of a host: `store.local` is `store-local`, `*.x.local` is `wild-x-local`.
pub fn cert_name_for(host: &str) -> String {
    let mut s: String = host.to_ascii_lowercase().replace("*.", "wild-").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.truncate(40);
    s.trim_matches('-').to_string()
}

fn cert_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') && !n.starts_with('-')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertRow {
    pub name: String,
    pub names: Vec<String>,
    /// Days until it runs out (negative: ran out).
    pub days: i64,
    /// Routes that use it.
    pub used_by: Vec<String>,
    pub problem: Option<String>,
}

impl CertRow {
    pub fn line(&self) -> String {
        let when = if self.days < 0 { "expired".to_string() } else if self.days <= WARN_DAYS { format!("runs out in {} day(s)", self.days) } else { format!("valid for {} more days", self.days) };
        match &self.problem {
            Some(p) => format!("{}: {p}", self.name),
            None => format!("{} covers {}: {when}{}", self.name, self.names.join(", "), if self.used_by.is_empty() { "" } else { " (in use)" }),
        }
    }
}

fn first_cert(chain_pem: &str) -> Result<crate::cloudroute::CertInfo, String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let first = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).next().ok_or("no certificate in the vault entry")?.map_err(|e| format!("not PEM: {e}"))?;
    crate::cloudroute::read_cert(first.as_ref()).ok_or_else(|| "the certificate could not be read".to_string())
}

// ------------------------------------------------------------------------------------------------------------ the manager

/// What a change of the listener asks for. Fields left `None` stay as they are.
#[derive(Clone, Debug, Default)]
pub struct ListenerChange {
    pub enabled: Option<bool>,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    /// The caller has the person's approval for ports 80 and 443 (below 1024 the listener needs the right to bind them).
    pub standard_ports_approved: bool,
    /// Open (true) or close (false) the ports to the local network in the firewall.
    pub firewall: Option<bool>,
    /// With `firewall`: hours until the opening ends (None or 0: no end time).
    pub firewall_hours: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    pub bind: String,
    pub ports: Vec<u16>,
    pub firewall: bool,
    pub firewall_until: i64,
    /// Ports the firewall gate has open now.
    pub firewall_ports: Vec<u16>,
    pub names: Vec<String>,
    pub last_errors: Vec<String>,
    /// Hosts that are not served over https, and why.
    pub refused: Vec<(String, String)>,
}

impl Status {
    pub fn line(&self) -> String {
        if !self.enabled {
            return "Off: nothing listens for domain names".into();
        }
        if !self.running {
            return format!("On, but not listening{}", self.last_errors.last().map(|e| format!(": {e}")).unwrap_or_default());
        }
        let ports = self.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
        if self.firewall {
            format!("Listening on port {ports}, open to the local network{}", if self.firewall_until > 0 { " until the end time" } else { "" })
        } else {
            format!("Listening on port {ports}, on this device only ({}); other computers cannot reach it until you open the firewall", self.bind)
        }
    }
}

struct Run {
    listener: Listener,
    bind: IpAddr,
    firewall: bool,
    ports: (u16, u16),
    gate_ports: Arc<FirewallState>,
}

/// The ports a gate has open, shared so the status can show them.
#[derive(Default)]
struct FirewallState(Mutex<Vec<u16>>);

struct RecordingGate {
    inner: Arc<dyn PortGate>,
    open: Arc<FirewallState>,
}

impl PortGate for RecordingGate {
    fn open(&self, port: u16) -> Result<(), String> {
        self.inner.open(port)?;
        let mut v = self.open.0.lock().unwrap_or_else(|e| e.into_inner());
        if !v.contains(&port) {
            v.push(port);
            v.sort_unstable();
        }
        Ok(())
    }
    fn close(&self, port: u16) {
        self.inner.close(port);
        self.open.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != port);
    }
}

struct Inner {
    file: DomainsFile,
    run: Option<Run>,
    start_error: Option<String>,
    last_cert_check: i64,
}

pub struct Manager {
    dir: PathBuf,
    deps: Deps,
    inner: Mutex<Inner>,
    /// The thread that calls [`Manager::tick`] (certificate warnings, the end of a firewall opening) while somebody is signed in.
    ticker: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    log: Arc<Mutex<VecDeque<String>>>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Arc<Manager>>> {
    static ALL: std::sync::OnceLock<Mutex<BTreeMap<String, Arc<Manager>>>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| Mutex::new(BTreeMap::new()))
}

impl Manager {
    pub fn new(dir: &Path, deps: Deps) -> Manager {
        Manager { dir: dir.to_path_buf(), inner: Mutex::new(Inner { file: load(dir), run: None, start_error: None, last_cert_check: 0 }), deps, ticker: Mutex::new(None), log: Arc::new(Mutex::new(VecDeque::new())) }
    }

    /// The manager of a signed-in person (one per data folder and person), made once.
    pub fn shared(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Arc<Manager> {
        let key = format!("{}|{user}", svc.dir().display());
        let mut all = registry().lock().unwrap_or_else(|e| e.into_inner());
        all.entry(key).or_insert_with(|| Arc::new(Manager::new(svc.dir(), real_deps(svc, user, audit)))).clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn say(&self, action: &str, detail: &str) {
        (self.deps.audit)(&format!("domains.{action}"), detail);
    }

    fn now(&self) -> i64 {
        (self.deps.now)()
    }

    // -------------------------------------------------------------------------------------------- reading

    pub fn routes(&self) -> Vec<Route> {
        self.lock().file.routes.clone()
    }

    pub fn settings(&self) -> ListenerCfg {
        self.lock().file.listener.clone()
    }

    pub fn route(&self, host: &str, path: &str) -> Option<Route> {
        self.lock().file.routes.iter().find(|r| same(r, host, path)).cloned()
    }

    /// The Service namespace of a route, when it points at a cluster Service.
    pub fn namespace_of(&self, r: &Route) -> Option<String> {
        match &r.target {
            Target::Service { name, .. } => self.lock().file.service_ns.get(name).cloned(),
            Target::Local { .. } => None,
        }
    }

    /// The services found on the local cluster (the error is the plain-words reason when there are none to show).
    pub fn discover(&self) -> Result<Vec<Found>, String> {
        parse_services(&self.deps.kube.services()?)
    }

    pub fn own(&self) -> Vec<Own> {
        own_services((self.deps.web_port)())
    }

    pub fn status(&self) -> Status {
        let g = self.lock();
        let cfg = g.file.listener.clone();
        let (running, bind, ports, fw_ports, refused) = match &g.run {
            Some(r) => (true, r.bind.to_string(), vec![r.ports.0, r.ports.1], r.gate_ports.0.lock().unwrap_or_else(|e| e.into_inner()).clone(), r.listener.refused()),
            None => (false, String::new(), vec![], vec![], vec![]),
        };
        let mut last_errors: Vec<String> = self.log.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|l| is_problem(l)).cloned().collect();
        if let Some(e) = &g.start_error {
            last_errors.push(e.clone());
        }
        let n = last_errors.len();
        last_errors.drain(..n.saturating_sub(5));
        Status { enabled: cfg.enabled, running, bind, ports, firewall: cfg.firewall, firewall_until: cfg.firewall_until, firewall_ports: fw_ports, names: if running { local_names(&g.file.routes) } else { vec![] }, last_errors, refused }
    }

    // -------------------------------------------------------------------------------------------- routes

    /// The table the listener serves: a route with no port of its own listens on the http port (plain) or the https port.
    pub fn effective_table(file: &DomainsFile) -> RouteTable {
        let mut t = RouteTable { routes: file.routes.clone(), ingress_class: None };
        for r in &mut t.routes {
            if r.listen.is_none() {
                r.listen = Some(if r.scheme == Scheme::Http { file.listener.http_port } else { file.listener.https_port });
            }
        }
        t
    }

    fn check_table(&self, file: &DomainsFile) -> Result<(), String> {
        let t = Self::effective_table(file);
        t.check(&[])?;
        for r in &file.routes {
            if let Some(CertRef::Vault(n)) = &r.cert {
                if !self.cert_exists(n) {
                    return Err(format!("route {}: there is no certificate called \"{n}\" in your vault. Make a trial one or import yours first.", r.host));
                }
            }
            if let Target::Service { name, .. } = &r.target {
                if !file.service_ns.contains_key(name) {
                    return Err(format!("route {}: the namespace of the service \"{name}\" is not known; choose it from the services found in the cluster", r.host));
                }
            }
        }
        let ns: std::collections::BTreeSet<&String> = file.routes.iter().filter_map(|r| match &r.target { Target::Service { name, .. } => file.service_ns.get(name), _ => None }).collect();
        if ns.len() > 1 {
            return Err(format!("routes to services of different namespaces ({}) cannot be served together yet; keep them in one namespace", ns.into_iter().cloned().collect::<Vec<_>>().join(", ")));
        }
        Ok(())
    }

    /// Add a route, or replace the one with the same host and path (`replace`: the key of the route being edited). Checked with the
    /// engine's own checker; nothing changes when it refuses. `namespace` is needed for a `Target::Service`.
    pub fn save_route(&self, replace: Option<(&str, &str)>, mut route: Route, namespace: Option<&str>) -> Result<(), String> {
        route.host = route.host.trim().to_ascii_lowercase();
        let mut g = self.lock();
        let mut file = g.file.clone();
        if let Some((h, p)) = replace {
            if !file.routes.iter().any(|r| same(r, h, p)) {
                return Err("that route is gone".into());
            }
            file.routes.retain(|r| !same(r, h, p));
        }
        if file.routes.iter().any(|r| same(r, &route.host, &route.path_prefix)) {
            return Err(format!("there is already a route for {}{}; edit that one", route.host, route.path_prefix));
        }
        if let (Target::Service { name, .. }, Some(ns)) = (&route.target, namespace) {
            file.service_ns.insert(name.clone(), ns.to_string());
        }
        file.routes.push(route.clone());
        self.check_table(&file)?;
        save(&self.dir, &file)?;
        g.file = file;
        self.say("route.save", &format!("{}{} -> {}", route.host, route.path_prefix, target_text(&route.target)));
        drop(g);
        self.apply();
        Ok(())
    }

    pub fn remove_route(&self, host: &str, path: &str) -> Result<Route, String> {
        let mut g = self.lock();
        let Some(old) = g.file.routes.iter().find(|r| same(r, host, path)).cloned() else { return Err(format!("there is no route for {host}{path}")) };
        let mut file = g.file.clone();
        file.routes.retain(|r| !same(r, host, path));
        let used: Vec<String> = file.routes.iter().filter_map(|r| match &r.target { Target::Service { name, .. } => Some(name.clone()), _ => None }).collect();
        file.service_ns.retain(|k, _| used.contains(k));
        save(&self.dir, &file)?;
        g.file = file;
        self.say("route.remove", &format!("{}{}", old.host, old.path_prefix));
        drop(g);
        self.apply();
        Ok(old)
    }

    // -------------------------------------------------------------------------------------------- the listener

    /// Change the listener's settings. The firewall opening and the start are separate changes: a call that turns the listener on cannot
    /// also open the firewall (each is its own approval).
    pub fn set_listener(&self, c: ListenerChange) -> Result<String, String> {
        let mut g = self.lock();
        let mut cfg = g.file.listener.clone();
        if c.enabled == Some(true) && c.firewall == Some(true) && !cfg.enabled {
            return Err("turning the listener on and opening the firewall are two steps with an approval each: turn it on first, then open the firewall".into());
        }
        if let Some(p) = c.http_port {
            cfg.http_port = p;
        }
        if let Some(p) = c.https_port {
            cfg.https_port = p;
        }
        for (what, p) in [("http", cfg.http_port), ("https", cfg.https_port)] {
            if p == 0 {
                return Err(format!("the {what} port is a number from 1024 to 65535 (or 80 and 443 after approval)"));
            }
            if p < 1024 && !((what == "http" && p == STANDARD_HTTP) || (what == "https" && p == STANDARD_HTTPS)) {
                return Err(format!("port {p} is below 1024 and is not the standard {what} port; use 1024 or more, or {} after approval", if what == "http" { STANDARD_HTTP } else { STANDARD_HTTPS }));
            }
            if p < 1024 && !c.standard_ports_approved && (c.http_port.is_some() || c.https_port.is_some() || c.enabled == Some(true)) {
                return Err(format!("port {p} (the standard web port) needs your approval: ports below 1024 need the right to bind them, and anything on this computer that listens there is replaced in your browser's eyes"));
            }
        }
        if cfg.http_port == cfg.https_port {
            return Err("the http and https ports must differ".into());
        }
        let held = (self.deps.held)();
        for p in [cfg.http_port, cfg.https_port] {
            if held.contains(&p) {
                return Err(format!("port {p} is used by another service of this device; choose another"));
            }
        }
        if let Some(on) = c.enabled {
            cfg.enabled = on;
            if !on {
                cfg.firewall = false;
                cfg.firewall_until = 0;
            }
        }
        if let Some(open) = c.firewall {
            if open && !cfg.enabled {
                return Err("the listener is off; turn it on before opening the firewall".into());
            }
            cfg.firewall = open;
            cfg.firewall_until = match (open, c.firewall_hours) {
                (true, Some(h)) if h > 0 => self.now() + i64::from(h.min(24 * 365)) * 3600,
                _ => 0,
            };
        }
        let mut file = g.file.clone();
        file.listener = cfg.clone();
        if cfg.enabled {
            self.check_table(&file)?;
        }
        save(&self.dir, &file)?;
        g.file = file;
        g.start_error = None;
        let detail = format!("enabled {}, http {}, https {}, firewall {}{}", cfg.enabled, cfg.http_port, cfg.https_port, cfg.firewall, if cfg.firewall_until > 0 { format!(" until {}", cfg.firewall_until) } else { String::new() });
        self.say("listener.set", &detail);
        drop(g);
        self.apply();
        let st = self.status();
        Ok(st.line())
    }

    /// Make the running listener match the saved settings: start, restart (a port or the bind changed), update the table, or stop.
    fn apply(&self) {
        let mut g = self.lock();
        let file = g.file.clone();
        let want = &file.listener;
        if !want.enabled {
            self.stop_locked(&mut g);
            return;
        }
        let bind: IpAddr = if want.firewall { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
        let table = Self::effective_table(&file);
        if let Some(r) = &g.run {
            if r.bind == bind && r.firewall == want.firewall && r.ports == (want.http_port, want.https_port) {
                match r.listener.update(table) {
                    Ok(()) => {
                        g.start_error = None;
                        let _ = self.deps.names.set(&local_names(&file.routes));
                        return;
                    }
                    Err(e) => {
                        g.start_error = Some(e);
                        return;
                    }
                }
            }
        }
        self.stop_locked(&mut g);
        if table.routes.is_empty() {
            let _ = self.deps.names.set(&[]);
            return;
        }
        match self.start(&file, table, bind) {
            Ok(run) => {
                g.run = Some(run);
                g.start_error = None;
                let _ = self.deps.names.set(&local_names(&file.routes));
                self.say("listener.start", &format!("{bind} http {} https {}", want.http_port, want.https_port));
            }
            Err(e) => {
                self.say("listener.failed", &e);
                g.start_error = Some(e);
            }
        }
    }

    fn start(&self, file: &DomainsFile, table: RouteTable, bind: IpAddr) -> Result<Run, String> {
        let want = &file.listener;
        let open = Arc::new(FirewallState::default());
        let gate: Arc<dyn PortGate> = if want.firewall {
            let inner = self.deps.gates.make(GateReach::LocalNetwork, want.firewall_until).map_err(|e| format!("the firewall is not available, so the listener does not open to the network: {e}"))?;
            Arc::new(RecordingGate { inner, open: open.clone() })
        } else {
            Arc::new(NoGate)
        };
        let mut o = Options { bind, redirect_port: want.http_port, cert_reload: Some(Duration::from_secs(60)), ..Options::default() };
        let log = self.log.clone();
        o.log = Arc::new(move |l| {
            let mut q = log.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= MAX_ERRORS {
                q.pop_front();
            }
            q.push_back(l.chars().take(300).collect());
        });
        let spaces: std::collections::BTreeSet<&String> = table.routes.iter().filter_map(|r| match &r.target { Target::Service { name, .. } => file.service_ns.get(name), _ => None }).collect();
        if let Some(ns) = spaces.into_iter().next() {
            o.dialer = Some((self.deps.dialer)(ns)?);
        }
        let source = Arc::new(VaultCerts::new(Box::new(Reader(self.deps.secrets.clone()))));
        let listener = Listener::start(table, source, gate, o)?;
        Ok(Run { listener, bind, firewall: want.firewall, ports: (want.http_port, want.https_port), gate_ports: open })
    }

    fn stop_locked(&self, g: &mut Inner) {
        if let Some(r) = g.run.take() {
            r.listener.stop();
            self.say("listener.stop", "the ports are closed and the firewall opening is removed");
        }
        let _ = self.deps.names.set(&[]);
    }

    /// Sign-in: start the listener if the person left it on. A failure is kept for the page; it does not stop the sign-in.
    pub fn resume(&self) {
        if self.lock().file.listener.enabled {
            self.apply();
        }
    }

    /// Sign-out: stop the listener (the vault closes with the session, so the certificates could not be read any more).
    pub fn suspend(&self) {
        if let Some(stop) = self.ticker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let mut g = self.lock();
        self.stop_locked(&mut g);
    }

    /// Called every few seconds: ends an expired firewall opening (the listener is switched off with it), and checks the certificates now
    /// and then. Returns what it did, for the page.
    pub fn tick(&self) -> Vec<String> {
        let now = self.now();
        let mut done = Vec::new();
        let expired = {
            let g = self.lock();
            g.file.listener.enabled && g.file.listener.firewall && g.file.listener.firewall_until > 0 && now >= g.file.listener.firewall_until
        };
        if expired {
            if self.set_listener(ListenerChange { enabled: Some(false), ..Default::default() }).is_ok() {
                (self.deps.notify)("Domain names: the firewall opening ended", "The time you gave for opening the domain listener to the local network passed, so the listener was switched off and the firewall rule removed. Turn it on in Settings, Domains when you need it again.");
                done.push("the firewall opening ended; the listener is off".to_string());
            }
        }
        let due = {
            let mut g = self.lock();
            let due = now - g.last_cert_check >= CERT_CHECK_EVERY;
            if due {
                g.last_cert_check = now;
            }
            due
        };
        if due {
            done.extend(self.check_certs());
        }
        done
    }

    /// Warn (inbox) about a certificate of a route that has run out or runs out within 30 days. Returns the warnings.
    pub fn check_certs(&self) -> Vec<String> {
        let now = self.now();
        let mut out = Vec::new();
        for r in self.routes() {
            let Some(CertRef::Vault(n)) = &r.cert else { continue };
            let problem = match self.cert_pem(n) {
                Err(e) => Some(format!("cannot be read: {e}")),
                Ok((chain, key)) => match check_certificate(&chain, &key, &r.host, now) {
                    Err(e) => Some(e),
                    Ok((info, _)) if (info.not_after - now) / 86400 <= WARN_DAYS => Some(format!("runs out in {} day(s)", (info.not_after - now) / 86400)),
                    Ok(_) => None,
                },
            };
            if let Some(p) = problem {
                let title = format!("Certificate for {} needs attention", r.host);
                (self.deps.notify)(&title, &format!("The certificate \"{n}\" of {}: {p}. Renew it in Settings, Domains (the page can make a trial one or import yours).", r.host));
                out.push(format!("{}: {p}", r.host));
            }
        }
        out
    }

    // -------------------------------------------------------------------------------------------- certificates in the vault

    fn cert_exists(&self, name: &str) -> bool {
        self.deps.secrets.names().map(|v| v.iter().any(|n| *n == format!("{VAULT_PREFIX}{name}-chain"))).unwrap_or(false)
    }

    fn cert_pem(&self, name: &str) -> Result<(String, String), String> {
        let chain = self.deps.secrets.get(&format!("{VAULT_PREFIX}{name}-chain"))?;
        let key = self.deps.secrets.get(&format!("{VAULT_PREFIX}{name}-key"))?;
        Ok((chain.to_string(), key.to_string()))
    }

    /// The certificates in the vault with what they cover and when they run out.
    pub fn certs(&self) -> Result<Vec<CertRow>, String> {
        let routes = self.routes();
        let mut names: Vec<String> = self.deps.secrets.names()?.into_iter().filter_map(|n| n.strip_prefix(VAULT_PREFIX).and_then(|x| x.strip_suffix("-chain")).map(str::to_string)).collect();
        names.sort();
        let now = self.now();
        Ok(names
            .into_iter()
            .map(|name| {
                let used_by: Vec<String> = routes.iter().filter(|r| r.cert == Some(CertRef::Vault(name.clone()))).map(|r| r.host.clone()).collect();
                match self.cert_pem(&name).and_then(|(c, _)| first_cert(&c)) {
                    Ok(i) => CertRow { name, names: i.names, days: (i.not_after - now).div_euclid(86400), used_by, problem: None },
                    Err(e) => CertRow { name, names: vec![], days: 0, used_by, problem: Some(e) },
                }
            })
            .collect())
    }

    fn put_cert(&self, name: &str, chain: &str, key: &str) -> Result<(), String> {
        if !cert_name_ok(name) {
            return Err(format!("\"{name}\" is not a certificate name (lower-case letters, digits and -, at most 40)"));
        }
        self.deps.secrets.set(&format!("{VAULT_PREFIX}{name}-chain"), chain)?;
        if let Err(e) = self.deps.secrets.set(&format!("{VAULT_PREFIX}{name}-key"), key) {
            let _ = self.deps.secrets.delete(&format!("{VAULT_PREFIX}{name}-chain"));
            return Err(e);
        }
        Ok(())
    }

    /// A self-signed certificate for `host`, kept in the vault, for a trial. Browsers do not trust it and show a warning. Returns its name.
    pub fn make_trial_cert(&self, host: &str) -> Result<String, String> {
        let host = host.trim().to_ascii_lowercase();
        let (chain, key) = crate::cloudroute::self_signed(&host)?;
        let name = cert_name_for(&host);
        self.put_cert(&name, &chain, &key)?;
        self.say("cert.trial", &format!("{name} for {host}"));
        self.reload_certs();
        Ok(name)
    }

    /// Keep a certificate the person has (PEM chain and key) in the vault, after the engine's own check against `host`.
    pub fn import_cert(&self, name: &str, host: &str, chain_pem: &str, key_pem: &str) -> Result<String, String> {
        let (_, line) = check_certificate(chain_pem, key_pem, host.trim(), self.now())?;
        self.put_cert(name, chain_pem, key_pem)?;
        self.say("cert.import", &format!("{name} for {host}"));
        self.reload_certs();
        Ok(line)
    }

    pub fn delete_cert(&self, name: &str) -> Result<(), String> {
        if let Some(r) = self.routes().iter().find(|r| r.cert == Some(CertRef::Vault(name.to_string()))) {
            return Err(format!("the certificate \"{name}\" is used by the route {}; change or remove that route first", r.host));
        }
        self.deps.secrets.delete(&format!("{VAULT_PREFIX}{name}-chain"))?;
        self.deps.secrets.delete(&format!("{VAULT_PREFIX}{name}-key"))?;
        self.say("cert.delete", name);
        Ok(())
    }

    fn reload_certs(&self) {
        let lines = self.lock().run.as_ref().map(|r| r.listener.reload_certs()).unwrap_or_default();
        let mut q = self.log.lock().unwrap_or_else(|e| e.into_inner());
        for l in lines {
            if q.len() >= MAX_ERRORS {
                q.pop_front();
            }
            q.push_back(l);
        }
    }

    /// The text of the problem lines for a route with a certificate (for the page): the engine's own check.
    pub fn cert_state(&self, r: &Route) -> Option<String> {
        let CertRef::Vault(n) = r.cert.as_ref()? else { return None };
        Some(match self.cert_pem(n).and_then(|(c, k)| check_certificate(&c, &k, &r.host, self.now())) {
            Ok((_, line)) => line,
            Err(e) => e,
        })
    }
}

fn is_problem(l: &str) -> bool {
    let l = l.to_ascii_lowercase();
    ["cannot", "refused", "fail", "error", "expired", "invalid", "not served", "no certificate", "502", "denied"].iter().any(|w| l.contains(w))
}

pub fn target_text(t: &Target) -> String {
    match t {
        Target::Local { port } => format!("port {port} on this device"),
        Target::Service { name, port } => format!("service {name}:{port}"),
    }
}

pub fn access_text(a: Access) -> &'static str {
    match a {
        Access::Members => "people of this device (not enforced by the listener)",
        Access::Public => "anyone who can reach the port",
    }
}

// ------------------------------------------------------------------------------------------------------------ the real world

/// **Sign-in hook.** Starts the listener if the person left it on (their vault is open now) and the thread that keeps the checks going.
pub fn start_for_login(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) {
    let m = Manager::shared(svc, user, audit);
    if m.lock().file.listener.enabled || !m.lock().file.routes.is_empty() {
        m.resume();
        m.ensure_ticker();
    }
}

/// **Sign-out hook.** Stops the listener and the checks (the vault closes with the session).
pub fn on_logout(svc: &Arc<crate::services::Services>, user: &str) {
    let m = registry().lock().unwrap_or_else(|e| e.into_inner()).get(&format!("{}|{user}", svc.dir().display())).cloned();
    if let Some(m) = m {
        m.suspend();
    }
}

impl Manager {
    /// Starts the checking thread once. It holds only a weak link, so it ends with the manager or when [`Manager::suspend`] is called.
    pub fn ensure_ticker(self: &Arc<Manager>) {
        let mut t = self.ticker.lock().unwrap_or_else(|e| e.into_inner());
        if t.is_some() {
            return;
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        *t = Some(stop.clone());
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("iyakki-domains".into()).spawn(move || loop {
            for _ in 0..30 {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            match weak.upgrade() {
                Some(m) => {
                    m.tick();
                }
                None => return,
            }
        });
    }
}

/// The real dependencies of a signed-in person.
pub fn real_deps(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Deps {
    let (s1, u1) = (svc.clone(), user.to_string());
    let dir = svc.dir().to_path_buf();
    let kubeconfig = svc.k3sdev.layout().kubeconfig();
    let held_dir = dir.clone();
    let web_dir = dir.clone();
    Deps {
        kube: Arc::new(K3sKube(svc.k3sdev.clone())),
        secrets: Arc::new(crate::gitkeys::ServicesStore::new(svc, user)),
        gates: Arc::new(RealGates),
        names: Arc::new(FileNames(dir)),
        notify: Arc::new(move |title, text| {
            let _ = s1.notify_once(&u1, crate::inbox::Kind::Warning, title, text, None);
        }),
        dialer: Arc::new(move |ns| {
            let kc = kubeconfig.display().to_string();
            Ok(Arc::new(crate::cloudlisten::pod::PortForward::new(ns, Some(&kc), None)?) as Arc<dyn ServiceDialer>)
        }),
        held: Arc::new(move || {
            let st = crate::webaccess::read_status(&held_dir);
            let mut v = vec![7979, 7980];
            if st.listening && st.port != 0 {
                v.push(st.port);
            }
            v
        }),
        web_port: Arc::new(move || Some(crate::webaccess::read_status(&web_dir)).filter(|s| s.listening && s.port != 0).map(|s| s.port)),
        audit,
        now: Arc::new(crate::clock::now_unix),
    }
}

#[cfg(test)]
#[path = "domains_tests.rs"]
pub(crate) mod domains_tests_support;
