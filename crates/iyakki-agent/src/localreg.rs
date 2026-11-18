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
//! **How it is used.** The registry has no sign-in, so it is never published on the network: the Service is ClusterIP and the Deployment
//! binds `hostPort: <port>` with `hostIP: 127.0.0.1` (the pattern of the Gitea loopback port). Pods and the node pull
//! `localhost:<port>/<repo>:<tag>` (containerd allows plain HTTP for `localhost`, so no node needs a `registries.yaml`, a restart or
//! root), and IYAKKI pushes to `localhost:<port>` with [`crate::ocipush`] (no docker or podman). Only a single-node cluster on this
//! machine fits (the dev cluster); a cluster on another machine is refused. An older install that is a NodePort Service (open on the
//! LAN, found 2026-10-28) is converted by [`regsetup::close_legacy`]: by [`create`] (quick-install registry), by the next start of the
//! managed cluster (`k3sdev`), and by the registry setup `apply`; every conversion is audited as `registry.nodeport.closed`.
//!
//! Programs started: `kubectl` through the [`Runner`] (or `k3s kubectl` for the managed cluster, see [`K3sKubectl`]); nothing else.

pub fn target_of(p: &crate::devcluster::ClusterProfile) -> Target {
    Target { name: p.name.clone(), kubeconfig: p.kubeconfig.clone(), context: p.context.clone(), node_host: crate::platform::engine::node_host(p) }
}

pub use iyakki_shared::localreg::{Dependency, Info, Opts, SIZES_GI, State, Target, describe_plan, describe_remove, pull_address, status_line};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::devcluster::Kc;
use crate::ocipush::{self, RegistryHttp};
use crate::regsetup::{self, Facts, KubeCtx, PlanEnv, RegistryOption, IN_NAME, IN_NS};
use crate::wsexec::{Out, Runner};




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
        "There is no kubectl on this machine. The dev cluster installed by IYAKKI brings its own (k3s kubectl), so install it with Store \u{2192} Setup \u{2192} k3s; for another cluster install kubectl from the CLI tile.".to_string()
    } else if low.contains("not found in $path") || low.contains("executable file not found") || low.contains("no such file or directory") {
        "kubectl is not available on this machine and k3s's own kubectl was not found either; install the dev cluster first (Store \u{2192} Setup \u{2192} k3s).".to_string()
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




/// What exists in the cluster now (read-only kubectl, and `GET /v2/_catalog` against the registry).
pub fn inspect(run: &Arc<dyn Runner>, http: &dyn RegistryHttp, t: &Target) -> State {
    let k = kc(run, t);
    let s = k.read(&["get", "service", IN_NAME, "-n", IN_NS, "-o", "json"]);
    if !s.ok() {
        let c = s.combined();
        return if not_found(&c) { State::Absent } else { State::Unknown(explain_kubectl(&c)) };
    }
    let sj: Value = serde_json::from_str(&s.stdout).unwrap_or(Value::Null);
    let d = k.read(&["get", "deployment", IN_NAME, "-n", IN_NS, "-o", "json"]);
    let dj: Value = if d.ok() { serde_json::from_str(&d.stdout).unwrap_or(Value::Null) } else { Value::Null };
    let ready = dj["status"]["readyReplicas"].as_u64().unwrap_or(0) >= 1;
    // the loopback hostPort of the Deployment; an older install had a NodePort Service (still readable, and converted by `create`)
    let port = regsetup::hostport_of(&dj).or_else(|| sj["spec"]["ports"].as_array().and_then(|p| p.iter().find_map(|p| p["nodePort"].as_u64())).map(|p| p as u16));
    let Some(port) = port else {
        return State::Unknown(format!("The Service {IN_NS}/{IN_NAME} exists but its registry has no port on this machine; remove the registry and create it again."));
    };
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


// ------------------------------------------------------------------------------------------------------------------ create

pub use iyakki_shared::platform_types::Wait;





/// Create the registry (or confirm it). `say` gets one line per step. On failure the message says what to do.
pub fn create(run: &Arc<dyn Runner>, http: &dyn RegistryHttp, t: &Target, o: &Opts, say: &mut dyn FnMut(&str)) -> Result<Info, String> {
    if !SIZES_GI.contains(&o.size_gi) && !(1..=500).contains(&o.size_gi) {
        return Err(format!("{} Gi is not a volume size (1 to 500)", o.size_gi));
    }
    say("[1/6] checking the cluster");
    let k0 = kc(run, t);
    let (nodes_ready, storage_class, mut facts) = match check_dependency(run, t) {
        Dependency::Met { nodes_ready, storage_class, facts } => (nodes_ready, storage_class, facts),
        Dependency::Missing(m) => return Err(m),
    };
    say(&format!("      cluster{} is reachable, {nodes_ready} node(s) Ready, StorageClass {storage_class}", if t.name.is_empty() { String::new() } else { format!(" \"{}\"", t.name) }));
    // an older install is open on the network (NodePort, no password): close it first, whatever else happens
    let converted = regsetup::close_legacy(&|a: &[&str]| k0.kubectl(a, 20, 60))?;
    if let Some(line) = &converted {
        say(&format!("      SECURITY: {line}"));
    }
    match inspect(run, http, t) {
        State::Ready(i) if converted.is_none() => {
            say(&format!("[6/6] the registry exists already: {}", status_line(&State::Ready(i.clone()))));
            return Ok(i);
        }
        State::Ready(_) => say("      the registry was converted; applying its objects again"),
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
    let kube = |args: &[&str]| {
        let o = k.kubectl(args, 10, 30);
        crate::pvcguard::KOut { ok: o.ok(), stdout: o.stdout.clone(), text: o.combined() }
    };
    let g = crate::pvcguard::guard(&lint.text, Some(IN_NS), &kube)?;
    for n in &g.notes {
        say(&format!("      {n}"));
    }
    if g.manifest.trim() != lint.text.trim() {
        std::fs::write(&file, &g.manifest).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let a = k.kubectl(&["apply", "-f", &file.display().to_string(), "-n", IN_NS], 20, 90);
    if !a.ok() {
        return Err(format!("kubectl apply failed: {}", crate::pvcguard::explain_apply_error(&explain_kubectl(&a.combined()))));
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
