//! Read-only view of a Kubernetes (k3s) namespace for the AI IDE: workloads, services, ingresses, pods (with details and
//! logs), config map and secret NAMES, volume claims and nodes. It only runs `kubectl get`, `describe`-style reads and
//! `logs`; nothing is applied, deleted or scaled (that comes with the deploy flow, docs/K3S-DEV.md).
//!
//! `kubectl` is started through [`crate::wsexec`] (argument array, no shell, timeouts, output caps). Secrets are listed
//! with `-o name` only, so their values never reach this process. A [`Kube`] trait lets tests and the screenshots use a
//! fake cluster.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::wsexec::Out;

pub const REQUEST_TIMEOUT_SECS: u64 = 8;
pub const MAX_OUTPUT: usize = 4 * 1024 * 1024;
pub const LOG_TAIL: usize = 200;

/// Where kubectl runs: which cluster and namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub namespace: String,
    pub selector: String,
    /// A kubeconfig context (empty: the current one).
    pub context: String,
    /// A kubeconfig file (empty: `$KUBECONFIG` or the default).
    pub kubeconfig: String,
}

impl Target {
    fn base_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if !self.kubeconfig.is_empty() {
            a.push("--kubeconfig".into());
            a.push(self.kubeconfig.clone());
        }
        if !self.context.is_empty() {
            a.push("--context".into());
            a.push(self.context.clone());
        }
        a.push(format!("--request-timeout={REQUEST_TIMEOUT_SECS}s"));
        a
    }
}

/// Runs kubectl with these arguments (everything after `kubectl`).
pub trait Kube: Send + Sync {
    fn run(&self, args: &[String]) -> Out;
}

/// The real thing.
pub struct Kubectl;

impl Kube for Kubectl {
    fn run(&self, args: &[String]) -> Out {
        let mut argv = vec!["kubectl".to_string()];
        argv.extend(args.iter().cloned());
        let cwd = std::env::temp_dir();
        crate::wsexec::run_argv(&cwd, &argv, &[], Duration::from_secs(REQUEST_TIMEOUT_SECS + 6), MAX_OUTPUT)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Good,
    Warn,
    Bad,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cluster {
    Ok,
    KubectlMissing,
    Unreachable(String),
    /// Reachable, but this account may not read something (the message says what).
    Forbidden(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    pub kind: String,
    pub name: String,
    pub ready: i64,
    pub desired: i64,
    pub images: Vec<String>,
    pub age: u64,
    pub health: Health,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub kind: String,
    pub cluster_ip: String,
    pub ports: String,
    pub external: String,
    /// The nodePort numbers of the Service (empty unless NodePort or LoadBalancer).
    pub node_ports: Vec<i64>,
    pub age: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ingress {
    pub name: String,
    pub class: String,
    pub hosts: String,
    pub address: String,
    pub age: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pod {
    pub name: String,
    pub phase: String,
    pub ready: String,
    pub restarts: i64,
    pub node: String,
    pub age: u64,
    /// The waiting / terminated reason of the worst container, if any (`CrashLoopBackOff`, `ImagePullBackOff`…).
    pub reason: String,
    pub health: Health,
    pub containers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pvc {
    pub name: String,
    pub status: String,
    pub size: String,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub ready: bool,
    pub roles: String,
    pub version: String,
    pub cpu: String,
    pub memory: String,
    /// The node's InternalIP (empty when unknown).
    pub ip: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cluster: Cluster,
    pub target: Target,
    pub context: String,
    pub namespaces: Vec<String>,
    /// The namespace exists in the cluster (false: the view shows how to create it).
    pub namespace_found: bool,
    pub workloads: Vec<Workload>,
    pub services: Vec<Service>,
    pub ingresses: Vec<Ingress>,
    pub pods: Vec<Pod>,
    pub configmaps: Vec<String>,
    /// Names only.
    pub secrets: Vec<String>,
    pub pvcs: Vec<Pvc>,
    pub nodes: Vec<NodeInfo>,
    pub taken_at: u64,
    /// Plain-English notes about problems found (pods that cannot start and why).
    pub notes: Vec<String>,
}

impl Snapshot {
    pub fn empty(target: &Target, cluster: Cluster, now: u64) -> Snapshot {
        Snapshot { cluster, target: target.clone(), context: String::new(), namespaces: vec![], namespace_found: false, workloads: vec![], services: vec![], ingresses: vec![], pods: vec![], configmaps: vec![], secrets: vec![], pvcs: vec![], nodes: vec![], taken_at: now, notes: vec![] }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restarts: i64,
    pub state: String,
    pub reason: String,
    pub message: String,
    pub requests: String,
    pub limits: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub age: u64,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodDetail {
    pub name: String,
    pub phase: String,
    pub node: String,
    pub ip: String,
    pub owner: String,
    pub containers: Vec<ContainerInfo>,
    pub events: Vec<EventRow>,
    pub explanation: Vec<String>,
}

// ------------------------------------------------------------------------------------------------------------ helpers

fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    let mut c = v;
    for p in path {
        c = &c[*p];
    }
    c
}

fn text(v: &Value, path: &[&str]) -> String {
    at(v, path).as_str().unwrap_or("").to_string()
}

fn int(v: &Value, path: &[&str]) -> i64 {
    at(v, path).as_i64().unwrap_or(0)
}

/// Seconds since 1970 of `2024-05-01T12:34:56Z` (UTC); 0 when it cannot be read.
pub fn parse_time(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let n = |a: usize, z: usize| s.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19)) else { return 0 };
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + h * 3600 + mi * 60 + se).max(0) as u64
}

fn age_of(v: &Value, now: u64) -> u64 {
    let t = parse_time(&text(v, &["metadata", "creationTimestamp"]));
    if t == 0 { 0 } else { now.saturating_sub(t) }
}

/// `5s`, `3m`, `2h`, `4d`.
pub fn age_text(secs: u64) -> String {
    if secs < 60 { format!("{secs}s") } else if secs < 3600 { format!("{}m", secs / 60) } else if secs < 86_400 { format!("{}h", secs / 3600) } else { format!("{}d", secs / 86_400) }
}

/// The tag (or digest start) of an image reference: `ghcr.io/x/web:1.4.2` gives `web:1.4.2`.
pub fn short_image(img: &str) -> String {
    let name = img.rsplit('/').next().unwrap_or(img);
    name.split('@').next().unwrap_or(name).to_string()
}

/// A reason kubectl shows, in plain English, with what to do.
pub fn explain(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "ImagePullBackOff" | "ErrImagePull" => "The image cannot be downloaded. Check the image name and tag, that it exists in the registry, and that the cluster may pull from it (an image imported into k3s needs imagePullPolicy IfNotPresent or Never). If the events say \"server gave HTTP response to HTTPS client\", the registry is plain HTTP and each node needs a mirror entry in /etc/rancher/k3s/registries.yaml (the assistant's registry_setup_help gives the exact text); \"x509\" means an untrusted certificate, \"no basic auth credentials\" a registry that needs a login, \"connection refused\" a registry the node cannot reach.",
        "CrashLoopBackOff" => "The program starts and then exits again and again. Read its log (press l) for the error; common causes are a missing setting, a wrong command, or a failing dependency.",
        "CreateContainerConfigError" | "CreateContainerError" => "The container could not be created: a ConfigMap or Secret it uses is missing or has no such key, or the manifest says runAsNonRoot: true while the image runs as root or has a non-numeric USER (add a numeric `USER 10001` to the Dockerfile, rebuild with a new tag, redeploy). deploy_status and k8s_events show the exact cause.",
        "OOMKilled" => "The program used more memory than its limit, so Kubernetes stopped it. Raise the memory limit or reduce what the program uses.",
        "Unschedulable" | "FailedScheduling" => "No node can run this pod: not enough CPU or memory, a taint, or a volume that cannot be placed. Check the events below.",
        "ContainerCreating" | "PodInitializing" => "The pod is starting. If this lasts minutes, check the events (image pull, volume mount).",
        "Evicted" => "The node needed room and removed this pod. It is usually recreated by its workload.",
        "Error" => "The program exited with an error. Read its log (press l).",
        "InvalidImageName" => "The image name is not valid. Fix the image field of the workload.",
        "BackOff" => "Kubernetes is waiting before it tries again after a failure. Read the log and events for the first error.",
        _ => return None,
    })
}

fn classify_failure(o: &Out) -> Cluster {
    if o.missing {
        return Cluster::KubectlMissing;
    }
    let e = format!("{}{}", o.stderr, o.stdout).to_lowercase();
    if o.timed_out || e.contains("unable to connect") || e.contains("connection refused") || e.contains("no route to host") || e.contains("i/o timeout") || e.contains("couldn't get current server") || e.contains("dial tcp") || e.contains("no configuration has been provided") || e.contains("was refused") || e.contains("tls handshake timeout") || e.contains("context deadline") {
        let first = o.stderr.lines().next().unwrap_or("the cluster did not answer").trim();
        return Cluster::Unreachable(if o.timed_out { "kubectl timed out".into() } else { first.chars().take(200).collect() });
    }
    if e.contains("forbidden") {
        return Cluster::Forbidden(o.stderr.lines().next().unwrap_or("forbidden").chars().take(200).collect());
    }
    Cluster::Failed(o.combined().lines().next().unwrap_or("kubectl failed").chars().take(200).collect())
}

fn json(o: &Out) -> Option<Value> {
    serde_json::from_str(&o.stdout).ok()
}

fn names(o: &Out) -> Vec<String> {
    o.stdout.lines().filter_map(|l| l.trim().split_once('/').map(|x| x.1.to_string())).collect()
}

fn health_pod(phase: &str, reason: &str, ready: bool) -> Health {
    match (phase, reason) {
        (_, "CrashLoopBackOff" | "ImagePullBackOff" | "ErrImagePull" | "CreateContainerConfigError" | "OOMKilled" | "Error" | "Evicted" | "InvalidImageName" | "CreateContainerError") => Health::Bad,
        ("Failed", _) => Health::Bad,
        ("Succeeded", _) => Health::Idle,
        ("Pending", _) => Health::Warn,
        ("Running", _) if ready => Health::Good,
        ("Running", _) => Health::Warn,
        _ => Health::Warn,
    }
}

fn parse_pod(p: &Value, now: u64) -> Pod {
    let statuses = at(p, &["status", "containerStatuses"]).as_array().cloned().unwrap_or_default();
    let total = at(p, &["spec", "containers"]).as_array().map_or(0, |a| a.len());
    let ready_n = statuses.iter().filter(|c| c["ready"].as_bool() == Some(true)).count();
    let restarts: i64 = statuses.iter().map(|c| c["restartCount"].as_i64().unwrap_or(0)).sum();
    let mut reason = String::new();
    for c in &statuses {
        let w = text(c, &["state", "waiting", "reason"]);
        let t = text(c, &["state", "terminated", "reason"]);
        let last = text(c, &["lastState", "terminated", "reason"]);
        if !w.is_empty() && w != "PodInitializing" || (reason.is_empty() && !w.is_empty()) {
            reason = w;
        } else if !t.is_empty() && t != "Completed" && reason.is_empty() {
            reason = t;
        } else if last == "OOMKilled" && reason.is_empty() {
            reason = last;
        }
    }
    if reason.is_empty() {
        let r = text(p, &["status", "reason"]);
        if !r.is_empty() {
            reason = r;
        } else if at(p, &["status", "conditions"]).as_array().is_some_and(|c| c.iter().any(|x| x["reason"] == "Unschedulable")) {
            reason = "Unschedulable".into();
        }
    }
    let phase = text(p, &["status", "phase"]);
    let ready = total > 0 && ready_n == total;
    let containers = at(p, &["spec", "containers"]).as_array().map(|a| a.iter().map(|c| text(c, &["name"])).collect()).unwrap_or_default();
    Pod { name: text(p, &["metadata", "name"]), health: health_pod(&phase, &reason, ready), phase, ready: format!("{ready_n}/{total}"), restarts, node: text(p, &["spec", "nodeName"]), age: age_of(p, now), reason, containers }
}

fn parse_workload(kind: &str, w: &Value, now: u64) -> Workload {
    let (ready, desired) = match kind {
        "DaemonSet" => (int(w, &["status", "numberReady"]), int(w, &["status", "desiredNumberScheduled"])),
        "Job" => (int(w, &["status", "succeeded"]), at(w, &["spec", "completions"]).as_i64().unwrap_or(1)),
        _ => (int(w, &["status", "readyReplicas"]), at(w, &["spec", "replicas"]).as_i64().unwrap_or(1)),
    };
    let images = at(w, &["spec", "template", "spec", "containers"]).as_array().map(|a| a.iter().map(|c| short_image(&text(c, &["image"]))).collect()).unwrap_or_default();
    let failed = int(w, &["status", "failed"]);
    let (health, status) = if kind == "Job" && failed > 0 && ready < desired {
        (Health::Bad, format!("{failed} failed"))
    } else if kind == "Job" && ready >= desired {
        (Health::Idle, "complete".to_string())
    } else if desired == 0 {
        (Health::Idle, "scaled to 0".to_string())
    } else if ready >= desired {
        (Health::Good, "available".to_string())
    } else if ready == 0 {
        (Health::Bad, "unavailable".to_string())
    } else {
        (Health::Warn, "updating".to_string())
    };
    Workload { kind: kind.to_string(), name: text(w, &["metadata", "name"]), ready, desired, images, age: age_of(w, now), health, status }
}

fn parse_service(s: &Value, now: u64) -> Service {
    let ports = at(s, &["spec", "ports"]).as_array().map(|a| {
        a.iter().map(|p| {
            let (port, proto) = (int(p, &["port"]), p["protocol"].as_str().unwrap_or("TCP"));
            let target = at(p, &["targetPort"]).as_i64().map(|t| t.to_string()).or_else(|| at(p, &["targetPort"]).as_str().map(str::to_string)).unwrap_or_default();
            let node = int(p, &["nodePort"]);
            let mut t = format!("{port}/{proto}");
            if !target.is_empty() && target != port.to_string() {
                t.push_str(&format!("→{target}"));
            }
            if node > 0 {
                t.push_str(&format!(":{node}"));
            }
            t
        }).collect::<Vec<_>>().join(", ")
    }).unwrap_or_default();
    let mut ext: Vec<String> = at(s, &["status", "loadBalancer", "ingress"]).as_array().map(|a| a.iter().map(|i| { let ip = text(i, &["ip"]); if ip.is_empty() { text(i, &["hostname"]) } else { ip } }).collect()).unwrap_or_default();
    if let Some(a) = at(s, &["spec", "externalIPs"]).as_array() {
        ext.extend(a.iter().filter_map(|x| x.as_str().map(str::to_string)));
    }
    let kind = text(s, &["spec", "type"]);
    let external = if ext.is_empty() { if kind == "LoadBalancer" { "<pending>".into() } else { String::new() } } else { ext.join(", ") };
    Service { name: text(s, &["metadata", "name"]), kind, cluster_ip: text(s, &["spec", "clusterIP"]), ports, external, node_ports: at(s, &["spec", "ports"]).as_array().map(|a| a.iter().map(|p| int(p, &["nodePort"])).filter(|n| *n > 0).collect()).unwrap_or_default(), age: age_of(s, now) }
}

fn parse_ingress(i: &Value, now: u64) -> Ingress {
    let hosts = at(i, &["spec", "rules"]).as_array().map(|a| a.iter().map(|r| text(r, &["host"])).filter(|h| !h.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let address = at(i, &["status", "loadBalancer", "ingress"]).as_array().map(|a| a.iter().map(|x| { let ip = text(x, &["ip"]); if ip.is_empty() { text(x, &["hostname"]) } else { ip } }).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    Ingress { name: text(i, &["metadata", "name"]), class: text(i, &["spec", "ingressClassName"]), hosts, address, age: age_of(i, now) }
}

fn parse_node(n: &Value) -> NodeInfo {
    let ready = at(n, &["status", "conditions"]).as_array().is_some_and(|c| c.iter().any(|x| x["type"] == "Ready" && x["status"] == "True"));
    let roles: Vec<String> = n["metadata"]["labels"].as_object().map(|m| m.keys().filter_map(|k| k.strip_prefix("node-role.kubernetes.io/").map(str::to_string)).collect()).unwrap_or_default();
    NodeInfo { name: text(n, &["metadata", "name"]), ready, roles: if roles.is_empty() { "worker".into() } else { roles.join(",") }, version: text(n, &["status", "nodeInfo", "kubeletVersion"]), cpu: text(n, &["status", "allocatable", "cpu"]), memory: text(n, &["status", "allocatable", "memory"]), ip: at(n, &["status", "addresses"]).as_array().and_then(|a| a.iter().find(|x| x["type"] == "InternalIP")).and_then(|x| x["address"].as_str()).unwrap_or("").to_string() }
}

/// How each Service can really be reached, one line each. Only a LoadBalancer's external address or a node address with a
/// NodePort is called reachable from outside; a ClusterIP (10.43.x.x on k3s) is said to be cluster-internal.
pub fn reachable_lines(snap: &Snapshot) -> Vec<String> {
    let node_ip = snap.nodes.iter().find(|n| n.ready && !n.ip.is_empty()).or_else(|| snap.nodes.iter().find(|n| !n.ip.is_empty())).map(|n| n.ip.clone());
    let port_of = |s: &Service| s.ports.split(',').next().and_then(|p| p.trim().split('/').next()).unwrap_or("80").to_string();
    let mut out = Vec::new();
    for s in &snap.services {
        if s.cluster_ip == "None" {
            continue;
        }
        let mut ways = Vec::new();
        if !s.external.is_empty() && s.external != "<pending>" {
            for e in s.external.split(", ") {
                ways.push(format!("http://{e}:{} (external address of the LoadBalancer)", port_of(s)));
            }
        }
        if !s.node_ports.is_empty() {
            match &node_ip {
                Some(ip) => ways.extend(s.node_ports.iter().map(|p| format!("http://{ip}:{p} (node address + NodePort)"))),
                None => ways.push(format!("NodePort {} on the node's address (node address unknown)", s.node_ports.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))),
            }
        }
        if ways.is_empty() {
            let why = if s.kind == "LoadBalancer" { "its external address is still pending" } else { "it is cluster-internal" };
            out.push(format!("{}: not reachable from outside: {} is a {} address and {why}. Change the Service type to NodePort (or LoadBalancer), or use `kubectl port-forward`.", s.name, s.cluster_ip, if s.kind.is_empty() { "ClusterIP" } else { s.kind.as_str() }));
        } else {
            out.push(format!("{}: reachable at: {}", s.name, ways.join(" ; ")));
        }
    }
    out
}

/// Fetch everything the view shows with a handful of read-only `kubectl get` calls.
pub fn fetch(kube: &dyn Kube, t: &Target, now: u64) -> Snapshot {
    let base = t.base_args();
    let run = |extra: &[&str]| {
        let mut a = base.clone();
        a.extend(extra.iter().map(|s| s.to_string()));
        kube.run(&a)
    };
    let ns_out = run(&["get", "namespaces", "-o", "json"]);
    let ns_json = if ns_out.ok() { json(&ns_out) } else { None };
    let forbidden_ns = !ns_out.ok() && matches!(classify_failure(&ns_out), Cluster::Forbidden(_));
    if !ns_out.ok() && !forbidden_ns {
        return Snapshot::empty(t, classify_failure(&ns_out), now);
    }
    let mut snap = Snapshot::empty(t, Cluster::Ok, now);
    if let Some(v) = &ns_json {
        snap.namespaces = at(v, &["items"]).as_array().map(|a| a.iter().map(|n| text(n, &["metadata", "name"])).collect()).unwrap_or_default();
        snap.namespaces.sort();
        snap.namespace_found = snap.namespaces.contains(&t.namespace);
    } else {
        snap.namespace_found = true;
        snap.namespaces = vec![t.namespace.clone()];
    }
    let ctx = run(&["config", "current-context"]);
    snap.context = if ctx.ok() { ctx.stdout.trim().to_string() } else { t.context.clone() };
    if !snap.namespace_found {
        snap.notes.push(format!("The namespace \"{}\" does not exist in this cluster yet. It is created by the deploy flow (docs/K3S-DEV.md), or with kubectl create namespace {}.", t.namespace, t.namespace));
        return snap;
    }
    let sel: Vec<&str> = if t.selector.is_empty() { vec![] } else { vec!["-l", t.selector.as_str()] };
    let mut main_args = vec!["-n", t.namespace.as_str(), "get", "deployments,statefulsets,daemonsets,jobs,services,ingresses,pods,persistentvolumeclaims", "-o", "json"];
    main_args.extend(sel.iter().copied());
    let main = run(&main_args);
    if !main.ok() {
        let c = classify_failure(&main);
        snap.cluster = if matches!(c, Cluster::Failed(_)) && !main.stdout.is_empty() { Cluster::Failed(main.stderr.lines().next().unwrap_or("kubectl failed").to_string()) } else { c };
        // `get` of a kind this cluster lacks (no ingress controller) fails as a whole: try the core kinds alone
        if matches!(snap.cluster, Cluster::Failed(_)) {
            let mut core = vec!["-n", t.namespace.as_str(), "get", "deployments,statefulsets,daemonsets,jobs,services,pods,persistentvolumeclaims", "-o", "json"];
            core.extend(sel.iter().copied());
            let again = run(&core);
            if again.ok() {
                snap.cluster = Cluster::Ok;
                fill_main(&mut snap, &again, now);
            }
        }
    } else {
        fill_main(&mut snap, &main, now);
    }
    // names only, never values
    snap.configmaps = names(&run(&["-n", t.namespace.as_str(), "get", "configmaps", "-o", "name"]));
    snap.secrets = names(&run(&["-n", t.namespace.as_str(), "get", "secrets", "-o", "name"]));
    let nodes = run(&["get", "nodes", "-o", "json"]);
    if let Some(v) = json(&nodes) {
        snap.nodes = at(&v, &["items"]).as_array().map(|a| a.iter().map(parse_node).collect()).unwrap_or_default();
    }
    for p in &snap.pods {
        if p.health == Health::Bad || (p.health == Health::Warn && !p.reason.is_empty()) {
            if let Some(e) = explain(&p.reason) {
                snap.notes.push(format!("{}: {} — {e}", p.name, p.reason));
            }
        }
    }
    snap
}

fn fill_main(snap: &mut Snapshot, out: &Out, now: u64) {
    let Some(v) = json(out) else {
        snap.cluster = Cluster::Failed("kubectl printed something unreadable".into());
        return;
    };
    for it in at(&v, &["items"]).as_array().cloned().unwrap_or_default() {
        match it["kind"].as_str().unwrap_or("") {
            k @ ("Deployment" | "StatefulSet" | "DaemonSet" | "Job") => snap.workloads.push(parse_workload(k, &it, now)),
            "Service" => snap.services.push(parse_service(&it, now)),
            "Ingress" => snap.ingresses.push(parse_ingress(&it, now)),
            "Pod" => snap.pods.push(parse_pod(&it, now)),
            "PersistentVolumeClaim" => snap.pvcs.push(Pvc { name: text(&it, &["metadata", "name"]), status: text(&it, &["status", "phase"]), size: text(&it, &["status", "capacity", "storage"]), class: text(&it, &["spec", "storageClassName"]) }),
            _ => {}
        }
    }
}

/// A pod's details: containers (state, requests), events, and plain-English explanations of what is wrong.
pub fn pod_detail(kube: &dyn Kube, t: &Target, pod: &str, now: u64) -> Result<PodDetail, String> {
    if pod.is_empty() || pod.starts_with('-') || pod.contains(['/', ' ']) {
        return Err("not a pod name".into());
    }
    let base = t.base_args();
    let mut a = base.clone();
    a.extend(["-n", t.namespace.as_str(), "get", "pod", pod, "-o", "json"].map(String::from));
    let o = kube.run(&a);
    if !o.ok() {
        return Err(o.combined().lines().next().unwrap_or("kubectl failed").to_string());
    }
    let p = json(&o).ok_or("unreadable answer")?;
    let spec_c = at(&p, &["spec", "containers"]).as_array().cloned().unwrap_or_default();
    let stat = at(&p, &["status", "containerStatuses"]).as_array().cloned().unwrap_or_default();
    let res = |c: &Value, k: &str| {
        let r = &c["resources"][k];
        let cpu = r["cpu"].as_str().unwrap_or("");
        let mem = r["memory"].as_str().unwrap_or("");
        match (cpu.is_empty(), mem.is_empty()) {
            (true, true) => String::new(),
            _ => format!("cpu {} · mem {}", if cpu.is_empty() { "-" } else { cpu }, if mem.is_empty() { "-" } else { mem }),
        }
    };
    let mut containers = Vec::new();
    let mut explanation = Vec::new();
    for c in &spec_c {
        let name = text(c, &["name"]);
        let st = stat.iter().find(|s| s["name"] == name.as_str()).cloned().unwrap_or(Value::Null);
        let (state, reason, message) = if let Some(w) = st["state"].get("waiting") {
            ("waiting".to_string(), text(w, &["reason"]), text(w, &["message"]))
        } else if let Some(r) = st["state"].get("running") {
            let _ = r;
            ("running".to_string(), String::new(), String::new())
        } else if let Some(tm) = st["state"].get("terminated") {
            ("terminated".to_string(), text(tm, &["reason"]), text(tm, &["message"]))
        } else {
            ("unknown".to_string(), String::new(), String::new())
        };
        let last = text(&st, &["lastState", "terminated", "reason"]);
        if let Some(e) = explain(&reason).or_else(|| explain(&last)) {
            explanation.push(format!("{name}: {}. {e}", if reason.is_empty() { last.clone() } else { reason.clone() }));
        }
        containers.push(ContainerInfo { name, image: text(c, &["image"]), ready: st["ready"].as_bool() == Some(true), restarts: int(&st, &["restartCount"]), state, reason: if reason.is_empty() { last } else { reason }, message: message.chars().take(300).collect(), requests: res(c, "requests"), limits: res(c, "limits") });
    }
    if at(&p, &["status", "conditions"]).as_array().is_some_and(|c| c.iter().any(|x| x["reason"] == "Unschedulable")) {
        if let Some(e) = explain("Unschedulable") {
            explanation.push(format!("Unschedulable. {e}"));
        }
    }
    let mut a = base;
    a.extend(["-n", t.namespace.as_str(), "get", "events", "--field-selector"].map(String::from));
    a.push(format!("involvedObject.name={pod}"));
    a.extend(["-o", "json"].map(String::from));
    let eo = kube.run(&a);
    let mut events: Vec<EventRow> = json(&eo).map(|v| at(&v, &["items"]).as_array().cloned().unwrap_or_default().iter().map(|e| {
        let ts = {
            let l = text(e, &["lastTimestamp"]);
            if l.is_empty() { text(e, &["eventTime"]) } else { l }
        };
        let t0 = parse_time(&ts);
        EventRow { kind: text(e, &["type"]), reason: text(e, &["reason"]), message: text(e, &["message"]).chars().take(300).collect(), age: if t0 == 0 { 0 } else { now.saturating_sub(t0) }, count: e["count"].as_i64().unwrap_or(1) }
    }).collect()).unwrap_or_default();
    events.sort_by_key(|e| e.age);
    events.truncate(12);
    for e in &events {
        if e.reason == "FailedScheduling" && explanation.iter().all(|x| !x.contains("Unschedulable")) {
            if let Some(x) = explain("FailedScheduling") {
                explanation.push(format!("FailedScheduling. {x}"));
            }
        }
    }
    Ok(PodDetail { name: text(&p, &["metadata", "name"]), phase: text(&p, &["status", "phase"]), node: text(&p, &["spec", "nodeName"]), ip: text(&p, &["status", "podIP"]), owner: at(&p, &["metadata", "ownerReferences"]).as_array().and_then(|a| a.first()).map(|o| format!("{}/{}", text(o, &["kind"]), text(o, &["name"]))).unwrap_or_default(), containers, events, explanation })
}

/// The last `tail` lines of a pod's log (a container name when the pod has several).
pub fn pod_logs(kube: &dyn Kube, t: &Target, pod: &str, container: &str, tail: usize) -> Result<Vec<String>, String> {
    if pod.is_empty() || pod.starts_with('-') || pod.contains(['/', ' ']) || container.starts_with('-') {
        return Err("not a pod name".into());
    }
    let mut a = t.base_args();
    a.extend(["-n", t.namespace.as_str(), "logs", pod].map(String::from));
    if !container.is_empty() {
        a.push("-c".into());
        a.push(container.into());
    }
    a.push(format!("--tail={}", tail.min(1000)));
    let o = kube.run(&a);
    if !o.ok() {
        return Err(o.combined().lines().next().unwrap_or("kubectl logs failed").trim().to_string());
    }
    Ok(o.stdout.lines().map(|l| l.chars().take(400).collect()).collect())
}

// ------------------------------------------------------------------------------------------------- the background feed

#[derive(Default)]
struct Shared {
    snap: Option<Arc<Snapshot>>,
    target: Target,
    refresh: bool,
    pod: Option<(String, String)>,
    logs: Option<(String, Result<Vec<String>, String>)>,
    detail: Option<(String, Result<PodDetail, String>)>,
    want_detail: Option<String>,
    fetches: u64,
}

/// Keeps a [`Snapshot`] fresh on a thread: every `interval` while the view is visible (the UI never waits for kubectl),
/// the log of the selected pod every `log_interval`. The thread ends when the feed is dropped.
pub struct Feed {
    shared: Arc<Mutex<Shared>>,
    visible: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
}

impl Feed {
    pub fn start(kube: Arc<dyn Kube>, target: Target, interval: Duration, log_interval: Duration) -> Feed {
        let shared = Arc::new(Mutex::new(Shared { target, refresh: true, ..Default::default() }));
        let (visible, alive) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(true)));
        let (s2, v2, a2) = (shared.clone(), visible.clone(), alive.clone());
        std::thread::spawn(move || {
            let (mut last, mut last_log) = (None::<std::time::Instant>, None::<std::time::Instant>);
            while a2.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(25));
                if !v2.load(Ordering::Relaxed) {
                    continue;
                }
                let (target, due, pod, want_detail) = {
                    let mut s = s2.lock().unwrap();
                    let due = s.refresh || last.is_none_or(|t| t.elapsed() >= interval);
                    s.refresh = false;
                    (s.target.clone(), due, s.pod.clone(), s.want_detail.take())
                };
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                if due {
                    let snap = fetch(&*kube, &target, now);
                    let mut s = s2.lock().unwrap();
                    if s.target == target {
                        s.snap = Some(Arc::new(snap));
                        s.fetches += 1;
                    }
                    last = Some(std::time::Instant::now());
                }
                if let Some(p) = want_detail {
                    let d = pod_detail(&*kube, &target, &p, now);
                    s2.lock().unwrap().detail = Some((p, d));
                }
                if let Some((p, c)) = pod {
                    if last_log.is_none_or(|t| t.elapsed() >= log_interval) {
                        let l = pod_logs(&*kube, &target, &p, &c, LOG_TAIL);
                        let mut s = s2.lock().unwrap();
                        if s.pod.as_ref().is_some_and(|x| x.0 == p) {
                            s.logs = Some((p, l));
                        }
                        last_log = Some(std::time::Instant::now());
                    }
                } else {
                    last_log = None;
                }
            }
        });
        Feed { shared, visible, alive }
    }

    pub fn set_visible(&self, on: bool) {
        self.visible.store(on, Ordering::Relaxed);
    }
    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.shared.lock().unwrap().snap.clone()
    }
    pub fn fetches(&self) -> u64 {
        self.shared.lock().unwrap().fetches
    }
    pub fn refresh(&self) {
        self.shared.lock().unwrap().refresh = true;
    }
    /// Look at another namespace or selector (the old snapshot is dropped, a new one is fetched at once).
    pub fn set_target(&self, t: Target) {
        let mut s = self.shared.lock().unwrap();
        if s.target != t {
            s.target = t;
            s.snap = None;
            s.refresh = true;
            s.pod = None;
            s.logs = None;
            s.detail = None;
        }
    }
    pub fn target(&self) -> Target {
        self.shared.lock().unwrap().target.clone()
    }
    /// Follow the log of a pod (None stops following).
    pub fn follow(&self, pod: Option<(String, String)>) {
        let mut s = self.shared.lock().unwrap();
        if s.pod != pod {
            s.logs = None;
            s.pod = pod;
        }
    }
    pub fn logs(&self) -> Option<(String, Result<Vec<String>, String>)> {
        self.shared.lock().unwrap().logs.clone()
    }
    pub fn request_detail(&self, pod: &str) {
        let mut s = self.shared.lock().unwrap();
        s.detail = None;
        s.want_detail = Some(pod.to_string());
    }
    pub fn detail(&self) -> Option<(String, Result<PodDetail, String>)> {
        self.shared.lock().unwrap().detail.clone()
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

// --------------------------------------------------------------------------------------------------------- the fake

/// A cluster that answers from a list of (text the arguments contain, answer). Records every call. For tests and screenshots.
pub struct FakeKube {
    pub rules: Mutex<Vec<(String, Out)>>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeKube {
    pub fn new() -> FakeKube {
        FakeKube { rules: Mutex::new(Vec::new()), calls: Mutex::new(Vec::new()) }
    }
    pub fn answer(&self, contains: &str, stdout: &str) -> &Self {
        self.rules.lock().unwrap().insert(0, (contains.into(), Out { code: Some(0), stdout: stdout.into(), ..Default::default() }));
        self
    }
    pub fn fail(&self, contains: &str, stderr: &str) -> &Self {
        self.rules.lock().unwrap().insert(0, (contains.into(), Out { code: Some(1), stderr: stderr.into(), ..Default::default() }));
        self
    }
    pub fn missing(&self) -> &Self {
        self.rules.lock().unwrap().insert(0, (String::new(), Out { missing: true, stderr: "kubectl: No such file or directory".into(), ..Default::default() }));
        self
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for FakeKube {
    fn default() -> Self {
        Self::new()
    }
}

impl Kube for FakeKube {
    fn run(&self, args: &[String]) -> Out {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        for (k, o) in self.rules.lock().unwrap().iter() {
            if line.contains(k.as_str()) {
                return o.clone();
            }
        }
        Out { code: Some(1), stderr: format!("fake: no answer for {line}"), ..Default::default() }
    }
}

/// A cluster with a small shop in `shop-dev` (some healthy things and some failing), for the screenshots and tests.
/// Ages are relative to `now`.
pub fn sample_cluster(now: u64) -> FakeKube {
    fn ts(now: u64, ago: u64) -> String {
        let t = now.saturating_sub(ago) as i64;
        let (days, rem) = (t.div_euclid(86_400), t.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
    }
    let k = FakeKube::new();
    let ns = serde_json::json!({"items":[{"metadata":{"name":"default"}},{"metadata":{"name":"kube-system"}},{"metadata":{"name":"shop-dev"}},{"metadata":{"name":"shop-prod"}},{"metadata":{"name":"iyakki-dev"}}]});
    k.answer("get namespaces -o json", &ns.to_string());
    k.answer("config current-context", "k3s-dev\n");
    let pod = |name: &str, phase: &str, ready: bool, restarts: i64, waiting: Option<&str>, node: &str, ago: u64| {
        let state = match waiting {
            Some(r) => serde_json::json!({"waiting": {"reason": r, "message": "back-off restarting failed container"}}),
            None => serde_json::json!({"running": {"startedAt": ts(now, ago)}}),
        };
        serde_json::json!({"kind":"Pod","metadata":{"name":name,"creationTimestamp":ts(now,ago),"ownerReferences":[{"kind":"ReplicaSet","name":"x"}]},
            "spec":{"nodeName":node,"containers":[{"name":"app","image":"registry.local/shop/web:1.4.2","resources":{"requests":{"cpu":"100m","memory":"128Mi"},"limits":{"cpu":"500m","memory":"256Mi"}}}]},
            "status":{"phase":phase,"podIP":"10.42.0.12","containerStatuses":[{"name":"app","ready":ready,"restartCount":restarts,"state":state,"lastState":{}}]}})
    };
    let items = serde_json::json!([
        {"kind":"Deployment","metadata":{"name":"web","creationTimestamp":ts(now,86400*9)},"spec":{"replicas":3,"template":{"spec":{"containers":[{"name":"app","image":"registry.local/shop/web:1.4.2"}]}}},"status":{"readyReplicas":3}},
        {"kind":"Deployment","metadata":{"name":"api","creationTimestamp":ts(now,86400*9)},"spec":{"replicas":2,"template":{"spec":{"containers":[{"name":"app","image":"registry.local/shop/api:0.9.0"}]}}},"status":{"readyReplicas":1}},
        {"kind":"Deployment","metadata":{"name":"worker","creationTimestamp":ts(now,3600*5)},"spec":{"replicas":1,"template":{"spec":{"containers":[{"name":"app","image":"registry.local/shop/worker:latest"}]}}},"status":{}},
        {"kind":"StatefulSet","metadata":{"name":"postgres","creationTimestamp":ts(now,86400*20)},"spec":{"replicas":1,"template":{"spec":{"containers":[{"name":"db","image":"postgres:16.3"}]}}},"status":{"readyReplicas":1}},
        {"kind":"DaemonSet","metadata":{"name":"log-agent","creationTimestamp":ts(now,86400*20)},"spec":{"template":{"spec":{"containers":[{"name":"a","image":"fluent/fluent-bit:3.0"}]}}},"status":{"numberReady":1,"desiredNumberScheduled":1}},
        {"kind":"Job","metadata":{"name":"migrate-42","creationTimestamp":ts(now,600)},"spec":{"completions":1,"template":{"spec":{"containers":[{"name":"m","image":"registry.local/shop/api:0.9.0"}]}}},"status":{"succeeded":1}},
        {"kind":"Service","metadata":{"name":"web","creationTimestamp":ts(now,86400*9)},"spec":{"type":"LoadBalancer","clusterIP":"10.43.12.7","ports":[{"port":80,"protocol":"TCP","targetPort":8080}]},"status":{"loadBalancer":{"ingress":[{"ip":"192.168.1.50"}]}}},
        {"kind":"Service","metadata":{"name":"api","creationTimestamp":ts(now,86400*9)},"spec":{"type":"ClusterIP","clusterIP":"10.43.99.3","ports":[{"port":9000,"protocol":"TCP","targetPort":9000}]}},
        {"kind":"Service","metadata":{"name":"postgres","creationTimestamp":ts(now,86400*20)},"spec":{"type":"ClusterIP","clusterIP":"None","ports":[{"port":5432,"protocol":"TCP","targetPort":5432}]}},
        {"kind":"Ingress","metadata":{"name":"shop","creationTimestamp":ts(now,86400*9)},"spec":{"ingressClassName":"traefik","rules":[{"host":"shop.dev.local"},{"host":"api.shop.dev.local"}]},"status":{"loadBalancer":{"ingress":[{"ip":"192.168.1.50"}]}}},
        pod("web-6d9f7c-abcde","Running",true,0,None,"k3s-node-1",86400*2),
        pod("web-6d9f7c-fghij","Running",true,0,None,"k3s-node-1",86400*2),
        pod("web-6d9f7c-klmno","Running",true,1,None,"k3s-node-2",3600*7),
        pod("api-5b8d44-pqrst","Running",true,0,None,"k3s-node-2",3600*30),
        pod("api-5b8d44-uvwxy","Running",false,14,Some("CrashLoopBackOff"),"k3s-node-2",3600*30),
        pod("worker-7c6c5d-zzzzz","Pending",false,0,Some("ImagePullBackOff"),"k3s-node-1",3600*5),
        pod("postgres-0","Running",true,0,None,"k3s-node-1",86400*20),
        {"kind":"PersistentVolumeClaim","metadata":{"name":"data-postgres-0"},"spec":{"storageClassName":"local-path"},"status":{"phase":"Bound","capacity":{"storage":"10Gi"}}}
    ]);
    k.answer("get deployments,statefulsets", &serde_json::json!({"items": items}).to_string());
    k.answer("get configmaps -o name", "configmap/shop-config\nconfigmap/kube-root-ca.crt\n");
    k.answer("get secrets -o name", "secret/shop-db\nsecret/registry-pull\n");
    let nodes = serde_json::json!({"items":[
        {"metadata":{"name":"k3s-node-1","labels":{"node-role.kubernetes.io/control-plane":"true","node-role.kubernetes.io/master":"true"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.30.4+k3s1"},"allocatable":{"cpu":"4","memory":"15800Mi"}}},
        {"metadata":{"name":"k3s-node-2","labels":{}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.30.4+k3s1"},"allocatable":{"cpu":"4","memory":"7900Mi"}}}]});
    k.answer("get nodes -o json", &nodes.to_string());
    let p = pod("api-5b8d44-uvwxy","Running",false,14,Some("CrashLoopBackOff"),"k3s-node-2",3600*30);
    k.answer("get pod api-5b8d44-uvwxy -o json", &p.to_string());
    let ev = serde_json::json!({"items":[
        {"type":"Warning","reason":"BackOff","message":"Back-off restarting failed container app in pod api-5b8d44-uvwxy","lastTimestamp":ts(now,40),"count":212},
        {"type":"Normal","reason":"Pulled","message":"Container image \"registry.local/shop/api:0.9.0\" already present on machine","lastTimestamp":ts(now,300),"count":15}]});
    k.answer("get events", &ev.to_string());
    k.answer("logs ", "ready\nlistening on :8080\nGET /healthz 200\n");
    k.answer("logs api-5b8d44-uvwxy", "2026-10-13T08:01:02Z starting api 0.9.0\n2026-10-13T08:01:02Z connecting to postgres:5432\n2026-10-13T08:01:03Z error: password authentication failed for user \"shop\"\n2026-10-13T08:01:03Z fatal: cannot start without a database\n");
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_790_000_000;

    fn target() -> Target {
        Target { namespace: "shop-dev".into(), ..Default::default() }
    }

    #[test]
    fn times_and_ages() {
        assert_eq!(parse_time("1970-01-02T00:00:10Z"), 86_410);
        assert_eq!(parse_time("2024-02-29T12:00:00Z"), 1_709_208_000);
        assert_eq!(parse_time("nonsense"), 0);
        assert_eq!(age_text(59), "59s");
        assert_eq!(age_text(7200), "2h");
        assert_eq!(age_text(86_400 * 3), "3d");
        assert_eq!(short_image("ghcr.io/acme/web:1.2.3"), "web:1.2.3");
        assert_eq!(short_image("nginx@sha256:abcd"), "nginx");
    }

    #[test]
    fn the_namespace_view_has_workloads_services_pods_and_names_only() {
        let k = sample_cluster(NOW);
        let s = fetch(&k, &target(), NOW);
        assert_eq!(s.cluster, Cluster::Ok);
        assert!(s.namespace_found && s.namespaces.contains(&"shop-dev".to_string()) && s.namespaces.len() == 5);
        assert_eq!(s.context, "k3s-dev");
        let web = s.workloads.iter().find(|w| w.name == "web").unwrap();
        assert_eq!((web.kind.as_str(), web.ready, web.desired, web.health, web.images.clone()), ("Deployment", 3, 3, Health::Good, vec!["web:1.4.2".to_string()]));
        let api = s.workloads.iter().find(|w| w.name == "api").unwrap();
        assert_eq!((api.ready, api.desired, api.health), (1, 2, Health::Warn));
        assert_eq!(s.workloads.iter().find(|w| w.name == "worker").unwrap().health, Health::Bad);
        assert_eq!(s.workloads.iter().find(|w| w.name == "migrate-42").unwrap().health, Health::Idle);
        assert_eq!(s.workloads.len(), 6);
        let svc = s.services.iter().find(|x| x.name == "web").unwrap();
        assert_eq!((svc.kind.as_str(), svc.cluster_ip.as_str(), svc.ports.as_str(), svc.external.as_str()), ("LoadBalancer", "10.43.12.7", "80/TCP→8080", "192.168.1.50"));
        assert_eq!(s.ingresses[0].hosts, "shop.dev.local, api.shop.dev.local");
        assert_eq!(s.pods.len(), 7);
        let bad = s.pods.iter().find(|p| p.name == "api-5b8d44-uvwxy").unwrap();
        assert_eq!((bad.reason.as_str(), bad.restarts, bad.health, bad.ready.as_str()), ("CrashLoopBackOff", 14, Health::Bad, "0/1"));
        assert_eq!(s.pods.iter().find(|p| p.name.starts_with("worker")).unwrap().reason, "ImagePullBackOff");
        assert_eq!(s.pods.iter().find(|p| p.name == "web-6d9f7c-abcde").unwrap().age, 86_400 * 2);
        assert_eq!(s.configmaps, vec!["shop-config", "kube-root-ca.crt"]);
        assert_eq!(s.secrets, vec!["shop-db", "registry-pull"], "names only");
        assert_eq!(s.pvcs[0].size, "10Gi");
        assert_eq!(s.nodes.len(), 2);
        assert_eq!(s.nodes[0].roles, "control-plane,master");
        assert!(s.notes.iter().any(|n| n.contains("CrashLoopBackOff") && n.contains("log")), "{:?}", s.notes);
        assert!(s.notes.iter().any(|n| n.contains("ImagePullBackOff") && n.contains("registry")));
    }

    #[test]
    fn only_reads_and_secret_values_are_never_requested() {
        let k = sample_cluster(NOW);
        fetch(&k, &target(), NOW);
        pod_detail(&k, &target(), "api-5b8d44-uvwxy", NOW).unwrap();
        pod_logs(&k, &target(), "api-5b8d44-uvwxy", "", 50).unwrap();
        let calls = k.calls();
        assert!(calls.len() >= 8);
        for c in &calls {
            let verb = c.split_whitespace().find(|w| matches!(*w, "get" | "logs" | "config")).unwrap_or("?");
            assert!(matches!(verb, "get" | "logs" | "config"), "only reads: {c}");
            for bad in ["apply", "delete", "scale", "patch", "exec", "edit", "create", "rollout", "port-forward"] {
                assert!(!c.split_whitespace().any(|w| w == bad), "{c}");
            }
            if c.contains("secret") {
                assert!(c.ends_with("-o name"), "secrets are listed by name only: {c}");
            }
        }
        // even a cluster that answers a secret query with values cannot leak them: only names are read
        let k2 = sample_cluster(NOW);
        k2.answer("get secrets -o name", "secret/shop-db\n");
        let s = fetch(&k2, &target(), NOW);
        let dump = format!("{s:?}");
        assert!(!dump.contains("c3VwZXI") && s.secrets == vec!["shop-db"]);
    }

    #[test]
    fn selectors_namespaces_and_kubeconfig_reach_kubectl() {
        let k = sample_cluster(NOW);
        let t = Target { namespace: "shop-dev".into(), selector: "app=web".into(), context: "edge".into(), kubeconfig: "/tmp/kc".into() };
        fetch(&k, &t, NOW);
        let calls = k.calls();
        assert!(calls.iter().all(|c| c.starts_with("--kubeconfig /tmp/kc --context edge --request-timeout=8s")), "{calls:?}");
        assert!(calls.iter().any(|c| c.contains("-n shop-dev get deployments,statefulsets") && c.ends_with("-l app=web")), "{calls:?}");
    }

    #[test]
    fn details_logs_and_plain_english_reasons() {
        let k = sample_cluster(NOW);
        let d = pod_detail(&k, &target(), "api-5b8d44-uvwxy", NOW).unwrap();
        assert_eq!((d.phase.as_str(), d.node.as_str()), ("Running", "k3s-node-2"));
        let c = &d.containers[0];
        assert_eq!((c.state.as_str(), c.reason.as_str(), c.restarts, c.requests.as_str(), c.limits.as_str()), ("waiting", "CrashLoopBackOff", 14, "cpu 100m · mem 128Mi", "cpu 500m · mem 256Mi"));
        assert!(d.explanation.iter().any(|e| e.contains("exits again and again")), "{:?}", d.explanation);
        assert_eq!(d.events[0].reason, "BackOff");
        assert!(d.events[0].age <= d.events[1].age);
        let l = pod_logs(&k, &target(), "api-5b8d44-uvwxy", "", 200).unwrap();
        assert!(l.iter().any(|x| x.contains("password authentication failed")));
        assert!(k.calls().iter().any(|c| c.contains("logs api-5b8d44-uvwxy --tail=200")));
        assert!(pod_logs(&k, &target(), "--all-containers", "", 5).is_err() && pod_detail(&k, &target(), "a/b", NOW).is_err(), "no option injection");
        for r in ["ImagePullBackOff", "CrashLoopBackOff", "OOMKilled", "CreateContainerConfigError", "Unschedulable"] {
            assert!(explain(r).is_some(), "{r}");
        }
        assert!(explain("Whatever").is_none());
    }

    #[test]
    fn a_missing_kubectl_and_an_unreachable_cluster_are_told_apart() {
        let k = FakeKube::new();
        k.missing();
        assert_eq!(fetch(&k, &target(), NOW).cluster, Cluster::KubectlMissing);
        let k = FakeKube::new();
        k.fail("get namespaces", "The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?");
        match fetch(&k, &target(), NOW).cluster {
            Cluster::Unreachable(m) => assert!(m.contains("refused")),
            other => panic!("{other:?}"),
        }
        let k = FakeKube::new();
        k.fail("get namespaces", "error: Unable to connect to the server: dial tcp 10.0.0.1:6443: i/o timeout");
        assert!(matches!(fetch(&k, &target(), NOW).cluster, Cluster::Unreachable(_)));
        let k = FakeKube::new();
        k.fail("get namespaces", "error: no configuration has been provided");
        assert!(matches!(fetch(&k, &target(), NOW).cluster, Cluster::Unreachable(_)));
        // a namespace that does not exist yet
        let k = sample_cluster(NOW);
        let s = fetch(&k, &Target { namespace: "nope-dev".into(), ..Default::default() }, NOW);
        assert!(!s.namespace_found && s.notes[0].contains("does not exist") && s.workloads.is_empty());
        // a person who may not list namespaces still sees their own
        let k = sample_cluster(NOW);
        k.fail("get namespaces", "Error from server (Forbidden): namespaces is forbidden: User cannot list resource");
        let s = fetch(&k, &target(), NOW);
        assert_eq!(s.cluster, Cluster::Ok);
        assert_eq!(s.workloads.len(), 6);
    }

    #[test]
    fn a_cluster_without_ingress_support_still_shows_the_rest() {
        let k = sample_cluster(NOW);
        k.fail("deployments,statefulsets,daemonsets,jobs,services,ingresses,pods", "error: the server doesn't have a resource type \"ingresses\"");
        k.answer("services,pods,persistentvolumeclaims", r#"{"items":[{"kind":"Deployment","metadata":{"name":"web"},"spec":{"replicas":1,"template":{"spec":{"containers":[]}}},"status":{"readyReplicas":1}}]}"#);
        let s = fetch(&k, &target(), NOW);
        assert_eq!(s.cluster, Cluster::Ok);
        assert!(!s.workloads.is_empty() && s.ingresses.is_empty());
    }

    #[test]
    fn the_feed_refreshes_only_while_visible_and_follows_logs() {
        let k = Arc::new(sample_cluster(NOW));
        let feed = Feed::start(k.clone(), target(), Duration::from_millis(60), Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(200));
        assert!(feed.latest().is_none() && k.calls().is_empty(), "hidden: nothing runs");
        feed.set_visible(true);
        let wait = |f: &dyn Fn() -> bool| {
            for _ in 0..200 {
                if f() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(wait(&|| feed.latest().is_some()));
        assert!(wait(&|| feed.fetches() >= 3), "refreshes on its own");
        feed.follow(Some(("api-5b8d44-uvwxy".into(), String::new())));
        assert!(wait(&|| feed.logs().is_some()));
        assert!(feed.logs().unwrap().1.unwrap().iter().any(|l| l.contains("fatal")));
        feed.request_detail("api-5b8d44-uvwxy");
        assert!(wait(&|| feed.detail().is_some()));
        assert!(feed.detail().unwrap().1.is_ok());
        feed.set_target(Target { namespace: "default".into(), ..Default::default() });
        assert!(feed.latest().is_none() || feed.latest().unwrap().target.namespace == "default");
        assert!(wait(&|| feed.latest().is_some_and(|s| s.target.namespace == "default")));
        feed.set_visible(false);
        std::thread::sleep(Duration::from_millis(100));
        let n = k.calls().len();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(k.calls().len(), n, "hidden again: no more calls");
        drop(feed);
    }
}
