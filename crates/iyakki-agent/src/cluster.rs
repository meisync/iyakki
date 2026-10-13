//! A small container cluster (k3s) on the machine itself, so the assistant can start containers and run code in them:
//! "let me try this Python program" starts a Python container, runs it with limits, shows what it printed and removes
//! it. It needs a real machine: root, cgroup v2 and the k3s program installed from the CLI tile. k3s is started as a
//! background process of IYAKKI (not as a system service), its kubeconfig is kept in IYAKKI's own folder, and stopping
//! it asks k3s to shut down cleanly.
//!
//! Code runs in a Job in its own namespace: an unprivileged user, no extra capabilities, no access to the cluster's
//! API token, memory, processor, disk and time limits, and no network unless the person allowed it for that run.

use std::ffi::CString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::clis::CliManager;
use crate::exec::{self, Extra};

pub const NAMESPACE: &str = "iyakki-sandbox";
pub const MAX_CODE: usize = 64 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

impl ClusterState {
    pub fn label(&self) -> String {
        match self {
            ClusterState::Stopped => "stopped".into(),
            ClusterState::Starting => "starting (the first start takes a minute or two)".into(),
            ClusterState::Running => "running".into(),
            ClusterState::Stopping => "stopping".into(),
            ClusterState::Failed(e) => format!("failed: {e}"),
        }
    }
}

/// What a sandbox language runs in.
pub struct Language {
    pub name: &'static str,
    pub image: &'static str,
    pub command: &'static [&'static str],
}

pub const LANGUAGES: &[Language] = &[
    Language { name: "python", image: "python:3.12-slim", command: &["python", "-u", "-c"] },
    Language { name: "node", image: "node:22-slim", command: &["node", "-e"] },
    Language { name: "shell", image: "alpine:3.20", command: &["sh", "-c"] },
];

pub fn language(name: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.name == name.trim().to_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxResult {
    pub ok: bool,
    pub output: String,
    pub seconds: u64,
}

#[derive(Default)]
struct Overrides {
    root: Option<bool>,
    cgroups: Option<bool>,
    ramfs: Option<bool>,
}

/// One thing to put in the new root that k3s is given on a RAM-rooted system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JailStep {
    /// A top-level folder: bound (with everything mounted under it) into the new root.
    Bind(String),
    /// A top-level link: copied as it is.
    Link(String, String),
    /// A top-level folder that is only created empty (the one that holds the new root itself).
    Empty(String),
}

/// The new root's contents, from a listing of `/` (name, is a folder, link target). `holder` is the top-level folder that
/// carries the new root and so must not be bound into itself.
pub fn plan_jail(entries: &[(String, bool, Option<String>)], holder: &str) -> Vec<JailStep> {
    let mut v = Vec::new();
    for (name, is_dir, link) in entries {
        if let Some(t) = link {
            v.push(JailStep::Link(name.clone(), t.clone()));
        } else if *is_dir && name == holder {
            v.push(JailStep::Empty(name.clone()));
        } else if *is_dir {
            v.push(JailStep::Bind(name.clone()));
        }
    }
    v.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    v
}

/// The system runs from RAM, where `/` is the `rootfs` that runc cannot `pivot_root` away from ("pivot_root .: invalid
/// argument"). So k3s is started in a mount namespace of its own whose root is a tmpfs holding the same folders (bound from
/// the real ones), which lets containers start. Everything k3s writes still lands in the real folders.
fn start_in_ram_root_jail(cmd: &mut Command) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    const HOLDER: &str = "mnt";
    let entries: Vec<(String, bool, Option<String>)> = std::fs::read_dir("/")
        .map_err(io_err)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ft = e.file_type().ok()?;
            let link = ft.is_symlink().then(|| std::fs::read_link(e.path()).ok().map(|p| p.display().to_string())).flatten();
            Some((name, ft.is_dir(), link))
        })
        .collect();
    let steps = plan_jail(&entries, HOLDER);
    // everything the child needs is made here, because it must not allocate between fork and exec
    let c = |s: &str| CString::new(s).map_err(io_err);
    let root = c("/mnt/iyakki-k3s-root")?;
    let (dot, slash, tmpfs) = (c(".")?, c("/")?, c("tmpfs")?);
    let mut binds: Vec<(CString, CString)> = Vec::new();
    let mut dirs: Vec<CString> = Vec::new();
    let mut links: Vec<(CString, CString)> = Vec::new();
    for st in &steps {
        match st {
            JailStep::Bind(n) => {
                dirs.push(c(&format!("/mnt/iyakki-k3s-root/{n}"))?);
                binds.push((c(&format!("/{n}"))?, c(&format!("/mnt/iyakki-k3s-root/{n}"))?));
            }
            JailStep::Empty(n) => dirs.push(c(&format!("/mnt/iyakki-k3s-root/{n}"))?),
            JailStep::Link(n, t) => links.push((c(t)?, c(&format!("/mnt/iyakki-k3s-root/{n}"))?)),
        }
    }
    let _ = std::fs::create_dir_all("/mnt/iyakki-k3s-root");
    // SAFETY: the closure only calls libc functions on data prepared above and returns an error without allocating.
    unsafe {
        cmd.pre_exec(move || {
            let fail = || Err(std::io::Error::last_os_error());
            if libc::unshare(libc::CLONE_NEWNS) != 0 {
                return fail();
            }
            // nothing mounted in here may show up outside, and the other way round
            if libc::mount(std::ptr::null(), slash.as_ptr(), std::ptr::null(), libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) != 0 {
                return fail();
            }
            if libc::mount(tmpfs.as_ptr(), root.as_ptr(), tmpfs.as_ptr(), 0, std::ptr::null()) != 0 {
                return fail();
            }
            for d in &dirs {
                if libc::mkdir(d.as_ptr(), 0o755) != 0 {
                    return fail();
                }
            }
            for (src, dst) in &binds {
                if libc::mount(src.as_ptr(), dst.as_ptr(), std::ptr::null(), libc::MS_BIND | libc::MS_REC, std::ptr::null()) != 0 {
                    return fail();
                }
            }
            for (target, path) in &links {
                if libc::symlink(target.as_ptr(), path.as_ptr()) != 0 {
                    return fail();
                }
            }
            // like switch_root: move the new root over `/` and chroot into it (pivot_root refuses to leave a rootfs)
            if libc::chdir(root.as_ptr()) != 0
                || libc::mount(dot.as_ptr(), slash.as_ptr(), std::ptr::null(), libc::MS_MOVE, std::ptr::null()) != 0
                || libc::chroot(dot.as_ptr()) != 0
                || libc::chdir(slash.as_ptr()) != 0
            {
                return fail();
            }
            Ok(())
        });
    }
    Ok(())
}

/// On a RAM root the node reports no ephemeral storage, so a Job that asks for some could never be placed. The memory limit
/// (the files live in RAM) still bounds what the code can write.
pub fn without_storage_limits(job: &mut Value) {
    if let Some(l) = job.pointer_mut("/spec/template/spec/containers/0/resources/limits").and_then(Value::as_object_mut) {
        l.remove("ephemeral-storage");
    }
    if let Some(e) = job.pointer_mut("/spec/template/spec/volumes/0/emptyDir").and_then(Value::as_object_mut) {
        e.remove("sizeLimit");
    }
}

pub struct Cluster {
    dir: PathBuf,
    clis: Arc<CliManager>,
    state: Mutex<ClusterState>,
    child: Mutex<Option<Child>>,
    gen: std::sync::atomic::AtomicU64,
    over: Mutex<Overrides>,
}

fn io_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Cluster {
    pub fn new(dir: &Path, clis: Arc<CliManager>) -> Cluster {
        Cluster { dir: dir.into(), clis, state: Mutex::new(ClusterState::Stopped), child: Mutex::new(None), gen: std::sync::atomic::AtomicU64::new(0), over: Mutex::new(Overrides::default()) }
    }

    /// For tests on machines that are not the real thing.
    pub fn assume_machine(&self, root: bool, cgroups: bool) {
        let mut o = self.over.lock().unwrap();
        o.root = Some(root);
        o.cgroups = Some(cgroups);
    }

    /// For tests: pretend `/` is (or is not) a RAM root file system.
    pub fn assume_ramfs(&self, ramfs: bool) {
        self.over.lock().unwrap().ramfs = Some(ramfs);
    }

    /// The live system runs from RAM: `/` is `rootfs`, which the kubelet cannot measure.
    fn root_is_ramfs(&self) -> bool {
        self.over.lock().unwrap().ramfs.unwrap_or_else(|| std::fs::read_to_string("/proc/mounts").is_ok_and(|m| m.lines().any(|l| { let mut p = l.split_whitespace(); matches!((p.next(), p.next(), p.next()), (Some("rootfs"), Some("/"), Some("rootfs")) | (Some("none"), Some("/"), Some("ramfs"))) })))
    }

    pub fn kubeconfig(&self) -> PathBuf {
        self.dir.join("kubeconfig")
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join("k3s.log")
    }

    pub fn state(&self) -> ClusterState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, s: ClusterState) {
        *self.state.lock().unwrap() = s;
    }

    /// What is missing for the cluster to run here (empty when it can).
    pub fn requirements(&self) -> Vec<String> {
        let o = self.over.lock().unwrap();
        let mut v = Vec::new();
        // SAFETY: geteuid has no preconditions.
        let root = o.root.unwrap_or_else(|| unsafe { libc::geteuid() } == 0);
        if !root {
            v.push("it must run as root (this is a desktop test window, not the machine itself)".to_string());
        }
        if !o.cgroups.unwrap_or_else(|| Path::new("/sys/fs/cgroup/cgroup.controllers").exists()) {
            v.push("the kernel needs cgroup v2 (/sys/fs/cgroup/cgroup.controllers is missing)".to_string());
        }
        if self.clis.is_installed("k3s").is_none() {
            v.push("k3s is not installed (install it in the CLIs tab)".to_string());
        }
        v
    }

    fn k3s(&self) -> PathBuf {
        self.clis.bin_dir().join("k3s")
    }

    fn extras(&self) -> Vec<Extra> {
        vec![Extra { name: "k3s".into(), host: None }]
    }

    /// Runs `k3s kubectl --kubeconfig … <args>`.
    fn kubectl(&self, args: &[&str], secs: u64) -> Result<String, String> {
        let mut a = vec!["kubectl".to_string(), "--kubeconfig".to_string(), self.kubeconfig().display().to_string()];
        a.extend(args.iter().map(|s| s.to_string()));
        exec::run_with_env(&self.extras(), "k3s", &a, &self.dir, None, secs, Some(&self.clis.bin_dir()), &[("KUBECONFIG".into(), self.kubeconfig().display().to_string())])
    }

    pub fn log_tail(&self, lines: usize) -> String {
        let t = std::fs::read_to_string(self.log_path()).unwrap_or_default();
        let all: Vec<&str> = t.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let missing = self.requirements();
        if !missing.is_empty() {
            return Err(format!("the container cluster cannot start here: {}", missing.join("; ")));
        }
        if matches!(self.state(), ClusterState::Starting | ClusterState::Running) {
            return Err("the cluster is already running".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(io_err)?;
        let _ = std::fs::remove_file(self.kubeconfig());
        // from a RAM root the kubelet cannot read the root file system's capacity ("cannot find filesystem info for device rootfs"),
        // so per-pod disk limits are switched off there (memory and processor limits are not affected)
        let conf_d = self.dir.join("data/agent/etc/kubelet.conf.d");
        if self.root_is_ramfs() {
            std::fs::create_dir_all(&conf_d).map_err(io_err)?;
            std::fs::write(conf_d.join("10-iyakki.conf"), "apiVersion: kubelet.config.k8s.io/v1beta1\nkind: KubeletConfiguration\nlocalStorageCapacityIsolation: false\n").map_err(io_err)?;
        } else {
            let _ = std::fs::remove_file(conf_d.join("10-iyakki.conf"));
        }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.log_path()).map_err(io_err)?;
        let log2 = log.try_clone().map_err(io_err)?;
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(self.k3s());
        cmd.args(["server", "--data-dir"])
            .arg(self.dir.join("data"))
            .arg("--write-kubeconfig")
            .arg(self.kubeconfig())
            .args(["--write-kubeconfig-mode", "600", "--disable", "traefik", "--disable", "servicelb", "--disable", "metrics-server", "--node-name", "iyakki"])
            .env_clear()
            .env("PATH", format!("{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", self.clis.bin_dir().display()))
            .env("HOME", &self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2))
            .process_group(0);
        if self.root_is_ramfs() && self.over.lock().unwrap().ramfs.is_none() {
            start_in_ram_root_jail(&mut cmd)?;
        }
        let child = cmd.spawn().map_err(|e| format!("could not start k3s: {e}"))?;
        *self.child.lock().unwrap() = Some(child);
        self.set_state(ClusterState::Starting);
        let gen = self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let me = self.clone();
        std::thread::Builder::new().name("iyakki-k3s-monitor".into()).spawn(move || me.monitor(gen)).map_err(io_err)?;
        Ok(())
    }

    fn current(&self, gen: u64) -> bool {
        self.gen.load(std::sync::atomic::Ordering::SeqCst) == gen
    }

    /// Watches the k3s process and whether its node is ready.
    fn monitor(&self, gen: u64) {
        let began = Instant::now();
        while self.current(gen) {
            {
                let mut c = self.child.lock().unwrap();
                if let Some(ch) = c.as_mut() {
                    if let Ok(Some(status)) = ch.try_wait() {
                        *c = None;
                        drop(c);
                        if self.current(gen) {
                            self.set_state(ClusterState::Failed(format!("k3s exited ({status}); the end of its log: {}", self.log_tail(3).replace('\n', " | "))));
                        }
                        return;
                    }
                } else {
                    return;
                }
            }
            if self.kubeconfig().exists() {
                let ready = self.kubectl(&["get", "nodes", "-o", "jsonpath={.items[*].status.conditions[?(@.type=='Ready')].status}"], 15).map(|o| o.contains("True")).unwrap_or(false);
                if self.current(gen) {
                    if ready {
                        if self.state() == ClusterState::Starting {
                            self.set_state(ClusterState::Running);
                        }
                    } else if self.state() == ClusterState::Running {
                        self.set_state(ClusterState::Starting);
                    }
                }
            }
            if began.elapsed() > Duration::from_secs(600) && self.state() == ClusterState::Starting {
                self.set_state(ClusterState::Failed("the node did not become ready within ten minutes".into()));
                return;
            }
            let wait = if self.state() == ClusterState::Running { 10 } else { 2 };
            for _ in 0..wait * 10 {
                if !self.current(gen) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// Asks k3s to shut down (SIGTERM, which stops its containers cleanly) and, if it has not by then, ends it.
    pub fn stop(&self) -> Result<(), String> {
        let child = self.child.lock().unwrap().take();
        self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(mut child) = child else {
            self.set_state(ClusterState::Stopped);
            return Err("the cluster is not running".into());
        };
        self.set_state(ClusterState::Stopping);
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
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(self.kubeconfig());
        self.set_state(ClusterState::Stopped);
        Ok(())
    }

    fn manifest_file(&self, name: &str, v: &Value) -> Result<PathBuf, String> {
        std::fs::create_dir_all(self.dir.join("jobs")).map_err(io_err)?;
        let p = self.dir.join("jobs").join(format!("{name}.json"));
        let mut f = std::fs::File::create(&p).map_err(io_err)?;
        f.write_all(v.to_string().as_bytes()).map_err(io_err)?;
        Ok(p)
    }

    /// The Job that runs `code` (exposed so it can be checked without a cluster).
    pub fn job_manifest(name: &str, lang: &Language, code: &str, timeout: u64, network: bool) -> Value {
        let mut command: Vec<&str> = lang.command.to_vec();
        command.push(code);
        json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {"name": name, "namespace": NAMESPACE, "labels": {"app": "iyakki-sandbox"}},
            "spec": {
                "backoffLimit": 0,
                "activeDeadlineSeconds": timeout,
                "ttlSecondsAfterFinished": 120,
                "template": {
                    "metadata": {"labels": {"app": "iyakki-sandbox", "iyakki-net": if network { "open" } else { "none" }}},
                    "spec": {
                        "restartPolicy": "Never",
                        "automountServiceAccountToken": false,
                        "enableServiceLinks": false,
                        "securityContext": {"runAsNonRoot": true, "runAsUser": 65534, "runAsGroup": 65534, "seccompProfile": {"type": "RuntimeDefault"}},
                        "containers": [{
                            "name": "run",
                            "image": lang.image,
                            "command": command,
                            "workingDir": "/work",
                            "env": [{"name": "HOME", "value": "/work"}, {"name": "PYTHONDONTWRITEBYTECODE", "value": "1"}],
                            "securityContext": {"allowPrivilegeEscalation": false, "capabilities": {"drop": ["ALL"]}},
                            "resources": {"limits": {"cpu": "1", "memory": "512Mi", "ephemeral-storage": "256Mi"}, "requests": {"cpu": "50m", "memory": "32Mi"}},
                            "volumeMounts": [{"name": "work", "mountPath": "/work"}]
                        }],
                        "volumes": [{"name": "work", "emptyDir": {"sizeLimit": "128Mi"}}]
                    }
                }
            }
        })
    }

    /// The namespace, and the rule that keeps code without network permission off the network.
    pub fn setup_manifest() -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "List",
            "items": [
                {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}},
                {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy", "metadata": {"name": "no-network", "namespace": NAMESPACE},
                 "spec": {"podSelector": {"matchLabels": {"iyakki-net": "none"}}, "policyTypes": ["Ingress", "Egress"]}},
                {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy", "metadata": {"name": "no-ingress", "namespace": NAMESPACE},
                 "spec": {"podSelector": {}, "policyTypes": ["Ingress"]}}
            ]
        })
    }

    /// A plain sentence for why a job did not succeed, from what the cluster recorded.
    fn failure_reason(&self, name: &str, timeout: u64) -> String {
        let job = self.kubectl(&["get", "job", name, "-n", NAMESPACE, "-o", "json"], 30).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let deadline = job.as_ref().and_then(|j| j.pointer("/status/conditions")).and_then(Value::as_array).is_some_and(|c| c.iter().any(|x| x.get("reason").and_then(Value::as_str) == Some("DeadlineExceeded")));
        if deadline {
            return format!("stopped: it ran longer than {timeout} seconds");
        }
        let pods = self.kubectl(&["get", "pods", "-n", NAMESPACE, "-l", &format!("job-name={name}"), "-o", "json"], 30).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let term = pods.as_ref().and_then(|p| p.pointer("/items/0/status/containerStatuses/0/state/terminated").or_else(|| p.pointer("/items/0/status/containerStatuses/0/lastState/terminated")));
        match term {
            Some(t) if t.get("reason").and_then(Value::as_str) == Some("OOMKilled") => "stopped: it used more than its memory limit (512 MB)".into(),
            Some(t) => match t.get("exitCode").and_then(Value::as_i64) {
                Some(c) if c != 0 => format!("it exited with status {c}"),
                _ => String::new(),
            },
            None => String::new(),
        }
    }

    /// Runs `code` in a fresh container of the language's image and returns what it printed. Waits at most
    /// `timeout` seconds for the code itself (plus time to fetch the image the first time).
    pub fn sandbox_run(&self, language_name: &str, code: &str, timeout: u64, network: bool) -> Result<SandboxResult, String> {
        let lang = language(language_name).ok_or_else(|| format!("languages available: {}", LANGUAGES.iter().map(|l| l.name).collect::<Vec<_>>().join(", ")))?;
        if code.trim().is_empty() || code.len() > MAX_CODE || code.contains('\0') {
            return Err(format!("give some code (up to {} KB)", MAX_CODE / 1024));
        }
        if self.state() != ClusterState::Running {
            return Err(format!("the container cluster is {}. Start it first (cluster_start), then try again.", self.state().label()));
        }
        let timeout = timeout.clamp(5, 600);
        let mut r = [0u8; 4];
        let _ = iyakki_sec::random_fill(&mut r);
        let name = format!("sbx-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let setup = self.manifest_file("setup", &Self::setup_manifest())?;
        self.kubectl(&["apply", "-f", &setup.display().to_string()], 60)?;
        let mut manifest = Self::job_manifest(&name, lang, code, timeout, network);
        if self.root_is_ramfs() {
            without_storage_limits(&mut manifest);
        }
        let file = self.manifest_file(&name, &manifest)?;
        self.kubectl(&["apply", "-f", &file.display().to_string()], 60)?;
        let _ = std::fs::remove_file(&file);
        let started = Instant::now();
        // the code gets `timeout` seconds once it runs; fetching the image the first time may take longer
        let give_up = Duration::from_secs(timeout + 240);
        let mut outcome: Option<bool> = None;
        while started.elapsed() < give_up {
            let j = self.kubectl(&["get", "job", &name, "-n", NAMESPACE, "-o", "json"], 30).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
            if let Some(j) = &j {
                if j.pointer("/status/succeeded").and_then(Value::as_u64).unwrap_or(0) >= 1 {
                    outcome = Some(true);
                    break;
                }
                if j.pointer("/status/failed").and_then(Value::as_u64).unwrap_or(0) >= 1 {
                    outcome = Some(false);
                    break;
                }
            }
            // an image that cannot be fetched is reported at once instead of waiting out the clock
            if let Ok(t) = self.kubectl(&["get", "pods", "-n", NAMESPACE, "-l", &format!("job-name={name}"), "-o", "json"], 30) {
                if let Ok(p) = serde_json::from_str::<Value>(&t) {
                    let reason = p.pointer("/items/0/status/containerStatuses/0/state/waiting/reason").and_then(Value::as_str).unwrap_or("");
                    if matches!(reason, "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName") && started.elapsed() > Duration::from_secs(20) {
                        let _ = self.kubectl(&["delete", "job", &name, "-n", NAMESPACE, "--wait=false"], 30);
                        return Err(format!("the image {} could not be fetched ({reason}); the machine needs to reach the container registry the first time (docker.io)", lang.image));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        let mut logs = self.kubectl(&["logs", &format!("job/{name}"), "-n", NAMESPACE, "--tail=500", &format!("--limit-bytes={MAX_OUTPUT}")], 30).unwrap_or_default();
        if logs.trim() == "(no output, exit status 0)" {
            logs.clear();
        }
        // why it did not succeed: out of memory, out of time, or an exit status
        let why = if outcome == Some(true) { String::new() } else { self.failure_reason(&name, timeout) };
        let _ = self.kubectl(&["delete", "job", &name, "-n", NAMESPACE, "--wait=false"], 30);
        let seconds = started.elapsed().as_secs();
        let tail = |l: &str, w: &str| if w.is_empty() { l.to_string() } else if l.trim().is_empty() { format!("[{w}]") } else { format!("{}\n[{w}]", l.trim_end()) };
        match outcome {
            Some(ok) => Ok(SandboxResult { ok, output: tail(&logs, &why), seconds }),
            None => Ok(SandboxResult { ok: false, output: tail(&logs, if why.is_empty() { "it did not finish in time" } else { &why }), seconds }),
        }
    }
}


#[doc(hidden)]
/// For tests (here and in the shell): installs a stand-in `k3s` into a CLI manager's folder. `server` writes the
/// kubeconfig and waits; `kubectl …` answers like a healthy cluster and records what it was asked in `rec`.
pub fn write_fake_k3s(root: &Path, rec: &Path, job_result: &str) {
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let prog = root.join("pkgs/k3s/v1/k3s");
    std::fs::create_dir_all(prog.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = server ]; then\n  \
           while [ $# -gt 0 ]; do [ \"$1\" = --write-kubeconfig ] && cfg=\"$2\"; shift; done\n  \
           echo 'apiVersion: v1' > \"$cfg\"\n  echo 'k3s started'\n  trap 'echo stopping; exit 0' TERM\n  while true; do sleep 0.1; done\nfi\n\
         # kubectl --kubeconfig PATH <args>\n\
         shift; shift; shift\n\
         echo \"$@\" >> {rec}\n\
         case \"$1\" in\n  \
           get) case \"$2\" in\n    nodes) echo True;;\n    job) echo '{job_result}';;\n    pods) if echo '{job_result}' | grep -q oomkilled; then echo '{{\"items\":[{{\"status\":{{\"containerStatuses\":[{{\"state\":{{\"terminated\":{{\"reason\":\"OOMKilled\",\"exitCode\":137}}}}}}]}}}}]}}'; else echo '{{\"items\":[]}}'; fi;;\n  esac;;\n  \
           apply) for f in \"$@\"; do [ -f \"$f\" ] && cat \"$f\" >> {rec}.manifests && echo >> {rec}.manifests; done;;\n  \
           logs) echo 'hello from the container';;\n\
         esac\n",
        rec = rec.display(),
        job_result = job_result
    );
    std::fs::write(&prog, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_file(root.join("bin/k3s"));
    std::os::unix::fs::symlink(&prog, root.join("bin/k3s")).unwrap();
    std::fs::write(root.join("state.json"), r#"{"k3s":{"id":"k3s","version":"v1","reported":"","at":1,"bins":["k3s"],"verified":true,"by":"root"}}"#).unwrap();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::archive::tests::tmp;

    /// A stand-in `k3s`: `server` writes the kubeconfig and waits; `kubectl …` answers like a healthy cluster and
    /// records what it was asked.
    fn fake_k3s(d: &Path, job_result: &str) -> Arc<CliManager> {
        let root = d.join("cli");
        super::write_fake_k3s(&root, &d.join("kubectl.log"), job_result);
        Arc::new(CliManager::new(&root))
    }

    fn wait_state(c: &Cluster, want: ClusterState) {
        for _ in 0..400 {
            if c.state() == want {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the cluster is {:?}, wanted {want:?}", c.state());
    }

    #[test]
    fn the_cluster_says_what_is_missing_and_only_starts_on_a_real_machine_with_k3s() {
        let d = tmp("cl-req");
        let clis = Arc::new(CliManager::new(&d.join("cli")));
        let c = Arc::new(Cluster::new(&d.join("cluster"), clis));
        c.assume_machine(false, false);
        let r = c.requirements();
        assert_eq!(r.len(), 3, "{r:?}");
        assert!(r[0].contains("root") && r[1].contains("cgroup v2") && r[2].contains("k3s is not installed"));
        assert!(c.start().unwrap_err().contains("cannot start here"));
        assert_eq!(c.state(), ClusterState::Stopped);
        assert!(c.stop().unwrap_err().contains("not running"));
        assert!(c.sandbox_run("python", "print(1)", 30, false).unwrap_err().contains("Start it first"));
        // a machine that can, with k3s installed
        let clis = fake_k3s(&d, "{}");
        let c2 = Arc::new(Cluster::new(&d.join("cluster2"), clis));
        c2.assume_machine(true, true);
        assert!(c2.requirements().is_empty());
    }

    #[test]
    fn the_cluster_starts_becomes_ready_runs_code_in_a_limited_job_and_stops_cleanly() {
        let d = tmp("cl-run");
        let clis = fake_k3s(&d, r#"{"status":{"succeeded":1}}"#);
        let c = Arc::new(Cluster::new(&d.join("cluster"), clis));
        c.assume_machine(true, true);
        c.start().unwrap();
        assert!(matches!(c.state(), ClusterState::Starting | ClusterState::Running));
        wait_state(&c, ClusterState::Running);
        assert!(c.kubeconfig().exists());
        assert!(c.start().unwrap_err().contains("already running"));
        // run Python: the job is applied with the limits, the output comes back, the job is deleted
        let r = c.sandbox_run("python", "print('hi')\nprint(2+2)", 30, false).unwrap();
        assert!(r.ok && r.output.contains("hello from the container"), "{r:?}");
        let rec = std::fs::read_to_string(d.join("kubectl.log")).unwrap();
        assert!(rec.contains("apply -f") && rec.contains("get job sbx-") && rec.contains("logs job/sbx-") && rec.contains("delete job sbx-"), "{rec}");
        let manifests = std::fs::read_to_string(d.join("kubectl.log.manifests")).unwrap();
        assert!(manifests.contains("iyakki-sandbox") && manifests.contains("python:3.12-slim") && manifests.contains("\"runAsNonRoot\":true") && manifests.contains("\"drop\":[\"ALL\"]") && manifests.contains("\"memory\":\"512Mi\"") && manifests.contains("\"automountServiceAccountToken\":false") && manifests.contains("\"iyakki-net\":\"none\"") && manifests.contains("no-network"), "{manifests}");
        assert!(manifests.contains("print('hi')\\nprint(2+2)"), "the code goes in as one argument, never through a shell");
        // bad input is refused before anything is sent
        let before = std::fs::read_to_string(d.join("kubectl.log")).unwrap().lines().count();
        assert!(c.sandbox_run("cobol", "x", 30, false).unwrap_err().contains("languages available"));
        assert!(c.sandbox_run("python", "   ", 30, false).is_err() && c.sandbox_run("python", &"x".repeat(MAX_CODE + 1), 30, false).is_err());
        assert_eq!(std::fs::read_to_string(d.join("kubectl.log")).unwrap().lines().count(), before);
        // stop: the process ends and the cluster is stopped; running code needs it started again
        c.stop().unwrap();
        assert_eq!(c.state(), ClusterState::Stopped);
        assert!(!c.kubeconfig().exists());
        assert!(c.log_tail(5).contains("k3s started"));
        assert!(c.sandbox_run("python", "print(1)", 30, false).is_err());
    }

    #[test]
    fn why_a_job_failed_is_said_in_words_out_of_memory_out_of_time_or_an_exit_status() {
        let d = tmp("cl-why");
        let clis = fake_k3s(&d, r#"{"status":{"failed":1},"x":"oomkilled"}"#);
        let c = Arc::new(Cluster::new(&d.join("cluster"), clis));
        c.assume_machine(true, true);
        c.start().unwrap();
        wait_state(&c, ClusterState::Running);
        let r = c.sandbox_run("python", "b = bytearray(10**10)", 20, false).unwrap();
        assert!(!r.ok && r.output.contains("[stopped: it used more than its memory limit (512 MB)]"), "{r:?}");
        assert!(!r.output.contains("exit status 0"), "no confusing placeholder: {r:?}");
        c.stop().unwrap();
        let d2 = tmp("cl-why2");
        let clis = fake_k3s(&d2, r#"{"status":{"failed":1,"conditions":[{"type":"Failed","reason":"DeadlineExceeded"}]}}"#);
        let c2 = Arc::new(Cluster::new(&d2.join("cluster"), clis));
        c2.assume_machine(true, true);
        c2.start().unwrap();
        wait_state(&c2, ClusterState::Running);
        let r = c2.sandbox_run("python", "while True: pass", 8, false).unwrap();
        assert!(r.output.contains("ran longer than 8 seconds"), "{r:?}");
        c2.stop().unwrap();
    }

    #[test]
    fn on_a_ram_root_the_job_asks_for_no_disk_space() {
        let mut m = Cluster::job_manifest("sbx-1", language("shell").unwrap(), "echo hi", 5, false);
        without_storage_limits(&mut m);
        let t = m.to_string();
        assert!(!t.contains("ephemeral-storage") && !t.contains("sizeLimit"));
        assert!(t.contains("\"memory\":\"512Mi\""));
    }

    #[test]
    fn the_new_root_binds_every_top_level_folder_copies_links_and_leaves_its_own_holder_empty() {
        let l = |n: &str, d: bool, t: Option<&str>| (n.to_string(), d, t.map(str::to_string));
        let plan = plan_jail(&[l("bin", false, Some("usr/bin")), l("etc", true, None), l("mnt", true, None), l("proc", true, None), l("tmp", true, None), l("init", false, None)], "mnt");
        assert!(plan.contains(&JailStep::Link("bin".into(), "usr/bin".into())));
        assert!(plan.contains(&JailStep::Empty("mnt".into())), "the folder that holds the new root is not bound into itself");
        for n in ["etc", "proc", "tmp"] {
            assert!(plan.contains(&JailStep::Bind(n.into())), "{n}");
        }
        assert!(!plan.iter().any(|s| matches!(s, JailStep::Bind(n) | JailStep::Empty(n) if n == "init")), "plain files at the top are left out");
        assert_eq!(plan.len(), 5);
    }

    #[test]
    fn a_ram_root_gets_a_kubelet_setting_that_a_disk_root_does_not() {
        let d = tmp("cl-ram");
        let clis = fake_k3s(&d, "{}");
        let c = Arc::new(Cluster::new(&d.join("cluster"), clis));
        c.assume_machine(true, true);
        c.assume_ramfs(true);
        c.start().unwrap();
        wait_state(&c, ClusterState::Running);
        let conf = d.join("cluster/data/agent/etc/kubelet.conf.d/10-iyakki.conf");
        assert!(std::fs::read_to_string(&conf).unwrap().contains("localStorageCapacityIsolation: false"));
        c.stop().unwrap();
        c.assume_ramfs(false);
        c.start().unwrap();
        wait_state(&c, ClusterState::Running);
        assert!(!conf.exists(), "a normal root file system keeps the disk limits");
        c.stop().unwrap();
    }

    #[test]
    fn a_failed_job_a_network_run_and_a_k3s_that_dies_are_reported() {
        let d = tmp("cl-fail");
        let clis = fake_k3s(&d, r#"{"status":{"failed":1}}"#);
        let c = Arc::new(Cluster::new(&d.join("cluster"), clis));
        c.assume_machine(true, true);
        c.start().unwrap();
        wait_state(&c, ClusterState::Running);
        let r = c.sandbox_run("node", "throw new Error('x')", 20, true).unwrap();
        assert!(!r.ok, "{r:?}");
        let manifests = std::fs::read_to_string(d.join("kubectl.log.manifests")).unwrap();
        assert!(manifests.contains("node:22-slim") && manifests.contains("\"iyakki-net\":\"open\""), "network was asked for, so the pod is labelled open");
        // the manifests themselves
        let m = Cluster::job_manifest("sbx-1", language("shell").unwrap(), "echo hi", 5, false);
        assert_eq!(m["spec"]["template"]["spec"]["containers"][0]["command"], json!(["sh", "-c", "echo hi"]));
        assert_eq!(m["spec"]["activeDeadlineSeconds"], 5);
        // k3s dying by itself is noticed
        let child_id = c.child.lock().unwrap().as_ref().map(|ch| ch.id()).unwrap();
        // SAFETY: killing the stand-in k3s this test started.
        unsafe { libc::kill(child_id as i32, libc::SIGKILL) };
        for _ in 0..200 {
            if matches!(c.state(), ClusterState::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(matches!(c.state(), ClusterState::Failed(ref e) if e.contains("k3s exited")), "{:?}", c.state());
        assert!(c.sandbox_run("python", "print(1)", 30, false).unwrap_err().contains("failed"));
    }
}

