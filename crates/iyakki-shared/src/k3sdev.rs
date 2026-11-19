//! Types moved out of `iyakki_agent::k3sdev` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

pub const POD_NAME: &str = "iyakki-k3s";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Native,
    Podman,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Native => "native k3s",
            Backend::Podman => "k3s in Podman",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Svc {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

impl Svc {
    pub fn word(&self) -> &'static str {
        match self {
            Svc::NotInstalled => "not installed",
            Svc::Stopped => "stopped",
            Svc::Starting => "starting",
            Svc::Running => "running",
            Svc::Stopping => "stopping",
            Svc::Failed(_) => "failed",
        }
    }
}

/// Where this machine stands (see [`K3sDev::situation`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Situation {
    /// IYAKKI installed a cluster here.
    Managed,
    /// A desktop window: the machine manages its own cluster (or has none); IYAKKI never installs or starts one.
    Window { found: Vec<String> },
    /// A cluster that is not IYAKKI's exists on this machine.
    Foreign(Vec<String>),
    /// Nothing on this machine: IYAKKI may set one up (the problems, if any, say why it cannot yet).
    Free { problems: Vec<String> },
}

pub fn valid_id(id: &str) -> bool {
    (2..=16).contains(&id.len()) && id != "local" && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-') && !id.ends_with('-')
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub const NODE_IP: &str = "10.255.77.1";

/// k3s refuses to start without a network interface that has a default route ("no default routes found in /proc/net/route").
/// After a boot the DHCP client may not have installed one yet, so IYAKKI waits for it before every start of k3s.
pub const ROUTE_FILE: &str = "/proc/net/route";

/// Does this text of `/proc/net/route` hold an IPv4 default route through an interface that is up and is not the loopback?
/// Columns: Iface Destination Gateway Flags RefCnt Use Metric Mask ...; destination and mask 00000000, flag 0x1 (RTF_UP).
/// IPv6-only routes do not count (k3s needs IPv4 here), malformed lines are skipped.
pub fn has_default_route(text: &str) -> bool {
    text.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 8 || f[0] == "lo" {
            return false;
        }
        let flags = u32::from_str_radix(f[3], 16).unwrap_or(0);
        f[1] == "00000000" && f[7] == "00000000" && flags & 1 != 0
    })
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::clis;
use std::path::Path;
use std::path::PathBuf;

pub const NODE_NAME: &str = "iyakki-dev";

/// The interface that carries the node address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeNet {
    pub ip: String,
    pub iface: String,
    /// flannel runs host-gw instead of vxlan: on the loopback and on a bridge (which has no ports, so no carrier, and vxlan on a
    /// device without carrier is the risky part). A dummy interface keeps vxlan.
    pub host_gw: bool,
}

/// Which managed local cluster a [`K3sDev`] is: the first one (`local`, exactly today's names, ports and folder) or an extra one
/// with its own folder, API port, kubelet ports, node name and address ranges, so two of them never share a port or a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    pub dir_name: String,
    pub api_port: u16,
    pub kubelet_port: u16,
    pub node_name: String,
    pub pod_name: String,
    pub cluster_cidr: String,
    pub service_cidr: String,
    pub primary: bool,
}

impl Instance {
    /// The cluster IYAKKI has always managed: id `local`.
    pub fn primary() -> Instance {
        Instance { id: "local".into(), dir_name: "k3s-dev".into(), api_port: API_PORT, kubelet_port: 10250, node_name: NODE_NAME.into(), pod_name: POD_NAME.into(), cluster_cidr: "10.42.0.0/16".into(), service_cidr: "10.43.0.0/16".into(), primary: true }
    }

    /// An extra local cluster in slot `slot` (1, 2, ...): `id` is a short lowercase name (`valid_id`).
    pub fn extra(id: &str, slot: u16) -> Instance {
        let slot = slot.clamp(1, 60);
        Instance {
            id: id.into(),
            dir_name: format!("k3s-dev-{id}"),
            api_port: API_PORT + 100 * slot,
            kubelet_port: 10250 + 10 * slot,
            node_name: format!("{NODE_NAME}-{id}"),
            pod_name: format!("{POD_NAME}-{id}"),
            cluster_cidr: format!("10.{}.0.0/16", 42 + 2 * slot),
            service_cidr: format!("10.{}.0.0/16", 43 + 2 * slot),
            primary: false,
        }
    }

    /// The cluster DNS address inside the service range (`10.45.0.10` for `10.45.0.0/16`).
    pub fn cluster_dns(&self) -> String {
        let base = self.service_cidr.split('/').next().unwrap_or("10.43.0.0");
        let mut o: Vec<&str> = base.split('.').collect();
        if o.len() == 4 {
            o[3] = "10";
        }
        o.join(".")
    }

    /// The ports this instance listens on besides the API (kubelet, its health port, kube-proxy, controller manager, scheduler).
    pub fn extra_ports(&self) -> Vec<u16> {
        if self.primary {
            return vec![10250];
        }
        let k = self.kubelet_port;
        vec![k, k + 1, k + 2, k + 3, k + 4]
    }

    /// Every TCP port the instance needs free, API first.
    pub fn all_ports(&self) -> Vec<u16> {
        let mut v = vec![self.api_port];
        v.extend(self.extra_ports());
        v
    }
}

/// Where everything of the managed cluster lives: one folder inside the IYAKKI data partition.
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(data_dir: &Path) -> Layout {
        Layout { root: data_dir.join("k3s-dev") }
    }
    pub fn for_instance(data_dir: &Path, i: &Instance) -> Layout {
        Layout { root: data_dir.join(&i.dir_name) }
    }
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin/k3s")
    }
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn kubeconfig(&self) -> PathBuf {
        self.root.join("kubeconfig")
    }
    pub fn log(&self) -> PathBuf {
        self.root.join("k3s.log")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state.json")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

/// The sentence the page and the tools give when setup is refused.
pub fn refusal_text(found: &[String]) -> String {
    format!("This machine already has a cluster, so IYAKKI will not set up another one: {}. Use it instead (Settings \u{2192} Dev cluster lists it and lets you test the connection, set it as the default and choose a registry). Nothing was changed.", found.join("; "))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub backend: Backend,
    pub version: String,
    pub url: String,
    pub checksum_url: String,
    pub expected_sha: String,
    pub file: String,
    pub size: Option<u64>,
    pub install_path: PathBuf,
    pub data_dir: PathBuf,
    pub kubeconfig: PathBuf,
    pub log: PathBuf,
    pub registry: String,
    /// The command that starts the cluster (native: the server; Podman: the container is created with `create` and started with `start`).
    pub server: Vec<String>,
    pub pull: Vec<String>,
    pub create: Vec<String>,
    pub image: String,
    pub notes: Vec<String>,
    pub instance: Instance,
}

/// The arguments of `k3s server` for this layout: only what the plan lists.
pub fn server_args(l: &Layout) -> Vec<String> {
    server_args_for(l, &Instance::primary())
}

/// The extra flags that keep a second cluster off the first one's ports and address ranges (none for the primary).
pub fn instance_flags(i: &Instance) -> Vec<String> {
    if i.primary {
        return vec![];
    }
    let k = i.kubelet_port;
    [
        format!("--https-listen-port={}", i.api_port),
        format!("--cluster-cidr={}", i.cluster_cidr),
        format!("--service-cidr={}", i.service_cidr),
        format!("--cluster-dns={}", i.cluster_dns()),
        format!("--kubelet-arg=port={k}"),
        format!("--kubelet-arg=healthz-port={}", k + 1),
        format!("--kube-proxy-arg=metrics-bind-address=127.0.0.1:{}", k + 2),
        format!("--kube-controller-manager-arg=secure-port={}", k + 3),
        format!("--kube-scheduler-arg=secure-port={}", k + 4),
    ]
    .into()
}

pub fn server_args_for(l: &Layout, i: &Instance) -> Vec<String> {
    server_args_with(l, i, None)
}

/// The arguments with a fixed node address (`--node-ip`, the interface flannel uses): `None` leaves k3s to pick the address of the
/// default route (the old behaviour, also the fallback when the dummy interface cannot be made).
pub fn server_args_with(l: &Layout, i: &Instance, node_ip: Option<&NodeNet>) -> Vec<String> {
    let mut v: Vec<String> = ["server", "--data-dir", &l.data().display().to_string(), "--write-kubeconfig", &l.kubeconfig().display().to_string(), "--write-kubeconfig-mode", "600", "--disable", "traefik", "--disable", "servicelb", "--node-name", &i.node_name].iter().map(|s| s.to_string()).collect();
    if let Some(n) = node_ip {
        v.push(format!("--node-ip={}", n.ip));
        v.push(format!("--flannel-iface={}", n.iface));
        if n.host_gw {
            // vxlan needs an interface with carrier; one node has no peers to tunnel to
            v.push("--flannel-backend=host-gw".into());
        }
    }
    v.extend(instance_flags(i));
    v
}

/// The image tag of a release: `v1.31.1+k3s1` is `v1.31.1-k3s1` on the registry.
pub fn image_for(version: &str) -> String {
    format!("docker.io/rancher/k3s:{}", version.replace('+', "-"))
}

/// The download address, the checksum file's address and the file name of a k3s release for this machine's processor.
pub fn release_urls(version: &str) -> Result<(String, String, String), String> {
    let d = clis::def("k3s").ok_or("k3s is not in the catalogue")?;
    let (clis::Source::File(src), Some(clis::Checksum::Lines(sum))) = (d.source, d.checksum) else { return Err("the k3s catalogue entry has changed shape".into()) };
    let url = clis::template(src, version)?;
    let file = url.rsplit('/').next().unwrap_or("k3s").to_string();
    Ok((url, clis::template(sum, version)?, file))
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", (b + (1 << 19)) >> 20) }
}

/// The plan, as the person reads it before approving.
pub fn plan_text(p: &Plan, root: bool, ram_root: bool) -> String {
    let mut s = String::new();
    s.push_str(&format!("Set up a local cluster: {} {}\n\n", p.backend.label(), p.version));
    match p.backend {
        Backend::Native => {
            s.push_str("WHAT WILL BE DOWNLOADED (nothing else is downloaded by IYAKKI)\n");
            s.push_str(&format!("  {}\n  size: {}\n", p.url, p.size.map_or("unknown".to_string(), human)));
            s.push_str(&format!("  checked against the SHA-256 in the project's official file {}\n  expected: {}\n", p.checksum_url, p.expected_sha));
            s.push_str("  If the two differ the download is thrown away and nothing is installed.\n");
        }
        Backend::Podman => {
            s.push_str("WHAT WILL BE DOWNLOADED (by podman, not by IYAKKI)\n");
            s.push_str(&format!("  image {} (about 200 MB)\n  {}\n", p.image, p.pull.join(" ")));
        }
    }
    s.push_str("\nWHERE IT IS INSTALLED (inside the IYAKKI data partition; nothing goes to /usr/local, /etc or /var/lib/rancher)\n");
    if p.backend == Backend::Native {
        s.push_str(&format!("  program     {}\n", p.install_path.display()));
    }
    s.push_str(&format!("  data folder {}   (Reset deletes only this folder)\n  kubeconfig  {}   (mode 0600; registered as this machine's cluster)\n  log         {}\n", p.data_dir.display(), p.kubeconfig.display(), p.log.display()));
    s.push_str("\nWHAT IT STARTS\n");
    match p.backend {
        Backend::Native => {
            s.push_str(&format!("  {}\n", p.server.join(" ")));
            s.push_str(&format!("  as a service of IYAKKI: started now and at every start of IYAKKI (switch off with Autostart), restarted with a back-off when it fails (up to {MAX_FAILURES} times in {} minutes, then it stops and you are told).\n", FAIL_WINDOW.as_secs() / 60));
            if ram_root {
                s.push_str("  The system runs from RAM, so k3s is started in a mount namespace of its own whose root is a tmpfs (the same method as the sandbox cluster).\n");
            }
        }
        Backend::Podman => {
            s.push_str(&format!("  {}\n  then: podman start {}  (IYAKKI starts it, watches it and starts it again when it stops)\n", p.create.join(" "), p.instance.pod_name));
            s.push_str("  The container is privileged and uses the host network: that is how k3s runs in a container.\n");
        }
    }
    s.push_str("  Switched off to save memory and ports: traefik, servicelb. metrics-server stays on (kubectl top, the usage columns of the Kubernetes tile).\n");
    s.push_str("\nPORTS\n");
    for (port, what) in ports_for(&p.instance) {
        s.push_str(&format!("  {port}  {what}\n"));
    }
    s.push_str(&format!("  The API server listens on every interface (TLS and a token are required); keep the machine off untrusted networks or firewall port {}.\n", p.instance.api_port));
    s.push_str("\nREGISTRY\n");
    s.push_str(match p.registry.as_str() {
        "system" => "  B, a system registry (registry:2 with podman or docker on localhost:5000). It is set up after the cluster is Ready, as a separate step you approve.\n",
        "none" => "  none for now. You can choose one later on this page.\n",
        _ => "  A, a registry pod inside the cluster (recommended, no root, no node changes). It is set up after the cluster is Ready, as a separate step you approve.\n",
    });
    s.push_str("\nROOT\n");
    s.push_str(if root { "  IYAKKI runs as root here, so the cluster can start. IYAKKI never uses sudo.\n" } else { "  IYAKKI is NOT root here: k3s cannot start. Setup is refused.\n" });
    s.push_str("\nNEEDS about 2 GB of memory, cgroup v2 and 3 GB free on the data partition.\n");
    for n in &p.notes {
        s.push_str(&format!("\nNote: {n}\n"));
    }
    s.trim_end().to_string()
}

/// The ports the cluster uses.
pub fn ports() -> Vec<(String, &'static str)> {
    ports_for(&Instance::primary())
}

/// The ports one managed local cluster uses.
pub fn ports_for(i: &Instance) -> Vec<(String, &'static str)> {
    let mut v = vec![(format!("{}/tcp", i.api_port), "the Kubernetes API (kubectl)"), (format!("{}/tcp", i.kubelet_port), "the kubelet")];
    if !i.primary {
        let k = i.kubelet_port;
        v.push((format!("{}-{}/tcp", k + 1, k + 4), "kubelet health, kube-proxy metrics, controller manager and scheduler"));
    }
    v.extend(ports_common());
    v
}

pub fn ports_common() -> Vec<(String, &'static str)> {
    vec![
        ("8472/udp".into(), "flannel VXLAN between nodes"),
        ("30000-32767/tcp".into(), "NodePort services (the in-cluster registry is not one: it listens on 127.0.0.1:30500 only)"),
    ]
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use std::time::Duration;

pub const API_PORT: u16 = 6443;

/// At most this many unexpected exits within [`FAIL_WINDOW`] before the supervisor gives up.
pub const MAX_FAILURES: usize = 5;

pub const FAIL_WINDOW: Duration = Duration::from_secs(600);

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// What SYSTEM and the Dev cluster page show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub installed: bool,
    pub backend: Backend,
    /// `not installed`, `stopped`, `starting`, `running`, `stopping`, `failed`.
    pub state: String,
    pub failed_reason: String,
    pub version: String,
    pub node_ready: Option<bool>,
    /// (running, all) pods in every namespace.
    pub pods: Option<(usize, usize)>,
    pub data_bytes: Option<u64>,
    pub disk_pct: Option<u8>,
    pub disk_free: Option<u64>,
    pub restarts: u32,
    pub last_restart_reason: String,
    pub autostart: bool,
    /// The last unexpected exit of k3s (unix seconds; 0: none) and why.
    pub last_exit_at: i64,
    pub last_exit_reason: String,
    pub checked_at: i64,
    /// "waiting for the network..." while the start waits for a default route; empty otherwise.
    pub note: String,
}

impl Health {
    /// The one line SYSTEM shows.
    pub fn line(&self) -> String {
        if !self.installed {
            return "not installed".into();
        }
        let mut s = self.state.clone();
        if !self.note.is_empty() {
            s.push_str(&format!(" · {}", self.note));
        }
        if !self.version.is_empty() {
            s.push_str(&format!(" · {}", self.version));
        }
        match self.node_ready {
            Some(true) => s.push_str(" · node Ready"),
            Some(false) => s.push_str(" · node NOT ready"),
            None => {}
        }
        if let Some((r, n)) = self.pods {
            s.push_str(&format!(" · {r}/{n} pods"));
        }
        s
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----

/// What is remembered about the installation (`state.json`; its presence means "IYAKKI manages a cluster here").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Layout of `state.json` (see `iyakki-schema`): 0 = alpha.1.
    pub schema: u32,
    /// Fields a newer release wrote: kept, and written back.
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
    pub backend: Backend,
    pub version: String,
    pub sha256: String,
    pub url: String,
    pub installed_at: i64,
    pub installed_by: String,
    /// Start at boot (and when IYAKKI starts).
    pub autostart: bool,
    /// The registry option chosen at setup: `in-cluster` (A), `system` (B) or `none`; applying it is a separate approved step.
    pub registry_choice: String,
    /// Podman only: the image and the digest it had when it was pulled.
    pub image: String,
    pub image_digest: String,
    pub restarts: u32,
    pub last_restart_reason: String,
    pub last_restart_at: i64,
    /// The fixed node address the cluster was started with last (empty: never, or the address of the default route was used).
    pub node_ip: String,
    /// The last unexpected exit (unix seconds; 0: none) and why, for the Dev cluster page.
    pub last_exit_at: i64,
    pub last_exit_reason: String,
    /// The fixed node address was refused by k3s (or never became Ready): later starts leave it out until it is switched on again.
    pub node_ip_disabled: bool,
    /// The cluster became Ready with the fixed node address at least once on this machine.
    pub node_ip_worked: bool,
}

/// `state.json`: schema 1 stamps the alpha.1 layout (later fields have serde defaults).
pub const STATE_SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "dev cluster state", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the alpha.1 layout", run: |_| {} }] };

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alert {
    /// The cluster stopped unexpectedly (the first exit of an outage; the supervisor is restarting it).
    Down(String),
    /// The supervisor gave up after repeated failures, or the node never became Ready.
    GaveUp(String),
    Disk(u8),
    /// k3s refused the fixed node address; the cluster runs on the network address.
    NodeIpRefused(String),
    /// Services open on the LAN (NodePort or LoadBalancer) that no document lists: one line each.
    Exposure(Vec<String>),
}

impl Alert {
    pub fn title_body(&self) -> (String, String) {
        match self {
            Alert::Down(why) => ("The dev cluster stopped and was restarted".into(), format!("{why}. IYAKKI is restarting it (with a back-off). Settings \u{2192} Dev cluster or SYSTEM shows the log.")),
            Alert::GaveUp(why) => ("The dev cluster keeps failing and was stopped".into(), format!("IYAKKI gave up: {why}. Settings \u{2192} Dev cluster or SYSTEM shows the log; Start tries again once the cause is fixed.")),
            Alert::NodeIpRefused(why) => ("The fixed node address was refused; the cluster runs on the network address".into(), format!("{why}. The cluster was started again without it and stays that way until you switch \"Use a fixed node address (experimental)\" on in Settings \u{2192} Dev cluster.")),
            Alert::Exposure(v) => ("The dev cluster has Services open on your network that are not on the list".into(), format!("{}. A NodePort or LoadBalancer Service can be reached from other computers (the firewall is off by default). IYAKKI changes nothing by itself: if you did not expect this, change the Service to ClusterIP or turn the firewall on (Settings \u{2192} Firewall); if you did, you can ignore this.", v.join("; "))),
            Alert::Disk(p) => (format!("The dev cluster's disk is {p}% full"), format!("The data partition holding the cluster is over {DISK_ALERT_PCT}% full. Free space (remove unused images or namespaces), or Reset the cluster in Settings \u{2192} Dev cluster."))
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub const DISK_ALERT_PCT: u8 = 85;
