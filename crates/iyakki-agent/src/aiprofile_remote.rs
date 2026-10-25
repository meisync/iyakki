//! The second source of the AI hardware profile (docs/AI-HARDWARE.md): the GPUs of the REMOTE cluster IYAKKI works against.
//!
//! A machine with no GPU can still run model servers when the cluster it deploys to has one. This module reads the cluster's nodes
//! with the existing read-only runner (`kubectl get nodes -o json`, [`crate::k8s::Kube`]) and turns `status.allocatable` and
//! `status.capacity` (`nvidia.com/gpu`, `amd.com/gpu`, `gpu.intel.com/i915`) plus the labels GPU Feature Discovery and the AMD labeller
//! write (`nvidia.com/gpu.product`, `nvidia.com/gpu.memory` in MiB, `amd.com/gpu.vram`) into a [`RemoteProfile`].
//!
//! * **Never blocks drawing.** [`current`] returns what is cached and, when a refresh is due, starts one on a thread.
//! * **Back-off.** A refresh is due [`REFRESH_SECS`] after the last good answer; after a failure the wait doubles from
//!   [`RETRY_SECS`] up to [`RETRY_MAX_SECS`]. A cluster that stops answering keeps its last value, marked stale.
//! * **A GPU with unknown memory** is "GPU found, memory unknown": it shows the tools and never hides them.
//! * Only the cluster the person chose and that answers is read (`devcluster` registers it), and never one on this machine.

use crate::gpu::Vendor;
use crate::k8s::{self, Kube, Target};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Memory kept free on a cluster GPU for the CUDA/ROCm context and other pods (GB).
pub const REMOTE_RESERVE_GB: f64 = 1.0;
pub const REFRESH_SECS: u64 = 60;
pub const RETRY_SECS: u64 = 30;
pub const RETRY_MAX_SECS: u64 = 600;

/// One kind of GPU in the cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteGpu {
    pub vendor: Vendor,
    /// "NVIDIA RTX 4090" (from the label, dashes made spaces) or "NVIDIA GPU" when the node has no product label.
    pub name: String,
    /// How many of them in the whole cluster.
    pub count: u32,
    /// Memory of one, GB (None: no label says).
    pub mem_gb: Option<f64>,
    /// A node that has them.
    pub node: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteProfile {
    /// The cluster's name as the person sees it.
    pub cluster: String,
    pub gpus: Vec<RemoteGpu>,
    /// Seconds since the epoch of the answer this is from.
    pub fetched_at: u64,
    /// The last refresh failed: this is the last value known.
    pub stale: bool,
    pub error: String,
}

impl RemoteProfile {
    pub fn empty(cluster: &str, now: u64) -> RemoteProfile {
        RemoteProfile { cluster: cluster.into(), gpus: vec![], fetched_at: now, stale: false, error: String::new() }
    }
    /// The GPU with the most memory (unknown memory counts as the smallest).
    pub fn best(&self) -> Option<&RemoteGpu> {
        self.gpus.iter().max_by(|a, b| a.mem_gb.unwrap_or(-1.0).partial_cmp(&b.mem_gb.unwrap_or(-1.0)).unwrap_or(std::cmp::Ordering::Equal))
    }
    /// Memory one pod can use on the best GPU (0 when no GPU or unknown).
    pub fn usable_gb(&self) -> f64 {
        self.best().and_then(|g| g.mem_gb).map_or(0.0, |m| (m - REMOTE_RESERVE_GB).max(0.0))
    }
    /// A GPU is there and big enough.
    pub fn capable(&self) -> bool {
        self.usable_gb() >= crate::aiprofile::MIN_LLM_GB
    }
    /// A GPU is there and no label says how big.
    pub fn unknown(&self) -> bool {
        !self.gpus.is_empty() && self.best().is_some_and(|g| g.mem_gb.is_none())
    }
    /// "2 x NVIDIA RTX 4090 (24 GB each)", "GPU found, memory unknown", or "no GPU".
    pub fn words(&self) -> String {
        match self.best() {
            None => "no GPU".into(),
            Some(g) => {
                let total: u32 = self.gpus.iter().filter(|x| x.name == g.name && x.mem_gb == g.mem_gb).map(|x| x.count).sum();
                match g.mem_gb {
                    Some(m) => format!("{total} x {} ({} GB{})", g.name, fmt_gb(m), if total > 1 { " each" } else { "" }),
                    None => format!("{total} x {} (GPU found, memory unknown)", g.name),
                }
            }
        }
    }
}

fn fmt_gb(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{}", v.round() as u64) } else { format!("{v:.1}") }
}

/// "24G", "16Gi", "8192M", "8192Mi", "24564" (MiB, as NVIDIA's label) to GB.
pub fn parse_mem_label(s: &str) -> Option<f64> {
    let t = s.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let n: f64 = t[..split].parse().ok()?;
    let v = match t[split..].trim().to_ascii_lowercase().as_str() {
        "g" | "gi" | "gb" | "gib" => n,
        "m" | "mi" | "mb" | "mib" | "" => n / 1024.0,
        "t" | "ti" => n * 1024.0,
        _ => return None,
    };
    (v.is_finite() && v > 0.0).then_some(v)
}

fn count_of(v: &Value, key: &str) -> u32 {
    for part in ["allocatable", "capacity"] {
        if let Some(n) = v["status"][part][key].as_str().and_then(|s| s.trim().parse::<u32>().ok()).or_else(|| v["status"][part][key].as_u64().map(|n| n.min(4096) as u32)) {
            return n.min(4096);
        }
    }
    0
}

/// The GPUs in the JSON of `kubectl get nodes -o json`. Garbage gives no GPUs.
pub fn parse_nodes(json: &str) -> Vec<RemoteGpu> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return vec![] };
    let mut out: Vec<RemoteGpu> = Vec::new();
    for n in v["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let name = n["metadata"]["name"].as_str().unwrap_or("").to_string();
        let lab = |k: &str| n["metadata"]["labels"][k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let mut add = |vendor: Vendor, count: u32, product: Option<String>, mem: Option<f64>| {
            if count == 0 {
                return;
            }
            let nm = match (&product, vendor) {
                (Some(p), v) => {
                    let p = p.replace('-', " ");
                    if p.to_ascii_lowercase().starts_with(&v.name().to_ascii_lowercase()) { p } else { format!("{} {p}", v.name()) }
                }
                (None, v) => format!("{} GPU", v.name()),
            };
            match out.iter_mut().find(|g| g.name == nm && g.mem_gb == mem) {
                Some(g) => g.count += count,
                None => out.push(RemoteGpu { vendor, name: nm, count, mem_gb: mem, node: name.clone() }),
            }
        };
        let nv = count_of(n, "nvidia.com/gpu");
        add(Vendor::Nvidia, nv, lab("nvidia.com/gpu.product"), lab("nvidia.com/gpu.memory").and_then(|m| parse_mem_label(&m)));
        let amd = count_of(n, "amd.com/gpu");
        add(Vendor::Amd, amd, lab("amd.com/gpu.product-name").or_else(|| lab("amd.com/gpu.family")), lab("amd.com/gpu.vram").and_then(|m| parse_mem_label(&m)));
        let intel = count_of(n, "gpu.intel.com/i915") + count_of(n, "gpu.intel.com/xe");
        add(Vendor::Intel, intel, lab("gpu.intel.com/product"), lab("gpu.intel.com/memory.max").and_then(|m| m.parse::<f64>().ok()).map(|b| b / (1u64 << 30) as f64).filter(|g| *g > 0.0));
    }
    out
}

/// Read the cluster's nodes (read-only) into a profile. `Err`: kubectl failed or printed something unreadable.
pub fn probe(kube: &dyn Kube, target: &Target, cluster: &str, now: u64) -> Result<RemoteProfile, String> {
    let mut a = target.base_args();
    a.extend(["get", "nodes", "-o", "json"].map(String::from));
    let out = kube.run(&a);
    if !out.ok() {
        let why = out.stderr.lines().next().unwrap_or("kubectl failed").trim().to_string();
        return Err(if out.missing { "kubectl is not installed".into() } else { why });
    }
    if serde_json::from_str::<Value>(&out.stdout).is_err() {
        return Err("kubectl printed something unreadable".into());
    }
    let mut p = RemoteProfile::empty(cluster, now);
    p.gpus = parse_nodes(&out.stdout);
    Ok(p)
}

// ------------------------------------------------------------------------------------------------ the cache

#[derive(Clone, Debug, Default)]
struct Entry {
    profile: Option<RemoteProfile>,
    failures: u32,
    /// The earliest time (seconds) the next refresh may start.
    next: u64,
    running: bool,
}

/// What was last read per cluster (key: kubeconfig and context). Pure: the caller gives the clock.
#[derive(Default)]
pub struct Cache {
    m: Mutex<HashMap<String, Entry>>,
}

pub fn key(t: &Target) -> String {
    format!("{}|{}", t.kubeconfig, t.context)
}

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    pub fn get(&self, t: &Target) -> Option<RemoteProfile> {
        self.get_key(&key(t))
    }

    /// The cached value under a plain key (the cluster registry uses the cluster id).
    pub fn get_key(&self, k: &str) -> Option<RemoteProfile> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).get(k).and_then(|e| e.profile.clone())
    }

    /// Is a refresh due now (and none running)? Marks it running.
    pub fn begin(&self, t: &Target, now: u64) -> bool {
        self.begin_key(&key(t), now)
    }

    pub fn begin_key(&self, k: &str, now: u64) -> bool {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry(k.to_string()).or_default();
        if e.running || now < e.next {
            return false;
        }
        e.running = true;
        true
    }

    /// Store the result of a refresh: a good one replaces the value, a failure keeps the last one marked stale and backs off.
    pub fn finish(&self, t: &Target, cluster: &str, r: Result<RemoteProfile, String>, now: u64) {
        self.finish_key(&key(t), cluster, r, now)
    }

    pub fn finish_key(&self, k: &str, cluster: &str, r: Result<RemoteProfile, String>, now: u64) {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry(k.to_string()).or_default();
        e.running = false;
        match r {
            Ok(p) => {
                e.profile = Some(p);
                e.failures = 0;
                e.next = now + REFRESH_SECS;
            }
            Err(why) => {
                e.failures = e.failures.saturating_add(1);
                e.next = now + (RETRY_SECS << e.failures.saturating_sub(1).min(5)).min(RETRY_MAX_SECS);
                let mut p = e.profile.clone().unwrap_or_else(|| RemoteProfile::empty(cluster, now));
                p.stale = true;
                p.error = why;
                e.profile = Some(p);
            }
        }
    }

    /// One refresh, synchronously, when due (the thread of [`current`] and the tests).
    pub fn refresh(&self, kube: &dyn Kube, t: &Target, cluster: &str, now: u64) {
        if self.begin(t, now) {
            let r = probe(kube, t, cluster, now);
            self.finish(t, cluster, r, now);
        }
    }
}

// ------------------------------------------------------------------------------------------------ the chosen cluster

/// The target to read for a chosen cluster: none for a cluster on this machine (its GPUs are this machine's own) or one that does not answer.
pub fn target_of(p: &crate::devcluster::ClusterProfile) -> Option<(Target, String)> {
    (p.reachable && !p.local).then(|| (Target { namespace: String::new(), selector: String::new(), context: p.context.clone(), kubeconfig: p.kubeconfig.clone() }, if p.name.is_empty() { "cluster".to_string() } else { p.name.clone() }))
}

static CACHE: OnceLock<Arc<Cache>> = OnceLock::new();
static TARGET: Mutex<Option<(Target, String)>> = Mutex::new(None);

fn cache() -> Arc<Cache> {
    CACHE.get_or_init(|| Arc::new(Cache::new())).clone()
}

/// The remote cluster the person works against (registered by `devcluster` when the chosen cluster answers and is not on this
/// machine); `None` when there is none. Not a process and not a call: the cluster is read later by [`current`].
pub fn register_target(t: Option<(Target, String)>) {
    *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = t;
}

pub fn registered() -> Option<(Target, String)> {
    TARGET.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What is known about the chosen remote cluster's GPUs, at once; starts a background refresh when one is due.
pub fn current() -> Option<RemoteProfile> {
    let (t, name) = registered()?;
    let c = cache();
    let now = now_secs();
    if c.begin(&t, now) {
        let c2 = c.clone();
        std::thread::spawn(move || {
            let r = probe(&k8s::Kubectl, &t, &name, now);
            c2.finish(&t, &name, r, now_secs());
        });
    }
    c.get(&registered()?.0)
}

// ------------------------------------------------------------------------------------------------ every registry cluster

static CLUSTERS: OnceLock<Arc<Cache>> = OnceLock::new();

/// What was last read per cluster of the registry (key: the cluster id): the badge of the cluster switcher and of Settings -> Clusters.
pub fn clusters_cache() -> Arc<Cache> {
    CLUSTERS.get_or_init(|| Arc::new(Cache::new())).clone()
}

/// The badge text of a profile: `2 x NVIDIA RTX 4090 (24 GB each)`, with ` (last known)` when the last refresh failed. `None`: no GPU.
pub fn badge_of(p: &RemoteProfile) -> Option<String> {
    p.best()?;
    let w = p.words();
    Some(if p.stale { format!("{w} (last known)") } else { w })
}

/// Store a cluster's answer to `kubectl get nodes -o json` (the Clusters page and the switcher's refresh read the same JSON, so a
/// cluster is asked once). Garbage counts as a failure.
pub fn note_nodes(cache: &Cache, id: &str, name: &str, json: &str, now: u64) {
    let r = if serde_json::from_str::<Value>(json).is_err() {
        Err("kubectl printed something unreadable".to_string())
    } else {
        let mut p = RemoteProfile::empty(name, now);
        p.gpus = parse_nodes(json);
        Ok(p)
    };
    cache.finish_key(id, name, r, now);
}

/// A cluster did not answer: the last value stays, marked stale, and the next look backs off.
pub fn note_failure(cache: &Cache, id: &str, name: &str, why: &str, now: u64) {
    cache.finish_key(id, name, Err(why.to_string()), now);
}

#[cfg(test)]
#[path = "aiprofile_remote_tests.rs"]
mod tests;
