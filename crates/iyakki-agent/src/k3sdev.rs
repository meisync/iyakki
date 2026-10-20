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

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Is the managed dev cluster starting or running in this process? (The sandbox cluster refuses to start then.)
pub fn managed_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
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

/// Everything that says a cluster already exists. `ignore` are folders whose files do not count (IYAKKI's own: the CLI tile's `k3s`
/// and the managed folder). `connected` is the cluster some workspace is connected to, `sandbox_up` that IYAKKI's sandbox cluster runs.
pub fn existing(m: &Machine, ignore: &[PathBuf], connected: Option<&str>, sandbox_up: bool) -> Vec<String> {
    let mut v = Vec::new();
    let ignored = |p: &Path| ignore.iter().any(|i| p.starts_with(i));
    if m.k3s_yaml.exists() {
        v.push(format!("{} exists (a k3s server was installed here)", m.k3s_yaml.display()));
    }
    if m.lib_dir.exists() {
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
}

/// The arguments of `k3s server` for this layout: only what the plan lists.
pub fn server_args(l: &Layout) -> Vec<String> {
    ["server", "--data-dir", &l.data().display().to_string(), "--write-kubeconfig", &l.kubeconfig().display().to_string(), "--write-kubeconfig-mode", "600", "--disable", "traefik", "--disable", "servicelb", "--disable", "metrics-server", "--node-name", NODE_NAME].iter().map(|s| s.to_string()).collect()
}

fn podman_create(l: &Layout, image: &str) -> Vec<String> {
    let mut a: Vec<String> = ["podman", "create", "--name", POD_NAME, "--privileged", "--network=host", "--tmpfs", "/run", "--tmpfs", "/var/run", "-v"].iter().map(|s| s.to_string()).collect();
    a.push(format!("{}:/var/lib/rancher/k3s", l.data().display()));
    a.push("-v".into());
    a.push(format!("{}:/output", l.root.display()));
    a.extend(["-e", "K3S_KUBECONFIG_OUTPUT=/output/kubeconfig", "-e", "K3S_KUBECONFIG_MODE=600"].iter().map(|s| s.to_string()));
    a.push(image.to_string());
    a.extend(["server", "--disable", "traefik", "--disable", "servicelb", "--disable", "metrics-server", "--node-name", NODE_NAME].iter().map(|s| s.to_string()));
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
    };
    match backend {
        Backend::Native => {
            let (head, _) = clis.open_follow(st, &url, d.hosts)?;
            p.size = head.header("content-length").and_then(|v| v.parse::<u64>().ok());
            drop(head);
            let text = clis.small(st, &checksum_url, d.hosts)?;
            p.expected_sha = clis::extract_hash(&text, &file).ok_or("the vendor's checksum file has no entry for this download")?;
            let mut s = vec![l.bin().display().to_string()];
            s.extend(server_args(l));
            p.server = s;
        }
        Backend::Podman => {
            p.image = image_for(&version);
            p.pull = vec!["podman".into(), "pull".into(), p.image.clone()];
            p.create = podman_create(l, &p.image);
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
            s.push_str(&format!("  {}\n  then: podman start {POD_NAME}  (IYAKKI starts it, watches it and starts it again when it stops)\n", p.create.join(" ")));
            s.push_str("  The container is privileged and uses the host network: that is how k3s runs in a container.\n");
        }
    }
    s.push_str("  Switched off to save memory and ports: traefik, servicelb, metrics-server.\n");
    s.push_str("\nPORTS\n");
    for (port, what) in ports() {
        s.push_str(&format!("  {port}  {what}\n"));
    }
    s.push_str(&format!("  The API server listens on every interface (TLS and a token are required); keep the machine off untrusted networks or firewall port {API_PORT}.\n"));
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
    vec![
        (format!("{API_PORT}/tcp"), "the Kubernetes API (kubectl)"),
        ("10250/tcp".into(), "the kubelet"),
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
}

impl Health {
    /// The one line SYSTEM shows.
    pub fn line(&self) -> String {
        if !self.installed {
            return "not installed".into();
        }
        let mut s = self.state.clone();
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
    /// Tests: leave the process-wide state alone (`RUNNING`, the connected cluster, the machine's registered kubeconfig).
    quiet: bool,
    connected: Option<Option<String>>,
}

pub struct K3sDev {
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
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl K3sDev {
    pub fn new(data_dir: &Path, clis: Arc<CliManager>, run: Arc<dyn Runner>) -> Arc<K3sDev> {
        let layout = Layout::new(data_dir);
        let installed = load_state(&layout).is_some();
        let d = Arc::new(K3sDev {
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
        });
        if installed {
            crate::devcluster::register_machine_kubeconfig(Some(d.layout.kubeconfig()));
        }
        d
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn set_audit(&self, f: AuditFn) {
        *lock(&self.audit) = Some(f);
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
            RUNNING.store(up, Ordering::SeqCst);
            if s == Svc::Running {
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
        let m = lock(&self.machine).clone();
        let mut ignore = vec![self.layout.root.clone(), self.clis.bin_dir()];
        if let Some(d) = self.clis.bin_dir().parent() {
            ignore.push(d.join("pkg"));
        }
        let sandbox = lock(&self.sandbox_up).clone().is_some_and(|f| f());
        let own_running = managed_running();
        let connected = match lock(&self.over).connected.clone() {
            Some(c) => c,
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
        if !self.quiet() {
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
        let _ = self.run.run(&tmp, &["podman".into(), "rm".into(), "-f".into(), POD_NAME.into()], &[], Duration::from_secs(60), 4096, None);
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
        match st.backend {
            Backend::Native => {
                let child = self.spawn_native()?;
                *lock(&self.child) = Some(child);
            }
            Backend::Podman => self.start_pod(&st)?,
        }
        self.set_svc(Svc::Starting);
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.clone();
        std::thread::Builder::new().name("iyakki-k3s-dev".into()).spawn(move || me.monitor(gen, st.backend)).map_err(io_err)?;
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
        let log2 = log.try_clone().map_err(io_err)?;
        let mut cmd = Command::new(l.bin());
        cmd.args(server_args(l))
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
        let exists = self.run.run(&tmp, &["podman".into(), "inspect".into(), POD_NAME.into()], &[], Duration::from_secs(20), 4096, None).ok();
        if !exists {
            let create = podman_create(&self.layout, &st.image);
            let c = self.run.run(&tmp, &create, &[], Duration::from_secs(120), 16 * 1024, None);
            if !c.ok() {
                return Err(format!("podman create failed: {}", c.combined().lines().last().unwrap_or("no message").trim()));
            }
        }
        let o = self.run.run(&tmp, &["podman".into(), "start".into(), POD_NAME.into()], &[], Duration::from_secs(120), 16 * 1024, None);
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
    fn monitor(self: Arc<Self>, gen: u64, backend: Backend) {
        let began = Instant::now();
        let mut failures: Vec<Instant> = Vec::new();
        let mut last_disk = Instant::now() - Duration::from_secs(3600);
        let mut chmodded = false;
        while self.current(gen) {
            // 1. has it died?
            let died: Option<String> = match backend {
                Backend::Native => {
                    let mut c = lock(&self.child);
                    match c.as_mut() {
                        Some(ch) => match ch.try_wait() {
                            Ok(Some(status)) => {
                                *c = None;
                                Some(format!("k3s exited ({status}); the end of its log: {}", self.log_tail_text(3).replace('\n', " | ")))
                            }
                            _ => None,
                        },
                        None => return,
                    }
                }
                Backend::Podman => {
                    let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "inspect".into(), "--format".into(), "{{.State.Status}} {{.State.ExitCode}}".into(), POD_NAME.into()], &[], Duration::from_secs(20), 4096, None);
                    let t = o.stdout.trim().to_string();
                    if o.ok() && !t.starts_with("running") && !t.starts_with("created") { Some(format!("the container is {t}; the end of its log: {}", self.log_tail_text(3).replace('\n', " | "))) } else { None }
                }
            };
            if let Some(reason) = died {
                if !self.current(gen) {
                    return;
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
                    return;
                }
                self.set_svc(Svc::Starting);
                if !self.pause(gen, self.backoff(failures.len() - 1)) {
                    return;
                }
                let r = match backend {
                    Backend::Native => self.spawn_native().map(|c| *lock(&self.child) = Some(c)),
                    Backend::Podman => self.installed().ok_or_else(|| "state is gone".to_string()).and_then(|s| self.start_pod(&s)),
                };
                match r {
                    Ok(()) => {
                        self.note_restart(&reason, true);
                        self.audit("system", "devcluster.restart", &reason);
                    }
                    Err(e) => {
                        self.set_svc(Svc::Failed(format!("{reason}; restarting failed: {e}")));
                        return;
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
                return;
            }
            if !self.pause(gen, self.poll_every(self.state() == Svc::Running)) {
                return;
            }
        }
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
            Backend::Podman => vec!["podman".into(), "exec".into(), POD_NAME.into(), "kubectl".into()],
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
                    self.set_svc(Svc::Stopped);
                    return Err("the cluster is not running".into());
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
                let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "stop".into(), "-t".into(), "30".into(), POD_NAME.into()], &[], Duration::from_secs(90), 16 * 1024, None);
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
        if typed.trim() != NODE_NAME {
            return Err(format!("Reset deletes everything the cluster stores (its objects, volumes and images). Type {NODE_NAME} to confirm; nothing was changed."));
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
        if typed.trim() != NODE_NAME {
            return Err(format!("Uninstall removes the cluster and all its data. Type {NODE_NAME} to confirm; nothing was changed."));
        }
        let st = self.installed().ok_or("no cluster is installed by IYAKKI here")?;
        self.audit(user, "devcluster.uninstall", &self.layout.root.display().to_string());
        if matches!(self.state(), Svc::Starting | Svc::Running | Svc::Stopping | Svc::Failed(_)) {
            let _ = self.stop(user);
        }
        if st.backend == Backend::Podman && self.run.has("podman") {
            let _ = self.run.run(&std::env::temp_dir(), &["podman".into(), "rm".into(), "-f".into(), POD_NAME.into()], &[], Duration::from_secs(60), 4096, None);
        }
        let root = self.layout.root.clone();
        self.remove_managed(&root)?;
        if !self.quiet() {
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
                let _ = self.run.run(&tmp, &["podman".into(), "rm".into(), "-f".into(), POD_NAME.into()], &[], Duration::from_secs(60), 4096, None);
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
            let o = self.run.run(&std::env::temp_dir(), &["podman".into(), "logs".into(), "--tail".into(), lines.to_string(), POD_NAME.into()], &[], Duration::from_secs(20), 256 * 1024, None);
            return o.combined();
        }
        self.log_tail_text(lines)
    }
}

// ------------------------------------------------------------------------------------------------ small helpers

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
fn space(p: &Path) -> Option<(u64, u64)> {
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
