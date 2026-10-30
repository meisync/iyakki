//! The local k3s cluster as a data source (`kube`). A query is the NAME of a view (`nodes`, `pod_problems`, `warning_events`, ...), never
//! text that reaches kubectl: each view is a fixed, read-only look at the cluster ([`KubeRead`]: `get` and `version`, no Secrets, no logs,
//! no exec) turned into a table by code in this file. Nothing can be applied, deleted, scaled or restarted through it.
//!
//! * The cluster is down, not installed or stopped: the `cluster` view says so in one row and every other view shows a one-row (or empty)
//!   answer that says "the cluster is not running", never an error.
//! * metrics-server (CPU and memory) is optional: when it does not answer, the views that need it say so plainly.
//! * Only the fields named below are read from the JSON. Container commands, environment values and annotations are never copied into a
//!   frame, and the free text that is (messages of events and statuses) is cut short and passed through the credential detector.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde_json::Value;

use super::cluster::{ClusterApi, KubeRead};
use super::connector::*;
use super::frame::{clip, num, parse_time_ms, ColType, Column, Frame};

/// Items looked at per list; a bigger cluster is cut with a notice.
const MAX_ITEMS: usize = 20_000;
const MAX_TABLE_ROWS: usize = 200;
const EVENT_WINDOW_MS: i64 = 3_600_000;
/// A pod that has been deleting this long is stuck.
const STUCK_TERMINATING_MS: i64 = 300_000;

pub fn info() -> KindInfo {
    KindInfo {
        kind: "kube".into(),
        label: "Kubernetes (local k3s)".into(),
        language: Lang::Kube,
        about: "The k3s cluster that runs on this machine: nodes, pods, deployments, events, services, volumes and, when metrics-server answers, CPU and memory. The query is the name of a view. Read-only: Secrets, logs and exec are not reachable, nothing can be changed.".into(),
        fields: vec![Field::new("cluster", "Cluster", FieldKind::Choice(vec!["local".into()]), "the k3s cluster of this machine (more clusters later)").default("local")],
        example: "no address: the cluster of this machine".into(),
    }
}

pub struct Kube {
    id: String,
    api: std::sync::Arc<dyn ClusterApi>,
}

impl Kube {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<Kube, DashError> {
        match cfg.value("cluster") {
            "" | "local" => Ok(Kube { id: cfg.id.clone(), api: d.cluster.clone() }),
            other => Err(DashError::config(format!("Kubernetes “{}”: the cluster `{}` is not known; only `local` (the k3s of this machine) exists", cfg.name, clip(other, 30)))),
        }
    }

    pub fn with_api(id: &str, api: std::sync::Arc<dyn ClusterApi>) -> Kube {
        Kube { id: id.into(), api }
    }
}

// ------------------------------------------------------------------------------------------------------ views

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A table; when empty one row says "none", when the cluster is down one row says so in column `msg`.
    Table { msg: usize },
    /// A chart or a number: when the cluster is down the answer is empty and carries the notice.
    Chart,
    /// Answers for itself, also when the cluster is down.
    Status,
}

type Cols = &'static [(&'static str, ColType)];

struct View {
    name: &'static str,
    about: &'static str,
    shape: Shape,
    cols: Cols,
    rows: fn(&Ctx) -> Result<Vec<Vec<Value>>, DashError>,
}

use ColType::{Float, Int, Str, Time};

const VIEWS: &[View] = &[
    View { name: "cluster", about: "state, k3s version, nodes ready, age, metrics-server", shape: Shape::Status, cols: &[("State", Str), ("Detail", Str), ("Version", Str), ("Nodes ready", Str), ("Cluster age", Str), ("Ready for", Str), ("Metrics (CPU, memory)", Str)], rows: v_cluster },
    View { name: "nodes", about: "each node: ready or not, roles, version, bad conditions", shape: Shape::Table { msg: 1 }, cols: &[("Node", Str), ("State", Str), ("Roles", Str), ("Version", Str), ("Conditions", Str), ("Age", Str)], rows: v_nodes },
    View { name: "node_states", about: "nodes by Ready, NotReady, Unknown", shape: Shape::Chart, cols: &[("State", Str), ("Nodes", Int)], rows: v_node_states },
    View { name: "pods_by_phase", about: "pods by phase", shape: Shape::Chart, cols: &[("Phase", Str), ("Pods", Int)], rows: v_pods_by_phase },
    View { name: "pods_by_namespace", about: "pods per namespace", shape: Shape::Chart, cols: &[("Namespace", Str), ("Pods", Int)], rows: v_pods_by_namespace },
    View { name: "pod_problems", about: "pods in CrashLoopBackOff, ImagePullBackOff, Pending, Failed, not ready ... with the reason", shape: Shape::Table { msg: 3 }, cols: &[("Namespace", Str), ("Pod", Str), ("Problem", Str), ("Reason", Str), ("Restarts", Int), ("Node", Str)], rows: v_pod_problems },
    View { name: "top_restarts", about: "the containers that restarted most", shape: Shape::Table { msg: 1 }, cols: &[("Namespace", Str), ("Pod", Str), ("Container", Str), ("Restarts", Int), ("Last exit", Str)], rows: v_top_restarts },
    View { name: "deployments_unavailable", about: "deployments that are not fully available", shape: Shape::Table { msg: 2 }, cols: &[("Namespace", Str), ("Deployment", Str), ("Problem", Str), ("Desired", Int), ("Ready", Int), ("Available", Int), ("Updated", Int)], rows: v_deployments },
    View { name: "node_usage", about: "CPU and memory per node (metrics-server)", shape: Shape::Table { msg: 1 }, cols: &[("Node", Str), ("Detail", Str), ("CPU (cores)", Float), ("CPU %", Float), ("Memory", Int), ("Memory %", Float)], rows: v_node_usage },
    View { name: "namespace_cpu", about: "CPU per namespace in millicores (metrics-server)", shape: Shape::Chart, cols: &[("Namespace", Str), ("CPU (m)", Int)], rows: v_namespace_cpu },
    View { name: "namespace_memory", about: "memory per namespace (metrics-server)", shape: Shape::Chart, cols: &[("Namespace", Str), ("Memory", Int)], rows: v_namespace_memory },
    View { name: "pod_usage", about: "the 15 pods using most memory, with CPU (metrics-server)", shape: Shape::Table { msg: 1 }, cols: &[("Namespace", Str), ("Pod", Str), ("CPU (m)", Int), ("Memory", Int)], rows: v_pod_usage },
    View { name: "warning_events", about: "warning events of the last hour", shape: Shape::Table { msg: 4 }, cols: &[("Time", Time), ("Namespace", Str), ("Object", Str), ("Reason", Str), ("Message", Str), ("Count", Int)], rows: v_events },
    View { name: "exposed_services", about: "NodePort and LoadBalancer services, documented or not", shape: Shape::Table { msg: 1 }, cols: &[("Namespace", Str), ("Service", Str), ("Type", Str), ("Ports", Str), ("Status", Str)], rows: v_exposed },
    View { name: "pvcs", about: "persistent volume claims: phase, size, usage when the kubelet reports it", shape: Shape::Table { msg: 1 }, cols: &[("Namespace", Str), ("Claim", Str), ("Phase", Str), ("Size", Int), ("Used", Int), ("Used %", Float), ("Class", Str)], rows: v_pvcs },
    View { name: "stat_nodes_not_ready", about: "number of nodes that are not Ready", shape: Shape::Chart, cols: &[("nodes not ready", Int)], rows: s_nodes_not_ready },
    View { name: "stat_pods_with_problems", about: "number of pods in the pod_problems list", shape: Shape::Chart, cols: &[("pods with problems", Int)], rows: s_pod_problems },
    View { name: "stat_deployments_unavailable", about: "number of deployments not fully available", shape: Shape::Chart, cols: &[("deployments unavailable", Int)], rows: s_deployments },
    View { name: "stat_warnings_last_hour", about: "number of warning events in the last hour", shape: Shape::Chart, cols: &[("warnings (1 h)", Int)], rows: s_events },
    View { name: "stat_exposed_undocumented", about: "number of services open on the LAN that are not documented", shape: Shape::Chart, cols: &[("exposed, undocumented", Int)], rows: s_exposed },
    View { name: "stat_pvcs_not_bound", about: "number of volume claims that are not Bound", shape: Shape::Chart, cols: &[("claims not bound", Int)], rows: s_pvcs },
];

/// The names of the views, for the browser and the docs.
pub fn view_names() -> Vec<&'static str> {
    VIEWS.iter().map(|v| v.name).collect()
}

/// Words that start a kubectl write; a query that begins with one is refused as a write, not just as an unknown view.
const WRITE_WORDS: &[&str] = &["apply", "delete", "scale", "rollout", "patch", "create", "replace", "edit", "label", "annotate", "drain", "cordon", "uncordon", "taint", "exec", "cp", "run", "set", "expose", "autoscale", "attach", "port-forward", "proxy", "certificate", "debug", "kubectl", "k3s"];

/// What the person wrote, as a view name. Anything else is refused before the cluster is asked.
pub fn view_name(text: &str) -> Result<&'static str, DashError> {
    let t = text.trim().trim_end_matches(';').trim().to_ascii_lowercase();
    if let Some(v) = VIEWS.iter().find(|v| v.name == t) {
        return Ok(v.name);
    }
    let first = t.split_whitespace().next().unwrap_or("");
    if WRITE_WORDS.contains(&first) {
        return Err(DashError::read_only(format!("`{}` is not a view: this source never runs commands against the cluster", clip(&t, 40))).fix(format!("Write the name of a view: {}", view_names().join(", "))));
    }
    Err(DashError::new(ErrKind::BadQuery, if t.is_empty() { "the query is empty".to_string() } else { format!("`{}` is not a view of the cluster", clip(&t, 40)) }).fix(format!("Write the name of one view: {}", view_names().join(", "))))
}

// ------------------------------------------------------------------------------------------------------ running

struct Ctx<'a> {
    api: &'a dyn ClusterApi,
    now: i64,
    notes: RefCell<Vec<String>>,
}

impl Ctx<'_> {
    fn note(&self, n: impl Into<String>) {
        let n = n.into();
        let mut v = self.notes.borrow_mut();
        if !v.contains(&n) {
            v.push(n);
        }
    }

    fn text(&self, what: &KubeRead) -> Result<String, DashError> {
        let t = self.api.kube_read(what)?;
        if t.len() > MAX_RESPONSE_BYTES {
            return Err(DashError::new(ErrKind::Limit, "the cluster's answer is larger than 8 MB").fix("This view cannot show a cluster that big."));
        }
        Ok(t)
    }

    fn json(&self, what: &KubeRead) -> Result<Value, DashError> {
        let t = self.text(what)?;
        match serde_json::from_str::<Value>(&t) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Err(DashError::parse("kubectl did not answer with JSON (the answer may have been cut because it was too large)")),
        }
    }

    /// The `items` of a list, at most [`MAX_ITEMS`].
    fn items<'v>(&self, v: &'v Value, what: &str) -> &'v [Value] {
        let a = v.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        if a.len() > MAX_ITEMS {
            self.note(format!("only the first {MAX_ITEMS} {what} were looked at"));
        }
        &a[..a.len().min(MAX_ITEMS)]
    }
}

/// The cluster is not there (stopped, not installed, no answer) or an optional part (metrics) is missing: an answer, not a failure.
fn soft(e: &DashError) -> bool {
    e.kind == ErrKind::Missing || e.message.starts_with("the cluster is not available")
}

fn down_text(e: &DashError) -> String {
    let m = e.message.strip_prefix("the cluster is not available: ").unwrap_or(&e.message);
    if e.kind == ErrKind::Missing {
        return format!("{} {}", e.message.trim_end_matches('.'), e.fix.as_deref().unwrap_or("")).trim().to_string();
    }
    format!("The cluster is not running ({})", m.trim_end_matches('.'))
}

fn frame_of(cols: Cols) -> Frame {
    Frame { columns: cols.iter().map(|(n, t)| Column { name: n.to_string(), ty: *t }).collect(), ..Default::default() }
}

fn message_row(cols: Cols, msg_col: usize, first: &str, msg: &str) -> Vec<Value> {
    (0..cols.len()).map(|i| if i == 0 { Value::from(first) } else if i == msg_col { Value::from(msg) } else { Value::Null }).collect()
}

/// Run a view. `now_ms` is the end of the request's range.
pub fn run_view(api: &dyn ClusterApi, text: &str, now_ms: i64) -> Result<Frame, DashError> {
    let name = view_name(text)?;
    let view = VIEWS.iter().find(|v| v.name == name).expect("view_name returns a view");
    let ctx = Ctx { api, now: now_ms, notes: RefCell::new(vec![]) };
    let mut f = frame_of(view.cols);
    match (view.rows)(&ctx) {
        Ok(rows) => {
            f.rows = rows;
            if f.rows.is_empty() {
                if let Shape::Table { msg } = view.shape {
                    f.rows.push(message_row(view.cols, msg, "none", "nothing to report"));
                }
            }
        }
        Err(e) if soft(&e) && view.shape != Shape::Status => {
            let m = down_text(&e);
            match view.shape {
                Shape::Table { msg } => f.rows.push(message_row(view.cols, msg, if e.kind == ErrKind::Missing { "not available" } else { "cluster is not running" }, &m)),
                _ => f.meta.notices.push(m),
            }
        }
        Err(e) => return Err(e),
    }
    f.meta.notices.extend(ctx.notes.into_inner());
    f.cap(MAX_ROWS, MAX_SERIES, MAX_POINTS);
    f.meta.executed = name.to_string();
    Ok(f)
}

impl DataSource for Kube {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "kube"
    }
    fn language(&self) -> Lang {
        Lang::Kube
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        cancel.check()?;
        let v: Value = serde_json::from_str(&self.api.kube_read(&KubeRead::Version)?).map_err(|_| DashError::parse("kubectl did not answer with JSON"))?;
        let ver = v.pointer("/serverVersion/gitVersion").and_then(Value::as_str).unwrap_or("(version unknown)");
        Ok(TestInfo { ok: true, detail: format!("Kubernetes {}", clip(ver, 40)) })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        cancel.check()?;
        Ok(if path.is_empty() { VIEWS.iter().map(|v| Node { name: v.name.into(), kind: NodeKind::Table, detail: v.about.into(), has_children: false }).collect() } else { vec![] })
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        cancel.check()?;
        let mut f = run_view(self.api.as_ref(), &req.text, req.range.to_ms)?;
        cancel.check()?;
        f.cap(req.limit(), MAX_SERIES, MAX_POINTS);
        f.meta.took_ms = took(started);
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        if VIEWS.iter().any(|v| v.name == target) { target.to_string() } else { "cluster".into() }
    }
}

// ------------------------------------------------------------------------------------------------------ small helpers

fn st<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

fn int(v: &Value, ptr: &str) -> i64 {
    v.pointer(ptr).and_then(Value::as_i64).unwrap_or(0)
}

fn s(x: impl Into<String>) -> Value {
    Value::from(x.into())
}

/// Free text from the cluster: the first line, cut, with anything that looks like a credential replaced.
fn clean(text: &str, n: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let line = match crate::credguard::detect(line) {
        Some(f) => f.redacted,
        None => line.to_string(),
    };
    clip(&line, n)
}

/// `250m`, `12345Ki`, `1Gi`, `1500000n`, `2`, `1e3`: a Kubernetes quantity as a number (cores or bytes).
pub fn quantity(q: &str) -> Option<f64> {
    let q = q.trim();
    if q.is_empty() {
        return None;
    }
    let ok = |x: f64| (x.is_finite() && x >= 0.0).then_some(x);
    if let Ok(x) = q.parse::<f64>() {
        return ok(x);
    }
    const BIN: [(&str, f64); 6] = [("Ki", 1024.0), ("Mi", 1_048_576.0), ("Gi", 1_073_741_824.0), ("Ti", 1_099_511_627_776.0), ("Pi", 1_125_899_906_842_624.0), ("Ei", 1_152_921_504_606_846_976.0)];
    const DEC: [(&str, f64); 9] = [("n", 1e-9), ("u", 1e-6), ("m", 1e-3), ("k", 1e3), ("K", 1e3), ("M", 1e6), ("G", 1e9), ("T", 1e12), ("P", 1e15)];
    for (suf, mul) in BIN.iter().chain(DEC.iter()) {
        if let Some(n) = q.strip_suffix(suf) {
            return n.parse::<f64>().ok().and_then(|x| ok(x * mul));
        }
    }
    q.strip_suffix('E').and_then(|n| n.parse::<f64>().ok()).and_then(|x| ok(x * 1e18))
}

/// `45s`, `12m`, `3h 5m`, `4d 3h`.
pub fn age_text(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

fn condition<'a>(v: &'a Value, ptr: &str, ty: &str) -> Option<&'a Value> {
    v.pointer(ptr).and_then(Value::as_array)?.iter().find(|c| st(c, "/type") == ty)
}

// ------------------------------------------------------------------------------------------------------ nodes

struct NodeRow {
    name: String,
    state: &'static str,
    roles: String,
    version: String,
    conditions: String,
    age: String,
    created: Option<i64>,
    ready_since: Option<i64>,
}

fn node_rows(nodes: &Value, ctx: &Ctx) -> Vec<NodeRow> {
    let mut out: Vec<NodeRow> = ctx
        .items(nodes, "nodes")
        .iter()
        .map(|n| {
            let ready = condition(n, "/status/conditions", "Ready");
            let state = match ready.map(|c| st(c, "/status")) {
                Some("True") => "Ready",
                Some("False") => "NotReady",
                _ => "Unknown",
            };
            let mut roles: Vec<String> = n.pointer("/metadata/labels").and_then(Value::as_object).map(|m| m.keys().filter_map(|k| k.strip_prefix("node-role.kubernetes.io/").map(|r| if r.is_empty() { "worker".to_string() } else { r.to_string() })).collect()).unwrap_or_default();
            roles.sort();
            let mut bad: Vec<String> = n.pointer("/status/conditions").and_then(Value::as_array).into_iter().flatten().filter(|c| st(c, "/type") != "Ready" && st(c, "/status") == "True").map(|c| st(c, "/type").to_string()).collect();
            if n.pointer("/spec/unschedulable").and_then(Value::as_bool) == Some(true) {
                bad.push("SchedulingDisabled".into());
            }
            if let (Some(c), "False" | "Unknown") = (ready, ready.map_or("", |c| st(c, "/status"))) {
                let m = clean(st(c, "/message"), 80);
                if !m.is_empty() {
                    bad.push(m);
                }
            }
            let created = parse_time_ms(st(n, "/metadata/creationTimestamp"));
            NodeRow {
                name: clean(st(n, "/metadata/name"), 63),
                state,
                roles: if roles.is_empty() { "-".into() } else { roles.join(",") },
                version: clean(st(n, "/status/nodeInfo/kubeletVersion"), 40),
                conditions: if bad.is_empty() { "ok".into() } else { bad.join(", ") },
                age: created.map_or(String::new(), |t| age_text((ctx.now - t) / 1000)),
                created,
                ready_since: ready.filter(|c| st(c, "/status") == "True").and_then(|c| parse_time_ms(st(c, "/lastTransitionTime"))),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn v_nodes(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let nodes = ctx.json(&KubeRead::Nodes)?;
    Ok(node_rows(&nodes, ctx).into_iter().map(|n| vec![s(n.name), s(n.state), s(n.roles), s(n.version), s(n.conditions), s(n.age)]).collect())
}

fn v_node_states(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let nodes = ctx.json(&KubeRead::Nodes)?;
    let rows = node_rows(&nodes, ctx);
    Ok(["Ready", "NotReady", "Unknown"].iter().map(|k| vec![s(*k), Value::from(rows.iter().filter(|n| n.state == *k).count() as i64)]).collect())
}

fn s_nodes_not_ready(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let nodes = ctx.json(&KubeRead::Nodes)?;
    Ok(vec![vec![Value::from(node_rows(&nodes, ctx).iter().filter(|n| n.state != "Ready").count() as i64)]])
}

fn v_cluster(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let row = |state: &str, detail: String, ver: String, ready: String, age: String, up: String, metrics: String| vec![vec![s(state), s(detail), s(ver), s(ready), s(age), s(up), s(metrics)]];
    let nodes = match ctx.json(&KubeRead::Nodes) {
        Ok(n) => n,
        Err(e) if soft(&e) => return Ok(row("not running", down_text(&e), String::new(), String::new(), String::new(), String::new(), String::new())),
        Err(e) => return Ok(row("not answering", clean(&e.to_string(), 200), String::new(), String::new(), String::new(), String::new(), String::new())),
    };
    let rows = node_rows(&nodes, ctx);
    let ready = rows.iter().filter(|n| n.state == "Ready").count();
    let version = ctx.json(&KubeRead::Version).ok().map(|v| clean(st(&v, "/serverVersion/gitVersion"), 40)).filter(|v| !v.is_empty()).unwrap_or_else(|| "unknown".into());
    let (state, detail) = match (rows.len(), ready) {
        (0, _) => ("no nodes", "the API answers but no node has joined yet".to_string()),
        (n, r) if r == n => ("running", format!("{n} node{} Ready", if n == 1 { "" } else { "s" })),
        (n, 0) => ("not ready", format!("none of {n} nodes is Ready")),
        (n, r) => ("degraded", format!("{} of {n} nodes not Ready", n - r)),
    };
    let age = rows.iter().filter_map(|n| n.created).min().map_or(String::new(), |t| age_text((ctx.now - t) / 1000));
    let up = rows.iter().filter_map(|n| n.ready_since).max().map_or(String::new(), |t| age_text((ctx.now - t) / 1000));
    let metrics = match ctx.text(&KubeRead::NodeMetrics) {
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) => format!("available ({} node{} reporting)", ctx.items(&v, "nodes").len(), if ctx.items(&v, "nodes").len() == 1 { "" } else { "s" }),
            Err(_) => "not available: the metrics API answered with something unreadable".into(),
        },
        Err(e) => format!("not available: {}", clean(&e.message, 120)),
    };
    Ok(row(state, detail, version, format!("{ready}/{}", rows.len()), age, up, metrics))
}

// ------------------------------------------------------------------------------------------------------ pods

fn v_pods_by_phase(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let pods = ctx.json(&KubeRead::Pods)?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for p in ctx.items(&pods, "pods") {
        let ph = st(p, "/status/phase");
        *counts.entry(if ph.is_empty() { "Unknown".into() } else { clean(ph, 20) }).or_default() += 1;
    }
    let mut out = vec![];
    for k in ["Running", "Pending", "Succeeded", "Failed", "Unknown"] {
        if let Some(n) = counts.remove(k) {
            out.push(vec![s(k), Value::from(n)]);
        }
    }
    out.extend(counts.into_iter().map(|(k, n)| vec![s(k), Value::from(n)]));
    Ok(out)
}

fn v_pods_by_namespace(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let pods = ctx.json(&KubeRead::Pods)?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for p in ctx.items(&pods, "pods") {
        *counts.entry(clean(st(p, "/metadata/namespace"), 63)).or_default() += 1;
    }
    let mut v: Vec<(String, i64)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(30);
    Ok(v.into_iter().map(|(k, n)| vec![s(k), Value::from(n)]).collect())
}

/// Container statuses of a pod, init containers first.
fn container_statuses(p: &Value) -> impl Iterator<Item = &Value> {
    ["/status/initContainerStatuses", "/status/containerStatuses"].into_iter().flat_map(move |ptr| p.pointer(ptr).and_then(Value::as_array).into_iter().flatten())
}

struct PodProblem {
    ns: String,
    pod: String,
    problem: String,
    reason: String,
    restarts: i64,
    node: String,
}

fn last_exit(c: &Value) -> String {
    let t = c.pointer("/lastState/terminated");
    match t {
        Some(t) if !st(t, "/reason").is_empty() || t.get("exitCode").is_some() => {
            let r = st(t, "/reason");
            format!("{} (exit {})", if r.is_empty() { "terminated" } else { r }, int(t, "/exitCode"))
        }
        _ => String::new(),
    }
}

fn pod_problem(p: &Value, now: i64) -> Option<PodProblem> {
    let phase = st(p, "/status/phase");
    if phase == "Succeeded" {
        return None;
    }
    let restarts: i64 = container_statuses(p).map(|c| int(c, "/restartCount")).sum();
    let mk = |problem: &str, reason: String| PodProblem { ns: clean(st(p, "/metadata/namespace"), 63), pod: clean(st(p, "/metadata/name"), 80), problem: clean(problem, 40), reason: clean(&reason, 200), restarts, node: clean(st(p, "/spec/nodeName"), 63) };
    // a container that waits for a reason other than starting up
    let waiting: Vec<(&str, String)> = container_statuses(p)
        .filter_map(|c| {
            let r = st(c, "/state/waiting/reason");
            if r.is_empty() || r == "ContainerCreating" || r == "PodInitializing" {
                return None;
            }
            let m = st(c, "/state/waiting/message");
            let le = last_exit(c);
            let detail = match (m.is_empty(), le.is_empty()) {
                (true, true) => String::new(),
                (false, true) => m.to_string(),
                (true, false) => format!("last exit: {le}"),
                (false, false) => format!("{m}; last exit: {le}"),
            };
            Some((r, format!("{}: {detail}", st(c, "/name")).trim_end_matches(": ").to_string()))
        })
        .collect();
    for prio in ["CrashLoopBackOff", "ImagePullBackOff", "ErrImagePull"] {
        if let Some((r, d)) = waiting.iter().find(|w| w.0 == prio) {
            return Some(mk(r, d.clone()));
        }
    }
    if let Some((r, d)) = waiting.first() {
        return Some(mk(r, d.clone()));
    }
    if let Some(c) = container_statuses(p).find(|c| int(c, "/state/terminated/exitCode") != 0 && c.pointer("/state/terminated").is_some() && phase != "Failed") {
        let r = st(c, "/state/terminated/reason");
        return Some(mk(if r.is_empty() { "Error" } else { r }, format!("{}: exit {}", st(c, "/name"), int(c, "/state/terminated/exitCode"))));
    }
    if let Some(t) = parse_time_ms(st(p, "/metadata/deletionTimestamp")) {
        if now - t > STUCK_TERMINATING_MS {
            return Some(mk("Terminating", format!("deleting for {}", age_text((now - t) / 1000))));
        }
    }
    match phase {
        "Pending" => {
            let sched = condition(p, "/status/conditions", "PodScheduled").filter(|c| st(c, "/status") == "False");
            let reason = match sched {
                Some(c) => format!("{}: {}", st(c, "/reason"), st(c, "/message")),
                None if !st(p, "/status/message").is_empty() => st(p, "/status/message").to_string(),
                None => "waiting to start".to_string(),
            };
            Some(mk("Pending", reason))
        }
        "Failed" => Some(mk(if st(p, "/status/reason").is_empty() { "Failed" } else { st(p, "/status/reason") }, st(p, "/status/message").to_string())),
        "Unknown" => Some(mk("Unknown", "the node does not report this pod".into())),
        "Running" => {
            let c = condition(p, "/status/conditions", "Ready")?;
            if st(c, "/status") != "False" {
                return None;
            }
            let bad: Vec<&str> = container_statuses(p).filter(|c| c.get("ready").and_then(Value::as_bool) == Some(false)).map(|c| st(c, "/name")).collect();
            Some(mk("NotReady", if bad.is_empty() { st(c, "/message").to_string() } else { format!("not ready: {}", bad.join(", ")) }))
        }
        _ => None,
    }
}

fn problem_rows(ctx: &Ctx) -> Result<Vec<PodProblem>, DashError> {
    let pods = ctx.json(&KubeRead::Pods)?;
    let mut v: Vec<PodProblem> = ctx.items(&pods, "pods").iter().filter_map(|p| pod_problem(p, ctx.now)).collect();
    v.sort_by(|a, b| b.restarts.cmp(&a.restarts).then_with(|| (&a.ns, &a.pod).cmp(&(&b.ns, &b.pod))));
    Ok(v)
}

fn v_pod_problems(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let v = problem_rows(ctx)?;
    if v.len() > MAX_TABLE_ROWS {
        ctx.note(format!("{} pods have a problem; the {MAX_TABLE_ROWS} with most restarts are shown", v.len()));
    }
    Ok(v.into_iter().take(MAX_TABLE_ROWS).map(|p| vec![s(p.ns), s(p.pod), s(p.problem), s(p.reason), Value::from(p.restarts), s(p.node)]).collect())
}

fn s_pod_problems(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(vec![vec![Value::from(problem_rows(ctx)?.len() as i64)]])
}

fn v_top_restarts(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let pods = ctx.json(&KubeRead::Pods)?;
    let mut v: Vec<(i64, String, String, String, String)> = vec![];
    for p in ctx.items(&pods, "pods") {
        for c in container_statuses(p) {
            let n = int(c, "/restartCount");
            if n > 0 {
                v.push((n, clean(st(p, "/metadata/namespace"), 63), clean(st(p, "/metadata/name"), 80), clean(st(c, "/name"), 63), last_exit(c)));
            }
        }
    }
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| (&a.1, &a.2, &a.3).cmp(&(&b.1, &b.2, &b.3))));
    Ok(v.into_iter().take(10).map(|(n, ns, pod, c, le)| vec![s(ns), s(pod), s(c), Value::from(n), s(le)]).collect())
}

// ------------------------------------------------------------------------------------------------------ deployments

fn deployment_rows(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let d = ctx.json(&KubeRead::Deployments)?;
    let mut out: Vec<Vec<Value>> = vec![];
    for it in ctx.items(&d, "deployments") {
        let desired = it.pointer("/spec/replicas").and_then(Value::as_i64).unwrap_or(1);
        let available = int(it, "/status/availableReplicas");
        let avail_cond = condition(it, "/status/conditions", "Available");
        let cond_false = avail_cond.is_some_and(|c| st(c, "/status") == "False");
        if desired <= 0 || (available >= desired && !cond_false) {
            continue;
        }
        let progress = condition(it, "/status/conditions", "Progressing").filter(|c| st(c, "/status") == "False" || st(c, "/reason") == "ProgressDeadlineExceeded");
        let problem = match (progress, avail_cond.filter(|_| cond_false)) {
            (Some(c), _) => format!("{}: {}", st(c, "/reason"), st(c, "/message")),
            (None, Some(c)) => format!("{}: {}", st(c, "/reason"), st(c, "/message")),
            _ => format!("{} of {desired} available", available),
        };
        out.push(vec![s(clean(st(it, "/metadata/namespace"), 63)), s(clean(st(it, "/metadata/name"), 80)), s(clean(&problem, 200)), Value::from(desired), Value::from(int(it, "/status/readyReplicas")), Value::from(available), Value::from(int(it, "/status/updatedReplicas"))]);
    }
    out.sort_by(|a, b| (a[0].as_str(), a[1].as_str()).cmp(&(b[0].as_str(), b[1].as_str())));
    Ok(out)
}

fn v_deployments(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let mut v = deployment_rows(ctx)?;
    v.truncate(MAX_TABLE_ROWS);
    Ok(v)
}

fn s_deployments(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(vec![vec![Value::from(deployment_rows(ctx)?.len() as i64)]])
}

// ------------------------------------------------------------------------------------------------------ CPU and memory

fn usage_of(m: &Value, key: &str) -> Option<f64> {
    quantity(st(m, &format!("/usage/{key}")))
}

fn pct(used: f64, total: f64) -> Value {
    if total > 0.0 { num((used / total * 1000.0).round() / 10.0) } else { Value::Null }
}

fn v_node_usage(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let nodes = ctx.json(&KubeRead::Nodes)?;
    let metrics = ctx.json(&KubeRead::NodeMetrics)?;
    let mut alloc: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for n in ctx.items(&nodes, "nodes") {
        alloc.insert(st(n, "/metadata/name").to_string(), (quantity(st(n, "/status/allocatable/cpu")).unwrap_or(0.0), quantity(st(n, "/status/allocatable/memory")).unwrap_or(0.0)));
    }
    let mut out = vec![];
    for m in ctx.items(&metrics, "node metrics") {
        let name = st(m, "/metadata/name");
        let (Some(cpu), Some(mem)) = (usage_of(m, "cpu"), usage_of(m, "memory")) else {
            ctx.note("a node reported usage that could not be read and was left out");
            continue;
        };
        let (ac, am) = alloc.get(name).copied().unwrap_or((0.0, 0.0));
        out.push(vec![s(clean(name, 63)), Value::Null, num((cpu * 100.0).round() / 100.0), pct(cpu, ac), num(mem.round()), pct(mem, am)]);
    }
    out.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    Ok(out)
}

/// (namespace, pod, cpu cores, memory bytes) of every pod that reported.
fn pod_metrics(ctx: &Ctx) -> Result<Vec<(String, String, f64, f64)>, DashError> {
    let m = ctx.json(&KubeRead::PodMetrics)?;
    let mut out = vec![];
    for p in ctx.items(&m, "pod metrics") {
        let (mut cpu, mut mem, mut good, mut bad) = (0.0, 0.0, 0, false);
        for c in p.get("containers").and_then(Value::as_array).into_iter().flatten() {
            match (usage_of(c, "cpu"), usage_of(c, "memory")) {
                (Some(a), Some(b)) => {
                    cpu += a;
                    mem += b;
                    good += 1;
                }
                _ => bad = true,
            }
        }
        if bad {
            ctx.note("a container reported usage that could not be read and was left out");
        }
        if good == 0 {
            continue;
        }
        out.push((clean(st(p, "/metadata/namespace"), 63), clean(st(p, "/metadata/name"), 80), cpu, mem));
    }
    Ok(out)
}

fn per_namespace(ctx: &Ctx, cpu: bool) -> Result<Vec<Vec<Value>>, DashError> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for (ns, _, c, m) in pod_metrics(ctx)? {
        *sums.entry(ns).or_default() += if cpu { c * 1000.0 } else { m };
    }
    let mut v: Vec<(String, f64)> = sums.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(30);
    Ok(v.into_iter().map(|(k, x)| vec![s(k), Value::from(x.round() as i64)]).collect())
}

fn v_namespace_cpu(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    per_namespace(ctx, true)
}

fn v_namespace_memory(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    per_namespace(ctx, false)
}

fn v_pod_usage(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let mut v = pod_metrics(ctx)?;
    v.sort_by(|a, b| b.3.total_cmp(&a.3).then_with(|| (&a.0, &a.1).cmp(&(&b.0, &b.1))));
    v.truncate(15);
    Ok(v.into_iter().map(|(ns, pod, c, m)| vec![s(ns), s(pod), Value::from((c * 1000.0).round() as i64), Value::from(m.round() as i64)]).collect())
}

// ------------------------------------------------------------------------------------------------------ events

/// (time, namespace, object, reason, message, count) of the warnings of the last hour, newest first.
fn warning_rows(ctx: &Ctx) -> Result<Vec<(i64, Vec<Value>)>, DashError> {
    let ev = ctx.json(&KubeRead::Events)?;
    let (mut untimed, mut out) = (0, vec![]);
    for e in ctx.items(&ev, "events") {
        if !st(e, "/type").is_empty() && st(e, "/type") != "Warning" {
            continue;
        }
        let t = ["/lastTimestamp", "/series/lastObservedTime", "/eventTime", "/metadata/creationTimestamp"].iter().map(|p| st(e, p)).filter(|x| !x.is_empty()).find_map(parse_time_ms);
        let Some(t) = t else {
            untimed += 1;
            continue;
        };
        if ctx.now - t > EVENT_WINDOW_MS {
            continue;
        }
        let obj = format!("{}/{}", clean(st(e, "/involvedObject/kind"), 30), clean(st(e, "/involvedObject/name"), 80));
        let count = e.get("count").and_then(Value::as_i64).or_else(|| e.pointer("/series/count").and_then(Value::as_i64)).unwrap_or(1);
        let ns = [st(e, "/involvedObject/namespace"), st(e, "/metadata/namespace")].into_iter().find(|x| !x.is_empty()).unwrap_or("");
        let msg = if st(e, "/message").is_empty() { st(e, "/note") } else { st(e, "/message") };
        out.push((t, vec![Value::from(t), s(clean(ns, 63)), s(obj), s(clean(st(e, "/reason"), 40)), s(clean(msg, 200)), Value::from(count)]));
    }
    if untimed > 0 {
        ctx.note(format!("{untimed} warning event{} without a time left out", if untimed == 1 { "" } else { "s" }));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(out)
}

fn v_events(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let v = warning_rows(ctx)?;
    if v.len() > 100 {
        ctx.note(format!("{} warnings in the last hour; the newest 100 are shown", v.len()));
    }
    Ok(v.into_iter().take(100).map(|x| x.1).collect())
}

fn s_events(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(vec![vec![Value::from(warning_rows(ctx)?.len() as i64)]])
}

// ------------------------------------------------------------------------------------------------------ services and claims

fn exposed_rows(ctx: &Ctx) -> Result<(Vec<Vec<Value>>, usize), DashError> {
    let text = ctx.text(&KubeRead::Services)?;
    let listed = serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object()).ok_or_else(|| DashError::parse("kubectl did not answer with JSON"))?;
    // a list without items is an empty list, as in the other views
    let report = if listed.get("items").is_some_and(Value::is_array) { crate::exposure::check(&text).map_err(DashError::parse)? } else { Default::default() };
    let mut rows = vec![];
    let mut line = |e: &crate::exposure::Exposed, status: String| {
        let ports = if e.ports.is_empty() { "-".to_string() } else { e.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ") };
        rows.push(vec![s(clean(&e.namespace, 63)), s(clean(&e.name, 63)), s(e.kind.clone()), s(ports), s(status)]);
    };
    for e in &report.unexpected {
        line(e, "NOT documented: open on the LAN".into());
    }
    for e in &report.expected {
        let why = crate::exposure::DOCUMENTED.iter().find(|d| e.ports.contains(&d.port)).map_or("", |d| d.why);
        line(e, format!("documented: {}", clip(why, 120)));
    }
    Ok((rows, report.unexpected.len()))
}

fn v_exposed(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(exposed_rows(ctx)?.0)
}

fn s_exposed(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(vec![vec![Value::from(exposed_rows(ctx)?.1 as i64)]])
}

/// Used bytes per (namespace, claim) from the kubelets' summaries. A node that does not answer is noted, not fatal.
fn volume_usage(ctx: &Ctx, nodes: &Value) -> BTreeMap<(String, String), f64> {
    let mut out = BTreeMap::new();
    let names: Vec<&str> = ctx.items(nodes, "nodes").iter().map(|n| st(n, "/metadata/name")).filter(|n| !n.is_empty()).take(8).collect();
    let mut failed = 0;
    for n in &names {
        let Ok(v) = ctx.json(&KubeRead::NodeStats((*n).to_string())) else {
            failed += 1;
            continue;
        };
        for pod in v.get("pods").and_then(Value::as_array).into_iter().flatten().take(MAX_ITEMS) {
            for vol in pod.get("volume").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(used), claim, ns) = (vol.get("usedBytes").and_then(Value::as_f64), st(vol, "/pvcRef/name"), st(vol, "/pvcRef/namespace")) {
                    if !claim.is_empty() {
                        out.insert((ns.to_string(), claim.to_string()), used);
                    }
                }
            }
        }
    }
    if failed > 0 {
        ctx.note("the kubelet's volume statistics could not be read, so usage is blank");
    } else if out.is_empty() {
        ctx.note("the kubelet reported no volume usage (volumes backed by a plain host folder, like local-path, usually report none)");
    }
    out
}

fn claim_rows(ctx: &Ctx) -> Result<Vec<(bool, Vec<Value>)>, DashError> {
    let pvcs = ctx.json(&KubeRead::Pvcs)?;
    let items = ctx.items(&pvcs, "claims");
    let usage = if items.is_empty() { BTreeMap::new() } else { ctx.json(&KubeRead::Nodes).map(|n| volume_usage(ctx, &n)).unwrap_or_default() };
    let mut out: Vec<(bool, Vec<Value>)> = items
        .iter()
        .map(|c| {
            let (ns, name) = (st(c, "/metadata/namespace"), st(c, "/metadata/name"));
            let phase = if st(c, "/status/phase").is_empty() { "Unknown" } else { st(c, "/status/phase") };
            let size = quantity(st(c, "/status/capacity/storage")).or_else(|| quantity(st(c, "/spec/resources/requests/storage")));
            let used = usage.get(&(ns.to_string(), name.to_string())).copied();
            (phase != "Bound", vec![s(clean(ns, 63)), s(clean(name, 63)), s(clean(phase, 20)), size.map_or(Value::Null, |x| Value::from(x.round() as i64)), used.map_or(Value::Null, |x| Value::from(x.round() as i64)), match (used, size) { (Some(u), Some(t)) => pct(u, t), _ => Value::Null }, s(clean(st(c, "/spec/storageClassName"), 40))])
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| (a.1[0].as_str(), a.1[1].as_str()).cmp(&(b.1[0].as_str(), b.1[1].as_str()))));
    Ok(out)
}

fn v_pvcs(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    Ok(claim_rows(ctx)?.into_iter().take(MAX_TABLE_ROWS).map(|x| x.1).collect())
}

fn s_pvcs(ctx: &Ctx) -> Result<Vec<Vec<Value>>, DashError> {
    let pvcs = ctx.json(&KubeRead::Pvcs)?;
    Ok(vec![vec![Value::from(ctx.items(&pvcs, "claims").iter().filter(|c| st(c, "/status/phase") != "Bound").count() as i64)]])
}
