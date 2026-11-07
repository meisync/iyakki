//! What each cluster item of Quick install may use: the pod's name, its CPU and memory requests and limits, and the idle memory the
//! owner measured on the Mac mini on 2026-11-06. The manifests carry the same numbers (a test per item compares them), the item's
//! popup shows them, and the review of an install warns when the selected services together could take more than 60 % of the memory.
//!
//! Nothing here says why a service is busy: ClickHouse used about a core while idle, and the System tile's "What is it busy with"
//! aid is where that is looked into.

use super::*;

/// One item's pod and its numbers. `cpu_*` are Kubernetes quantities ("100m", "2"), `mem_*` too ("512Mi", "1Gi").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Res {
    pub item: Item,
    pub ns: &'static str,
    /// The Deployment (and the pod name prefix).
    pub pod: &'static str,
    pub cpu_req: &'static str,
    pub cpu_lim: &'static str,
    pub mem_req: &'static str,
    pub mem_lim: &'static str,
    /// Memory used while idle as the owner measured it on 2026-11-06 (megabytes); 0 = not measured.
    pub idle_mb: u32,
}

pub const ELASTICSEARCH: Res = Res { item: Item::Elasticsearch, ns: NAMESPACE, pod: "elasticsearch", cpu_req: "250m", cpu_lim: "2", mem_req: "1Gi", mem_lim: "1536Mi", idle_mb: 925 };
pub const OPENSEARCH: Res = Res { item: Item::OpenSearch, ns: NAMESPACE, pod: "opensearch", cpu_req: "100m", cpu_lim: "2", mem_req: "1Gi", mem_lim: "1536Mi", idle_mb: 990 };
pub const CLICKHOUSE: Res = Res { item: Item::ClickHouse, ns: NAMESPACE, pod: "clickhouse", cpu_req: "100m", cpu_lim: "1", mem_req: "512Mi", mem_lim: "1Gi", idle_mb: 775 };
pub const GRAPHDB: Res = Res { item: Item::GraphDb, ns: NAMESPACE, pod: "graphdb", cpu_req: "100m", cpu_lim: "2", mem_req: "640Mi", mem_lim: "1Gi", idle_mb: 400 };
pub const PROMETHEUS: Res = Res { item: Item::Prometheus, ns: PROM_NAMESPACE, pod: "prometheus", cpu_req: "100m", cpu_lim: "500m", mem_req: "256Mi", mem_lim: "768Mi", idle_mb: 190 };
pub const GITEA: Res = Res { item: Item::Gitea, ns: gitea::NAMESPACE, pod: "gitea", cpu_req: "100m", cpu_lim: "1", mem_req: "256Mi", mem_lim: "1Gi", idle_mb: 180 };
pub const MATRIX: Res = Res { item: Item::Matrix, ns: matrix::NS, pod: "matrix", cpu_req: "50m", cpu_lim: "1", mem_req: "128Mi", mem_lim: "512Mi", idle_mb: 0 };
pub const SEAWEEDFS: Res = Res { item: Item::SeaweedFs, ns: objstore::NS, pod: "seaweedfs", cpu_req: "100m", cpu_lim: "1", mem_req: "256Mi", mem_lim: "1Gi", idle_mb: 0 };
pub const RUSTFS: Res = Res { item: Item::RustFs, ns: objstore::NS, pod: "rustfs", cpu_req: "100m", cpu_lim: "1", mem_req: "256Mi", mem_lim: "1Gi", idle_mb: 0 };
pub const REGISTRY: Res = Res { item: Item::Registry, ns: crate::regsetup::IN_NS, pod: crate::regsetup::IN_NAME, cpu_req: "50m", cpu_lim: "500m", mem_req: "64Mi", mem_lim: "512Mi", idle_mb: 0 };
/// The model servers: a model needs the memory, so the ceiling is high on purpose (half of a 32 GB machine), and the CPU limit lets
/// them use six cores. Both leave room for the other services.
pub const LEMONADE: Res = Res { item: Item::Lemonade, ns: LEMONADE_NAMESPACE, pod: "lemonade", cpu_req: "500m", cpu_lim: "6", mem_req: "1Gi", mem_lim: "16Gi", idle_mb: 0 };
pub const OLLAMA: Res = Res { item: Item::Ollama, ns: LEMONADE_NAMESPACE, pod: "ollama", cpu_req: "500m", cpu_lim: "6", mem_req: "2Gi", mem_lim: "16Gi", idle_mb: 0 };

pub const ALL: [Res; 12] = [ELASTICSEARCH, OPENSEARCH, CLICKHOUSE, GRAPHDB, PROMETHEUS, GITEA, MATRIX, SEAWEEDFS, RUSTFS, REGISTRY, LEMONADE, OLLAMA];

pub fn of(item: Item) -> Option<&'static Res> {
    ALL.iter().find(|r| r.item == item)
}

/// A memory quantity ("512Mi", "1536Mi", "16Gi") in megabytes; 0 when it is not one of these.
pub fn mem_mb(q: &str) -> u32 {
    if let Some(n) = q.strip_suffix("Mi") {
        n.parse().unwrap_or(0)
    } else if let Some(n) = q.strip_suffix("Gi") {
        n.parse::<u32>().map(|g| g * 1024).unwrap_or(0)
    } else {
        0
    }
}

/// A memory quantity in words: "512 MB", "1.5 GB", "16 GB".
pub fn mem_words(q: &str) -> String {
    let mb = mem_mb(q);
    if mb >= 1024 && mb % 1024 == 0 {
        format!("{} GB", mb / 1024)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// A CPU quantity in words: "100m" is "0.1 of a core", "2" is "2 cores".
pub fn cpu_words(q: &str) -> String {
    match q.strip_suffix('m').and_then(|n| n.parse::<u32>().ok()) {
        Some(m) if m >= 1000 => format!("{} cores", m / 1000),
        Some(m) => format!("{} of a core", m as f64 / 1000.0),
        None if q == "1" => "1 core".into(),
        None => format!("{q} cores"),
    }
}

impl Res {
    /// The lines the popup shows: the pod, then what it asks for and what it may use at most, then what was measured when idle.
    pub fn lines(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("Pod".to_string(), format!("deployment {} in namespace {} (the pod's name starts with {}-)", self.pod, self.ns, self.pod)),
            ("CPU".to_string(), format!("asks for {} ({}), may use up to {} ({})", self.cpu_req, cpu_words(self.cpu_req), self.cpu_lim, cpu_words(self.cpu_lim))),
            ("Memory".to_string(), format!("asks for {} ({}), may use up to {} ({}); past that the cluster restarts it", self.mem_req, mem_words(self.mem_req), self.mem_lim, mem_words(self.mem_lim))),
        ];
        if self.idle_mb > 0 {
            v.push(("Idle (measured)".to_string(), format!("about {} MB of memory while idle, measured on the Mac mini on 2026-11-06", self.idle_mb)));
        }
        v
    }

    /// The same as plain text for chat.
    pub fn text(&self) -> String {
        self.lines().into_iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; ")
    }
}

/// The share of the machine's memory above which the review warns (percent).
pub const WARN_PERCENT: u64 = 60;

/// The warning for a plan: the selected services (the steps the plan will run) together with the ones already installed, by their
/// memory limits (the most they may use). `None` when the memory is unknown, when no service of the table is involved, or when the
/// total stays at or under 60 % of the machine. Plain words.
pub fn total_warning(steps: &[Item], installed: &[Item], ram_mb: u64) -> Option<String> {
    if ram_mb == 0 || !steps.iter().any(|s| of(*s).is_some()) {
        return None;
    }
    let mut all: Vec<Item> = installed.to_vec();
    for s in steps {
        if !all.contains(s) {
            all.push(*s);
        }
    }
    let rows: Vec<&Res> = all.iter().filter_map(|i| of(*i)).collect();
    let total: u64 = rows.iter().map(|r| mem_mb(r.mem_lim) as u64).sum();
    if total * 100 <= ram_mb * WARN_PERCENT {
        return None;
    }
    let names = rows.iter().map(|r| format!("{} {}", r.item.title(), mem_words(r.mem_lim))).collect::<Vec<_>>().join(", ");
    Some(format!(
        "Memory warning: together these services (new and already installed) may use up to {:.1} GB of this machine's {:.1} GB, more than {WARN_PERCENT} % of it ({names}). They rarely use that much when idle, but a busy day can make the machine slow or make the cluster restart a service. Install fewer of them, or add memory, if that worries you.",
        total as f64 / 1024.0,
        ram_mb as f64 / 1024.0
    ))
}
