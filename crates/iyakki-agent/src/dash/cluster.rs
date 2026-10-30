//! Services of the managed cluster as DASH addresses. The Elasticsearch and Prometheus that Quick install puts in the cluster are
//! `ClusterIP` Services: their `*.svc` names resolve only inside the cluster and nothing listens on this machine's localhost. A source
//! may therefore be written as
//!
//! * `cluster://<namespace>/<service>:<port>` (`cluster+https://` for TLS), or
//! * `http(s)://<service>.<namespace>.svc[.cluster.local]:<port>`,
//!
//! and is turned into the Service's CURRENT cluster IP each time a connector is built (through the managed `kubectl`, cached for a
//! few seconds, forgotten after a failed connect). k3s runs natively here, so a cluster IP is reachable from this machine.
//!
//! What the policy sees is the NAME (`<service>.<namespace>.svc`), never the IP: an account that may not reach that name cannot use the
//! source, and an IP address typed by hand is still checked as itself. Only an IP that the cluster's own API gave for a ClusterIP Service
//! (not loopback, link-local, multicast, unspecified; not an ExternalName or headless Service) is ever connected to, by IP literal, so there
//! is no DNS to rebind, and the HTTP client follows no redirect.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::connector::{DashError, ErrKind};

/// Every "nothing listens here" error contains this; the screen offers its repair button on it.
pub const LOCALHOST_MARK: &str = "nothing listens on this machine";
pub const ES_NAMESPACE: &str = "iyakki-data";
pub const PROM_NAMESPACE: &str = "iyakki-monitoring";

/// A Service named in a source address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterRef {
    pub ns: String,
    pub svc: String,
    pub port: Option<u16>,
    pub https: bool,
    /// What follows the port in the address (`/prometheus`), kept as written.
    pub tail: String,
}

impl ClusterRef {
    /// The host name the account's network policy is asked about.
    pub fn policy_host(&self) -> String {
        format!("{}.{}.svc", self.svc, self.ns)
    }
    /// The friendly spelling.
    pub fn friendly(&self) -> String {
        format!("{}://{}/{}{}{}", if self.https { "cluster+https" } else { "cluster" }, self.ns, self.svc, self.port.map_or(String::new(), |p| format!(":{p}")), self.tail)
    }
}

fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !s.starts_with('-') && !s.ends_with('-')
}

fn split_port(s: &str) -> Result<(&str, Option<u16>), String> {
    match s.rsplit_once(':') {
        Some((n, p)) => match p.parse::<u16>() {
            Ok(p) if p > 0 => Ok((n, Some(p))),
            _ => Err(format!("`{p}` is not a port")),
        },
        None => Ok((s, None)),
    }
}

/// `None`: not a cluster address (an ordinary one). `Some(Err)`: meant as one but written wrong.
pub fn parse(url: &str) -> Option<Result<ClusterRef, String>> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let (end, tail) = match rest.find(['/', '?', '#']) {
        Some(i) if scheme.starts_with("cluster") => {
            // cluster://ns/svc[:port][/more]: the first '/' separates the namespace
            let after = &rest[i..];
            if after.starts_with('/') {
                let rest2 = &after[1..];
                let j = rest2.find(['/', '?', '#']).unwrap_or(rest2.len());
                (format!("{}/{}", &rest[..i], &rest2[..j]), rest2[j..].to_string())
            } else {
                (rest.to_string(), String::new())
            }
        }
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), String::new()),
    };
    let bad = |m: String| Some(Err(m));
    match scheme.as_str() {
        "cluster" | "cluster+https" => {
            if end.contains('@') {
                return bad("do not put a user name or a password in the address".into());
            }
            let Some((ns, svc_port)) = end.split_once('/') else { return bad("write it as cluster://<namespace>/<service>:<port>, for example cluster://iyakki-data/elasticsearch:9200".into()) };
            let (svc, port) = match split_port(svc_port) {
                Ok(x) => x,
                Err(e) => return bad(e),
            };
            if !label_ok(ns) || !label_ok(svc) {
                return bad(format!("`{ns}/{svc}` is not a namespace and Service name (lowercase letters, digits and -)"));
            }
            Some(Ok(ClusterRef { ns: ns.into(), svc: svc.into(), port, https: scheme.ends_with("https"), tail }))
        }
        "http" | "https" => {
            let host_port = end.as_str();
            if host_port.contains('@') {
                return None;
            }
            let (host, port) = match split_port(host_port) {
                Ok(x) => x,
                Err(_) => return None,
            };
            let host = host.to_ascii_lowercase();
            let name = host.strip_suffix(".svc.cluster.local").or_else(|| host.strip_suffix(".svc"))?;
            let labels: Vec<&str> = name.split('.').collect();
            if labels.len() != 2 || !labels.iter().all(|l| label_ok(l)) {
                return bad(format!("`{host}` should be <service>.<namespace>.svc"));
            }
            Some(Ok(ClusterRef { ns: labels[1].into(), svc: labels[0].into(), port, https: scheme == "https", tail }))
        }
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------------ the cluster

/// One Service as the cluster reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvcInfo {
    pub ns: String,
    pub name: String,
    pub kind: String,
    pub cluster_ip: String,
    pub ports: Vec<(String, u16)>,
    pub app: String,
    /// Some pod is ready behind it (`None`: could not be told).
    pub ready: Option<bool>,
}

/// What DASH asks of the cluster. Tests use a fake.
pub trait ClusterApi: Send + Sync {
    fn service(&self, ns: &str, name: &str) -> Result<SvcInfo, DashError>;
    fn services(&self) -> Result<Vec<SvcInfo>, DashError>;
    /// Drop what was remembered about a Service (its connection just failed, its IP may have changed).
    fn forget(&self, _ns: &str, _name: &str) {}
    /// Does a process on this machine accept connections on that port of localhost?
    fn host_listens(&self, port: u16) -> bool;
    /// One of the fixed, read-only looks at the cluster (see [`KubeRead`]) as the JSON text the API gave. The `kube` data source is the only
    /// caller; what can be asked is this enum and nothing else, so no text from a person or a model ever reaches kubectl.
    fn kube_read(&self, _what: &KubeRead) -> Result<String, DashError> {
        Err(no_cluster("there is no cluster here"))
    }
}

/// The looks at the cluster the `kube` data source may take. Every one is a `get` (or `version`); none names a Secret, a log or an exec.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KubeRead {
    Version,
    Nodes,
    Pods,
    /// Warning events of all namespaces.
    Events,
    Deployments,
    Services,
    Pvcs,
    /// metrics-server (`metrics.k8s.io`), shipped by k3s.
    NodeMetrics,
    PodMetrics,
    /// The kubelet's volume statistics of one node (through the API server's node proxy).
    NodeStats(String),
}

impl KubeRead {
    /// The arguments given to kubectl, or why this look is refused (a node name that is not a name).
    pub fn args(&self) -> Result<Vec<String>, DashError> {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Ok(match self {
            KubeRead::Version => a(&["version", "-o", "json"]),
            KubeRead::Nodes => a(&["get", "nodes", "-o", "json"]),
            KubeRead::Pods => a(&["get", "pods", "-A", "-o", "json"]),
            KubeRead::Events => a(&["get", "events", "-A", "--field-selector", "type=Warning", "-o", "json"]),
            KubeRead::Deployments => a(&["get", "deployments", "-A", "-o", "json"]),
            KubeRead::Services => a(&["get", "svc", "-A", "-o", "json"]),
            KubeRead::Pvcs => a(&["get", "pvc", "-A", "-o", "json"]),
            KubeRead::NodeMetrics => a(&["get", "--raw", "/apis/metrics.k8s.io/v1beta1/nodes"]),
            KubeRead::PodMetrics => a(&["get", "--raw", "/apis/metrics.k8s.io/v1beta1/pods"]),
            KubeRead::NodeStats(n) => {
                if n.is_empty() || n.len() > 253 || !n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') {
                    return Err(DashError::config("that is not a node name"));
                }
                vec!["get".into(), "--raw".into(), format!("/api/v1/nodes/{n}/proxy/stats/summary")]
            }
        })
    }

    fn is_metrics(&self) -> bool {
        matches!(self, KubeRead::NodeMetrics | KubeRead::PodMetrics | KubeRead::NodeStats(_))
    }
}

/// No cluster at all (tests, and builds without one).
pub struct NoCluster;

impl ClusterApi for NoCluster {
    fn service(&self, _: &str, _: &str) -> Result<SvcInfo, DashError> {
        Err(no_cluster("there is no cluster here"))
    }
    fn services(&self) -> Result<Vec<SvcInfo>, DashError> {
        Err(no_cluster("there is no cluster here"))
    }
    fn host_listens(&self, _: u16) -> bool {
        false
    }
}

pub fn no_cluster(why: &str) -> DashError {
    DashError::new(ErrKind::Network, format!("the cluster is not available: {why}")).fix("Cluster addresses (cluster://...) are looked up in IYAKKI's own k3s cluster. Install it and start it in Store (Quick install: k3s), then test the source again.")
}

/// `kubectl` against the managed cluster; `Err` is a plain sentence.
pub trait Kubectl: Send + Sync {
    fn kubectl(&self, args: &[&str]) -> Result<String, String>;
    /// Like [`Kubectl::kubectl`] for answers that can be large (every pod of the cluster).
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.kubectl(args)
    }
}

impl Kubectl for crate::k3sdev::K3sDev {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.checked(args, crate::k3sdev::KUBECTL_CAP)
    }
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.checked(args, crate::k3sdev::KUBECTL_BIG_CAP)
    }
}

impl crate::k3sdev::K3sDev {
    fn checked(&self, args: &[&str], cap: usize) -> Result<String, String> {
        use crate::k3sdev::Svc;
        match self.state() {
            Svc::NotInstalled => return Err("k3s is not installed by IYAKKI on this machine".into()),
            Svc::Running => {}
            other => return Err(format!("k3s is {}", other.word())),
        }
        let out = self.kubectl_cap(args, cap);
        if out.code == Some(0) { Ok(out.stdout) } else { Err(out.combined().trim().to_string()) }
    }
}

/// [`ClusterApi`] over a [`Kubectl`], with a short memory.
pub struct KubeCluster {
    kube: Arc<dyn Kubectl>,
    ttl: Duration,
    one: Mutex<HashMap<(String, String), (Instant, SvcInfo)>>,
    all: Mutex<Option<(Instant, Vec<SvcInfo>)>>,
    /// The `kube` data source's looks, one slot each so that panels asking at the same moment make one kubectl call.
    reads: Mutex<HashMap<KubeRead, Arc<Mutex<Option<(Instant, String)>>>>>,
}

impl KubeCluster {
    pub fn new(kube: Arc<dyn Kubectl>, ttl: Duration) -> KubeCluster {
        KubeCluster { kube, ttl, one: Mutex::new(HashMap::new()), all: Mutex::new(None), reads: Mutex::new(HashMap::new()) }
    }

    fn fail(e: &str) -> DashError {
        let l = e.to_ascii_lowercase();
        if l.starts_with("k3s is") || l.contains("refused") || l.contains("unable to connect") || l.contains("timeout") || l.contains("deadline") || l.contains("no such file") {
            no_cluster(&format!("{}", super::frame::clip(e.lines().next().unwrap_or(""), 160)))
        } else {
            DashError::new(ErrKind::Network, format!("kubectl said: {}", super::frame::clip(e.lines().next().unwrap_or(""), 200))).fix("Check the cluster (Store > k3s) and that this account may read Services.")
        }
    }
}

fn parse_svc(v: &Value) -> SvcInfo {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    let ports = v.pointer("/spec/ports").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| Some((p.get("name").and_then(Value::as_str).unwrap_or("").to_string(), u16::try_from(p.get("port")?.as_u64()?).ok()?))).collect()).unwrap_or_default();
    let app = ["/metadata/labels/app.kubernetes.io~1name", "/metadata/labels/app"].iter().map(|p| s(p)).find(|x| !x.is_empty()).unwrap_or_default();
    SvcInfo { ns: s("/metadata/namespace"), name: s("/metadata/name"), kind: s("/spec/type"), cluster_ip: s("/spec/clusterIP"), ports, app, ready: None }
}

fn endpoints_ready(v: &Value) -> bool {
    v.get("subsets").and_then(Value::as_array).is_some_and(|a| a.iter().any(|s| s.get("addresses").and_then(Value::as_array).is_some_and(|x| !x.is_empty())))
}

impl ClusterApi for KubeCluster {
    fn service(&self, ns: &str, name: &str) -> Result<SvcInfo, DashError> {
        let key = (ns.to_string(), name.to_string());
        if let Some((t, i)) = self.one.lock().unwrap().get(&key) {
            if t.elapsed() < self.ttl {
                return Ok(i.clone());
            }
        }
        let txt = match self.kube.kubectl(&["-n", ns, "get", "svc", name, "-o", "json"]) {
            Ok(t) => t,
            Err(e) if e.contains("NotFound") || e.contains("not found") => {
                return Err(DashError::config(format!("the cluster has no Service `{name}` in namespace `{ns}` (kubectl says: {})", super::frame::clip(e.lines().next().unwrap_or(""), 160))).fix(format!("List them with `kubectl get svc -A`. Quick install puts Elasticsearch at cluster://{ES_NAMESPACE}/elasticsearch:9200 and Prometheus at cluster://{PROM_NAMESPACE}/prometheus:9090 (install them in Store first).")));
            }
            Err(e) => return Err(Self::fail(&e)),
        };
        let v: Value = serde_json::from_str(&txt).map_err(|_| DashError::parse("kubectl did not answer with a Service"))?;
        let mut info = parse_svc(&v);
        info.ns = ns.into();
        info.name = name.into();
        info.ready = self.kube.kubectl(&["-n", ns, "get", "endpoints", name, "-o", "json"]).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| endpoints_ready(&v));
        self.one.lock().unwrap().insert(key, (Instant::now(), info.clone()));
        Ok(info)
    }

    fn services(&self) -> Result<Vec<SvcInfo>, DashError> {
        if let Some((t, l)) = &*self.all.lock().unwrap() {
            if t.elapsed() < self.ttl {
                return Ok(l.clone());
            }
        }
        let txt = self.kube.kubectl(&["get", "svc", "-A", "-o", "json"]).map_err(|e| Self::fail(&e))?;
        let v: Value = serde_json::from_str(&txt).map_err(|_| DashError::parse("kubectl did not answer with a list of Services"))?;
        let mut list: Vec<SvcInfo> = v.get("items").and_then(Value::as_array).map(|a| a.iter().map(parse_svc).collect()).unwrap_or_default();
        if let Some(ep) = self.kube.kubectl(&["get", "endpoints", "-A", "-o", "json"]).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            for s in &mut list {
                s.ready = ep.get("items").and_then(Value::as_array).and_then(|a| a.iter().find(|e| e.pointer("/metadata/name").and_then(Value::as_str) == Some(&s.name) && e.pointer("/metadata/namespace").and_then(Value::as_str) == Some(&s.ns))).map(endpoints_ready);
            }
        }
        *self.all.lock().unwrap() = Some((Instant::now(), list.clone()));
        Ok(list)
    }

    fn forget(&self, ns: &str, name: &str) {
        self.one.lock().unwrap().remove(&(ns.to_string(), name.to_string()));
        *self.all.lock().unwrap() = None;
    }

    fn kube_read(&self, what: &KubeRead) -> Result<String, DashError> {
        let args = what.args()?;
        let slot = self.reads.lock().unwrap().entry(what.clone()).or_default().clone();
        let mut slot = slot.lock().unwrap();
        if let Some((t, text)) = &*slot {
            if t.elapsed() < self.ttl {
                return Ok(text.clone());
            }
        }
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.kube.kubectl_big(&a) {
            Ok(text) => {
                *slot = Some((Instant::now(), text.clone()));
                Ok(text)
            }
            Err(e) => {
                *slot = None;
                let d = Self::fail(&e);
                if what.is_metrics() && !d.message.starts_with("the cluster is not available") {
                    return Err(DashError::new(ErrKind::Missing, format!("the metrics API is not answering ({})", super::frame::clip(e.lines().next().unwrap_or(""), 140))).fix("k3s ships metrics-server in kube-system; it needs a minute after the cluster starts. CPU and memory appear when it answers."));
                }
                Err(d)
            }
        }
    }

    fn host_listens(&self, port: u16) -> bool {
        listens(port)
    }
}

/// A real connect to this machine's loopback (refused answers at once).
pub fn listens(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(300)).is_ok()
}

// ------------------------------------------------------------------------------------------------------ resolving

fn usable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => !(a.is_loopback() || a.is_unspecified() || a.is_link_local() || a.is_multicast() || a.is_broadcast()),
        IpAddr::V6(a) => !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || (a.segments()[0] & 0xffc0) == 0xfe80),
    }
}

/// The address to connect to (`http://10.43.1.2:9200/tail`) for a cluster reference, with the Service's own port mapping checked.
pub fn resolve(api: &dyn ClusterApi, r: &ClusterRef) -> Result<String, DashError> {
    let s = api.service(&r.ns, &r.svc)?;
    if s.kind == "ExternalName" {
        return Err(DashError::config(format!("`{}` is an ExternalName Service (it points outside the cluster)", r.friendly())).fix("Only Services with a cluster IP are used here. Put the external address into the source instead."));
    }
    let ip: IpAddr = match s.cluster_ip.parse() {
        Ok(ip) if usable_ip(ip) => ip,
        _ => {
            return Err(DashError::config(format!("the Service {}/{} has no usable cluster IP (it says `{}`)", r.ns, r.svc, s.cluster_ip)).fix("A headless Service cannot be reached from this machine: use a Service with a cluster IP, or kubectl port-forward."));
        }
    };
    let port = match r.port {
        Some(p) if s.ports.iter().any(|x| x.1 == p) => p,
        Some(p) => {
            let have: Vec<String> = s.ports.iter().map(|x| x.1.to_string()).collect();
            return Err(DashError::config(format!("the Service {}/{} has no port {p} (it offers {})", r.ns, r.svc, if have.is_empty() { "none".into() } else { have.join(", ") })).fix("Use one of its ports in the address."));
        }
        None if s.ports.len() == 1 => s.ports[0].1,
        None => return Err(DashError::config(format!("the Service {}/{} has several ports; add the one you want, for example {}:{}", r.ns, r.svc, r.friendly(), s.ports.first().map_or(0, |x| x.1)))),
    };
    if s.ready == Some(false) {
        return Err(DashError::new(ErrKind::Network, format!("the Service {}/{} exists but has no ready pod behind it", r.ns, r.svc)).fix(format!("Look at `kubectl -n {} get pods`: a pod that is still starting (the first start pulls an image) or crashing explains it. Test again when it is Running.", r.ns)));
    }
    let host = match ip {
        IpAddr::V4(a) => a.to_string(),
        IpAddr::V6(a) => format!("[{a}]"),
    };
    Ok(format!("{}://{host}:{port}{}", if r.https { "https" } else { "http" }, r.tail.trim_end_matches('/')))
}

// ------------------------------------------------------------------------------------------------------ finding and suggesting

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: &'static str,
    pub ns: String,
    pub name: String,
    pub port: u16,
    pub ready: bool,
}

impl Candidate {
    pub fn address(&self) -> String {
        format!("cluster://{}/{}:{}", self.ns, self.name, self.port)
    }
}

pub fn default_port(kind: &str) -> Option<u16> {
    match kind {
        "prometheus" => Some(9090),
        "elasticsearch" => Some(9200),
        "clickhouse" => Some(8123),
        _ => None,
    }
}

pub fn kind_of_port(port: u16) -> Option<&'static str> {
    match port {
        9090 => Some("prometheus"),
        9200 => Some("elasticsearch"),
        8123 | 9000 => Some("clickhouse"),
        _ => None,
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "prometheus" => "Prometheus",
        "elasticsearch" => "Elasticsearch",
        _ => "ClickHouse",
    }
}

/// Services that look like a data source of some kind, by name, label and port.
pub fn candidates(list: &[SvcInfo]) -> Vec<Candidate> {
    let mut out = vec![];
    for s in list {
        if s.kind == "ExternalName" || s.cluster_ip.is_empty() || s.cluster_ip == "None" {
            continue;
        }
        let id = format!("{} {}", s.name, s.app).to_ascii_lowercase();
        let has = |p: u16| s.ports.iter().any(|x| x.1 == p);
        let kind = if id.contains("clickhouse") && has(8123) {
            Some(("clickhouse", 8123))
        } else if (id.contains("elasticsearch") || id.contains("opensearch")) && has(9200) {
            Some(("elasticsearch", 9200))
        } else if id.contains("prometheus") && !["exporter", "alertmanager", "pushgateway", "operator", "state-metrics", "adapter"].iter().any(|x| id.contains(x)) && has(9090) {
            Some(("prometheus", 9090))
        } else {
            None
        };
        if let Some((kind, port)) = kind {
            out.push(Candidate { kind, ns: s.ns.clone(), name: s.name.clone(), port, ready: s.ready.unwrap_or(true) });
        }
    }
    // IYAKKI's own namespaces first, then ready ones
    out.sort_by_key(|c| (!c.ns.starts_with("iyakki-"), !c.ready, c.ns.clone(), c.name.clone()));
    out
}

/// What the Add source form pre-fills for the address of a source of `kind`, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Empty when no instance was found.
    pub url: String,
    pub note: String,
}

/// Built from what is really reachable: a Service in the cluster (stable cluster address), else a process listening on this
/// machine (localhost), else nothing, with the reason. Never a guess.
pub fn suggest(api: &dyn ClusterApi, kind: &str) -> Option<Suggestion> {
    let port = default_port(kind)?;
    let found = api.services().map(|l| candidates(&l)).map(|c| c.into_iter().filter(|c| c.kind == kind).collect::<Vec<_>>());
    if let Ok(c) = &found {
        if let Some(best) = c.first() {
            let note = if best.ready { format!("found in the cluster: {}/{}", best.ns, best.name) } else { format!("found in the cluster ({}/{}) but no pod is ready yet", best.ns, best.name) };
            return Some(Suggestion { url: best.address(), note });
        }
    }
    if api.host_listens(port) {
        return Some(Suggestion { url: format!("http://localhost:{port}"), note: format!("something listens on this machine at port {port}") });
    }
    let why = match &found {
        Err(e) => format!("not found: {}", e.message.trim_end_matches('.')),
        Ok(_) => format!("no {} found in the cluster and nothing listens on this machine at port {port}", kind_label(kind)),
    };
    Some(Suggestion { url: String::new(), note: format!("{why}. Install it in Store (Quick install), or type its address; in-cluster ones are written cluster://<namespace>/<service>:{port}") })
}

/// The sentence for a refused connection to this machine's own `port`: where the instance really is.
pub fn localhost_refused(what: &str, port: u16, found: &[Candidate]) -> DashError {
    let kind = kind_of_port(port);
    let here: Vec<&Candidate> = found.iter().filter(|c| Some(c.kind) == kind).collect();
    let fix = match (kind, here.first()) {
        (_, Some(c)) => format!("Edit the source and use {}: the {} found in the cluster runs INSIDE the cluster, not on this machine.", c.address(), kind_label(c.kind)),
        (Some("elasticsearch"), None) => format!("Edit the source and use cluster://{ES_NAMESPACE}/elasticsearch:9200 (the source called 'Cluster Elasticsearch' has it): the Elasticsearch from Quick install runs INSIDE the cluster. If you run your own, start it on this machine first."),
        (Some("prometheus"), None) => format!("Edit the source and use cluster://{PROM_NAMESPACE}/prometheus:9090 (the source called 'Cluster Prometheus' has it): the Prometheus from Quick install runs INSIDE the cluster. If you run your own, start it on this machine first."),
        (Some(_), None) => format!("Edit the source and use cluster://{ES_NAMESPACE}/clickhouse:8123 (the source called 'Cluster ClickHouse' has it) once ClickHouse is installed from Quick install: it runs INSIDE the cluster. A ClickHouse of your own is written cluster://<namespace>/<service>:8123 (see `kubectl get svc -A`), or start it on this machine first."),
        (None, None) => "Start the server on this machine, or use its real address. A Service of the cluster is written cluster://<namespace>/<service>:<port>.".to_string(),
    };
    DashError::new(ErrKind::Network, format!("{what}: nothing listens on this machine at port {port}")).fix(fix)
}

/// The port and what follows it when `url` points at this machine's own loopback.
pub fn local_target(url: &str) -> Option<(u16, String)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let (auth, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, String::new()),
    };
    let (host, port) = auth.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?;
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return None;
    }
    Some((port.parse().ok()?, tail))
}

/// The cluster address a source that points at localhost should use instead: the best Service of the source's own kind. `Err` is a plain
/// sentence. Nothing is changed here.
pub fn repair_url(cfg: &super::connector::SourceConfig, api: &dyn ClusterApi) -> Result<String, String> {
    let Some((_, tail)) = local_target(cfg.value("url")) else { return Err("this source does not point at this machine's localhost".into()) };
    if default_port(&cfg.kind).is_none() {
        return Err(format!("there is no cluster address to offer for a {} source", cfg.kind));
    }
    let list = api.services().map_err(|e| e.message)?;
    let best = candidates(&list).into_iter().find(|c| c.kind == cfg.kind);
    match best {
        Some(c) => Ok(format!("{}{}", c.address(), tail.trim_end_matches('/'))),
        None => Err(format!("no {} Service was found in the cluster (install it in Store, or type its address)", kind_label(&cfg.kind))),
    }
}

/// The sentence for a `.svc` (or other cluster-only) host name that could not be found.
pub fn svc_name_unresolvable(what: &str, host: &str) -> DashError {
    DashError::new(ErrKind::Network, format!("{what}: the name {host} could not be found")).fix("Names ending in .svc only work inside the cluster. IYAKKI can look them up for you in a DASH source: write cluster://<namespace>/<service>:<port> (for example cluster://iyakki-data/elasticsearch:9200).")
}

/// What can be done for a source that points at this machine's localhost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairPlan {
    /// The Service is in the cluster: this is its address.
    UseCluster(String),
    /// The cluster answers and has no such Service: it is not installed. `item` is the Setup item that installs it, if there is one.
    NotInstalled { label: &'static str, item: Option<&'static str> },
    /// The cluster could not be asked (not installed, not running).
    NoCluster(String),
    /// Not a source this applies to.
    NotLocal,
}

pub fn repair_plan(cfg: &super::connector::SourceConfig, api: &dyn ClusterApi) -> RepairPlan {
    if local_target(cfg.value("url")).is_none() || default_port(&cfg.kind).is_none() {
        return RepairPlan::NotLocal;
    }
    let item = match cfg.kind.as_str() {
        "elasticsearch" => Some("elasticsearch"),
        "prometheus" => Some("prometheus"),
        "clickhouse" => Some("clickhouse"),
        _ => None,
    };
    match repair_url(cfg, api) {
        Ok(u) => RepairPlan::UseCluster(u),
        // something answers on this machine's port: not "not installed", leave it to the source's own error
        Err(_) if local_target(cfg.value("url")).is_some_and(|(p, _)| api.host_listens(p)) => RepairPlan::NotLocal,
        Err(_) => match api.services() {
            // IYAKKI's own cluster is not installed at all: the service cannot be there either (installing it brings k3s along)
            Err(e) if item.is_some() && e.message.contains("k3s is not installed") => RepairPlan::NotInstalled { label: kind_label(&cfg.kind), item },
            Err(e) => RepairPlan::NoCluster(e.message),
            Ok(_) => RepairPlan::NotInstalled { label: kind_label(&cfg.kind), item },
        },
    }
}

/// Keep a source that the older Quick install saved with a baked-in cluster IP working: rewritten to the stable form, once.
/// Looks at the account's own file only (never copies a shared source into it).
pub fn migrate_sources(store: &super::store::Sources) -> bool {
    let mut changed = false;
    for mut s in store.own() {
        let want = match s.id.as_str() {
            "prometheus-cluster" => ("prometheus", format!("cluster://{PROM_NAMESPACE}/prometheus:9090")),
            _ => continue,
        };
        let url = s.value("url").to_string();
        let raw_ip = url.strip_prefix("http://").and_then(|r| r.rsplit_once(':')).is_some_and(|(h, _)| h.parse::<IpAddr>().is_ok());
        if s.kind == want.0 && raw_ip {
            s.values.insert("url".into(), want.1);
            changed |= store.save(s).is_ok();
        }
    }
    changed
}

/// The migration for the account's file and for the shared one.
pub fn migrate_all(data_dir: &std::path::Path, user: &str) -> bool {
    let a = migrate_sources(&super::store::Sources::open(data_dir, user));
    let b = user != "system" && migrate_sources(&super::store::Sources::open(data_dir, "system"));
    a || b
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
