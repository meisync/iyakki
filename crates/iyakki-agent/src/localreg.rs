//! The local image registry in one click: a CNCF Distribution registry (`registry:2`, Apache-2.0) as a Deployment, a PVC and a
//! NodePort Service in the namespace `iyakki-registry` of a cluster IYAKKI can reach (the managed k3s dev cluster, or the cluster the
//! person chose), so that a service built on this device (the private store) has somewhere to put its image.
//!
//! * [`check_dependency`]: the cluster answers, a node is Ready and there is a StorageClass for the volume. Otherwise one plain
//!   sentence says what is missing; nothing is created.
//! * [`inspect`] / [`status_line`]: absent, starting, or ready with its address and how many images (tags) it holds. Calling
//!   [`create`] on a registry that is ready changes nothing (idempotent); one that exists but is not ready is applied again.
//! * [`create`]: namespace, the manifest of [`crate::regsetup::in_cluster_manifest_sized`], rollout, then `GET /v2/` from this
//!   machine. It reports every step through a callback (the job's log) and turns failures into a sentence with the fix.
//! * [`remove`]: deletes the namespace (the images in it go with it) after the person typed its name.
//!
//! **How it is used.** Pods pull `localhost:<nodePort>/<repo>:<tag>`: containerd allows plain HTTP for `localhost`, and a NodePort is
//! reachable on every Ready node at localhost, so no node needs a `registries.yaml`, a restart or root (the cluster is not restarted).
//! IYAKKI pushes to `<node address>:<nodePort>` with [`crate::ocipush`] (no docker or podman). The registry has no sign-in: it is
//! reachable by anything that can reach the node's NodePort, like every NodePort Service; the plan and the docs say so, and the way to
//! keep it on this machine is the machine's firewall (`k3sdev::ports()` lists 30500 among the ports).
//!
//! Programs started: `kubectl` through the [`Runner`] (or `k3s kubectl` for the managed cluster, see [`K3sKubectl`]); nothing else.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::devcluster::Kc;
use crate::ocipush::{self, RegistryHttp};
use crate::regsetup::{self, Facts, KubeCtx, PlanEnv, RegistryOption, IN_NAME, IN_NS};
use crate::wsexec::{Out, Runner};

/// The registry's address as the pods and the nodes pull it, for a NodePort.
pub fn pull_address(port: u16) -> String {
    format!("localhost:{port}")
}

/// A cluster and how to reach it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub kubeconfig: String,
    pub context: String,
    /// The host this machine reaches the nodes' NodePorts at: `localhost` for a cluster on this machine.
    pub node_host: String,
}

impl Target {
    pub fn from_profile(p: &crate::devcluster::ClusterProfile) -> Target {
        Target { name: p.name.clone(), kubeconfig: p.kubeconfig.clone(), context: p.context.clone(), node_host: crate::platform::engine::node_host(p) }
    }

    fn kube(&self) -> KubeCtx {
        KubeCtx { kubeconfig: self.kubeconfig.clone(), context: self.context.clone() }
    }

    fn push_address(&self, port: u16) -> String {
        let h = if self.node_host.is_empty() { "localhost" } else { self.node_host.as_str() };
        format!("{h}:{port}")
    }

    fn on_this_machine(&self) -> bool {
        matches!(self.node_host.as_str(), "" | "localhost" | "127.0.0.1" | "::1")
    }
}

/// `kubectl` for the managed cluster: when no `kubectl` is on the machine, `k3s kubectl` (the program the cluster was installed with)
/// answers in its place. Everything else goes to the inner runner unchanged.
pub struct K3sKubectl {
    pub inner: Arc<dyn Runner>,
    pub k3s: PathBuf,
}

impl Runner for K3sKubectl {
    fn run(&self, cwd: &std::path::Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<crate::wsexec::Sink>) -> Out {
        if argv.first().map(String::as_str) == Some("kubectl") && !self.inner.has("kubectl") && self.k3s.exists() {
            let mut a = vec![self.k3s.display().to_string(), "kubectl".to_string()];
            a.extend(argv[1..].iter().cloned());
            return self.inner.run(cwd, &a, env, timeout, cap, sink);
        }
        self.inner.run(cwd, argv, env, timeout, cap, sink)
    }
    fn has(&self, program: &str) -> bool {
        self.inner.has(program) || (program == "kubectl" && self.k3s.exists())
    }
}

// ------------------------------------------------------------------------------------------------------------------ plain words

fn first_line(t: &str) -> String {
    t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(240).collect()
}

fn not_found(t: &str) -> bool {
    t.contains("NotFound") || t.contains("not found")
}

/// What a failed `kubectl` call means for the person.
pub fn explain_kubectl(raw: &str) -> String {
    let l = first_line(raw);
    let low = raw.to_lowercase();
    if low.contains("connection refused") || low.contains("unable to connect") || low.contains("no route to host") || low.contains("i/o timeout") || low.contains("context deadline") || low.contains("timed out") {
        format!("The cluster does not answer ({l}). If it is the dev cluster, start it first (Settings \u{2192} Dev cluster) and wait until it is Ready; for another cluster check that it is up and reachable from here.")
    } else if low.contains("forbidden") || low.contains("unauthorized") {
        format!("The cluster refused the request ({l}). The identity in the kubeconfig may not create namespaces and workloads; use an administrator kubeconfig for this one step.")
    } else if low.contains("no such file") || low.contains("unable to read") || low.contains("kubeconfig") && low.contains("not") {
        format!("The cluster's kubeconfig cannot be read ({l}).")
    } else if low.contains("kubectl is not installed") {
        "There is no kubectl on this machine. The dev cluster installed by IYAKKI brings its own (k3s kubectl), so install it with Quick install \u{2192} k3s; for another cluster install kubectl from the CLI tile.".to_string()
    } else if low.contains("not found in $path") || low.contains("executable file not found") || low.contains("no such file or directory") {
        "kubectl is not available on this machine and k3s's own kubectl was not found either; install the dev cluster first (Quick install \u{2192} k3s).".to_string()
    } else {
        l
    }
}

/// Why a pod does not start, from `kubectl get pods -o json`.
pub fn diagnose_pods(json: &str) -> Option<String> {
    let j: Value = serde_json::from_str(json).ok()?;
    for p in j["items"].as_array()? {
        for cs in p["status"]["containerStatuses"].as_array().cloned().unwrap_or_default() {
            let w = &cs["state"]["waiting"];
            let reason = w["reason"].as_str().unwrap_or("");
            match reason {
                "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName" => {
                    return Some("The cluster could not pull the registry image (docker.io/library/registry:2) from Docker Hub. The cluster needs internet access to Docker Hub once, and Docker Hub limits anonymous pulls per hour. Check the connection, then press Retry.".to_string());
                }
                "CrashLoopBackOff" | "CreateContainerConfigError" | "CreateContainerError" => {
                    return Some(format!("The registry container does not stay up ({reason}: {}). Its log: kubectl -n {IN_NS} logs deploy/{IN_NAME}.", first_line(w["message"].as_str().unwrap_or(""))));
                }
                _ => {}
            }
        }
        for c in p["status"]["conditions"].as_array().cloned().unwrap_or_default() {
            if c["type"].as_str() == Some("PodScheduled") && c["status"].as_str() == Some("False") {
                let m = first_line(c["message"].as_str().unwrap_or(""));
                let why = if m.contains("persistentvolumeclaim") || m.contains("unbound") { "its volume is not provisioned (the StorageClass has not created it)" } else { "no node can run it" };
                return Some(format!("The registry pod is waiting to be scheduled: {why} ({m})."));
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------------------------ the dependency

#[derive(Clone, Debug)]
pub enum Dependency {
    Met { nodes_ready: usize, storage_class: String, facts: Facts },
    /// One plain sentence: what is missing and what to do.
    Missing(String),
}

impl Dependency {
    pub fn met(&self) -> bool {
        matches!(self, Dependency::Met { .. })
    }
}

fn kc(run: &Arc<dyn Runner>, t: &Target) -> Kc {
    Kc::new(run.clone(), &t.kubeconfig, &t.context)
}

/// The cluster answers, at least one node is Ready, and a StorageClass can hold the volume.
pub fn check_dependency(run: &Arc<dyn Runner>, t: &Target) -> Dependency {
    let k = kc(run, t);
    let n = k.kubectl(&["get", "nodes", "-o", "json"], 10, 20);
    if !n.ok() {
        return Dependency::Missing(explain_kubectl(&n.combined()));
    }
    let Ok(j) = serde_json::from_str::<Value>(&n.stdout) else {
        return Dependency::Missing("The cluster's answer about its nodes could not be read.".into());
    };
    let ready = j["items"].as_array().map_or(0, |a| a.iter().filter(|i| i["status"]["conditions"].as_array().is_some_and(|c| c.iter().any(|c| c["type"].as_str() == Some("Ready") && c["status"].as_str() == Some("True")))).count());
    if ready == 0 {
        return Dependency::Missing("The cluster answers, but no node is Ready yet. Wait a minute (a cluster that just started needs 20 to 60 seconds) and try again; Settings \u{2192} Dev cluster shows its state.".into());
    }
    let facts = regsetup::probe(run.as_ref(), &t.kube());
    match &facts.storage_class {
        Some(c) => Dependency::Met { nodes_ready: ready, storage_class: c.clone(), facts: facts.clone() },
        None => Dependency::Missing("The cluster has no default StorageClass and no local-path class, so the registry would have no disk and lose its images whenever its pod restarts. k3s ships the local-path class; if it was switched off, turn it back on (or mark any StorageClass as the default) and try again.".into()),
    }
}

// ------------------------------------------------------------------------------------------------------------------ what exists

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    /// Where this machine pushes (`localhost:30500`, or `<node>:30500`).
    pub push: String,
    /// What pods and nodes pull from.
    pub pull: String,
    pub node_port: u16,
    pub ready: bool,
    /// Images (tags) the registry holds; None: it did not answer from this machine.
    pub images: Option<usize>,
    pub repositories: usize,
    /// `10Gi`, or `emptyDir` for a registry created without a volume.
    pub storage: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Absent,
    /// The objects exist, the pod is not ready: the reason when it is known.
    Starting(Info, String),
    Ready(Info),
    /// The cluster could not be asked.
    Unknown(String),
}

impl State {
    pub fn info(&self) -> Option<&Info> {
        match self {
            State::Starting(i, _) | State::Ready(i) => Some(i),
            _ => None,
        }
    }
}

/// What exists in the cluster now (read-only kubectl, and `GET /v2/_catalog` against the registry).
pub fn inspect(run: &Arc<dyn Runner>, http: &dyn RegistryHttp, t: &Target) -> State {
    let k = kc(run, t);
    let s = k.read(&["get", "service", IN_NAME, "-n", IN_NS, "-o", "json"]);
    if !s.ok() {
        let c = s.combined();
        return if not_found(&c) { State::Absent } else { State::Unknown(explain_kubectl(&c)) };
    }
    let sj: Value = serde_json::from_str(&s.stdout).unwrap_or(Value::Null);
    let Some(port) = sj["spec"]["ports"].as_array().and_then(|p| p.iter().find_map(|p| p["nodePort"].as_u64())).map(|p| p as u16) else {
        return State::Unknown(format!("The Service {IN_NS}/{IN_NAME} exists but has no NodePort; remove the registry and create it again."));
    };
    let d = k.read(&["get", "deployment", IN_NAME, "-n", IN_NS, "-o", "json"]);
    let ready = d.ok() && serde_json::from_str::<Value>(&d.stdout).is_ok_and(|j| j["status"]["readyReplicas"].as_u64().unwrap_or(0) >= 1);
    let pvc = k.read(&["get", "pvc", "registry-data", "-n", IN_NS, "-o", "jsonpath={.spec.resources.requests.storage}"]);
    let storage = if pvc.ok() && !pvc.stdout.trim().is_empty() { pvc.stdout.trim().to_string() } else { "emptyDir".into() };
    let mut info = Info { push: t.push_address(port), pull: pull_address(port), node_port: port, ready, images: None, repositories: 0, storage };
    if !ready {
        let pods = k.read(&["get", "pods", "-n", IN_NS, "-o", "json"]);
        let why = diagnose_pods(&pods.stdout).unwrap_or_else(|| "its pod is starting".to_string());
        return State::Starting(info, why);
    }
    if let Ok(c) = ocipush::catalog(http, &info.push) {
        info.images = Some(c.iter().map(|(_, t)| t.len()).sum());
        info.repositories = c.len();
    }
    State::Ready(info)
}

/// "registry ready · localhost:30500 · used by 3 images" and the like (the one line the deploy screen and the menu show).
pub fn status_line(s: &State) -> String {
    match s {
        State::Absent => "no local registry yet".into(),
        State::Unknown(e) => format!("registry state unknown: {e}"),
        State::Starting(i, why) => format!("registry starting · {} · {why}", i.pull),
        State::Ready(i) => {
            let used = match i.images {
                Some(0) => "no images yet".to_string(),
                Some(1) => "used by 1 image".to_string(),
                Some(n) => format!("used by {n} images"),
                None => "not reachable from this machine yet".to_string(),
            };
            format!("registry ready \u{b7} {} \u{b7} {used}", i.pull)
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ create

/// How long [`create`] waits for the registry to answer from this machine.
#[derive(Clone, Copy, Debug)]
pub struct Wait {
    pub tries: u32,
    pub pause: Duration,
}

impl Wait {
    pub const REAL: Wait = Wait { tries: 12, pause: Duration::from_secs(5) };
    /// Up to two minutes for a cluster that was just installed or started.
    pub const CLUSTER_START: Wait = Wait { tries: 24, pause: Duration::from_secs(5) };
    pub const NONE: Wait = Wait { tries: 1, pause: Duration::ZERO };
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub size_gi: u32,
    /// 0: 30500 or the next free NodePort.
    pub node_port: u16,
    pub wait: Wait,
    /// Where the manifest is written (it is applied from a file).
    pub work_dir: PathBuf,
}

impl Opts {
    pub fn new(work_dir: PathBuf) -> Opts {
        Opts { size_gi: regsetup::DEFAULT_SIZE_GI, node_port: 0, wait: Wait::REAL, work_dir }
    }
}

/// The volume sizes the form offers.
pub const SIZES_GI: [u32; 4] = [5, 10, 20, 50];

/// The text shown before the button is pressed: what will be created, in plain words.
pub fn describe_plan(size_gi: u32, t: &Target) -> String {
    format!(
        "A local image registry will be created in the cluster{}:\n- namespace {IN_NS}\n- the CNCF Distribution registry (docker.io/library/registry:2, Apache-2.0), pulled once from Docker Hub by the cluster\n- a {size_gi} Gi volume (StorageClass local-path unless the cluster has another default), so the images survive restarts\n- a NodePort Service (30500, or the next free port): pods and the node pull localhost:<port>, IYAKKI pushes to {}:<port>\nThe cluster is NOT restarted and no node file is changed. The registry has no sign-in: anything that can reach this machine's NodePort can push to it, so keep that port off untrusted networks.",
        if t.name.is_empty() { String::new() } else { format!(" \"{}\"", t.name) },
        if t.on_this_machine() { "localhost" } else { t.node_host.as_str() }
    )
}

/// Create the registry (or confirm it). `say` gets one line per step. On failure the message says what to do.
pub fn create(run: &Arc<dyn Runner>, http: &dyn RegistryHttp, t: &Target, o: &Opts, say: &mut dyn FnMut(&str)) -> Result<Info, String> {
    if !SIZES_GI.contains(&o.size_gi) && !(1..=500).contains(&o.size_gi) {
        return Err(format!("{} Gi is not a volume size (1 to 500)", o.size_gi));
    }
    say("[1/6] checking the cluster");
    let (nodes_ready, storage_class, mut facts) = match check_dependency(run, t) {
        Dependency::Met { nodes_ready, storage_class, facts } => (nodes_ready, storage_class, facts),
        Dependency::Missing(m) => return Err(m),
    };
    say(&format!("      cluster{} is reachable, {nodes_ready} node(s) Ready, StorageClass {storage_class}", if t.name.is_empty() { String::new() } else { format!(" \"{}\"", t.name) }));
    match inspect(run, http, t) {
        State::Ready(i) => {
            say(&format!("[6/6] the registry exists already: {}", status_line(&State::Ready(i.clone()))));
            return Ok(i);
        }
        State::Starting(_, why) => say(&format!("      a registry exists but is not ready ({why}); applying it again")),
        State::Unknown(e) => return Err(e),
        State::Absent => {}
    }
    facts.size_gi = o.size_gi;
    let mut env = PlanEnv { kube: Some(t.kube()), api_host: if t.on_this_machine() { String::new() } else { t.node_host.clone() }, facts, ..PlanEnv::default() };
    env.registries_yaml = PathBuf::new();
    let plan = regsetup::plan(&RegistryOption::InCluster { node_port: o.node_port }, &env).map_err(|e| {
        if e.contains("already used") || e.contains("taken") {
            format!("{e}. Another Service in the cluster holds that NodePort; leave the port empty to use the next free one.")
        } else {
            e
        }
    })?;
    let manifest = plan.manifest.clone().ok_or("the plan has no manifest")?;
    let lint = crate::deploylint::lint(&manifest, IN_NS, &|_| false);
    if !lint.ok() {
        return Err(format!("the registry manifest did not pass the manifest check: {}", lint.report()));
    }
    let k = kc(run, t);
    say(&format!("[2/6] namespace {IN_NS}"));
    let ns = k.kubectl(&["get", "namespace", IN_NS, "-o", "name"], 10, 20);
    if !ns.ok() {
        if !not_found(&ns.combined()) {
            return Err(explain_kubectl(&ns.combined()));
        }
        let c = k.kubectl(&["create", "namespace", IN_NS], 10, 30);
        if !c.ok() && !c.combined().contains("AlreadyExists") {
            return Err(format!("Could not create the namespace {IN_NS}: {}", explain_kubectl(&c.combined())));
        }
        say("      created");
    } else {
        say("      exists");
    }
    std::fs::create_dir_all(&o.work_dir).map_err(|e| format!("{}: {e}", o.work_dir.display()))?;
    let file = o.work_dir.join("iyakki-registry.yaml");
    std::fs::write(&file, &lint.text).map_err(|e| format!("{}: {e}", file.display()))?;
    say("[3/6] applying the registry objects (volume, Deployment, Service)");
    let a = k.kubectl(&["apply", "-f", &file.display().to_string(), "-n", IN_NS], 20, 90);
    if !a.ok() {
        return Err(format!("kubectl apply failed: {}", explain_kubectl(&a.combined())));
    }
    for l in a.stdout.lines().filter(|l| !l.trim().is_empty()).take(6) {
        say(&format!("      {}", l.trim()));
    }
    say("[4/6] waiting for the registry pod (the first start pulls registry:2 from Docker Hub, about 25 MB)");
    let r = k.kubectl(&["rollout", "status", &format!("deployment/{IN_NAME}"), "-n", IN_NS, "--timeout=150s"], 20, 170);
    if !r.ok() {
        let pods = k.read(&["get", "pods", "-n", IN_NS, "-o", "json"]);
        let why = diagnose_pods(&pods.stdout).unwrap_or_else(|| first_line(&r.combined()));
        return Err(format!("The registry pod did not become ready: {why}"));
    }
    let port = plan.pull.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).ok_or("the plan has no port")?;
    let push = t.push_address(port);
    say(&format!("[5/6] checking that {push} answers from this machine"));
    let mut up = false;
    for n in 0..o.wait.tries.max(1) {
        if ocipush::answers(http, &push) {
            up = true;
            break;
        }
        if n + 1 < o.wait.tries {
            std::thread::sleep(o.wait.pause);
        }
    }
    if !up {
        return Err(format!("The registry pod is running, but {push} does not answer from this machine. Something else may be using port {port} on the node, kube-proxy may not expose NodePorts on the node's address, or a firewall blocks it. Check with: kubectl -n {IN_NS} get svc {IN_NAME}. Press Retry after fixing it."));
    }
    let info = match inspect(run, http, t) {
        State::Ready(i) => i,
        other => return Err(format!("The registry answers but the cluster does not report it ready: {}", status_line(&other))),
    };
    say(&format!("[6/6] done: {}", status_line(&State::Ready(info.clone()))));
    say(&format!("      pods pull {}/<name>:<tag>; IYAKKI pushes to {}", info.pull, info.push));
    Ok(info)
}

// ------------------------------------------------------------------------------------------------------------------ remove

/// The text of the confirmation: what is lost.
pub fn describe_remove(s: &State) -> String {
    let images = s.info().and_then(|i| i.images).map(|n| format!("{n} image(s)")).unwrap_or_else(|| "its images".into());
    format!("Remove the local image registry? This deletes the namespace {IN_NS} with its volume and {images}. Services whose images came from it (the private store) keep running, but their pods cannot be pulled again after a restart until you create the registry and deploy them again. Type {IN_NS} to confirm.")
}

/// Delete the registry's namespace. `typed` must be the namespace's name. Nothing else in the cluster is touched.
pub fn remove(run: &Arc<dyn Runner>, http: &dyn RegistryHttp, t: &Target, typed: &str, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    if typed.trim() != IN_NS {
        return Err(format!("Removing deletes the registry's volume and every image in it. Type {IN_NS} to confirm; nothing was changed."));
    }
    match inspect(run, http, t) {
        State::Absent => {
            say("there is no local registry; nothing to remove");
            return Ok(());
        }
        State::Unknown(e) => return Err(e),
        _ => {}
    }
    say(&format!("deleting namespace {IN_NS} (this takes up to a minute)"));
    let d = kc(run, t).kubectl(&["delete", "namespace", IN_NS, "--wait=true", "--timeout=120s"], 20, 150);
    if !d.ok() && !not_found(&d.combined()) {
        return Err(format!("Could not remove the registry: {}", explain_kubectl(&d.combined())));
    }
    say("removed");
    Ok(())
}

#[cfg(test)]
#[path = "localreg_tests.rs"]
pub(crate) mod tests;
