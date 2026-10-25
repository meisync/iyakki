//! Finding and connecting to the cluster a project develops against (docs/K3S-DEV.md, phase 1, window mode), and the rule
//! that IYAKKI never starts a second one by accident.
//!
//! * [`resolve`] looks through the places a kubeconfig can be, in order (the workspace's `project.json`, the person's earlier
//!   choice, `$KUBECONFIG`, `~/.kube/config`, `/etc/rancher/k3s/k3s.yaml`, k3s's own data folder), tests each readable one with
//!   `kubectl` and returns a [`ClusterProfile`] per file: cluster name, context, server, whether it answers, whether it is
//!   k3s. A kubeconfig the person cannot read (root-only) is reported with the one-time fix; nothing here ever runs `sudo`.
//! * [`guard`] refuses commands that would start, stop, install or remove a cluster (`k3s server`, `systemctl restart k3s`,
//!   `kubeadm init`, `minikube start`, `kind create cluster` …) while a cluster is connected or in a desktop window, with a
//!   message that tells the model what to do instead. `kubectl`, `k3s kubectl` and `k3s ctr images ls` stay allowed.
//!
//! Every program is started through [`crate::wsexec::Runner`] (argument arrays, `KUBECONFIG` in the environment, timeouts,
//! output caps), so tests use a fake runner.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workspace::K8sConfig;
use crate::wsexec::{Out, Runner};

pub const REACH_TIMEOUT_SECS: u64 = 3;
pub const OUTPUT_CAP: usize = 256 * 1024;

// ------------------------------------------------------------------------------------------------ the executor

/// `kubectl` against one kubeconfig and context, through a [`Runner`].
#[derive(Clone)]
pub struct Kc {
    pub run: Arc<dyn Runner>,
    pub kubeconfig: String,
    pub context: String,
}

impl Kc {
    pub fn new(run: Arc<dyn Runner>, kubeconfig: &str, context: &str) -> Kc {
        Kc { run, kubeconfig: kubeconfig.into(), context: context.into() }
    }

    pub fn env(&self) -> Vec<(String, String)> {
        if self.kubeconfig.is_empty() { vec![] } else { vec![("KUBECONFIG".into(), self.kubeconfig.clone())] }
    }

    /// The command line for `args` (everything after `kubectl`).
    pub fn argv(&self, args: &[&str], request_secs: u64) -> Vec<String> {
        let mut a: Vec<String> = vec!["kubectl".into()];
        if !self.kubeconfig.is_empty() {
            a.push("--kubeconfig".into());
            a.push(self.kubeconfig.clone());
        }
        if !self.context.is_empty() {
            a.push("--context".into());
            a.push(self.context.clone());
        }
        a.push(format!("--request-timeout={request_secs}s"));
        a.extend(args.iter().map(|s| s.to_string()));
        a
    }

    pub fn kubectl(&self, args: &[&str], request_secs: u64, timeout_secs: u64) -> Out {
        self.run.run(&std::env::temp_dir(), &self.argv(args, request_secs), &self.env(), Duration::from_secs(timeout_secs), OUTPUT_CAP, None)
    }

    /// A short read: 8 s to answer.
    pub fn read(&self, args: &[&str]) -> Out {
        self.kubectl(args, 8, 14)
    }
}

/// The adapter that lets the IDE's cluster view (`k8s::fetch`) run through the same executor.
pub struct RunnerKube {
    pub run: Arc<dyn Runner>,
}

impl crate::k8s::Kube for RunnerKube {
    fn run(&self, args: &[String]) -> Out {
        self.run_with(args, crate::k8s::REQUEST_TIMEOUT_SECS + 6)
    }

    fn run_with(&self, args: &[String], timeout_secs: u64) -> Out {
        let mut argv = vec!["kubectl".to_string()];
        argv.extend(args.iter().cloned());
        let kc = args.windows(2).find(|w| w[0] == "--kubeconfig").map(|w| w[1].clone());
        let env: Vec<(String, String)> = kc.map(|k| vec![("KUBECONFIG".to_string(), k)]).unwrap_or_default();
        self.run.run(&std::env::temp_dir(), &argv, &env, Duration::from_secs(timeout_secs), crate::k8s::MAX_OUTPUT, None)
    }
}

/// `kubectl` inside the managed cluster's container (the Podman backend of `k3sdev`): the kubeconfig and context of the caller's
/// arguments are left out, because the container's own kubectl already points at its cluster.
pub struct PodKube {
    pub run: Arc<dyn Runner>,
    pub container: String,
}

impl crate::k8s::Kube for PodKube {
    fn run(&self, args: &[String]) -> Out {
        self.run_with(args, crate::k8s::REQUEST_TIMEOUT_SECS + 6)
    }

    fn run_with(&self, args: &[String], timeout_secs: u64) -> Out {
        let mut argv: Vec<String> = vec!["podman".into(), "exec".into(), self.container.clone(), "kubectl".into()];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--kubeconfig" || a == "--context" {
                it.next();
            } else {
                argv.push(a.clone());
            }
        }
        self.run.run(&std::env::temp_dir(), &argv, &[], Duration::from_secs(timeout_secs), crate::k8s::MAX_OUTPUT, None)
    }
}

// ------------------------------------------------------------------------------------------------ profiles

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The workspace's `project.json`.
    Project,
    /// The person's earlier choice.
    Remembered,
    /// `$KUBECONFIG`.
    Env,
    /// `~/.kube/config`.
    UserKube,
    /// `/etc/rancher/k3s/k3s.yaml`.
    K3s,
    /// k3s's data folder (root-only in practice).
    K3sData,
    /// The service account this pod runs as (`kubeprofiles::detect_in_cluster`).
    InCluster,
    /// A kubeconfig held in a vault entry (`vault:<name>` / `uvault:<name>`), written to a 0600 temp file only for the child process.
    Vault,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Project => "this workspace",
            Source::Remembered => "your earlier choice",
            Source::Env => "$KUBECONFIG",
            Source::UserKube => "~/.kube/config",
            Source::K3s => "k3s on this machine",
            Source::K3sData => "k3s data folder",
            Source::InCluster => "this pod's service account",
            Source::Vault => "a vault entry",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterProfile {
    /// What the chip shows first: `k3s`, or the cluster's own name.
    pub name: String,
    pub kubeconfig: String,
    pub context: String,
    pub server: String,
    pub source: Source,
    pub exists: bool,
    pub readable: bool,
    /// Whether `kubectl` was run against it (the rest of the list is not tested once one answers).
    pub tested: bool,
    pub reachable: bool,
    pub k3s: bool,
    /// The API server is on this machine.
    pub local: bool,
    pub version: String,
    /// What the identity may do in the workspace namespace (filled for the chosen profile).
    pub can_i: String,
    pub problem: String,
    /// The one-time fix for an unreadable or unreachable cluster.
    pub fix: String,
}

impl ClusterProfile {
    fn empty(path: &str, source: Source) -> ClusterProfile {
        ClusterProfile { name: String::new(), kubeconfig: path.into(), context: String::new(), server: String::new(), source, exists: true, readable: false, tested: false, reachable: false, k3s: false, local: false, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() }
    }

    pub fn state(&self) -> &'static str {
        if !self.readable {
            "not readable"
        } else if !self.tested {
            "not tested"
        } else if self.reachable {
            "reachable"
        } else {
            "unreachable"
        }
    }

    /// `k3s · default · reachable`
    pub fn chip(&self) -> String {
        let ctx = if self.context.is_empty() { "no context" } else { self.context.as_str() };
        format!("{} · {} · {}", if self.name.is_empty() { "cluster" } else { &self.name }, ctx, self.state())
    }

    /// One line for the model and the messages.
    pub fn describe(&self) -> String {
        format!("{} (context {}, {}{}, kubeconfig {})", if self.name.is_empty() { "cluster" } else { &self.name }, if self.context.is_empty() { "-" } else { &self.context }, if self.server.is_empty() { "server unknown" } else { &self.server }, if self.k3s { ", k3s" } else { "" }, self.kubeconfig)
    }

    pub fn looks_like_production(&self) -> bool {
        let t = format!("{} {} {}", self.name, self.context, self.server).to_lowercase();
        t.contains("prod") && !t.contains("nonprod") && !t.contains("non-prod")
    }
}

/// Why the assistant may only read from this cluster (`None`: it may write, with the usual approvals). Used by the tools and by
/// Settings \u{2192} Dev cluster, so both say the same thing.
pub fn read_only_reason(p: &ClusterProfile) -> Option<String> {
    // a remote cluster that only came from $KUBECONFIG or ~/.kube/config (never chosen for this workspace) is read-only
    if !p.local && matches!(p.source, Source::Env | Source::UserKube) {
        return Some(format!("the cluster \"{}\" ({}) is not on this machine and was never chosen for this workspace (it came from {}), so the assistant only reads from it; choose it explicitly (K8S panel \u{2192} Cluster\u{2026}) or choose this machine's k3s", p.name, p.server, p.source.label()));
    }
    if p.source == Source::InCluster && p.can_i.contains("cluster-admin like") {
        return Some("this pod's service account can do everything in the cluster (cluster-admin), so the assistant only reads; give the account a Role limited to one namespace".to_string());
    }
    p.looks_like_production().then(|| format!("the cluster \"{}\" looks like production, so the assistant only reads from it; deploy to a development cluster (K8S panel \u{2192} Change cluster)", p.name))
}

/// Where kubeconfigs are looked for. [`Env::real`] reads the process; tests give their own.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub kubeconfig_env: Option<String>,
    pub home: PathBuf,
    pub k3s_files: Vec<PathBuf>,
    pub k3s_data_files: Vec<PathBuf>,
    /// More candidates after the usual ones: this pod's service account, vault-held kubeconfigs (`vault:<name>`).
    pub extra: Vec<(Source, PathBuf)>,
}

static MACHINE_KUBECONFIG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The kubeconfig of the cluster IYAKKI manages on this machine (`k3sdev.rs`): it is listed as "k3s on this machine" next to
/// `/etc/rancher/k3s/k3s.yaml`. `None` removes it.
pub fn register_machine_kubeconfig(p: Option<PathBuf>) {
    // the managed cluster's folder holds `kubeconfig` and `bin/k3s` side by side (`k3sdev::Layout`)
    crate::wsexec::register_managed_k3s(p.as_ref().and_then(|k| k.parent()).map(|d| d.join("bin/k3s")));
    *MACHINE_KUBECONFIG.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

pub fn machine_kubeconfig() -> Option<PathBuf> {
    MACHINE_KUBECONFIG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

impl Env {
    pub fn real() -> Env {
        let mut k3s_files: Vec<PathBuf> = vec!["/etc/rancher/k3s/k3s.yaml".into()];
        if let Some(m) = machine_kubeconfig() {
            k3s_files.push(m);
        }
        let mut extra = Vec::new();
        if let Some(p) = crate::kubeprofiles::in_cluster_kubeconfig() {
            extra.push((Source::InCluster, p));
        }
        Env { kubeconfig_env: std::env::var("KUBECONFIG").ok().filter(|s| !s.is_empty()), home: std::env::var("HOME").map(PathBuf::from).unwrap_or_default(), k3s_files, k3s_data_files: vec!["/var/lib/rancher/k3s/server/cred/admin.kubeconfig".into()], extra }
    }
}

/// The person's last choice, kept per user next to their other settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub kubeconfig: String,
    pub context: String,
}

impl Choice {
    fn path(dir: &Path, user: &str) -> PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("devcluster-{safe}.json"))
    }
    pub fn load(dir: &Path, user: &str) -> Option<Choice> {
        serde_json::from_str(&std::fs::read_to_string(Self::path(dir, user)).ok()?).ok().filter(|c: &Choice| !c.kubeconfig.is_empty())
    }
    pub fn save(&self, dir: &Path, user: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(dir, user), serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

/// Kubeconfig files this person hid in IYAKKI (`devcluster-hidden-<user>.json`): nothing is edited or deleted in the file itself.
pub fn hidden_load(dir: &Path, user: &str) -> Vec<String> {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    std::fs::read_to_string(dir.join(format!("devcluster-hidden-{safe}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn hidden_save(dir: &Path, user: &str, list: &[String]) -> Result<(), String> {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("devcluster-hidden-{safe}.json")), serde_json::to_string_pretty(list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn hide(dir: &Path, user: &str, kubeconfig: &str) -> Result<(), String> {
    let mut l = hidden_load(dir, user);
    if !l.iter().any(|x| x == kubeconfig) {
        l.push(kubeconfig.to_string());
    }
    hidden_save(dir, user, &l)
}

/// `kubectl`/`helm` arguments pinned to the workspace's cluster: any `--kubeconfig`, `--context`, `--kube-context`, `--server` the
/// caller gave is dropped and the workspace's own put first, so a plain `kubectl` never reaches `$KUBECONFIG`'s cluster.
/// None for other programs.
pub fn pin_cluster_args(program: &str, args: &[String], kubeconfig: &str, context: &str) -> Option<Vec<String>> {
    let base = program.rsplit('/').next().unwrap_or(program);
    if !matches!(base, "kubectl" | "helm") || kubeconfig.is_empty() {
        return None;
    }
    let drop_with_value = ["--kubeconfig", "--context", "--kube-context", "--server", "-s", "--kube-apiserver"];
    let mut out: Vec<String> = vec!["--kubeconfig".into(), kubeconfig.into()];
    if !context.is_empty() {
        out.push(if base == "helm" { "--kube-context".into() } else { "--context".into() });
        out.push(context.into());
    }
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if drop_with_value.contains(&a.as_str()) {
            skip = true;
            continue;
        }
        if drop_with_value.iter().any(|d| a.starts_with(&format!("{d}="))) {
            continue;
        }
        out.push(a.clone());
    }
    Some(out)
}

/// The kubeconfig files to look at, in order, without duplicates.
pub fn candidates(project: &K8sConfig, remembered: Option<&Choice>, env: &Env) -> Vec<(Source, PathBuf)> {
    let mut v: Vec<(Source, PathBuf)> = Vec::new();
    let mut add = |s: Source, p: PathBuf| {
        let key = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if !v.iter().any(|(_, q)| std::fs::canonicalize(q).unwrap_or_else(|_| q.clone()) == key) {
            v.push((s, p));
        }
    };
    if !project.kubeconfig.trim().is_empty() {
        add(Source::Project, expand(project.kubeconfig.trim(), &env.home));
    }
    if let Some(c) = remembered.filter(|c| !c.kubeconfig.is_empty()) {
        add(Source::Remembered, expand(&c.kubeconfig, &env.home));
    }
    // nothing chosen: this machine's own k3s comes first, so a $KUBECONFIG / ~/.kube/config that names some other cluster
    // never wins by default
    for p in &env.k3s_files {
        add(Source::K3s, p.clone());
    }
    for p in &env.k3s_data_files {
        add(Source::K3sData, p.clone());
    }
    if let Some(e) = &env.kubeconfig_env {
        for p in e.split(':').filter(|p| !p.is_empty()) {
            add(Source::Env, PathBuf::from(p));
        }
    }
    if !env.home.as_os_str().is_empty() {
        add(Source::UserKube, env.home.join(".kube/config"));
    }
    for (s, p) in &env.extra {
        add(*s, p.clone());
    }
    v
}

fn expand(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

/// What to do about a kubeconfig that the person cannot read.
pub fn unreadable_fix(path: &str) -> String {
    format!("{path} exists but your user cannot read it (it is owned by root). One-time fix, which you run yourself because IYAKKI never runs sudo: `sudo cp {path} ~/.kube/k3s.yaml && sudo chown $USER ~/.kube/k3s.yaml && chmod 600 ~/.kube/k3s.yaml`, or let everyone read it with `sudo chmod 644 {path}`. Then use Change cluster → Refresh.")
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect()
}

fn host_of(server: &str) -> String {
    let rest = server.split("://").nth(1).unwrap_or(server);
    let host = rest.split('/').next().unwrap_or("");
    if let Some(v6) = host.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("").to_string();
    }
    host.split(':').next().unwrap_or("").to_string()
}

/// This machine's own IPv4 addresses (Linux: `/proc/net/fib_trie` lists them as `/32 host LOCAL`).
pub fn local_ips() -> Vec<String> {
    let t = std::fs::read_to_string("/proc/net/fib_trie").unwrap_or_default();
    let (mut ips, mut last) = (Vec::new(), String::new());
    for l in t.lines() {
        let l = l.trim();
        if let Some(ip) = l.strip_prefix("|-- ") {
            last = ip.to_string();
        } else if l.contains("/32 host LOCAL") && !last.is_empty() && !ips.contains(&last) {
            ips.push(last.clone());
        }
    }
    ips
}

fn is_local_host(h: &str) -> bool {
    matches!(h, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0") || h.starts_with("127.") || std::fs::read_to_string("/etc/hostname").is_ok_and(|n| !h.is_empty() && n.trim() == h) || (!h.is_empty() && local_ips().iter().any(|ip| ip == h))
}

/// Look at one kubeconfig: readable? which context and server? does the API answer? is it k3s?
pub fn inspect(run: &Arc<dyn Runner>, path: &Path, source: Source, context: &str, test: bool) -> ClusterProfile {
    let ps = path.display().to_string();
    let mut p = ClusterProfile::empty(&ps, source);
    let is_k3s_path = |s: &str| s.starts_with("/etc/rancher/k3s/") || s.starts_with("/var/lib/rancher/k3s/") || machine_kubeconfig().is_some_and(|m| m.display().to_string() == s);
    if is_k3s_path(&ps) {
        p.name = "k3s".into();
        p.k3s = true;
    }
    let held_in_vault = crate::kubeprofiles::is_vault_ref(&ps);
    // a vault reference is not a file: it is looked up (and written to a temp file) only when a program is started with it
    let meta = if held_in_vault { std::fs::metadata("/") } else { std::fs::metadata(path) };
    match meta {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            p.exists = false;
            p.problem = format!("{ps} does not exist");
            return p;
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            p.problem = format!("{ps} cannot be read (permission denied)");
            p.fix = unreadable_fix(&ps);
            return p;
        }
        _ => {}
    }
    if !held_in_vault && std::fs::File::open(path).is_err() {
        p.problem = format!("{ps} cannot be read (permission denied)");
        p.fix = unreadable_fix(&ps);
        return p;
    }
    p.readable = true;
    let in_k3s_path = is_k3s_path(&ps);
    let kc = Kc::new(run.clone(), &ps, context);
    let cur = kc.kubectl(&["config", "current-context"], 3, 8);
    p.context = if !context.is_empty() { context.to_string() } else if cur.ok() { cur.stdout.trim().to_string() } else { String::new() };
    if cur.missing {
        p.problem = "kubectl is not installed".into();
        p.fix = "Install kubectl (or use `k3s kubectl`); IYAKKI only needs the kubectl program.".into();
        return p;
    }
    if p.context.is_empty() {
        p.problem = format!("no current context in {ps}: {}", first_line(&cur.combined()));
    }
    let srv = kc.kubectl(&["config", "view", "--minify", "-o", "jsonpath={.clusters[0].cluster.server}"], 3, 8);
    if srv.ok() {
        p.server = srv.stdout.trim().to_string();
    }
    p.local = is_local_host(&host_of(&p.server)) || (p.server.is_empty() && in_k3s_path);
    p.k3s = in_k3s_path;
    p.name = if p.k3s { "k3s".into() } else if !p.context.is_empty() { p.context.clone() } else { "cluster".into() };
    if !test {
        return p;
    }
    p.tested = true;
    let ver = kc.kubectl(&["version", "-o", "json"], REACH_TIMEOUT_SECS, REACH_TIMEOUT_SECS + 5);
    if let Ok(v) = serde_json::from_str::<Value>(&ver.stdout) {
        let git = v["serverVersion"]["gitVersion"].as_str().unwrap_or("").to_string();
        if !git.is_empty() {
            p.reachable = true;
            p.k3s |= git.contains("k3s");
            p.version = git;
        }
    }
    if !p.reachable {
        // an old client prints no JSON on failure; ask for something simple before giving up
        let ns = kc.kubectl(&["get", "namespaces", "-o", "name"], REACH_TIMEOUT_SECS, REACH_TIMEOUT_SECS + 5);
        if ns.ok() {
            p.reachable = true;
        } else {
            let err = if ver.stderr.trim().is_empty() { ns.combined() } else { ver.stderr.clone() };
            let l = first_line(&err);
            p.problem = if l.is_empty() { "the cluster did not answer".into() } else { l.clone() };
            if l.contains("refused") || l.contains("timeout") || l.contains("deadline") || ver.timed_out {
                p.fix = format!("The API server at {} did not answer. If this is k3s, check `systemctl status k3s` (IYAKKI will not start or restart it for you); for a remote cluster check the VPN or SSH tunnel.", if p.server.is_empty() { "the server address" } else { &p.server });
            } else if l.contains("Unauthorized") || l.contains("forbidden") || l.contains("Forbidden") {
                p.fix = "The cluster answered but rejected these credentials: refresh the kubeconfig or pick another cluster.".into();
            }
        }
    }
    if p.k3s {
        p.name = "k3s".into();
    }
    p
}

/// What the identity of a profile may do in `namespace`: three `kubectl auth can-i` questions.
pub fn can_i_summary(run: &Arc<dyn Runner>, p: &ClusterProfile, namespace: &str) -> String {
    // this pod's service account: what its Role allows in the namespace (`auth can-i --list`)
    if p.source == Source::InCluster {
        return crate::kubeprofiles::can_i_text(&crate::kubeprofiles::can_i_list(run, p, namespace));
    }
    let kc = Kc::new(run.clone(), &p.kubeconfig, &p.context);
    let ask = |verb: &str, what: &str, ns: Option<&str>| -> &'static str {
        let mut a = vec!["auth", "can-i", verb, what];
        if let Some(n) = ns {
            a.push("-n");
            a.push(n);
        }
        let o = kc.kubectl(&a, 3, 8);
        if o.stdout.trim() == "yes" { "yes" } else { "no" }
    };
    format!("get pods {} · create deployments {} · create namespaces {}", ask("get", "pods", Some(namespace)), ask("create", "deployments", Some(namespace)), ask("create", "namespaces", None))
}

#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub profiles: Vec<ClusterProfile>,
    pub chosen: Option<usize>,
    pub note: String,
    /// Kubeconfig files the person hid (IYAKKI-side only): never probed, never listed, never used.
    pub hidden: Vec<String>,
    /// Nothing was chosen for this workspace and more than one cluster answers: the person should choose.
    pub ambiguous: bool,
}

impl Resolution {
    pub fn chosen(&self) -> Option<&ClusterProfile> {
        self.chosen.and_then(|i| self.profiles.get(i))
    }
    pub fn reachable(&self) -> Option<&ClusterProfile> {
        self.chosen().filter(|p| p.reachable)
    }
    /// What the K8S panel and the chat say when nothing can be used.
    pub fn explain(&self) -> String {
        if let Some(p) = self.chosen() {
            if p.reachable {
                return format!("Connected: {}", p.describe());
            }
        }
        let mut t = String::new();
        for p in &self.profiles {
            if !p.fix.is_empty() {
                t.push_str(&p.fix);
                t.push('\n');
            }
        }
        if t.is_empty() {
            t = if self.profiles.is_empty() { "No kubeconfig was found (looked in $KUBECONFIG, ~/.kube/config and /etc/rancher/k3s/k3s.yaml). Point this workspace at one in Settings, or install a cluster yourself.".into() } else { "No cluster answered.".into() };
        }
        t.trim_end().to_string()
    }
}

/// Look through the candidates and pick the first that answers. With `test_all` every readable one is tested (for the
/// chooser); otherwise testing stops at the first that answers.
pub fn resolve(run: &Arc<dyn Runner>, env: &Env, project: &K8sConfig, remembered: Option<&Choice>, namespace: &str, test_all: bool) -> Resolution {
    resolve_hiding(run, env, project, remembered, namespace, test_all, &[])
}

fn hidden_has(hidden: &[String], path: &Path) -> bool {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    hidden.iter().any(|h| Path::new(h) == path || std::fs::canonicalize(h).unwrap_or_else(|_| PathBuf::from(h)) == key)
}

/// [`resolve`] without the kubeconfig files in `hidden`: they are skipped before anything is run on them, so no call reaches
/// their servers and no tool can use them.
pub fn resolve_hiding(run: &Arc<dyn Runner>, env: &Env, project: &K8sConfig, remembered: Option<&Choice>, namespace: &str, test_all: bool, hidden: &[String]) -> Resolution {
    let mut res = Resolution::default();
    for (src, path) in candidates(project, remembered, env) {
        if hidden_has(hidden, &path) {
            res.hidden.push(path.display().to_string());
            if matches!(src, Source::Project | Source::Remembered) {
                res.note = format!("{} is hidden in IYAKKI (K8S panel → Cluster… → Show hidden clusters brings it back), so it is not used.", path.display());
            }
            continue;
        }
        let context = match src {
            Source::Project => project.profile.trim(),
            Source::Remembered => remembered.map_or("", |c| c.context.as_str()),
            _ => "",
        };
        // an explicit choice ends the search; a default is only a default when it is the one cluster that answers
        let explicit_chosen = res.chosen.and_then(|i| res.profiles.get(i)).is_some_and(|p| matches!(p.source, Source::Project | Source::Remembered));
        let done = res.chosen.is_some() && !test_all && explicit_chosen;
        let p = inspect(run, &path, src, context, !done);
        if !p.exists {
            // an explicit choice that has gone is worth saying; the usual places are not
            if matches!(src, Source::Project | Source::Remembered) {
                res.note = format!("{}", p.problem);
            }
            continue;
        }
        // a cluster the person picked (this workspace's, or their own earlier choice) stays chosen even when it does not
        // answer: falling back to another cluster silently could deploy somewhere they did not mean
        let explicit = matches!(src, Source::Project | Source::Remembered) && p.readable;
        res.profiles.push(p);
        if res.chosen.is_none() && (explicit || res.profiles.last().is_some_and(|p| p.reachable)) {
            res.chosen = Some(res.profiles.len() - 1);
        }
    }
    let explicit_chosen = res.chosen().is_some_and(|p| matches!(p.source, Source::Project | Source::Remembered));
    res.ambiguous = !explicit_chosen && res.profiles.iter().filter(|p| p.reachable).count() > 1;
    if res.chosen.is_none() && !res.profiles.is_empty() {
        // none answers: show the first one that can be read, else the first
        res.chosen = Some(res.profiles.iter().position(|p| p.readable).unwrap_or(0));
    }
    if let Some(i) = res.chosen {
        if res.profiles[i].reachable {
            let s = can_i_summary(run, &res.profiles[i], namespace);
            res.profiles[i].can_i = s;
        }
    }
    res
}

/// A resolution shared between the thread that computes it and the panels and tools that read it.
#[derive(Default)]
pub struct Shared {
    pub resolution: Option<Resolution>,
    pub resolving: bool,
    pub generation: u64,
}

pub type Slot = Arc<Mutex<Shared>>;

pub fn slot() -> Slot {
    Arc::new(Mutex::new(Shared::default()))
}

/// Compute a resolution on a background thread into `slot` (the K8S panel never waits on `kubectl`).
pub fn resolve_in_background(slot: &Slot, run: Arc<dyn Runner>, env: Env, project: K8sConfig, remembered: Option<Choice>, namespace: String, test_all: bool, hidden: Vec<String>) {
    let gen = {
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        g.resolving = true;
        g.generation += 1;
        g.generation
    };
    let slot = slot.clone();
    std::thread::spawn(move || {
        let r = resolve_hiding(&run, &env, &project, remembered.as_ref(), &namespace, test_all, &hidden);
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.generation == gen {
            if let Some(p) = r.reachable() {
                set_active(Some(&p.describe()));
            } else {
                set_active(None);
            }
            // the AI hardware profile reads the GPUs of a remote cluster that answers (aiprofile_remote.rs)
            crate::aiprofile_remote::register_target(r.reachable().and_then(crate::aiprofile_remote::target_of));
            g.resolution = Some(r);
            g.resolving = false;
        }
    });
}

// ------------------------------------------------------------------------------------------------ never start a second cluster

static ACTIVE: Mutex<Option<String>> = Mutex::new(None);

/// The connected cluster, as the block message names it (`None`: none is connected).
pub fn set_active(desc: Option<&str>) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = desc.map(str::to_string);
}

pub fn active() -> Option<String> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The words of the refusal.
pub fn block_message(what: &str, connected: Option<&str>) -> String {
    match connected {
        Some(c) => format!("Refused: {what}. A cluster is already running and connected ({c}). IYAKKI will not start another one here. Do not try again. To deploy: write the app and a Dockerfile in the workspace, then call deploy_build, deploy_load and k8s_apply (a manifest in deploy/), and check with k8s_status and k8s_logs. They already use this cluster."),
        None => format!("Refused: {what}. This is a desktop window on a machine that manages its own cluster, and IYAKKI will not start, stop or install one here. Connect to the existing one (K8S panel → Change cluster) or ask the person to set one up. Do not try to start one."),
    }
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn unit_is_cluster(u: &str) -> bool {
    let u = u.trim_end_matches(".service");
    matches!(u, "rke2-server" | "rke2-agent" | "microk8s") || u.starts_with("k3s")
}

fn nonflag(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect()
}

/// What this command would do to a cluster, if anything: `Some("start a k3s server")`. Pure: it only reads the words.
pub fn cluster_op(program: &str, args: &[String]) -> Option<String> {
    let prog = base(program);
    let words = nonflag(args);
    match prog {
        // wrappers: look at what they run
        "sudo" | "doas" | "nohup" | "setsid" | "nice" | "time" | "stdbuf" | "ionice" | "chroot" => {
            let i = args.iter().position(|a| !a.starts_with('-') && !a.contains('='))?;
            cluster_op(&args[i], &args[i + 1..])
        }
        "env" => {
            let i = args.iter().position(|a| !a.starts_with('-') && !a.contains('='))?;
            cluster_op(&args[i], &args[i + 1..])
        }
        "timeout" => {
            let i = args.iter().position(|a| !a.starts_with('-'))?;
            let rest = args.get(i + 1..)?;
            let j = rest.iter().position(|a| !a.starts_with('-'))?;
            cluster_op(&rest[j], &rest[j + 1..])
        }
        "sh" | "bash" | "zsh" | "dash" | "ash" => {
            let i = args.iter().position(|a| a == "-c" || (a.starts_with('-') && a.ends_with('c') && a.len() <= 4))?;
            shell_line_op(args.get(i + 1)?)
        }
        "k3s" => match words.first().copied() {
            Some("server") | Some("agent") => Some(format!("start a k3s {}", words[0])),
            Some("etcd-snapshot") if words.get(1) == Some(&"restore") => Some("restore k3s from a snapshot".into()),
            _ if args.iter().any(|a| a == "--cluster-reset") => Some("reset the k3s cluster".into()),
            _ => None,
        },
        "k3s-killall.sh" | "k3s-agent-killall.sh" => Some("stop all k3s processes".into()),
        "k3s-uninstall.sh" | "k3s-agent-uninstall.sh" => Some("uninstall k3s".into()),
        "rke2" => matches!(words.first().copied(), Some("server") | Some("agent")).then(|| "start an rke2 cluster".to_string()),
        "snap" => {
            let v = words.first().copied();
            (matches!(v, Some("start") | Some("stop") | Some("restart") | Some("remove") | Some("install")) && words.iter().skip(1).any(|w| w.starts_with("microk8s") || w.starts_with("k3s"))).then(|| "change a snap-installed cluster".to_string())
        }
        "systemctl" | "service" | "rc-service" => {
            let verbs = ["start", "stop", "restart", "reload", "try-restart", "reload-or-restart", "kill", "disable", "enable", "mask", "reset-failed", "set-property", "daemon-reexec"];
            let verb = if prog == "systemctl" { words.first().copied() } else { words.get(1).copied() };
            let unit = words.iter().find(|u| unit_is_cluster(u)).copied();
            (verb.is_some_and(|v| verbs.contains(&v)) && unit.is_some()).then(|| format!("{} the cluster service {}", verb.unwrap_or(""), unit.unwrap_or("")))
        }
        "kubeadm" => matches!(words.first().copied(), Some("init") | Some("join") | Some("reset") | Some("upgrade")).then(|| format!("run kubeadm {}", words[0])),
        "minikube" => matches!(words.first().copied(), Some("start") | Some("stop") | Some("delete") | Some("pause") | Some("unpause")).then(|| format!("run minikube {}", words[0])),
        "kind" => (words.first() == Some(&"create") || words.first() == Some(&"delete")).then(|| format!("{} a kind cluster", words[0])),
        "k3d" => (words.first() == Some(&"cluster") && matches!(words.get(1).copied(), Some("create") | Some("delete") | Some("start") | Some("stop"))).then(|| format!("{} a k3d cluster", words[1])),
        "microk8s" => matches!(words.first().copied(), Some("start") | Some("stop") | Some("reset") | Some("install") | Some("enable") | Some("disable")).then(|| format!("run microk8s {}", words[0])),
        "docker" | "podman" | "nerdctl" => {
            let runs = matches!(words.first().copied(), Some("run") | Some("create") | Some("start") | Some("compose")) || (words.first() == Some(&"container") && matches!(words.get(1).copied(), Some("run") | Some("create")));
            let image_is_cluster = args.iter().any(|a| {
                let a = a.to_lowercase();
                a.contains("rancher/k3s") || a.contains("rancher/rke2") || a.contains("kindest/node") || a.contains("gcr.io/k8s-minikube") || a.starts_with("k3s:") || a == "k3s"
            });
            (runs && image_is_cluster).then(|| format!("run a cluster node in a {prog} container"))
        }
        "curl" | "wget" | "fetch" => args.iter().any(|a| a.contains("get.k3s.io") || a.contains("get.rke2.io") || a.contains("rancher/k3s/releases")).then(|| "download the k3s installer".to_string()),
        "bash-install-k3s" => Some("install k3s".into()),
        _ => None,
    }
}

/// The same for a shell line (the IDE's bottom terminal and `terminal`): every `;`, `&&`, `||`, `|` and `$( )` part.
pub fn shell_line_op(line: &str) -> Option<String> {
    if line.contains("INSTALL_K3S") || line.contains("get.k3s.io") {
        return Some("install k3s".into());
    }
    let cleaned = line.replace("$(", ";").replace('`', ";").replace(['(', ')'], ";");
    for seg in cleaned.split([';', '|', '&', '\n']) {
        let toks = crate::wsexec::split_command(seg);
        let Some(i) = toks.iter().position(|t| !t.contains('=') || t.starts_with('/') || t.starts_with('.')) else { continue };
        if let Some(op) = cluster_op(&toks[i], &toks[i + 1..]) {
            return Some(op);
        }
    }
    None
}

/// The rule itself, with the state passed in so it can be tested: refuse when a cluster is connected or this is a desktop window.
pub fn guard_with(window: bool, connected: Option<&str>, program: &str, args: &[String]) -> Result<(), String> {
    if !window && connected.is_none() {
        return Ok(());
    }
    match cluster_op(program, args) {
        Some(what) => Err(block_message(&what, connected)),
        None => Ok(()),
    }
}

/// [`guard_with`] with the process's own state: window mode (`webengine::host_session()`) and the connected cluster.
pub fn guard(program: &str, args: &[String]) -> Result<(), String> {
    guard_with(crate::webengine::host_session(), active().as_deref(), program, args)
}

/// For a shell line.
pub fn guard_line(line: &str) -> Result<(), String> {
    guard_line_with(crate::webengine::host_session(), active().as_deref(), line)
}

pub fn guard_line_with(window: bool, connected: Option<&str>, line: &str) -> Result<(), String> {
    if !window && connected.is_none() {
        return Ok(());
    }
    match shell_line_op(line) {
        Some(what) => Err(block_message(&what, connected)),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsexec::FakeRunner;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-devcluster-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn kubeconfig(d: &Path, name: &str) -> PathBuf {
        let p = d.join(name);
        std::fs::write(&p, "apiVersion: v1\nkind: Config\n").unwrap();
        p
    }

    const VERSION_K3S: &str = r#"{"clientVersion":{"gitVersion":"v1.36.5+k3s1"},"serverVersion":{"gitVersion":"v1.36.5+k3s1"}}"#;

    /// A runner that knows two clusters by kubeconfig file name.
    fn runner(good: &[&str], bad: &[&str]) -> Arc<FakeRunner> {
        let r = FakeRunner::new(&["kubectl", "podman", "docker", "k3s"]);
        for g in good {
            r.answer(&format!("--kubeconfig {g} --request-timeout=3s version"), VERSION_K3S);
            r.answer(&format!("--kubeconfig {g} --request-timeout=3s config current-context"), "default\n");
            r.answer(&format!("--kubeconfig {g} --request-timeout=3s config view"), "https://127.0.0.1:6443");
        }
        for b in bad {
            r.answer(&format!("--kubeconfig {b} --request-timeout=3s config current-context"), "remote-ctx\n");
            r.answer(&format!("--kubeconfig {b} --request-timeout=3s config view"), "https://10.9.9.9:6443");
            r.fail(&format!("--kubeconfig {b} --request-timeout=3s version"), 1, "Unable to connect to the server: dial tcp 10.9.9.9:6443: connect: connection refused");
            r.fail(&format!("--kubeconfig {b} --request-timeout=3s get namespaces"), 1, "Unable to connect to the server: dial tcp 10.9.9.9:6443: connect: connection refused");
        }
        r.answer("auth can-i create namespaces", "yes\n");
        r.answer("auth can-i", "yes\n");
        Arc::new(r)
    }

    fn as_dyn(r: &Arc<FakeRunner>) -> Arc<dyn Runner> {
        r.clone()
    }

    #[test]
    fn a_reachable_k3s_is_found_tested_and_chosen_with_the_kubeconfig_in_the_environment() {
        let d = tmp("reach");
        let k3s = kubeconfig(&d, "k3s.yaml");
        let env = Env { kubeconfig_env: None, home: d.clone(), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
        let r = runner(&[&k3s.display().to_string()], &[]);
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), None, "shop-dev", false);
        let p = res.reachable().expect("reachable");
        assert_eq!((p.name.as_str(), p.context.as_str(), p.reachable, p.k3s, p.local), ("k3s", "default", true, true, true), "{p:?}");
        assert_eq!(p.chip(), "k3s · default · reachable");
        assert!(p.can_i.contains("create deployments yes"), "{}", p.can_i);
        assert_eq!(p.server, "https://127.0.0.1:6443");
        // every call named the kubeconfig in the environment, none ran a shell or sudo
        for c in r.calls() {
            assert_eq!(c.argv[0], "kubectl", "{:?}", c.argv);
            assert!(c.env.iter().any(|(k, v)| k == "KUBECONFIG" && v == &k3s.display().to_string()), "{c:?}");
        }
        assert!(!r.lines().iter().any(|l| l.contains("sudo")));
    }

    #[test]
    fn candidates_come_in_order_without_duplicates_and_the_workspace_choice_wins() {
        let d = tmp("order");
        let (a, b, c) = (kubeconfig(&d, "env.yaml"), kubeconfig(&d, "k3s.yaml"), kubeconfig(&d, "project.yaml"));
        std::fs::create_dir_all(d.join(".kube")).unwrap();
        let home_cfg = kubeconfig(&d.join(".kube"), "config");
        let env = Env { kubeconfig_env: Some(format!("{}:{}", a.display(), home_cfg.display())), home: d.clone(), k3s_files: vec![b.clone(), a.clone()], k3s_data_files: vec![], ..Default::default() };
        let order: Vec<(Source, PathBuf)> = candidates(&K8sConfig::default(), None, &env);
        assert_eq!(order.iter().map(|x| x.0).collect::<Vec<_>>(), vec![Source::K3s, Source::K3s, Source::Env], "this machine's k3s comes before $KUBECONFIG; ~/.kube/config is the same file as the second $KUBECONFIG entry: {order:?}");
        let project = K8sConfig { kubeconfig: c.display().to_string(), profile: "mine".into(), ..Default::default() };
        let order = candidates(&project, Some(&Choice { kubeconfig: b.display().to_string(), context: String::new() }), &env);
        assert_eq!(order[0], (Source::Project, c.clone()));
        assert_eq!(order[1], (Source::Remembered, b.clone()), "the person's earlier choice is next");
        // a tilde in project.json is the home folder
        let project = K8sConfig { kubeconfig: "~/.kube/config".into(), ..Default::default() };
        assert_eq!(candidates(&project, None, &env)[0].1, d.join(".kube/config"));
    }

    #[test]
    fn an_unreachable_cluster_is_reported_and_the_next_candidate_is_used() {
        let d = tmp("unreach");
        let (remote, k3s) = (kubeconfig(&d, "remote.yaml"), kubeconfig(&d, "k3s.yaml"));
        let env = Env { kubeconfig_env: Some(remote.display().to_string()), home: d.clone(), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
        let r = runner(&[&k3s.display().to_string()], &[&remote.display().to_string()]);
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), None, "x-dev", true);
        assert_eq!(res.profiles.len(), 2);
        let first = &res.profiles[1];
        assert!(!first.reachable && first.tested && first.problem.contains("refused") && first.fix.contains("did not answer"), "{first:?}");
        assert_eq!(first.chip(), "remote-ctx · remote-ctx · unreachable");
        assert_eq!(res.chosen, Some(0), "the k3s file answers and comes first");
        assert!(res.reachable().is_some_and(|p| p.k3s));
        assert!(!res.ambiguous, "only one cluster answers");
        // when nothing answers, the explanation has the first fix
        let only_bad = runner(&[], &[&remote.display().to_string()]);
        let env2 = Env { kubeconfig_env: Some(remote.display().to_string()), home: d.join("nohome"), k3s_files: vec![], k3s_data_files: vec![], ..Default::default() };
        let res = resolve(&as_dyn(&only_bad), &env2, &K8sConfig::default(), None, "x-dev", false);
        assert!(res.reachable().is_none());
        assert!(res.explain().contains("systemctl status k3s") || res.explain().contains("did not answer"), "{}", res.explain());
    }

    #[test]
    fn the_workspaces_own_choice_stays_even_when_it_does_not_answer() {
        let d = tmp("sticky");
        let (remote, k3s) = (kubeconfig(&d, "remote.yaml"), kubeconfig(&d, "k3s.yaml"));
        let env = Env { kubeconfig_env: None, home: d.join("nohome"), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
        let r = runner(&[&k3s.display().to_string()], &[&remote.display().to_string()]);
        let project = K8sConfig { kubeconfig: remote.display().to_string(), ..Default::default() };
        let res = resolve(&as_dyn(&r), &env, &project, None, "x-dev", false);
        let p = res.chosen().expect("chosen");
        assert!(p.kubeconfig.ends_with("remote.yaml") && !p.reachable && res.reachable().is_none(), "no silent fallback: {p:?}");
        assert!(res.explain().contains("did not answer"));
        // the same for the person's earlier choice
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), Some(&Choice { kubeconfig: remote.display().to_string(), context: String::new() }), "x-dev", false);
        assert!(res.chosen().is_some_and(|p| p.kubeconfig.ends_with("remote.yaml")));
        // asking to test everything still lists the others, so the chooser can offer the one that works
        let res = resolve(&as_dyn(&r), &env, &project, None, "x-dev", true);
        assert_eq!(res.profiles.len(), 2);
        assert!(res.profiles[1].reachable && res.chosen == Some(0));
    }

    #[test]
    fn an_explicit_choice_ends_the_search_a_default_tests_the_rest_and_two_answers_are_ambiguous() {
        let d = tmp("lazy");
        let (a, b) = (kubeconfig(&d, "a.yaml"), kubeconfig(&d, "b.yaml"));
        let env = Env { kubeconfig_env: Some(format!("{}:{}", a.display(), b.display())), home: d.clone(), k3s_files: vec![], k3s_data_files: vec![], ..Default::default() };
        let both = [a.display().to_string(), b.display().to_string()];
        // the workspace named a: b is never run
        let r = runner(&[&both[0], &both[1]], &[]);
        let project = K8sConfig { kubeconfig: both[0].clone(), ..Default::default() };
        let res = resolve(&as_dyn(&r), &env, &project, None, "x-dev", false);
        assert_eq!(res.chosen, Some(0));
        assert!(res.profiles[0].tested && !res.profiles[1].tested && !res.ambiguous);
        assert_eq!(res.profiles[1].state(), "not tested");
        assert!(!r.lines().iter().any(|l| l.contains(&both[1]) && l.contains(" version")), "{:?}", r.lines());
        // nothing chosen and both answer: the first is only a default, the person is asked
        let r2 = runner(&[&both[0], &both[1]], &[]);
        let res = resolve(&as_dyn(&r2), &env, &K8sConfig::default(), None, "x-dev", false);
        assert!(res.profiles.iter().all(|p| p.tested && p.reachable));
        assert!(res.ambiguous && res.chosen == Some(0));
    }

    #[test]
    fn a_root_only_kubeconfig_is_explained_with_the_one_time_fix_and_nothing_is_run() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("unreadable");
        let locked = kubeconfig(&d, "k3s.yaml");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root can read anything: the test is only meaningful for an ordinary user
        if std::fs::File::open(&locked).is_ok() {
            return;
        }
        let env = Env { kubeconfig_env: None, home: d.join("nohome"), k3s_files: vec![locked.clone()], k3s_data_files: vec![], ..Default::default() };
        let r = runner(&[], &[]);
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), None, "x-dev", false);
        let p = &res.profiles[0];
        assert!(!p.readable && !p.reachable && p.state() == "not readable", "{p:?}");
        assert!(p.fix.contains("sudo cp") && p.fix.contains("chmod 644") && p.fix.contains("never runs sudo"), "{}", p.fix);
        assert!(res.explain().contains("sudo chmod 644"));
        assert!(r.calls().is_empty(), "an unreadable file is not handed to kubectl");
        // and the program never runs sudo itself
        assert!(!r.lines().iter().any(|l| l.starts_with("sudo")));
    }

    #[test]
    fn a_missing_kubectl_and_a_missing_remembered_file_are_said_plainly() {
        let d = tmp("nokubectl");
        let f = kubeconfig(&d, "k3s.yaml");
        let env = Env { kubeconfig_env: None, home: d.join("nohome"), k3s_files: vec![f], k3s_data_files: vec![], ..Default::default() };
        let run: Arc<dyn Runner> = Arc::new(crate::wsexec::System);
        // the system runner with a PATH that has no kubectl is not used here: ask the fake to behave as "not found"
        let _ = run;
        let r = Arc::new(FakeRunner::new(&[]));
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), Some(&Choice { kubeconfig: d.join("gone.yaml").display().to_string(), context: String::new() }), "x-dev", false);
        assert!(res.note.contains("does not exist"), "{:?}", res.note);
        assert!(res.reachable().is_none());
        // no candidates at all
        let none = Env { kubeconfig_env: None, home: d.join("nohome"), k3s_files: vec![], k3s_data_files: vec![], ..Default::default() };
        let res = resolve(&as_dyn(&r), &none, &K8sConfig::default(), None, "x-dev", false);
        assert!(res.explain().contains("No kubeconfig was found"));
    }

    #[test]
    fn the_choice_is_remembered_per_user() {
        let d = tmp("choice");
        let c = Choice { kubeconfig: "/home/a/.kube/k3s.yaml".into(), context: "default".into() };
        c.save(&d, "alice").unwrap();
        assert_eq!(Choice::load(&d, "alice"), Some(c.clone()));
        assert_eq!(Choice::load(&d, "bob"), None, "another user does not see it");
        assert_eq!(Choice::path(&d, "../../etc/x").parent(), Some(d.as_path()), "the user name cannot escape the folder");
    }

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_command_guard_blocks_what_starts_stops_or_installs_a_cluster_and_allows_the_rest() {
        let connected = Some("k3s (context default, https://127.0.0.1:6443, k3s, kubeconfig /etc/rancher/k3s/k3s.yaml)");
        let blocked: &[(&str, &[&str])] = &[
            ("k3s", &["server"]),
            ("k3s", &["server", "--rootless"]),
            ("/usr/local/bin/k3s", &["agent", "--server", "https://x:6443"]),
            ("k3s", &["--debug", "server"]),
            ("k3s-killall.sh", &[]),
            ("/usr/local/bin/k3s-uninstall.sh", &[]),
            ("systemctl", &["restart", "k3s"]),
            ("systemctl", &["stop", "k3s.service"]),
            ("systemctl", &["--user", "start", "k3s"]),
            ("systemctl", &["disable", "--now", "k3s"]),
            ("service", &["k3s", "restart"]),
            ("sudo", &["systemctl", "start", "k3s"]),
            ("sudo", &["-E", "k3s", "server"]),
            ("env", &["A=b", "k3s", "server"]),
            ("timeout", &["10", "k3s", "server"]),
            ("bash", &["-c", "k3s server &"]),
            ("sh", &["-c", "curl -sfL https://get.k3s.io | sh -"]),
            ("curl", &["-sfL", "https://get.k3s.io"]),
            ("kubeadm", &["init"]),
            ("kubeadm", &["reset", "-f"]),
            ("minikube", &["start"]),
            ("minikube", &["delete"]),
            ("kind", &["create", "cluster"]),
            ("kind", &["delete", "cluster"]),
            ("k3d", &["cluster", "create", "dev"]),
            ("microk8s", &["start"]),
            ("docker", &["run", "-d", "--privileged", "rancher/k3s:v1.30.0", "server"]),
            ("podman", &["run", "kindest/node:v1.30"]),
        ];
        for (prog, args) in blocked {
            let e = guard_with(true, connected, prog, &a(args)).expect_err(&format!("{prog} {args:?} must be blocked"));
            assert!(e.contains("already running and connected") && e.contains("will not start another one") && e.contains("deploy_build"), "{e}");
            assert!(e.contains("https://127.0.0.1:6443"), "the message names the profile: {e}");
            // a connected cluster blocks even outside a desktop window
            assert!(guard_with(false, connected, prog, &a(args)).is_err());
            // a desktop window with nothing connected blocks too, with its own words
            let e2 = guard_with(true, None, prog, &a(args)).unwrap_err();
            assert!(e2.contains("will not start, stop or install"), "{e2}");
            // on the image, with nothing connected, the rule does not apply
            assert!(guard_with(false, None, prog, &a(args)).is_ok());
        }
        let allowed: &[(&str, &[&str])] = &[
            ("kubectl", &["get", "pods"]),
            ("kubectl", &["apply", "-f", "deploy.yaml"]),
            ("k3s", &["kubectl", "get", "pods"]),
            ("k3s", &["ctr", "images", "ls"]),
            ("k3s", &["ctr", "images", "import", "/tmp/a.tar"]),
            ("k3s", &["crictl", "ps"]),
            ("k3s", &["--version"]),
            ("systemctl", &["status", "k3s"]),
            ("systemctl", &["is-active", "k3s"]),
            ("systemctl", &["restart", "nginx"]),
            ("journalctl", &["-u", "k3s"]),
            ("podman", &["build", "-t", "x", "."]),
            ("podman", &["run", "--rm", "alpine", "echo", "hi"]),
            ("docker", &["ps"]),
            ("kind", &["get", "clusters"]),
            ("minikube", &["status"]),
            ("curl", &["-s", "https://example.com"]),
            ("sudo", &["ls"]),
            ("bash", &["-c", "kubectl get pods | head"]),
        ];
        for (prog, args) in allowed {
            assert!(guard_with(true, connected, prog, &a(args)).is_ok(), "{prog} {args:?} must be allowed");
        }
        // shell lines (the terminal panel)
        for line in ["k3s server", "sudo systemctl restart k3s", "echo hi; k3s server --rootless", "kubectl get pods && minikube start", "FOO=1 k3s server", "curl -sfL https://get.k3s.io | INSTALL_K3S_EXEC=server sh -", "$(k3s server)"] {
            assert!(guard_line_with(true, connected, line).is_err(), "{line}");
        }
        for line in ["kubectl get pods", "k3s kubectl get pods -A", "k3s ctr images ls", "ls -la | grep k3s", "systemctl status k3s", "echo 'k3s server' > /dev/null"] {
            let r = guard_line_with(true, connected, line);
            if line.starts_with("echo 'k3s server'") {
                // text inside quotes that is only an argument of echo is not a command
                assert!(r.is_ok(), "{line}: {r:?}");
            } else {
                assert!(r.is_ok(), "{line}: {r:?}");
            }
        }
    }

    #[test]
    fn a_default_prefers_this_machines_k3s_over_kubeconfig_env() {
        let d = tmp("prefer");
        let (remote, k3s) = (kubeconfig(&d, "remote.yaml"), kubeconfig(&d, "k3s.yaml"));
        let env = Env { kubeconfig_env: Some(remote.display().to_string()), home: d.join("nohome"), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
        let r = runner(&[&k3s.display().to_string(), &remote.display().to_string()], &[]);
        let res = resolve(&as_dyn(&r), &env, &K8sConfig::default(), None, "x-dev", false);
        assert!(res.chosen().is_some_and(|p| p.source == Source::K3s), "the local k3s is the default");
        // a workspace choice of the local file wins even though $KUBECONFIG names the remote; nothing else is run
        let project = K8sConfig { kubeconfig: k3s.display().to_string(), ..Default::default() };
        let r2 = runner(&[&k3s.display().to_string()], &[&remote.display().to_string()]);
        let res = resolve(&as_dyn(&r2), &env, &project, None, "x-dev", false);
        assert!(res.chosen().is_some_and(|p| p.source == Source::Project && p.reachable) && !res.ambiguous);
        assert!(!r2.lines().iter().any(|l| l.contains("remote.yaml") && (l.contains(" version") || l.contains(" get ") || l.contains("auth"))), "the remote's API is never called: {:?}", r2.lines());
    }

    #[test]
    fn hidden_clusters_are_never_probed_listed_or_used_and_hiding_is_reversible() {
        let d = tmp("hide");
        let (remote, k3s) = (kubeconfig(&d, "remote.yaml"), kubeconfig(&d, "k3s.yaml"));
        let env = Env { kubeconfig_env: Some(remote.display().to_string()), home: d.join("nohome"), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
        let (rs, ks) = (remote.display().to_string(), k3s.display().to_string());
        hide(&d, "alice", &rs).unwrap();
        hide(&d, "alice", &rs).unwrap();
        assert_eq!(hidden_load(&d, "alice"), vec![rs.clone()], "once, per user");
        assert!(hidden_load(&d, "bob").is_empty());
        let r = runner(&[&ks, &rs], &[]);
        let res = resolve_hiding(&as_dyn(&r), &env, &K8sConfig::default(), None, "x-dev", true, &hidden_load(&d, "alice"));
        assert_eq!(res.profiles.len(), 1);
        assert_eq!(res.hidden, vec![rs.clone()]);
        assert!(!res.ambiguous);
        assert!(!r.lines().iter().any(|l| l.contains("remote.yaml")), "no call at all names the hidden cluster: {:?}", r.lines());
        // even when the workspace names it, it is skipped (with a reason)
        let project = K8sConfig { kubeconfig: rs.clone(), ..Default::default() };
        let r2 = runner(&[&ks, &rs], &[]);
        let res = resolve_hiding(&as_dyn(&r2), &env, &project, None, "x-dev", false, &[rs.clone()]);
        assert!(res.note.contains("hidden") && res.chosen().is_some_and(|p| p.kubeconfig == ks));
        assert!(!r2.lines().iter().any(|l| l.contains("remote.yaml")));
        // show hidden: back in the list
        hidden_save(&d, "alice", &[]).unwrap();
        let r3 = runner(&[&ks, &rs], &[]);
        let res = resolve_hiding(&as_dyn(&r3), &env, &K8sConfig::default(), None, "x-dev", true, &hidden_load(&d, "alice"));
        assert_eq!(res.profiles.len(), 2);
        assert!(res.ambiguous, "both answer and nothing is chosen");
        // the kubeconfig file itself is untouched
        assert_eq!(std::fs::read_to_string(&remote).unwrap(), "apiVersion: v1\nkind: Config\n");
    }

    #[test]
    fn kubectl_and_helm_are_pinned_to_the_workspace_cluster() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let p = pin_cluster_args("kubectl", &a(&["get", "pods", "--kubeconfig=/home/u/.kube/config", "--context", "remote", "-s", "https://192.168.1.68:6443", "-n", "x"]), "/etc/rancher/k3s/k3s.yaml", "default").unwrap();
        assert_eq!(p, a(&["--kubeconfig", "/etc/rancher/k3s/k3s.yaml", "--context", "default", "get", "pods", "-n", "x"]));
        let h = pin_cluster_args("helm", &a(&["ls"]), "/k", "c").unwrap();
        assert_eq!(h, a(&["--kubeconfig", "/k", "--kube-context", "c", "ls"]));
        assert!(pin_cluster_args("ls", &a(&["-la"]), "/k", "c").is_none());
        assert!(pin_cluster_args("kubectl", &a(&["get"]), "", "").is_none());
    }
}
