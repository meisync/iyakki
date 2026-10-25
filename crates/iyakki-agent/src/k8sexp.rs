//! The engine of the K8S tile (docs/K8S-TILE.md): explore any namespace of a cluster, one `kubectl get ... -o json` call per list.
//!
//! * [`list`] turns one kind of resource into table rows with columns suited to the kind ([`columns`]); [`namespaces`] gives the
//!   namespace picker its pod counts and failing marks from one call. [`ExpFeed`] keeps both fresh on a thread, only while the tile is
//!   on screen, and slows down when the cluster is slow.
//! * [`get_object`], [`object_yaml`], [`summary`], [`object_events`] and [`related`] are the details pane. A Secret's values never leave
//!   [`object_yaml`] unless the person asked to reveal them ([`MASK`] stands in for them), and lists of Secrets only count keys.
//! * [`fetch_logs`] and [`LogBuf`] are the Logs tab: a bounded buffer that follow mode keeps appending to.
//! * [`Action`] is every change the tile can make. Each one validates its names (DNS-1123), becomes an argument array for the managed
//!   `kubectl` (never a shell) and names what the person has to confirm.
//!
//! Everything runs through the [`Kube`] trait, so tests use a fake cluster and the real thing goes through `wsexec` (managed `k3s
//! kubectl` when the machine has no kubectl, vault-held kubeconfigs as temp files for the duration of one call).

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::k8sretry::{Retry, Status, Tracker, Verdict};
use crate::k8s::{age_of, age_text, at, classify_failure, explain, int, json, parse_ingress, parse_pod, parse_service, parse_time, text, Cluster, EventRow, Health, Kube, Target};

/// Rows the table shows at most ("N more: refine the filter").
pub const MAX_SHOWN: usize = 500;
/// Rows kept in memory per list.
pub const MAX_KEPT: usize = 5000;
/// What a Secret's value is shown as.
pub const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
/// Lines the log buffer keeps.
pub const LOG_CAP: usize = 5000;
/// An action waits this long for kubectl.
pub const ACTION_TIMEOUT_SECS: u64 = 30;
/// The longest a log line is kept.
pub const LOG_LINE_MAX: usize = 2000;
/// Namespaces that can never be deleted from the tile.
pub const PROTECTED_NAMESPACES: [&str; 4] = ["kube-system", "kube-public", "kube-node-lease", "default"];

// ------------------------------------------------------------------------------------------------------------ kinds

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    ReplicaSets,
    Jobs,
    CronJobs,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
    Pvcs,
    Events,
    Nodes,
    Namespaces,
    ServiceAccounts,
    Crds,
}

impl Kind {
    pub const ALL: [Kind; 17] = [Kind::Pods, Kind::Deployments, Kind::StatefulSets, Kind::DaemonSets, Kind::ReplicaSets, Kind::Jobs, Kind::CronJobs, Kind::Services, Kind::Ingresses, Kind::ConfigMaps, Kind::Secrets, Kind::Pvcs, Kind::Events, Kind::Nodes, Kind::Namespaces, Kind::ServiceAccounts, Kind::Crds];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Pods => "Pods",
            Kind::Deployments => "Deployments",
            Kind::StatefulSets => "StatefulSets",
            Kind::DaemonSets => "DaemonSets",
            Kind::ReplicaSets => "ReplicaSets",
            Kind::Jobs => "Jobs",
            Kind::CronJobs => "CronJobs",
            Kind::Services => "Services",
            Kind::Ingresses => "Ingresses",
            Kind::ConfigMaps => "ConfigMaps",
            Kind::Secrets => "Secrets",
            Kind::Pvcs => "PersistentVolumeClaims",
            Kind::Events => "Events",
            Kind::Nodes => "Nodes",
            Kind::Namespaces => "Namespaces",
            Kind::ServiceAccounts => "ServiceAccounts",
            Kind::Crds => "Custom resources",
        }
    }

    /// The name `kubectl get` takes.
    pub fn resource(self) -> &'static str {
        match self {
            Kind::Pods => "pods",
            Kind::Deployments => "deployments",
            Kind::StatefulSets => "statefulsets",
            Kind::DaemonSets => "daemonsets",
            Kind::ReplicaSets => "replicasets",
            Kind::Jobs => "jobs",
            Kind::CronJobs => "cronjobs",
            Kind::Services => "services",
            Kind::Ingresses => "ingresses",
            Kind::ConfigMaps => "configmaps",
            Kind::Secrets => "secrets",
            Kind::Pvcs => "persistentvolumeclaims",
            Kind::Events => "events",
            Kind::Nodes => "nodes",
            Kind::Namespaces => "namespaces",
            Kind::ServiceAccounts => "serviceaccounts",
            Kind::Crds => "customresourcedefinitions",
        }
    }

    /// The kind as the API names it (`Pod`), for event field selectors.
    pub fn api_kind(self) -> &'static str {
        match self {
            Kind::Pods => "Pod",
            Kind::Deployments => "Deployment",
            Kind::StatefulSets => "StatefulSet",
            Kind::DaemonSets => "DaemonSet",
            Kind::ReplicaSets => "ReplicaSet",
            Kind::Jobs => "Job",
            Kind::CronJobs => "CronJob",
            Kind::Services => "Service",
            Kind::Ingresses => "Ingress",
            Kind::ConfigMaps => "ConfigMap",
            Kind::Secrets => "Secret",
            Kind::Pvcs => "PersistentVolumeClaim",
            Kind::Events => "Event",
            Kind::Nodes => "Node",
            Kind::Namespaces => "Namespace",
            Kind::ServiceAccounts => "ServiceAccount",
            Kind::Crds => "CustomResourceDefinition",
        }
    }

    pub fn from_api_kind(k: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|x| x.api_kind() == k)
    }

    pub fn from_resource(r: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|x| x.resource() == r)
    }

    pub fn namespaced(self) -> bool {
        !matches!(self, Kind::Nodes | Kind::Namespaces | Kind::Crds)
    }

    /// The lower-case singular `kubectl rollout` and `scale` take (`deployment`).
    fn singular(self) -> String {
        self.api_kind().to_lowercase()
    }
}

/// Which namespaces a list covers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    All,
    Ns(String),
}

impl Scope {
    pub fn label(&self) -> String {
        match self {
            Scope::All => "All namespaces".into(),
            Scope::Ns(n) => n.clone(),
        }
    }
    /// The name stored in the person's choices (`*` for all).
    pub fn id(&self) -> String {
        match self {
            Scope::All => "*".into(),
            Scope::Ns(n) => n.clone(),
        }
    }
    pub fn from_id(s: &str) -> Scope {
        if s.is_empty() || s == "*" { Scope::All } else { Scope::Ns(s.into()) }
    }
}

// ------------------------------------------------------------------------------------------------------------ names

/// A namespace name: a DNS-1123 label.
pub fn valid_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 63 && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
}

/// A resource name: a DNS-1123 subdomain (labels joined by dots).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(valid_label_part)
}

fn valid_label_part(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 63 && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
}

/// A label selector like `app=web,tier=front`: only the characters labels may hold.
fn valid_selector(s: &str) -> bool {
    !s.is_empty() && s.len() <= 1000 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=,".contains(c))
}

// ------------------------------------------------------------------------------------------------------------ rows

#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub title: &'static str,
    /// Share of the table width.
    pub weight: f32,
    /// 0: always shown; higher numbers are dropped first when the table is narrow.
    pub prio: u8,
}

fn col(title: &'static str, weight: f32, prio: u8) -> Col {
    Col { title, weight, prio }
}

/// The columns of one kind. With `with_ns` (all namespaces) a Namespace column follows the first one.
pub fn columns(kind: Kind, with_ns: bool) -> Vec<Col> {
    let mut c = match kind {
        Kind::Pods => vec![col("Name", 0.26, 0), col("Ready", 0.07, 1), col("Status", 0.14, 0), col("Restarts", 0.08, 2), col("Age", 0.07, 1), col("Node", 0.18, 4), col("IP", 0.12, 5)],
        Kind::Deployments => vec![col("Name", 0.34, 0), col("Ready", 0.14, 0), col("Up-to-date", 0.16, 2), col("Available", 0.16, 1), col("Age", 0.12, 1)],
        Kind::StatefulSets => vec![col("Name", 0.5, 0), col("Ready", 0.2, 0), col("Age", 0.2, 1)],
        Kind::DaemonSets => vec![col("Name", 0.34, 0), col("Ready", 0.14, 0), col("Up-to-date", 0.16, 2), col("Available", 0.16, 1), col("Age", 0.12, 1)],
        Kind::ReplicaSets => vec![col("Name", 0.4, 0), col("Desired", 0.14, 1), col("Current", 0.14, 2), col("Ready", 0.14, 0), col("Age", 0.12, 1)],
        Kind::Jobs => vec![col("Name", 0.4, 0), col("Completions", 0.18, 1), col("Status", 0.2, 0), col("Age", 0.12, 1)],
        Kind::CronJobs => vec![col("Name", 0.32, 0), col("Schedule", 0.2, 1), col("Suspend", 0.1, 2), col("Active", 0.1, 2), col("Last run", 0.14, 1), col("Age", 0.1, 1)],
        Kind::Services => vec![col("Name", 0.22, 0), col("Type", 0.12, 1), col("Cluster IP", 0.15, 2), col("Ports", 0.2, 1), col("External", 0.2, 3), col("Age", 0.08, 1)],
        Kind::Ingresses => vec![col("Name", 0.24, 0), col("Class", 0.12, 2), col("Hosts", 0.34, 0), col("Address", 0.18, 3), col("Age", 0.08, 1)],
        Kind::ConfigMaps => vec![col("Name", 0.6, 0), col("Keys", 0.15, 1), col("Age", 0.15, 1)],
        Kind::Secrets => vec![col("Name", 0.5, 0), col("Type", 0.28, 1), col("Keys", 0.1, 1), col("Age", 0.1, 1)],
        Kind::Pvcs => vec![col("Name", 0.34, 0), col("Status", 0.14, 0), col("Capacity", 0.14, 1), col("Class", 0.2, 2), col("Age", 0.1, 1)],
        Kind::Events => vec![col("Type", 0.09, 0), col("Reason", 0.15, 0), col("Object", 0.24, 1), col("Message", 0.4, 0), col("Age", 0.07, 1)],
        Kind::Nodes => vec![col("Name", 0.2, 0), col("Status", 0.12, 0), col("Roles", 0.14, 2), col("Version", 0.12, 3), col("CPU", 0.07, 1), col("Memory", 0.09, 1), col("CPU use", 0.09, 2), col("Mem use", 0.09, 2), col("Age", 0.06, 3)],
        Kind::Namespaces => vec![col("Name", 0.6, 0), col("Status", 0.2, 0), col("Age", 0.2, 1)],
        Kind::ServiceAccounts => vec![col("Name", 0.6, 0), col("Secrets", 0.2, 1), col("Age", 0.2, 1)],
        Kind::Crds => vec![col("Name", 0.42, 0), col("Group", 0.26, 1), col("Scope", 0.1, 2), col("Kind", 0.14, 0), col("Age", 0.08, 1)],
    };
    if with_ns && kind.namespaced() {
        c.insert(1, col("Namespace", 0.14, 1));
    }
    c
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub namespace: String,
    pub cells: Vec<String>,
    /// A number to sort the cell by (ages in seconds, counts, ratios); `None`: sort by text.
    pub keys: Vec<Option<f64>>,
    pub health: Option<Health>,
    /// An Event of type Warning (shown highlighted).
    pub warn: bool,
}

type Cell = (String, Option<f64>);

fn t(s: impl Into<String>) -> Cell {
    (s.into(), None)
}

fn n(v: i64) -> Cell {
    (v.to_string(), Some(v as f64))
}

fn age_cell(secs: u64) -> Cell {
    (age_text(secs), Some(secs as f64))
}

fn ratio(a: i64, b: i64) -> Cell {
    (format!("{a}/{b}"), Some(if b <= 0 { 0.0 } else { a as f64 / b as f64 }))
}

fn dash(s: String) -> String {
    if s.is_empty() { "\u{2014}".into() } else { s }
}

fn keys_in(v: &Value, path: &[&str]) -> i64 {
    at(v, path).as_object().map_or(0, |m| m.len() as i64)
}

/// `16314232Ki` as `15.6Gi`.
pub fn human_mem(s: &str) -> String {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    let unit = &s[digits.len()..];
    let Ok(v) = digits.parse::<f64>() else { return s.to_string() };
    let bytes = match unit {
        "Ki" => v * 1024.0,
        "Mi" => v * 1024.0 * 1024.0,
        "Gi" => v * 1024.0 * 1024.0 * 1024.0,
        "Ti" => v * 1024f64.powi(4),
        "" => v,
        _ => return s.to_string(),
    };
    if bytes >= 1024f64.powi(3) { format!("{:.1}Gi", bytes / 1024f64.powi(3)) } else { format!("{:.0}Mi", bytes / 1024.0 / 1024.0) }
}

fn workload_health(ready: i64, desired: i64) -> Health {
    if desired == 0 {
        Health::Idle
    } else if ready >= desired {
        Health::Good
    } else if ready == 0 {
        Health::Bad
    } else {
        Health::Warn
    }
}

fn event_time(e: &Value) -> u64 {
    for p in [&["lastTimestamp"][..], &["series", "lastObservedTime"], &["eventTime"], &["firstTimestamp"], &["metadata", "creationTimestamp"]] {
        let t0 = parse_time(&text(e, p));
        if t0 > 0 {
            return t0;
        }
    }
    0
}

fn cells_for(kind: Kind, it: &Value, now: u64) -> (Vec<Cell>, Option<Health>, bool) {
    let age = age_of(it, now);
    match kind {
        Kind::Pods => {
            let p = parse_pod(it, now);
            let deleting = !text(it, &["metadata", "deletionTimestamp"]).is_empty();
            let status = if deleting {
                "Terminating".to_string()
            } else if !p.reason.is_empty() {
                p.reason.clone()
            } else if p.phase == "Succeeded" {
                "Completed".to_string()
            } else {
                p.phase.clone()
            };
            let health = if deleting { Health::Warn } else { p.health };
            let (a, b) = p.ready.split_once('/').map(|(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(0))).unwrap_or((0, 0));
            (vec![t(p.name.clone()), ratio(a, b), t(status), n(p.restarts), age_cell(p.age), t(dash(p.node)), t(dash(text(it, &["status", "podIP"])))], Some(health), false)
        }
        Kind::Deployments => {
            let desired = at(it, &["spec", "replicas"]).as_i64().unwrap_or(1);
            let ready = int(it, &["status", "readyReplicas"]);
            (vec![t(text(it, &["metadata", "name"])), ratio(ready, desired), n(int(it, &["status", "updatedReplicas"])), n(int(it, &["status", "availableReplicas"])), age_cell(age)], Some(workload_health(ready, desired)), false)
        }
        Kind::StatefulSets => {
            let desired = at(it, &["spec", "replicas"]).as_i64().unwrap_or(1);
            let ready = int(it, &["status", "readyReplicas"]);
            (vec![t(text(it, &["metadata", "name"])), ratio(ready, desired), age_cell(age)], Some(workload_health(ready, desired)), false)
        }
        Kind::DaemonSets => {
            let desired = int(it, &["status", "desiredNumberScheduled"]);
            let ready = int(it, &["status", "numberReady"]);
            (vec![t(text(it, &["metadata", "name"])), ratio(ready, desired), n(int(it, &["status", "updatedNumberScheduled"])), n(int(it, &["status", "numberAvailable"])), age_cell(age)], Some(workload_health(ready, desired)), false)
        }
        Kind::ReplicaSets => {
            let desired = at(it, &["spec", "replicas"]).as_i64().unwrap_or(1);
            let ready = int(it, &["status", "readyReplicas"]);
            (vec![t(text(it, &["metadata", "name"])), n(desired), n(int(it, &["status", "replicas"])), n(ready), age_cell(age)], Some(if desired == 0 { Health::Idle } else { workload_health(ready, desired) }), false)
        }
        Kind::Jobs => {
            let done = int(it, &["status", "succeeded"]);
            let want = at(it, &["spec", "completions"]).as_i64().unwrap_or(1);
            let failed = int(it, &["status", "failed"]);
            let (status, h) = if done >= want {
                ("Complete".to_string(), Health::Idle)
            } else if failed > 0 {
                (format!("{failed} failed"), Health::Bad)
            } else {
                ("Running".to_string(), Health::Good)
            };
            (vec![t(text(it, &["metadata", "name"])), ratio(done, want), t(status), age_cell(age)], Some(h), false)
        }
        Kind::CronJobs => {
            let last = parse_time(&text(it, &["status", "lastScheduleTime"]));
            let suspended = at(it, &["spec", "suspend"]).as_bool() == Some(true);
            let active = at(it, &["status", "active"]).as_array().map_or(0, |a| a.len()) as i64;
            (vec![t(text(it, &["metadata", "name"])), t(text(it, &["spec", "schedule"])), t(if suspended { "yes" } else { "no" }), n(active), if last == 0 { t("\u{2014}") } else { age_cell(now.saturating_sub(last)) }, age_cell(age)], Some(if suspended { Health::Idle } else { Health::Good }), false)
        }
        Kind::Services => {
            let s = parse_service(it, now);
            (vec![t(s.name), t(s.kind), t(s.cluster_ip), t(dash(s.ports)), t(dash(s.external)), age_cell(s.age)], None, false)
        }
        Kind::Ingresses => {
            let i = parse_ingress(it, now);
            (vec![t(i.name), t(dash(i.class)), t(dash(i.hosts)), t(dash(i.address)), age_cell(i.age)], None, false)
        }
        Kind::ConfigMaps => (vec![t(text(it, &["metadata", "name"])), n(keys_in(it, &["data"]) + keys_in(it, &["binaryData"])), age_cell(age)], None, false),
        // names, types and a count of keys: the values are never read
        Kind::Secrets => (vec![t(text(it, &["metadata", "name"])), t(text(it, &["type"])), n(keys_in(it, &["data"])), age_cell(age)], None, false),
        Kind::Pvcs => {
            let phase = text(it, &["status", "phase"]);
            let h = match phase.as_str() {
                "Bound" => Health::Good,
                "Lost" => Health::Bad,
                _ => Health::Warn,
            };
            (vec![t(text(it, &["metadata", "name"])), t(phase), t(dash(text(it, &["status", "capacity", "storage"]))), t(dash(text(it, &["spec", "storageClassName"]))), age_cell(age)], Some(h), false)
        }
        Kind::Events => {
            let ty = text(it, &["type"]);
            let warn = ty == "Warning";
            let t0 = event_time(it);
            let object = format!("{}/{}", text(it, &["involvedObject", "kind"]).to_lowercase(), text(it, &["involvedObject", "name"]));
            let msg: String = text(it, &["message"]).replace('\n', " ");
            (vec![t(ty), t(text(it, &["reason"])), t(object), t(msg), if t0 == 0 { t("\u{2014}") } else { age_cell(now.saturating_sub(t0)) }], Some(if warn { Health::Warn } else { Health::Idle }), warn)
        }
        Kind::Nodes => {
            let nd = k8s_node(it);
            let cordoned = at(it, &["spec", "unschedulable"]).as_bool() == Some(true);
            let status = match (nd.ready, cordoned) {
                (true, false) => "Ready".to_string(),
                (true, true) => "Ready,SchedulingDisabled".to_string(),
                (false, false) => "NotReady".to_string(),
                (false, true) => "NotReady,SchedulingDisabled".to_string(),
            };
            let h = if !nd.ready { Health::Bad } else if cordoned { Health::Warn } else { Health::Good };
            (vec![t(nd.name), t(status), t(nd.roles), t(dash(nd.version)), t(dash(nd.cpu)), t(dash(human_mem(&nd.memory))), t("\u{2014}"), t("\u{2014}"), age_cell(age)], Some(h), false)
        }
        Kind::Namespaces => {
            let phase = text(it, &["status", "phase"]);
            let h = if phase == "Active" { Health::Good } else { Health::Warn };
            (vec![t(text(it, &["metadata", "name"])), t(phase), age_cell(age)], Some(h), false)
        }
        Kind::ServiceAccounts => (vec![t(text(it, &["metadata", "name"])), n(at(it, &["secrets"]).as_array().map_or(0, |a| a.len()) as i64), age_cell(age)], None, false),
        Kind::Crds => (vec![t(text(it, &["metadata", "name"])), t(text(it, &["spec", "group"])), t(text(it, &["spec", "scope"])), t(text(it, &["spec", "names", "kind"])), age_cell(age)], None, false),
    }
}

fn k8s_node(it: &Value) -> crate::k8s::NodeInfo {
    crate::k8s::parse_node(it)
}

/// Rows of one kind from the JSON `kubectl get ... -o json` printed. Cells line up with [`columns`] for the same `with_ns`.
pub fn parse_rows(kind: Kind, v: &Value, now: u64, with_ns: bool) -> (Vec<Row>, usize) {
    let items = at(v, &["items"]).as_array().cloned().unwrap_or_default();
    let total = items.len();
    let rows = items
        .iter()
        .take(MAX_KEPT)
        .map(|it| {
            let (cells, health, warn) = cells_for(kind, it, now);
            let namespace = text(it, &["metadata", "namespace"]);
            let mut cells = cells;
            if with_ns && kind.namespaced() {
                cells.insert(1, t(namespace.clone()));
            }
            let name = text(it, &["metadata", "name"]);
            Row { name, namespace, keys: cells.iter().map(|c| c.1).collect(), cells: cells.into_iter().map(|c| c.0).collect(), health, warn }
        })
        .collect();
    (rows, total)
}

/// How two rows compare in a column (numbers by value, the rest by text, case-insensitively; ties by name).
pub fn cmp_rows(a: &Row, b: &Row, col: usize, desc: bool) -> std::cmp::Ordering {
    let o = match (a.keys.get(col).copied().flatten(), b.keys.get(col).copied().flatten()) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cells.get(col).map(|s| s.to_lowercase()).cmp(&b.cells.get(col).map(|s| s.to_lowercase())),
    };
    let o = if o == std::cmp::Ordering::Equal { a.name.cmp(&b.name) } else { o };
    if desc { o.reverse() } else { o }
}

/// Sort rows by a column.
pub fn sort_rows(rows: &mut [Row], col: usize, desc: bool) {
    rows.sort_by(|a, b| cmp_rows(a, b, col, desc));
}

/// The column a kind is sorted by until the person picks another: Events newest first (the Age column, smallest first), the rest by name.
pub fn default_sort(kind: Kind, with_ns: bool) -> (usize, bool) {
    if kind == Kind::Events { (4 + usize::from(with_ns && kind.namespaced()), false) } else { (0, false) }
}

/// Indexes of the rows that contain every word of `query` in their name or cells (case-insensitive).
pub fn filter_rows(rows: &[Row], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| {
            if words.is_empty() {
                return true;
            }
            let hay = format!("{} {}", r.name, r.cells.join(" ")).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .map(|(i, _)| i)
        .collect()
}

/// "12 pods \u{b7} 2 not ready": the count line over the table.
pub fn count_line(kind: Kind, rows: &[Row], total: usize) -> String {
    let shown = rows.len();
    let noun = if kind == Kind::Pods { "pods".to_string() } else { kind.label().to_lowercase() };
    let mut s = format!("{total} {noun}");
    let bad = rows.iter().filter(|r| matches!(r.health, Some(Health::Bad) | Some(Health::Warn))).count();
    match kind {
        Kind::Pods => {
            let notready = rows.iter().filter(|r| !matches!(r.health, Some(Health::Good) | Some(Health::Idle))).count();
            if notready > 0 {
                s.push_str(&format!(" \u{b7} {notready} not ready"));
            }
        }
        Kind::Nodes => {
            let nr = rows.iter().filter(|r| r.health == Some(Health::Bad)).count();
            if nr > 0 {
                s.push_str(&format!(" \u{b7} {nr} not ready"));
            }
        }
        Kind::Events => {
            let w = rows.iter().filter(|r| r.warn).count();
            if w > 0 {
                s.push_str(&format!(" \u{b7} {w} warnings"));
            }
        }
        Kind::Deployments | Kind::StatefulSets | Kind::DaemonSets | Kind::ReplicaSets | Kind::Jobs | Kind::Pvcs => {
            if bad > 0 {
                s.push_str(&format!(" \u{b7} {bad} not ready"));
            }
        }
        _ => {}
    }
    if shown < total {
        s.push_str(&format!(" \u{b7} {} kept", shown));
    }
    s
}

// ------------------------------------------------------------------------------------------------------------ lists

#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub kind: Kind,
    pub scope: Scope,
    pub cluster: Cluster,
    pub rows: Vec<Row>,
    /// How many items the cluster has (more than `rows` when the list was cut at [`MAX_KEPT`]).
    pub total: usize,
    /// Nodes only: whether `kubectl top` answered (metrics-server).
    pub metrics: Option<bool>,
    pub taken_at: u64,
    /// How long the call took, for backing off.
    pub took_ms: u64,
    /// A failed listing whose cause may pass (see `k8sretry`): the feed keeps the last good list on screen.
    pub transient: bool,
}

impl Listing {
    fn failed(kind: Kind, scope: &Scope, cluster: Cluster, now: u64) -> Listing {
        Listing { kind, scope: scope.clone(), cluster, rows: vec![], total: 0, metrics: None, taken_at: now, took_ms: 0, transient: false }
    }
}

/// `kubectl ... get <kind> -o json` for the scope: one call (two for Nodes: `top nodes` fills the usage columns when metrics-server answers).
pub fn list(kube: &dyn Kube, tg: &Target, scope: &Scope, kind: Kind, now: u64) -> Listing {
    list_with(kube, tg, scope, kind, now, &Retry::off())
}

/// [`list`] with the retry policy of the caller: a transient failure of the call is tried again (three quick retries for the feed);
/// a call that works is still exactly one call.
pub fn list_with(kube: &dyn Kube, tg: &Target, scope: &Scope, kind: Kind, now: u64, retry: &Retry) -> Listing {
    if let Scope::Ns(ns) = scope {
        if !valid_label(ns) {
            return Listing::failed(kind, scope, Cluster::Failed(format!("\"{ns}\" is not a namespace name")), now);
        }
    }
    let started = Instant::now();
    let mut a = tg.base_args();
    // `-n` is a global kubectl flag and may come first; `-A` belongs to `get` and kubectl 1.37 rejects it in front of the verb
    // ("flags cannot be placed before plugin name"), so it goes after the resource
    if kind.namespaced() {
        if let Scope::Ns(ns) = scope {
            a.extend(["-n".into(), ns.clone()]);
        }
    }
    a.extend(["get".into(), kind.resource().into()]);
    if kind.namespaced() && *scope == Scope::All {
        a.push("-A".into());
    }
    a.extend(["-o".into(), "json".into()]);
    let att = retry.run(kube, &a);
    let o = att.out;
    if !o.ok() {
        let mut l = Listing::failed(kind, scope, classify_failure(&o), now);
        l.transient = att.verdict == Some(Verdict::Transient);
        return l;
    }
    let Some(v) = json(&o) else {
        let why = if o.truncated { "there are too many items to list at once: choose one namespace or a smaller kind" } else { "kubectl printed something unreadable" };
        return Listing::failed(kind, scope, Cluster::Failed(why.into()), now);
    };
    let with_ns = *scope == Scope::All;
    let (mut rows, total) = parse_rows(kind, &v, now, with_ns);
    let mut metrics = None;
    if kind == Kind::Nodes {
        let mut b = tg.base_args();
        b.extend(["top".into(), "nodes".into(), "--no-headers".into()]);
        let top = kube.run(&b);
        if top.ok() {
            apply_top(&mut rows, &top.stdout);
            metrics = Some(true);
        } else {
            metrics = Some(false);
        }
    }
    let (c, d) = default_sort(kind, with_ns);
    sort_rows(&mut rows, c, d);
    Listing { kind, scope: scope.clone(), cluster: Cluster::Ok, rows, total, metrics, taken_at: now, took_ms: started.elapsed().as_millis() as u64, transient: false }
}

/// Fill the CPU / memory use columns of Nodes from `kubectl top nodes --no-headers` (`name 250m 6% 1024Mi 13%`).
pub fn apply_top(rows: &mut [Row], out: &str) {
    for l in out.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        if let Some(r) = rows.iter_mut().find(|r| r.name == f[0]) {
            if r.cells.len() >= 8 {
                r.cells[6] = format!("{} ({})", f[1], f[2]);
                r.cells[7] = format!("{} ({})", f[3], f[4]);
                r.keys[6] = f[2].trim_end_matches('%').parse().ok();
                r.keys[7] = f[4].trim_end_matches('%').parse().ok();
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsInfo {
    pub name: String,
    pub phase: String,
    pub pods: usize,
    /// Pods that are failing (CrashLoopBackOff, ImagePullBackOff, Failed...).
    pub failing: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsList {
    pub cluster: Cluster,
    pub items: Vec<NsInfo>,
    /// The pod counts are real (false: this account may not list pods of every namespace).
    pub pods_known: bool,
    pub taken_at: u64,
    pub took_ms: u64,
    /// A failed answer whose cause may pass: the feed keeps the last good one.
    pub transient: bool,
}

/// The namespaces with their pod counts: ONE call (`get namespaces,pods -A -o json`), or the namespaces alone when pods may not be listed.
pub fn namespaces(kube: &dyn Kube, tg: &Target, now: u64) -> NsList {
    namespaces_with(kube, tg, now, &Retry::off())
}

/// [`namespaces`] with the retry policy of the caller.
pub fn namespaces_with(kube: &dyn Kube, tg: &Target, now: u64, retry: &Retry) -> NsList {
    let started = Instant::now();
    let mut a = tg.base_args();
    a.extend(["get".into(), "namespaces,pods".into(), "-A".into(), "-o".into(), "json".into()]);
    let mut att = retry.run(kube, &a);
    let mut o = att.out.clone();
    let mut pods_known = true;
    if !o.ok() && att.verdict != Some(Verdict::Transient) && matches!(classify_failure(&o), Cluster::Forbidden(_) | Cluster::Failed(_)) {
        let mut b = tg.base_args();
        b.extend(["get".into(), "namespaces".into(), "-o".into(), "json".into()]);
        att = retry.run(kube, &b);
        o = att.out.clone();
        pods_known = false;
    }
    if !o.ok() {
        return NsList { cluster: classify_failure(&o), items: vec![], pods_known: false, taken_at: now, took_ms: 0, transient: att.verdict == Some(Verdict::Transient) };
    }
    let Some(v) = json(&o) else {
        return NsList { cluster: Cluster::Failed("kubectl printed something unreadable".into()), items: vec![], pods_known: false, taken_at: now, took_ms: 0, transient: false };
    };
    let mut map: BTreeMap<String, NsInfo> = BTreeMap::new();
    let mut pods: Vec<(String, bool)> = Vec::new();
    for it in at(&v, &["items"]).as_array().cloned().unwrap_or_default() {
        match it["kind"].as_str().unwrap_or("") {
            "Pod" => {
                let p = parse_pod(&it, now);
                let deleting = !text(&it, &["metadata", "deletionTimestamp"]).is_empty();
                pods.push((text(&it, &["metadata", "namespace"]), p.health == Health::Bad && !deleting));
            }
            _ => {
                let name = text(&it, &["metadata", "name"]);
                map.insert(name.clone(), NsInfo { name, phase: text(&it, &["status", "phase"]), pods: 0, failing: 0 });
            }
        }
    }
    for (ns, bad) in pods {
        if let Some(i) = map.get_mut(&ns) {
            i.pods += 1;
            i.failing += usize::from(bad);
        }
    }
    NsList { cluster: Cluster::Ok, items: map.into_values().collect(), pods_known, taken_at: now, took_ms: started.elapsed().as_millis() as u64, transient: false }
}

// ------------------------------------------------------------------------------------------------------------ the feed

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub target: Target,
    pub scope: Scope,
    pub kind: Kind,
}

#[derive(Default)]
struct Shared {
    want: Option<Want>,
    listing: Option<Arc<Listing>>,
    ns: Option<Arc<NsList>>,
    refresh: bool,
    fetches: u64,
    ns_fetches: u64,
    /// Milliseconds the last list and namespace calls took (the feed waits at least three times as long between calls).
    slow_ms: u64,
    /// How the failures of the list and of the namespace summary stand (see `k8sretry`).
    tracker: Tracker,
    ns_tracker: Tracker,
}

/// Keeps the current list and the namespace summary fresh on a thread: every `interval` while visible (the namespaces every
/// `ns_interval`), backing off to three times the time a call took when the cluster is slow (at most one minute). The UI never waits.
///
/// A transient failure (see `k8sretry`) is retried three times inside the refresh, the old list stays on screen (see [`ExpFeed::status`])
/// and the next tries come after 2, 4, 8, then 15 s; the failure replaces the list only after the stale window or when it is persistent.
/// Refresh ends a wait at once. One thread makes one call at a time, so two calls for the same list never overlap.
pub struct ExpFeed {
    shared: Arc<Mutex<Shared>>,
    visible: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    retry: Retry,
}

fn cluster_text(c: &Cluster) -> String {
    match c {
        Cluster::Unreachable(m) | Cluster::Forbidden(m) | Cluster::Failed(m) => m.clone(),
        Cluster::KubectlMissing => "kubectl was not found".to_string(),
        Cluster::Ok => String::new(),
    }
}

fn clock() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl ExpFeed {
    pub fn start(kube: Arc<dyn Kube>, interval: Duration, ns_interval: Duration) -> ExpFeed {
        ExpFeed::start_with(kube, interval, ns_interval, Retry::system())
    }

    /// [`ExpFeed::start`] with another retry policy (tests pass a fake clock).
    pub fn start_with(kube: Arc<dyn Kube>, interval: Duration, ns_interval: Duration, retry: Retry) -> ExpFeed {
        let shared = Arc::new(Mutex::new(Shared { refresh: true, ..Default::default() }));
        let (visible, alive) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(true)));
        let (s2, v2, a2, r2) = (shared.clone(), visible.clone(), alive.clone(), retry.clone());
        std::thread::spawn(move || {
            let (mut last, mut last_ns) = (None::<u64>, None::<u64>);
            while a2.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
                if !v2.load(Ordering::Relaxed) {
                    continue;
                }
                let now_ms = r2.timer.now_ms();
                let (want, forced, slow, wait, ns_wait) = {
                    let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
                    let f = s.refresh;
                    s.refresh = false;
                    if f {
                        s.tracker.reset_backoff();
                        s.ns_tracker.reset_backoff();
                    }
                    let back = Duration::from_millis((s.slow_ms * 3).min(60_000));
                    (s.want.clone(), f, s.slow_ms, s.tracker.next_delay(interval.max(back)), s.ns_tracker.next_delay(ns_interval.max(back)))
                };
                let _ = slow;
                let Some(want) = want else { continue };
                let due = forced || last.is_none_or(|t| now_ms.saturating_sub(t) >= wait.as_millis() as u64);
                if due {
                    let epoch = r2.gate.epoch();
                    let l = list_with(&*kube, &want.target, &want.scope, want.kind, r2.timer.wall_secs(), &r2);
                    let cancelled = r2.gate.epoch() != epoch;
                    let done_ms = r2.timer.now_ms();
                    let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
                    if s.want.as_ref() == Some(&want) && !(cancelled && l.transient) {
                        s.slow_ms = l.took_ms;
                        if l.cluster == Cluster::Ok {
                            s.tracker.ok(r2.timer.wall_secs());
                            s.listing = Some(Arc::new(l));
                        } else {
                            let verdict = if l.transient { Verdict::Transient } else { Verdict::Persistent };
                            let (said, starting) = (cluster_text(&l.cluster), r2.hints.starting_now(done_ms));
                            // transient and inside the window: the last good list (or nothing yet) stays
                            if !s.tracker.failed(done_ms, verdict, &said, starting) {
                                s.listing = Some(Arc::new(l));
                            }
                        }
                        s.fetches += 1;
                    }
                    last = Some(done_ms);
                }
                let now_ms = r2.timer.now_ms();
                let ns_due = forced || last_ns.is_none_or(|t| now_ms.saturating_sub(t) >= ns_wait.as_millis() as u64);
                if ns_due {
                    let epoch = r2.gate.epoch();
                    let l = namespaces_with(&*kube, &want.target, r2.timer.wall_secs(), &r2);
                    let cancelled = r2.gate.epoch() != epoch;
                    let done_ms = r2.timer.now_ms();
                    let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
                    if s.want.as_ref().map(|w| &w.target) == Some(&want.target) && !(cancelled && l.transient) {
                        s.slow_ms = s.slow_ms.max(l.took_ms);
                        if l.cluster == Cluster::Ok {
                            s.ns_tracker.ok(r2.timer.wall_secs());
                            s.ns = Some(Arc::new(l));
                        } else {
                            let verdict = if l.transient { Verdict::Transient } else { Verdict::Persistent };
                            let (said, starting) = (cluster_text(&l.cluster), r2.hints.starting_now(done_ms));
                            if !s.ns_tracker.failed(done_ms, verdict, &said, starting) {
                                s.ns = Some(Arc::new(l));
                            }
                        }
                        s.ns_fetches += 1;
                    }
                    last_ns = Some(r2.timer.now_ms());
                }
            }
        });
        ExpFeed { shared, visible, alive, retry }
    }

    pub fn set_visible(&self, on: bool) {
        self.visible.store(on, Ordering::Relaxed);
    }

    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    /// Look at this cluster / scope / kind. The old list is dropped when the target changed (so a stale table never shows under a new
    /// title); the namespaces are dropped only when the cluster changed.
    pub fn set_want(&self, w: Want) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.want.as_ref() != Some(&w) {
            if s.want.as_ref().map(|x| &x.target) != Some(&w.target) {
                s.ns = None;
                s.slow_ms = 0;
            }
            s.listing = None;
            s.tracker = Tracker::default();
            if s.ns.is_none() {
                s.ns_tracker = Tracker::default();
                self.retry.hints.forget();
            }
            s.want = Some(w);
            s.refresh = true;
            self.retry.gate.bump();
        }
    }

    pub fn want(&self) -> Option<Want> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).want.clone()
    }

    /// Fetch now: a wait between retries ends at once and the backoff starts over.
    pub fn refresh(&self) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).refresh = true;
        self.retry.gate.bump();
    }

    /// How the failures of the list stand (nothing while all is well): stale data on screen, the banner due, when it last worked.
    pub fn status(&self) -> Status {
        let s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let (now_ms, now_s) = (self.retry.timer.now_ms(), self.retry.timer.wall_secs());
        let st = s.tracker.status(now_ms, now_s);
        if st.failing { st } else { s.ns_tracker.status(now_ms, now_s) }
    }

    /// The managed k3s is starting: refused connections are expected and the banner waits longer.
    pub fn set_starting(&self, on: bool) {
        self.retry.hints.set_starting(on, self.retry.timer.now_ms());
    }

    /// The cluster is the managed k3s of this computer: a refused connection is a restart, not a wrong address.
    pub fn set_local(&self, on: bool) {
        self.retry.hints.set_local(on);
    }
    /// The cluster log (the last 50 kubectl problems), shared with the IDE's panel code through `k8sretry`.
    pub fn cluster_log(&self) -> Arc<crate::k8sretry::ClusterLog> {
        self.retry.log.clone()
    }

    pub fn listing(&self) -> Option<Arc<Listing>> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).listing.clone()
    }

    pub fn ns(&self) -> Option<Arc<NsList>> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).ns.clone()
    }

    pub fn fetches(&self) -> u64 {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).fetches
    }

    pub fn ns_fetches(&self) -> u64 {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).ns_fetches
    }
}

impl Drop for ExpFeed {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------------------------------------------------ details

fn one_line(o: &crate::wsexec::Out, fallback: &str) -> String {
    o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or(fallback).trim().chars().take(300).collect()
}

/// `kubectl get <kind> <name> -o json` (validated names; Secrets come back with their values, so callers pass the result to
/// [`object_yaml`] / [`summary`] and nothing else).
pub fn get_object(kube: &dyn Kube, tg: &Target, kind: Kind, ns: &str, name: &str) -> Result<Value, String> {
    if !valid_name(name) || (kind.namespaced() && !valid_label(ns)) {
        return Err("not a valid name".into());
    }
    let mut a = tg.base_args();
    if kind.namespaced() {
        a.extend(["-n".into(), ns.into()]);
    }
    a.extend(["get".into(), kind.resource().into(), name.into(), "-o".into(), "json".into()]);
    let o = kube.run(&a);
    if !o.ok() {
        return Err(one_line(&o, "kubectl failed"));
    }
    json(&o).ok_or_else(|| "kubectl printed something unreadable".to_string())
}

/// What a Secret's value is when it is revealed: the text, or a note for bytes that are not text.
fn reveal_value(b64: &str) -> String {
    match crate::models::b64_decode(b64.trim()) {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => format!("(binary, {} bytes)", e.into_bytes().len()),
        },
        None => "(not base64)".to_string(),
    }
}

/// Replace a Secret's values by [`MASK`] right after they are read, so that the values are not kept while the object is shown
/// masked (only a reveal reads them again and keeps them).
pub fn scrub_secret(v: &mut Value) {
    for key in ["data", "stringData"] {
        if let Some(m) = v.get_mut(key).and_then(Value::as_object_mut) {
            for (_, val) in m.iter_mut() {
                *val = Value::String(MASK.to_string());
            }
        }
    }
    if let Some(m) = v.get_mut("metadata").and_then(|m| m.get_mut("annotations")).and_then(Value::as_object_mut) {
        m.remove("kubectl.kubernetes.io/last-applied-configuration");
    }
}

/// The object as the details pane's YAML: Secret values replaced by [`MASK`] unless `reveal`, the last-applied annotation (a copy of
/// the values) always removed.
pub fn object_yaml(kind: Kind, v: &Value, reveal: bool) -> String {
    let mut v = v.clone();
    // (`v["x"]` on a mutable value would add a null entry; `get_mut` leaves what is not there alone)
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        if let Some(m) = meta.get_mut("annotations").and_then(Value::as_object_mut) {
            m.remove("kubectl.kubernetes.io/last-applied-configuration");
            if m.is_empty() {
                meta.remove("annotations");
            }
        }
        meta.remove("managedFields");
    }
    if kind == Kind::Secrets {
        for key in ["data", "stringData"] {
            if let Some(m) = v.get_mut(key).and_then(Value::as_object_mut) {
                for (_, val) in m.iter_mut() {
                    let shown = if reveal {
                        if key == "data" { reveal_value(val.as_str().unwrap_or("")) } else { val.as_str().unwrap_or("").to_string() }
                    } else {
                        MASK.to_string()
                    };
                    *val = Value::String(shown);
                }
            }
        }
    }
    to_yaml(&v)
}

fn yaml_scalar(s: &str) -> String {
    let plain_ok = !s.is_empty()
        && !s.starts_with(|c: char| c.is_whitespace() || "-?:,[]{}#&*!|>'\"%@`".contains(c))
        && !s.ends_with(char::is_whitespace)
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with(':')
        && !s.contains(['\n', '\t', '\r'])
        && s.parse::<f64>().is_err()
        && !matches!(s.to_lowercase().as_str(), "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~");
    if plain_ok {
        return s.to_string();
    }
    let mut q = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            '\t' => q.push_str("\\t"),
            '\r' => q.push_str("\\r"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

fn yaml_key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) { k.to_string() } else { yaml_scalar(k) }
}

fn yaml_into(v: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match v {
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str(&format!("{pad}{{}}\n"));
            }
            for (k, val) in m {
                yaml_entry(&format!("{pad}{}:", yaml_key(k)), val, indent, out);
            }
        }
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str(&format!("{pad}[]\n"));
            }
            for val in a {
                yaml_entry(&format!("{pad}-"), val, indent, out);
            }
        }
        other => out.push_str(&format!("{pad}{}\n", scalar_text(other))),
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_scalar(s),
        _ => String::new(),
    }
}

fn yaml_entry(head: &str, val: &Value, indent: usize, out: &mut String) {
    match val {
        Value::Object(m) if !m.is_empty() => {
            if head.ends_with('-') {
                // "- key: value" with the rest of the keys lined up under the first
                let mut inner = String::new();
                yaml_into(val, indent + 2, &mut inner);
                let first = inner.trim_start_matches(' ');
                out.push_str(&format!("{head} {first}"));
            } else {
                out.push_str(&format!("{head}\n"));
                yaml_into(val, indent + 2, out);
            }
        }
        Value::Array(a) if !a.is_empty() => {
            if head.ends_with('-') {
                let mut inner = String::new();
                yaml_into(val, indent + 2, &mut inner);
                let first = inner.trim_start_matches(' ');
                out.push_str(&format!("{head} {first}"));
            } else {
                out.push_str(&format!("{head}\n"));
                yaml_into(val, indent, out);
            }
        }
        Value::Object(_) => out.push_str(&format!("{head} {{}}\n")),
        Value::Array(_) => out.push_str(&format!("{head} []\n")),
        Value::String(s) if s.contains('\n') && !head.ends_with('-') => {
            out.push_str(&format!("{head} |\n"));
            let pad = " ".repeat(indent + 2);
            for l in s.trim_end_matches('\n').split('\n') {
                out.push_str(&format!("{pad}{l}\n"));
            }
        }
        other => out.push_str(&format!("{head} {}\n", scalar_text(other))),
    }
}

/// A small YAML writer for what the API returns (objects, lists, strings, numbers): keys in the order of the value (the API's
/// is alphabetical), long text as a block.
pub fn to_yaml(v: &Value) -> String {
    let mut out = String::new();
    yaml_into(v, 0, &mut out);
    out
}

fn owner_of(v: &Value) -> Option<(String, String)> {
    at(v, &["metadata", "ownerReferences"]).as_array().and_then(|a| a.first()).map(|o| (text(o, &["kind"]), text(o, &["name"])))
}

fn pod_status_words(v: &Value, now: u64) -> String {
    let p = parse_pod(v, now);
    if !text(v, &["metadata", "deletionTimestamp"]).is_empty() {
        return "Terminating: it is being removed".into();
    }
    match p.health {
        Health::Good => "Running and ready".into(),
        Health::Idle => "Finished (completed)".into(),
        _ if !p.reason.is_empty() => format!("{} \u{2014} {}", p.reason, explain(&p.reason).map_or("check the events", |_| "it is not working yet")),
        _ => format!("{} (not ready: {})", p.phase, p.ready),
    }
}

/// Key fields of an object in plain words, (label, value) pairs.
pub fn summary(kind: Kind, v: &Value, now: u64) -> Vec<(String, String)> {
    let mut o: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, val: String| {
        if !val.is_empty() {
            o.push((k.to_string(), val));
        }
    };
    add("Name", text(v, &["metadata", "name"]));
    if kind.namespaced() {
        add("Namespace", text(v, &["metadata", "namespace"]));
    }
    let age = age_of(v, now);
    if age > 0 {
        add("Created", format!("{} ago", age_text(age)));
    }
    match kind {
        Kind::Pods => {
            let p = parse_pod(v, now);
            add("Status", pod_status_words(v, now));
            if let Some(e) = explain(&p.reason) {
                add("What it means", e.to_string());
            }
            add("Containers ready", p.ready.clone());
            add("Restarts", p.restarts.to_string());
            add("Node", p.node);
            add("Pod IP", text(v, &["status", "podIP"]));
            if let Some((k, n)) = owner_of(v) {
                add("Owned by", format!("{k} {n} (it recreates the pod if it is deleted)"));
            } else {
                add("Owned by", "nothing (deleting it removes it for good)".into());
            }
            let images: Vec<String> = at(v, &["spec", "containers"]).as_array().map(|a| a.iter().map(|c| format!("{} \u{2192} {}", text(c, &["name"]), text(c, &["image"]))).collect()).unwrap_or_default();
            add("Containers", images.join("; "));
            add("QoS class", text(v, &["status", "qosClass"]));
        }
        Kind::Deployments | Kind::StatefulSets | Kind::ReplicaSets => {
            let desired = at(v, &["spec", "replicas"]).as_i64().unwrap_or(1);
            add("Replicas", format!("{} ready of {} wanted", int(v, &["status", "readyReplicas"]), desired));
            add("Strategy", text(v, &["spec", "strategy", "type"]));
            add("Images", images_of(v));
            add("Selector", selector_string(v).unwrap_or_default());
        }
        Kind::DaemonSets => {
            add("Pods", format!("{} ready of {} wanted (one per node)", int(v, &["status", "numberReady"]), int(v, &["status", "desiredNumberScheduled"])));
            add("Images", images_of(v));
        }
        Kind::Jobs => {
            add("Completions", format!("{} of {}", int(v, &["status", "succeeded"]), at(v, &["spec", "completions"]).as_i64().unwrap_or(1)));
            add("Failed attempts", int(v, &["status", "failed"]).to_string());
            add("Images", images_of(v));
        }
        Kind::CronJobs => {
            add("Schedule", text(v, &["spec", "schedule"]));
            add("Suspended", if at(v, &["spec", "suspend"]).as_bool() == Some(true) { "yes".into() } else { "no".into() });
            add("Last run", text(v, &["status", "lastScheduleTime"]));
        }
        Kind::Services => {
            let s = parse_service(v, now);
            add("Type", s.kind);
            add("Cluster IP", s.cluster_ip);
            add("Ports", s.ports);
            add("External address", dash_none(s.external));
            let sel = at(v, &["spec", "selector"]).as_object().map(|m| m.iter().map(|(k, x)| format!("{k}={}", x.as_str().unwrap_or(""))).collect::<Vec<_>>().join(",")).unwrap_or_default();
            add("Sends traffic to pods with", sel);
        }
        Kind::Ingresses => {
            let i = parse_ingress(v, now);
            add("Class", i.class);
            add("Hosts", i.hosts);
            add("Address", dash_none(i.address));
        }
        Kind::ConfigMaps => {
            let keys: Vec<&String> = at(v, &["data"]).as_object().map(|m| m.keys().collect()).unwrap_or_default();
            add("Keys", keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
        }
        Kind::Secrets => {
            add("Type", text(v, &["type"]));
            let keys: Vec<&String> = at(v, &["data"]).as_object().map(|m| m.keys().collect()).unwrap_or_default();
            add("Keys", keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
            add("Values", "hidden (the YAML tab shows them as dots until you reveal them)".into());
        }
        Kind::Pvcs => {
            add("Status", text(v, &["status", "phase"]));
            add("Size", text(v, &["status", "capacity", "storage"]));
            add("Storage class", text(v, &["spec", "storageClassName"]));
            add("Volume", text(v, &["spec", "volumeName"]));
        }
        Kind::Events => {
            add("Type", text(v, &["type"]));
            add("Reason", text(v, &["reason"]));
            add("About", format!("{}/{}", text(v, &["involvedObject", "kind"]).to_lowercase(), text(v, &["involvedObject", "name"])));
            add("Message", text(v, &["message"]));
            add("Times seen", int(v, &["count"]).max(1).to_string());
        }
        Kind::Nodes => {
            let nd = k8s_node(v);
            add("Status", if nd.ready { "Ready".into() } else { "Not ready".into() });
            if at(v, &["spec", "unschedulable"]).as_bool() == Some(true) {
                add("Scheduling", "cordoned: no new pods are placed here".into());
            }
            add("Roles", nd.roles);
            add("Version", nd.version);
            add("Address", nd.ip);
            add("CPU", nd.cpu);
            add("Memory", human_mem(&nd.memory));
            add("OS image", text(v, &["status", "nodeInfo", "osImage"]));
        }
        Kind::Namespaces => add("Status", text(v, &["status", "phase"])),
        Kind::ServiceAccounts => add("Secrets", at(v, &["secrets"]).as_array().map_or(0, |a| a.len()).to_string()),
        Kind::Crds => {
            add("Group", text(v, &["spec", "group"]));
            add("Kind", text(v, &["spec", "names", "kind"]));
            add("Scope", text(v, &["spec", "scope"]));
        }
    }
    if let Some(m) = at(v, &["metadata", "labels"]).as_object() {
        if !m.is_empty() {
            let l = m.iter().map(|(k, x)| format!("{k}={}", x.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ");
            o.push(("Labels".into(), l.chars().take(300).collect()));
        }
    }
    o
}

fn dash_none(s: String) -> String {
    if s.is_empty() { "none".into() } else { s }
}

fn images_of(v: &Value) -> String {
    let path: &[&str] = if v["kind"] == "CronJob" { &["spec", "jobTemplate", "spec", "template", "spec", "containers"] } else { &["spec", "template", "spec", "containers"] };
    at(v, path).as_array().map(|a| a.iter().map(|c| text(c, &["image"])).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// `app=web,tier=front` from a workload's `spec.selector.matchLabels` or a Service's `spec.selector`.
pub fn selector_string(v: &Value) -> Option<String> {
    let m = at(v, &["spec", "selector", "matchLabels"]).as_object().or_else(|| at(v, &["spec", "selector"]).as_object().filter(|m| m.values().all(Value::is_string)))?;
    let s = m.iter().map(|(k, x)| format!("{k}={}", x.as_str().unwrap_or(""))).collect::<Vec<_>>().join(",");
    if valid_selector(&s) { Some(s) } else { None }
}

/// Events about one object, newest first (at most 50).
pub fn object_events(kube: &dyn Kube, tg: &Target, kind: Kind, ns: &str, name: &str, now: u64) -> Result<Vec<EventRow>, String> {
    if !valid_name(name) || (kind.namespaced() && !valid_label(ns)) {
        return Err("not a valid name".into());
    }
    let mut a = tg.base_args();
    if kind.namespaced() {
        a.extend(["-n".into(), ns.into()]);
    }
    a.extend(["get".into(), "events".into()]);
    if !kind.namespaced() {
        a.push("-A".into());
    }
    a.extend(["--field-selector".into(), format!("involvedObject.name={name},involvedObject.kind={}", kind.api_kind()), "-o".into(), "json".into()]);
    let o = kube.run(&a);
    if !o.ok() {
        return Err(one_line(&o, "kubectl failed"));
    }
    let v = json(&o).ok_or("kubectl printed something unreadable")?;
    let mut ev: Vec<EventRow> = at(&v, &["items"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let t0 = event_time(e);
            EventRow { kind: text(e, &["type"]), reason: text(e, &["reason"]), message: text(e, &["message"]).chars().take(400).collect(), age: if t0 == 0 { 0 } else { now.saturating_sub(t0) }, count: e["count"].as_i64().unwrap_or(1) }
        })
        .collect();
    ev.sort_by_key(|e| e.age);
    ev.truncate(50);
    Ok(ev)
}

/// One related object: how it is related, and where to find it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rel {
    pub relation: String,
    pub kind: Kind,
    pub namespace: String,
    pub name: String,
    pub note: String,
}

fn rel(relation: &str, kind: Kind, ns: &str, name: &str, note: &str) -> Rel {
    Rel { relation: relation.into(), kind, namespace: ns.into(), name: name.into(), note: note.into() }
}

fn labels_of(v: &Value) -> BTreeMap<String, String> {
    at(v, &["metadata", "labels"]).as_object().map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default()
}

fn fetch_items(kube: &dyn Kube, tg: &Target, args: &[&str]) -> Vec<Value> {
    let mut a = tg.base_args();
    a.extend(args.iter().map(|s| s.to_string()));
    let o = kube.run(&a);
    if !o.ok() {
        return vec![];
    }
    json(&o).map(|v| at(&v, &["items"]).as_array().cloned().unwrap_or_default()).unwrap_or_default()
}

/// What an object is tied to: pod \u{2192} owner \u{2192} deployment, and the services that select it; service \u{2192} endpoints and pods; workload \u{2192}
/// pods and services; node \u{2192} its pods. At most three read-only calls; a failed call just leaves its rows out.
pub fn related(kube: &dyn Kube, tg: &Target, kind: Kind, ns: &str, obj: &Value) -> Vec<Rel> {
    let name = text(obj, &["metadata", "name"]);
    let mut out: Vec<Rel> = Vec::new();
    if !valid_name(&name) || (kind.namespaced() && !valid_label(ns)) {
        return out;
    }
    match kind {
        Kind::Pods => {
            if let Some((ok, on)) = owner_of(obj) {
                if let Some(k) = Kind::from_api_kind(&ok) {
                    out.push(rel("Owner", k, ns, &on, "recreates this pod if it goes away"));
                }
                if ok == "ReplicaSet" && valid_name(&on) {
                    if let Ok(rs) = get_object(kube, tg, Kind::ReplicaSets, ns, &on) {
                        if let Some((k2, n2)) = owner_of(&rs) {
                            if let Some(k) = Kind::from_api_kind(&k2) {
                                out.push(rel("Controlled by", k, ns, &n2, ""));
                            }
                        }
                    }
                }
            }
            let labels = labels_of(obj);
            for s in fetch_items(kube, tg, &["-n", ns, "get", "services", "-o", "json"]) {
                let sel: BTreeMap<String, String> = at(&s, &["spec", "selector"]).as_object().map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                if !sel.is_empty() && sel.iter().all(|(k, x)| labels.get(k) == Some(x)) {
                    out.push(rel("Service selecting it", Kind::Services, ns, &text(&s, &["metadata", "name"]), &parse_service(&s, 0).ports));
                }
            }
            let node = text(obj, &["spec", "nodeName"]);
            if !node.is_empty() {
                out.push(rel("Runs on node", Kind::Nodes, "", &node, ""));
            }
        }
        Kind::Services => {
            if let Some(sel) = selector_string(obj) {
                for p in fetch_items(kube, tg, &["-n", ns, "get", "pods", "-l", &sel, "-o", "json"]).iter().take(30) {
                    let pp = parse_pod(p, 0);
                    out.push(rel("Pod behind it", Kind::Pods, ns, &pp.name, &format!("{} {}", pp.ready, pp.phase)));
                }
            }
            let mut a = tg.base_args();
            a.extend(["-n".into(), ns.into(), "get".into(), "endpoints".into(), name.clone(), "-o".into(), "json".into()]);
            let o = kube.run(&a);
            if let Some(e) = o.ok().then(|| json(&o)).flatten() {
                let (mut ready, mut notready) = (0, 0);
                for sub in at(&e, &["subsets"]).as_array().cloned().unwrap_or_default() {
                    ready += at(&sub, &["addresses"]).as_array().map_or(0, |a| a.len());
                    notready += at(&sub, &["notReadyAddresses"]).as_array().map_or(0, |a| a.len());
                }
                out.push(rel("Endpoints", Kind::Services, ns, &name, &format!("{ready} ready, {notready} not ready")));
            }
        }
        Kind::Deployments | Kind::StatefulSets | Kind::DaemonSets | Kind::ReplicaSets | Kind::Jobs => {
            if let Some(sel) = selector_string(obj) {
                for p in fetch_items(kube, tg, &["-n", ns, "get", "pods", "-l", &sel, "-o", "json"]).iter().take(30) {
                    let pp = parse_pod(p, 0);
                    out.push(rel("Pod", Kind::Pods, ns, &pp.name, &format!("{} {}", pp.ready, if pp.reason.is_empty() { pp.phase.clone() } else { pp.reason.clone() })));
                }
            }
            if let Some((ok, on)) = owner_of(obj) {
                if let Some(k) = Kind::from_api_kind(&ok) {
                    out.push(rel("Owner", k, ns, &on, ""));
                }
            }
            let labels: BTreeMap<String, String> = at(obj, &["spec", "template", "metadata", "labels"]).as_object().map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            for s in fetch_items(kube, tg, &["-n", ns, "get", "services", "-o", "json"]) {
                let sel: BTreeMap<String, String> = at(&s, &["spec", "selector"]).as_object().map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                if !sel.is_empty() && sel.iter().all(|(k, x)| labels.get(k) == Some(x)) {
                    out.push(rel("Service in front", Kind::Services, ns, &text(&s, &["metadata", "name"]), ""));
                }
            }
        }
        Kind::Nodes => {
            let field = format!("spec.nodeName={name}");
            for p in fetch_items(kube, tg, &["get", "pods", "-A", "--field-selector", &field, "-o", "json"]).iter().take(50) {
                let pp = parse_pod(p, 0);
                out.push(rel("Pod on this node", Kind::Pods, &text(p, &["metadata", "namespace"]), &pp.name, &format!("{} {}", pp.ready, pp.phase)));
            }
        }
        _ => {
            if let Some((ok, on)) = owner_of(obj) {
                if let Some(k) = Kind::from_api_kind(&ok) {
                    out.push(rel("Owner", k, ns, &on, ""));
                }
            }
        }
    }
    out
}

/// The container names of a pod, init containers last.
pub fn pod_containers(v: &Value) -> Vec<String> {
    let mut c: Vec<String> = at(v, &["spec", "containers"]).as_array().map(|a| a.iter().map(|x| text(x, &["name"])).collect()).unwrap_or_default();
    c.extend(at(v, &["spec", "initContainers"]).as_array().map(|a| a.iter().map(|x| text(x, &["name"])).collect::<Vec<_>>()).unwrap_or_default());
    c
}

/// The command to run in a terminal for a shell in the pod (the tile has no terminal of its own).
pub fn exec_command(tg: &Target, ns: &str, pod: &str, container: &str) -> String {
    let mut s = String::from("kubectl");
    if !tg.context.is_empty() {
        s.push_str(&format!(" --context {}", tg.context));
    }
    s.push_str(&format!(" -n {ns} exec -it {pod}"));
    if !container.is_empty() {
        s.push_str(&format!(" -c {container}"));
    }
    s.push_str(" -- sh");
    s
}

// ------------------------------------------------------------------------------------------------------------ logs

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogReq {
    pub container: String,
    pub previous: bool,
    pub tail: usize,
    /// Follow mode asks only for what came after this timestamp.
    pub since_time: Option<String>,
}

impl LogReq {
    pub const TAILS: [usize; 3] = [100, 500, 2000];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub ts: String,
    pub text: String,
}

/// The arguments of `kubectl logs` for the request.
pub fn logs_args(tg: &Target, ns: &str, pod: &str, r: &LogReq) -> Result<Vec<String>, String> {
    if !valid_label(ns) || !valid_name(pod) || (!r.container.is_empty() && !valid_label_part(&r.container.replace('.', "-"))) {
        return Err("not a valid pod name".into());
    }
    let mut a = tg.base_args();
    a.extend(["-n".into(), ns.into(), "logs".into(), pod.into(), "--timestamps".into()]);
    if !r.container.is_empty() {
        a.extend(["-c".into(), r.container.clone()]);
    }
    if r.previous {
        a.push("--previous".into());
    }
    match &r.since_time {
        Some(ts) if ts.len() <= 40 && ts.chars().all(|c| c.is_ascii_digit() || "-:.TZ+".contains(c)) => a.push(format!("--since-time={ts}")),
        _ => a.push(format!("--tail={}", r.tail.clamp(1, 2000))),
    }
    Ok(a)
}

fn split_ts(l: &str) -> LogLine {
    if let Some((ts, rest)) = l.split_once(' ') {
        if ts.len() >= 20 && ts.as_bytes()[4] == b'-' && ts.contains('T') && ts.ends_with('Z') {
            return LogLine { ts: ts.to_string(), text: rest.chars().take(LOG_LINE_MAX).collect() };
        }
    }
    LogLine { ts: String::new(), text: l.chars().take(LOG_LINE_MAX).collect() }
}

pub fn fetch_logs(kube: &dyn Kube, tg: &Target, ns: &str, pod: &str, r: &LogReq) -> Result<Vec<LogLine>, String> {
    let a = logs_args(tg, ns, pod, r)?;
    let o = kube.run(&a);
    if !o.ok() {
        return Err(one_line(&o, "kubectl logs failed"));
    }
    Ok(o.stdout.lines().map(split_ts).collect())
}

/// The log lines in view: never more than `cap`; the oldest fall off the front.
#[derive(Clone, Debug)]
pub struct LogBuf {
    pub cap: usize,
    pub lines: VecDeque<LogLine>,
    /// Lines that fell off the front.
    pub dropped: u64,
}

impl LogBuf {
    pub fn new(cap: usize) -> LogBuf {
        LogBuf { cap: cap.max(1), lines: VecDeque::new(), dropped: 0 }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.dropped = 0;
    }

    pub fn last_ts(&self) -> Option<String> {
        self.lines.iter().rev().find(|l| !l.ts.is_empty()).map(|l| l.ts.clone())
    }

    fn push(&mut self, l: LogLine) {
        self.lines.push_back(l);
        while self.lines.len() > self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    /// Replace everything with a fresh batch.
    pub fn set(&mut self, batch: Vec<LogLine>) {
        self.clear();
        for l in batch {
            self.push(l);
        }
    }

    /// Append what a follow poll brought that the buffer does not have yet (lines are told apart by their timestamps; lines that
    /// share the newest timestamp are counted). Returns how many lines were added.
    pub fn merge(&mut self, batch: Vec<LogLine>) -> usize {
        let Some(last) = self.last_ts() else {
            let n = batch.len();
            for l in batch {
                self.push(l);
            }
            return n;
        };
        let held_same = self.lines.iter().rev().take_while(|l| l.ts == last || l.ts.is_empty()).filter(|l| l.ts == last).count();
        let mut seen_same = 0;
        let mut added = 0;
        for l in batch {
            if l.ts.is_empty() {
                continue;
            }
            if l.ts.as_str() > last.as_str() {
                self.push(l);
                added += 1;
            } else if l.ts == last {
                seen_same += 1;
                if seen_same > held_same {
                    self.push(l);
                    added += 1;
                }
            }
        }
        added
    }
}

// ------------------------------------------------------------------------------------------------------------ actions

/// What the person has to do to confirm an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// A card naming the object with a Yes / No.
    Click,
    /// Type the object's name first.
    TypeName(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Restart { kind: Kind, ns: String, name: String },
    Scale { kind: Kind, ns: String, name: String, replicas: u32 },
    DeletePod { ns: String, name: String },
    Cordon { node: String },
    Uncordon { node: String },
    Delete { kind: Kind, ns: String, name: String },
}

pub const MAX_REPLICAS: u32 = 1000;

impl Action {
    pub fn validate(&self) -> Result<(), String> {
        let ns_ok = |ns: &str| if valid_label(ns) { Ok(()) } else { Err(format!("\"{ns}\" is not a namespace name")) };
        let name_ok = |n: &str| if valid_name(n) { Ok(()) } else { Err(format!("\"{n}\" is not a valid resource name")) };
        match self {
            Action::Restart { kind, ns, name } => {
                if !matches!(kind, Kind::Deployments | Kind::StatefulSets | Kind::DaemonSets) {
                    return Err(format!("{} cannot be restarted this way", kind.label()));
                }
                ns_ok(ns)?;
                name_ok(name)
            }
            Action::Scale { kind, ns, name, replicas } => {
                if !matches!(kind, Kind::Deployments | Kind::StatefulSets | Kind::ReplicaSets) {
                    return Err(format!("{} cannot be scaled", kind.label()));
                }
                if *replicas > MAX_REPLICAS {
                    return Err(format!("at most {MAX_REPLICAS} replicas"));
                }
                ns_ok(ns)?;
                name_ok(name)
            }
            Action::DeletePod { ns, name } => {
                ns_ok(ns)?;
                name_ok(name)
            }
            Action::Cordon { node } | Action::Uncordon { node } => name_ok(node),
            Action::Delete { kind, ns, name } => {
                if matches!(kind, Kind::Pods | Kind::Nodes | Kind::Namespaces | Kind::Events | Kind::Crds) {
                    return Err(format!("{} cannot be deleted with this button", kind.label()));
                }
                ns_ok(ns)?;
                name_ok(name)
            }
        }
    }

    /// The argument arrays (after `kubectl`) in order. The first must work; a failing later step only adds a note.
    pub fn steps(&self, tg: &Target) -> Result<Vec<Vec<String>>, String> {
        self.validate()?;
        let base = tg.base_args_secs(ACTION_TIMEOUT_SECS - 5);
        let with = |extra: &[&str]| {
            let mut a = base.clone();
            a.extend(extra.iter().map(|s| s.to_string()));
            a
        };
        Ok(match self {
            Action::Restart { kind, ns, name } => {
                let target = format!("{}/{name}", kind.singular());
                vec![with(&["-n", ns, "rollout", "restart", &target]), with(&["-n", ns, "rollout", "status", &target, "--timeout=20s"])]
            }
            Action::Scale { kind, ns, name, replicas } => vec![with(&["-n", ns, "scale", &format!("{}/{name}", kind.singular()), &format!("--replicas={replicas}")])],
            Action::DeletePod { ns, name } => vec![with(&["-n", ns, "delete", "pod", name, "--wait=false"])],
            Action::Cordon { node } => vec![with(&["cordon", node])],
            Action::Uncordon { node } => vec![with(&["uncordon", node])],
            Action::Delete { kind, ns, name } => vec![with(&["-n", ns, "delete", kind.resource(), name, "--wait=false"])],
        })
    }

    pub fn confirm(&self) -> Confirm {
        match self {
            Action::Delete { name, .. } => Confirm::TypeName(name.clone()),
            _ => Confirm::Click,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Action::Restart { kind, name, .. } => format!("Restart {} {name}", kind.api_kind().to_lowercase()),
            Action::Scale { kind, name, replicas, .. } => format!("Scale {} {name} to {replicas}", kind.api_kind().to_lowercase()),
            Action::DeletePod { name, .. } => format!("Delete pod {name}"),
            Action::Cordon { node } => format!("Cordon node {node}"),
            Action::Uncordon { node } => format!("Uncordon node {node}"),
            Action::Delete { kind, name, .. } => format!("Delete {} {name}", kind.api_kind().to_lowercase()),
        }
    }

    /// What the confirmation card says will happen, in plain words. `owned` (pods): the pod has a controller that recreates it.
    pub fn warning(&self, owned: bool) -> String {
        match self {
            Action::Restart { .. } => "The pods are replaced one after another with new ones. Traffic keeps flowing while that happens.".into(),
            Action::Scale { replicas: 0, .. } => "Scaling to 0 stops every pod of it. Nothing serves until you scale it up again.".into(),
            Action::Scale { .. } => "The number of pods changes to the number you chose.".into(),
            Action::DeletePod { .. } if owned => "The pod stops. Its controller creates a new one at once, so this restarts it.".into(),
            Action::DeletePod { .. } => "Nothing owns this pod, so nothing recreates it: it is gone for good.".into(),
            Action::Cordon { .. } => "No new pods are placed on this node. Pods already on it keep running.".into(),
            Action::Uncordon { .. } => "The node accepts new pods again.".into(),
            Action::Delete { .. } => "It is removed from the cluster. This cannot be undone, and anything that depends on it stops working.".into(),
        }
    }

    pub fn audit_name(&self) -> &'static str {
        match self {
            Action::Restart { .. } => "k8s.restart",
            Action::Scale { .. } => "k8s.scale",
            Action::DeletePod { .. } => "k8s.delete.pod",
            Action::Cordon { .. } => "k8s.cordon",
            Action::Uncordon { .. } => "k8s.uncordon",
            Action::Delete { .. } => "k8s.delete",
        }
    }

    /// Who is not part of this: cluster, namespace, kind and name only. Never a value.
    pub fn audit_detail(&self, cluster: &str) -> String {
        match self {
            Action::Restart { kind, ns, name } => format!("cluster={cluster} ns={ns} {}/{name}", kind.api_kind()),
            Action::Scale { kind, ns, name, replicas } => format!("cluster={cluster} ns={ns} {}/{name} replicas={replicas}", kind.api_kind()),
            Action::DeletePod { ns, name } => format!("cluster={cluster} ns={ns} Pod/{name}"),
            Action::Cordon { node } | Action::Uncordon { node } => format!("cluster={cluster} Node/{node}"),
            Action::Delete { kind, ns, name } => format!("cluster={cluster} ns={ns} {}/{name}", kind.api_kind()),
        }
    }
}

/// Run the steps of an action; `emit` gets each output line as it is known. Ok: a one-line result for the toast.
pub fn run_action(kube: &dyn Kube, tg: &Target, a: &Action, emit: &mut dyn FnMut(&str)) -> Result<String, String> {
    let steps = a.steps(tg)?;
    let mut result = String::new();
    for (i, args) in steps.iter().enumerate() {
        emit(&format!("$ kubectl {}", shown_args(args)));
        let o = kube.run_with(args, ACTION_TIMEOUT_SECS);
        for l in o.combined().lines().filter(|l| !l.trim().is_empty()).take(40) {
            emit(l);
        }
        if o.missing {
            return Err("kubectl was not found".into());
        }
        if !o.ok() {
            if i == 0 {
                let why = if o.timed_out { "kubectl timed out after 30 seconds".to_string() } else { one_line(&o, "kubectl failed") };
                return Err(why);
            }
            emit("(the change was sent; it had not finished within the time shown)");
            result.push_str(" It was sent; it is still going.");
        } else if i == 0 {
            result = format!("{} done.", a.title());
        }
    }
    Ok(result)
}

/// The arguments for the panel: the kubeconfig path (it may name a vault entry) and context are left out.
fn shown_args(args: &[String]) -> String {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if x == "--kubeconfig" || x == "--context" {
            it.next();
        } else if !x.starts_with("--request-timeout") {
            out.push(x.as_str());
        }
    }
    out.join(" ")
}

// ------------------------------------------------------------------------------------------------------------ choices

/// What the tile remembers about one cluster.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterPrefs {
    pub namespace: String,
    pub kind: String,
    pub allow_changes: bool,
}

/// The person's choices, kept next to their other settings (`k8s-tile-<user>.json`): the last cluster, and per cluster the
/// namespace, the kind and the Allow changes switch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub cluster: String,
    pub per: BTreeMap<String, ClusterPrefs>,
}

impl Prefs {
    fn path(dir: &Path, user: &str) -> std::path::PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("k8s-tile-{safe}.json"))
    }

    pub fn load(dir: &Path, user: &str) -> Prefs {
        std::fs::read_to_string(Self::path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(dir, user), serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    pub fn of(&self, cluster: &str) -> ClusterPrefs {
        self.per.get(cluster).cloned().unwrap_or_default()
    }

    pub fn edit(&mut self, cluster: &str, f: impl FnOnce(&mut ClusterPrefs)) {
        f(self.per.entry(cluster.to_string()).or_default());
    }
}

#[cfg(test)]
#[path = "k8sexp_tests.rs"]
mod tests;
