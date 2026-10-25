//! A local k3s cluster that IYAKKI itself installs and keeps on a bare-metal machine (docs/K3S-DEV.md, phase 4).
//!
//! * **Only when no cluster exists.** [`existing`] looks for a k3s/rke2 that is installed or running (files, systemd units, processes),
//!   for a connected cluster and for the sandbox cluster; if it finds one, [`install`] and [`K3sDev::start`] refuse and say what they found.
//!   In a desktop window IYAKKI never installs or starts a cluster at all.
//! * **Nothing silent.** [`lookup`] reads the latest release (version, the vendor's checksum file, the size) and builds a [`Plan`];
//!   [`plan_text`] shows everything it will download, where it installs, what it starts and on which ports. Only after the person
//!   approves does [`install`] run, and every step is audited.
//! * **Inside the data partition.** Program, data, kubeconfig and log live in `<data>/k3s-dev/`; nothing is written to `/usr/local`,
//!   `/etc` or `/var/lib/rancher`. Reset deletes only the `data` folder there (after a typed confirmation); Uninstall deletes the folder.
//! * **Two ways to run it.** Native (the k3s program, started by IYAKKI in its own process group, in the RAM-root jail of `cluster.rs`
//!   when the system runs from RAM) or, only where `podman` is installed, the official k3s image as a privileged container. Either way
//!   the supervisor thread restarts it with a back-off when it dies, and gives up (and tells the administrators) after repeated failures.
//! * **Health** ([`Health`]): state, version, node Ready, pods, size of the data folder, how full the disk is, the last restart reason;
//!   alerts ([`K3sDev::take_alerts`]) when the cluster goes down or the disk passes 85 %.
//!
//! Programs other than the k3s server itself run through [`crate::wsexec::Runner`] with argument arrays (`k3s kubectl`, `podman`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clis::{self, CliManager};
use crate::wsexec::{Out, Runner};

pub const API_PORT: u16 = 6443;
pub const NODE_NAME: &str = "iyakki-dev";
pub const POD_NAME: &str = "iyakki-k3s";
pub const DISK_ALERT_PCT: u8 = 85;
const DISK_CLEAR_PCT: u8 = 80;
/// At most this many unexpected exits within [`FAIL_WINDOW`] before the supervisor gives up.
const MAX_FAILURES: usize = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(600);
/// The data partition must have at least this much free room to install.
pub const MIN_FREE: u64 = 3 << 30;

/// k3s refuses to start without a network interface that has a default route ("no default routes found in /proc/net/route").
/// After a boot the DHCP client may not have installed one yet, so IYAKKI waits for it before every start of k3s.
pub const ROUTE_FILE: &str = "/proc/net/route";
/// After this long without a default route the status says "no network yet"; the wait goes on (without limit, until Stop).
const NET_GRACE: Duration = Duration::from_secs(90);
const NET_POLL: Duration = Duration::from_secs(3);
pub const WAITING_NET: &str = "waiting for the network\u{2026}";
pub const NO_NET_YET: &str = "no network yet: k3s needs a network interface with a default route; it starts by itself when one appears";

/// A machine whose clock starts in the past (a Mac's RTC reads 2024) must not start k3s before the clock is plausible: its
/// certificates, made later, are "not yet valid" at the fake date. The start waits until the clock floor was applied
/// ([`ClockSource::floor_pending`]) AND (a time server answered OR the machine has been up for [`CLOCK_WAIT_LIMIT`]: offline
/// the floor is good enough).
pub const CLOCK_WAIT_LIMIT: Duration = Duration::from_secs(120);
pub const WAITING_CLOCK: &str = "waiting for the clock to be set\u{2026}";
/// A jump of the wall clock against the monotonic clock of more than this while k3s runs restarts k3s once (not a failure).
pub const CLOCK_STEP_SECS: i64 = 180;
/// k3s complaining about certificates within this long of its start (and the clock plausible) is restarted once.
const CERT_WINDOW: Duration = Duration::from_secs(120);
/// At most this many clock-step restarts per supervisor (a clock that keeps jumping is not made worse by a restart loop).
const MAX_CLOCK_RESTARTS: usize = 3;

/// What the start gate and the step detection know about the clock. The real one is [`RealClock`]; tests bring a fake.
pub trait ClockSource: Send + Sync {
    /// Wall clock, unix seconds.
    fn wall(&self) -> i64;
    /// Monotonic time since boot (never jumps).
    fn mono(&self) -> Duration;
    /// The clock floor has not been applied yet (early boot).
    fn floor_pending(&self) -> bool;
    /// A time server has answered.
    fn synced(&self) -> bool;
}

pub struct RealClock;

impl ClockSource for RealClock {
    fn wall(&self) -> i64 {
        now()
    }
    fn mono(&self) -> Duration {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let up = std::fs::read_to_string("/proc/uptime").ok().and_then(|t| t.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()));
        up.map(Duration::from_secs_f64).unwrap_or_else(|| START.get_or_init(Instant::now).elapsed())
    }
    fn floor_pending(&self) -> bool {
        crate::clockfloor::pending()
    }
    fn synced(&self) -> bool {
        crate::ntp::synced()
    }
}

/// Is the clock plausible enough to start k3s?
pub fn clock_ready(floor_pending: bool, synced: bool, uptime: Duration, limit: Duration) -> bool {
    !floor_pending && (synced || uptime >= limit)
}

/// By how many seconds did the wall clock jump between two looks (`None`: by less than `threshold`)? The monotonic clock
/// says how much time really passed.
pub fn clock_stepped(prev_wall: i64, prev_mono: Duration, wall: i64, mono: Duration, threshold: i64) -> Option<i64> {
    let passed = mono.saturating_sub(prev_mono).as_secs() as i64;
    let jump = (wall - prev_wall) - passed;
    (jump.abs() > threshold).then_some(jump)
}

/// Does this piece of k3s log show the failure of a certificate checked at the wrong date?
pub fn log_shows_clock_trouble(text: &str) -> bool {
    text.contains("CA cert validation failed") || (text.contains("x509") && text.contains("not yet valid"))
}

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

/// How the wait for a default route ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetWait {
    Ready,
    Cancelled,
}

/// What the wait tells the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetNote {
    /// First seconds (usually DHCP is still working).
    Waiting,
    /// The grace time is over and there is still no route: a waiting state, not a failure.
    NoNetworkYet,
}

/// Waits until `has_route()` is true. `sleep(step)` returns false when the wait was cancelled (Stop); the elapsed time is the sum of the
/// steps, so a test with a fake clock needs no real time. Never counts as a failure and never gives up.
pub fn wait_for_route(mut has_route: impl FnMut() -> bool, mut sleep: impl FnMut(Duration) -> bool, mut note: impl FnMut(NetNote), grace: Duration, step: Duration) -> NetWait {
    let mut elapsed = Duration::ZERO;
    let mut said_no_net = false;
    note(NetNote::Waiting);
    loop {
        if has_route() {
            return NetWait::Ready;
        }
        if !sleep(step) {
            return NetWait::Cancelled;
        }
        elapsed += step;
        if elapsed >= grace && !said_no_net {
            said_no_net = true;
            note(NetNote::NoNetworkYet);
        }
    }
}

/// "exit status 3", "signal 9 (SIGKILL)" or "signal 11 (SIGSEGV), core dumped": how a process ended, for the log and the audit.
pub fn exit_text(st: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = st.signal() {
        let name = match sig {
            libc::SIGKILL => " (SIGKILL)",
            libc::SIGTERM => " (SIGTERM)",
            libc::SIGSEGV => " (SIGSEGV)",
            libc::SIGABRT => " (SIGABRT)",
            libc::SIGBUS => " (SIGBUS)",
            libc::SIGHUP => " (SIGHUP)",
            libc::SIGINT => " (SIGINT)",
            _ => "",
        };
        format!("signal {sig}{name}{}", if st.core_dumped() { ", core dumped" } else { "" })
    } else {
        match st.code() {
            Some(c) => format!("exit status {c}"),
            None => st.to_string(),
        }
    }
}

/// The IPv4 address of the interface that carries the default route (`route_text` is /proc/net/route), as 4 bytes in text.
fn default_route_iface(route_text: &str) -> Option<String> {
    route_text.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        let flags = f.get(3).and_then(|x| u32::from_str_radix(x, 16).ok()).unwrap_or(0);
        (f.len() >= 8 && f[0] != "lo" && f[1] == "00000000" && f[7] == "00000000" && flags & 1 != 0).then(|| f[0].to_string())
    })
}

fn iface_ipv4(name: &str) -> Option<String> {
    let mut found = None;
    // SAFETY: getifaddrs fills a list that is read and then freed with freeifaddrs.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut p = list;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null() && (*a.ifa_addr).sa_family as i32 == libc::AF_INET && std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy() == name {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                let b = sin.sin_addr.s_addr.to_ne_bytes();
                found = Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
                break;
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

/// How long the supervisor may go without a heartbeat before the watchdog takes over.
const STALL_AFTER: Duration = Duration::from_secs(150);

/// The ids of the managed local clusters that are starting or running in this process.
static RUNNING_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn running_ids() -> std::sync::MutexGuard<'static, Vec<String>> {
    RUNNING_IDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Is a managed dev cluster (any of them) starting or running in this process? (The sandbox cluster refuses to start then.)
pub fn managed_running() -> bool {
    !running_ids().is_empty()
}

/// How many managed local clusters are starting or running in this process.
pub fn managed_running_count() -> usize {
    running_ids().len()
}

// ------------------------------------------------------------------------------------------------ instances

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

/// A short id for an extra cluster: lowercase letters, digits and `-`, 2 to 16 characters, not `local`.
pub fn valid_id(id: &str) -> bool {
    (2..=16).contains(&id.len()) && id != "local" && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-') && !id.ends_with('-')
}

/// `(user, action, detail)`.
pub type AuditFn = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

fn io_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now() -> i64 {
    crate::clock::now_unix()
}

// ------------------------------------------------------------------------------------------------ layout and state

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

/// What is remembered about the installation (`state.json`; its presence means "IYAKKI manages a cluster here").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
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
}

fn load_state(l: &Layout) -> Option<State> {
    serde_json::from_str(&std::fs::read_to_string(l.state()).ok()?).ok()
}

fn save_state(l: &Layout, s: &State) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&l.root).map_err(io_err)?;
    let tmp = l.root.join("state.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).map_err(io_err)?).map_err(io_err)?;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    std::fs::rename(&tmp, l.state()).map_err(io_err)
}

// ------------------------------------------------------------------------------------------------ does a cluster exist already?

/// The places a cluster on this machine shows itself. [`Machine::real`] is this computer; tests give their own folders.
#[derive(Clone, Debug)]
pub struct Machine {
    pub k3s_yaml: PathBuf,
    pub lib_dir: PathBuf,
    /// Program files that would be a system-wide k3s.
    pub bins: Vec<PathBuf>,
    pub units: Vec<PathBuf>,
    pub proc_dir: PathBuf,
}

impl Machine {
    pub fn real() -> Machine {
        let mut bins: Vec<PathBuf> = ["/usr/local/bin/k3s", "/usr/bin/k3s", "/usr/local/bin/rke2", "/usr/bin/rke2", "/usr/local/bin/k3s-uninstall.sh"].iter().map(PathBuf::from).collect();
        for d in std::env::var("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
            bins.push(Path::new(d).join("k3s"));
        }
        Machine {
            k3s_yaml: "/etc/rancher/k3s/k3s.yaml".into(),
            lib_dir: "/var/lib/rancher/k3s".into(),
            bins,
            units: ["/etc/systemd/system/k3s.service", "/etc/systemd/system/k3s-agent.service", "/etc/systemd/system/rke2-server.service", "/etc/init.d/k3s", "/etc/init.d/k3s-agent"].iter().map(PathBuf::from).collect(),
            proc_dir: "/proc".into(),
        }
    }
}

/// Whether `/var/lib/rancher/k3s` holds a cluster, not just the binaries k3s unpacks into its `data` folder the first time any of its
/// commands (`k3s kubectl`, a `kubectl` link to the k3s program) runs without its own data directory. Those binaries are no cluster; any other
/// entry (server, agent, storage, ...) is, and so is a folder that cannot be read.
fn lib_dir_has_state(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        Ok(it) => it.flatten().any(|e| e.file_name() != "data"),
    }
}

/// Everything that says a cluster already exists. `ignore` are folders whose files do not count (IYAKKI's own: the CLI tile's `k3s`
/// and the managed folder). `connected` is the cluster some workspace is connected to, `sandbox_up` that IYAKKI's sandbox cluster runs.
pub fn existing(m: &Machine, ignore: &[PathBuf], connected: Option<&str>, sandbox_up: bool) -> Vec<String> {
    let mut v = Vec::new();
    let ignored = |p: &Path| ignore.iter().any(|i| p.starts_with(i));
    if m.k3s_yaml.exists() {
        v.push(format!("{} exists (a k3s server was installed here)", m.k3s_yaml.display()));
    }
    if lib_dir_has_state(&m.lib_dir) {
        v.push(format!("{} exists (k3s has run here)", m.lib_dir.display()));
    }
    for b in &m.bins {
        if !ignored(b) && b.exists() && !v.iter().any(|x| x.contains(&b.display().to_string())) {
            v.push(format!("{} is installed", b.display()));
        }
    }
    for u in &m.units {
        if u.exists() {
            v.push(format!("the service file {} exists", u.display()));
        }
    }
    if let Ok(rd) = std::fs::read_dir(&m.proc_dir) {
        let mut seen: Vec<String> = Vec::new();
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let comm = comm.trim();
            if matches!(comm, "k3s" | "k3s-server" | "k3s-agent" | "rke2" | "kube-apiserver" | "kubelet" | "minikube" | "microk8s") && !seen.iter().any(|s| s == comm) {
                seen.push(comm.to_string());
            }
        }
        for c in seen {
            v.push(format!("a process named {c} is running"));
        }
    }
    if let Some(c) = connected {
        v.push(format!("a cluster is connected: {c}"));
    }
    if sandbox_up {
        v.push("IYAKKI's sandbox cluster (the CLI tile's k3s) is running".to_string());
    }
    v
}

/// The sentence the page and the tools give when setup is refused.
pub fn refusal_text(found: &[String]) -> String {
    format!("This machine already has a cluster, so IYAKKI will not set up another one: {}. Use it instead (Settings \u{2192} Dev cluster lists it and lets you test the connection, set it as the default and choose a registry). Nothing was changed.", found.join("; "))
}

// ------------------------------------------------------------------------------------------------ the plan

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
fn instance_flags(i: &Instance) -> Vec<String> {
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
    let mut v: Vec<String> = ["server", "--data-dir", &l.data().display().to_string(), "--write-kubeconfig", &l.kubeconfig().display().to_string(), "--write-kubeconfig-mode", "600", "--disable", "traefik", "--disable", "servicelb", "--node-name", &i.node_name].iter().map(|s| s.to_string()).collect();
    v.extend(instance_flags(i));
    v
}

fn podman_create(l: &Layout, image: &str, i: &Instance) -> Vec<String> {
    let mut a: Vec<String> = ["podman", "create", "--name", &i.pod_name, "--privileged", "--network=host", "--tmpfs", "/run", "--tmpfs", "/var/run", "-v"].iter().map(|s| s.to_string()).collect();
    a.push(format!("{}:/var/lib/rancher/k3s", l.data().display()));
    a.push("-v".into());
    a.push(format!("{}:/output", l.root.display()));
    a.extend(["-e", "K3S_KUBECONFIG_OUTPUT=/output/kubeconfig", "-e", "K3S_KUBECONFIG_MODE=600"].iter().map(|s| s.to_string()));
    a.push(image.to_string());
    a.extend(["server", "--disable", "traefik", "--disable", "servicelb", "--node-name", &i.node_name].iter().map(|s| s.to_string()));
    a.extend(instance_flags(i));
    a
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

/// Read what a setup would download (version, the vendor's checksum, the size) and build the plan. This only reads from the network
/// (the release redirect, the checksum file and the headers of the download); it installs and starts nothing.
pub fn lookup(clis: &CliManager, st: &dyn iyakki_proxy::Streamer, l: &Layout, backend: Backend, registry: &str) -> Result<Plan, String> {
    lookup_for(clis, st, l, backend, registry, &Instance::primary())
}

/// [`lookup`] for one managed local cluster (the primary or an extra one).
pub fn lookup_for(clis: &CliManager, st: &dyn iyakki_proxy::Streamer, l: &Layout, backend: Backend, registry: &str, inst: &Instance) -> Result<Plan, String> {
    let d = clis::def("k3s").ok_or("k3s is not in the catalogue")?;
    let version = clis.latest_version(d, st)?;
    let (clis::Source::File(src), Some(clis::Checksum::Lines(sum))) = (d.source, d.checksum) else { return Err("the k3s catalogue entry has changed shape".into()) };
    let url = clis::template(src, &version)?;
    let checksum_url = clis::template(sum, &version)?;
    let file = url.rsplit('/').next().unwrap_or("k3s").to_string();
    let mut p = Plan {
        backend,
        version: version.clone(),
        url: url.clone(),
        checksum_url: checksum_url.clone(),
        expected_sha: String::new(),
        file: file.clone(),
        size: None,
        install_path: l.bin(),
        data_dir: l.data(),
        kubeconfig: l.kubeconfig(),
        log: l.log(),
        registry: if registry.is_empty() { "in-cluster".into() } else { registry.into() },
        server: Vec::new(),
        pull: Vec::new(),
        create: Vec::new(),
        image: String::new(),
        notes: Vec::new(),
        instance: inst.clone(),
    };
    match backend {
        Backend::Native => {
            let (head, _) = clis.open_follow(st, &url, d.hosts)?;
            p.size = head.header("content-length").and_then(|v| v.parse::<u64>().ok());
            drop(head);
            let text = clis.small(st, &checksum_url, d.hosts)?;
            p.expected_sha = clis::extract_hash(&text, &file).ok_or("the vendor's checksum file has no entry for this download")?;
            let mut s = vec![l.bin().display().to_string()];
            s.extend(server_args_for(l, inst));
            p.server = s;
        }
        Backend::Podman => {
            p.image = image_for(&version);
            p.pull = vec!["podman".into(), "pull".into(), p.image.clone()];
            p.create = podman_create(l, &p.image, inst);
            p.install_path = PathBuf::new();
            p.notes.push("The image is pulled by tag from Docker Hub. Podman verifies the layers against the image's own digest, but there is no separately published checksum to compare with; the digest it ends up with is recorded and shown in the log.".into());
        }
    }
    Ok(p)
}

fn human(b: u64) -> String {
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

fn ports_common() -> Vec<(String, &'static str)> {
    vec![
        ("8472/udp".into(), "flannel VXLAN between nodes"),
        ("30000-32767/tcp".into(), "NodePort services (the in-cluster registry uses 30500)"),
    ]
}

// ------------------------------------------------------------------------------------------------ the service

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alert {
    /// The cluster stopped unexpectedly (the first exit of an outage; the supervisor is restarting it).
    Down(String),
    /// The supervisor gave up after repeated failures, or the node never became Ready.
    GaveUp(String),
    Disk(u8),
}

impl Alert {
    pub fn title_body(&self) -> (String, String) {
        match self {
            Alert::Down(why) => ("The dev cluster went down".into(), format!("{why}. IYAKKI is restarting it. Settings \u{2192} Dev cluster or SYSTEM shows the log.")),
            Alert::GaveUp(why) => ("The dev cluster keeps failing and was stopped".into(), format!("{why}. Settings \u{2192} Dev cluster or SYSTEM shows the log; Start tries again once the cause is fixed.")),
            Alert::Disk(p) => (format!("The dev cluster's disk is {p}% full"), format!("The data partition holding the cluster is over {DISK_ALERT_PCT}% full. Free space (remove unused images or namespaces), or Reset the cluster in Settings \u{2192} Dev cluster."))
        }
    }
}

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

struct Live {
    svc: Svc,
    progress: Progress,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

#[derive(Default)]
struct Overrides {
    ramfs: Option<bool>,
    root: Option<bool>,
    cgroups: Option<bool>,
    backoff: Option<Duration>,
    poll: Option<Duration>,
    window: Option<bool>,
    /// Tests: leave the process-wide state alone (the running set, the connected cluster, the machine's registered kubeconfig).
    quiet: bool,
    connected: Option<Option<String>>,
    /// Tests: the file read instead of /proc/net/route.
    route_file: Option<PathBuf>,
    net_grace: Option<Duration>,
    /// Tests: the address the default route's interface has now (`Some(None)`: none).
    ip: Option<Option<String>>,
    stall_after: Option<Duration>,
    /// Tests: the clock the start gate and the step detection look at.
    clock: Option<Arc<dyn ClockSource>>,
}

pub struct K3sDev {
    inst: Instance,
    layout: Layout,
    clis: Arc<CliManager>,
    run: Arc<dyn Runner>,
    machine: Mutex<Machine>,
    live: Mutex<Live>,
    child: Mutex<Option<Child>>,
    gen: AtomicU64,
    health: Mutex<Health>,
    alerts: Mutex<Vec<Alert>>,
    disk_alerted: AtomicBool,
    /// An outage was already announced (cleared when the node is Ready again or the cluster is started by hand).
    outage: AtomicBool,
    audit: Mutex<Option<AuditFn>>,
    sandbox_up: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
    over: Mutex<Overrides>,
    /// "waiting for the network..." / "no network yet..." while the start waits for a default route; empty otherwise.
    net_note: Mutex<String>,
    /// Only one start at a time (autostart, the start request of a quick-install job and the Settings button can meet at boot).
    start_lock: Mutex<()>,
    /// The supervisor's last sign of life.
    beat_at: Mutex<Instant>,
    /// The address of the default route's interface when k3s was last started (an exit after it changed is a move, not a crash).
    spawn_ip: Mutex<Option<String>>,
    /// When k3s was last started: the monotonic time and the length of the log then (to look at its first minutes only).
    spawn_mark: Mutex<Option<(Duration, u64)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl K3sDev {
    pub fn new(data_dir: &Path, clis: Arc<CliManager>, run: Arc<dyn Runner>) -> Arc<K3sDev> {
        K3sDev::new_instance(data_dir, clis, run, Instance::primary())
    }

    /// One managed local cluster: the primary (`Instance::primary()`, what `new` makes) or an extra one. Every instance has the
    /// same start gate (clock, network, memory), watchdog and health checks; only the folder, ports, names and ranges differ.
    pub fn new_instance(data_dir: &Path, clis: Arc<CliManager>, run: Arc<dyn Runner>, inst: Instance) -> Arc<K3sDev> {
        let layout = Layout::for_instance(data_dir, &inst);
        let installed = load_state(&layout).is_some();
        let d = Arc::new(K3sDev {
            inst,
            layout,
            clis,
            run,
            machine: Mutex::new(Machine::real()),
            live: Mutex::new(Live { svc: if installed { Svc::Stopped } else { Svc::NotInstalled }, progress: Progress::default() }),
            child: Mutex::new(None),
            gen: AtomicU64::new(0),
            health: Mutex::new(Health::default()),
            alerts: Mutex::new(Vec::new()),
            disk_alerted: AtomicBool::new(false),
            outage: AtomicBool::new(false),
            audit: Mutex::new(None),
            sandbox_up: Mutex::new(None),
            over: Mutex::new(Overrides::default()),
            net_note: Mutex::new(String::new()),
            start_lock: Mutex::new(()),
            beat_at: Mutex::new(Instant::now()),
            spawn_ip: Mutex::new(None),
            spawn_mark: Mutex::new(None),
        });
        if installed && d.inst.primary {
            crate::devcluster::register_machine_kubeconfig(Some(d.layout.kubeconfig()));
        }
        d
    }

    pub fn instance(&self) -> &Instance {
        &self.inst
    }

    pub fn id(&self) -> &str {
        &self.inst.id
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn set_audit(&self, f: AuditFn) {
        *lock(&self.audit) = Some(f);
    }

    /// An extra cluster uses the same audit log and sandbox probe as the first one (call after both are set).
    pub fn inherit_hooks(&self, from: &K3sDev) {
        if let Some(f) = lock(&from.audit).clone() {
            *lock(&self.audit) = Some(f);
        }
        if let Some(f) = lock(&from.sandbox_up).clone() {
            *lock(&self.sandbox_up) = Some(f);
        }
    }

    pub fn set_sandbox_probe(&self, f: Arc<dyn Fn() -> bool + Send + Sync>) {
        *lock(&self.sandbox_up) = Some(f);
    }

    /// For tests: pretend to be (or not to be) root, to have cgroup v2, a RAM root, a desktop window; use other places for the
    /// machine; use short back-offs.
    #[doc(hidden)]
    pub fn assume(&self, root: bool, cgroups: bool, ramfs: bool, window: bool, machine: Machine, backoff: Duration, poll: Duration) {
        let mut o = lock(&self.over);
        o.root = Some(root);
        o.cgroups = Some(cgroups);
        o.ramfs = Some(ramfs);
        o.window = Some(window);
        o.backoff = Some(backoff);
        o.poll = Some(poll);
        o.quiet = true;
        drop(o);
        *lock(&self.machine) = machine;
    }

    /// Tests: the cluster some workspace is connected to (`None`: none).
    #[doc(hidden)]
    pub fn assume_connected(&self, c: Option<&str>) {
        lock(&self.over).connected = Some(c.map(str::to_string));
    }

    /// Tests: read this file as /proc/net/route, and say "no network yet" after `grace`.
    #[doc(hidden)]
    pub fn assume_route_file(&self, f: &Path, grace: Duration) {
        let mut o = lock(&self.over);
        o.route_file = Some(f.to_path_buf());
        o.net_grace = Some(grace);
    }

    /// Is there a default route? Tests that did not give a route file are not about the network: yes.
    fn has_route(&self) -> bool {
        let (f, quiet) = {
            let o = lock(&self.over);
            (o.route_file.clone(), o.quiet)
        };
        match f {
            Some(f) => has_default_route(&std::fs::read_to_string(f).unwrap_or_default()),
            None if quiet => true,
            None => has_default_route(&std::fs::read_to_string(ROUTE_FILE).unwrap_or_default()),
        }
    }

    /// Tests: pretend the default route's interface has this address (`None`: none).
    #[doc(hidden)]
    pub fn assume_ip(&self, ip: Option<&str>) {
        lock(&self.over).ip = Some(ip.map(str::to_string));
    }

    /// Tests: the supervisor counts as stalled after this long without a heartbeat.
    #[doc(hidden)]
    pub fn assume_stall_after(&self, d: Duration) {
        lock(&self.over).stall_after = Some(d);
    }

    /// Tests: the clock the gate and the step detection use.
    #[doc(hidden)]
    pub fn assume_clock(&self, c: Arc<dyn ClockSource>) {
        lock(&self.over).clock = Some(c);
    }

    /// The clock to look at: a test's, the real one, or none (tests that are not about the clock: the gate is open).
    fn clock_src(&self) -> Option<Arc<dyn ClockSource>> {
        let o = lock(&self.over);
        match &o.clock {
            Some(c) => Some(c.clone()),
            None if o.quiet => None,
            None => Some(Arc::new(RealClock)),
        }
    }

    fn clock_ready_now(&self) -> bool {
        self.clock_src().is_none_or(|c| clock_ready(c.floor_pending(), c.synced(), c.mono(), CLOCK_WAIT_LIMIT))
    }

    /// Waits (cancellable by Stop) until the clock is plausible. False: cancelled.
    fn await_clock(&self, gen: u64) -> bool {
        if self.clock_ready_now() {
            return true;
        }
        let step = lock(&self.over).poll.unwrap_or(NET_POLL).min(Duration::from_secs(1));
        self.audit("system", "devcluster.waiting-for-clock", "");
        self.set_net_note(WAITING_CLOCK);
        self.log_line(WAITING_CLOCK);
        while !self.clock_ready_now() {
            self.beat();
            if !self.pause(gen, step) {
                self.set_net_note("");
                return false;
            }
        }
        self.set_net_note("");
        let why = if self.clock_src().is_some_and(|c| c.synced()) { "a time server answered" } else { "no time server answered; the saved time is good enough" };
        self.log_line(&format!("the clock is plausible ({why}), starting k3s"));
        self.current(gen)
    }

    /// Stop the running k3s (its process group) and start it again. Not a failure: nothing is counted or alerted.
    fn restart_native_for_clock(&self, backend: Backend, action: &str, why: &str) -> Result<(), String> {
        self.audit("system", action, why);
        self.log_line(&format!("restarting k3s: {why}"));
        self.set_svc(Svc::Starting);
        let child = lock(&self.child).take();
        if let Some(mut child) = child {
            // SAFETY: signalling the process group we created with `process_group(0)`.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
            let t = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(None) if t.elapsed() <= Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(30)),
                    Ok(None) => {
                        // SAFETY: as above.
                        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => break,
                }
            }
        }
        let _ = std::fs::remove_file(self.layout.kubeconfig());
        self.spawn_backend(backend)
    }

    fn default_ip(&self) -> Option<String> {
        if let Some(ip) = lock(&self.over).ip.clone() {
            return ip;
        }
        let f = lock(&self.over).route_file.clone();
        let text = std::fs::read_to_string(f.unwrap_or_else(|| ROUTE_FILE.into())).unwrap_or_default();
        iface_ipv4(&default_route_iface(&text)?)
    }

    fn beat(&self) {
        *lock(&self.beat_at) = Instant::now();
    }

    fn set_net_note(&self, t: &str) {
        *lock(&self.net_note) = t.to_string();
    }

    fn log_line(&self, t: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.layout.log()) {
            let _ = writeln!(f, "iyakki: {t}");
        }
    }

    /// Waits (cancellable by Stop) for a default route; the status line and the log say so. False: cancelled.
    fn await_network(&self, gen: u64) -> bool {
        if self.has_route() {
            return true;
        }
        let grace = lock(&self.over).net_grace.unwrap_or(NET_GRACE);
        let step = lock(&self.over).poll.unwrap_or(NET_POLL).min(NET_POLL);
        self.audit("system", "devcluster.waiting-for-network", "");
        let r = wait_for_route(
            || self.has_route(),
            |d| {
                self.beat();
                self.pause(gen, d)
            },
            |n| {
                let t = if n == NetNote::Waiting { WAITING_NET } else { NO_NET_YET };
                self.set_net_note(t);
                self.log_line(t);
            },
            grace,
            step,
        );
        self.set_net_note("");
        if r == NetWait::Ready {
            self.log_line("a default route exists, starting k3s");
        }
        r == NetWait::Ready && self.current(gen)
    }

    fn quiet(&self) -> bool {
        lock(&self.over).quiet
    }

    fn audit(&self, user: &str, action: &str, detail: &str) {
        if let Some(f) = lock(&self.audit).clone() {
            f(user, action, detail);
        }
    }

    fn window(&self) -> bool {
        lock(&self.over).window.unwrap_or_else(crate::webengine::host_session)
    }

    fn ramfs(&self) -> bool {
        lock(&self.over).ramfs.unwrap_or_else(crate::cluster::ramfs_root_now)
    }

    pub fn installed(&self) -> Option<State> {
        load_state(&self.layout)
    }

    pub fn state(&self) -> Svc {
        let s = lock(&self.live).svc.clone();
        // installed by another process (or by hand) since this one started
        if s == Svc::NotInstalled && load_state(&self.layout).is_some() {
            lock(&self.live).svc = Svc::Stopped;
            return Svc::Stopped;
        }
        s
    }

    fn set_svc(&self, s: Svc) {
        let up = matches!(s, Svc::Starting | Svc::Running | Svc::Stopping);
        if !self.quiet() {
            {
                let mut ids = running_ids();
                ids.retain(|i| *i != self.inst.id);
                if up {
                    ids.push(self.inst.id.clone());
                }
            }
            if !self.inst.primary {
                // only the primary cluster is "the connected cluster" of the old single-cluster logic
            } else if s == Svc::Running {
                if let Some(desc) = self.installed().map(|_| format!("k3s (kubeconfig {})", self.layout.kubeconfig().display())) {
                    crate::devcluster::set_active(Some(&desc));
                }
            } else if !up {
                crate::devcluster::set_active(None);
            }
        }
        lock(&self.live).svc = s;
    }

    pub fn progress(&self) -> Progress {
        lock(&self.live).progress.clone()
    }

    fn set_progress(&self, phase: &str, done: u64, total: u64) {
        lock(&self.live).progress = Progress { phase: phase.into(), done, total };
    }

    /// Why a cluster cannot be set up or started here, for a desktop window, or "already exists". Empty: fine.
    pub fn found_elsewhere(&self) -> Vec<String> {
        let mut m = lock(&self.machine).clone();
        // another managed local cluster of this process running is not "a cluster found elsewhere": its processes are ours
        let siblings = running_ids().iter().any(|i| *i != self.inst.id);
        if siblings || !self.inst.primary {
            m.proc_dir = PathBuf::from("/nonexistent/iyakki-no-proc");
        }
        let mut ignore = vec![self.layout.root.clone(), self.clis.bin_dir()];
        if let Some(d) = self.clis.bin_dir().parent() {
            ignore.push(d.join("pkg"));
        }
        let sandbox = lock(&self.sandbox_up).clone().is_some_and(|f| f());
        let own_running = managed_running();
        let connected = match lock(&self.over).connected.clone() {
            Some(c) => c,
            None if !self.inst.primary => None,
            None => crate::devcluster::active().filter(|_| !own_running),
        };
        existing(&m, &ignore, connected.as_deref(), sandbox)
    }

    /// The plan, as text, with this machine's facts (root or not, RAM root or not).
    pub fn describe(&self, p: &Plan) -> String {
        let root = !self.requirements().iter().any(|r| r.contains("not running as root"));
        plan_text(p, root, self.ramfs())
    }

    /// What the Dev cluster page should offer on this machine.
    pub fn situation(&self) -> Situation {
        if self.installed().is_some() {
            return Situation::Managed;
        }
        let found = self.found_elsewhere();
        if self.window() {
            return Situation::Window { found };
        }
        if !found.is_empty() {
            return Situation::Foreign(found);
        }
        Situation::Free { problems: self.requirements() }
    }

    /// What is missing for the cluster to run here (root, cgroup v2, a persistent data disk with room).
    pub fn requirements(&self) -> Vec<String> {
        let (o_root, o_cg) = {
            let o = lock(&self.over);
            (o.root, o.cgroups)
        };
        let mut v = Vec::new();
        // SAFETY: geteuid has no preconditions.
        if !o_root.unwrap_or_else(|| unsafe { libc::geteuid() } == 0) {
            v.push("IYAKKI is not running as root here (this is a desktop test window, not the machine itself), so k3s cannot start".to_string());
        }
        if !o_cg.unwrap_or_else(|| Path::new("/sys/fs/cgroup/cgroup.controllers").exists()) {
            v.push("the kernel needs cgroup v2 (/sys/fs/cgroup/cgroup.controllers is missing)".to_string());
        }
        let parent = self.layout.root.parent().unwrap_or(Path::new("/")).to_path_buf();
        if lock(&self.over).root.is_none() {
            if let Some(t) = fs_type_of(&parent, &std::fs::read_to_string("/proc/mounts").unwrap_or_default()) {
                if matches!(t.as_str(), "tmpfs" | "ramfs" | "rootfs") {
                    v.push(format!("the data folder {} is in memory ({t}), so a cluster would fill the RAM and be lost at every restart; make a data disk first (Settings \u{2192} Storage)", parent.display()));
                }
            }
            if let Some(free) = free_bytes(&parent) {
                if free < MIN_FREE {
                    v.push(format!("only {} is free on the data partition; the cluster needs at least {}", human(free), human(MIN_FREE)));
                }
            }
        }
        v
    }

    // -------------------------------------------------------------------------- install

    /// Do the approved plan: download and verify (native) or pull and create (Podman), write the state. Does not start the cluster.
    /// Everything is checked again here, not only when the plan was shown.
    pub fn install(&self, plan: &Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String> {
        if self.window() {
            return Err("In a desktop window IYAKKI never installs a cluster: this machine manages its own. Install k3s yourself if you want one, or connect a remote cluster.".into());
        }
        if self.installed().is_some() {
            return Err("IYAKKI already manages a cluster here (Settings \u{2192} Dev cluster). Use Upgrade, Reset or Uninstall.".into());
        }
        let found = self.found_elsewhere();
        if !found.is_empty() {
            self.audit(user, "devcluster.setup.refused", &found.join("; "));
            return Err(refusal_text(&found));
        }
        let missing = self.requirements();
        if !missing.is_empty() {
            self.audit(user, "devcluster.setup.refused", &missing.join("; "));
            return Err(format!("The cluster cannot be set up here: {}.", missing.join("; ")));
        }
        self.audit(user, "devcluster.setup", &format!("{} {} registry={}", plan.backend.label(), plan.version, plan.registry));
        let l = &self.layout;
        for d in [&l.root, &l.tmp(), &l.root.join("bin")] {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let mut state = State { backend: plan.backend, version: plan.version.clone(), sha256: plan.expected_sha.clone(), url: plan.url.clone(), installed_at: now(), installed_by: user.into(), autostart: true, registry_choice: plan.registry.clone(), image: plan.image.clone(), ..Default::default() };
        let r = match plan.backend {
            Backend::Native => self.fetch_binary(plan, st, user).and_then(|new| self.commit_binary(&new)),
            Backend::Podman => self.pull_image(plan, user).map(|digest| state.image_digest = digest),
        };
        if let Err(e) = r {
            let _ = std::fs::remove_dir_all(l.tmp());
            // nothing was installed: remove the folders this attempt made when they hold nothing else
            let _ = std::fs::remove_dir(l.root.join("bin"));
            let _ = std::fs::remove_dir(l.root.join("tmp"));
            let _ = std::fs::remove_dir(&l.root);
            self.audit(user, "devcluster.setup.failed", &e);
            return Err(e);
        }
        std::fs::create_dir_all(l.data()).map_err(io_err)?;
        save_state(l, &state)?;
        if !self.quiet() && self.inst.primary {
            crate::devcluster::register_machine_kubeconfig(Some(l.kubeconfig()));
        }
        self.set_svc(Svc::Stopped);
        self.set_progress("installed", 0, 0);
        self.audit(user, "devcluster.installed", &format!("{} {} sha256 {}", plan.backend.label(), plan.version, plan.expected_sha));
        Ok(())
    }

    /// Download the program to a temp file next to the final place, hashing as it arrives; compare with the vendor's checksum; only then
    /// make it executable. Returns the verified file (not yet in place).
    fn fetch_binary(&self, plan: &Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<PathBuf, String> {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        let d = clis::def("k3s").ok_or("k3s is not in the catalogue")?;
        self.set_progress("downloading", 0, 0);
        self.audit(user, "devcluster.download", &plan.url);
        let (head, _) = self.clis.open_follow(st, &plan.url, d.hosts)?;
        let total = head.header("content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if total > clis::MAX_DOWNLOAD {
            return Err(format!("the download is {} MB, more than the {} MB allowed", total >> 20, clis::MAX_DOWNLOAD >> 20));
        }
        self.set_progress("downloading", 0, total);
        std::fs::create_dir_all(self.layout.tmp()).map_err(io_err)?;
        std::fs::create_dir_all(self.layout.root.join("bin")).map_err(io_err)?;
        let tmp = self.layout.tmp().join("k3s.download");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(io_err)?);
        let (mut hasher, mut body, mut got) = (Sha256::new(), head.body, 0u64);
        let mut buf = vec![0u8; 128 * 1024];
        let read = (|| -> Result<(), String> {
            loop {
                let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
                if n == 0 {
                    break;
                }
                got += n as u64;
                if got > clis::MAX_DOWNLOAD {
                    return Err("the download is larger than allowed".into());
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(io_err)?;
                self.set_progress("downloading", got, total);
            }
            out.flush().map_err(io_err)
        })();
        if let Err(e) = read {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        drop(out);
        if total > 0 && got != total {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("the download stopped at {} of {} MB", got >> 20, total >> 20));
        }
        self.set_progress("checking the checksum", got, total);
        let have: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let want = if plan.expected_sha.is_empty() {
            let text = self.clis.small(st, &plan.checksum_url, d.hosts)?;
            clis::extract_hash(&text, &plan.file).ok_or("the vendor's checksum file has no entry for this download")?
        } else {
            plan.expected_sha.clone()
        };
        if want != have {
            let _ = std::fs::remove_file(&tmp);
            self.audit(user, "devcluster.checksum.mismatch", &format!("expected {want} got {have}"));
            return Err(format!("the download does not match the vendor's checksum (expected {}\u{2026}, got {}\u{2026}); it was thrown away and nothing was installed", &want[..12.min(want.len())], &have[..12]));
        }
        let new = self.layout.root.join("bin/k3s.new");
        let _ = std::fs::remove_file(&new);
        std::fs::rename(&tmp, &new).map_err(io_err)?;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(io_err)?;
        self.audit(user, "devcluster.verified", &format!("sha256 {have}"));
        Ok(new)
    }

    /// Put the verified program in place; the old one stays as `k3s.prev` until the next replacement.
    fn commit_binary(&self, new: &Path) -> Result<(), String> {
        let bin = self.layout.bin();
        if bin.exists() {
            let _ = std::fs::remove_file(self.layout.root.join("bin/k3s.prev"));
            std::fs::rename(&bin, self.layout.root.join("bin/k3s.prev")).map_err(io_err)?;
        }
        std::fs::rename(new, &bin).map_err(io_err)
    }

    fn pull_image(&self, plan: &Plan, user: &str) -> Result<String, String> {
        if !self.run.has("podman") {
            return Err("podman is not installed on this machine, so the Podman variant is not available. The image needs the podman packages; use the native variant.".into());
        }
        self.set_progress("pulling the image with podman", 0, 0);
        self.audit(user, "devcluster.pull", &plan.image);
        let tmp = std::env::temp_dir();
        let o = self.run.run(&tmp, &plan.pull, &[], Duration::from_secs(1800), 64 * 1024, None);
        if !o.ok() {
            return Err(format!("podman pull failed: {}", o.combined().lines().last().unwrap_or("no message").trim()));
        }
        let dg = self.run.run(&tmp, &["podman".into(), "image".into(), "inspect".into(), "--format".into(), "{{.Digest}}".into(), plan.image.clone()], &[], Duration::from_secs(30), 4096, None);
        let digest = if dg.ok() { dg.stdout.trim().to_string() } else { String::new() };
        let _ = self.run.run(&tmp, &["podman".into(), "rm".into(), "-f".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(60), 4096, None);
        let c = self.run.run(&tmp, &plan.create, &[], Duration::from_secs(120), 16 * 1024, None);
        if !c.ok() {
            return Err(format!("podman create failed: {}", c.combined().lines().last().unwrap_or("no message").trim()));
        }
        self.audit(user, "devcluster.image", &format!("{} digest {}", plan.image, digest));
        Ok(digest)
    }

    // -------------------------------------------------------------------------- start, stop, restart

    /// Start the cluster (installed by IYAKKI). Refuses in a desktop window, when another cluster exists, or when it already runs.
    pub fn start(self: &Arc<Self>, user: &str) -> Result<(), String> {
        let _one_at_a_time = lock(&self.start_lock);
        let st = self.installed().ok_or("no cluster is installed by IYAKKI here (Settings \u{2192} Dev cluster \u{2192} Set up a local cluster)")?;
        if self.window() {
            return Err("In a desktop window IYAKKI never starts a cluster: this machine manages its own.".into());
        }
        if matches!(self.state(), Svc::Starting | Svc::Running | Svc::Stopping) {
            return Err("the cluster is already running".into());
        }
        let found = self.found_elsewhere();
        if !found.is_empty() {
            self.audit(user, "devcluster.start.refused", &found.join("; "));
            return Err(refusal_text(&found));
        }
        let missing = self.requirements();
        if !missing.is_empty() {
            return Err(format!("The cluster cannot start here: {}.", missing.join("; ")));
        }
        self.audit(user, "devcluster.start", st.backend.label());
        self.outage.store(false, Ordering::SeqCst);
        std::fs::create_dir_all(self.layout.data()).map_err(io_err)?;
        let _ = std::fs::remove_file(self.layout.kubeconfig());
        // no default route yet (after a boot DHCP may still be working): the supervisor waits for it, then starts k3s
        let pending = !self.has_route() || !self.clock_ready_now();
        if !pending {
            *lock(&self.spawn_ip) = self.default_ip();
            match st.backend {
                Backend::Native => {
                    let child = self.spawn_native()?;
                    *lock(&self.child) = Some(child);
                }
                Backend::Podman => self.start_pod(&st)?,
            }
        } else if st.backend == Backend::Native && !self.layout.bin().is_file() {
            return Err(format!("{} is missing; reinstall (Uninstall, then Set up)", self.layout.bin().display()));
        }
        self.set_svc(Svc::Starting);
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.clone();
        self.beat();
        let w = self.clone();
        std::thread::Builder::new().name("iyakki-k3s-dev".into()).spawn(move || me.monitor(gen, st.backend, pending)).map_err(io_err)?;
        let _ = std::thread::Builder::new().name("iyakki-k3s-watch".into()).spawn(move || w.watchdog(gen, st.backend));
        Ok(())
    }

    fn spawn_native(&self) -> Result<Child, String> {
        use std::os::unix::process::CommandExt;
        let l = &self.layout;
        if !l.bin().is_file() {
            return Err(format!("{} is missing; reinstall (Uninstall, then Set up)", l.bin().display()));
        }
        let conf_d = l.data().join("agent/etc/kubelet.conf.d");
        let ramfs = self.ramfs();
        if ramfs {
            std::fs::create_dir_all(&conf_d).map_err(io_err)?;
            std::fs::write(conf_d.join("10-iyakki.conf"), "apiVersion: kubelet.config.k8s.io/v1beta1\nkind: KubeletConfiguration\nlocalStorageCapacityIsolation: false\n").map_err(io_err)?;
        } else {
            let _ = std::fs::remove_file(conf_d.join("10-iyakki.conf"));
        }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(l.log()).map_err(io_err)?;
        *lock(&self.spawn_mark) = self.clock_src().map(|c| (c.mono(), log.metadata().map(|m| m.len()).unwrap_or(0)));
        let log2 = log.try_clone().map_err(io_err)?;
        let mut cmd = Command::new(l.bin());
        cmd.args(server_args_for(l, &self.inst))
            .env_clear()
            .env("PATH", format!("{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", l.root.join("bin").display()))
            .env("HOME", &l.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2))
            .process_group(0);
        if ramfs && lock(&self.over).ramfs.is_none() {
            crate::cluster::start_in_ram_root_jail(&mut cmd)?;
        }
        // "text file busy" happens when another thread forked while the program file was still open for writing (just after a
        // download or an upgrade); it clears within moments
        let mut last = String::new();
        for _ in 0..40 {
            match cmd.spawn() {
                Ok(c) => return Ok(c),
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    last = e.to_string();
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("could not start k3s: {e}")),
            }
        }
        Err(format!("could not start k3s: {last}"))
    }

    fn start_pod(&self, st: &State) -> Result<(), String> {
        let tmp = std::env::temp_dir();
        if !self.run.has("podman") {
            return Err("podman is not installed, so the Podman variant cannot start. Uninstall and set up the native variant.".into());
        }
        let exists = self.run.run(&tmp, &["podman".into(), "inspect".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(20), 4096, None).ok();
        if !exists {
            let create = podman_create(&self.layout, &st.image, &self.inst);
            let c = self.run.run(&tmp, &create, &[], Duration::from_secs(120), 16 * 1024, None);
            if !c.ok() {
                return Err(format!("podman create failed: {}", c.combined().lines().last().unwrap_or("no message").trim()));
            }
        }
        let o = self.run.run(&tmp, &["podman".into(), "start".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(120), 16 * 1024, None);
        if o.ok() { Ok(()) } else { Err(format!("podman start failed: {}", o.combined().lines().last().unwrap_or("no message").trim())) }
    }

    fn current(&self, gen: u64) -> bool {
        self.gen.load(Ordering::SeqCst) == gen
    }

    fn pause(&self, gen: u64, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if !self.current(gen) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20).min(d));
        }
        self.current(gen)
    }

    fn poll_every(&self, running: bool) -> Duration {
        lock(&self.over).poll.unwrap_or(Duration::from_secs(if running { 10 } else { 2 }))
    }

    fn backoff(&self, n: usize) -> Duration {
        lock(&self.over).backoff.unwrap_or(Duration::from_secs((5u64 << n.min(4)).min(60)))
    }

    /// Watches the process (or container): the node becoming Ready, an unexpected exit (restart with a back-off, or give up), disk use.
    fn spawn_backend(&self, backend: Backend) -> Result<(), String> {
        *lock(&self.spawn_ip) = self.default_ip();
        match backend {
            Backend::Native => self.spawn_native().map(|c| *lock(&self.child) = Some(c)),
            Backend::Podman => self.installed().ok_or_else(|| "state is gone".to_string()).and_then(|s| self.start_pod(&s)),
        }
    }

    /// The supervisor. Every way out is loud: audited as `devcluster.supervisor-stopped` with the reason, and the state never stays
    /// Starting/Running without a supervisor (a panic counts too).
    fn monitor(self: Arc<Self>, gen: u64, backend: Backend, pending: bool) {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.monitor_inner(gen, backend, pending)));
        let reason = match r {
            Ok(r) => r,
            Err(e) => {
                let m = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
                format!("the supervisor panicked: {m}")
            }
        };
        self.supervisor_ended(gen, reason);
    }

    fn supervisor_ended(&self, gen: u64, reason: String) {
        self.audit("system", "devcluster.supervisor-stopped", &reason);
        self.log_line(&format!("supervisor stopped: {reason}"));
        if self.current(gen) && matches!(self.state(), Svc::Starting | Svc::Running) {
            let why = format!("IYAKKI stopped watching k3s ({reason})");
            self.set_svc(Svc::Failed(why.clone()));
            self.alerts_push(Alert::GaveUp(why));
        }
    }

    /// Tests: run the supervisor wrapper around a body that panics.
    #[doc(hidden)]
    pub fn monitor_panic_for_test(self: &Arc<Self>, gen: u64) {
        let r = std::panic::catch_unwind(|| -> String { panic!("test panic") });
        let reason = format!("the supervisor panicked: {}", r.err().and_then(|e| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default());
        self.supervisor_ended(gen, reason);
    }

    /// Looks at the supervisor from outside: when it has not shown a sign of life for a long time (blocked, or gone) while the cluster
    /// is meant to run, it takes over: the exit of k3s is recorded and k3s is started again, or a new supervisor replaces the old one.
    fn watchdog(self: Arc<Self>, gen: u64, backend: Backend) {
        while self.current(gen) {
            let every = lock(&self.over).poll.unwrap_or(Duration::from_secs(5)).max(Duration::from_millis(20));
            if !self.pause(gen, every) {
                return;
            }
            let stale = lock(&self.over).stall_after.unwrap_or(STALL_AFTER);
            if self.watchdog_step(gen, backend, stale) {
                return;
            }
        }
    }

    /// One look of the watchdog; true: it took over (this watchdog is done, a new supervisor and watchdog run).
    #[doc(hidden)]
    pub fn watchdog_step(self: &Arc<Self>, gen: u64, backend: Backend, stale: Duration) -> bool {
        if !self.current(gen) || !matches!(self.state(), Svc::Starting | Svc::Running) || lock(&self.beat_at).elapsed() < stale {
            return false;
        }
        let mut gone: Option<String> = None;
        if backend == Backend::Native {
            let mut c = lock(&self.child);
            match c.as_mut() {
                None => gone = Some("the handle of the k3s process was lost".to_string()),
                Some(ch) => {
                    if let Ok(Some(st)) = ch.try_wait() {
                        gone = Some(format!("k3s exited ({})", exit_text(&st)));
                        *c = None;
                    }
                }
            }
        }
        // retire the stalled supervisor (a Stop in the meantime wins)
        if self.gen.compare_exchange(gen, gen + 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return false;
        }
        let ngen = gen + 1;
        self.audit("system", "devcluster.supervisor-stalled", gone.as_deref().unwrap_or("k3s still runs; a new supervisor takes over"));
        if let Some(reason) = gone {
            self.log_line(&reason);
            self.audit("system", "devcluster.exited", &format!("{reason} (found by the watchdog)"));
            self.set_svc(Svc::Starting);
            match self.spawn_backend(backend) {
                Ok(()) => {
                    self.note_restart(&reason, true);
                    self.audit("system", "devcluster.restart", &reason);
                }
                Err(e) => {
                    self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                    self.alerts_push(Alert::GaveUp(format!("{reason}; restarting failed: {e}")));
                    return true;
                }
            }
        }
        self.beat();
        let (me, w) = (self.clone(), self.clone());
        let _ = std::thread::Builder::new().name("iyakki-k3s-dev".into()).spawn(move || me.monitor(ngen, backend, false));
        let _ = std::thread::Builder::new().name("iyakki-k3s-watch".into()).spawn(move || w.watchdog(ngen, backend));
        true
    }

    fn monitor_inner(self: &Arc<Self>, gen: u64, backend: Backend, pending: bool) -> String {
        if pending {
            if !self.await_network(gen) {
                return "stopped while waiting for the network".into();
            }
            if !self.await_clock(gen) {
                return "stopped while waiting for the clock".into();
            }
            if let Err(e) = self.spawn_backend(backend) {
                self.set_svc(Svc::Failed(e));
                return "could not start k3s".into();
            }
        }
        let clock = if backend == Backend::Native { self.clock_src() } else { None };
        let mut last_look = clock.as_ref().map(|c| (c.wall(), c.mono()));
        let mut clock_restarts = 0usize;
        let mut last_step_mono: Option<Duration> = None;
        let mut cert_restarted = false;
        let mut began = Instant::now();
        let mut failures: Vec<Instant> = Vec::new();
        let mut last_disk = Instant::now() - Duration::from_secs(3600);
        let mut chmodded = false;
        while self.current(gen) {
            self.beat();
            // 0. did the clock jump (the network time arrived)? Then k3s, started at the fake date, is started again once.
            let mut stepped: Option<i64> = None;
            if let Some(c) = &clock {
                let (w, m) = (c.wall(), c.mono());
                if let Some((pw, pm)) = last_look {
                    stepped = clock_stepped(pw, pm, w, m, CLOCK_STEP_SECS);
                }
                last_look = Some((w, m));
                if stepped.is_some() {
                    last_step_mono = Some(m);
                }
            }
            if let (Some(jump), true) = (stepped, clock_restarts < MAX_CLOCK_RESTARTS) {
                if lock(&self.child).as_mut().is_some_and(|ch| matches!(ch.try_wait(), Ok(None))) {
                    clock_restarts += 1;
                    if let Err(e) = self.restart_native_for_clock(backend, "devcluster.restart-clock-step", &format!("the clock moved by {jump} s")) {
                        self.set_svc(Svc::Failed(format!("clock step; restarting failed: {e}")));
                        return "restarting failed".into();
                    }
                    began = Instant::now();
                    last_look = clock.as_ref().map(|c| (c.wall(), c.mono()));
                    continue;
                }
            }
            // 0b. certificate trouble in the first minutes after a start at the wrong date, now that the clock is plausible
            if let (Some(c), false) = (&clock, cert_restarted) {
                let mark = *lock(&self.spawn_mark);
                if let Some((m0, off)) = mark {
                    if c.mono().saturating_sub(m0) <= CERT_WINDOW && self.clock_ready_now() && log_shows_clock_trouble(&read_from(&self.layout.log(), off, 256 * 1024)) {
                        cert_restarted = true;
                        if let Err(e) = self.restart_native_for_clock(backend, "devcluster.restart-clock-cert", "k3s reported a certificate that is not valid yet") {
                            self.set_svc(Svc::Failed(format!("certificate trouble; restarting failed: {e}")));
                            return "restarting failed".into();
                        }
                        began = Instant::now();
                        last_look = Some((c.wall(), c.mono()));
                        continue;
                    }
                }
            }
            // 1. has it died?
            let died: Option<String> = match backend {
                Backend::Native => {
                    let mut c = lock(&self.child);
                    match c.as_mut() {
                        Some(ch) => match ch.try_wait() {
                            Ok(Some(status)) => {
                                *c = None;
                                Some(format!("k3s exited ({}); the end of its log: {}", exit_text(&status), self.log_tail_text(3).replace('\n', " | ")))
                            }
                            _ => None,
                        },
                        None => Some("the handle of the k3s process was lost (it is not tracked any more)".to_string()),
                    }
                }
                Backend::Podman => {
                    let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "inspect".into(), "--format".into(), "{{.State.Status}} {{.State.ExitCode}}".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(20), 4096, None);
                    let t = o.stdout.trim().to_string();
                    if o.ok() && !t.starts_with("running") && !t.starts_with("created") { Some(format!("the container is {t}; the end of its log: {}", self.log_tail_text(3).replace('\n', " | "))) } else { None }
                }
            };
            if let Some(reason) = died {
                if !self.current(gen) {
                    return "superseded by a newer start or stopped".into();
                }
                self.log_line(reason.split("; the end of its log").next().unwrap_or(&reason));
                // the machine moved to another network (the stick travels): k3s ends itself when its node address changes. That is
                // expected: no failure, no alert, start it again at once so the node takes the new address.
                let (then, now_ip) = (lock(&self.spawn_ip).clone(), self.default_ip());
                if self.has_route() && then.is_some() && now_ip.is_some() && then != now_ip {
                    self.audit("system", "devcluster.exited-ip-changed", &format!("{reason}; address {} -> {}", then.unwrap_or_default(), now_ip.unwrap_or_default()));
                    self.set_svc(Svc::Starting);
                    if let Err(e) = self.spawn_backend(backend) {
                        self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                        return "restarting failed".into();
                    }
                    began = Instant::now();
                    continue;
                }
                // it ended right after the clock jumped: the jump is the cause, not a bug. Start it again, no failure counted.
                if let (Some(c), Some(at)) = (&clock, last_step_mono) {
                    if c.mono().saturating_sub(at) <= CERT_WINDOW && clock_restarts < MAX_CLOCK_RESTARTS {
                        clock_restarts += 1;
                        self.audit("system", "devcluster.restart-clock-step", &format!("{reason}; it ended after the clock moved"));
                        self.set_svc(Svc::Starting);
                        if let Err(e) = self.spawn_backend(backend) {
                            self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                            return "restarting failed".into();
                        }
                        began = Instant::now();
                        last_step_mono = None;
                        last_look = Some((c.wall(), c.mono()));
                        continue;
                    }
                }
                // it died while the machine has no default route (k3s: "no default routes found"): the network is missing, k3s is
                // not broken. Wait for the route and start it again; this is no failure, no alert and no restart is counted.
                if !self.has_route() {
                    self.audit("system", "devcluster.exited-no-network", &reason);
                    self.set_svc(Svc::Starting);
                    if !self.await_network(gen) {
                        return "stopped while waiting for the network".into();
                    }
                    if let Err(e) = self.spawn_backend(backend) {
                        self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                        return "restarting failed".into();
                    }
                    began = Instant::now();
                    continue;
                }
                failures.retain(|t| t.elapsed() < FAIL_WINDOW);
                failures.push(Instant::now());
                if !self.outage.swap(true, Ordering::SeqCst) {
                    self.alerts_push(Alert::Down(reason.clone()));
                }
                self.audit("system", "devcluster.exited", &reason);
                if failures.len() > MAX_FAILURES {
                    let why = format!("{reason}; it failed {MAX_FAILURES} times within {} minutes, so IYAKKI stopped trying", FAIL_WINDOW.as_secs() / 60);
                    self.set_svc(Svc::Failed(why.clone()));
                    self.alerts_push(Alert::GaveUp(why));
                    self.note_restart(&reason, false);
                    return "gave up after repeated failures".into();
                }
                self.set_svc(Svc::Starting);
                if !self.pause(gen, self.backoff(failures.len() - 1)) {
                    return "stopped during the back-off".into();
                }
                match self.spawn_backend(backend) {
                    Ok(()) => {
                        self.note_restart(&reason, true);
                        self.audit("system", "devcluster.restart", &reason);
                    }
                    Err(e) => {
                        self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                        return "restarting failed".into();
                    }
                }
                continue;
            }
            // 2. is the node ready?
            let ready = self.kubeconfig_ready(backend) && self.kubectl(&["get", "nodes", "-o", "jsonpath={.items[*].status.conditions[?(@.type=='Ready')].status}"]).stdout.contains("True");
            if self.current(gen) {
                if ready {
                    if !chmodded {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(self.layout.kubeconfig(), std::fs::Permissions::from_mode(0o600));
                        chmodded = true;
                    }
                    if self.state() == Svc::Starting {
                        self.outage.store(false, Ordering::SeqCst);
                        self.set_svc(Svc::Running);
                        self.audit("system", "devcluster.ready", "");
                    }
                } else if self.state() == Svc::Running {
                    self.set_svc(Svc::Starting);
                }
            }
            // 3. health, disk alert
            if self.state() == Svc::Running && last_disk.elapsed() >= Duration::from_secs(if lock(&self.over).poll.is_some() { 0 } else { 60 }) {
                last_disk = Instant::now();
                let h = self.health_now();
                self.check_disk(&h);
                *lock(&self.health) = h;
            }
            if began.elapsed() > Duration::from_secs(900) && self.state() == Svc::Starting {
                self.set_svc(Svc::Failed("the node did not become Ready within fifteen minutes".into()));
                self.alerts_push(Alert::GaveUp("the node did not become Ready within fifteen minutes".into()));
                return "the node did not become Ready".into();
            }
            if !self.pause(gen, self.poll_every(self.state() == Svc::Running)) {
                return "stopped or superseded".into();
            }
        }
        "superseded by a newer start or stopped".into()
    }

    fn kubeconfig_ready(&self, backend: Backend) -> bool {
        match backend {
            Backend::Native | Backend::Podman => self.layout.kubeconfig().exists(),
        }
    }

    fn alerts_push(&self, a: Alert) {
        let mut v = lock(&self.alerts);
        if !v.contains(&a) {
            v.push(a);
        }
    }

    /// Alerts raised since the last call (the shell puts them in the administrators' INBOX).
    pub fn take_alerts(&self) -> Vec<Alert> {
        std::mem::take(&mut *lock(&self.alerts))
    }

    /// Tests: feed a disk figure to the alert rule.
    #[doc(hidden)]
    pub fn check_disk_for_test(&self, pct: u8) {
        self.check_disk(&Health { disk_pct: Some(pct), ..Default::default() });
    }

    fn check_disk(&self, h: &Health) {
        if let Some(p) = h.disk_pct {
            if p >= DISK_ALERT_PCT && !self.disk_alerted.swap(true, Ordering::SeqCst) {
                self.alerts_push(Alert::Disk(p));
            } else if p < DISK_CLEAR_PCT {
                self.disk_alerted.store(false, Ordering::SeqCst);
            }
        }
    }

    fn note_restart(&self, reason: &str, restarted: bool) {
        if let Some(mut s) = self.installed() {
            if restarted {
                s.restarts += 1;
            }
            s.last_restart_reason = reason.chars().take(400).collect();
            s.last_restart_at = now();
            let _ = save_state(&self.layout, &s);
        }
    }

    /// `kubectl` against the managed cluster: `k3s kubectl` (native) or `podman exec` (Podman). Never needs a kubectl on the machine.
    pub fn kubectl(&self, args: &[&str]) -> Out {
        let backend = self.installed().map(|s| s.backend).unwrap_or_default();
        let kc = self.layout.kubeconfig().display().to_string();
        let mut argv: Vec<String> = match backend {
            Backend::Native => vec![self.layout.bin().display().to_string(), "kubectl".into(), "--kubeconfig".into(), kc.clone()],
            Backend::Podman => vec!["podman".into(), "exec".into(), self.inst.pod_name.clone(), "kubectl".into()],
        };
        argv.push("--request-timeout=10s".into());
        argv.extend(args.iter().map(|s| s.to_string()));
        let env: Vec<(String, String)> = if backend == Backend::Native { vec![("KUBECONFIG".into(), kc)] } else { vec![] };
        self.run.run(&self.layout.root, &argv, &env, Duration::from_secs(20), 256 * 1024, None)
    }

    /// Stop it (SIGTERM to the process group, SIGKILL after 30 s; `podman stop` for the container).
    pub fn stop(&self, user: &str) -> Result<(), String> {
        let st = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        self.audit(user, "devcluster.stop", st.backend.label());
        self.gen.fetch_add(1, Ordering::SeqCst);
        match st.backend {
            Backend::Native => {
                let child = lock(&self.child).take();
                let Some(mut child) = child else {
                    // still waiting for the network (nothing was started yet): Stop cancels the wait
                    let waiting = self.state() == Svc::Starting;
                    self.set_net_note("");
                    self.set_svc(Svc::Stopped);
                    return if waiting { Ok(()) } else { Err("the cluster is not running".into()) };
                };
                self.set_svc(Svc::Stopping);
                // SAFETY: signalling the process group we created with `process_group(0)`.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
                let t = Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if t.elapsed() > Duration::from_secs(30) => {
                            // SAFETY: as above.
                            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(30)),
                        Err(_) => break,
                    }
                }
            }
            Backend::Podman => {
                self.set_svc(Svc::Stopping);
                let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "stop".into(), "-t".into(), "30".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(90), 16 * 1024, None);
                if !o.ok() && !o.combined().contains("no such") {
                    self.set_svc(Svc::Failed(format!("podman stop failed: {}", o.combined().trim())));
                    return Err(format!("podman stop failed: {}", o.combined().trim()));
                }
            }
        }
        let _ = std::fs::remove_file(self.layout.kubeconfig());
        self.set_svc(Svc::Stopped);
        Ok(())
    }

    pub fn restart(self: &Arc<Self>, user: &str) -> Result<(), String> {
        self.audit(user, "devcluster.restart.requested", "");
        if matches!(self.state(), Svc::Starting | Svc::Running | Svc::Failed(_)) {
            let _ = self.stop(user);
        }
        self.start(user)
    }

    /// Delete the cluster's state (the data folder only) so it starts fresh. The person must have typed [`NODE_NAME`].
    pub fn reset(&self, typed: &str, user: &str) -> Result<(), String> {
        if typed.trim() != self.inst.node_name {
            return Err(format!("Reset deletes everything the cluster stores (its objects, volumes and images). Type {} to confirm; nothing was changed.", self.inst.node_name));
        }
        self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        self.audit(user, "devcluster.reset", &self.layout.data().display().to_string());
        if matches!(self.state(), Svc::Starting | Svc::Running | Svc::Stopping | Svc::Failed(_)) {
            let _ = self.stop(user);
        }
        let data = self.layout.data();
        self.remove_managed(&data)?;
        let _ = std::fs::remove_file(self.layout.kubeconfig());
        std::fs::create_dir_all(&data).map_err(io_err)?;
        if let Some(mut s) = self.installed() {
            s.restarts = 0;
            s.last_restart_reason = String::new();
            let _ = save_state(&self.layout, &s);
        }
        self.audit(user, "devcluster.reset.done", "");
        Ok(())
    }

    /// Remove a folder only if it is inside the managed folder, is not a link and the managed state exists.
    fn remove_managed(&self, p: &Path) -> Result<(), String> {
        if !self.layout.state().is_file() || !p.starts_with(&self.layout.root) {
            return Err(format!("{} is not inside the folder IYAKKI manages; nothing was deleted", p.display()));
        }
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a link; nothing was deleted", p.display())),
            Ok(_) => std::fs::remove_dir_all(p).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Stop and remove the cluster IYAKKI installed (program, data, kubeconfig, log and the Podman container). The person typed [`NODE_NAME`].
    pub fn uninstall(&self, typed: &str, user: &str) -> Result<(), String> {
        if typed.trim() != self.inst.node_name {
            return Err(format!("Uninstall removes the cluster and all its data. Type {} to confirm; nothing was changed.", self.inst.node_name));
        }
        let st = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        self.audit(user, "devcluster.uninstall", &self.layout.root.display().to_string());
        if matches!(self.state(), Svc::Starting | Svc::Running | Svc::Stopping | Svc::Failed(_)) {
            let _ = self.stop(user);
        }
        if st.backend == Backend::Podman && self.run.has("podman") {
            let _ = self.run.run(&std::env::temp_dir(), &["podman".into(), "rm".into(), "-f".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(60), 4096, None);
        }
        let root = self.layout.root.clone();
        self.remove_managed(&root)?;
        if !self.quiet() && self.inst.primary {
            crate::devcluster::register_machine_kubeconfig(None);
        }
        self.set_svc(Svc::NotInstalled);
        *lock(&self.health) = Health::default();
        self.audit(user, "devcluster.uninstall.done", "");
        Ok(())
    }

    /// Look for a newer release and, if there is one, download and verify it, stop the cluster, replace the program and start again.
    /// Returns the plan that was used (`None`: already current).
    pub fn upgrade_plan(&self, st: &dyn iyakki_proxy::Streamer) -> Result<Option<Plan>, String> {
        let cur = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        let p = lookup(&self.clis, st, &self.layout, cur.backend, &cur.registry_choice)?;
        Ok(if p.version == cur.version { None } else { Some(p) })
    }

    /// Do an approved upgrade (the plan from [`K3sDev::upgrade_plan`]): verify first, then replace.
    pub fn upgrade(self: &Arc<Self>, plan: &Plan, st: &dyn iyakki_proxy::Streamer, user: &str) -> Result<(), String> {
        let cur = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        let was_running = matches!(self.state(), Svc::Starting | Svc::Running);
        self.audit(user, "devcluster.upgrade", &format!("{} -> {}", cur.version, plan.version));
        let mut next = cur.clone();
        match cur.backend {
            Backend::Native => {
                let new = self.fetch_binary(plan, st, user)?;
                if was_running {
                    self.stop(user)?;
                }
                self.commit_binary(&new)?;
            }
            Backend::Podman => {
                let tmp = std::env::temp_dir();
                self.set_progress("pulling the image with podman", 0, 0);
                let o = self.run.run(&tmp, &plan.pull, &[], Duration::from_secs(1800), 64 * 1024, None);
                if !o.ok() {
                    return Err(format!("podman pull failed: {}", o.combined().lines().last().unwrap_or("no message").trim()));
                }
                if was_running {
                    self.stop(user)?;
                }
                let _ = self.run.run(&tmp, &["podman".into(), "rm".into(), "-f".into(), self.inst.pod_name.clone()], &[], Duration::from_secs(60), 4096, None);
                next.image = plan.image.clone();
            }
        }
        next.version = plan.version.clone();
        next.sha256 = plan.expected_sha.clone();
        next.url = plan.url.clone();
        save_state(&self.layout, &next)?;
        self.audit(user, "devcluster.upgraded", &plan.version);
        if was_running {
            self.start(user)?;
        }
        Ok(())
    }

    pub fn set_autostart(&self, on: bool, user: &str) -> Result<(), String> {
        let mut s = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        s.autostart = on;
        save_state(&self.layout, &s)?;
        self.audit(user, "devcluster.autostart", if on { "on" } else { "off" });
        Ok(())
    }

    pub fn set_registry_choice(&self, choice: &str, user: &str) -> Result<(), String> {
        let mut s = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        if !matches!(choice, "in-cluster" | "system" | "none") {
            return Err(format!("unknown registry option `{choice}`"));
        }
        s.registry_choice = choice.into();
        save_state(&self.layout, &s)?;
        self.audit(user, "devcluster.registry", choice);
        Ok(())
    }

    /// The supervisor's start hook: called once when IYAKKI starts. Starts the cluster if IYAKKI installed one, Autostart is on, this is
    /// not a desktop window and no other cluster is in the way. Quiet when there is nothing to do.
    pub fn autostart(self: &Arc<Self>) {
        let Some(s) = self.installed() else { return };
        if !s.autostart || self.window() || self.state() != Svc::Stopped {
            return;
        }
        let me = self.clone();
        let _ = std::thread::Builder::new().name("iyakki-k3s-autostart".into()).spawn(move || {
            if let Err(e) = me.start("system") {
                me.audit("system", "devcluster.autostart.failed", &e);
                me.alerts_push(Alert::Down(format!("it did not start at boot: {e}")));
            }
        });
    }

    // -------------------------------------------------------------------------- health and logs

    /// The last health snapshot (cheap; the supervisor refreshes it while the cluster runs).
    pub fn health(&self) -> Health {
        let mut h = lock(&self.health).clone();
        h.installed = self.installed().is_some();
        let st = self.installed().unwrap_or_default();
        h.backend = st.backend;
        h.restarts = st.restarts;
        h.autostart = st.autostart;
        h.last_restart_reason = st.last_restart_reason.clone();
        let s = self.state();
        h.state = s.word().to_string();
        h.note = if matches!(s, Svc::Starting) { lock(&self.net_note).clone() } else { String::new() };
        h.failed_reason = if let Svc::Failed(r) = &s { r.clone() } else { String::new() };
        if !matches!(s, Svc::Running) {
            h.node_ready = if matches!(s, Svc::Starting) { Some(false) } else { None };
            h.pods = None;
        }
        h
    }

    /// Ask the cluster now (a few `kubectl` calls and a walk over the data folder).
    pub fn health_now(&self) -> Health {
        let mut h = self.health();
        if !h.installed {
            return h;
        }
        if self.state() == Svc::Running || self.state() == Svc::Starting {
            let v = self.kubectl(&["get", "nodes", "-o", "jsonpath={.items[0].status.nodeInfo.kubeletVersion}"]);
            if v.ok() {
                h.version = v.stdout.trim().to_string();
            }
            let n = self.kubectl(&["get", "nodes", "-o", "jsonpath={.items[*].status.conditions[?(@.type=='Ready')].status}"]);
            if n.ok() {
                h.node_ready = Some(n.stdout.contains("True"));
            }
            let p = self.kubectl(&["get", "pods", "-A", "--no-headers"]);
            if p.ok() {
                let lines: Vec<&str> = p.stdout.lines().filter(|l| !l.trim().is_empty()).collect();
                let running = lines.iter().filter(|l| l.split_whitespace().nth(3).is_some_and(|s| s == "Running" || s == "Completed")).count();
                h.pods = Some((running, lines.len()));
            }
        } else if let Some(s) = self.installed() {
            h.version = s.version;
        }
        let data = self.layout.data();
        let mut budget = 400_000u32;
        h.data_bytes = Some(dir_size(&data, &mut budget));
        let probe = if data.exists() { data } else { self.layout.root.clone() };
        if let Some((free, total)) = space(&probe) {
            h.disk_free = Some(free);
            if total > 0 {
                h.disk_pct = Some((((total - free.min(total)) as f64 / total as f64) * 100.0).round() as u8);
            }
        }
        h.checked_at = now();
        h
    }

    /// Store a fresh health snapshot (the Dev cluster page and SYSTEM call this from a thread now and then).
    pub fn refresh_health(&self) -> Health {
        let h = self.health_now();
        self.check_disk(&h);
        *lock(&self.health) = h.clone();
        h
    }

    fn log_tail_text(&self, lines: usize) -> String {
        let t = read_tail(&self.layout.log(), 128 * 1024);
        let all: Vec<&str> = t.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }

    /// The end of the service log, as text (Podman: `podman logs --tail`).
    pub fn log_tail(&self, lines: usize) -> String {
        if self.installed().is_some_and(|s| s.backend == Backend::Podman) && self.run.has("podman") {
            let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "logs".into(), "--tail".into(), lines.to_string(), self.inst.pod_name.clone()], &[], Duration::from_secs(20), 256 * 1024, None);
            return o.combined();
        }
        self.log_tail_text(lines)
    }
}

// ------------------------------------------------------------------------------------------------ small helpers

/// The log from byte `off` on (at most `max` bytes of it).
fn read_from(p: &Path, off: u64, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if off >= len || f.seek(SeekFrom::Start(off)).is_err() {
        return String::new();
    }
    let mut b = Vec::new();
    let _ = f.take(max).read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

fn read_tail(p: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > max {
        let _ = f.seek(SeekFrom::Start(len - max));
    }
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

/// Free and total bytes of the file system holding `p`.
pub(crate) fn space(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a NUL-terminated path and `st` is a writable statvfs.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bs = st.f_frsize as u64;
    Some((st.f_bavail as u64 * bs, st.f_blocks as u64 * bs))
}

fn free_bytes(p: &Path) -> Option<u64> {
    let mut q = p.to_path_buf();
    while !q.exists() {
        if !q.pop() {
            return None;
        }
    }
    space(&q).map(|s| s.0)
}

/// The file system type of the mount holding `p` (the longest mount point that is a prefix), from the text of `/proc/mounts`.
pub fn fs_type_of(p: &Path, mounts: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for l in mounts.lines() {
        let mut it = l.split_whitespace();
        let (Some(_), Some(mp), Some(t)) = (it.next(), it.next(), it.next()) else { continue };
        if p.starts_with(mp) && best.as_ref().is_none_or(|b| mp.len() >= b.0) {
            best = Some((mp.len(), t.to_string()));
        }
    }
    best.map(|b| b.1)
}

/// Bytes under a folder (links are not followed); stops counting after `budget` entries.
fn dir_size(p: &Path, budget: &mut u32) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    let mut total = 0;
    for e in rd.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if m.is_dir() {
            total += dir_size(&e.path(), budget);
        } else if m.is_file() {
            total += m.len();
        }
    }
    total
}

#[cfg(test)]
#[path = "k3sdev_tests.rs"]
mod tests;
