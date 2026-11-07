//! Which Quick install service or pod a process belongs to, for the System tile's Processes tab and the `system_processes` and
//! `system_pod_busy` chat tools.
//!
//! * [`parse_cgroup`] (pure): the kernel's control-group text of a process (the text of the process's `cgroup` file, read by
//!   [`crate::sysinfo`], the one reader) to the pod uid and container id of containerd/k3s, in the cgroup v2 systemd style
//!   (`kubepods-burstable-pod<uid with underscores>.slice/cri-containerd-<id>.scope`) and the v1 or cgroupfs style
//!   (`/kubepods/burstable/pod<uid>/<id>`).
//! * [`PodTable`]: the pod uid to namespace, pod, service and requests/limits table, from `kubectl get pods -A -o json` (read only,
//!   through the managed cluster's kubectl with its usual timeouts and size caps). With the cluster off or unreachable it is empty
//!   and every label is absent; nothing is reported as wrong.
//! * [`PodCache`]: the table kept fresh in the background for the tile.
//! * [`rows`], [`groups`], [`near_limit`]: what the tab and the tool show.
//! * [`busy_text`]: the recent log tail (redacted, capped) and `kubectl top` numbers of one pod. Nothing is guessed.

use crate::dash::cluster::Kubectl;
use crate::sysinfo::Proc;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A process at or above this share of its limit is "near its limit".
pub const NEAR_PCT: f64 = 90.0;
/// Most rows, and most characters, a tool answer carries.
pub const MAX_ROWS: usize = 50;
pub const MAX_OUT: usize = 8000;
const LOG_LINES: usize = 40;
const LOG_BYTES: usize = 6000;
const REFRESH_SECS: u64 = 15;

// ------------------------------------------------------------------------------------------------------- cgroup text

/// A pod and container as the control-group text names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgRef {
    /// The pod uid in its usual form with dashes.
    pub pod_uid: String,
    /// The container id (hex), `None` for a process of the pod's own slice (the sandbox).
    pub container_id: Option<String>,
}

fn uid_ok(u: &str) -> bool {
    let b = u.as_bytes();
    b.len() == 36 && [8, 13, 18, 23].iter().all(|i| b[*i] == b'-') && b.iter().enumerate().all(|(i, c)| [8, 13, 18, 23].contains(&i) || c.is_ascii_hexdigit())
}

/// The pod uid and container id in one process's control-group text (any number of lines), or `None` when it is no pod's process.
pub fn parse_cgroup(text: &str) -> Option<CgRef> {
    for line in text.lines() {
        // `hierarchy-id:controllers:path`; the path may itself hold colons only in odd cases, so split twice
        let path = line.splitn(3, ':').nth(2).unwrap_or(line);
        if !path.contains("kubepods") {
            continue;
        }
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some(at) = segs.iter().position(|s| pod_uid_of(s).is_some()) else { continue };
        let pod_uid = pod_uid_of(segs[at]).unwrap_or_default();
        let container_id = segs.get(at + 1).and_then(|s| container_id_of(s));
        return Some(CgRef { pod_uid, container_id });
    }
    None
}

fn pod_uid_of(seg: &str) -> Option<String> {
    let s = seg.strip_suffix(".slice").unwrap_or(seg);
    let rest = s.rsplit_once("-pod").map(|(_, r)| r).or_else(|| s.strip_prefix("pod"))?;
    let u = rest.replace('_', "-");
    uid_ok(&u).then_some(u)
}

fn container_id_of(seg: &str) -> Option<String> {
    let s = seg.strip_suffix(".scope").unwrap_or(seg);
    let id = match s.rsplit_once('-') {
        Some((pre, id)) if matches!(pre, "cri-containerd" | "crio" | "docker" | "containerd") => id,
        Some(_) => return None,
        None => s,
    };
    (id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_ascii_lowercase())
}

// ------------------------------------------------------------------------------------------------------ the pod table

/// Requests and limits: processor in millicores, memory in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Res {
    pub cpu_req: Option<u64>,
    pub cpu_lim: Option<u64>,
    pub mem_req: Option<u64>,
    pub mem_lim: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerEntry {
    pub name: String,
    pub id: String,
    pub res: Res,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodEntry {
    pub ns: String,
    pub pod: String,
    pub service: String,
    pub containers: Vec<ContainerEntry>,
}

/// What a process is labelled with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub ns: String,
    pub pod: String,
    pub service: String,
    pub container: String,
    pub res: Res,
}

impl Label {
    /// `clickhouse (iyakki-data)`.
    pub fn text(&self) -> String {
        format!("{} ({})", self.service, self.ns)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PodTable {
    by_uid: HashMap<String, PodEntry>,
}

/// `500m` is 500, `2` is 2000.
pub fn parse_millicores(q: &str) -> Option<u64> {
    let q = q.trim();
    match q.strip_suffix('m') {
        Some(n) => n.parse::<u64>().ok(),
        None => q.parse::<f64>().ok().filter(|v| *v >= 0.0).map(|v| (v * 1000.0).round() as u64),
    }
}

/// `512Mi`, `1Gi`, `1G`, `128974848`, `129e6`.
pub fn parse_bytes(q: &str) -> Option<u64> {
    let q = q.trim();
    let split = q.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(q.len());
    let (n, unit) = q.split_at(split);
    let n: f64 = n.parse().ok()?;
    let mul: f64 = match unit {
        "" => 1.0,
        "Ki" => 1024.0,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        "k" | "K" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        _ if unit.starts_with(['e', 'E']) => 10f64.powi(unit[1..].parse().ok()?),
        _ => return None,
    };
    Some((n * mul) as u64)
}

fn res_of(c: &Value) -> Res {
    let g = |k: &str, f: &str| c.pointer(&format!("/resources/{k}/{f}")).and_then(Value::as_str);
    Res { cpu_req: g("requests", "cpu").and_then(parse_millicores), cpu_lim: g("limits", "cpu").and_then(parse_millicores), mem_req: g("requests", "memory").and_then(parse_bytes), mem_lim: g("limits", "memory").and_then(parse_bytes) }
}

/// A deployment's pod `gitea-6d9f8b7c5-x2k4q` is owned by the ReplicaSet `gitea-6d9f8b7c5`: the service is what is before the hash.
fn strip_hash(name: &str) -> &str {
    match name.rsplit_once('-') {
        Some((a, h)) if (5..=10).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_alphanumeric()) && h.bytes().any(|b| b.is_ascii_digit()) => a,
        _ => name,
    }
}

fn service_of(p: &Value) -> String {
    let s = |ptr: &str| p.pointer(ptr).and_then(Value::as_str).filter(|x| !x.is_empty());
    if let Some(n) = s("/metadata/labels/app.kubernetes.io~1name").or_else(|| s("/metadata/labels/app")) {
        return n.to_string();
    }
    let name = s("/metadata/name").unwrap_or("");
    let owner = p.pointer("/metadata/ownerReferences/0");
    match owner.and_then(|o| Some((o.get("kind")?.as_str()?, o.get("name")?.as_str()?))) {
        Some(("ReplicaSet", n)) => strip_hash(n).to_string(),
        Some((_, n)) => n.to_string(),
        None => name.to_string(),
    }
}

impl PodTable {
    pub fn empty() -> PodTable {
        PodTable::default()
    }

    pub fn len(&self) -> usize {
        self.by_uid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_uid.is_empty()
    }

    /// From the JSON of `kubectl get pods -A -o json`. Text that is no pod list gives an empty table.
    pub fn parse(json: &str) -> PodTable {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return PodTable::empty() };
        let mut by_uid = HashMap::new();
        for p in v.get("items").and_then(Value::as_array).into_iter().flatten().take(5000) {
            let s = |ptr: &str| p.pointer(ptr).and_then(Value::as_str).unwrap_or("").to_string();
            let uid = s("/metadata/uid").to_ascii_lowercase();
            if !uid_ok(&uid) {
                continue;
            }
            let ids: HashMap<String, String> = p.pointer("/status/containerStatuses").and_then(Value::as_array).into_iter().flatten().filter_map(|c| {
                let id = c.get("containerID")?.as_str()?.rsplit('/').next()?.to_ascii_lowercase();
                Some((c.get("name")?.as_str()?.to_string(), id))
            }).collect();
            let containers = p.pointer("/spec/containers").and_then(Value::as_array).into_iter().flatten().map(|c| {
                let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                ContainerEntry { id: ids.get(&name).cloned().unwrap_or_default(), name, res: res_of(c) }
            }).collect();
            by_uid.insert(uid, PodEntry { ns: s("/metadata/namespace"), pod: s("/metadata/name"), service: service_of(p), containers });
        }
        PodTable { by_uid }
    }

    /// The label of a process from its control-group text; `None` for a host process, an unknown pod or no table.
    pub fn label(&self, cgroup: &str) -> Option<Label> {
        let r = parse_cgroup(cgroup)?;
        let e = self.by_uid.get(&r.pod_uid)?;
        let c = r.container_id.as_ref().and_then(|id| e.containers.iter().find(|c| !c.id.is_empty() && (c.id == *id || c.id.starts_with(id.as_str()) || id.starts_with(c.id.as_str()))));
        let (container, res) = match c {
            Some(c) => (c.name.clone(), c.res),
            None => (String::new(), sum_res(&e.containers)),
        };
        Some(Label { ns: e.ns.clone(), pod: e.pod.clone(), service: e.service.clone(), container, res })
    }

    /// The pod named (and its namespace, when given).
    pub fn find(&self, ns: &str, pod: &str) -> Option<&PodEntry> {
        self.by_uid.values().find(|e| e.pod == pod && (ns.is_empty() || e.ns == ns))
    }
}

/// A whole pod's figures: the sum, and only where every container has one.
fn sum_res(cs: &[ContainerEntry]) -> Res {
    let sum = |f: fn(&Res) -> Option<u64>| -> Option<u64> { if cs.is_empty() { None } else { cs.iter().map(|c| f(&c.res)).sum() } };
    Res { cpu_req: sum(|r| r.cpu_req), cpu_lim: sum(|r| r.cpu_lim), mem_req: sum(|r| r.mem_req), mem_lim: sum(|r| r.mem_lim) }
}

/// Read the table through kubectl; any failure is an empty table.
pub fn fetch_table(kube: &dyn Kubectl) -> PodTable {
    kube.kubectl_big(&["get", "pods", "-A", "-o", "json"]).map(|t| PodTable::parse(&t)).unwrap_or_default()
}

// --------------------------------------------------------------------------------------------------- the background cache

#[derive(Default)]
struct CacheInner {
    table: Arc<PodTable>,
    at: Option<u64>,
    busy: bool,
    /// The last "what is it busy with" answer: (pod, text).
    answer: Option<(String, String)>,
    asking: bool,
}

/// The table for the tile: refreshed in the background at most every 15 seconds while the cluster runs, dropped when it stops.
#[derive(Clone, Default)]
pub struct PodCache(Arc<Mutex<CacheInner>>);

impl PodCache {
    pub fn new() -> PodCache {
        PodCache::default()
    }

    pub fn table(&self) -> Arc<PodTable> {
        self.0.lock().unwrap().table.clone()
    }

    /// Called on every refresh of the tab. `running`: the cluster is up. Does the reading on another thread.
    pub fn poll(&self, kube: Arc<dyn Kubectl>, running: bool, now: u64) {
        let go = self.plan(running, now);
        if go {
            let me = self.clone();
            std::thread::spawn(move || me.refresh_now(&*kube, now));
        }
    }

    /// Decide (and mark) whether to read now; a stopped cluster empties the table.
    fn plan(&self, running: bool, now: u64) -> bool {
        let mut g = self.0.lock().unwrap();
        if !running {
            g.table = Arc::new(PodTable::empty());
            g.at = None;
            return false;
        }
        if g.busy || g.at.is_some_and(|t| now < t + REFRESH_SECS) {
            return false;
        }
        g.busy = true;
        true
    }

    /// Read now, on this thread (the tests, and the thread of [`PodCache::poll`]).
    pub fn refresh_now(&self, kube: &dyn Kubectl, now: u64) {
        let t = fetch_table(kube);
        let mut g = self.0.lock().unwrap();
        g.table = Arc::new(t);
        g.at = Some(now);
        g.busy = false;
    }

    /// Start "what is this pod busy with" on another thread; [`PodCache::answer`] gets the text.
    pub fn ask_busy(&self, kube: Arc<dyn Kubectl>, ns: String, pod: String) {
        {
            let mut g = self.0.lock().unwrap();
            if g.asking {
                return;
            }
            g.asking = true;
            g.answer = None;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let text = busy_text(&*kube, &ns, &pod);
            let mut g = me.0.lock().unwrap();
            g.answer = Some((pod, text));
            g.asking = false;
        });
    }

    pub fn asking(&self) -> bool {
        self.0.lock().unwrap().asking
    }

    /// Take the finished answer, if any.
    pub fn take_answer(&self) -> Option<(String, String)> {
        self.0.lock().unwrap().answer.take()
    }
}

// ------------------------------------------------------------------------------------------------------ rows and groups

/// One process with its label.
#[derive(Clone, Debug)]
pub struct Row {
    pub pid: u32,
    pub user: String,
    pub name: String,
    pub cmd: String,
    pub cpu: f32,
    pub rss: u64,
    pub label: Option<Label>,
    pub near: bool,
}

/// Processor use (percent of one core) or memory against the limits of the container: at 90% or more of either.
pub fn near_limit(rss: u64, cpu_pct: f32, res: &Res) -> bool {
    let mem = res.mem_lim.filter(|l| *l > 0).is_some_and(|l| rss as f64 * 100.0 / l as f64 >= NEAR_PCT);
    let cpu = res.cpu_lim.filter(|l| *l > 0).is_some_and(|l| cpu_pct as f64 * 10.0 * 100.0 / l as f64 >= NEAR_PCT);
    mem || cpu
}

/// Every process with its label (from `table`) and its near-limit marker.
pub fn rows(procs: &[Proc], cpu: &HashMap<u32, f32>, table: &PodTable) -> Vec<Row> {
    procs.iter().map(|p| {
        let c = cpu.get(&p.pid).copied().unwrap_or(0.0);
        let label = if p.cgroup.is_empty() { None } else { table.label(&p.cgroup) };
        let near = label.as_ref().is_some_and(|l| near_limit(p.rss, c, &l.res));
        Row { pid: p.pid, user: p.user.clone(), name: p.name.clone(), cmd: p.cmd.clone(), cpu: c, rss: p.rss, label, near }
    }).collect()
}

/// The processes of one service (or of the host) together.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// `clickhouse (iyakki-data)`, or `host` for processes with no pod.
    pub name: String,
    pub count: usize,
    pub cpu: f32,
    pub rss: u64,
    pub res: Option<Res>,
    pub near: bool,
    pub pod: Option<(String, String)>,
}

/// Group the rows by service (namespace and service), biggest memory first.
pub fn groups(rows: &[Row]) -> Vec<Group> {
    let mut m: Vec<Group> = Vec::new();
    for r in rows {
        let name = r.label.as_ref().map_or_else(|| "host".to_string(), Label::text);
        let i = match m.iter().position(|g| g.name == name) {
            Some(i) => i,
            None => {
                m.push(Group { name, count: 0, cpu: 0.0, rss: 0, res: r.label.as_ref().map(|l| l.res), near: false, pod: r.label.as_ref().map(|l| (l.ns.clone(), l.pod.clone())) });
                m.len() - 1
            }
        };
        let g = &mut m[i];
        g.count += 1;
        g.cpu += r.cpu;
        g.rss += r.rss;
        g.near |= r.near;
    }
    m.sort_by(|a, b| b.rss.cmp(&a.rss).then(a.name.cmp(&b.name)));
    m
}

/// `req 250m / lim 1000m cpu, req 256 MB / lim 1.0 GB mem`, or empty when the pod sets none.
pub fn res_text(r: &Res) -> String {
    let cpu = |v: Option<u64>| v.map_or("-".to_string(), |m| if m % 1000 == 0 { format!("{}", m / 1000) } else { format!("{m}m") });
    let mem = |v: Option<u64>| v.map_or("-".to_string(), crate::sysinfo::human_bytes);
    if *r == Res::default() {
        return String::new();
    }
    format!("cpu {}/{}  mem {}/{} (request/limit)", cpu(r.cpu_req), cpu(r.cpu_lim), mem(r.mem_req), mem(r.mem_lim))
}

// ------------------------------------------------------------------------------------------------------------ redaction

/// A command line or log line with credentials hidden: the shared detector first, then `name=value` and `--name value` pairs whose
/// name says it is a secret.
pub fn redact(line: &str) -> String {
    let mut s = line.to_string();
    for _ in 0..6 {
        match crate::credguard::detect(&s) {
            Some(f) if f.redacted != s => s = f.redacted,
            _ => break,
        }
    }
    const WORDS: [&str; 7] = ["password", "passwd", "secret", "token", "apikey", "api-key", "api_key"];
    let toks: Vec<String> = s.split(' ').map(String::from).collect();
    let mut out = Vec::with_capacity(toks.len());
    let mut hide_next = false;
    for t in toks {
        let low = t.to_ascii_lowercase();
        if hide_next {
            hide_next = false;
            out.push("[hidden]".to_string());
            continue;
        }
        if let Some(i) = t.find('=') {
            if WORDS.iter().any(|w| low[..i].contains(w)) {
                out.push(format!("{}=[hidden]", &t[..i]));
                continue;
            }
        } else if low.starts_with("--") && WORDS.iter().any(|w| low.contains(w)) {
            hide_next = true;
        }
        out.push(t);
    }
    out.join(" ")
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

// -------------------------------------------------------------------------------------------------------------- the tools

/// `system_processes`: the top `limit` processes by processor or memory with the service label.
pub fn top_text(rows: &[Row], by_memory: bool, limit: usize, group: bool, cluster_known: bool) -> String {
    let limit = limit.clamp(1, MAX_ROWS);
    let mut out = String::new();
    if group {
        let mut g = groups(rows);
        if !by_memory {
            g.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
        }
        out.push_str(&format!("Top {} of {} groups by {} (service, processes, cpu % of one core, memory):\n", limit.min(g.len()), g.len(), if by_memory { "memory" } else { "processor" }));
        for x in g.iter().take(limit) {
            let r = x.res.map(|r| res_text(&r)).filter(|t| !t.is_empty()).map_or(String::new(), |t| format!("; {t}"));
            out.push_str(&format!("- {}: {} process{}, {:.0}%, {}{}{}\n", x.name, x.count, if x.count == 1 { "" } else { "es" }, x.cpu, crate::sysinfo::human_bytes(x.rss), r, if x.near { "; NEAR ITS LIMIT" } else { "" }));
        }
    } else {
        let mut v: Vec<&Row> = rows.iter().collect();
        if by_memory {
            v.sort_by(|a, b| b.rss.cmp(&a.rss));
        } else {
            v.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal).then(b.rss.cmp(&a.rss)));
        }
        out.push_str(&format!("Top {} of {} processes by {} (pid, user, cpu % of one core, memory, service, command):\n", limit.min(v.len()), v.len(), if by_memory { "memory" } else { "processor" }));
        for r in v.iter().take(limit) {
            let svc = r.label.as_ref().map_or_else(|| "host".to_string(), |l| format!("{} pod {}", l.text(), l.pod));
            let cmd = if r.cmd.is_empty() { format!("[{}]", r.name) } else { clip(&redact(&r.cmd), 120) };
            let lim = r.label.as_ref().map(|l| res_text(&l.res)).filter(|t| !t.is_empty()).map_or(String::new(), |t| format!(" [{t}]"));
            out.push_str(&format!("- {} {} {:.0}% {} {}{}{} {}\n", r.pid, r.user, r.cpu, crate::sysinfo::human_bytes(r.rss), svc, lim, if r.near { " NEAR ITS LIMIT" } else { "" }, cmd));
        }
    }
    if !cluster_known {
        out.push_str("(No service names: the cluster is not running or not reachable.)\n");
    }
    clip(&out, MAX_OUT)
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// The recent log tail (redacted, capped) and `kubectl top` of one pod. Read only; no guess at the cause.
pub fn busy_text(kube: &dyn Kubectl, ns: &str, pod: &str) -> String {
    if !name_ok(ns) || !name_ok(pod) {
        return "That is not a namespace and pod name.".into();
    }
    let mut out = format!("Pod {pod} in {ns}\n");
    match kube.kubectl(&["-n", ns, "top", "pod", pod, "--containers", "--no-headers"]) {
        Ok(t) if !t.trim().is_empty() => {
            out.push_str("kubectl top (pod, container, cpu, memory):\n");
            for l in t.lines().take(10) {
                out.push_str(&format!("  {}\n", clip(&l.split_whitespace().collect::<Vec<_>>().join("  "), 160)));
            }
        }
        Ok(_) => out.push_str("kubectl top: no numbers yet.\n"),
        Err(e) => out.push_str(&format!("kubectl top: {}\n", clip(e.lines().next().unwrap_or("not available"), 160))),
    }
    let tail = format!("--tail={LOG_LINES}");
    let limit = format!("--limit-bytes={LOG_BYTES}");
    match kube.kubectl(&["-n", ns, "logs", pod, "--all-containers=true", "--prefix=true", &tail, &limit]) {
        Ok(t) if !t.trim().is_empty() => {
            out.push_str(&format!("Last log lines (up to {LOG_LINES}, secrets hidden):\n"));
            for l in t.lines().rev().take(LOG_LINES).collect::<Vec<_>>().into_iter().rev() {
                out.push_str(&format!("  {}\n", clip(&redact(l), 240)));
            }
        }
        Ok(_) => out.push_str("The pod's log is empty.\n"),
        Err(e) => out.push_str(&format!("Log: {}\n", clip(e.lines().next().unwrap_or("not available"), 160))),
    }
    clip(&out, MAX_OUT)
}

/// The text a person or model gets for `system_pod_busy`.
pub fn busy_for(kube: &dyn Kubectl, table: &PodTable, ns: &str, pod: &str) -> Result<String, String> {
    if table.is_empty() {
        return Err("the cluster is not running or not reachable, so there is no pod to look at".into());
    }
    let e = table.find(ns, pod).ok_or_else(|| format!("no pod named {pod} in the cluster"))?;
    Ok(busy_text(kube, &e.ns, &e.pod))
}

// ------------------------------------------------------------------------------------------------------------ chat wiring

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;
use iyakki_sec::policy::{CliLevel, Policy};
use serde_json::json;

pub const TOOLS: [&str; 2] = ["system_processes", "system_pod_busy"];

pub fn is_tool(name: &str) -> bool {
    TOOLS.contains(&name)
}

fn allowed(p: &Policy) -> bool {
    p.cli != CliLevel::Off && p.manage_settings
}

/// Both tools only read, so neither asks. Administrators only, like the Processes tab.
pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !allowed(p) {
        return Vec::new();
    }
    let t = |name: &str, description: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: description.into(), schema }, risk: Risk::Safe, untrusted: true };
    vec![
        t(
            "system_processes",
            "The busiest processes on this machine, each with the Quick install service or pod it belongs to (for example clickhouse in iyakki-data) and the pod's memory and processor requests and limits, with a NEAR ITS LIMIT marker. Read only. `sort`: cpu (default) or memory. `limit`: how many (default 15, at most 50). `group`: true adds up the processes of each service. Command lines are shown with secrets hidden.",
            json!({"type": "object", "properties": {"sort": {"type": "string", "enum": ["cpu", "memory"]}, "limit": {"type": "integer"}, "group": {"type": "boolean"}}, "required": []}),
        ),
        t(
            "system_pod_busy",
            "What a pod is busy with: its last log lines (secrets hidden, capped) and `kubectl top` numbers. Use it for a pod at high processor use (take the pod name from system_processes). Read only; it shows facts and does not guess a cause.",
            json!({"type": "object", "properties": {"pod": {"type": "string"}, "namespace": {"type": "string"}}, "required": ["pod"]}),
        ),
    ]
}

pub fn describe(name: &str, args: &Value) -> String {
    match name {
        "system_pod_busy" => format!("look at the log and usage of pod {}", args.get("pod").and_then(Value::as_str).unwrap_or("?")),
        _ => "list the busiest processes".into(),
    }
}

fn running(svc: &Services) -> bool {
    svc.k3sdev.state() == crate::k3sdev::Svc::Running
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy) -> Result<String, String> {
    if !allowed(p) {
        return Err("only administrators may look at the processes".into());
    }
    let kube: Arc<dyn Kubectl> = svc.k3sdev.clone();
    call_with(name, args, &crate::sysinfo::SysRoots::real(), &kube, running(svc))
}

/// [`call`] with the machine and the cluster given (the tests give fakes).
pub fn call_with(name: &str, args: &Value, roots: &crate::sysinfo::SysRoots, kube: &Arc<dyn Kubectl>, cluster_up: bool) -> Result<String, String> {
    let table = if cluster_up { fetch_table(&**kube) } else { PodTable::empty() };
    match name {
        "system_processes" => {
            let by_memory = args.get("sort").and_then(Value::as_str) == Some("memory");
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(15) as usize;
            let group = args.get("group").and_then(Value::as_bool).unwrap_or(false);
            let before = crate::sysinfo::processes(roots);
            std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 5 } else { 700 }));
            let after = crate::sysinfo::processes(roots);
            let cpu = crate::sysinfo::proc_cpu(&before, &after, if cfg!(test) { 1.0 } else { 0.7 });
            Ok(top_text(&rows(&after, &cpu, &table), by_memory, limit, group, !table.is_empty()))
        }
        "system_pod_busy" => {
            let pod = args.get("pod").and_then(Value::as_str).unwrap_or("").trim();
            let ns = args.get("namespace").and_then(Value::as_str).unwrap_or("").trim();
            busy_for(&**kube, &table, ns, pod)
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

#[cfg(test)]
#[path = "sysprocs_tests.rs"]
mod tests;
