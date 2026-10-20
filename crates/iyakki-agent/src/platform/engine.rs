//! The engine: choose a cluster, plan, preview, apply, status, stop, start, upgrade, backup and remove, all through the [`Runner`]
//! with the manifest linter and the namespace guards of the dev-cluster tools. Every write needs the approval flag and a profile that
//! passes [`deploy_refusal`] again at that moment; a value generated for a secret never appears in an argument, a log or a result line.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use zeroize::Zeroizing;

use super::installed::{self, ClusterRef, Installed};
use super::*;
use crate::devcluster::{self, ClusterProfile, Kc};

// ------------------------------------------------------------------------------------------------ choosing the cluster

/// A cluster the person may pick, with the reason it cannot be used (None: usable).
#[derive(Clone, Debug)]
pub struct ListedProfile {
    pub id: String,
    pub profile: ClusterProfile,
    pub refusal: Option<String>,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A short stable id for a profile (what `platform_list` shows and `platform_deploy` takes).
pub fn profile_id(p: &ClusterProfile) -> String {
    format!("c{:012x}", fnv(&format!("{}#{}", p.kubeconfig, p.context)) & 0xffff_ffff_ffff)
}

/// Why nothing may be deployed to this profile (None: it may).
pub fn deploy_refusal(p: &ClusterProfile) -> Option<String> {
    if !p.exists || !p.readable {
        return Some(format!("refused: the kubeconfig {} cannot be read{}", p.kubeconfig, if p.problem.is_empty() { String::new() } else { format!(" ({})", p.problem) }));
    }
    if !p.reachable {
        let why = if p.problem.is_empty() { String::new() } else { format!(" ({})", p.problem) };
        let todo = if p.k3s && p.local {
            ". The k3s on this machine is stopped or still starting: a cluster that was just installed or started needs up to two minutes, so press Look again (r) in a moment; if it stays like this, start it in Settings \u{2192} Dev cluster"
        } else {
            ". Check that it is running and reachable from this machine, then press Look again (r)"
        };
        return Some(format!("refused: the cluster \"{}\" does not answer{why}{todo}", p.name));
    }
    if p.looks_like_production() {
        return Some(format!("refused: the cluster \"{}\" ({}) looks like production; platform services are only deployed to development clusters", p.name, p.server));
    }
    devcluster::read_only_reason(p).map(|r| format!("refused: {r}"))
}

/// Every cluster this person's IYAKKI can see (hidden ones left out), tested, with its id and refusal. Also written to
/// `platform-profiles-<user>.json` so an approval card can name the server without running anything.
pub fn list_profiles(pl: &Platform) -> Vec<ListedProfile> {
    let remembered = devcluster::Choice::load(&pl.dir, &pl.user);
    let hidden = devcluster::hidden_load(&pl.dir, &pl.user);
    let res = devcluster::resolve_hiding(&pl.run, &pl.env, &crate::workspace::K8sConfig::default(), remembered.as_ref(), "default", true, &hidden);
    let out: Vec<ListedProfile> = res.profiles.into_iter().map(|p| ListedProfile { id: profile_id(&p), refusal: deploy_refusal(&p), profile: p }).collect();
    let cache: Vec<Value> = out.iter().map(|l| serde_json::json!({"id": l.id, "name": l.profile.name, "server": l.profile.server, "kubeconfig": l.profile.kubeconfig, "context": l.profile.context, "source": l.profile.source, "refusal": l.refusal})).collect();
    let _ = std::fs::create_dir_all(&pl.dir);
    let _ = std::fs::write(cache_path(&pl.dir, &pl.user), serde_json::to_string_pretty(&cache).unwrap_or_default());
    out
}

fn cache_path(dir: &Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    dir.join(format!("platform-profiles-{safe}.json"))
}

/// `(name, server, refusal)` of a profile id from the last [`list_profiles`], read from disk only.
pub fn cached_profile(dir: &Path, user: &str, id: &str) -> Option<(String, String, Option<String>)> {
    let v: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(cache_path(dir, user)).ok()?).ok()?;
    let e = v.iter().find(|e| e["id"].as_str() == Some(id))?;
    Some((e["name"].as_str().unwrap_or("").to_string(), e["server"].as_str().unwrap_or("").to_string(), e["refusal"].as_str().map(str::to_string)))
}

/// The profile with this id, from the last [`list_profiles`] (so only a cluster that was listed to the person can be named), inspected
/// again now. A hidden, unknown, changed or refused cluster is an error.
pub fn profile_by_id(pl: &Platform, id: &str) -> Result<ClusterProfile, String> {
    let unknown = || format!("refused: `{id}` is not one of the clusters you can see (it was never listed, is hidden, or is gone: call platform_list for the ids)");
    let cache: Vec<Value> = std::fs::read_to_string(cache_path(&pl.dir, &pl.user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let e = cache.iter().find(|e| e["id"].as_str() == Some(id)).ok_or_else(unknown)?;
    let kubeconfig = e["kubeconfig"].as_str().unwrap_or("").to_string();
    let context = e["context"].as_str().unwrap_or("").to_string();
    let source: devcluster::Source = serde_json::from_value(e["source"].clone()).map_err(|_| unknown())?;
    if devcluster::hidden_load(&pl.dir, &pl.user).iter().any(|h| *h == kubeconfig) {
        return Err(format!("refused: the kubeconfig {kubeconfig} is hidden in IYAKKI"));
    }
    let p = devcluster::inspect(&pl.run, Path::new(&kubeconfig), source, &context, true);
    if profile_id(&p) != id {
        return Err(unknown());
    }
    match deploy_refusal(&p) {
        Some(r) => Err(r),
        None => Ok(p),
    }
}

/// The profile of an installed service, inspected now and checked like a new deployment (and that it is still the same server).
pub fn guard_write(pl: &Platform, c: &ClusterRef) -> Result<ClusterProfile, String> {
    let hidden = devcluster::hidden_load(&pl.dir, &pl.user);
    if hidden.iter().any(|h| *h == c.kubeconfig) {
        return Err(format!("refused: the kubeconfig {} is hidden in IYAKKI; show it again before changing anything there", c.kubeconfig));
    }
    let p = devcluster::inspect(&pl.run, Path::new(&c.kubeconfig), c.source, &c.context, true);
    if let Some(r) = deploy_refusal(&p) {
        return Err(r);
    }
    if !c.server.is_empty() && !p.server.is_empty() && p.server != c.server {
        return Err(format!("refused: the cluster behind {} is now {} but this service was deployed to {}; nothing was changed", c.kubeconfig, p.server, c.server));
    }
    Ok(p)
}

pub fn kc_of(pl: &Platform, c: &ClusterRef) -> Kc {
    Kc::new(pl.run.clone(), &c.kubeconfig, &c.context)
}

// ------------------------------------------------------------------------------------------------ namespaces

pub(crate) fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

fn not_found(t: &str) -> bool {
    t.contains("NotFound") || t.contains("not found")
}

/// The namespace guard of the platform tools: IYAKKI manages it (`managed-by=iyakki`), it is not protected, and (when
/// `need_service`) it carries `iyakki.dev/service=<id>`, i.e. IYAKKI created it for this service.
pub fn guard_namespace(kc: &Kc, ns: &str, service: &str, need_service: bool) -> Result<(), String> {
    let manual = format!("To let IYAKKI manage `{ns}`: kubectl label namespace {ns} {LABEL_MANAGED}=iyakki");
    if crate::k8stools::protected_namespace(ns) {
        return Err(format!("refused: `{ns}` is a protected namespace (default, kube-* and iyakki-registry); IYAKKI never changes services there"));
    }
    let o = kc.read(&["get", "namespace", ns, "-o", "json"]);
    if !o.ok() {
        let t = o.combined();
        return Err(if not_found(&t) { format!("the namespace `{ns}` does not exist") } else { format!("could not read namespace {ns}: {}", first_line(&t)) });
    }
    let j: Value = serde_json::from_str(&o.stdout).map_err(|_| format!("could not read the labels of namespace {ns}"))?;
    let l = &j["metadata"]["labels"];
    if l[LABEL_MANAGED].as_str() != Some("iyakki") {
        return Err(format!("refused: namespace `{ns}` is not one IYAKKI manages: it lacks the label {LABEL_MANAGED}=iyakki. {manual}"));
    }
    if need_service && l[LABEL_SERVICE].as_str() != Some(service) {
        return Err(format!("refused: namespace `{ns}` was not created by IYAKKI for {service} (it lacks {LABEL_SERVICE}={service}), so it is never deleted by IYAKKI"));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ probing the cluster

fn host_of(server: &str) -> String {
    let s = server.trim().trim_start_matches("https://").trim_start_matches("http://");
    let s = s.split('/').next().unwrap_or("");
    if let Some(rest) = s.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_string();
    }
    s.rsplit_once(':').map_or(s, |x| x.0).to_string()
}

/// The address people reach the cluster's nodes at.
pub fn node_host(p: &ClusterProfile) -> String {
    let h = host_of(&p.server);
    if p.local || h.is_empty() || matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1") { "localhost".into() } else { h }
}

/// Read what planning depends on (read-only kubectl). Fails only when the nodes cannot be read: nothing else can be trusted then.
pub fn probe(kc: &Kc, p: &ClusterProfile, ns: &str, instance: &str) -> Result<ClusterFacts, String> {
    let mut f = ClusterFacts { api_host: node_host(p), ..Default::default() };
    let n = kc.read(&["get", "nodes", "-o", "json"]);
    if !n.ok() {
        return Err(format!("could not read the nodes: {}", crate::localreg::explain_kubectl(&n.combined())));
    }
    let j: Value = serde_json::from_str(&n.stdout).map_err(|_| "could not read the nodes: the answer is not JSON".to_string())?;
    for i in j["items"].as_array().cloned().unwrap_or_default() {
        let ready = i["status"]["conditions"].as_array().is_some_and(|c| c.iter().any(|c| c["type"].as_str() == Some("Ready") && c["status"].as_str() == Some("True")));
        let node = NodeFact { name: i["metadata"]["name"].as_str().unwrap_or("").into(), ready, arch: i["status"]["nodeInfo"]["architecture"].as_str().unwrap_or("").into() };
        if ready {
            f.pod_capacity += i["status"]["allocatable"]["pods"].as_str().and_then(|s| s.parse::<usize>().ok()).unwrap_or(110);
        }
        f.nodes.push(node);
    }
    let sc = kc.read(&["get", "storageclass", "-o", "json"]);
    if sc.ok() {
        if let Ok(j) = serde_json::from_str::<Value>(&sc.stdout) {
            let items = j["items"].as_array().cloned().unwrap_or_default();
            let name = |s: &Value| s["metadata"]["name"].as_str().map(str::to_string);
            f.storage_classes = items.iter().filter_map(name).collect();
            f.storage_class = items.iter().find(|s| s["metadata"]["annotations"]["storageclass.kubernetes.io/is-default-class"].as_str() == Some("true")).and_then(name).or_else(|| f.storage_classes.iter().find(|n| *n == "local-path").cloned());
        }
    }
    let sv = kc.read(&["get", "services", "-A", "--no-headers", "-o", "custom-columns=NS:.metadata.namespace,NAME:.metadata.name,TYPE:.spec.type,PORTS:.spec.ports[*].nodePort"]);
    if sv.ok() {
        for l in sv.stdout.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() >= 4 {
                let ports: Vec<u16> = w[3].split(',').filter_map(|p| p.parse().ok()).collect();
                f.taken_ports.extend(ports.iter().copied());
                f.services.push((w[0].into(), w[1].into(), w[2].into(), ports));
            }
        }
    }
    let wl = kc.read(&["get", "deployments,statefulsets,daemonsets", "-A", "--no-headers", "-o", "custom-columns=NS:.metadata.namespace,NAME:.metadata.name,IMAGES:.spec.template.spec.containers[*].image"]);
    if wl.ok() {
        for l in wl.stdout.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() >= 3 {
                f.workloads.push((w[0].into(), w[1].into(), w[2].split(',').map(str::to_string).collect()));
            }
        }
    }
    let pods = kc.read(&["get", "pods", "-A", "--no-headers", "-o", "name"]);
    if pods.ok() {
        f.pods = pods.stdout.lines().filter(|l| !l.trim().is_empty()).count();
    }
    let own = kc.read(&["get", "services", "-n", ns, "-l", &format!("{LABEL_INSTANCE}={instance}"), "--no-headers", "-o", "custom-columns=NAME:.metadata.name,PORTS:.spec.ports[*].nodePort"]);
    if own.ok() {
        for l in own.stdout.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() >= 2 {
                if let Some(p) = w[1].split(',').find_map(|p| p.parse::<u16>().ok()) {
                    f.own_ports.push((w[0].into(), p));
                    f.taken_ports.retain(|t| *t != p);
                }
            }
        }
    }
    let nsr = kc.read(&["get", "namespace", ns, "-o", "json"]);
    f.namespace = if nsr.ok() {
        let j: Value = serde_json::from_str(&nsr.stdout).unwrap_or(Value::Null);
        let l = &j["metadata"]["labels"];
        if l[LABEL_MANAGED].as_str() == Some("iyakki") { NamespaceFact::Managed { service: l[LABEL_SERVICE].as_str().map(str::to_string) } } else { NamespaceFact::Foreign }
    } else if not_found(&nsr.combined()) {
        NamespaceFact::Missing
    } else {
        return Err(format!("could not check namespace {ns}: {}", first_line(&nsr.combined())));
    };
    let reg = kc.read(&["get", "service", crate::regsetup::IN_NAME, "-n", crate::regsetup::IN_NS, "-o", "json"]);
    if reg.ok() {
        if let Ok(j) = serde_json::from_str::<Value>(&reg.stdout) {
            if let Some(port) = j["spec"]["ports"].as_array().and_then(|p| p.iter().find_map(|p| p["nodePort"].as_u64())) {
                f.registry_pull = Some(format!("localhost:{port}"));
                f.registry_push = Some(format!("{}:{port}", f.api_host));
            }
        }
    }
    Ok(f)
}

// ------------------------------------------------------------------------------------------------ the plan

/// The "engine" of a [`MirrorPlan`] that has no container engine: IYAKKI builds the one-file image and pushes it itself
/// (`ocipush.rs`). Only for a service whose image is one program it builds locally.
pub const BUILTIN_ENGINE: &str = "iyakki";

#[derive(Clone, Debug)]
pub struct MirrorPlan {
    pub engine: String,
    /// `(source image, pushed to, pulled by the pods as)` per image.
    pub images: Vec<(String, String, String)>,
    pub platform: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub service: String,
    pub title: String,
    pub version: String,
    pub instance: String,
    pub cluster: ClusterRef,
    pub profile: ClusterProfile,
    pub namespace: String,
    pub create_namespace: bool,
    pub opts: Options,
    pub images: Vec<ImageRef>,
    pub image_refs: Vec<String>,
    pub mirror: Option<MirrorPlan>,
    /// The images must go through the in-cluster registry and the cluster has none yet: the deploy screen offers to create it.
    pub needs_registry: bool,
    pub manifest: String,
    pub manifest_path: PathBuf,
    pub secrets: Vec<SecretSpec>,
    pub urls: Vec<UrlInfo>,
    pub exposes: Vec<String>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
    pub existing: Vec<Existing>,
    pub storage_class: Option<String>,
    pub network_policy: Option<String>,
    pub how_to_use: String,
    pub vault_prefix: String,
    pub notes_lint: Vec<String>,
    /// The CPU architecture (`amd64` / `arm64`) a locally built image must have: the Ready nodes', else this machine's.
    pub target_arch: String,
}

impl Plan {
    pub fn can_apply(&self) -> bool {
        self.blockers.is_empty()
    }
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        o => o,
    }
}

fn pick_port(want: &str, own: Option<u16>, taken: &[u16], first: u16, used: &[u16]) -> Result<u16, String> {
    if !want.is_empty() {
        let p: u16 = want.parse().map_err(|_| format!("`{want}` is not a port"))?;
        if own != Some(p) && (taken.contains(&p) || used.contains(&p)) {
            return Err(format!("NodePort {p} is already used by another Service in the cluster; choose another (30000 to 32767) or leave it empty"));
        }
        return Ok(p);
    }
    if let Some(p) = own {
        return Ok(p);
    }
    (first..=*NODE_PORTS.end()).chain(*NODE_PORTS.start()..first).find(|p| !taken.contains(p) && !used.contains(p)).ok_or_else(|| "every NodePort from 30000 to 32767 is taken".to_string())
}

/// Wait for a cluster that was just installed or started: the API answers before the node is Ready and before the StorageClass exists.
/// Only the k3s on this machine is waited for; any other cluster is planned as it is. `say` gets a progress line about every 10 s.
pub fn wait_for_cluster(pl: &Platform, profile: &ClusterProfile, wait: crate::localreg::Wait, say: &mut dyn FnMut(&str)) {
    if !(profile.k3s && profile.local) {
        return;
    }
    let t = crate::localreg::Target { name: profile.name.clone(), kubeconfig: profile.kubeconfig.clone(), context: profile.context.clone(), node_host: "localhost".into() };
    for n in 0..wait.tries {
        match crate::localreg::check_dependency(&pl.run, &t) {
            crate::localreg::Dependency::Met { .. } => {
                if n > 0 {
                    say("the cluster is Ready");
                }
                return;
            }
            crate::localreg::Dependency::Missing(why) => {
                if n + 1 == wait.tries {
                    return;
                }
                if n == 0 || (n as u64 * wait.pause.as_secs().max(1)) % 10 == 0 {
                    say(&format!("waiting for the dev cluster to be ready ({}s of {}s): {why}", n as u64 * wait.pause.as_secs(), wait.tries as u64 * wait.pause.as_secs()));
                }
                std::thread::sleep(wait.pause);
            }
        }
    }
}

/// [`plan`] after [`wait_for_cluster`].
pub fn plan_waiting(pl: &Platform, def: &dyn ServiceDef, profile: &ClusterProfile, given: &Options, wait: crate::localreg::Wait, say: &mut dyn FnMut(&str)) -> Result<Plan, String> {
    wait_for_cluster(pl, profile, wait, say);
    plan(pl, def, profile, given)
}

/// Plan one deployment: check the options and the cluster, build and check the manifest. `Err` only for a refused cluster, bad
/// options or an unreadable cluster; everything the person can fix in the cluster is a blocker in the plan, shown with the preview.
pub fn plan(pl: &Platform, def: &dyn ServiceDef, profile: &ClusterProfile, given: &Options) -> Result<Plan, String> {
    if let Some(r) = deploy_refusal(profile) {
        return Err(r);
    }
    if devcluster::hidden_load(&pl.dir, &pl.user).iter().any(|h| *h == profile.kubeconfig) {
        return Err(format!("refused: the kubeconfig {} is hidden in IYAKKI", profile.kubeconfig));
    }
    let opts = validate_options(def, given)?;
    let ns = opts.get("namespace").to_string();
    let version = opts.get("version").to_string();
    let exposure = Exposure::parse(opts.get("exposure"))?;
    let cluster = ClusterRef::of(profile);
    let existing_records = installed::load(&pl.dir, &pl.user);
    let instance = installed::instance_id(def.id(), &ns, &format!("{}#{}", cluster.kubeconfig, cluster.context), &existing_records);
    let kc = Kc::new(pl.run.clone(), &profile.kubeconfig, &profile.context);
    let facts = probe(&kc, profile, &ns, &instance)?;
    let mut blockers: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // nodes and capacity
    let ready: Vec<&NodeFact> = facts.nodes.iter().filter(|n| n.ready).collect();
    let down: Vec<&str> = facts.nodes.iter().filter(|n| !n.ready).map(|n| n.name.as_str()).collect();
    if ready.is_empty() {
        blockers.push(format!("no node is Ready{}: nothing could be scheduled", if down.is_empty() { String::new() } else { format!(" (NotReady: {})", down.join(", ")) }));
    } else if !down.is_empty() {
        warnings.push(format!("NotReady nodes: {}; pods are only scheduled on the {} Ready node(s)", down.join(", "), ready.len()));
    }
    if facts.pod_capacity > 0 && facts.pods >= facts.pod_capacity {
        blockers.push(format!("the nodes' pod limit is reached ({} pods of {})", facts.pods, facts.pod_capacity));
    } else if facts.pod_capacity > 0 && facts.pod_capacity - facts.pods < 3 {
        warnings.push(format!("only {} free pod slots are left on the nodes", facts.pod_capacity - facts.pods));
    }

    // storage
    let wanted_class = opts.get("storage_class");
    let storage_class = if !wanted_class.is_empty() {
        if !facts.storage_classes.is_empty() && !facts.storage_classes.iter().any(|c| c == wanted_class) {
            blockers.push(format!("the StorageClass `{wanted_class}` does not exist (the cluster has: {})", facts.storage_classes.join(", ")));
        }
        Some(wanted_class.to_string())
    } else {
        match &facts.storage_class {
            Some(c) => Some(c.clone()),
            None => {
                blockers.push("the cluster has no default StorageClass and no local-path class, so the PersistentVolumeClaim would stay Pending: install a storage provisioner (k3s ships local-path) or name a class in Storage class".into());
                None
            }
        }
    };

    // namespace
    let create_namespace = match &facts.namespace {
        NamespaceFact::Missing => {
            notes.push(format!("The namespace {ns} does not exist: a visible first step creates it and labels it {LABEL_MANAGED}=iyakki and {LABEL_SERVICE}={} (you approve this plan, which includes that step).", def.id()));
            true
        }
        NamespaceFact::Managed { service: Some(s) } if s != def.id() => {
            blockers.push(format!("the namespace {ns} belongs to the service {s}; choose another namespace"));
            false
        }
        NamespaceFact::Managed { .. } => {
            notes.push(format!("The namespace {ns} already exists and is managed by IYAKKI: the service is added to it."));
            false
        }
        NamespaceFact::Foreign => {
            blockers.push(format!("the namespace {ns} exists but IYAKKI does not manage it (no {LABEL_MANAGED}=iyakki label). Choose another name, or label it yourself: kubectl label namespace {ns} {LABEL_MANAGED}=iyakki"));
            false
        }
    };

    // ports
    let own = |suffix: &str| facts.own_ports.iter().find(|(n, _)| n.ends_with(suffix)).map(|x| x.1);
    let (mut http_port, mut ssh_port) = (0u16, 0u16);
    if exposure == Exposure::NodePort {
        match pick_port(opts.get("http_node_port"), own("http"), &facts.taken_ports, 30300, &[]) {
            Ok(p) => http_port = p,
            Err(e) => blockers.push(e),
        }
        match pick_port(opts.get("ssh_node_port"), own("ssh"), &facts.taken_ports, 30222, &[http_port]) {
            Ok(p) => ssh_port = p,
            Err(e) => blockers.push(e),
        }
    }

    // images
    let images = def.images(&version);
    let (mut image_refs, mut mirror): (Vec<String>, Option<MirrorPlan>) = (images.iter().map(ImageRef::reference).collect(), None);
    let mut needs_registry = false;
    if opts.flag("mirror") || def.always_mirror() {
        match (&facts.registry_pull, &facts.registry_push) {
            (Some(pull), Some(push)) => {
                // no podman or docker: a service whose image is one locally built program is built and pushed by IYAKKI itself
                let builtin = def.local_binary().is_some() && images.iter().all(|i| i.image.starts_with("localhost/"));
                let engine = ["podman", "docker"].into_iter().find(|e| pl.run.has(e)).or(builtin.then_some(BUILTIN_ENGINE));
                match engine {
                    None => blockers.push("mirroring needs podman or docker on this machine, and neither is installed".into()),
                    Some(e) => {
                        let archs: std::collections::BTreeSet<&str> = ready.iter().map(|n| n.arch.as_str()).filter(|a| !a.is_empty()).collect();
                        let platform = (archs.len() == 1).then(|| format!("linux/{}", archs.iter().next().expect("one")));
                        if archs.len() > 1 {
                            warnings.push("the nodes have different architectures, but a mirrored image is copied for this machine's architecture only".into());
                        }
                        let mut m = MirrorPlan { engine: e.into(), images: vec![], platform };
                        let mut refs = Vec::new();
                        for img in &images {
                            let target = format!("iyakki-platform/{}", img.repo_and_tag());
                            m.images.push((img.image.clone(), format!("{push}/{target}"), format!("{pull}/{target}")));
                            refs.push(format!("{pull}/{target}"));
                        }
                        image_refs = refs;
                        mirror = Some(m);
                        if e == BUILTIN_ENGINE {
                            notes.push(format!("No podman or docker is needed: IYAKKI builds the one-file image from the server program and pushes it itself to the in-cluster registry at {push}; the pods pull it as {pull}/... (no node needs configuring)."));
                        } else {
                            notes.push(format!("The images are pulled here from their registry (Docker Hub for Gitea) and pushed to the in-cluster registry at {push}; the pods pull them as {pull}/... (no node needs configuring)."));
                        }
                    }
                }
            }
            _ => {
                needs_registry = true;
                blockers.push(if def.always_mirror() {
                    format!("{} is built on this device, so the cluster cannot get it from Docker Hub: it is pushed to the in-cluster registry, and this cluster has none yet. Press \"Create registry\" to make one (one step, about a minute, no cluster restart); the deploy then continues by itself.", images.first().map(|i| i.image.trim_start_matches("localhost/").to_string()).unwrap_or_else(|| def.title()))
                } else {
                    "mirroring needs the in-cluster registry, and this cluster has none yet. Press \"Create registry\" to make one (one step, about a minute, no cluster restart), or turn Mirror images off.".to_string()
                });
            }
        }
    } else {
        notes.push("The cluster pulls the pinned images itself from their public registry; a cluster with no internet access can mirror them through the in-cluster registry (option Mirror images).".into());
    }
    for img in &images {
        let bad: Vec<&str> = ready.iter().filter(|n| !n.arch.is_empty() && !img.archs.contains(&n.arch.as_str())).map(|n| n.name.as_str()).collect();
        if !bad.is_empty() {
            warnings.push(format!("{} is published for {} only, but the node(s) {} use a different CPU architecture: the pod would fail with an exec format error or ImagePullBackOff", img.image, img.archs.join(", "), bad.join(", ")));
        }
    }
    if let Some(m) = &mirror {
        if m.platform.is_none() && ready.iter().any(|n| n.arch != host_arch()) {
            warnings.push(format!("this machine is {} and a node is not: the mirrored image would be the wrong architecture", host_arch()));
        }
    }

    // an existing service of the same kind
    let mut existing = def.detect_existing(&facts);
    if matches!(&facts.namespace, NamespaceFact::Managed { service: Some(s) } if s == def.id()) {
        // deploying again into the namespace IYAKKI made for this service: that one is ours, not a second
        existing.retain(|e| e.namespace != ns);
    }
    for e in &existing {
        warnings.push(format!("a {} is already running here: {}/{} ({}). Connect to it instead of installing a second one (Connect to existing).", e.product, e.namespace, e.name, e.image));
    }

    // a program built on this machine: say now whether it is at hand (never a blocker: the wizard can build or take a file)
    let target_arch = {
        let archs: std::collections::BTreeSet<&str> = ready.iter().map(|n| n.arch.as_str()).filter(|a| !a.is_empty()).collect();
        archs.iter().next().map(|a| a.to_string()).unwrap_or_else(|| host_arch().to_string())
    };
    if let (Some(lb), true) = (def.local_binary(), mirror.is_some()) {
        let want = localbin::Arch::parse(&target_arch).unwrap_or_else(localbin::Arch::host);
        let d = localbin::discover(pl.run.as_ref(), &lb, &localbin::Places::real(&pl.dir, &lb), want);
        match &d.found {
            Some(f) => notes.push(format!("The server program for the image: {} ({}, static {}).", f.path.display(), f.origin.word(), f.arch.uname())),
            None => warnings.push(format!("There is no static {} for {} yet: the deploy will stop before changing anything and offer to build it or to use a file you choose (or build the image from dist/store-server/Dockerfile).", lb.file, want.uname())),
        }
    }

    // the manifest
    let vault_prefix = format!("svc.{instance}");
    let ctx = BuildCtx { namespace: ns.clone(), opts: opts.clone(), version: version.clone(), images: image_refs.clone(), storage_class: storage_class.clone(), host: if exposure == Exposure::Ingress { opts.get("ingress_host").to_string() } else { facts.api_host.clone() }, http_port, ssh_port, vault_prefix: vault_prefix.clone(), instance: instance.clone() };
    let built = def.build(&ctx)?;
    let lint = crate::deploylint::lint(&built.manifest, &ns, &|_| false);
    if !lint.ok() {
        return Err(format!("the manifest of {} did not pass the manifest check:\n{}", def.title(), lint.report()));
    }
    let builtin = def.id() == "gitea";
    let mut notes_lint = Vec::new();
    for w in &lint.warnings {
        if builtin {
            return Err(format!("the manifest of {} did not pass the manifest check cleanly:\n{}", def.title(), lint.report()));
        }
        warnings.push(format!("manifest check: {w}"));
    }
    notes_lint.extend(lint.notes.iter().cloned());
    notes.extend(built.notes.iter().cloned());
    let manifest_path = pl.dir.join("platform").join(format!("{}-{}.yaml", pl.user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>(), instance));
    Ok(Plan {
        service: def.id().into(),
        title: def.title(),
        version,
        instance,
        cluster,
        profile: profile.clone(),
        namespace: ns.clone(),
        create_namespace,
        images,
        image_refs,
        mirror,
        needs_registry,
        manifest: lint.text,
        manifest_path,
        secrets: built.secrets,
        how_to_use: def.how_to_use(&built.urls, &opts),
        urls: built.urls,
        exposes: def.exposes(&opts),
        opts,
        warnings,
        blockers,
        notes,
        existing,
        storage_class,
        network_policy: def.network_policy_example(&ns),
        vault_prefix,
        notes_lint,
        target_arch,
    })
}

fn kc_line(c: &ClusterRef, args: &str) -> String {
    let mut s = String::from("kubectl");
    if !c.kubeconfig.is_empty() {
        s.push_str(&format!(" --kubeconfig {}", c.kubeconfig));
    }
    if !c.context.is_empty() {
        s.push_str(&format!(" --context {}", c.context));
    }
    format!("{s} {args}")
}

/// The exact plan, for the approval and the review screen. It lists every command, the manifest in full, the Secrets by name only,
/// what is exposed and everything the person should know before saying yes.
pub fn preview(p: &Plan) -> String {
    let c = &p.cluster;
    let mut s = String::new();
    s.push_str(&format!("DEPLOY {} {} to the cluster {}\n", p.title, p.version, c.line()));
    s.push_str(&format!("Namespace: {}\n", p.namespace));
    if !p.blockers.is_empty() {
        s.push_str("\nCANNOT BE APPLIED YET:\n");
        for b in &p.blockers {
            s.push_str(&format!("- {b}\n"));
        }
    }
    if !p.warnings.is_empty() {
        s.push_str("\nWARNINGS:\n");
        for w in &p.warnings {
            s.push_str(&format!("- {w}\n"));
        }
    }
    s.push_str("\nSTEPS (each runs only after you approve this plan):\n");
    let mut n = 1;
    let mut step = |t: String| {
        s.push_str(&format!("{n}. {t}\n"));
        n += 1;
    };
    if p.create_namespace {
        step(format!("{}   (a cluster-scoped object: this is the step that needs your approval)", kc_line(c, &format!("create namespace {}", p.namespace))));
        step(kc_line(c, &format!("label namespace {} {LABEL_MANAGED}=iyakki {LABEL_SERVICE}={} --overwrite", p.namespace, p.service)));
    }
    if let Some(m) = &p.mirror {
        for (src, push, _) in &m.images {
            step(format!("{} pull {}{src}", m.engine, m.platform.as_ref().map(|x| format!("--platform {x} ")).unwrap_or_default()));
            step(format!("{} tag {src} {push}", m.engine));
            step(format!("{} push {}{push}", m.engine, if m.engine == "podman" { "--tls-verify=false " } else { "" }));
        }
    }
    for sec in &p.secrets {
        step(format!("Create the Secret {} in {} (only if it does not exist yet) with a generated value for: {}. The values go to your vault first and to the Secret through a private temp file; they are never shown here, in a log or to the assistant.", sec.name, p.namespace, sec.entries.iter().map(|e| format!("{} (vault entry uvault:{})", e.label, e.vault)).collect::<Vec<_>>().join(", ")));
    }
    step(format!("{}   (the manifest below, checked by the manifest check)", kc_line(c, &format!("apply -f {} -n {}", p.manifest_path.display(), p.namespace))));
    step(format!("Wait until ready: {}", crate::platform::spec(&p.service).map(|d| d.workloads().iter().map(|w| kc_line(c, &format!("rollout status deployment/{} -n {} --timeout=300s", w.name, p.namespace))).collect::<Vec<_>>().join("; ")).unwrap_or_default()));
    step("First-run setup inside the pod and recording the service in your list of installed services.".into());
    s.push_str("\nIMAGES (pulled by the cluster; pinned versions):\n");
    for (i, img) in p.images.iter().enumerate() {
        s.push_str(&format!("- {} ({}){}\n", p.image_refs.get(i).unwrap_or(&img.image), img.archs.join("/"), if p.image_refs.get(i).is_some_and(|r| *r != img.reference()) { format!("  mirrored from {}", img.image) } else { String::new() }));
    }
    s.push_str(&format!("\nSTORAGE: PersistentVolumeClaim {} on StorageClass {}\n", p.opts.get("storage_size"), p.storage_class.as_deref().unwrap_or("(none)")));
    s.push_str("\nWHAT IT EXPOSES:\n");
    for e in &p.exposes {
        s.push_str(&format!("- {e}\n"));
    }
    s.push_str("\nADDRESSES AFTER THE DEPLOY:\n");
    for u in &p.urls {
        s.push_str(&format!("- {}: {}\n", u.label, u.url));
    }
    s.push_str("\nCREDENTIALS CREATED:\n");
    for sec in &p.secrets {
        for e in &sec.entries {
            s.push_str(&format!("- {} in the Kubernetes Secret {}/{} and in your vault as uvault:{}{}\n", e.label, p.namespace, sec.name, e.vault, if e.reveal { " (shown once after the deploy, masked, with Copy)" } else { "" }));
        }
    }
    if !p.existing.is_empty() {
        s.push_str("\nEXISTING:\n");
        for e in &p.existing {
            s.push_str(&format!("- {} {}/{} ({}){}\n", e.product, e.namespace, e.name, e.image, if e.url_hint.is_empty() { String::new() } else { format!(" at {}", e.url_hint) }));
        }
    }
    for note in p.notes.iter().chain(p.notes_lint.iter()) {
        s.push_str(&format!("\nNote: {note}\n"));
    }
    s.push_str(&format!("\nMANIFEST (applied to {}; no Secret values, no privileged containers, no hostPath):\n{}\n", p.namespace, p.manifest.trim_end()));
    if let Some(np) = &p.network_policy {
        s.push_str(&format!("\nEXAMPLE NetworkPolicy (NOT applied; copy it if your cluster enforces policies):\n{}\n", np.trim_end()));
    }
    s.trim_end().to_string()
}

// ------------------------------------------------------------------------------------------------ apply

#[derive(Default)]
pub struct Applied {
    pub done: Vec<String>,
    pub urls: Vec<UrlInfo>,
    /// Values to show once in the masked dialog (the admin password).
    pub reveal: Vec<Reveal>,
    pub ready: bool,
    pub how_to_use: String,
    pub instance: String,
}

impl std::fmt::Debug for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Applied {{ done: {:?}, urls: {}, reveal: {:?}, ready: {} }}", self.done, self.urls.len(), self.reveal, self.ready)
    }
}

fn run_step(pl: &Platform, argv: &[String], secs: u64) -> crate::wsexec::Out {
    pl.run.run(&std::env::temp_dir(), argv, &[], std::time::Duration::from_secs(secs), 64 * 1024, pl.live.clone())
}

fn mirror_images(pl: &Platform, m: &MirrorPlan, progress: &mut dyn FnMut(&str)) -> Result<(), String> {
    for (src, push, _) in &m.images {
        progress(&format!("mirroring {src}"));
        // an image built on this machine (`localhost/...`) is already here: nothing to pull
        let local = src.starts_with("localhost/");
        let mut pull: Vec<String> = vec![m.engine.clone(), "pull".into()];
        if let Some(pf) = &m.platform {
            pull.extend(["--platform".into(), pf.clone()]);
        }
        pull.push(src.clone());
        let o = if local { run_step(pl, &[m.engine.clone(), "image".into(), "inspect".into(), src.clone()], 60) } else { run_step(pl, &pull, 900) };
        if !o.ok() {
            return Err(format!("pulling {src} failed: {}", first_line(&o.combined())));
        }
        let o = run_step(pl, &[m.engine.clone(), "tag".into(), src.clone(), push.clone()], 60);
        if !o.ok() {
            return Err(format!("tagging {src} failed: {}", first_line(&o.combined())));
        }
        let mut pushc: Vec<String> = vec![m.engine.clone(), "push".into()];
        if m.engine == "podman" {
            pushc.push("--tls-verify=false".into());
        }
        pushc.push(push.clone());
        let o = run_step(pl, &pushc, 900);
        if !o.ok() {
            return Err(format!("pushing {push} failed: {}", first_line(&o.combined())));
        }
    }
    Ok(())
}

/// Do the plan. `approved` must be true (the person saw [`preview`]). The cluster is checked again first.
pub fn apply(pl: &Platform, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Applied, String> {
    apply_with(pl, plan, approved, progress, &crate::ocipush::TcpHttp::default())
}

/// [`apply`] with the HTTP client a built-in image push uses (tests give a fake registry).
pub fn apply_with(pl: &Platform, plan: &Plan, approved: bool, progress: &mut dyn FnMut(&str), http: &dyn crate::ocipush::RegistryHttp) -> Result<Applied, String> {
    if !approved {
        return Err("the deployment was not approved".into());
    }
    if !plan.blockers.is_empty() {
        return Err(format!("not applied: {}", plan.blockers.join("; ")));
    }
    let def = spec(&plan.service).ok_or_else(|| format!("unknown service {}", plan.service))?;
    let profile = guard_write(pl, &plan.cluster)?;
    let kc = Kc::new(pl.run.clone(), &profile.kubeconfig, &profile.context);
    let ns = plan.namespace.clone();
    let mut a = Applied { instance: plan.instance.clone(), how_to_use: plan.how_to_use.clone(), urls: plan.urls.clone(), ..Default::default() };
    pl.audit("deploy", &format!("{}:{}:{} start", plan.service, ns, plan.cluster.server));
    // a program the image is built from must be at hand before anything is created; otherwise nothing at all was changed
    if let (Some(lb), Some(_)) = (def.local_binary(), &plan.mirror) {
        let want = localbin::Arch::parse(&plan.target_arch).unwrap_or_else(localbin::Arch::host);
        let places = localbin::Places::real(&pl.dir, &lb);
        let d = localbin::discover(pl.run.as_ref(), &lb, &places, want);
        if d.found.is_none() {
            return Err(d.problem(&lb, &places.cache));
        }
    }
    if !plan.secrets.is_empty() {
        // a locked vault stops everything before the cluster is touched
        let probe = format!("{}.check", plan.vault_prefix);
        pl.vault.set(&probe, "ok").map_err(|e| format!("{e}; unlock your vault first. Nothing was changed in the cluster."))?;
        let _ = pl.vault.delete(&probe);
    }
    // 1. the namespace (a visible step)
    progress("checking the namespace");
    let facts_ns = kc.read(&["get", "namespace", &ns, "-o", "json"]);
    let mut created_ns = false;
    if !facts_ns.ok() {
        if !not_found(&facts_ns.combined()) {
            return Err(format!("could not check namespace {ns}: {}", first_line(&facts_ns.combined())));
        }
        progress(&format!("creating namespace {ns}"));
        let c = kc.kubectl(&["create", "namespace", &ns], 20, 40);
        if !c.ok() {
            return Err(format!("could not create namespace {ns}: {}", first_line(&c.combined())));
        }
        let l = kc.kubectl(&["label", "namespace", &ns, &format!("{LABEL_MANAGED}=iyakki"), &format!("{LABEL_SERVICE}={}", plan.service), "--overwrite"], 20, 40);
        if !l.ok() {
            return Err(format!("created namespace {ns} but could not label it: {}", first_line(&l.combined())));
        }
        created_ns = true;
        a.done.push(format!("created namespace {ns} (labelled as managed by IYAKKI)"));
    }
    if !created_ns {
        guard_namespace(&kc, &ns, &plan.service, false)?;
    }
    // 2. mirror
    if let Some(m) = &plan.mirror {
        if m.engine == BUILTIN_ENGINE {
            for (_, push, _) in &m.images {
                let (registry, repo_tag) = push.split_once('/').ok_or_else(|| format!("`{push}` is not an image address"))?;
                progress(&format!("building and pushing {repo_tag} to {registry} (no container engine needed)"));
                def.push_builtin(pl, http, &plan.version, &plan.target_arch, registry, repo_tag, progress)?;
            }
        } else {
            def.prepare_images(pl, &plan.version, &plan.target_arch, progress)?;
            mirror_images(pl, m, progress)?;
        }
        a.done.push(format!("mirrored {} image(s) into the in-cluster registry", m.images.len()));
    }
    // 3. secrets
    let mut vault_names: Vec<String> = Vec::new();
    for sec in &plan.secrets {
        progress(&format!("checking the Secret {}", sec.name));
        for e in &sec.entries {
            vault_names.push(e.vault.clone());
        }
        if secrets::secret_exists(&kc, &ns, &sec.name)? {
            a.done.push(format!("kept the existing Secret {} (generated values are never replaced)", sec.name));
            continue;
        }
        let mut pairs: Vec<(String, Zeroizing<String>)> = Vec::new();
        let mut written: Vec<String> = Vec::new();
        for e in &sec.entries {
            let v = Zeroizing::new((pl.gen)(if e.kind == SecretKind::Token { 40 } else { 24 }));
            if let Err(err) = pl.vault.set(&e.vault, &v) {
                for w in &written {
                    let _ = pl.vault.delete(w);
                }
                return Err(format!("{err}; the Secret was not created"));
            }
            written.push(e.vault.clone());
            pairs.push((e.key.clone(), v.clone()));
            if e.reveal {
                a.reveal.push(Reveal { label: e.label.clone(), vault: e.vault.clone(), value: v });
            }
        }
        let labels = [(LABEL_MANAGED, "iyakki"), (LABEL_SERVICE, plan.service.as_str()), (LABEL_INSTANCE, plan.instance.as_str())];
        if let Err(err) = secrets::create_secret(&kc, &pl.tmp, &ns, &sec.name, &pairs, &labels) {
            for w in &written {
                let _ = pl.vault.delete(w);
            }
            a.reveal.clear();
            return Err(err);
        }
        a.done.push(format!("created the Secret {} with generated values (also in your vault)", sec.name));
    }
    // 4. the manifest (empty: a repair that has nothing to put back)
    if plan.manifest.trim().is_empty() {
        a.done.push("no objects were missing, so nothing was applied".into());
    } else {
        let lint = crate::deploylint::lint(&plan.manifest, &ns, &|_| false);
        if !lint.ok() {
            return Err(format!("the manifest did not pass the manifest check:\n{}", lint.report()));
        }
        if let Some(d) = plan.manifest_path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&plan.manifest_path, &lint.text).map_err(|e| format!("{}: {e}", plan.manifest_path.display()))?;
        progress("applying the manifest");
        let f = plan.manifest_path.display().to_string();
        let o = kc.kubectl(&["apply", "-f", &f, "-n", &ns], 30, 120);
        if !o.ok() {
            return Err(format!("kubectl apply failed: {}", o.combined().trim()));
        }
        a.done.push(format!("applied the manifest to {ns}"));
    }
    // 5. wait
    let workloads = def.workloads();
    let mut ready = true;
    for w in &workloads {
        progress(&format!("waiting for {} to be ready", w.name));
        let r = run_wait(pl, &kc, &ns, &w.name, 300);
        if r.ok() {
            a.done.push(format!("{} is ready", w.name));
        } else {
            ready = false;
            let mut why = format!("{} is not ready yet: {}", w.name, first_line(&r.combined()));
            let ev = kc.read(&["get", "events", "-n", &ns, "--sort-by=.lastTimestamp"]);
            if ev.ok() {
                if let Some(d) = crate::regsetup::diagnose(&ev.stdout, "") {
                    why.push_str(&format!("\n{d}"));
                } else {
                    let tail: Vec<&str> = ev.stdout.lines().rev().take(4).collect();
                    why.push_str(&format!("\nLatest events:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
                }
            }
            a.done.push(why);
        }
    }
    a.ready = ready;
    // 6. first-run setup
    let mut added_vault: Vec<String> = Vec::new();
    if ready {
        if let Some(w) = workloads.first() {
            progress("first-run setup");
            let mut pc = PostCtx { p: pl, kc: kc.clone(), namespace: ns.clone(), instance: plan.instance.clone(), vault_prefix: plan.vault_prefix.clone(), opts: plan.opts.clone(), urls: plan.urls.clone(), workload: w.clone(), added_vault: vec![] };
            match def.post_deploy(&mut pc) {
                Ok(lines) => a.done.extend(lines),
                Err(e) => a.done.push(format!("first-run setup did not finish: {e}")),
            }
            added_vault = pc.added_vault;
        }
        if let (Some(path), Some(u)) = (def.health_path(), plan.urls.iter().find(|u| u.kind == "web")) {
            let url = format!("{}{}", u.url.trim_end_matches('/'), path);
            match pl.http.request("GET", &url, None, None) {
                Ok(r) if r.status == 200 => a.done.push(format!("{url} answers 200")),
                _ => a.done.push(format!("{url} did not answer from this machine yet (the pod is ready; the address may not be reachable from here)")),
            }
        }
    }
    // 7. the record
    for v in added_vault {
        if !vault_names.contains(&v) {
            vault_names.push(v);
        }
    }
    let now = crate::clock::now_unix();
    let prev = installed::find(&pl.dir, &pl.user, &plan.instance);
    let rec = Installed {
        id: plan.instance.clone(),
        service: plan.service.clone(),
        title: plan.title.clone(),
        version: plan.version.clone(),
        kind: def.kind().into(),
        cluster: Some(plan.cluster.clone()),
        namespace: ns.clone(),
        created_namespace: created_ns || prev.as_ref().is_some_and(|p| p.created_namespace),
        connected: false,
        options: plan.opts.clone(),
        urls: plan.urls.clone(),
        vault: vault_names,
        installed_at: prev.as_ref().map_or(now, |p| p.installed_at),
        updated_at: now,
        workspaces: prev.map(|p| p.workspaces).unwrap_or_default(),
        note: if ready { String::new() } else { "not ready yet".into() },
    };
    installed::upsert(&pl.dir, &pl.user, rec)?;
    a.done.push("recorded the service in your installed services".into());
    pl.audit("deploy", &format!("{}:{}:{} {}", plan.service, ns, plan.cluster.server, if ready { "ok" } else { "not ready" }));
    Ok(a)
}

fn run_wait(pl: &Platform, kc: &Kc, ns: &str, name: &str, secs: u64) -> crate::wsexec::Out {
    let args = ["rollout", "status", &format!("deployment/{name}"), "-n", ns, &format!("--timeout={secs}s")];
    let argv = kc.argv(&args, secs + 10);
    pl.run.run(&std::env::temp_dir(), &argv, &kc.env(), std::time::Duration::from_secs(secs + 30), 64 * 1024, pl.live.clone())
}

// ------------------------------------------------------------------------------------------------ status

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcState {
    Running,
    Starting,
    Stopped,
    Degraded,
    Missing,
    /// The cluster does not answer.
    Unreachable,
    /// A connected existing service (nothing of ours runs in a cluster).
    Connected,
}

impl SvcState {
    pub fn word(self) -> &'static str {
        match self {
            SvcState::Running => "running",
            SvcState::Starting => "starting",
            SvcState::Stopped => "stopped",
            SvcState::Degraded => "degraded",
            SvcState::Missing => "missing",
            SvcState::Unreachable => "cluster unreachable",
            SvcState::Connected => "connected",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: SvcState,
    pub ready: u32,
    pub desired: u32,
    pub pods: Vec<String>,
    pub live_images: Vec<String>,
    pub notes: Vec<String>,
    /// Does the first web address answer from this machine.
    pub reachable: Option<bool>,
}

impl Status {
    pub fn line(&self) -> String {
        format!("{} ({}/{} ready)", self.state.word(), self.ready, self.desired)
    }
}

/// Read-only: what the cluster says about an installed service (no approval, no guard beyond the namespace being read).
pub fn status(pl: &Platform, inst: &Installed, check_url: bool) -> Status {
    let mut st = Status { state: SvcState::Missing, ready: 0, desired: 0, pods: vec![], live_images: vec![], notes: vec![], reachable: None };
    let reach = |st: &mut Status| {
        if check_url {
            if let Some(u) = inst.web_url() {
                let path = spec(&inst.service).and_then(|d| d.health_path()).unwrap_or_default();
                st.reachable = Some(pl.http.request("GET", &format!("{}{}", u.trim_end_matches('/'), path), None, None).is_ok_and(|r| r.status < 500));
            }
        }
    };
    let Some(c) = &inst.cluster else {
        st.state = SvcState::Connected;
        reach(&mut st);
        return st;
    };
    if inst.connected {
        st.state = SvcState::Connected;
        reach(&mut st);
        return st;
    }
    let kc = kc_of(pl, c);
    let o = kc.read(&["get", "deployments,pods", "-n", &inst.namespace, "-l", &format!("{LABEL_INSTANCE}={}", inst.id), "-o", "json"]);
    if !o.ok() {
        let t = o.combined();
        st.state = if not_found(&t) { SvcState::Missing } else { SvcState::Unreachable };
        st.notes.push(first_line(&t));
        return st;
    }
    let j: Value = serde_json::from_str(&o.stdout).unwrap_or(Value::Null);
    let items = j["items"].as_array().cloned().unwrap_or_default();
    let mut deployments = 0;
    for i in &items {
        match i["kind"].as_str() {
            Some("Deployment") => {
                deployments += 1;
                st.desired += i["spec"]["replicas"].as_u64().unwrap_or(1) as u32;
                st.ready += i["status"]["readyReplicas"].as_u64().unwrap_or(0) as u32;
                for c in i["spec"]["template"]["spec"]["containers"].as_array().cloned().unwrap_or_default() {
                    if let Some(img) = c["image"].as_str() {
                        st.live_images.push(img.to_string());
                    }
                }
            }
            Some("Pod") => st.pods.push(format!("{} {}", i["metadata"]["name"].as_str().unwrap_or("?"), i["status"]["phase"].as_str().unwrap_or("?"))),
            _ => {}
        }
    }
    st.state = if deployments == 0 {
        SvcState::Missing
    } else if st.desired == 0 {
        SvcState::Stopped
    } else if st.ready >= st.desired {
        SvcState::Running
    } else if st.ready == 0 && st.pods.iter().all(|p| p.ends_with("Pending") || p.ends_with("Running")) {
        SvcState::Starting
    } else {
        SvcState::Degraded
    };
    if let Some(def) = spec(&inst.service) {
        let want: Vec<String> = def.images(&inst.version).iter().map(ImageRef::reference).collect();
        if !st.live_images.is_empty() && !want.iter().any(|w| st.live_images.contains(w)) && !inst.options.flag("mirror") && !def.always_mirror() {
            st.notes.push(format!("the running image ({}) differs from the recorded version {}", st.live_images.join(", "), inst.version));
        }
    }
    if st.state == SvcState::Running {
        reach(&mut st);
    }
    st
}

// ------------------------------------------------------------------------------------------------ stop / start

fn deployments(kc: &Kc, inst: &Installed) -> Result<Vec<(String, u64, Option<u64>)>, String> {
    let o = kc.read(&["get", "deployments", "-n", &inst.namespace, "-l", &format!("{LABEL_INSTANCE}={}", inst.id), "-o", "json"]);
    if !o.ok() {
        return Err(first_line(&o.combined()));
    }
    let j: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
    Ok(j["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| (i["metadata"]["name"].as_str().unwrap_or("").to_string(), i["spec"]["replicas"].as_u64().unwrap_or(1), i["metadata"]["annotations"][crate::k8stools::ANNOTATION_REPLICAS].as_str().and_then(|s| s.parse().ok())))
        .filter(|x| !x.0.is_empty())
        .collect())
}

pub fn preview_power(inst: &Installed, start: bool) -> String {
    let c = inst.cluster.as_ref().map(ClusterRef::line).unwrap_or_default();
    if start {
        format!("START {} in namespace {} on {c}: scale its Deployments back to the counts saved before the stop (PVC data and Secrets were kept).", inst.title, inst.namespace)
    } else {
        format!("STOP {} in namespace {} on {c}: scale its Deployments to 0 (the previous counts are kept in the annotation {}; the PersistentVolumeClaim, Services and Secrets stay, so nothing is lost). The service is unavailable until you start it.", inst.title, inst.namespace, crate::k8stools::ANNOTATION_REPLICAS)
    }
}

/// Stop (scale to 0, keeping the counts) or start (scale back) a deployed service. Needs approval.
pub fn power(pl: &Platform, inst: &Installed, start: bool, approved: bool) -> Result<Vec<String>, String> {
    if !approved {
        return Err("not approved".into());
    }
    let c = inst.cluster.as_ref().filter(|_| !inst.connected).ok_or("a connected service is not run by IYAKKI: there is nothing to stop or start")?;
    guard_write(pl, c)?;
    let kc = kc_of(pl, c);
    guard_namespace(&kc, &inst.namespace, &inst.service, false)?;
    let ws = deployments(&kc, inst).map_err(|e| format!("could not read the Deployments: {e}"))?;
    if ws.is_empty() {
        return Err("no Deployment of this service was found in the namespace".into());
    }
    let mut done = Vec::new();
    for (name, replicas, saved) in ws {
        let target = if start {
            if replicas > 0 {
                continue;
            }
            saved.unwrap_or(1)
        } else {
            if replicas == 0 {
                continue;
            }
            let a = kc.kubectl(&["annotate", &format!("deployment/{name}"), "-n", &inst.namespace, &format!("{}={replicas}", crate::k8stools::ANNOTATION_REPLICAS), "--overwrite"], 20, 40);
            if !a.ok() {
                return Err(format!("could not save the replica count of {name}: {}", first_line(&a.combined())));
            }
            0
        };
        let s = kc.kubectl(&["scale", &format!("deployment/{name}"), "-n", &inst.namespace, &format!("--replicas={target}")], 20, 40);
        if !s.ok() {
            return Err(format!("could not scale {name}: {}", first_line(&s.combined())));
        }
        done.push(format!("{name}: {replicas} to {target}"));
    }
    pl.audit(if start { "start" } else { "stop" }, &format!("{}:{}:{} ok", inst.service, inst.namespace, c.server));
    if done.is_empty() {
        done.push(if start { "already running".into() } else { "already stopped".into() });
    }
    Ok(done)
}

// ------------------------------------------------------------------------------------------------ upgrade

#[derive(Clone, Debug)]
pub struct UpgradePlan {
    pub inst: Installed,
    pub from: String,
    pub to: String,
    pub images: Vec<ImageRef>,
    pub refs: Vec<String>,
    pub mirror: Option<MirrorPlan>,
    pub backup_first: bool,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

/// Plan an upgrade to `to` (None: the newest known). Only forward steps along the service's pinned versions are allowed.
pub fn plan_upgrade(pl: &Platform, inst: &Installed, to: Option<&str>, backup_first: bool) -> Result<UpgradePlan, String> {
    let def = spec(&inst.service).ok_or_else(|| format!("unknown service {}", inst.service))?;
    if inst.connected {
        return Err("a connected service is not run by IYAKKI: upgrade it where it runs".into());
    }
    let c = inst.cluster.as_ref().ok_or("this record has no cluster")?;
    let path = def.upgrade_path(&inst.version);
    let to = match to.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => path.last().cloned().ok_or_else(|| format!("{} {} is the newest version IYAKKI knows: nothing to upgrade to", inst.title, inst.version))?,
    };
    if !path.contains(&to) {
        return Err(format!("{to} is not an upgrade of {} {}: the known upgrades are {} (IYAKKI only goes forward along pinned versions; a service cannot be downgraded safely)", inst.title, inst.version, if path.is_empty() { "none".to_string() } else { path.join(", ") }));
    }
    let profile = guard_write(pl, c)?;
    let kc = Kc::new(pl.run.clone(), &profile.kubeconfig, &profile.context);
    let facts = probe(&kc, &profile, &inst.namespace, &inst.id)?;
    let images = def.images(&to);
    let mut u = UpgradePlan { inst: inst.clone(), from: inst.version.clone(), to, refs: images.iter().map(ImageRef::reference).collect(), images, mirror: None, backup_first, warnings: vec![], blockers: vec![] };
    for img in &u.images {
        let bad: Vec<&str> = facts.nodes.iter().filter(|n| n.ready && !n.arch.is_empty() && !img.archs.contains(&n.arch.as_str())).map(|n| n.name.as_str()).collect();
        if !bad.is_empty() {
            u.warnings.push(format!("{} is not published for the architecture of the node(s) {}", img.image, bad.join(", ")));
        }
    }
    if inst.options.flag("mirror") {
        match (&facts.registry_pull, &facts.registry_push, ["podman", "docker"].into_iter().find(|e| pl.run.has(e))) {
            (Some(pull), Some(push), Some(e)) => {
                let mut m = MirrorPlan { engine: e.into(), images: vec![], platform: None };
                let mut refs = vec![];
                for img in &u.images {
                    let target = format!("iyakki-platform/{}", img.repo_and_tag());
                    m.images.push((img.image.clone(), format!("{push}/{target}"), format!("{pull}/{target}")));
                    refs.push(format!("{pull}/{target}"));
                }
                u.refs = refs;
                u.mirror = Some(m);
            }
            _ => u.blockers.push("this service was deployed with mirrored images: the in-cluster registry and podman or docker are needed to mirror the new version".into()),
        }
    }
    if !backup_first {
        u.warnings.push(format!("no backup is taken first. {} A pre-upgrade backup is strongly suggested: a newer version may migrate its data in a way older versions cannot read.", def.backup_hint()));
    }
    Ok(u)
}

pub fn preview_upgrade(u: &UpgradePlan) -> String {
    let c = u.inst.cluster.clone().expect("checked");
    let ns = &u.inst.namespace;
    let mut s = format!("UPGRADE {} from {} to {} on the cluster {}\nNamespace: {ns}\n", u.inst.title, u.from, u.to, c.line());
    for b in &u.blockers {
        s.push_str(&format!("CANNOT BE APPLIED: {b}\n"));
    }
    for w in &u.warnings {
        s.push_str(&format!("warning: {w}\n"));
    }
    s.push_str("\nSTEPS:\n");
    let mut n = 1;
    if u.backup_first {
        s.push_str(&format!("{n}. Back up first (a dump inside the pod; a failed backup stops the upgrade).\n"));
        n += 1;
    }
    if let Some(m) = &u.mirror {
        for (src, push, _) in &m.images {
            s.push_str(&format!("{n}. {} pull {src}, tag and push to {push}\n", m.engine));
            n += 1;
        }
    }
    if let Some(def) = spec(&u.inst.service) {
        for w in def.workloads() {
            let r = u.refs.get(w.image).cloned().unwrap_or_default();
            s.push_str(&format!("{n}. {}\n", kc_line(&c, &format!("set image deployment/{} {}={r} -n {ns}", w.name, w.container))));
            s.push_str(&format!("{}. {}\n", n + 1, kc_line(&c, &format!("rollout status deployment/{} -n {ns} --timeout=300s", w.name))));
            n += 2;
        }
    }
    s.push_str("\nIf the new version does not become ready, IYAKKI stops there and tells you; it does not roll back by itself (the data may already be migrated). Restore from the backup if needed.\n");
    s.trim_end().to_string()
}

pub fn upgrade(pl: &Platform, u: &UpgradePlan, approved: bool, progress: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    if !approved {
        return Err("the upgrade was not approved".into());
    }
    if !u.blockers.is_empty() {
        return Err(format!("not applied: {}", u.blockers.join("; ")));
    }
    let inst = &u.inst;
    let c = inst.cluster.as_ref().ok_or("this record has no cluster")?;
    guard_write(pl, c)?;
    let kc = kc_of(pl, c);
    guard_namespace(&kc, &inst.namespace, &inst.service, false)?;
    let def = spec(&inst.service).ok_or("unknown service")?;
    let mut done = Vec::new();
    pl.audit("upgrade", &format!("{}:{}:{} {}->{} start", inst.service, inst.namespace, c.server, u.from, u.to));
    if u.backup_first {
        progress("taking a backup first");
        let b = backup(pl, inst, false, true).map_err(|e| format!("the backup failed, so the upgrade did not start: {e}"))?;
        done.push(format!("backup taken: {}", b.remote));
    }
    if let Some(m) = &u.mirror {
        mirror_images(pl, m, progress)?;
        done.push("mirrored the new images".into());
    }
    for w in def.workloads() {
        let r = u.refs.get(w.image).cloned().ok_or("no image for the workload")?;
        progress(&format!("setting the image of {}", w.name));
        let o = kc.kubectl(&["set", "image", &format!("deployment/{}", w.name), &format!("{}={r}", w.container), "-n", &inst.namespace], 30, 60);
        if !o.ok() {
            return Err(format!("kubectl set image failed: {}", first_line(&o.combined())));
        }
        let _ = kc.kubectl(&["label", &format!("deployment/{}", w.name), "-n", &inst.namespace, &format!("app.kubernetes.io/version={}", u.to), "--overwrite"], 20, 40);
        progress(&format!("waiting for {}", w.name));
        let r = run_wait(pl, &kc, &inst.namespace, &w.name, 300);
        if !r.ok() {
            return Err(format!("{} did not become ready on {}: {}. It was not rolled back (its data may already be migrated); look at the pods, or restore from the backup.", w.name, u.to, first_line(&r.combined())));
        }
        done.push(format!("{} runs {}", w.name, u.to));
    }
    let mut rec = inst.clone();
    rec.version = u.to.clone();
    rec.updated_at = crate::clock::now_unix();
    rec.options.set("version", &u.to);
    installed::upsert(&pl.dir, &pl.user, rec)?;
    pl.audit("upgrade", &format!("{}:{}:{} ok", inst.service, inst.namespace, c.server));
    Ok(done)
}

// ------------------------------------------------------------------------------------------------ backup

#[derive(Debug, Clone)]
pub struct BackupResult {
    /// The file inside the pod (on the PVC).
    pub remote: String,
    pub local: Option<PathBuf>,
}

pub fn preview_backup(inst: &Installed, copy_local: bool) -> String {
    let def = spec(&inst.service);
    let hint = def.as_ref().map(|d| d.backup_hint()).unwrap_or_default();
    format!("BACK UP {} in namespace {} on {}\n{hint}\nA dump is written inside the pod (on the PersistentVolumeClaim){}. It reads the service's data; nothing is changed or stopped.", inst.title, inst.namespace, inst.cluster.as_ref().map(ClusterRef::line).unwrap_or_default(), if copy_local { format!(", then copied to {}", backup_dir(&PathBuf::from("<your data folder>"), &inst.id).display()) } else { String::new() })
}

pub fn backup_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join("platform-backups").join(id)
}

pub fn backup(pl: &Platform, inst: &Installed, copy_local: bool, approved: bool) -> Result<BackupResult, String> {
    if !approved {
        return Err("the backup was not approved".into());
    }
    let c = inst.cluster.as_ref().filter(|_| !inst.connected).ok_or("a connected service is not run by IYAKKI: back it up where it runs")?;
    guard_write(pl, c)?;
    let kc = kc_of(pl, c);
    guard_namespace(&kc, &inst.namespace, &inst.service, false)?;
    let def = spec(&inst.service).ok_or("unknown service")?;
    let w = def.workloads().into_iter().next().ok_or("this service has no workload")?;
    let plan = def.backup(&w).ok_or_else(|| format!("{} has no built-in backup: {}", inst.title, def.backup_hint()))?;
    let pod = kc.read(&["get", "pods", "-n", &inst.namespace, "-l", &format!("{LABEL_INSTANCE}={}", inst.id), "-o", "jsonpath={.items[0].metadata.name}"]);
    let pod = pod.stdout.trim().to_string();
    if !pod.ok_or_empty() {
        return Err("no running pod of this service was found (start it first)".into());
    }
    let stamp = crate::clock::format(&crate::clock::Zone::system(), crate::clock::now_unix(), "%Y%m%d-%H%M%S");
    let name = plan.name.replace("{stamp}", &stamp);
    let remote = plan.remote_file.replace("{name}", &name);
    let mut args: Vec<String> = vec!["exec".into(), pod.clone(), "-n".into(), inst.namespace.clone(), "-c".into(), w.container.clone(), "--".into()];
    args.extend(plan.exec.iter().map(|a| a.replace("{file}", &remote)));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let o = kc.kubectl(&refs, 30, 900);
    if !o.ok() {
        return Err(format!("the backup command failed: {}", first_line(&o.combined())));
    }
    let mut res = BackupResult { remote: remote.clone(), local: None };
    if copy_local {
        let dir = backup_dir(&pl.dir, &inst.id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let dest = dir.join(&name);
        let o = kc.kubectl(&["cp", &format!("{}/{pod}:{remote}", inst.namespace), &dest.display().to_string(), "-c", &w.container], 60, 900);
        if !o.ok() {
            return Err(format!("the backup was written inside the pod ({remote}) but copying it here failed: {}", first_line(&o.combined())));
        }
        res.local = Some(dest);
    }
    pl.audit("backup", &format!("{}:{}:{} ok", inst.service, inst.namespace, c.server));
    Ok(res)
}

trait OkOrEmpty {
    fn ok_or_empty(&self) -> bool;
}
impl OkOrEmpty for String {
    fn ok_or_empty(&self) -> bool {
        !self.trim().is_empty()
    }
}

// ------------------------------------------------------------------------------------------------ remove

#[derive(Clone, Debug)]
pub struct RemovePlan {
    pub inst: Installed,
    pub delete_data: bool,
}

pub fn plan_remove(inst: &Installed, delete_data: bool) -> RemovePlan {
    RemovePlan { inst: inst.clone(), delete_data }
}

pub fn preview_remove(r: &RemovePlan) -> String {
    let i = &r.inst;
    if i.connected {
        return format!("FORGET the connection to {} ({}): IYAKKI deployed nothing there and removes nothing in any cluster. The record and the vault entries it made for the connection are deleted from your account.", i.title, i.web_url().unwrap_or(""));
    }
    let c = i.cluster.as_ref().map(ClusterRef::line).unwrap_or_default();
    let sel = format!("{LABEL_INSTANCE}={}", i.id);
    let mut s = format!("REMOVE {} from namespace {} on the cluster {c}\n\n", i.title, i.namespace);
    s.push_str(&format!("1. kubectl delete deployments,statefulsets,services,ingresses,configmaps,networkpolicies,jobs -n {} -l {sel}\n", i.namespace));
    if r.delete_data {
        s.push_str(&format!("2. kubectl delete persistentvolumeclaims,secrets -n {} -l {sel}\n", i.namespace));
        s.push_str(&format!("3. delete your vault entries: {}\n", if i.vault.is_empty() { "none".to_string() } else { i.vault.iter().map(|v| format!("uvault:{v}")).collect::<Vec<_>>().join(", ") }));
        if i.created_namespace {
            s.push_str(&format!("4. delete the namespace {} if nothing else is left in it (IYAKKI created it for this service)\n", i.namespace));
        }
        s.push_str("\nWARNING: the PersistentVolumeClaim and everything on it (repositories, databases, uploads) is DELETED and cannot be recovered. Take a backup first.\n");
    } else {
        s.push_str("\nThe PersistentVolumeClaim, the Secrets, the vault entries and the namespace are KEPT, so deploying again restores the service with its data and passwords.\n");
    }
    s.push_str("The record is removed from your installed services in both cases.");
    s
}

/// Remove a deployed service. `confirm` must be the instance id typed by the person. Without `delete_data` the PVC, Secrets, vault entries
/// and namespace stay.
pub fn remove(pl: &Platform, r: &RemovePlan, confirm: &str, approved: bool) -> Result<Vec<String>, String> {
    if !approved {
        return Err("the removal was not approved".into());
    }
    let inst = &r.inst;
    if confirm.trim() != inst.id {
        return Err(format!("not removed: the confirmation does not match. The person must TYPE the service id `{}`; nothing was changed.", inst.id));
    }
    let mut done = Vec::new();
    if inst.connected {
        for v in &inst.vault {
            let _ = pl.vault.delete(v);
        }
        installed::remove(&pl.dir, &pl.user, &inst.id)?;
        pl.audit("remove", &format!("{} connection forgotten", inst.id));
        return Ok(vec!["forgot the connection and its vault entries".into()]);
    }
    let c = inst.cluster.as_ref().ok_or("this record has no cluster")?;
    guard_write(pl, c)?;
    let kc = kc_of(pl, c);
    guard_namespace(&kc, &inst.namespace, &inst.service, false)?;
    pl.audit("remove", &format!("{}:{}:{} start data={}", inst.service, inst.namespace, c.server, r.delete_data));
    let sel = format!("{LABEL_INSTANCE}={}", inst.id);
    let o = kc.kubectl(&["delete", "deployments,statefulsets,services,ingresses,configmaps,networkpolicies,jobs", "-n", &inst.namespace, "-l", &sel, "--wait=true", "--timeout=120s"], 130, 150);
    if !o.ok() {
        return Err(format!("kubectl delete failed: {}", first_line(&o.combined())));
    }
    done.push("deleted the workloads, Services and ConfigMaps".into());
    if r.delete_data {
        let o = kc.kubectl(&["delete", "persistentvolumeclaims,secrets", "-n", &inst.namespace, "-l", &sel, "--wait=true", "--timeout=120s"], 130, 150);
        if !o.ok() {
            return Err(format!("the workloads are gone but deleting the data failed: {}", first_line(&o.combined())));
        }
        done.push("deleted the PersistentVolumeClaim and the Secrets (the data is gone)".into());
        for v in &inst.vault {
            let _ = pl.vault.delete(v);
        }
        done.push("deleted the vault entries".into());
        if inst.created_namespace {
            match guard_namespace(&kc, &inst.namespace, &inst.service, true) {
                Ok(()) => {
                    let left = kc.read(&["get", "deployments,statefulsets,daemonsets,services,ingresses,persistentvolumeclaims,jobs,cronjobs,configmaps,secrets", "-n", &inst.namespace, "--no-headers", "-o", "name"]);
                    let others: Vec<String> = left.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.ends_with("kube-root-ca.crt")).collect();
                    if left.ok() && others.is_empty() {
                        let d = kc.kubectl(&["delete", "namespace", &inst.namespace, "--wait=false"], 20, 60);
                        done.push(if d.ok() { format!("deleted the namespace {}", inst.namespace) } else { format!("could not delete the namespace {}: {}", inst.namespace, first_line(&d.combined())) });
                    } else {
                        done.push(format!("kept the namespace {}: other objects are in it ({})", inst.namespace, others.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
                    }
                }
                Err(e) => done.push(format!("kept the namespace: {e}")),
            }
        }
    }
    installed::remove(&pl.dir, &pl.user, &inst.id)?;
    done.push("removed it from your installed services".into());
    pl.audit("remove", &format!("{}:{}:{} ok", inst.service, inst.namespace, c.server));
    Ok(done)
}

/// Shared for the UI and tools: an `Arc<Platform>` is what they hold.
pub type Shared = Arc<Platform>;
