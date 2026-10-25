//! The assistant's tools for the project's cluster (docs/K3S-DEV.md, phase 1): `k8s_status`, `k8s_get`, `k8s_describe`,
//! `k8s_logs`, `k8s_events`, `k8s_apply`, `k8s_delete`, `k8s_rollout_restart`, `deploy_build`, `deploy_load`, `deploy_status`.
//!
//! They exist only in a workspace chat whose cluster profile answered ([`crate::boardtools::ProjectTools::with_k8s`]) and talk
//! to the cluster through [`crate::devcluster::Kc`] and [`crate::wsexec::Runner`], so tests use a fake executor.
//!
//! * Reads need no approval. Secret values are never read (`get` shows names, type and size; `describe` of a Secret is refused).
//! * Writes ask first and are confined to the workspace's namespace (default `<workspace>-dev`, created on the first approved
//!   apply). A manifest goes through [`crate::deploylint`] before anything else. A cluster that looks like production is read-only.
//! * `deploy_load` never uses `sudo`. For a local k3s it prefers a registry on `localhost:5000` when k3s is set up to use it, and
//!   otherwise imports the image with `k3s ctr images import`, which needs root: when that is refused it explains the two supported
//!   fixes.
//! * Every call is audited as `k8s.<tool>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::deploylint;
use crate::devcluster::{ClusterProfile, Kc};
use crate::k8s::{self, Target};
use crate::wsexec::{Out, Runner, Sink};

pub const TOOLS: [&str; 18] = ["k8s_status", "k8s_get", "k8s_describe", "k8s_logs", "k8s_events", "k8s_apply", "k8s_delete", "k8s_rollout_restart", "deploy_build", "deploy_load", "deploy_status", "deploy_run_job", "registry_setup_help", "cluster_setup_plan", "k8s_stop_env", "k8s_teardown", "k8s_registry_cleanup", "registry_setup"];
pub const REGISTRY: &str = "localhost:5000";
mod podfix;
mod teardown;
mod testrun;
pub use teardown::{protected_namespace, ANNOTATION_REPLICAS, LABEL_MANAGED};
const READ_CAP: usize = 12 * 1024;
const BUILD_SECS: u64 = 900;
const KINDS: &[&str] = &["pods", "po", "deployments", "deploy", "services", "svc", "ingresses", "ing", "configmaps", "cm", "secrets", "statefulsets", "sts", "daemonsets", "ds", "jobs", "cronjobs", "replicasets", "rs", "events", "persistentvolumeclaims", "pvc", "nodes", "namespaces", "ns", "endpoints", "hpa", "all"];
const DELETABLE: &[&str] = &["pods", "deployments", "services", "ingresses", "configmaps", "secrets", "statefulsets", "daemonsets", "jobs", "cronjobs", "persistentvolumeclaims"];

/// How an image reaches the local cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadPath {
    /// `podman push` to `localhost:5000` (no root).
    Registry,
    /// `k3s ctr images import` (root).
    Import,
    /// Not possible from here, with the reason.
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    /// k3s's `registries.yaml` names `localhost:5000`.
    Yes,
    No,
    /// The file exists but cannot be read.
    Unknown,
}

/// The rule for getting an image to the cluster.
pub fn choose_path(local_cluster: bool, registry_up: bool, mirror: Mirror) -> LoadPath {
    if !local_cluster {
        return LoadPath::Unsupported("this cluster is not on this machine, so there is no shared image store to import into; it needs a registry the cluster can pull from, and none was found: set \"registry\" (host:port) under \"k8s\" in .iyakki/project.json, or run a registry Service of type NodePort in the cluster".into());
    }
    if registry_up && mirror == Mirror::Yes {
        LoadPath::Registry
    } else {
        LoadPath::Import
    }
}

#[derive(Default)]
struct State {
    /// Where the output of a running build or load is copied for a live view (the deploy panel).
    live: Option<Sink>,
    /// local image reference -> the reference the cluster uses
    loaded: BTreeMap<String, String>,
    built: Vec<String>,
    /// A registry choice made through `registry_setup` during this session: (push, pull, kind).
    saved: Option<(String, String, String)>,
    /// References built with a test target: never loaded or pushed.
    test_images: Vec<String>,
    /// The result of the last `deploy_run_job`, as data (FLOWS reads the logs and the exit code from it).
    last_job: Option<testrun::JobRun>,
}

pub struct K8sTools {
    pub run: Arc<dyn Runner>,
    pub profile: ClusterProfile,
    pub ns: String,
    pub ws: String,
    pub root: PathBuf,
    /// `(action, detail)` goes to the audit log as `k8s.<action>`.
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// k3s's registry mirror file (a field so tests can point elsewhere).
    pub registries_yaml: PathBuf,
    /// The registry's name for the cluster these tools work against (`cluster "gpu-box" (remote)`, `clusters::ClusterEntry::phrase`); empty:
    /// the resolved profile's own name. Every audit line names it.
    pub cluster_phrase: String,
    /// `host:port` of the registry a REMOTE cluster pulls from (project.json `k8s.registry`); empty: look for one in the cluster.
    pub registry_setting: String,
    /// `k8s.registry_pull`: the address the nodes pull from when it differs from `registry_setting` (`localhost:30500`).
    pub registry_pull: String,
    /// `k8s.registry_kind`: none, in-cluster, system or remote (empty: not chosen).
    pub registry_kind: String,
    state: Mutex<State>,
}

fn dns_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}

fn object_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') && !s.starts_with(['-', '.'])
}

fn selector_ok(s: &str) -> bool {
    s.len() <= 200 && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.,=!/() -".contains(c))
}

fn image_name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 120 && s.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.') && !p.starts_with(['-', '.', '_'])) && !s.contains("..")
}

fn tag_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !s.starts_with(['-', '.'])
}

fn cut(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… cut ({} more bytes)", &s[..end], s.len() - end)
}

fn tail_lines(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().collect();
    v[v.len().saturating_sub(n)..].join("\n")
}

fn arg<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

fn fnv(h: &mut u64, b: &[u8]) {
    for x in b {
        *h ^= *x as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// A short hash of the files under `dir` (names and contents; build output and VCS folders skipped), so each change to the
/// source is a new tag and the rollout happens.
pub fn content_tag(dir: &Path) -> String {
    let mut files = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 8 || out.len() > 5000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut v: Vec<_> = rd.flatten().collect();
        v.sort_by_key(|e| e.file_name());
        for e in v {
            let n = e.file_name().to_string_lossy().into_owned();
            if matches!(n.as_str(), ".git" | "target" | "node_modules" | ".iyakki" | ".venv" | "__pycache__") {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(&p, out, depth + 1);
            } else {
                out.push(p);
            }
        }
    }
    walk(dir, &mut files, 0);
    let mut h = 0xcbf29ce484222325u64;
    let mut total = 0usize;
    for f in files {
        fnv(&mut h, f.strip_prefix(dir).unwrap_or(&f).to_string_lossy().as_bytes());
        if total < 64 * 1024 * 1024 {
            if let Ok(b) = std::fs::read(&f) {
                total += b.len();
                fnv(&mut h, &b);
            }
        }
    }
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

impl K8sTools {
    pub fn new(run: Arc<dyn Runner>, profile: ClusterProfile, ns: &str, ws: &str, root: &Path, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> K8sTools {
        K8sTools { run, profile, ns: ns.into(), ws: ws.into(), root: root.into(), audit, registries_yaml: "/etc/rancher/k3s/registries.yaml".into(), cluster_phrase: String::new(), registry_setting: String::new(), registry_pull: String::new(), registry_kind: String::new(), state: Mutex::new(State::default()) }
    }

    pub fn handles(name: &str) -> bool {
        TOOLS.contains(&name)
    }

    /// Why an "allow for this session" answer never covers this cluster (`None`: it may): only a cluster on this machine
    /// that is not production-looking and that the assistant may write to.
    pub fn grant_blocker(&self) -> Option<String> {
        if !self.profile.local {
            return Some(format!("the cluster \"{}\" is not on this machine, so every change to it needs your answer", self.profile.name));
        }
        self.read_only().map(|_| "a production-looking cluster is never covered by an earlier answer".to_string())
    }

    /// The cluster and namespace a session grant is bound to.
    pub fn grant_target(&self) -> String {
        format!("{}|{}|{}", self.profile.context, self.profile.server, self.ns)
    }

    /// Copy the output of the next builds and loads into `sink` as it arrives.
    pub fn set_live(&self, sink: Option<Sink>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).live = sink;
    }

    fn live(&self) -> Option<Sink> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).live.clone()
    }

    /// The image reference of the latest build.
    pub fn built_ref(&self) -> Option<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).built.last().cloned()
    }

    /// The name the cluster uses for a loaded image.
    pub fn cluster_ref(&self, local: &str) -> Option<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.get(local).cloned()
    }

    fn kc(&self) -> Kc {
        Kc::new(self.run.clone(), &self.profile.kubeconfig, &self.profile.context)
    }

    /// The data of the last Job that `deploy_run_job` ran (taken: a second call returns None).
    pub fn take_last_job(&self) -> Option<testrun::JobRun> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_job.take()
    }

    fn engine(&self) -> Option<&'static str> {
        ["podman", "docker"].into_iter().find(|e| self.run.has(e))
    }

    /// `cluster "gpu-box" (remote)`: the cluster every change and every audit line is about.
    pub fn cluster_label(&self) -> String {
        if !self.cluster_phrase.is_empty() {
            return self.cluster_phrase.clone();
        }
        format!("cluster \"{}\"", if self.profile.name.is_empty() { "unnamed" } else { &self.profile.name })
    }

    fn audit(&self, tool: &str, what: &str) {
        (self.audit)(tool, &format!("{}:{}:{} on {}", self.ws, self.ns, what, self.cluster_label()));
    }

    fn read_only(&self) -> Option<String> {
        crate::devcluster::read_only_reason(&self.profile)
    }

    pub fn defs(&self) -> Vec<ToolDef> {
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
        let t = |name: &str, desc: String, schema: Value, risk: Risk, untrusted: bool| ToolDef { spec: ToolSpec { name: name.into(), description: desc, schema }, risk, untrusted };
        let ns = &self.ns;
        vec![
            t("k8s_status", format!("The Kubernetes cluster connected to this workspace: profile, context, namespace \"{ns}\", whether it answers, what you may do. Call it first. The cluster is ALREADY running: never start, install or stop k3s."), obj(json!({}), &[]), Risk::Safe, false),
            t("k8s_get", "List objects (read-only): `kind` (pods, deployments, services, ingresses, configmaps, secrets (names only), statefulsets, daemonsets, jobs, cronjobs, events, pvc, nodes, namespaces, all), optional `name`, `namespace` (default the workspace's), `selector` like app=web.".into(), obj(json!({"kind": {"type": "string"}, "name": {"type": "string"}, "namespace": {"type": "string"}, "selector": {"type": "string"}}), &["kind"]), Risk::Safe, true),
            t("k8s_describe", "Describe one object in plain kubectl detail (read-only; Secrets are refused).".into(), obj(json!({"kind": {"type": "string"}, "name": {"type": "string"}, "namespace": {"type": "string"}}), &["kind", "name"]), Risk::Safe, true),
            t("k8s_logs", "The log of a pod (read-only): `pod`, optional `container`, `tail` lines (default 100, most 500), `previous` for the last crashed run.".into(), obj(json!({"pod": {"type": "string"}, "container": {"type": "string"}, "tail": {"type": "integer"}, "previous": {"type": "boolean"}, "namespace": {"type": "string"}}), &["pod"]), Risk::Safe, true),
            t("k8s_events", "The latest events in the namespace (read-only), oldest first: why pods are not starting.".into(), obj(json!({"namespace": {"type": "string"}}), &[]), Risk::Safe, true),
            t("k8s_apply", format!("Apply a manifest to namespace \"{ns}\": `manifest_path` (a YAML file in the workspace, such as deploy/app.yaml) or `yaml` (text, saved to deploy/). It is checked first (no privileged containers, hostPath, host networking, added capabilities, cluster-scoped kinds or other namespaces; `imagePullPolicy: IfNotPresent` is set for images you loaded with deploy_load). The person sees the check and `kubectl diff` and approves. Creates the namespace on the first apply."), obj(json!({"manifest_path": {"type": "string"}, "yaml": {"type": "string"}}), &[]), Risk::Ask, false),
            t("k8s_delete", format!("Delete one object in namespace \"{ns}\" by `kind` and `name` (never a whole namespace or --all). The person approves."), obj(json!({"kind": {"type": "string"}, "name": {"type": "string"}}), &["kind", "name"]), Risk::Ask, false),
            t("k8s_rollout_restart", format!("Restart a deployment in namespace \"{ns}\" (rolling): `name`, optional `kind` (deployment, statefulset, daemonset). The person approves."), obj(json!({"name": {"type": "string"}, "kind": {"type": "string"}}), &["name"]), Risk::Ask, false),
            t("deploy_build", "Build a container image from a folder of the workspace with podman (docker if podman is not installed): `dir` (holds a Dockerfile or Containerfile AND the files it COPYs, such as package.json and the app: keep them in one folder; a COPY that matches no file is refused before any build), `image` name (lowercase, no registry), optional `tag` (default a hash of the files, so every change rolls out). Optional `target` builds one stage of a multi-stage Dockerfile (`--target`); a test target (a stage named test, unit-test…) builds the UNIT-TEST image `<image>-<target>:<tag>`, which runs the tests and coverage while building: the result is \"TEST BUILD (not for deploy)\" with the pass/fail counts, the coverage % and the last test output, a failing test returns the failing output as the error, and that image is never passed to deploy_load. Optional `build_args` (object) become `--build-arg`. Returns the image reference to use. The person approves the command.".into(), obj(json!({"dir": {"type": "string"}, "image": {"type": "string"}, "tag": {"type": "string"}, "dockerfile": {"type": "string"}, "target": {"type": "string"}, "build_args": {"type": "object"}}), &["dir", "image"]), Risk::Ask, false),
            t("deploy_load", "Make a built image available to the local cluster: pushes it to a local registry when k3s uses one, otherwise imports it into k3s (no sudo is ever used; if that needs root, it tells you the one-time fix). `image` is the reference deploy_build returned. Returns the image name the manifest must use. The person approves.".into(), obj(json!({"image": {"type": "string"}}), &["image"]), Risk::Ask, false),
            t("deploy_status", "Is the deployment healthy (read-only): `name` of a deployment to wait for its rollout (up to 20 s), or nothing for the namespace overview. Pods that cannot start come with a plain-English reason.".into(), obj(json!({"name": {"type": "string"}}), &[]), Risk::Safe, true),
            t("deploy_run_job", format!("Run an INTEGRATION-TEST image as a Kubernetes Job in namespace \"{ns}\" after the rollout, wait for it, and return pass/fail (from the Job status), the pod logs, the pass/fail counts and coverage found in them; the Job is deleted afterwards unless `keep` is true. `image` is the integration-test image (deploy_build it from tests/integration, then deploy_load and use the name it returns; on a remote cluster a bare localhost/ name cannot be pulled), optional `name`, `command` (list), `env` (object; BASE_URL defaults to the cluster DNS name of the single Service), `timeout_seconds` (default 300, at most 1800), `keep`, `allow_root` (default false: the Job runs as non-root). A timeout reports why it is not finishing (ImagePullBackOff, Pending…). Only the workspace namespace; the person approves."), obj(json!({"image": {"type": "string"}, "name": {"type": "string"}, "command": {"type": "array", "items": {"type": "string"}}, "env": {"type": "object"}, "timeout_seconds": {"type": "integer"}, "keep": {"type": "boolean"}, "allow_root": {"type": "boolean"}}), &["image"]), Risk::Ask, false),
            t("registry_setup_help", "Read-only: for the registry this cluster pulls from (the project's, the cluster's own, or `registry` host:port), the cause and ready-to-copy fix when nodes cannot pull (plain HTTP registry: /etc/rancher/k3s/registries.yaml, the commands, and the restart of k3s / k3s-agent on EACH node). IYAKKI never edits /etc or logs in to nodes: tell the person to run it. Never suggest Docker Hub.".into(), obj(json!({"registry": {"type": "string"}}), &[]), Risk::Safe, false),
            t("cluster_setup_plan", "Read-only plan text for the registry option of a k3s setup: `option` none | in-cluster (A: a registry pod in namespace iyakki-registry with a NodePort, optional `node_port`; no root, no node changes; recommended) | system (B: a registry:2 container on localhost:5000, only for a cluster on this machine) | remote (C: `registry` host:port, `scheme` http or https, `insecure`). Shows the exact objects, registries.yaml, the commands (root ones for the person to run), which parts need root and the project setting it would record. Nothing is changed.".into(), obj(json!({"option": {"type": "string"}, "registry": {"type": "string"}, "node_port": {"type": "integer"}, "scheme": {"type": "string"}, "insecure": {"type": "boolean"}}), &["option"]), Risk::Safe, false),
            t("k8s_stop_env", format!("Stop or start the workspace's dev environment in namespace \"{ns}\": `action` stop (scale every Deployment and StatefulSet to 0, keeping the previous replica counts in the annotation iyakki.dev/replicas; Services, ConfigMaps, Secrets and PVC data stay) or start (scale back). Only the namespace IYAKKI created for this workspace (labelled managed-by=iyakki). The person approves."), obj(json!({"action": {"type": "string"}}), &[]), Risk::Ask, false),
            t("k8s_teardown", format!("REMOVE ALL: delete the namespace \"{ns}\" and EVERYTHING in it (deployments, services, pods, PVCs and their DATA, configmaps, secrets), then wait for it to disappear and report leftovers. Only the namespace IYAKKI created for this workspace (labels managed-by=iyakki and iyakki.dev/workspace); default, kube-*, iyakki-registry and unlabelled namespaces are refused. ALWAYS asks, in every approval mode: the card lists exactly what goes. `confirm` must be the namespace name typed by the person (ask them with ask_user; never fill it in yourself). Use this to clean up when the dev task is done; never run kubectl delete namespace by hand and never try k8s_delete kind=all."), obj(json!({"confirm": {"type": "string"}}), &["confirm"]), Risk::Ask, false),
            t("k8s_registry_cleanup", format!("Delete the images pushed for this workspace (repositories under \"{ns}/\") from the registry IYAKKI uses. Optional, separate from k8s_teardown, always asks. It needs a registry that allows deletion; it never changes the registry's settings."), obj(json!({}), &[]), Risk::Ask, false),
            t("registry_setup", "Set up the registry the cluster pulls images from, as chosen by the person (same arguments as cluster_setup_plan; call that first and show the plan). `in-cluster` creates the namespace iyakki-registry (cluster-scoped: the person's approval covers it) and a registry:2 Deployment, PVC and NodePort Service with kubectl; `system` starts a registry:2 container with podman/docker; `remote` records an existing registry. It never uses sudo: a registries.yaml that needs root is returned as commands for the person to run. Records the choice in project.json so deploy_load uses it. The person approves.".into(), obj(json!({"option": {"type": "string"}, "registry": {"type": "string"}, "node_port": {"type": "integer"}, "scheme": {"type": "string"}, "insecure": {"type": "boolean"}}), &["option"]), Risk::Ask, false),
        ]
    }

    pub fn describe(&self, name: &str, args: &Value) -> String {
        let s = |k: &str| {
            let v = arg(args, k);
            if v.is_empty() { "?".to_string() } else { v.to_string() }
        };
        let ctx = format!("namespace {} on {}", self.ns, self.profile.chip());
        let ro = self.read_only().map(|_| " — will be REFUSED: production-looking cluster").unwrap_or("");
        match name {
            "k8s_status" => "check the cluster connection".into(),
            "k8s_get" => format!("list {} in {}", s("kind"), if arg(args, "namespace").is_empty() { self.ns.clone() } else { arg(args, "namespace").to_string() }),
            "k8s_describe" => format!("describe {} {}", s("kind"), s("name")),
            "k8s_logs" => format!("read the log of {}", s("pod")),
            "k8s_events" => "read the events".into(),
            "k8s_apply" => {
                let src = if !arg(args, "manifest_path").is_empty() { arg(args, "manifest_path").to_string() } else { "the YAML you wrote".to_string() };
                format!("apply {src} to {ctx}{ro}")
            }
            "k8s_delete" => format!("DELETE {} {} from {ctx}{ro}", s("kind"), s("name")),
            "k8s_rollout_restart" => format!("restart {} {} in {ctx}{ro}", if arg(args, "kind").is_empty() { "deployment" } else { arg(args, "kind") }, s("name")),
            "deploy_build" => {
                let (engine, argv) = match self.build_plan(args) {
                    Ok(p) => (p.0, p.1.join(" ")),
                    Err(e) => ("?", format!("({e})")),
                };
                format!("build the image from {} with {engine}: `{argv}`", s("dir"))
            }
            "deploy_load" => {
                let plan = self.load_plan(arg(args, "image"));
                format!("load {} into the cluster: {}{ro}", s("image"), plan.0)
            }
            "deploy_status" => "check the deployment".into(),
            "registry_setup_help" => "show how the nodes can pull from the registry".into(),
            "cluster_setup_plan" => "plan the registry option of the cluster setup (nothing is changed)".into(),
            "deploy_run_job" => format!("run {} as a Job in namespace {} (timeout {} s) and wait for the result{ro}", s("image"), self.ns, args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(300).clamp(1, 1800)),
            "k8s_stop_env" => format!("{} the dev environment in namespace {}{ro}", if arg(args, "action") == "start" { "START" } else { "STOP (scale to 0)" }, self.ns),
            "k8s_teardown" => format!("REMOVE ALL: delete namespace {} and everything in it, including PVC data (type the name to confirm){ro}", self.ns),
            "k8s_registry_cleanup" => format!("delete the images pushed for namespace {} from the registry{ro}", self.ns),
            "registry_setup" => format!("set up the registry (option {}): {}{ro}", s("option"), match self.setup_parts(args, false) {
                Ok(p) => crate::regsetup::root_line(&p),
                Err(e) => format!("({e})"),
            }),
            _ => name.into(),
        }
    }

    pub fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "k8s_apply" => Some(match self.prepare_apply(args, false) {
                Ok(p) if p.only_ns => format!("the manifest only holds the Namespace {}: it is created automatically on the first apply, so there is nothing else to apply", self.ns),
                Ok(p) => {
                    let diff = self.diff(&p.file);
                    format!("namespace {} on {}\n{}\n\n{}", self.ns, self.profile.describe(), if p.lint.report().is_empty() { "manifest check: clean".to_string() } else { format!("manifest check:\n{}", p.lint.report()) }, diff)
                }
                Err(e) => format!("this would be refused: {e}"),
            }),
            "deploy_build" => Some(match self.build_plan(args) {
                Ok((_, argv, dir, tag)) => format!("in {}\n{}\nimage tag {tag} (a hash of the files)", dir.display(), argv.join(" ")),
                Err(e) => format!("this would be refused: {e}"),
            }),
            "deploy_load" => Some(self.load_plan(arg(args, "image")).1),
            "deploy_run_job" => Some(self.run_job_preview(args)),
            "k8s_stop_env" => Some(self.stop_env_preview(args)),
            "k8s_teardown" => Some(self.teardown_preview()),
            "k8s_registry_cleanup" => Some(self.registry_cleanup_preview()),
            "registry_setup" => Some(match self.setup_parts(args, true) {
                Ok(p) => crate::regsetup::preview(&p),
                Err(e) => format!("this would be refused: {e}"),
            }),
            "k8s_delete" => Some(format!("kubectl delete {} {} -n {} (context {})", arg(args, "kind"), arg(args, "name"), self.ns, self.profile.context)),
            _ => None,
        }
    }

    pub fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let r = self.dispatch(name, args);
        let tool = name;
        match &r {
            Ok(_) => self.audit(tool, &format!("ok {}", args.to_string().chars().take(160).collect::<String>())),
            Err(e) => self.audit(tool, &format!("failed {}", e.chars().take(120).collect::<String>())),
        }
        r
    }

    fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "k8s_status" => Ok(self.status()),
            "k8s_get" => self.get(args),
            "k8s_describe" => self.describe_obj(args),
            "k8s_logs" => self.logs(args),
            "k8s_events" => self.events(args),
            "k8s_apply" => self.apply(args),
            "k8s_delete" => self.delete(args),
            "k8s_rollout_restart" => self.restart(args),
            "deploy_build" => self.build(args),
            "deploy_load" => self.load(args),
            "deploy_status" => self.deploy_status(args),
            "registry_setup_help" => self.registry_help(args),
            "cluster_setup_plan" => self.setup_plan(args),
            "registry_setup" => self.registry_setup(args),
            "deploy_run_job" => self.run_job(args),
            "k8s_stop_env" => self.stop_env(args),
            "k8s_teardown" => self.teardown(args),
            "k8s_registry_cleanup" => self.registry_cleanup(args),
            other => Err(format!("unknown tool `{other}`")),
        }
    }

    // -------------------------------------------------------------------------------------------- reads

    fn ns_arg(&self, args: &Value) -> Result<String, String> {
        let n = arg(args, "namespace");
        if n.is_empty() {
            return Ok(self.ns.clone());
        }
        if !dns_label(n) {
            return Err(format!("`{n}` is not a namespace name"));
        }
        Ok(n.to_string())
    }

    fn out_text(o: &Out) -> String {
        if o.missing {
            return "kubectl is not installed".into();
        }
        if o.timed_out {
            return "kubectl timed out".into();
        }
        o.combined()
    }

    fn status(&self) -> String {
        let kc = self.kc();
        let v = kc.kubectl(&["version", "-o", "json"], 3, 8);
        let version = serde_json::from_str::<Value>(&v.stdout).ok().and_then(|j| j["serverVersion"]["gitVersion"].as_str().map(str::to_string));
        let reachable = version.is_some();
        let ns_state = if reachable {
            let n = kc.kubectl(&["get", "namespace", &self.ns, "-o", "name"], 3, 8);
            if n.ok() { "exists".to_string() } else if n.stderr.contains("NotFound") || n.stderr.contains("not found") { "does not exist yet (it is created on the first approved k8s_apply)".to_string() } else { format!("unknown ({})", n.stderr.lines().next().unwrap_or("")) }
        } else {
            "unknown".into()
        };
        let can_i = if self.profile.can_i.is_empty() { String::new() } else { format!("\nidentity: {}", self.profile.can_i) };
        format!(
            "Kubernetes cluster connected to workspace \"{}\":\nprofile: {}\ncontext: {}\nserver: {}\nreachable: {}{}\nnamespace: {} ({})\nlocal k3s: {}{}\nWrites (apply, delete, restart, build, load) ask the person first and only touch namespace {}.\nThe cluster is already running: do not start, install or stop k3s.",
            self.ws,
            self.profile.describe(),
            self.profile.context,
            self.profile.server,
            if reachable { "yes" } else { "NO — the cluster did not answer" },
            version.map(|v| format!(" ({v})")).unwrap_or_default(),
            self.ns,
            ns_state,
            if self.profile.k3s && self.profile.local { "yes (images can be imported or pushed to localhost:5000)" } else { "no" },
            can_i,
            self.ns
        )
    }

    fn get(&self, args: &Value) -> Result<String, String> {
        let kind = arg(args, "kind").to_lowercase();
        if !KINDS.contains(&kind.as_str()) {
            return Err(format!("kind `{kind}` is not one of: {}", KINDS.join(", ")));
        }
        let ns = self.ns_arg(args)?;
        let mut a: Vec<String> = vec!["get".into(), kind.clone()];
        let name = arg(args, "name");
        if !name.is_empty() {
            if !object_name(name) {
                return Err(format!("`{name}` is not an object name"));
            }
            a.push(name.into());
        }
        if !matches!(kind.as_str(), "nodes" | "namespaces" | "ns") {
            a.push("-n".into());
            a.push(ns);
        }
        let sel = arg(args, "selector");
        if !sel.is_empty() {
            if !selector_ok(sel) {
                return Err("the selector has characters that are not allowed".into());
            }
            a.push("-l".into());
            a.push(sel.into());
        }
        // the table form: names, status, sizes; never the values of a Secret
        a.push("-o".into());
        a.push("wide".into());
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        let o = self.kc().read(&refs);
        if o.ok() {
            Ok(if o.stdout.trim().is_empty() { format!("No {kind} found in that namespace.\n{}", o.stderr.trim()) } else { cut(&o.stdout, READ_CAP) })
        } else {
            Err(cut(&Self::out_text(&o), 2000))
        }
    }

    fn describe_obj(&self, args: &Value) -> Result<String, String> {
        let kind = arg(args, "kind").to_lowercase();
        if matches!(kind.as_str(), "secret" | "secrets") {
            return Err("describing a Secret is refused: the assistant never reads secret values".into());
        }
        if !KINDS.contains(&kind.as_str()) && !KINDS.contains(&format!("{kind}s").as_str()) {
            return Err(format!("kind `{kind}` is not one of: {}", KINDS.join(", ")));
        }
        let name = arg(args, "name");
        if !object_name(name) {
            return Err(format!("`{name}` is not an object name"));
        }
        let ns = self.ns_arg(args)?;
        let mut a = vec!["describe", kind.as_str(), name];
        if !matches!(kind.as_str(), "node" | "nodes" | "namespace" | "namespaces" | "ns") {
            a.extend(["-n", ns.as_str()]);
        }
        let o = self.kc().read(&a);
        if o.ok() { Ok(self.with_diagnosis(cut(&o.stdout, READ_CAP))) } else { Err(cut(&Self::out_text(&o), 2000)) }
    }

    fn logs(&self, args: &Value) -> Result<String, String> {
        let pod = arg(args, "pod");
        if !object_name(pod) {
            return Err(format!("`{pod}` is not a pod name"));
        }
        let ns = self.ns_arg(args)?;
        let tail = args.get("tail").and_then(Value::as_u64).unwrap_or(100).clamp(1, 500).to_string();
        let mut a = vec!["logs", pod, "-n", ns.as_str(), "--tail", tail.as_str()];
        let c = arg(args, "container");
        if !c.is_empty() {
            if !object_name(c) {
                return Err(format!("`{c}` is not a container name"));
            }
            a.extend(["-c", c]);
        }
        if args.get("previous").and_then(Value::as_bool).unwrap_or(false) {
            a.push("--previous");
        }
        let mut o = self.kc().read(&a);
        let mut note = String::new();
        // a pod that no longer exists (a rollout replaced it): use the current pod of the same deployment and say so
        if !o.ok() && (o.stderr.contains("NotFound") || o.stderr.contains("not found")) {
            if let Some(cur) = self.current_pod_like(pod, &ns) {
                let mut b = vec!["logs", cur.as_str(), "-n", ns.as_str(), "--tail", tail.as_str()];
                if !c.is_empty() {
                    b.extend(["-c", c]);
                }
                let o2 = self.kc().read(&b);
                if o2.ok() {
                    note = format!("(pod {pod} no longer exists, a rollout replaced it; this is the log of the current pod {cur})\n");
                    o = o2;
                }
            }
        }
        if !o.ok() {
            return Err(cut(&Self::out_text(&o), 2000));
        }
        if o.stdout.trim().is_empty() {
            return Ok(format!("{note}The container wrote nothing to stdout or stderr (the app may simply not log). Do not call k8s_logs again for this pod. Check instead: readiness with deploy_status, the `reachable at:` address with http_request, and k8s_events."));
        }
        // the newest lines matter most: the tail is kept, about 1,500 tokens at most
        let body: String = if o.stdout.chars().count() > 6000 { format!("[earlier lines cut]\n{}", o.stdout.chars().skip(o.stdout.chars().count() - 6000).collect::<String>()) } else { o.stdout.clone() };
        Ok(format!("{note}{body}"))
    }

    /// The newest pod whose name starts with the deployment part of `pod` (`web-7d9f-abcde` gives `web-`), when it is another pod.
    /// Capture the evidence of a failure into `dir`: every container's log (current and previous), the pod descriptions, the events, the pod
    /// status (env values redacted), the manifests in effect, the Dockerfile(s) of the workspace and the images. Read-only; nothing leaves
    /// the workspace namespace. `pods` empty: every pod of the namespace (at most 8).
    pub fn capture_evidence(&self, dir: &Path, pods: &[String], lines: usize, since: &str) -> Result<crate::bugs::Evidence, String> {
        use crate::bugs::{write_evidence, Evidence};
        let ns = self.ns.clone();
        let lines = lines.clamp(1, crate::bugs::MAX_LINES).to_string();
        let kc = self.kc();
        let listed = kc.read(&["get", "pods", "-n", &ns, "-o", "json"]);
        if !listed.ok() {
            return Err(cut(&Self::out_text(&listed), 300));
        }
        let all: Value = serde_json::from_str(&listed.stdout).map_err(|e| format!("the pod list could not be read: {e}"))?;
        let mut ev = Evidence::default();
        let items: Vec<Value> = all["items"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| {
            let n = p["metadata"]["name"].as_str().unwrap_or("");
            pods.is_empty() || pods.iter().any(|w| n == w || n.starts_with(&format!("{w}-")))
        }).take(8).collect();
        let since_ok = !since.is_empty() && since.len() <= 8 && since.chars().all(|c| c.is_ascii_alphanumeric());
        for mut pod in items {
            let name = pod["metadata"]["name"].as_str().unwrap_or("").to_string();
            if !object_name(&name) {
                continue;
            }
            ev.pods.push(name.clone());
            for key in ["containerStatuses", "initContainerStatuses"] {
                for cs in pod["status"][key].as_array().into_iter().flatten() {
                    for p in ["/state/waiting/reason", "/state/terminated/reason", "/lastState/terminated/reason"] {
                        if let Some(r) = cs.pointer(p).and_then(Value::as_str).filter(|r| *r != "Completed") {
                            if !ev.reasons.iter().any(|x| x == r) {
                                ev.reasons.push(r.to_string());
                            }
                        }
                    }
                    if let Some(i) = cs["imageID"].as_str().filter(|i| !i.is_empty()) {
                        if !ev.images.iter().any(|x| x == i) {
                            ev.images.push(i.to_string());
                        }
                    }
                }
            }
            for c in pod["spec"]["containers"].as_array().into_iter().flatten() {
                if let Some(i) = c["image"].as_str() {
                    if ev.images.is_empty() || !ev.images.iter().any(|x| x.contains(i)) {
                        ev.images.push(i.to_string());
                    }
                }
            }
            let mut containers: Vec<String> = Vec::new();
            for key in ["initContainers", "containers"] {
                containers.extend(pod["spec"][key].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str().map(str::to_string)));
            }
            crate::bugs::redact_pod_json(&mut pod);
            if let Ok(f) = write_evidence(dir, &format!("{name}.status.json"), &serde_json::to_string_pretty(&pod).unwrap_or_default()) {
                ev.files.push(f);
            }
            for c in containers.iter().filter(|c| object_name(c)) {
                for previous in [false, true] {
                    let mut a: Vec<&str> = vec!["logs", name.as_str(), "-n", ns.as_str(), "-c", c.as_str(), "--tail", lines.as_str(), "--timestamps"];
                    if since_ok {
                        a.extend(["--since", since]);
                    }
                    if previous {
                        a.push("--previous");
                    }
                    let o = kc.read(&a);
                    if o.ok() {
                        let text = if o.stdout.trim().is_empty() { "(the container wrote nothing to stdout or stderr)".to_string() } else { o.stdout.clone() };
                        if let Ok(f) = write_evidence(dir, &format!("{name}__{c}{}.log", if previous { ".previous" } else { "" }), &text) {
                            ev.files.push(f);
                        }
                    } else if !previous {
                        if let Ok(f) = write_evidence(dir, &format!("{name}__{c}.log"), &format!("(the log could not be read: {})", cut(&Self::out_text(&o), 300))) {
                            ev.files.push(f);
                        }
                    }
                }
            }
            let d = kc.read(&["describe", "pod", name.as_str(), "-n", ns.as_str()]);
            if let Ok(f) = write_evidence(dir, &format!("{name}.describe.txt"), &Self::out_text(&d)) {
                ev.files.push(f);
            }
        }
        let e = kc.read(&["get", "events", "-n", ns.as_str(), "--sort-by=.lastTimestamp"]);
        if let Ok(f) = write_evidence(dir, "events.txt", &Self::out_text(&e)) {
            ev.files.push(f);
        }
        let m = kc.read(&["get", "deployments,services", "-n", ns.as_str(), "-o", "yaml"]);
        if let Ok(f) = write_evidence(dir, "manifest-in-effect.yaml", &Self::out_text(&m)) {
            ev.files.push(f);
        }
        if !ev.images.is_empty() {
            if let Ok(f) = write_evidence(dir, "images.txt", &ev.images.join("\n")) {
                ev.files.push(f);
            }
        }
        // the Dockerfile(s) of the workspace: the root and one folder down
        let mut dockerfiles = vec![self.root.join("Dockerfile")];
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
            subs.sort();
            dockerfiles.extend(subs.into_iter().take(10).map(|d| d.join("Dockerfile")));
        }
        for d in dockerfiles.into_iter().filter(|d| d.is_file()) {
            let rel = d.strip_prefix(&self.root).map(|p| p.display().to_string()).unwrap_or_default().replace('/', "__");
            if let Ok(t) = std::fs::read_to_string(&d) {
                if let Ok(f) = write_evidence(dir, &format!("{rel}.txt"), &t) {
                    ev.files.push(f);
                }
            }
        }
        Ok(ev)
    }

    fn current_pod_like(&self, pod: &str, ns: &str) -> Option<String> {
        let dep = pod.rsplitn(3, '-').nth(2)?;
        let o = self.kc().read(&["get", "pods", "-n", ns, "--sort-by=.metadata.creationTimestamp", "-o", "name"]);
        if !o.ok() {
            return None;
        }
        o.stdout.lines().filter_map(|l| l.strip_prefix("pod/")).filter(|n| n.starts_with(&format!("{dep}-")) && *n != pod).last().map(str::to_string)
    }

    fn events(&self, args: &Value) -> Result<String, String> {
        let ns = self.ns_arg(args)?;
        let o = self.kc().read(&["get", "events", "-n", &ns, "--sort-by=.lastTimestamp"]);
        if !o.ok() {
            return Err(cut(&Self::out_text(&o), 2000));
        }
        let lines: Vec<&str> = o.stdout.lines().collect();
        if lines.len() <= 1 {
            return Ok("No events in that namespace.".into());
        }
        let body = lines[1..].len().saturating_sub(40);
        Ok(self.with_diagnosis(format!("{}\n{}", lines[0], lines[1 + body..].join("\n"))))
    }

    /// The text, plus the cause and exact fix when it holds a registry pull error (plain HTTP, x509, no credentials, refused).
    fn with_diagnosis(&self, text: String) -> String {
        let (push, pull, _) = self.saved_registry();
        let d: Vec<String> = [crate::regsetup::diagnose(&text, if pull.is_empty() { &push } else { &pull }), self.pod_diagnosis(&text)].into_iter().flatten().collect();
        if d.is_empty() { text } else { format!("{text}\n\nDiagnosis: {}", d.join("\n")) }
    }

    fn registry_help(&self, args: &Value) -> Result<String, String> {
        let given = arg(args, "registry").trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string();
        let reg = if !given.is_empty() {
            given
        } else if self.local_cluster() && self.registry_setting.trim().is_empty() {
            REGISTRY.to_string()
        } else {
            self.remote_registry().or_else(|e| if self.registry_setting.trim().is_empty() { Err(e) } else { Ok(self.registry_setting.trim().trim_end_matches('/').to_string()) })?
        };
        if !reg.rsplit_once(':').is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok()) {
            return Err(format!("`{reg}` is not a registry address (host:port)"));
        }
        Ok(crate::regsetup::setup_help(&reg))
    }

    /// The saved registry choice: (where this machine pushes, what the nodes pull, kind). A choice made by `registry_setup` in this
    /// session wins over the values the tools were created with.
    pub fn saved_registry(&self) -> (String, String, String) {
        let (push, pull, kind) = match self.state.lock().unwrap_or_else(|e| e.into_inner()).saved.clone() {
            Some(s) => s,
            None => (self.registry_setting.trim().trim_end_matches('/').to_string(), self.registry_pull.trim().trim_end_matches('/').to_string(), self.registry_kind.trim().to_string()),
        };
        if self.system_on_remote_with(&push, &kind) {
            // the nodes of another machine cannot reach this machine's localhost: that choice is not used
            return (String::new(), String::new(), kind);
        }
        (push, pull, kind)
    }

    fn system_on_remote_with(&self, push: &str, kind: &str) -> bool {
        let host = push.rsplit_once(':').map_or(push, |x| x.0);
        !self.local_cluster() && !push.is_empty() && matches!(host, "localhost" | "127.0.0.1" | "[::1]") && kind != "in-cluster"
    }

    /// The saved registry is on THIS machine (`localhost:5000`) but the cluster is on another one.
    pub fn system_on_remote(&self) -> bool {
        let (push, kind) = match self.state.lock().unwrap_or_else(|e| e.into_inner()).saved.clone() {
            Some(s) => (s.0, s.2),
            None => (self.registry_setting.trim().to_string(), self.registry_kind.trim().to_string()),
        };
        self.system_on_remote_with(&push, &kind)
    }

    /// Option A was chosen but not applied yet (kind set, no address).
    fn in_cluster_pending(&self) -> bool {
        let (push, _, kind) = self.saved_registry();
        kind == "in-cluster" && push.is_empty()
    }

    fn registry_hint(&self) -> String {
        if self.system_on_remote() {
            "The saved registry is localhost:5000 on THIS machine, which the nodes of this remote cluster cannot reach, so it is not used. ".to_string()
        } else if self.in_cluster_pending() {
            "The in-cluster registry (option A) was chosen but is not set up yet: ".to_string()
        } else {
            String::new()
        }
    }

    /// A registry other than the system one on localhost:5000 is set, so images go through it even for a cluster on this machine.
    fn configured_registry(&self) -> bool {
        let (push, _, _) = self.saved_registry();
        !push.is_empty() && (!self.local_cluster() || push != REGISTRY)
    }

    /// The parsed option and its plan. `facts`: read the cluster (storage class, NodePorts in use) for an in-cluster plan.
    fn setup_parts(&self, args: &Value, facts: bool) -> Result<crate::regsetup::SetupPlan, String> {
        let scheme = if arg(args, "scheme").is_empty() { "http" } else { arg(args, "scheme") };
        let port = args["node_port"].as_u64().map(|n| n.to_string()).unwrap_or_default();
        let hostport = if matches!(arg(args, "option").to_lowercase().as_str(), "in-cluster" | "incluster" | "in_cluster" | "cluster" | "pod") { port.as_str() } else { arg(args, "registry") };
        let opt = crate::regsetup::RegistryOption::parse(arg(args, "option"), hostport, scheme, args["insecure"].as_bool().unwrap_or(false))?;
        if opt == crate::regsetup::RegistryOption::Local && !self.local_cluster() {
            return Err("the system registry (B) only fits a cluster on this machine: its node pulls localhost:5000. Use the in-cluster registry (A) or an existing registry (C)".into());
        }
        let mut env = crate::regsetup::PlanEnv::real(self.engine(), Some(&self.root));
        env.registries_yaml = self.registries_yaml.clone();
        let kube = crate::regsetup::KubeCtx { kubeconfig: self.profile.kubeconfig.clone(), context: self.profile.context.clone() };
        env.api_host = if self.profile.local { String::new() } else { server_host(&self.profile.server) };
        if facts && matches!(opt, crate::regsetup::RegistryOption::InCluster { .. }) {
            env.facts = crate::regsetup::probe(&*self.run, &kube);
        }
        env.kube = Some(kube);
        crate::regsetup::plan(&opt, &env)
    }

    fn setup_plan(&self, args: &Value) -> Result<String, String> {
        let p = self.setup_parts(args, true)?;
        Ok(format!("{}\n\nThis is a plan only. Nothing was changed. To do it, call registry_setup with the same arguments (the person approves), or the person chooses K8S panel \u{2192} Registry\u{2026} \u{2192} Set up registry\u{2026}.", crate::regsetup::preview(&p)))
    }

    fn registry_setup(&self, args: &Value) -> Result<String, String> {
        let p = self.setup_parts(args, true)?;
        if matches!(p.option, crate::regsetup::RegistryOption::InCluster { .. }) {
            if let Some(e) = self.read_only() {
                return Err(e);
            }
        }
        let a = crate::regsetup::apply(&p, &*self.run, true)?;
        if p.option == crate::regsetup::RegistryOption::None {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).saved = Some((String::new(), String::new(), "none".into()));
        } else if !p.registry.is_empty() {
            let pull = if p.pull == p.registry { String::new() } else { p.pull.clone() };
            self.state.lock().unwrap_or_else(|e| e.into_inner()).saved = Some((p.registry.clone(), pull, p.option.kind().to_string()));
        }
        let mut out = format!("Registry option {} done:\n- {}", p.option.kind(), if a.done.is_empty() { "nothing to do".to_string() } else { a.done.join("\n- ") });
        if !a.to_run.is_empty() {
            out.push_str(&format!("\n\nIYAKKI does not use sudo. For the nodes to use {} the person must run, as root:\n{}", p.registry, a.to_run.join("\n")));
        }
        if !p.pull.is_empty() && p.pull != p.registry {
            out.push_str(&format!("\n\ndeploy_load now pushes to {} and the manifest uses {}/<namespace>/<image>:<tag> (pods pull localhost).", p.registry, p.pull));
        } else if !p.registry.is_empty() {
            out.push_str(&format!("\n\ndeploy_load now pushes to {}.", p.registry));
        }
        Ok(out)
    }

    fn deploy_status(&self, args: &Value) -> Result<String, String> {
        let kc = self.kc();
        let name = arg(args, "name");
        let mut text = String::new();
        if !name.is_empty() {
            if !object_name(name) {
                return Err(format!("`{name}` is not a deployment name"));
            }
            let o = kc.kubectl(&["rollout", "status", &format!("deployment/{name}"), "-n", &self.ns, "--timeout=20s"], 8, 40);
            text.push_str(&format!("rollout of {name}: {}\n", if o.ok() { "complete".to_string() } else { first_line(&Self::out_text(&o)) }));
        }
        let t = Target { namespace: self.ns.clone(), selector: String::new(), context: self.profile.context.clone(), kubeconfig: self.profile.kubeconfig.clone() };
        let snap = k8s::fetch(&crate::devcluster::RunnerKube { run: self.run.clone() }, &t, 0);
        match &snap.cluster {
            k8s::Cluster::Ok => {}
            other => return Err(format!("{text}the cluster is not usable: {other:?}")),
        }
        if !snap.namespace_found {
            return Ok(format!("{text}Namespace {} does not exist yet. It is created by the first approved k8s_apply.", self.ns));
        }
        for w in &snap.workloads {
            text.push_str(&format!("{} {}: {}/{} ready, {:?}, images {}\n", w.kind, w.name, w.ready, w.desired, w.health, w.images.join(", ")));
        }
        for s in &snap.services {
            text.push_str(&format!("Service {} {} {}{} {}\n", s.name, s.kind, s.cluster_ip, if s.cluster_ip != "None" && s.cluster_ip.starts_with("10.") { " (cluster-internal address)" } else { "" }, s.ports));
        }
        for l in k8s::reachable_lines(&snap) {
            text.push_str(&format!("{l}\n"));
        }
        for p in &snap.pods {
            text.push_str(&format!("Pod {}: {} ready {} restarts {}{}\n", p.name, p.phase, p.ready, p.restarts, if p.reason.is_empty() { String::new() } else { format!(" — {}", p.reason) }));
        }
        for n in &snap.notes {
            text.push_str(&format!("note: {n}\n"));
        }
        if snap.workloads.is_empty() && snap.pods.is_empty() {
            text.push_str("Nothing is deployed in this namespace yet.\n");
        }
        let mut diagnosis = String::new();
        if snap.pods.iter().any(|p| matches!(p.reason.as_str(), "ImagePullBackOff" | "ErrImagePull" | "CreateContainerConfigError" | "CreateContainerError" | "InvalidImageName")) {
            let ev = kc.read(&["get", "events", "-n", &self.ns]);
            let (push, pull, _) = self.saved_registry();
            for d in [crate::regsetup::diagnose(&ev.combined(), if pull.is_empty() { &push } else { &pull }), self.pod_diagnosis(&ev.combined())].into_iter().flatten() {
                text.push_str(&format!("\nDiagnosis: {d}\n"));
                diagnosis.push_str(&d);
            }
        }
        if let Some(n) = self.next_action(&snap, &diagnosis) {
            text.push_str(&format!("\n{n}\n"));
        }
        Ok(text.trim_end().to_string())
    }

    // -------------------------------------------------------------------------------------------- apply, delete, restart

    fn prepare_apply(&self, args: &Value, write_files: bool) -> Result<Prepared, String> {
        let (text, src) = if !arg(args, "manifest_path").is_empty() {
            let p = crate::workspace::resolve_in(&self.root, arg(args, "manifest_path"))?;
            let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", arg(args, "manifest_path")))?;
            if meta.len() > 512 * 1024 {
                return Err("the manifest is larger than 512 KiB".into());
            }
            (std::fs::read_to_string(&p).map_err(|e| e.to_string())?, arg(args, "manifest_path").to_string())
        } else if let Some(y) = args.get("yaml").and_then(Value::as_str).filter(|y| !y.trim().is_empty()) {
            if y.len() > 512 * 1024 {
                return Err("the YAML is larger than 512 KiB".into());
            }
            (y.to_string(), "deploy/inline.yaml".to_string())
        } else {
            return Err("give `manifest_path` (a YAML file in the workspace) or `yaml`".into());
        };
        // IYAKKI creates the workspace namespace itself: a Namespace document for it is dropped, the rest is applied
        let (text, skipped) = strip_own_namespace(&text, &self.ns);
        if !skipped.is_empty() && text.lines().all(|l| l.trim().is_empty() || l.trim() == "---") {
            return Ok(Prepared { lint: deploylint::Lint::default(), file: PathBuf::new(), skipped, only_ns: true });
        }
        let loaded = self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.clone();
        let is_local = |img: &str| {
            let bare = img.strip_prefix("localhost/").unwrap_or(img);
            img.starts_with("localhost/") || img.starts_with("localhost:") || loaded.iter().any(|(k, v)| k == img || v == img || k.strip_prefix("localhost/") == Some(bare))
        };
        let mut lint = deploylint::lint(&text, &self.ns, &is_local);
        if lint.ok() {
            let extra = self.apply_warnings(&text, &lint.images);
            lint.warnings.extend(extra);
        }
        if !lint.ok() {
            return Err(format!("the manifest was not applied; fix these first:\n{}", lint.report()));
        }
        if !self.local_cluster() {
            if let Some(img) = lint.images.iter().find(|i| i.starts_with("localhost/") || i.starts_with("localhost:5000/")) {
                return Err(format!("the manifest was not applied: the image `{img}` exists only on this machine, and the nodes of this remote cluster cannot pull it (it has no registry address). Run deploy_load for the built image and use the image name it returns (registry address, namespace, tag){}", if self.registry_hint().is_empty() { "; if it reports no registry, use Registry… (K8S panel) and pick A".to_string() } else { format!(". {}", self.registry_hint().trim_end_matches(": ")) }));
            }
        }
        let dir = self.root.join(".iyakki/deploy");
        let stem = Path::new(&src).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "manifest".into());
        let file = dir.join(format!("{stem}.checked.yaml"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&file, &lint.text).map_err(|e| e.to_string())?;
        if write_files && arg(args, "manifest_path").is_empty() {
            // text the assistant wrote is kept in the workspace, where the person can read and edit it
            let name = lint.objects.first().map(|o| o.1.clone()).filter(|n| object_name(n)).unwrap_or_else(|| "manifest".into());
            let saved = self.root.join("deploy");
            std::fs::create_dir_all(&saved).map_err(|e| e.to_string())?;
            std::fs::write(saved.join(format!("{name}.yaml")), &text).map_err(|e| e.to_string())?;
        }
        Ok(Prepared { lint, file, skipped, only_ns: false })
    }

    fn diff(&self, file: &Path) -> String {
        let f = file.display().to_string();
        let o = self.kc().kubectl(&["diff", "-f", &f, "-n", &self.ns], 8, 60);
        match o.code {
            Some(0) => "kubectl diff: no changes (everything is already as written)".into(),
            Some(1) => format!("kubectl diff:\n{}", cut(&o.stdout, 6000)),
            _ => {
                let e = Self::out_text(&o);
                if e.contains("namespaces \"") && e.contains("not found") || e.contains("NotFound") {
                    format!("namespace {} does not exist yet and will be created; all objects are new", self.ns)
                } else {
                    format!("kubectl diff could not run: {}", first_line(&e))
                }
            }
        }
    }

    fn apply(&self, args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let p = self.prepare_apply(args, true)?;
        let kc = self.kc();
        let mut text = String::new();
        let ns = kc.kubectl(&["get", "namespace", &self.ns, "-o", "name"], 5, 10);
        if !ns.ok() {
            if ns.stderr.contains("NotFound") || ns.stderr.contains("not found") {
                let c = kc.kubectl(&["create", "namespace", &self.ns], 8, 20);
                if !c.ok() {
                    return Err(format!("could not create namespace {}: {}", self.ns, first_line(&Self::out_text(&c))));
                }
                // IYAKKI marks the namespace it creates: Stop and Remove all only work on a namespace carrying these labels
                let l = kc.kubectl(&["label", "namespace", &self.ns, &format!("{}=iyakki", teardown::LABEL_MANAGED), &format!("{}={}", teardown::LABEL_WORKSPACE, self.ws), "--overwrite"], 8, 20);
                text.push_str(&format!("created namespace {}{}\n", self.ns, if l.ok() { " (labelled as managed by IYAKKI)" } else { " (could not label it, so Stop and Remove all will refuse it)" }));
            } else {
                return Err(format!("could not check namespace {}: {}", self.ns, first_line(&Self::out_text(&ns))));
            }
        }
        let skipped_note = if p.skipped.is_empty() { String::new() } else { format!("\nskipped Namespace {}: it is created automatically (it exists now)", self.ns) };
        if p.only_ns {
            return Ok(format!("{text}{}\nThere is nothing else in this manifest to apply: write the Deployment and Service (without a Namespace) and apply them.", skipped_note.trim_start()));
        }
        let f = p.file.display().to_string();
        let o = kc.kubectl(&["apply", "-f", &f, "-n", &self.ns], 8, 90);
        if !o.ok() {
            return Err(cut(&format!("{text}kubectl apply failed:\n{}", Self::out_text(&o)), 3000));
        }
        text.push_str(&o.stdout);
        text.push_str(&skipped_note);
        for n in &p.lint.notes {
            text.push_str(&format!("\nnote: {n}"));
        }
        for w in &p.lint.warnings {
            text.push_str(&format!("\nwarning: {w}"));
        }
        text.push_str(&format!("\n\nApplied to namespace {}. Check it with deploy_status{}.", self.ns, p.lint.objects.iter().find(|o| o.0 == "Deployment").map(|o| format!(" name={}", o.1)).unwrap_or_default()));
        Ok(text)
    }

    fn delete(&self, args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let kind = arg(args, "kind").to_lowercase();
        let kind = if kind.ends_with('s') { kind } else { format!("{kind}s") };
        let kind = match kind.as_str() {
            "ingresss" => "ingresses".to_string(),
            "persistentvolumeclaimss" => kind,
            _ => kind,
        };
        if !DELETABLE.contains(&kind.as_str()) {
            return Err(format!("only these can be deleted one by one: {} (never namespaces, nodes or --all). To remove everything in the workspace namespace use k8s_teardown (it lists what goes and asks; never run kubectl delete namespace by hand and never try kind=all); to stop it without deleting use k8s_stop_env.", DELETABLE.join(", ")));
        }
        let name = arg(args, "name");
        if !object_name(name) {
            return Err(format!("`{name}` is not an object name"));
        }
        let o = self.kc().kubectl(&["delete", &kind, name, "-n", &self.ns], 8, 60);
        if o.ok() { Ok(o.stdout.trim().to_string()) } else { Err(cut(&Self::out_text(&o), 2000)) }
    }

    fn restart(&self, args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let kind = match arg(args, "kind").to_lowercase().as_str() {
            "" | "deployment" | "deployments" | "deploy" => "deployment",
            "statefulset" | "statefulsets" | "sts" => "statefulset",
            "daemonset" | "daemonsets" | "ds" => "daemonset",
            other => return Err(format!("`{other}` cannot be restarted (deployment, statefulset, daemonset)")),
        };
        let name = arg(args, "name");
        if !object_name(name) {
            return Err(format!("`{name}` is not an object name"));
        }
        let o = self.kc().kubectl(&["rollout", "restart", &format!("{kind}/{name}"), "-n", &self.ns], 8, 30);
        if o.ok() { Ok(format!("{}\nWatch it with deploy_status name={name}.", o.stdout.trim())) } else { Err(cut(&Self::out_text(&o), 2000)) }
    }

    // -------------------------------------------------------------------------------------------- build and load

    /// (engine, argv, build directory, tag)
    fn build_plan(&self, args: &Value) -> Result<(&'static str, Vec<String>, PathBuf, String), String> {
        let engine = self.engine().ok_or("neither podman nor docker is installed; install podman (preferred) and try again")?;
        let dir = crate::workspace::resolve_in(&self.root, arg(args, "dir"))?;
        if !dir.is_dir() {
            return Err(if dir.exists() { crate::scope::explain_io(&std::io::Error::from_raw_os_error(20), &dir.join("Dockerfile"), "build in") } else { crate::scope::explain_io(&std::io::Error::from_raw_os_error(2), &dir, "build in") });
        }
        let given = arg(args, "dockerfile");
        let df = if given.is_empty() { ["Dockerfile", "Containerfile"].into_iter().find(|f| dir.join(f).is_file()).map(str::to_string) } else { Some(given.to_string()) };
        let Some(df) = df else { return Err(format!("{} has no Dockerfile or Containerfile", arg(args, "dir"))) };
        let dfp = crate::workspace::resolve_in(&dir, &df)?;
        if !dfp.is_file() {
            return Err(format!("{df} does not exist"));
        }
        // before any build: does the Dockerfile's COPY find its files in this folder (and is the target a stage)?
        let target = arg(args, "target");
        if !target.is_empty() && (target.len() > 63 || !target.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))) {
            return Err(format!("`{target}` is not a usable build target (a stage name: letters, digits, - _ .)"));
        }
        crate::dockerlint::check_target(&dir, &dfp, &self.root, target).map_err(|e| format!("not built: {e}"))?;
        let mut image = arg(args, "image").trim_start_matches("localhost/").to_string();
        if !image_name_ok(&image) {
            return Err(format!("`{}` is not a usable image name (lowercase letters, digits, - _ . and / only; no registry or tag)", arg(args, "image")));
        }
        let tag = if arg(args, "tag").is_empty() { content_tag(&dir) } else { arg(args, "tag").to_string() };
        if !tag_ok(&tag) {
            return Err(format!("`{tag}` is not a usable tag"));
        }
        // a test stage is built into its own image name, so it can never be mistaken for the app
        if testrun::is_test_target(target) {
            image = format!("{image}-{target}");
            if !image_name_ok(&image) {
                return Err(format!("`{image}` is not a usable image name"));
            }
        }
        let reference = format!("localhost/{image}:{tag}");
        let mut argv = vec![engine.to_string(), "build".into(), "-t".into(), reference];
        if !target.is_empty() {
            argv.extend(["--target".to_string(), target.to_string()]);
        }
        argv.extend(testrun::build_args(args)?);
        argv.extend(["-f".to_string(), dfp.display().to_string(), ".".to_string()]);
        Ok((engine, argv, dir, tag))
    }

    fn build(&self, args: &Value) -> Result<String, String> {
        let (engine, argv, dir, tag) = self.build_plan(args)?;
        let o = self.run.run(&dir, &argv, &[], Duration::from_secs(BUILD_SECS), 4 * 1024 * 1024, self.live());
        let reference = argv[3].clone();
        if testrun::is_test_target(arg(args, "target")) {
            return self.finish_test_build(&reference, arg(args, "target"), engine, &o);
        }
        if !o.ok() {
            let (summary, clean, note) = crate::dockerlint::build_failure_summary(&Self::out_text(&o));
            let why = if o.timed_out { format!("the build took longer than {BUILD_SECS} s and was stopped") } else if summary.is_empty() { "the build failed".to_string() } else if summary.starts_with("failed at") { format!("the build {summary}") } else { format!("the build failed: {summary}") };
            return Err(format!("{why} ({engine}). Last lines:\n{}{}", tail_lines(&clean, 40), note.map(|n| format!("\n{n}")).unwrap_or_default()));
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).built.push(reference.clone());
        let warn = match arg_dockerfile_warnings(&dir, &argv, &self.root) {
            w if w.is_empty() => String::new(),
            w => format!("\nWarning: {}", w.join("\nWarning: ")),
        };
        Ok(format!("Built {reference} with {engine} (tag {tag}).{warn}\nNext: deploy_load image={reference}, then use that image name in the manifest and k8s_apply it.\nLast lines of the build:\n{}", tail_lines(&Self::out_text(&o), 8)))
    }

    fn registry_up(&self) -> bool {
        self.run.has("curl") && self.run.run(&std::env::temp_dir(), &["curl".into(), "-fsS".into(), "--max-time".into(), "2".into(), format!("http://{REGISTRY}/v2/")], &[], Duration::from_secs(5), 4096, None).ok()
    }

    fn mirror(&self) -> Mirror {
        match std::fs::read_to_string(&self.registries_yaml) {
            Ok(t) if t.contains(REGISTRY) => Mirror::Yes,
            Ok(_) => Mirror::No,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Mirror::No,
            Err(_) => Mirror::Unknown,
        }
    }

    fn local_cluster(&self) -> bool {
        self.profile.k3s && self.profile.local
    }

    /// (one line for the approval, the commands for the preview)
    fn load_plan(&self, image: &str) -> (String, String) {
        let Some(engine) = self.engine() else { return ("neither podman nor docker is installed".into(), "install podman first".into()) };
        let r = reference(image);
        if self.is_test_image(&r) {
            return ("cannot: test images are never loaded or pushed".into(), "build the runtime image without a target".into());
        }
        if !self.local_cluster() || self.configured_registry() {
            return match self.remote_registry() {
                Ok(reg) => {
                    let target = remote_ref(&reg, &self.ns, &r);
                    let pulls = self.explicit_pull_ref(&r).or_else(|| self.nodeport_pull_ref(&reg, &r));
                    (format!("push it to the cluster's registry at {reg}; the nodes pull {}", pulls.as_deref().unwrap_or(&target)), format!("{engine} tag {r} {target}\n{engine} push {}{target}", if engine == "podman" { "--tls-verify=false " } else { "" }))
                }
                Err(why) => (format!("cannot: {why}"), why),
            };
        }
        if self.in_cluster_pending() {
            let why = format!("{}use Registry… (K8S panel) and approve the plan first", self.registry_hint());
            return (format!("cannot: {why}"), why);
        }
        match choose_path(self.local_cluster(), self.registry_up(), self.mirror()) {
            LoadPath::Registry => {
                let target = registry_ref(&r);
                (format!("push it to the registry at {REGISTRY} (k3s pulls from it)"), format!("{engine} tag {r} {target}\n{engine} push {}{target}", if engine == "podman" { "--tls-verify=false " } else { "" }))
            }
            LoadPath::Import => ("import it into the local k3s".into(), format!("{engine} save -o <workspace>/.iyakki/tmp/image.tar {r}\nk3s ctr images import <that file>\n(needs root; no sudo is used)")),
            LoadPath::Unsupported(why) => (format!("cannot: {why}"), why),
        }
    }

    fn load(&self, args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let engine = self.engine().ok_or("neither podman nor docker is installed; install podman (preferred) and try again")?;
        let r = reference(arg(args, "image"));
        if r.is_empty() || r.len() > 200 || r.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("give the image reference deploy_build returned".into());
        }
        if self.is_test_image(&r) {
            return Err(format!("{r} is a test image (built with a test target): test images are never loaded or pushed. Build the runtime image with deploy_build without a target, then deploy_load that."));
        }
        let exists = if engine == "podman" { vec!["podman".to_string(), "image".into(), "exists".into(), r.clone()] } else { vec!["docker".to_string(), "image".into(), "inspect".into(), r.clone()] };
        if !self.run.run(&self.root, &exists, &[], Duration::from_secs(20), 64 * 1024, None).ok() {
            return Err(format!("{engine} has no image {r}; build it first with deploy_build"));
        }
        if !self.local_cluster() || self.configured_registry() {
            return self.load_remote(engine, &r);
        }
        if self.in_cluster_pending() {
            return Err(format!("cannot load {r}: {}use Registry… (K8S panel) or registry_setup option in-cluster, and approve the plan", self.registry_hint()));
        }
        let registry_up = self.registry_up();
        let path = choose_path(self.local_cluster(), registry_up, self.mirror());
        match path {
            LoadPath::Unsupported(why) => Err(format!("cannot load {r}: {why}")),
            LoadPath::Registry => {
                let target = registry_ref(&r);
                let tag = self.run.run(&self.root, &[engine.into(), "tag".into(), r.clone(), target.clone()], &[], Duration::from_secs(30), 64 * 1024, None);
                if !tag.ok() {
                    return Err(format!("{engine} tag failed: {}", first_line(&Self::out_text(&tag))));
                }
                let mut push = vec![engine.to_string(), "push".to_string()];
                if engine == "podman" {
                    push.push("--tls-verify=false".into());
                }
                push.push(target.clone());
                let o = self.run.run(&self.root, &push, &[], Duration::from_secs(300), 1024 * 1024, None);
                if !o.ok() {
                    return Err(format!("push to {REGISTRY} failed: {}\nIf the registry is not an open one, or k3s does not use it, use the import path instead.", tail_lines(&Self::out_text(&o), 10)));
                }
                self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.insert(r.clone(), target.clone());
                Ok(format!("Pushed {target} to the local registry. k3s pulls it from there (no root needed).\nUse image: {target} in the manifest."))
            }
            LoadPath::Import => {
                let dir = self.root.join(".iyakki/tmp");
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let tar = dir.join(format!("image-{}.tar", content_tag_str(&r)));
                let save = self.run.run(&self.root, &[engine.into(), "save".into(), "-o".into(), tar.display().to_string(), r.clone()], &[], Duration::from_secs(300), 1024 * 1024, None);
                if !save.ok() {
                    let _ = std::fs::remove_file(&tar);
                    return Err(format!("{engine} save failed: {}", first_line(&Self::out_text(&save))));
                }
                let imp = self.run.run(&self.root, &["k3s".into(), "ctr".into(), "images".into(), "import".into(), tar.display().to_string()], &[], Duration::from_secs(300), 1024 * 1024, None);
                let _ = std::fs::remove_file(&tar);
                if !imp.ok() {
                    return Err(self.import_failure(&r, &imp, registry_up));
                }
                self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.insert(r.clone(), r.clone());
                Ok(format!("Imported {r} into the local k3s.\nUse image: {r} in the manifest (k8s_apply sets imagePullPolicy: IfNotPresent for it, because it is not in any registry)."))
            }
        }
    }

    /// The registry a remote cluster pulls from: the project setting, else a NodePort Service whose name says registry,
    /// reached on the API server's host. It must answer `GET /v2/`.
    pub fn remote_registry(&self) -> Result<String, String> {
        let saved = self.saved_registry().0;
        let reg = if !saved.is_empty() {
            saved.trim_start_matches("http://").to_string()
        } else {
            let o = self.kc().read(&["get", "services", "-A", "-o", "json"]);
            let port = o.ok().then(|| find_registry_port(&o.stdout)).flatten();
            let host = server_host(&self.profile.server);
            match (port, host.is_empty()) {
                (Some(p), false) => format!("{host}:{p}"),
                _ => return Err(format!("{}this cluster is not on this machine, so there is no shared image store to import into, and no registry was found: offer the person registry_setup (A, a registry pod inside the cluster, needs no root; K8S panel \u{2192} Registry\u{2026}), or set \"registry\" (host:port) under \"k8s\" in .iyakki/project.json", self.registry_hint())),
            }
        };
        if !self.run.has("curl") {
            return Ok(reg);
        }
        let probe = self.run.run(&std::env::temp_dir(), &["curl".into(), "-fsS".into(), "--max-time".into(), "4".into(), format!("http://{reg}/v2/")], &[], Duration::from_secs(8), 4096, None);
        if probe.ok() {
            Ok(reg)
        } else {
            Err(format!("the registry {reg} did not answer GET /v2/ from this machine ({}); check that it is reachable, or set \"registry\" under \"k8s\" in .iyakki/project.json", first_line(&Self::out_text(&probe))))
        }
    }

    /// For a registry found as a NodePort Service (no `k8s.registry` setting): the reference the NODES pull, `localhost:<nodePort>/<ns>/<name>:<tag>`.
    fn nodeport_pull_ref(&self, reg: &str, r: &str) -> Option<String> {
        if !self.saved_registry().0.is_empty() {
            return None;
        }
        let port = reg.rsplit_once(':')?.1;
        (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then(|| remote_ref(&format!("localhost:{port}"), &self.ns, r))
    }

    /// The saved pull address (`k8s.registry_pull`, set by the registry setup) applied to an image: what the NODES pull.
    fn explicit_pull_ref(&self, r: &str) -> Option<String> {
        let (push, pull, _) = self.saved_registry();
        (!pull.is_empty() && pull != push).then(|| remote_ref(&pull, &self.ns, r))
    }

    /// Nodes that are not Ready, as a warning for the result of a push (the registry address may not be what they see).
    fn node_warning(&self) -> String {
        let o = self.kc().read(&["get", "nodes", "-o", "json"]);
        if !o.ok() {
            return String::new();
        }
        let bad = not_ready_nodes(&o.stdout);
        if bad.is_empty() {
            String::new()
        } else {
            format!("\nWarning: these nodes are NotReady: {}. Pods scheduled there cannot pull the image; pin the Deployment to a Ready node (nodeSelector) if it stays Pending.", bad.join(", "))
        }
    }

    fn load_remote(&self, engine: &'static str, r: &str) -> Result<String, String> {
        let note = if self.system_on_remote() { format!("Note: {}The discovered NodePort registry was used instead; to choose properly run Registry… and pick A.\n", self.registry_hint()) } else { String::new() };
        let reg = self.remote_registry().map_err(|e| format!("cannot load {r}: {e}"))?;
        let target = remote_ref(&reg, &self.ns, r);
        let tag = self.run.run(&self.root, &[engine.into(), "tag".into(), r.into(), target.clone()], &[], Duration::from_secs(30), 64 * 1024, None);
        if !tag.ok() {
            return Err(format!("{engine} tag failed: {}", first_line(&Self::out_text(&tag))));
        }
        let mut push = vec![engine.to_string(), "push".to_string()];
        if engine == "podman" {
            push.push("--tls-verify=false".into());
        }
        push.push(target.clone());
        let o = self.run.run(&self.root, &push, &[], Duration::from_secs(600), 1024 * 1024, None);
        if !o.ok() {
            let t = Self::out_text(&o);
            let http = t.contains("server gave HTTP response to HTTPS client") || t.contains("insecure");
            let hint = if http && engine == "docker" {
                format!("\nDocker refuses plain HTTP registries unless the daemon allows it. One-time fix (IYAKKI does not edit /etc): add \"insecure-registries\": [\"{reg}\"] to /etc/docker/daemon.json and run `sudo systemctl restart docker`.")
            } else {
                String::new()
            };
            return Err(format!("push to {reg} failed: {}{hint}", tail_lines(&t, 10)));
        }
        if let Some(local) = self.explicit_pull_ref(r).or_else(|| self.nodeport_pull_ref(&reg, r)) {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.insert(r.to_string(), local.clone());
            self.audit("deploy_load", &format!("{r} -> {target} (pull as {local})"));
            return Ok(format!("{note}Pushed {r} to the cluster's registry as {target}.\nUse image: {local} in the manifest (deploy/*.yaml), not {r} and not {target}. The registry is a NodePort Service, which every node reaches at localhost:<nodePort>, and containerd allows plain HTTP for localhost, so the nodes pull it with no node configuration (the address {reg} is only for this machine: the nodes' containerd would try HTTPS there and fail). Caveat: this works on nodes where kube-proxy exposes the NodePort on localhost (the default iptables/ipvs mode) and only for nodes that are Ready; otherwise use the registries.yaml mirror (registry_setup_help).{}", self.node_warning()));
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded.insert(r.to_string(), target.clone());
        self.audit("deploy_load", &format!("{r} -> {target}"));
        Ok(format!("{note}Pushed {r} to the cluster's registry as {target}.\nUse image: {target} in the manifest (deploy/*.yaml), not {r}; the nodes pull it from {reg}.{}", self.node_warning()))
    }

    fn import_failure(&self, r: &str, o: &Out, registry_up: bool) -> String {
        let t = Self::out_text(o);
        let denied = t.contains("permission denied") || t.contains("Permission denied") || t.contains("operation not permitted") || t.contains("not permitted");
        if o.missing {
            return "k3s is not on the PATH, so the image cannot be imported. Install k3s or use a local registry.".into();
        }
        if !denied {
            return format!("k3s ctr images import failed: {}", tail_lines(&t, 8));
        }
        let mut s = format!("Importing {r} into k3s needs root (containerd's socket is root-only), and IYAKKI never uses sudo. Two supported fixes, once:\n1. A local registry (no root afterwards): `podman run -d --restart=always -p 5000:5000 --name registry docker.io/library/registry:2`, then as root create /etc/rancher/k3s/registries.yaml with\n   mirrors:\n     \"{REGISTRY}\":\n       endpoint:\n         - \"http://{REGISTRY}\"\n   and `sudo systemctl restart k3s` yourself. After that deploy_load pushes to {REGISTRY} automatically.\n2. A sudoers rule for exactly this command, e.g. in /etc/sudoers.d/iyakki: `{} ALL=(root) NOPASSWD: /usr/local/bin/k3s ctr images import *` (IYAKKI would still not run sudo itself; you run the import from a terminal).\n", std::env::var("USER").unwrap_or_else(|_| "youruser".into()));
        if registry_up {
            s.push_str(&format!("A registry is already answering on {REGISTRY}; only the k3s mirror file is missing (fix 1, second half).\n"));
        }
        s.push_str(&format!("Raw error: {}", first_line(&t)));
        s
    }
}

struct Prepared {
    lint: deploylint::Lint,
    file: PathBuf,
    /// Namespace documents dropped because IYAKKI creates that namespace itself.
    skipped: Vec<String>,
    /// The manifest held nothing but that Namespace.
    only_ns: bool,
}

/// Drop the `kind: Namespace` documents named `ns` from a manifest; returns the rest and the names dropped.
fn strip_own_namespace(text: &str, ns: &str) -> (String, Vec<String>) {
    let mut docs: Vec<String> = vec![String::new()];
    for l in text.lines() {
        if l.trim_end() == "---" {
            docs.push(String::new());
        } else {
            let d = docs.last_mut().expect("never empty");
            d.push_str(l);
            d.push('\n');
        }
    }
    let (mut kept, mut skipped) = (Vec::new(), Vec::new());
    for d in docs {
        let field = |k: &str| d.lines().find_map(|l| l.trim().strip_prefix(k)).map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        let top_kind = d.lines().find_map(|l| l.strip_prefix("kind:")).map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        if top_kind.as_deref() == Some("Namespace") && field("name:").as_deref() == Some(ns) {
            skipped.push(ns.to_string());
        } else {
            kept.push(d);
        }
    }
    let rest = kept.into_iter().filter(|d| d.lines().any(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))).collect::<Vec<_>>().join("---\n");
    (rest, skipped)
}

/// Warnings of the Dockerfile pre-flight (the Dockerfile is the argument after `-f`).
fn arg_dockerfile_warnings(dir: &Path, argv: &[String], root: &Path) -> Vec<String> {
    let df = argv.iter().position(|a| a == "-f").and_then(|i| argv.get(i + 1)).map(PathBuf::from);
    df.and_then(|d| crate::dockerlint::check(dir, &d, root).ok()).unwrap_or_default()
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(240).collect()
}

/// `web:abc` and `localhost/web:abc` are the same local image.
fn reference(image: &str) -> String {
    let i = image.trim();
    if i.is_empty() || i.starts_with("localhost/") || i.starts_with("localhost:") || i.split('/').next().is_some_and(|f| f.contains('.')) {
        i.to_string()
    } else {
        format!("localhost/{i}")
    }
}

/// `localhost/web:abc` -> `localhost:5000/web:abc`
fn registry_ref(r: &str) -> String {
    format!("{REGISTRY}/{}", r.strip_prefix("localhost/").unwrap_or(r))
}

/// `localhost/web:abc` -> `192.168.1.68:30500/shop-dev/web:abc`
fn remote_ref(reg: &str, ns: &str, r: &str) -> String {
    format!("{reg}/{ns}/{}", r.strip_prefix("localhost/").unwrap_or(r))
}

/// `https://192.168.1.68:6443` -> `192.168.1.68`; `https://[::1]:6443` -> `[::1]`
fn server_host(server: &str) -> String {
    let rest = server.split_once("://").map_or(server, |x| x.1);
    let hostport = rest.split('/').next().unwrap_or("");
    if hostport.starts_with('[') {
        return hostport.split_once(']').map(|x| format!("{}]", x.0)).unwrap_or_default();
    }
    hostport.split(':').next().unwrap_or("").to_string()
}

/// The first NodePort of a Service whose name contains "registry" (the list is `kubectl get services -A -o json`).
fn find_registry_port(json_text: &str) -> Option<u16> {
    let j: Value = serde_json::from_str(json_text).ok()?;
    j["items"].as_array()?.iter().filter(|s| s["metadata"]["name"].as_str().is_some_and(|n| n.contains("registry")) && s["spec"]["type"].as_str() == Some("NodePort")).find_map(|s| s["spec"]["ports"].as_array()?.iter().find_map(|p| p["nodePort"].as_u64().map(|n| n as u16)))
}

fn not_ready_nodes(json_text: &str) -> Vec<String> {
    let Ok(j) = serde_json::from_str::<Value>(json_text) else { return vec![] };
    j["items"].as_array().map(|a| a.iter().filter(|n| !n["status"]["conditions"].as_array().is_some_and(|c| c.iter().any(|x| x["type"] == "Ready" && x["status"] == "True"))).filter_map(|n| n["metadata"]["name"].as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn content_tag_str(s: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    fnv(&mut h, s.as_bytes());
    format!("{:08x}", h & 0xffff_ffff)
}

/// A first Deployment and Service for an app listening on `port`, with limits, a non-root user and a read-only root file
/// system left out on purpose (many images write to disk). It passes the linter.
pub fn starter_manifest(name: &str, image: &str, port: u16) -> String {
    format!(
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {name}\n  labels:\n    app: {name}\nspec:\n  replicas: 1\n  selector:\n    matchLabels:\n      app: {name}\n  template:\n    metadata:\n      labels:\n        app: {name}\n    spec:\n      containers:\n      - name: {name}\n        image: {image}\n        imagePullPolicy: IfNotPresent\n        ports:\n        - containerPort: {port}\n        resources:\n          requests:\n            cpu: 50m\n            memory: 64Mi\n          limits:\n            cpu: 500m\n            memory: 256Mi\n        securityContext:\n          runAsNonRoot: true\n          allowPrivilegeEscalation: false\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: {name}\nspec:\n  selector:\n    app: {name}\n  ports:\n  - port: {port}\n    targetPort: {port}\n"
    )
}

/// The system prompt line for a workspace chat with a connected cluster.
pub fn prompt_line(p: &ClusterProfile, ns: &str) -> String {
    format!("\nA Kubernetes cluster is already connected: {} (namespace {ns}). Use the k8s_* and deploy_* tools (k8s_status first). If deploy_load says there is no registry, offer registry_setup. Never start, install or stop k3s or any cluster, and never try: it is refused. To deploy: keep the Dockerfile, the app files and package.json in ONE folder and pass that folder as dir to deploy_build (a COPY that matches no file is refused before building), then deploy_load, then k8s_apply a manifest and deploy_status; the person approves each write. The namespace {ns} is created for you on the first apply: never write or apply a Namespace manifest; omit metadata.namespace from manifests or use that name. Before pushing, the person can run the unit tests in a container: deploy_build with target=test (a `test` stage in a multi-stage Dockerfile; never deploy an image whose tests failed); after the rollout, run the integration-test image with deploy_run_job. To clean up when the work is done, call k8s_stop_env (stop, keeps data) or k8s_teardown (removes the namespace); never run kubectl delete namespace by hand and never try k8s_delete kind=all. Bugs and failures get their own board row with a B- id (never an F- row): call bug_report with the failing pods so their logs are captured, fix, verify, and attach the passing run.", p.describe())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devcluster::Source;
    use crate::wsexec::FakeRunner;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn profile(k3s: bool) -> ClusterProfile {
        ClusterProfile { name: if k3s { "k3s".into() } else { "gke-dev".into() }, kubeconfig: "/home/u/.kube/k3s.yaml".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::K3s, exists: true, readable: true, tested: true, reachable: true, k3s, local: true, version: "v1.36.5+k3s1".into(), can_i: "get pods yes".into(), problem: String::new(), fix: String::new() }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-k8stools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("app")).unwrap();
        std::fs::create_dir_all(d.join("deploy")).unwrap();
        std::fs::write(d.join("app/Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(d.join("app/main.txt"), "hello").unwrap();
        d
    }

    struct Rig {
        t: K8sTools,
        r: Arc<FakeRunner>,
        root: PathBuf,
        audited: Arc<Mutex<Vec<String>>>,
    }

    fn rig(tag: &str, k3s: bool) -> Rig {
        let root = tmp(tag);
        let r = Arc::new(FakeRunner::new(&["kubectl", "podman", "docker", "k3s", "curl"]));
        let audited = Arc::new(Mutex::new(vec![]));
        let a2 = audited.clone();
        let mut t = K8sTools::new(r.clone(), profile(k3s), "shop-dev", "Shop", &root, Arc::new(move |a, d| a2.lock().unwrap().push(format!("k8s.{a} {d}"))));
        t.registries_yaml = root.join("registries.yaml");
        Rig { t, r, root, audited }
    }

    const MANIFEST: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  template:\n    spec:\n      containers:\n      - name: web\n        image: localhost/web:abc\n        resources:\n          limits:\n            cpu: 100m\n        securityContext:\n          runAsNonRoot: true\n";

    #[test]
    fn the_starter_manifest_passes_the_linter_with_no_warnings() {
        let m = starter_manifest("web", "localhost/web:abc", 8080);
        let l = deploylint::lint(&m, "shop-dev", &|i| i.starts_with("localhost/"));
        assert!(l.ok(), "{}", l.report());
        assert_eq!(l.objects, vec![("Deployment".into(), "web".into()), ("Service".into(), "web".into())]);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert!(l.notes.is_empty(), "it already has the pull policy");
    }

    #[test]
    fn the_load_path_is_chosen_by_what_the_machine_offers() {
        assert_eq!(choose_path(true, true, Mirror::Yes), LoadPath::Registry);
        assert_eq!(choose_path(true, true, Mirror::No), LoadPath::Import, "a registry nobody pulls from is no use");
        assert_eq!(choose_path(true, true, Mirror::Unknown), LoadPath::Import);
        assert_eq!(choose_path(true, false, Mirror::Yes), LoadPath::Import);
        assert_eq!(choose_path(true, false, Mirror::No), LoadPath::Import);
        assert!(matches!(choose_path(false, true, Mirror::Yes), LoadPath::Unsupported(_)));
    }

    #[test]
    fn the_tools_are_registered_with_the_right_risks() {
        let g = rig("defs", true);
        let defs = g.t.defs();
        assert_eq!(defs.iter().map(|d| d.spec.name.as_str()).collect::<Vec<_>>(), TOOLS.to_vec());
        for d in &defs {
            let writes = matches!(d.spec.name.as_str(), "k8s_apply" | "k8s_delete" | "k8s_rollout_restart" | "deploy_build" | "deploy_load" | "deploy_run_job" | "registry_setup" | "k8s_stop_env" | "k8s_teardown" | "k8s_registry_cleanup");
            assert_eq!(d.risk, if writes { Risk::Ask } else { Risk::Safe }, "{}", d.spec.name);
        }
        assert!(defs[0].spec.description.contains("never start, install or stop k3s"));
    }

    #[test]
    fn status_and_reads_use_the_profile_the_namespace_and_never_secret_values() {
        let g = rig("reads", true);
        g.r.answer("version -o json", r#"{"serverVersion":{"gitVersion":"v1.36.5+k3s1"}}"#);
        g.r.answer("get namespace shop-dev -o name", "namespace/shop-dev\n");
        let s = g.t.call("k8s_status", &json!({})).unwrap();
        assert!(s.contains("workspace \"Shop\"") && s.contains("reachable: yes") && s.contains("namespace: shop-dev (exists)") && s.contains("do not start, install or stop k3s"), "{s}");
        g.r.answer("get pods -n shop-dev -o wide", "NAME READY STATUS\nweb-1 1/1 Running\n");
        let o = g.t.call("k8s_get", &json!({"kind": "pods"})).unwrap();
        assert!(o.contains("web-1"));
        let line = g.r.lines().into_iter().find(|l| l.contains("get pods")).unwrap();
        assert!(line.starts_with("kubectl --kubeconfig /home/u/.kube/k3s.yaml --context default"), "{line}");
        assert!(g.r.calls().iter().all(|c| c.env.iter().any(|(k, v)| k == "KUBECONFIG" && v == "/home/u/.kube/k3s.yaml")), "the kubeconfig travels in the environment");
        // secrets: the table form only, and describe is refused
        g.r.answer("get secrets -n shop-dev -o wide", "NAME TYPE DATA\ndb Opaque 2\n");
        let o = g.t.call("k8s_get", &json!({"kind": "secrets"})).unwrap();
        assert!(o.contains("db") && !g.r.lines().iter().any(|l| l.contains("secrets") && (l.contains("-o json") || l.contains("jsonpath") || l.contains("-o yaml"))));
        assert!(g.t.call("k8s_describe", &json!({"kind": "secret", "name": "db"})).unwrap_err().contains("never reads secret values"));
        // bad input never reaches kubectl
        let before = g.r.calls().len();
        for bad in [json!({"kind": "pods; rm -rf /"}), json!({"kind": "pods", "name": "a b"}), json!({"kind": "pods", "namespace": "Bad_NS"}), json!({"kind": "pods", "selector": "a=b;c"})] {
            assert!(g.t.call("k8s_get", &bad).is_err(), "{bad}");
        }
        assert_eq!(g.r.calls().len(), before);
        // logs: tail is clamped
        g.r.answer("logs web-1", "line1\nline2\n");
        g.t.call("k8s_logs", &json!({"pod": "web-1", "tail": 100000})).unwrap();
        assert!(g.r.lines().iter().any(|l| l.contains("logs web-1 -n shop-dev --tail 500")));
        // the audit log has one entry per call
        assert!(g.audited.lock().unwrap().iter().all(|a| a.starts_with("k8s.k8s_") || a.starts_with("k8s.deploy_")), "{:?}", g.audited.lock().unwrap());
        assert!(g.audited.lock().unwrap().iter().any(|a| a.contains("Shop:shop-dev")));
    }

    #[test]
    fn every_audit_line_names_the_cluster_the_tools_work_against() {
        let mut g = rig("audit-cluster", true);
        g.r.answer("get pods", "NAME READY\nweb-1 1/1\n");
        g.t.call("k8s_get", &json!({"kind": "pods"})).unwrap();
        assert!(g.audited.lock().unwrap().last().unwrap().ends_with(" on cluster \"k3s\""), "the resolved profile's name by default: {:?}", g.audited.lock().unwrap());
        g.t.cluster_phrase = "cluster \"gpu-box\" (remote)".into();
        g.t.call("k8s_get", &json!({"kind": "pods"})).unwrap();
        let _ = g.t.call("k8s_get", &json!({"kind": "pods; x"}));
        let a = g.audited.lock().unwrap().clone();
        assert!(a.len() == 3 && a[1].ends_with(" on cluster \"gpu-box\" (remote)") && a[2].contains("failed") && a[2].ends_with(" on cluster \"gpu-box\" (remote)"), "{a:?}");
    }

    #[test]
    fn apply_checks_the_manifest_shows_the_diff_creates_the_namespace_and_stays_in_it() {
        let g = rig("apply", true);
        std::fs::write(g.root.join("deploy/app.yaml"), MANIFEST).unwrap();
        g.r.fail("diff -f", 1, "");
        g.r.answer("diff -f", "");
        let args = json!({"manifest_path": "deploy/app.yaml"});
        // the approval: namespace, context, check, and the diff
        let d = g.t.describe("k8s_apply", &args);
        assert!(d.contains("deploy/app.yaml") && d.contains("namespace shop-dev") && d.contains("k3s · default · reachable"), "{d}");
        let rr = Arc::new(FakeRunner::new(&["kubectl"]));
        let _ = rr;
        let pre = g.t.preview("k8s_apply", &args).unwrap();
        assert!(pre.contains("namespace shop-dev") && pre.contains("manifest check") && pre.contains("imagePullPolicy: IfNotPresent"), "{pre}");
        // apply: the namespace is created first, then the checked file is applied with -n
        g.r.fail("get namespace shop-dev", 1, "Error from server (NotFound): namespaces \"shop-dev\" not found");
        g.r.answer("create namespace shop-dev", "namespace/shop-dev created\n");
        g.r.answer("apply -f", "deployment.apps/web created\n");
        let out = g.t.call("k8s_apply", &args).unwrap();
        assert!(out.contains("created namespace shop-dev") && out.contains("deployment.apps/web created") && out.contains("IfNotPresent"), "{out}");
        let lines = g.r.lines();
        let ci = lines.iter().position(|l| l.contains("create namespace shop-dev")).unwrap();
        let ai = lines.iter().position(|l| l.contains(" apply -f ")).unwrap();
        assert!(ci < ai, "{lines:?}");
        assert!(lines[ai].contains("-n shop-dev") && lines[ai].contains(".iyakki/deploy/app.checked.yaml"), "{}", lines[ai]);
        let applied = std::fs::read_to_string(g.root.join(".iyakki/deploy/app.checked.yaml")).unwrap();
        assert!(applied.contains("imagePullPolicy: IfNotPresent"));
        assert_eq!(std::fs::read_to_string(g.root.join("deploy/app.yaml")).unwrap(), MANIFEST, "the person's file is not rewritten");
        // namespace confinement: another namespace in the manifest, or a cluster-scoped kind, is refused before kubectl is called
        let n = g.r.calls().len();
        std::fs::write(g.root.join("deploy/other.yaml"), MANIFEST.replace("  name: web\n", "  name: web\n  namespace: kube-system\n")).unwrap();
        let e = g.t.call("k8s_apply", &json!({"manifest_path": "deploy/other.yaml"})).unwrap_err();
        assert!(e.contains("kube-system") && e.contains("shop-dev"), "{e}");
        let e = g.t.call("k8s_apply", &json!({"yaml": "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: other\n"})).unwrap_err();
        assert!(e.contains("cluster-scoped"), "{e}");
        let e = g.t.call("k8s_apply", &json!({"manifest_path": "../outside.yaml"})).unwrap_err();
        assert!(e.contains("outside") || e.contains("No such"), "{e}");
        assert_eq!(g.r.calls().len(), n, "nothing reached kubectl");
        // delete and restart carry the workspace namespace and refuse the dangerous shapes
        g.r.answer("delete deployments web", "deployment.apps \"web\" deleted\n");
        g.t.call("k8s_delete", &json!({"kind": "deployment", "name": "web"})).unwrap();
        assert!(g.r.lines().iter().any(|l| l.ends_with("delete deployments web -n shop-dev")), "{:?}", g.r.lines());
        for bad in [json!({"kind": "namespace", "name": "shop-dev"}), json!({"kind": "nodes", "name": "n1"}), json!({"kind": "deployments", "name": "--all"}), json!({"kind": "deployments", "name": "a;b"})] {
            assert!(g.t.call("k8s_delete", &bad).is_err(), "{bad}");
        }
        g.r.answer("rollout restart deployment/web", "deployment.apps/web restarted\n");
        g.t.call("k8s_rollout_restart", &json!({"name": "web"})).unwrap();
        assert!(g.r.lines().iter().any(|l| l.ends_with("rollout restart deployment/web -n shop-dev")));
    }

    #[test]
    fn a_production_looking_cluster_is_read_only() {
        let mut g = rig("prod", true);
        g.t.profile.name = "prod-eu".into();
        g.t.profile.context = "prod-eu".into();
        for (tool, args) in [("k8s_apply", json!({"yaml": MANIFEST})), ("k8s_delete", json!({"kind": "pods", "name": "a"})), ("k8s_rollout_restart", json!({"name": "a"})), ("deploy_load", json!({"image": "web:1"}))] {
            let e = g.t.call(tool, &args).unwrap_err();
            assert!(e.contains("looks like production"), "{tool}: {e}");
            assert!(g.t.describe(tool, &args).contains("REFUSED"), "{tool}");
        }
        assert!(!g.r.lines().iter().any(|l| l.contains(" apply ") || l.contains(" delete ") || l.contains(" restart ") || l.contains("push") || l.contains("import") || l.contains(" save ")), "{:?}", g.r.lines());
    }

    #[test]
    fn build_uses_podman_first_then_docker_validates_names_and_tags_by_content() {
        let g = rig("build", true);
        let args = json!({"dir": "app", "image": "web"});
        g.r.answer("podman build", "STEP 1/1: FROM scratch\nSuccessfully tagged localhost/web:x\n");
        let d = g.t.describe("deploy_build", &args);
        assert!(d.contains("with podman") && d.contains("podman build -t localhost/web:"), "{d}");
        let out = g.t.call("deploy_build", &args).unwrap();
        assert!(out.contains("Built localhost/web:") && out.contains("deploy_load"), "{out}");
        let c = g.r.calls().into_iter().find(|c| c.argv[1] == "build").unwrap();
        assert_eq!(c.argv[0], "podman");
        assert!(c.cwd.ends_with("/app") && c.argv.last().map(String::as_str) == Some("."), "{c:?}");
        let tag1 = c.argv[3].rsplit(':').next().unwrap().to_string();
        assert_eq!(tag1.len(), 12);
        // the tag follows the content
        std::fs::write(g.root.join("app/main.txt"), "changed").unwrap();
        g.t.call("deploy_build", &args).unwrap();
        let tag2 = g.r.calls().into_iter().filter(|c| c.argv[1] == "build").last().unwrap().argv[3].rsplit(':').next().unwrap().to_string();
        assert_ne!(tag1, tag2);
        // an explicit tag; bad names; outside the workspace; no Dockerfile
        g.t.call("deploy_build", &json!({"dir": "app", "image": "web", "tag": "dev"})).unwrap();
        for bad in [json!({"dir": "app", "image": "Web"}), json!({"dir": "app", "image": "docker.io/x y"}), json!({"dir": "app", "image": "web", "tag": "a b"}), json!({"dir": "../etc", "image": "web"}), json!({"dir": "deploy", "image": "web"})] {
            assert!(g.t.call("deploy_build", &bad).is_err(), "{bad}");
        }
        // a failed build shows the end of the log
        let g2 = rig("buildfail", true);
        g2.r.fail("podman build", 125, "error: step 3 failed: no such file");
        let e = g2.t.call("deploy_build", &args).unwrap_err();
        assert!(e.contains("the build failed") && e.contains("step 3 failed"), "{e}");
        // docker is the fallback
        let root = tmp("dockeronly");
        let r = Arc::new(FakeRunner::new(&["docker", "kubectl"]));
        r.answer("docker build", "ok");
        let t = K8sTools::new(r.clone(), profile(true), "shop-dev", "Shop", &root, Arc::new(|_, _| {}));
        t.call("deploy_build", &args).unwrap();
        assert_eq!(r.calls()[0].argv[0], "docker");
        // neither
        let r = Arc::new(FakeRunner::new(&["kubectl"]));
        let t = K8sTools::new(r, profile(true), "shop-dev", "Shop", &root, Arc::new(|_, _| {}));
        assert!(t.call("deploy_build", &args).unwrap_err().contains("neither podman nor docker"));
    }

    #[test]
    fn load_imports_into_k3s_without_sudo_and_explains_the_two_fixes_when_root_is_needed() {
        let g = rig("import", true);
        g.r.answer("podman image exists localhost/web:abc", "");
        g.r.answer("podman save", "");
        g.r.answer("k3s ctr images import", "unpacking localhost/web:abc\n");
        let out = g.t.call("deploy_load", &json!({"image": "web:abc"})).unwrap();
        assert!(out.contains("Imported localhost/web:abc") && out.contains("IfNotPresent"), "{out}");
        let lines = g.r.lines();
        assert!(lines.iter().any(|l| l.starts_with("podman save -o ") && l.ends_with("localhost/web:abc")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("k3s ctr images import ")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("sudo")));
        assert!(std::fs::read_dir(g.root.join(".iyakki/tmp")).unwrap().next().is_none(), "the temporary archive is removed");
        // the loaded image makes a later manifest use IfNotPresent even without the localhost prefix
        let m = MANIFEST.replace("localhost/web:abc", "web:abc");
        g.r.answer("diff -f", "");
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("apply -f", "ok");
        let o = g.t.call("k8s_apply", &json!({"yaml": m})).unwrap();
        assert!(o.contains("note:") && o.contains("IfNotPresent"), "{o}");
        // root needed: the fixes, no sudo attempted
        let g = rig("importdenied", true);
        g.r.answer("podman image exists", "");
        g.r.answer("podman save", "");
        g.r.fail("k3s ctr images import", 1, "ctr: failed to dial \"/run/k3s/containerd/containerd.sock\": permission denied");
        let e = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("needs root") && e.contains("never uses sudo") && e.contains("registries.yaml") && e.contains("sudoers") && e.contains("localhost:5000"), "{e}");
        assert!(!g.r.lines().iter().any(|l| l.starts_with("sudo")));
        // an image that was never built
        let g = rig("noimage", true);
        g.r.fail("podman image exists", 1, "");
        assert!(g.t.call("deploy_load", &json!({"image": "web:zzz"})).unwrap_err().contains("build it first"));
    }

    #[test]
    fn load_prefers_the_registry_when_k3s_uses_it_and_a_remote_cluster_is_not_imported_into() {
        let g = rig("registry", true);
        std::fs::write(&g.t.registries_yaml, "mirrors:\n  \"localhost:5000\":\n    endpoint:\n      - \"http://localhost:5000\"\n").unwrap();
        g.r.answer("podman image exists", "");
        g.r.answer("curl -fsS", "{}");
        g.r.answer("podman tag", "");
        g.r.answer("podman push", "Writing manifest to image destination");
        let d = g.t.describe("deploy_load", &json!({"image": "localhost/web:abc"}));
        assert!(d.contains("push it to the registry"), "{d}");
        let out = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(out.contains("localhost:5000/web:abc"), "{out}");
        let lines = g.r.lines();
        assert!(lines.contains(&"podman tag localhost/web:abc localhost:5000/web:abc".to_string()), "{lines:?}");
        assert!(lines.contains(&"podman push --tls-verify=false localhost:5000/web:abc".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("ctr images import")), "no import when the registry works");
        // the manifest then uses the registry image and keeps IfNotPresent
        g.r.answer("diff -f", "");
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("apply -f", "ok");
        let o = g.t.call("k8s_apply", &json!({"yaml": MANIFEST.replace("localhost/web:abc", "localhost:5000/web:abc")})).unwrap();
        assert!(o.contains("IfNotPresent"), "{o}");
        // registry up but k3s does not mirror it: import
        let g = rig("nomirror", true);
        g.r.answer("podman image exists", "");
        g.r.answer("curl -fsS", "{}");
        g.r.answer("podman save", "");
        g.r.answer("k3s ctr images import", "ok");
        let out = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(out.contains("Imported"), "{out}");
        // a cluster elsewhere
        let mut g = rig("remote", true);
        g.t.profile.local = false;
        g.t.profile.server = "https://203.0.113.9:6443".into();
        g.r.answer("podman image exists", "");
        let e = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("not on this machine") && e.contains("registry"), "{e}");
    }

    const SVCS: &str = r#"{"items":[{"metadata":{"name":"registry","namespace":"meisync-registry"},"spec":{"type":"ClusterIP","ports":[{"port":5000}]}},{"metadata":{"name":"registry-nodeport","namespace":"meisync-registry"},"spec":{"type":"NodePort","ports":[{"port":5000,"nodePort":30500}]}},{"metadata":{"name":"web"},"spec":{"type":"NodePort","ports":[{"nodePort":31000}]}}]}"#;
    const NODES: &str = r#"{"items":[{"metadata":{"name":"node-main"},"status":{"conditions":[{"type":"Ready","status":"True"}]}},{"metadata":{"name":"macmini-k3s"},"status":{"conditions":[{"type":"Ready","status":"Unknown"}]}}]}"#;

    fn remote(tag: &str) -> Rig {
        let mut g = rig(tag, true);
        g.t.profile.local = false;
        g.t.profile.server = "https://192.168.1.68:6443".into();
        g
    }

    #[test]
    fn a_remote_cluster_gets_the_image_pushed_to_its_discovered_registry() {
        let g = remote("remotepush");
        g.r.answer("podman image exists", "");
        g.r.answer("get services -A", SVCS);
        g.r.answer("get nodes", NODES);
        g.r.answer("curl -fsS", "{}");
        g.r.answer("podman tag", "");
        g.r.answer("podman push", "done");
        let d = g.t.describe("deploy_load", &json!({"image": "localhost/web:abc"}));
        assert!(d.contains("192.168.1.68:30500"), "{d}");
        let out = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        let want = "192.168.1.68:30500/shop-dev/web:abc";
        let pull = "localhost:30500/shop-dev/web:abc";
        assert!(out.contains(&format!("Use image: {pull} in the manifest")) && out.contains("kube-proxy") && out.contains("Ready"), "{out}");
        assert!(out.contains(want) && out.contains("NotReady: macmini-k3s") && !out.contains("node-main"), "{out}");
        let lines = g.r.lines();
        assert!(lines.contains(&format!("podman tag localhost/web:abc {want}")), "{lines:?}");
        assert!(lines.contains(&format!("podman push --tls-verify=false {want}")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("http://192.168.1.68:30500/v2/")));
        // the linter accepts the pushed reference and keeps IfNotPresent
        g.r.answer("diff -f", "");
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("apply -f", "ok");
        let o = g.t.call("k8s_apply", &json!({"yaml": MANIFEST.replace("localhost/web:abc", pull)})).unwrap();
        assert!(o.contains("IfNotPresent") && !o.contains("not one the checker knows"), "{o}");
    }

    #[test]
    fn remote_push_uses_the_setting_hints_at_docker_insecure_registries_and_reports_failures() {
        let mut g = remote("remotesetting");
        g.t.registry_setting = "10.0.0.5:5000".into();
        g.r.answer("docker image inspect", "");
        g.r.answer("curl -fsS", "{}");
        g.r.answer("docker tag", "");
        g.r.fail("docker push", 1, "Get \"https://10.0.0.5:5000/v2/\": http: server gave HTTP response to HTTPS client");
        // docker only: podman is not installed here
        let r2 = Arc::new(FakeRunner::new(&["kubectl", "docker", "curl"]));
        for (k, v) in [("docker image inspect", ""), ("curl -fsS", "{}"), ("docker tag", "")] {
            r2.answer(k, v);
        }
        r2.fail("docker push", 1, "Get \"https://10.0.0.5:5000/v2/\": http: server gave HTTP response to HTTPS client");
        let mut t = K8sTools::new(r2.clone(), g.t.profile.clone(), "shop-dev", "Shop", &g.root, Arc::new(|_, _| {}));
        t.registry_setting = "10.0.0.5:5000".into();
        assert_eq!(t.nodeport_pull_ref("10.0.0.5:5000", "localhost/web:abc"), None, "an explicit registry keeps its own address");
        let e = t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("insecure-registries") && e.contains("10.0.0.5:5000") && e.contains("/etc/docker/daemon.json"), "{e}");
        assert!(!r2.lines().iter().any(|l| l.contains("get services")), "the setting wins over discovery");
        // an unreachable registry and a cluster without one
        let g = remote("remotedown");
        g.r.answer("podman image exists", "");
        g.r.answer("get services -A", SVCS);
        g.r.fail("curl -fsS", 7, "connection refused");
        let e = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("did not answer GET /v2/"), "{e}");
        assert_eq!(find_registry_port("{\"items\":[{\"metadata\":{\"name\":\"web\"},\"spec\":{\"type\":\"NodePort\",\"ports\":[{\"nodePort\":1}]}}]}"), None);
        assert_eq!(server_host("https://[::1]:6443"), "[::1]");
        assert_eq!(server_host("https://k3s.lan:6443/x"), "k3s.lan");
    }

    #[test]
    #[ignore = "reads the live registry of the configured cluster; run with --ignored"]
    fn live_remote_registry_answers_catalog() {
        let Some(t) = real_tools("iyakki-live", &std::env::temp_dir()) else { return };
        if t.profile.local {
            return;
        }
        let reg = t.remote_registry().expect("registry");
        let o = t.run.run(&std::env::temp_dir(), &["curl".into(), "-fsS".into(), "--max-time".into(), "5".into(), format!("http://{reg}/v2/_catalog")], &[], Duration::from_secs(8), 65536, None);
        assert!(o.ok(), "{}", o.combined());
    }

    #[test]
    fn deploy_status_reads_the_namespace_and_explains_pods_that_cannot_start() {
        let g = rig("status", true);
        g.r.fail("rollout status deployment/web", 1, "error: timed out waiting for the condition");
        let now = 0;
        let _ = now;
        g.r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        g.r.answer("config current-context", "default\n");
        g.r.answer("get deployments,statefulsets", r#"{"items":[
          {"kind":"Deployment","metadata":{"name":"web","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"replicas":1,"template":{"spec":{"containers":[{"image":"localhost/web:abc"}]}}},"status":{"readyReplicas":0,"replicas":1}},
          {"kind":"Pod","metadata":{"name":"web-1","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"nodeName":"n1","containers":[{"name":"web","image":"localhost/web:abc"}]},"status":{"phase":"Pending","containerStatuses":[{"name":"web","ready":false,"restartCount":0,"state":{"waiting":{"reason":"ImagePullBackOff"}}}]}}]}"#);
        g.r.answer("get configmaps", "");
        g.r.answer("get secrets", "");
        g.r.answer("get nodes", "{}");
        let s = g.t.call("deploy_status", &json!({"name": "web"})).unwrap();
        assert!(s.contains("rollout of web: error: timed out"), "{s}");
        assert!(s.contains("Pod web-1") && s.contains("ImagePullBackOff") && s.contains("IfNotPresent"), "the reason is explained, including the k3s import hint: {s}");
        assert!(s.contains("Deployment web: 0/1"), "{s}");
    }

    #[test]
    fn deploy_status_says_how_a_service_is_really_reached_and_never_calls_a_cluster_ip_external() {
        let g = rig("reach", true);
        g.r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        g.r.answer("config current-context", "default\n");
        g.r.answer("get deployments,statefulsets", r#"{"items":[
          {"kind":"Service","metadata":{"name":"web","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"type":"ClusterIP","clusterIP":"10.43.219.73","ports":[{"port":80,"protocol":"TCP","targetPort":3000}]}},
          {"kind":"Service","metadata":{"name":"api","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"type":"NodePort","clusterIP":"10.43.1.2","ports":[{"port":80,"protocol":"TCP","targetPort":3000,"nodePort":30080}]}},
          {"kind":"Service","metadata":{"name":"lb","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"type":"LoadBalancer","clusterIP":"10.43.1.3","ports":[{"port":8080,"protocol":"TCP","targetPort":3000}]},"status":{"loadBalancer":{"ingress":[{"ip":"192.168.1.50"}]}}}]}"#);
        g.r.answer("get configmaps", "");
        g.r.answer("get secrets", "");
        g.r.answer("get nodes", r#"{"items":[{"metadata":{"name":"n1","labels":{}},"status":{"conditions":[{"type":"Ready","status":"True"}],"addresses":[{"type":"InternalIP","address":"192.168.1.68"}],"nodeInfo":{"kubeletVersion":"v1"},"allocatable":{"cpu":"4","memory":"8Gi"}}}]}"#);
        let s = g.t.call("deploy_status", &json!({})).unwrap();
        assert!(s.contains("web: not reachable from outside: 10.43.219.73 is a ClusterIP address") && s.contains("NodePort"), "{s}");
        assert!(s.contains("api: reachable at: http://192.168.1.68:30080 (node address + NodePort)"), "{s}");
        assert!(s.contains("lb: reachable at: http://192.168.1.50:8080 (external address of the LoadBalancer)"), "{s}");
        assert!(s.contains("10.43.219.73 (cluster-internal address)"), "{s}");
        assert!(!s.contains("http://10.43"), "{s}");
        assert!(crate::toolblock::COMPACT_PROMPT.contains("cluster-internal") && crate::agent::AGENT_SYSTEM_PROMPT.contains("cluster-internal"));
    }

    #[test]
    fn logs_of_a_replaced_pod_come_from_the_current_pod_and_an_empty_log_says_what_to_check_instead() {
        let g = rig("logs2", true);
        g.r.fail("logs web-aaa-old", 1, "Error from server (NotFound): pods \"web-aaa-old\" not found");
        g.r.answer("--sort-by=.metadata.creationTimestamp", "pod/web-aaa-old\npod/db-1\npod/web-bbb-new\n");
        g.r.answer("logs web-bbb-new", "listening on 3000\n");
        g.r.answer("logs web-empty", "");
        let o = g.t.call("k8s_logs", &json!({"pod": "web-aaa-old"})).unwrap();
        assert!(o.contains("pod web-aaa-old no longer exists") && o.contains("current pod web-bbb-new") && o.contains("listening on 3000"), "{o}");
        let e = g.t.call("k8s_logs", &json!({"pod": "web-empty"})).unwrap();
        assert!(e.contains("wrote nothing to stdout") && e.contains("deploy_status") && e.contains("http_request") && e.contains("Do not call k8s_logs again"), "{e}");
        // a pod that never existed and has no sibling is still an error
        g.r.fail("logs ghost-1-x", 1, "Error from server (NotFound): pods \"ghost-1-x\" not found");
        assert!(g.t.call("k8s_logs", &json!({"pod": "ghost-1-x"})).unwrap_err().contains("NotFound"));
        // long logs keep the newest lines
        let long = (0..2000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        g.r.answer("logs web-long", &long);
        let o = g.t.call("k8s_logs", &json!({"pod": "web-long"})).unwrap();
        assert!(o.starts_with("[earlier lines cut]") && o.contains("line 1999") && o.chars().count() < 6100, "{}", o.chars().count());
    }

    const CRASH_PODS: &str = r#"{"items":[
      {"metadata":{"name":"web-7d9f8-abcde"},"spec":{"initContainers":[{"name":"init","image":"busybox"}],"containers":[{"name":"app","image":"localhost/web:abc","env":[{"name":"DB_PASSWORD","value":"s3cretvalue123"}]},{"name":"sidecar","image":"envoy:1"}]},
       "status":{"containerStatuses":[{"name":"app","imageID":"localhost/web@sha256:1111","state":{"waiting":{"reason":"CrashLoopBackOff"}},"lastState":{"terminated":{"reason":"Error"}}},{"name":"sidecar","imageID":"envoy@sha256:2222","state":{"running":{}}}]}},
      {"metadata":{"name":"db-0"},"spec":{"containers":[{"name":"db","image":"postgres:16"}]},"status":{"containerStatuses":[{"name":"db","state":{"running":{}}}]}}]}"#;

    fn crash_rig(tag: &str) -> Rig {
        crash_rig_with(tag, "2026-10-14T09:01:00Z envoy ready")
    }

    fn crash_rig_with(tag: &str, sidecar_log: &str) -> Rig {
        let g = rig(tag, true);
        // the previous run of the app, with a secret in it (rules are matched in order: the specific ones first)
        g.r.answer("logs web-7d9f8-abcde -n shop-dev -c app --tail 300 --timestamps --previous", "2026-10-14T09:00:01Z starting\n2026-10-14T09:00:02Z DB_PASSWORD=hunter2hunter2X9 connecting\n2026-10-14T09:00:03Z panic: cannot open /data");
        g.r.fail("--previous", 1, "previous terminated container \"sidecar\" not found");
        g.r.answer("logs web-7d9f8-abcde -n shop-dev -c app --tail 300 --timestamps", "2026-10-14T09:01:00Z starting again\n2026-10-14T09:01:01Z token ghp_abcdefghijklmnopqrstuvwxyz0123456789 loaded");
        g.r.answer("logs web-7d9f8-abcde -n shop-dev -c sidecar", sidecar_log);
        g.r.answer("logs web-7d9f8-abcde -n shop-dev -c init", "");
        g.r.answer("describe pod web-7d9f8-abcde", "Name: web-7d9f8-abcde\nEvents:\n  Back-off restarting failed container");
        g.r.answer("get events -n shop-dev", "LAST SEEN TYPE REASON OBJECT MESSAGE\n1m Warning BackOff pod/web-7d9f8-abcde Back-off restarting failed container");
        g.r.answer("get deployments,services -n shop-dev -o yaml", "apiVersion: v1\nkind: List\nitems: []\n");
        g.r.answer("get pods -n shop-dev -o json", CRASH_PODS);
        std::fs::write(g.root.join("Dockerfile"), "FROM node:18\nCMD [\"node\", \"server.js\"]\n").unwrap();
        g
    }

    #[test]
    fn evidence_has_every_containers_logs_current_and_previous_describe_events_manifest_and_dockerfile_with_secrets_redacted() {
        let g = crash_rig("evidence");
        let dir = g.root.join("ev");
        let ev = g.t.capture_evidence(&dir, &["web".to_string()], 300, "").unwrap();
        assert_eq!(ev.pods, vec!["web-7d9f8-abcde".to_string()], "only the named pod (by name or prefix)");
        for f in ["web-7d9f8-abcde__app.log", "web-7d9f8-abcde__app.previous.log", "web-7d9f8-abcde__sidecar.log", "web-7d9f8-abcde__init.log", "web-7d9f8-abcde.describe.txt", "web-7d9f8-abcde.status.json", "events.txt", "manifest-in-effect.yaml", "Dockerfile.txt", "images.txt"] {
            assert!(dir.join(f).is_file(), "{f} missing; have {:?}", ev.files);
        }
        assert!(!dir.join("web-7d9f8-abcde__sidecar.previous.log").exists(), "a container with no previous run has no previous log");
        let all: String = ev.files.iter().map(|f| std::fs::read_to_string(dir.join(f)).unwrap()).collect::<Vec<_>>().join("\n");
        assert!(!all.contains("hunter2") && !all.contains("ghp_") && !all.contains("s3cretvalue123"), "secrets, tokens and env values are redacted: {}", all.lines().filter(|l| l.contains("hunter2") || l.contains("ghp_") || l.contains("s3cret")).collect::<Vec<_>>().join(" | "));
        assert!(all.contains("panic: cannot open /data") && all.contains("DB_PASSWORD") && all.contains("(the container wrote nothing"), "the useful lines stay");
        assert_eq!(ev.reasons, vec!["CrashLoopBackOff".to_string(), "Error".to_string()]);
        assert!(ev.images.iter().any(|i| i.contains("sha256:1111")), "{:?}", ev.images);
        // no pod named: every pod of the namespace; a name that is not a pod name never reaches kubectl
        let all_pods = g.t.capture_evidence(&g.root.join("ev2"), &[], 300, "").unwrap();
        assert_eq!(all_pods.pods.len(), 2);
        assert!(g.r.lines().iter().all(|l| !l.contains(";") && !l.contains("$(")));
        // the number of lines is capped, and an over-long log keeps its head and tail
        g.r.answer("--tail 2000", "x");
        let _ = g.t.capture_evidence(&g.root.join("ev3"), &["web".to_string()], 99_999, "10m").unwrap();
        assert!(g.r.lines().iter().any(|l| l.contains("--tail 2000") && l.contains("--since 10m")), "{:?}", g.r.lines());
    }

    #[test]
    fn a_huge_log_is_cut_to_its_head_and_tail() {
        let big = format!("FIRSTLINE\n{}\nLASTLINE", "some log text ".repeat(100_000));
        let g = crash_rig_with("hugelog", &big);
        let dir = g.root.join("ev");
        g.t.capture_evidence(&dir, &["web".to_string()], 300, "").unwrap();
        let t = std::fs::read_to_string(dir.join("web-7d9f8-abcde__sidecar.log")).unwrap();
        assert!(t.starts_with("FIRSTLINE") && t.ends_with("LASTLINE") && t.contains("bytes cut from the middle") && t.len() < crate::bugs::MAX_FILE + 200, "{}", t.len());
    }

    #[test]
    fn bug_report_makes_a_b_row_with_the_logs_folder_and_the_same_failure_is_reported_once() {
        let g = crash_rig("bugtool");
        let root = g.root.clone();
        use crate::agent::Tools;
        struct Base;
        impl Tools for Base {
            fn defs(&self) -> Vec<ToolDef> {
                vec![]
            }
            fn call(&self, name: &str, _a: &Value) -> Result<String, String> {
                Err(format!("unknown tool `{name}`"))
            }
            fn describe(&self, name: &str, _a: &Value) -> String {
                name.into()
            }
        }
        let tools = crate::boardtools::ProjectTools::new(Arc::new(Base), &root, "Shop", &root.join("FEATURES.md")).with_k8s(Arc::new(g.t));
        assert!(Tools::defs(&tools).iter().any(|d| d.spec.name == "bug_report" && d.risk == Risk::Ask), "Ask in Ask mode; Auto approves a tool that only reads and writes in the workspace");
        let r = Tools::call(&tools, "bug_report", &json!({"title": "web crashes at start", "severity": "high", "repro": "kubectl rollout restart deployment/web", "pods": ["web"], "feature": "F-APP-02"})).unwrap();
        assert!(r.starts_with("B-APP-01 created") && r.contains(".iyakki/bugs/B-APP-01/"), "{r}");
        let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
        let f = b.get("B-APP-01").unwrap();
        assert!(f.is_bug() && f.status == "planned" && f.severity == "high" && f.repro.contains("rollout restart") && f.fix == "F-APP-02", "{f:?}");
        assert_eq!(f.logs, vec![".iyakki/bugs/B-APP-01/".to_string()]);
        assert!(!f.found.is_empty());
        let dir = root.join(".iyakki/bugs/B-APP-01");
        assert!(dir.join("web-7d9f8-abcde__app.previous.log").is_file() && dir.join("summary.md").is_file() && dir.join("Dockerfile.txt").is_file());
        let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
        assert!(summary.contains("CrashLoopBackOff") && summary.contains("web-7d9f8-abcde") && summary.contains("F-APP-02"), "{summary}");
        // the same failure (same reasons, pod pattern and image): no second bug, the sighting is added
        let again = Tools::call(&tools, "bug_report", &json!({"title": "web crashes again", "severity": "high", "pods": ["web"]})).unwrap();
        assert!(again.starts_with("already tracked as B-APP-01 (seen 2 times)"), "{again}");
        let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
        assert_eq!(b.features().len(), 1);
        assert!(b.get("B-APP-01").unwrap().notes.contains("seen 2 times"));
        // a bug is numbered on its own: the next one is B-APP-02, features keep F-
        Tools::call(&tools, "board_add", &json!({"prefix": "APP", "title": "Add login"})).unwrap();
        let other = Tools::call(&tools, "board_add", &json!({"prefix": "APP", "title": "Wrong total on the cart", "kind": "bug", "severity": "low", "repro": "add two items"})).unwrap();
        assert!(other.starts_with("added B-APP-02"), "{other}");
        let b = crate::featureboard::Board::parse(&std::fs::read_to_string(root.join("FEATURES.md")).unwrap());
        assert!(b.get("F-APP-01").is_some() && b.get("B-APP-02").is_some_and(|f| f.severity == "low" && !f.found.is_empty()));
        assert!(Tools::call(&tools, "board_add", &json!({"id": "F-APP-09", "title": "x", "kind": "bug"})).unwrap_err().contains("bug ids start with B-"));
        assert!(Tools::call(&tools, "bug_report", &json!({"severity": "low"})).unwrap_err().contains("title"));
    }

    const PULL_EVENT: &str = "LAST SEEN   TYPE      REASON   OBJECT      MESSAGE\n1m   Warning   Failed   pod/web-1   Failed to pull image \"192.168.1.68:30500/shop-dev/web:abc\": failed to do request: Head \"https://192.168.1.68:30500/v2/shop-dev/web/manifests/abc\": http: server gave HTTP response to HTTPS client\n";

    #[test]
    fn a_pull_failure_over_http_is_diagnosed_in_status_events_and_describe_with_the_exact_fix() {
        let g = rig("pullfail", true);
        g.r.answer("get events", PULL_EVENT);
        g.r.answer("describe pod web-1", &format!("Name: web-1\nEvents:\n  {PULL_EVENT}"));
        let ev = g.t.call("k8s_events", &json!({})).unwrap();
        assert!(ev.contains("Diagnosis:") && ev.contains("\"http://192.168.1.68:30500\"") && ev.contains("k3s-agent") && ev.contains("/etc/rancher/k3s/registries.yaml"), "{ev}");
        let d = g.t.call("k8s_describe", &json!({"kind": "pod", "name": "web-1"})).unwrap();
        assert!(d.contains("Diagnosis:") && d.contains("sudo systemctl restart k3s"), "{d}");
        assert!(!ev.to_lowercase().contains("docker hub"));
        // deploy_status
        g.r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        g.r.answer("config current-context", "default\n");
        g.r.answer("get deployments,statefulsets", r#"{"items":[{"kind":"Pod","metadata":{"name":"web-1","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"nodeName":"n1","containers":[{"name":"web","image":"x"}]},"status":{"phase":"Pending","containerStatuses":[{"name":"web","ready":false,"restartCount":0,"state":{"waiting":{"reason":"ErrImagePull"}}}]}}]}"#);
        g.r.answer("get configmaps", "");
        g.r.answer("get secrets", "");
        g.r.answer("get nodes", "{}");
        let s = g.t.call("deploy_status", &json!({})).unwrap();
        assert!(s.contains("ErrImagePull") && s.contains("Diagnosis:") && s.contains("192.168.1.68:30500"), "{s}");
    }

    fn in_cluster_answers(r: &FakeRunner) {
        r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#);
        r.answer("get services -A", r#"{"items":[]}"#);
        r.fail("get namespace iyakki-registry", 1, "Error from server (NotFound): namespaces \"iyakki-registry\" not found");
        r.answer("create namespace iyakki-registry", "namespace/iyakki-registry created");
        r.answer("apply -f", "deployment.apps/registry created");
        r.answer("rollout status", "successfully rolled out");
        r.answer("/v2/", "{}");
    }

    #[test]
    fn registry_setup_asks_shows_the_exact_plan_and_the_plan_tool_stays_read_only() {
        let g = rig("regsetup-ask", true);
        in_cluster_answers(&g.r);
        assert_eq!(g.t.defs().iter().find(|d| d.spec.name == "registry_setup").unwrap().risk, Risk::Ask);
        assert!(K8sTools::handles("registry_setup"));
        let args = json!({"option": "in-cluster"});
        let pv = g.t.preview("registry_setup", &args).unwrap();
        assert!(pv.contains("create namespace iyakki-registry") && pv.contains("nodePort: 30500") && pv.contains("Root: not needed"), "{pv}");
        assert!(g.t.describe("registry_setup", &args).contains("Root: not needed"));
        let plan = g.t.call("cluster_setup_plan", &args).unwrap();
        assert!(plan.contains("plan only") && plan.contains("namespace iyakki-registry"), "{plan}");
        assert!(!g.r.lines().iter().any(|l| l.contains("create namespace") || l.contains("apply -f")), "{:?}", g.r.lines());
        assert!(g.t.call("cluster_setup_plan", &json!({"option": "in-cluster", "node_port": 80})).is_err());
        // B only fits a cluster on this machine
        let r = remote("regsetup-b");
        assert!(r.t.call("registry_setup", &json!({"option": "system"})).unwrap_err().contains("only fits a cluster on this machine"));
        assert!(r.t.preview("registry_setup", &json!({"option": "system"})).unwrap().contains("refused"));
    }

    #[test]
    fn a_local_cluster_with_the_in_cluster_registry_pushes_and_pulls_localhost_nodeport() {
        let g = rig("regsetup-local", true);
        in_cluster_answers(&g.r);
        let out = g.t.call("registry_setup", &json!({"option": "in-cluster"})).unwrap();
        assert!(out.contains("created namespace iyakki-registry") && out.contains("deploy_load now pushes to localhost:30500"), "{out}");
        assert_eq!(g.t.saved_registry(), ("localhost:30500".into(), String::new(), "in-cluster".into()));
        let cfg = crate::workspace::ProjectConfig::load(&g.root).unwrap();
        assert_eq!((cfg.k8s.registry.as_str(), cfg.k8s.registry_kind.as_str()), ("localhost:30500", "in-cluster"));
        assert!(g.audited.lock().unwrap().iter().any(|a| a.starts_with("k8s.registry_setup ") && a.contains(":ok ")));
        g.r.answer("podman image exists", "");
        g.r.answer("get nodes", NODES);
        g.r.answer("podman tag", "");
        g.r.answer("podman push", "done");
        let d = g.t.describe("deploy_load", &json!({"image": "localhost/web:abc"}));
        assert!(d.contains("localhost:30500/shop-dev/web:abc"), "{d}");
        let o = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(o.contains("Use image: localhost:30500/shop-dev/web:abc"), "{o}");
        let lines = g.r.lines();
        assert!(lines.contains(&"podman push --tls-verify=false localhost:30500/shop-dev/web:abc".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("localhost:5000") || l.contains("images import")), "the saved choice replaces the :5000 path: {lines:?}");
        assert_eq!(g.t.cluster_ref("localhost/web:abc").as_deref(), Some("localhost:30500/shop-dev/web:abc"));
    }

    #[test]
    fn a_remote_cluster_with_the_in_cluster_registry_pushes_to_the_api_host_and_pods_pull_localhost() {
        let mut g = remote("regsetup-remote");
        in_cluster_answers(&g.r);
        g.r.answer("podman image exists", "");
        g.r.answer("get nodes", NODES);
        g.r.answer("podman tag", "");
        g.r.answer("podman push", "done");
        let out = g.t.call("registry_setup", &json!({"option": "in-cluster"})).unwrap();
        assert!(out.contains("pushes to 192.168.1.68:30500") && out.contains("localhost:30500/<namespace>"), "{out}");
        let cfg = crate::workspace::ProjectConfig::load(&g.root).unwrap();
        assert_eq!((cfg.k8s.registry.as_str(), cfg.k8s.registry_pull.as_str()), ("192.168.1.68:30500", "localhost:30500"));
        // a fresh tools object reads the saved values the way the IDE hands them over
        g.t.registry_setting = cfg.k8s.registry;
        g.t.registry_pull = cfg.k8s.registry_pull;
        g.t.registry_kind = cfg.k8s.registry_kind;
        let fresh = g.t;
        let o = fresh.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(o.contains("Use image: localhost:30500/shop-dev/web:abc"), "{o}");
        assert!(g.r.lines().contains(&"podman push --tls-verify=false 192.168.1.68:30500/shop-dev/web:abc".to_string()), "{:?}", g.r.lines());
    }

    #[test]
    fn the_saved_system_and_remote_choices_decide_push_and_pull_and_production_is_never_written() {
        // B on a local cluster keeps the :5000 path
        let mut g = rig("regsetup-sys", true);
        g.t.registry_setting = REGISTRY.into();
        g.t.registry_kind = "system".into();
        assert!(!g.t.configured_registry());
        // C on a local cluster: the saved address is used instead of :5000
        let mut c = rig("regsetup-c", true);
        c.t.registry_setting = "10.0.0.5:5000".into();
        c.t.registry_kind = "remote".into();
        c.r.answer("podman image exists", "");
        c.r.answer("curl -fsS", "{}");
        c.r.answer("podman tag", "");
        c.r.answer("podman push", "done");
        let o = c.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(o.contains("Use image: 10.0.0.5:5000/shop-dev/web:abc"), "{o}");
        // the system option applies with podman, writes nothing as non-root, and records :5000
        g.r.fail("podman inspect", 1, "no such container");
        g.r.answer("podman run -d", "id");
        let out = g.t.call("registry_setup", &json!({"option": "system"})).unwrap();
        assert!(out.contains("created the registry container") && out.contains("does not use sudo") || unsafe { libc::geteuid() } == 0, "{out}");
        assert_eq!(crate::workspace::ProjectConfig::load(&g.root).unwrap().k8s.registry_kind, "system");
        // production: the in-cluster option is refused, nothing is applied
        let mut p = rig("regsetup-prod", true);
        p.t.profile.name = "prod-eu".into();
        in_cluster_answers(&p.r);
        let e = p.t.call("registry_setup", &json!({"option": "in-cluster"})).unwrap_err();
        assert!(e.contains("production"), "{e}");
        assert!(!p.r.lines().iter().any(|l| l.contains("create namespace") || l.contains("apply -f")));
        // none clears the choice for the rest of the session
        c.t.call("registry_setup", &json!({"option": "none"})).unwrap();
        assert_eq!(c.t.saved_registry().2, "none");
        assert!(!c.t.configured_registry());
    }

    #[test]
    fn deploy_build_refuses_a_dockerfile_whose_files_are_one_folder_up_without_building_and_shows_the_real_failure() {
        let g = rig("buildlayout", true);
        let dir = g.root.join("nodejs-webserver");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM node:20-alpine\nWORKDIR /app\nCOPY package*.json .\nRUN npm install\nCOPY . .\nCMD [\"node\",\"app.js\"]\n").unwrap();
        std::fs::write(g.root.join("index.js"), "console.log(1)").unwrap();
        let args = json!({"dir": "nodejs-webserver", "image": "nodejs-webserver"});
        let e = g.t.call("deploy_build", &args).unwrap_err();
        assert!(e.contains("COPY package*.json matches no file") && e.contains("Files in that folder: Dockerfile.") && e.contains("index.js") && e.contains("pass dir="), "{e}");
        assert!(!g.r.lines().iter().any(|l| l.contains("build")), "nothing was built: {:?}", g.r.lines());
        assert!(g.t.preview("deploy_build", &args).unwrap().contains("this would be refused"));
        // the fixed layout builds; a CMD naming a missing file is a warning on the result
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        std::fs::write(dir.join("index.js"), "").unwrap();
        g.r.answer("podman build", "STEP 1/1\nSuccessfully tagged\n");
        let ok = g.t.call("deploy_build", &args).unwrap();
        assert!(ok.contains("Built localhost/nodejs-webserver:") && ok.contains("Warning: CMD runs `app.js`") && ok.contains("index.js instead"), "{ok}");
        // a failing docker build: the first failing step and its message, not the banner
        let g2 = rig("buildfail", true);
        let d2 = g2.root.join("app");
        std::fs::create_dir_all(&d2).unwrap();
        std::fs::write(d2.join("Dockerfile"), "FROM scratch\n").unwrap();
        g2.r.fail("podman build", 1, "DEPRECATED: The legacy builder is deprecated and will be removed in a future release.\n            Install the buildx component to build images with BuildKit:\n            https://docs.docker.com/go/buildx/\nStep 2/4 : RUN npm ci\nThe command '/bin/sh -c npm ci' returned a non-zero code: 1\n");
        let e = g2.t.call("deploy_build", &json!({"dir": "app", "image": "app"})).unwrap_err();
        assert!(e.starts_with("the build failed at `Step 2/4 : RUN npm ci`") && e.contains("non-zero code: 1"), "{e}");
        assert!(!e.contains("DEPRECATED") && e.matches("buildx").count() == 1, "{e}");
    }

    #[test]
    fn a_remote_cluster_that_only_came_from_the_environment_is_read_only_until_chosen() {
        let mut g = remote("envremote");
        g.t.profile.source = Source::Env;
        let e = g.t.call("k8s_apply", &json!({"yaml": MANIFEST})).unwrap_err();
        assert!(e.contains("never chosen for this workspace") && e.contains("192.168.1.68"), "{e}");
        g.t.profile.source = Source::Project;
        assert!(g.t.read_only().is_none(), "chosen for the workspace: writes are allowed");
    }

    const NS_OK: &str = r#"{"metadata":{"name":"shop-dev","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/workspace":"Shop"}}}"#;
    const WORKLOADS: &str = r#"{"items":[{"kind":"Deployment","metadata":{"name":"web"},"spec":{"replicas":2}},{"kind":"StatefulSet","metadata":{"name":"db"},"spec":{"replicas":1}},{"kind":"Deployment","metadata":{"name":"off","annotations":{"iyakki.dev/replicas":"3"}},"spec":{"replicas":0}}]}"#;
    const EVERYTHING: &str = r#"{"items":[{"kind":"Deployment","metadata":{"name":"web"},"spec":{"replicas":2}},{"kind":"Service","metadata":{"name":"web"},"spec":{"type":"NodePort"}},{"kind":"Ingress","metadata":{"name":"web"}},{"kind":"Pod","metadata":{"name":"web-1"},"status":{"phase":"Running"}},{"kind":"PersistentVolumeClaim","metadata":{"name":"data"},"spec":{"resources":{"requests":{"storage":"5Gi"}}},"status":{"phase":"Bound"}},{"kind":"ConfigMap","metadata":{"name":"app-config"}},{"kind":"ConfigMap","metadata":{"name":"kube-root-ca.crt"}}]}"#;

    #[test]
    fn stop_saves_the_replicas_then_scales_to_zero_and_start_restores_them_only_in_a_labelled_namespace() {
        let g = rig("stopenv", true);
        g.r.answer("get namespace shop-dev", NS_OK);
        g.r.answer("get deployments,statefulsets", WORKLOADS);
        g.r.answer("annotate", "annotated");
        g.r.answer("scale", "scaled");
        let pv = g.t.preview("k8s_stop_env", &json!({"action": "stop"})).unwrap();
        assert!(pv.contains("deployment web: 2 → 0") && pv.contains("statefulset db: 1 → 0") && !pv.contains("off:"), "{pv}");
        assert!(!g.r.lines().iter().any(|l| l.contains("scale ")), "the preview changes nothing");
        let out = g.t.call("k8s_stop_env", &json!({"action": "stop"})).unwrap();
        assert!(out.starts_with("Stopped namespace shop-dev") && out.contains("PVC data are kept"), "{out}");
        let lines = g.r.lines();
        let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
        assert!(pos("annotate deployment web iyakki.dev/replicas=2") < pos("scale deployment web --replicas=0"), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("annotate statefulset db iyakki.dev/replicas=1")) && lines.iter().any(|l| l.contains("scale statefulset db --replicas=0")));
        assert!(!lines.iter().any(|l| l.contains("scale deployment off")), "already at 0");
        // start brings the saved counts back (and 1 where nothing was saved)
        let s = g.t.call("k8s_stop_env", &json!({"action": "start"})).unwrap();
        assert!(s.starts_with("Started") && g.r.lines().iter().any(|l| l.contains("scale deployment off --replicas=3")), "{s}");
        assert!(g.t.call("k8s_stop_env", &json!({"action": "pause"})).is_err());
        assert!(g.audited.lock().unwrap().iter().any(|a| a.starts_with("k8s.stop ")) && g.audited.lock().unwrap().iter().any(|a| a.starts_with("k8s.start ")), "{:?}", g.audited.lock().unwrap());
        assert_eq!(g.t.defs().iter().find(|d| d.spec.name == "k8s_stop_env").unwrap().risk, Risk::Ask);
    }

    #[test]
    fn only_a_namespace_iyakki_created_for_this_workspace_can_be_stopped_or_removed() {
        for (tag, ns, answer) in [("unlabelled", "shop-dev", r#"{"metadata":{"name":"shop-dev","labels":{}}}"#), ("other-ws", "shop-dev", r#"{"metadata":{"labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/workspace":"Other"}}}"#), ("default", "default", NS_OK), ("kube", "kube-system", NS_OK), ("registry", "iyakki-registry", NS_OK)] {
            let mut g = rig(tag, true);
            g.t.ns = ns.into();
            g.r.answer(&format!("get namespace {ns}"), answer);
            g.r.answer("get deployments,statefulsets", WORKLOADS);
            let e = g.t.call("k8s_teardown", &json!({"confirm": ns})).unwrap_err();
            assert!(e.contains("refused") && e.contains(&format!("kubectl delete namespace {ns}")), "{tag}: {e}");
            assert!(g.t.call("k8s_stop_env", &json!({})).unwrap_err().contains("refused"), "{tag}");
            assert!(!g.r.lines().iter().any(|l| l.contains("delete namespace") || l.contains("scale ")), "{tag}: {:?}", g.r.lines());
            assert!(g.t.preview("k8s_teardown", &json!({"confirm": ns})).unwrap().contains("this would be refused"));
        }
        // a namespace that is not there
        let g = rig("missing", true);
        g.r.fail("get namespace shop-dev", 1, "Error from server (NotFound): namespaces \"shop-dev\" not found");
        assert!(g.t.call("k8s_teardown", &json!({"confirm": "shop-dev"})).unwrap_err().contains("nothing to stop or remove"));
        // never on production, and never on a remote that only came from the environment
        let mut p = rig("prod", true);
        p.t.profile.name = "prod-eu".into();
        p.r.answer("get namespace shop-dev", NS_OK);
        assert!(p.t.call("k8s_teardown", &json!({"confirm": "shop-dev"})).unwrap_err().contains("production"));
        let mut e = remote("envremote2");
        e.t.profile.source = Source::Env;
        e.r.answer("get namespace shop-dev", NS_OK);
        assert!(e.t.call("k8s_stop_env", &json!({})).unwrap_err().contains("never chosen"));
        assert!(!e.r.lines().iter().any(|l| l.contains("scale ") || l.contains("delete namespace")));
    }

    #[test]
    fn remove_all_lists_what_goes_needs_the_typed_name_waits_for_the_delete_and_offers_the_images() {
        let mut g = rig("teardown", true);
        g.t.registry_setting = "localhost:30500".into();
        g.t.registry_kind = "in-cluster".into();
        g.r.answer("get namespace shop-dev", NS_OK);
        g.r.answer("get deployments,statefulsets,services", EVERYTHING);
        g.r.answer("get secrets", "secret/shop-db\nsecret/registry-pull\n");
        g.r.answer("wait --for=delete", "namespace/shop-dev condition met");
        g.r.answer("delete namespace", "namespace \"shop-dev\" deleted");
        g.r.answer("_catalog", r#"{"repositories":["shop-dev/web","other-dev/x"]}"#);
        g.r.answer("tags/list", r#"{"name":"shop-dev/web","tags":["abc"]}"#);
        let pv = g.t.preview("k8s_teardown", &json!({})).unwrap();
        for want in ["REMOVE ALL", "Deployments (1): web (replicas 2)", "Services (1): web (NodePort)", "Ingresses (1): web", "Pods (1): web-1 (Running)", "data (5Gi, Bound)", "ConfigMaps (1): app-config", "Secrets, names only (2): shop-db, registry-pull", "DELETED", "typed"] {
            assert!(pv.contains(want), "{want} in {pv}");
        }
        assert!(!g.r.lines().iter().any(|l| l.contains("delete namespace")), "the preview deletes nothing");
        assert!(g.t.describe("k8s_teardown", &json!({})).contains("type the name"));
        // the typed name is required
        for bad in [json!({}), json!({"confirm": "shop"}), json!({"confirm": "Shop-dev"})] {
            assert!(g.t.call("k8s_teardown", &bad).unwrap_err().contains("confirmation does not match"), "{bad}");
        }
        assert!(!g.r.lines().iter().any(|l| l.contains("delete namespace")), "nothing deleted without it");
        let out = g.t.call("k8s_teardown", &json!({"confirm": "shop-dev"})).unwrap();
        assert!(out.contains("Namespace shop-dev is gone") && out.contains("shop-dev/web (abc)") && out.contains("k8s_registry_cleanup") && !out.contains("other-dev"), "{out}");
        let lines = g.r.lines();
        assert!(lines.iter().position(|l| l.contains("delete namespace shop-dev")) < lines.iter().position(|l| l.contains("wait --for=delete namespace/shop-dev")), "{lines:?}");
        assert!(g.audited.lock().unwrap().iter().any(|a| a.starts_with("k8s.teardown ")), "{:?}", g.audited.lock().unwrap());
        // AUTO and Autopilot still ask; so do grants and "always"
        assert!(crate::agent::hard_ask("k8s_teardown", &json!({})).is_some() && crate::agent::hard_ask("k8s_registry_cleanup", &json!({})).is_some());
        assert!(crate::agent::hard_ask("k8s_stop_env", &json!({})).is_none(), "stop asks (Risk::Ask) but Auto may run it");
        // the old refusal points at the new tool
        let e = g.t.call("k8s_delete", &json!({"kind": "all", "name": "x"})).unwrap_err();
        assert!(e.contains("k8s_teardown") && e.contains("one by one"), "{e}");
        assert!(prompt_line(&g.t.profile, "shop-dev").contains("k8s_teardown"));
    }

    #[test]
    fn a_namespace_that_stays_terminating_is_explained_and_registry_images_are_removed_only_when_allowed() {
        let mut g = rig("terminating", true);
        g.t.registry_setting = "localhost:30500".into();
        g.t.registry_kind = "in-cluster".into();
        g.r.answer("get namespace shop-dev -o json", r#"{"metadata":{"name":"shop-dev","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/workspace":"Shop"}},"spec":{"finalizers":["kubernetes"]},"status":{"conditions":[{"type":"NamespaceContentRemaining","status":"True","message":"Some resources are remaining: persistentvolumeclaims. has 1 resource instances"}]}}"#);
        g.r.answer("get deployments,statefulsets,services", EVERYTHING);
        g.r.fail("wait --for=delete", 1, "error: timed out waiting for the condition on namespaces/shop-dev");
        g.r.answer("delete namespace", "deleted");
        g.r.answer("get all -n shop-dev", "pod/web-1\n");
        let out = g.t.call("k8s_teardown", &json!({"confirm": "shop-dev"})).unwrap();
        assert!(out.contains("still Terminating") && out.contains("NamespaceContentRemaining") && out.contains("persistentvolumeclaims") && out.contains("finalizers: kubernetes") && out.contains("still present: pod/web-1") && out.contains("does not remove finalizers"), "{out}");
        // images: deletion allowed
        let h = rig("regclean", true);
        let mut h = h;
        h.t.registry_setting = "localhost:30500".into();
        h.t.registry_kind = "in-cluster".into();
        h.r.answer("_catalog", r#"{"repositories":["shop-dev/web"]}"#);
        h.r.answer("tags/list", r#"{"tags":["a","b"]}"#);
        h.r.answer("-I", "HTTP/1.1 200 OK\nDocker-Content-Digest: sha256:aaa\n");
        h.r.answer("-X DELETE", "202");
        let pv = h.t.preview("k8s_registry_cleanup", &json!({})).unwrap();
        assert!(pv.contains("shop-dev/web: a, b") && pv.contains("garbage collection"), "{pv}");
        let o = h.t.call("k8s_registry_cleanup", &json!({})).unwrap();
        assert!(o.contains("Deleted 1 image manifests") && o.contains("garbage"), "{o}");
        // deletion disabled: said plainly, nothing changed
        let mut d = rig("regclean-off", true);
        d.t.registry_setting = "localhost:30500".into();
        d.t.registry_kind = "in-cluster".into();
        d.r.answer("_catalog", r#"{"repositories":["shop-dev/web"]}"#);
        d.r.answer("tags/list", r#"{"tags":["a"]}"#);
        d.r.answer("-I", "Docker-Content-Digest: sha256:aaa\n");
        d.r.answer("-X DELETE", "405");
        let e = d.t.call("k8s_registry_cleanup", &json!({})).unwrap_err();
        assert!(e.contains("REGISTRY_STORAGE_DELETE_ENABLED=true") && e.contains("does not change"), "{e}");
    }

    #[test]
    fn the_first_apply_labels_the_namespace_it_creates() {
        let g = rig("labelns", true);
        g.r.fail("get namespace shop-dev", 1, "Error from server (NotFound): namespaces \"shop-dev\" not found");
        g.r.answer("create namespace shop-dev", "namespace/shop-dev created");
        g.r.answer("label namespace", "labeled");
        g.r.answer("diff -f", "");
        g.r.answer("apply -f", "ok");
        let o = g.t.call("k8s_apply", &json!({"yaml": MANIFEST.replace("localhost/web:abc", "web:1")}));
        let lines = g.r.lines();
        assert!(lines.iter().any(|l| l.contains("label namespace shop-dev app.kubernetes.io/managed-by=iyakki iyakki.dev/workspace=Shop")), "{lines:?} {o:?}");
    }

    #[test]
    fn a_namespace_document_for_the_workspace_namespace_is_skipped_and_other_cluster_scoped_kinds_say_what_to_do() {
        let g = rig("nsdoc", true);
        g.r.answer("diff -f", "");
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("apply -f", "deployment.apps/web created");
        let both = format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: shop-dev\n---\n{}", MANIFEST.replace("localhost/web:abc", "web:1"));
        let pv = g.t.preview("k8s_apply", &json!({"yaml": both})).unwrap();
        assert!(pv.contains("manifest check: clean") || pv.contains("manifest check"), "{pv}");
        let o = g.t.call("k8s_apply", &json!({"yaml": both})).unwrap();
        assert!(o.contains("deployment.apps/web created") && o.contains("skipped Namespace shop-dev: it is created automatically"), "{o}");
        let applied = std::fs::read_to_string(g.root.join(".iyakki/deploy/inline.checked.yaml")).or_else(|_| std::fs::read_to_string(g.root.join(".iyakki/deploy/web.checked.yaml"))).unwrap_or_default();
        assert!(!applied.contains("kind: Namespace"), "{applied}");
        // a manifest that is only the Namespace: nothing to apply, said plainly, the namespace is ensured
        let only = g.t.call("k8s_apply", &json!({"yaml": "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: shop-dev\n"})).unwrap();
        assert!(only.contains("skipped Namespace shop-dev") && only.contains("nothing else in this manifest"), "{only}");
        // another cluster-scoped kind (or another namespace's Namespace) is refused with the way out
        for doc in ["apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: x\n", "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: other\n"] {
            let e = g.t.call("k8s_apply", &json!({"yaml": doc})).unwrap_err();
            assert!(e.contains("cluster-scoped") && e.contains("remove this") && e.contains("tell the person to apply it by hand") && e.contains("Do not ask for permissions"), "{e}");
        }
        assert!(prompt_line(&g.t.profile, "shop-dev").contains("never write or apply a Namespace manifest"));
    }

    const MULTI: &str = "FROM node:20-alpine AS base\nWORKDIR /app\nCOPY package*.json ./\nRUN npm ci\nFROM base AS test\nCOPY tests ./tests\nCOPY . .\nRUN npm test -- --coverage\nFROM base AS runtime\nCOPY --from=base /app /app\nUSER node\nCMD [\"node\", \"index.js\"]\n";

    fn multi_stage_layout(g: &Rig, with_tests: bool) -> PathBuf {
        let dir = g.root.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), MULTI).unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        std::fs::write(dir.join("index.js"), "").unwrap();
        if with_tests {
            std::fs::create_dir_all(dir.join("tests")).unwrap();
            std::fs::write(dir.join("tests/a.test.js"), "").unwrap();
        }
        dir
    }

    #[test]
    fn a_test_target_builds_its_own_image_reports_counts_and_coverage_and_is_never_loaded() {
        let g = rig("testbuild", true);
        multi_stage_layout(&g, true);
        g.r.answer("podman build", "STEP 5/8: RUN npm test -- --coverage\nAll files |    87.5 |       80 |\nTests:       5 passed, 5 total\nSuccessfully tagged\n");
        let args = json!({"dir": "app", "image": "web", "target": "test", "build_args": {"NODE_ENV": "test"}});
        let d = g.t.describe("deploy_build", &args);
        assert!(d.contains("--target test") && d.contains("--build-arg NODE_ENV=test") && d.contains("localhost/web-test:"), "{d}");
        let o = g.t.call("deploy_build", &args).unwrap();
        assert!(o.starts_with("TEST BUILD (not for deploy): localhost/web-test:") && o.contains("Tests: 5 passed, 0 failed. Coverage: 87.5%") && o.contains("do NOT call deploy_load"), "{o}");
        assert!(g.t.built_ref().is_none(), "a test image is not the image to deploy");
        // deploy_load refuses it, by what was built and by its name
        let img = o.split_whitespace().find(|w| w.starts_with("localhost/web-test:")).unwrap().trim_end_matches(',').to_string();
        for i in [img.clone(), "localhost/other-test:abc".to_string()] {
            let e = g.t.call("deploy_load", &json!({"image": i})).unwrap_err();
            assert!(e.contains("test images are never loaded or pushed"), "{e}");
        }
        assert!(g.t.describe("deploy_load", &json!({"image": img})).contains("never loaded"));
        assert!(!g.r.lines().iter().any(|l| l.contains("push") || l.contains("images import")));
        // the runtime image of the same Dockerfile (no target) is a normal build
        g.r.answer("podman image exists", "");
        let rt = g.t.call("deploy_build", &json!({"dir": "app", "image": "web"})).unwrap();
        assert!(rt.starts_with("Built localhost/web:") && !rt.contains("TEST BUILD"), "{rt}");
        // a target that is not a stage, and a missing tests folder, are explained before any build
        let n0 = g.r.lines().len();
        let e = g.t.call("deploy_build", &json!({"dir": "app", "image": "web", "target": "nope"})).unwrap_err();
        assert!(e.contains("no stage named `nope`") && e.contains("base, test, runtime"), "{e}");
        let g2 = rig("testbuild-notests", true);
        multi_stage_layout(&g2, false);
        let e = g2.t.call("deploy_build", &json!({"dir": "app", "image": "web", "target": "test"})).unwrap_err();
        assert!(e.contains("COPY tests matches no file") && e.contains("tests folder"), "{e}");
        assert!(g2.r.lines().is_empty() && g.r.lines().len() >= n0);
    }

    #[test]
    fn a_failing_test_stage_returns_the_failing_output_not_the_banner() {
        let g = rig("testfail", true);
        multi_stage_layout(&g, true);
        g.r.fail("podman build", 1, "DEPRECATED: The legacy builder is deprecated and will be removed in a future release.\nStep 8/9 : RUN npm test -- --coverage\nFAIL src/a.test.js\n  ● adds numbers\n    expected 3 received 4\nTests:       1 failed, 4 passed, 5 total\nThe command '/bin/sh -c npm test' returned a non-zero code: 1\n");
        let e = g.t.call("deploy_build", &json!({"dir": "app", "image": "web", "target": "test"})).unwrap_err();
        assert!(e.starts_with("TEST BUILD FAILED (not for deploy") && e.contains("Tests: 4 passed, 1 failed") && e.contains("expected 3 received 4") && e.contains("do not deploy"), "{e}");
        assert!(!e.contains("DEPRECATED") && !e.contains("legacy builder is deprecated"), "{e}");
    }

    #[test]
    fn the_dockerfile_lint_understands_stages_and_targets() {
        let g = rig("stages", true);
        let dir = multi_stage_layout(&g, true);
        let df = dir.join("Dockerfile");
        assert_eq!(crate::dockerlint::stages(MULTI), vec!["base", "test", "runtime"]);
        assert!(crate::dockerlint::check_target(&dir, &df, &g.root, "test").is_ok());
        // stages after the target are not read: a bad COPY there does not stop the test build
        std::fs::write(&df, format!("{MULTI}FROM base AS extra\nCOPY missing.txt /m\n")).unwrap();
        assert!(crate::dockerlint::check_target(&dir, &df, &g.root, "test").is_ok());
        assert!(crate::dockerlint::check(&dir, &df, &g.root).unwrap_err().contains("missing.txt"));
    }

    const JOB_OK: &str = r#"{"status":{"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]}}"#;
    const JOB_FAILED: &str = r#"{"status":{"failed":1,"conditions":[{"type":"Failed","status":"True"}]}}"#;
    const POD_DONE: &str = r#"{"items":[{"metadata":{"name":"web-it-abcde"},"status":{"phase":"Succeeded","containerStatuses":[{"state":{"terminated":{"exitCode":0,"reason":"Completed"}}}]}}]}"#;

    fn job_rig(tag: &str, job: &str, pod: &str) -> Rig {
        let g = rig(tag, true);
        g.r.answer("get services", r#"{"items":[{"metadata":{"name":"web"},"spec":{"type":"ClusterIP","ports":[{"port":8080}]}}]}"#);
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("diff -f", "");
        g.r.answer("apply -f", "job.batch/web-it created");
        g.r.answer("get job web-it", job);
        g.r.answer("get pods", pod);
        g.r.answer("logs web-it-abcde", "Tests: 3 passed, 3 total\nAll files |    75 |  60 |\nok base url http://web.shop-dev.svc.cluster.local:8080\n");
        g.r.answer("delete job", "job.batch \"web-it\" deleted");
        g
    }

    #[test]
    fn deploy_run_job_applies_a_linted_job_waits_reads_the_logs_and_deletes_it() {
        let g = job_rig("jobpass", JOB_OK, POD_DONE);
        let args = json!({"image": "localhost/web-it:abc", "timeout_seconds": 120});
        let pv = g.t.preview("deploy_run_job", &args).unwrap();
        for want in ["kind: Job", "backoffLimit: 0", "activeDeadlineSeconds: 120", "restartPolicy: Never", "runAsNonRoot: true", "app.kubernetes.io/managed-by: iyakki", "iyakki.dev/workspace: Shop", "BASE_URL", "http://web.shop-dev.svc.cluster.local:8080", "limits:"] {
            assert!(pv.contains(want), "{want} in {pv}");
        }
        assert!(!pv.contains("privileged"), "{pv}");
        assert!(!g.r.lines().iter().any(|l| l.contains("apply -f")), "the preview applies nothing");
        let o = g.t.call("deploy_run_job", &args).unwrap();
        assert!(o.starts_with("INTEGRATION TEST PASSED") && o.contains("Tests: 3 passed, 0 failed. Coverage: 75%") && o.contains("Pod logs") && o.contains("The Job was removed"), "{o}");
        let lines = g.r.lines();
        let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
        assert!(pos("apply -f") < pos("get job web-it") && pos("get job web-it") < pos("logs web-it-abcde") && lines.iter().rposition(|l| l.contains("delete job web-it")).unwrap() > pos("logs web-it-abcde"), "{lines:?}");
        assert!(g.audited.lock().unwrap().iter().any(|a| a.starts_with("k8s.job ") && a.contains("passed")), "{:?}", g.audited.lock().unwrap());
        assert_eq!(g.t.defs().iter().find(|d| d.spec.name == "deploy_run_job").unwrap().risk, Risk::Ask);
        // keep=true leaves the Job
        let g2 = job_rig("jobkeep", JOB_OK, POD_DONE);
        let k = g2.t.call("deploy_run_job", &json!({"image": "localhost/web-it:abc", "keep": true})).unwrap();
        assert!(k.contains("The Job is kept") && g2.r.lines().iter().filter(|l| l.contains("delete job")).count() == 1, "only the clean-up of an older run: {k}");
    }

    #[test]
    fn a_failing_or_stuck_job_is_reported_with_logs_and_the_reason() {
        let g = job_rig("jobfail", JOB_FAILED, r#"{"items":[{"metadata":{"name":"web-it-abcde"},"status":{"phase":"Failed","containerStatuses":[{"state":{"terminated":{"exitCode":1,"reason":"Error"}}}]}}]}"#);
        let e = g.t.call("deploy_run_job", &json!({"image": "localhost/web-it:abc"})).unwrap_err();
        assert!(e.starts_with("INTEGRATION TEST FAILED") && e.contains("exited 1") && e.contains("Pod logs") && e.contains("The Job was removed"), "{e}");
        // never finishes: ImagePullBackOff with the registry diagnosis
        let g = job_rig("jobstuck", r#"{"status":{"active":1}}"#, r#"{"items":[{"metadata":{"name":"web-it-abcde"},"status":{"phase":"Pending","containerStatuses":[{"state":{"waiting":{"reason":"ImagePullBackOff","message":"Back-off pulling image"}}}]}}]}"#);
        g.r.answer("get events", "5s Warning Failed pod/web-it-abcde Failed to pull image \"192.168.1.68:30500/shop-dev/web-it:abc\": Head \"https://192.168.1.68:30500/v2/shop-dev/web-it/manifests/abc\": http: server gave HTTP response to HTTPS client\n");
        let e = g.t.call("deploy_run_job", &json!({"image": "localhost/web-it:abc", "timeout_seconds": 1})).unwrap_err();
        assert!(e.starts_with("INTEGRATION TEST TIMED OUT") && e.contains("ImagePullBackOff") && e.contains("Diagnosis") && e.contains("localhost:30500"), "{e}");
        assert!(g.r.lines().iter().any(|l| l.contains("delete job")), "a stuck Job is removed too");
    }

    #[test]
    fn deploy_run_job_is_refused_on_production_for_unusable_names_and_for_unreachable_images() {
        let mut p = job_rig("jobprod", JOB_OK, POD_DONE);
        p.t.profile.name = "prod-eu".into();
        assert!(p.t.call("deploy_run_job", &json!({"image": "localhost/web-it:abc"})).unwrap_err().contains("production"));
        assert!(!p.r.lines().iter().any(|l| l.contains("apply -f")));
        let g = job_rig("jobbad", JOB_OK, POD_DONE);
        for bad in [json!({}), json!({"image": "a b"}), json!({"image": "x", "name": "Bad_Name"}), json!({"image": "x", "env": {"A": "l1\nl2"}}), json!({"image": "x", "env": {"bad-name": "v"}}), json!({"image": "localhost/web-test:abc"})] {
            assert!(g.t.call("deploy_run_job", &bad).is_err(), "{bad}");
        }
        // a bare localhost/ image cannot be pulled by a remote cluster: deploy_load first
        let r = remote("jobremote");
        r.r.answer("get services", r#"{"items":[]}"#);
        r.r.answer("get namespace shop-dev", "namespace/shop-dev");
        let e = r.t.call("deploy_run_job", &json!({"image": "localhost/web-it:abc"})).unwrap_err();
        assert!(e.contains("cannot pull") && e.contains("deploy_load"), "{e}");
        // the manifest check rejects a privileged Job
        let l = deploylint::lint("apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: x\nspec:\n  template:\n    spec:\n      restartPolicy: Never\n      containers:\n      - name: t\n        image: x:1\n        securityContext:\n          privileged: true\n", "shop-dev", &|_| false);
        assert!(!l.ok() && l.report().contains("privileged"), "{}", l.report());
    }

    const ROOT_EVENT: &str = "5s  Warning  Failed  pod/student-attendance-deployment-7d9-x  Error: container has runAsNonRoot and image will run as root (pod: \"x\", container: \"app\")\n4s  Warning  Failed  pod/old-abc  Failed to pull image \"student-attendance-image\": pull access denied\n";

    #[test]
    fn startup_errors_are_diagnosed_in_events_and_status_and_status_ends_with_one_next_action() {
        let g = rig("podfix", true);
        g.r.answer("get events", ROOT_EVENT);
        let ev = g.t.call("k8s_events", &json!({})).unwrap();
        assert!(ev.contains("Diagnosis:") && ev.contains("USER 10001") && ev.contains("student-attendance-image has no registry address"), "{ev}");
        // deploy_status: two pods, one CreateContainerConfigError, one ImagePullBackOff
        g.r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        g.r.answer("config current-context", "default\n");
        g.r.answer("get deployments,statefulsets", r#"{"items":[
          {"kind":"Deployment","metadata":{"name":"student-attendance-deployment","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"replicas":1,"template":{"spec":{"containers":[{"image":"localhost:5000/student-attendance:abc"}]}}},"status":{"readyReplicas":0,"replicas":1}},
          {"kind":"Pod","metadata":{"name":"new-1","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"nodeName":"n1","containers":[{"name":"app","image":"localhost:5000/student-attendance:abc"}]},"status":{"phase":"Pending","containerStatuses":[{"name":"app","ready":false,"restartCount":0,"state":{"waiting":{"reason":"CreateContainerConfigError"}}}]}},
          {"kind":"Pod","metadata":{"name":"old-abc","creationTimestamp":"2026-10-13T09:00:00Z"},"spec":{"nodeName":"n1","containers":[{"name":"app","image":"student-attendance-image"}]},"status":{"phase":"Pending","containerStatuses":[{"name":"app","ready":false,"restartCount":0,"state":{"waiting":{"reason":"ImagePullBackOff"}}}]}}]}"#);
        g.r.answer("get configmaps", "");
        g.r.answer("get secrets", "");
        g.r.answer("get nodes", "{}");
        let s = g.t.call("deploy_status", &json!({})).unwrap();
        assert!(s.contains("Diagnosis:") && s.contains("runAsNonRoot") && s.contains("never built or loaded"), "{s}");
        let last = s.lines().last().unwrap();
        assert!(last.starts_with("Next: add `USER 10001`"), "one concrete action last: {last}");
        assert_eq!(s.matches("\nNext:").count(), 1, "{s}");
        // everything ready: no Next
        let ok = rig("podfix-ok", true);
        ok.r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
        ok.r.answer("config current-context", "default\n");
        ok.r.answer("get deployments,statefulsets", r#"{"items":[{"kind":"Deployment","metadata":{"name":"web","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"replicas":1,"template":{"spec":{"containers":[{"image":"localhost:5000/web:1"}]}}},"status":{"readyReplicas":1,"replicas":1}},{"kind":"Pod","metadata":{"name":"web-1","creationTimestamp":"2026-10-13T10:00:00Z"},"spec":{"nodeName":"n1","containers":[{"name":"web","image":"x"}]},"status":{"phase":"Running","containerStatuses":[{"name":"web","ready":true,"restartCount":0,"state":{"running":{}}}]}}]}"#);
        ok.r.answer("get configmaps", "");
        ok.r.answer("get secrets", "");
        ok.r.answer("get nodes", "{}");
        let s = ok.t.call("deploy_status", &json!({})).unwrap();
        assert!(!s.contains("Next:"), "{s}");
    }

    #[test]
    fn apply_warns_before_applying_about_the_dockerfile_user_a_stale_deployment_and_an_unbuilt_image() {
        let g = rig("prewarn", true);
        let app = g.root.join("student-attendance");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Dockerfile"), "FROM node:20-alpine\nWORKDIR /app\nCOPY . .\nCMD [\"node\",\"index.js\"]\n").unwrap();
        std::fs::create_dir_all(g.root.join("deploy")).unwrap();
        std::fs::write(g.root.join("deploy/student-attendance-deployment.yaml"), "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: student-attendance-deployment\nspec:\n  selector:\n    matchLabels:\n      app: student-attendance\n").unwrap();
        g.r.answer("get deployments -n shop-dev", r#"{"items":[{"metadata":{"name":"student-attendance-deployment"},"spec":{"selector":{"matchLabels":{"app":"student-attendance"}}}}]}"#);
        g.r.answer("diff -f", "");
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("apply -f", "deployment.apps/student-attendance created");
        let manifest = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: student-attendance\nspec:\n  selector:\n    matchLabels:\n      app: student-attendance\n  template:\n    metadata:\n      labels:\n        app: student-attendance\n    spec:\n      containers:\n      - name: app\n        image: student-attendance-image\n        resources:\n          limits:\n            cpu: 100m\n        securityContext:\n          runAsNonRoot: true\n";
        let pv = g.t.preview("k8s_apply", &json!({"yaml": manifest})).unwrap();
        assert!(pv.contains("has no USER, so the image runs as root") && pv.contains("USER 10001"), "{pv}");
        assert!(pv.contains("Deployment `student-attendance-deployment` already runs") && pv.contains("k8s_delete kind=deployment name=student-attendance-deployment") && pv.contains("deploy/student-attendance-deployment.yaml"), "{pv}");
        assert!(pv.contains("student-attendance-image: this image name has no registry address"), "{pv}");
        assert!(!g.r.lines().iter().any(|l| l.contains("apply -f")), "warnings come before anything is applied");
        let o = g.t.call("k8s_apply", &json!({"yaml": manifest})).unwrap();
        assert!(o.contains("warning:") && o.contains("USER 10001"), "{o}");
        // with a numeric USER the user warning is gone
        std::fs::write(app.join("Dockerfile"), "FROM node:20-alpine\nCOPY . .\nUSER 10001\nCMD [\"node\",\"index.js\"]\n").unwrap();
        let pv = g.t.preview("k8s_apply", &json!({"yaml": manifest})).unwrap();
        assert!(!pv.contains("has no USER"), "{pv}");
        std::fs::write(app.join("Dockerfile"), "FROM node\nUSER node\n").unwrap();
        assert!(g.t.preview("k8s_apply", &json!({"yaml": manifest})).unwrap().contains("USER node"), "a name is not enough");
        // a loaded image name is not flagged as unbuilt
        let loaded = manifest.replace("student-attendance-image", "localhost:5000/student-attendance:abc");
        assert!(!g.t.preview("k8s_apply", &json!({"yaml": loaded})).unwrap().contains("no registry address"));
    }

    #[test]
    fn a_system_registry_choice_on_a_remote_cluster_is_not_used_and_a_localhost_image_is_not_applied() {
        let mut g = remote("sys-on-remote");
        g.t.registry_setting = REGISTRY.into();
        g.t.registry_kind = "system".into();
        assert!(g.t.system_on_remote());
        g.r.answer("podman image exists", "");
        g.r.answer("get services -A", SVCS);
        g.r.answer("get nodes", NODES);
        g.r.answer("curl -fsS", "{}");
        g.r.answer("podman tag", "");
        g.r.answer("podman push", "done");
        let d = g.t.describe("deploy_load", &json!({"image": "localhost/web:abc"}));
        assert!(d.contains("localhost:30500/shop-dev/web:abc"), "{d}");
        let o = g.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap();
        assert!(o.contains("cannot reach") && o.contains("discovered NodePort registry") && o.contains("Use image: localhost:30500/shop-dev/web:abc"), "{o}");
        assert!(g.r.lines().contains(&"podman push --tls-verify=false 192.168.1.68:30500/shop-dev/web:abc".to_string()), "{:?}", g.r.lines());
        // nothing discoverable: the error says why and points at Registry… A
        let mut n = remote("sys-on-remote-none");
        n.t.registry_setting = REGISTRY.into();
        n.t.registry_kind = "system".into();
        n.r.answer("podman image exists", "");
        n.r.answer("get services -A", r#"{"items":[]}"#);
        let e = n.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("cannot reach") && e.contains("registry_setup") && e.contains("pick A") || e.contains("Registry"), "{e}");
        // option A chosen but not applied: deploy_load says so, on remote and local clusters
        let mut p = remote("pending-a");
        p.t.registry_kind = "in-cluster".into();
        p.r.answer("podman image exists", "");
        p.r.answer("get services -A", r#"{"items":[]}"#);
        let e = p.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("not set up yet"), "{e}");
        let mut l = rig("pending-a-local", true);
        l.t.registry_kind = "in-cluster".into();
        l.r.answer("podman image exists", "");
        let e = l.t.call("deploy_load", &json!({"image": "localhost/web:abc"})).unwrap_err();
        assert!(e.contains("not set up yet") && !l.r.lines().iter().any(|x| x.contains("images import")), "{e}");
        // the bare localhost/ image is refused by apply on a remote cluster, and fine on a local one
        let e = g.t.call("k8s_apply", &json!({"yaml": MANIFEST})).unwrap_err();
        assert!(e.contains("localhost/web:abc") && e.contains("cannot pull") && e.contains("deploy_load"), "{e}");
    }

    #[test]
    #[ignore = "reads the real cluster on this machine (kubectl get only); never applies anything"]
    fn live_in_cluster_plan_reads_the_cluster_and_applies_nothing() {
        let root = tmp("live-regplan");
        let Some(t) = real_tools("iyakki-live-regplan", &root) else { return };
        let p = t.call("cluster_setup_plan", &json!({"option": "in-cluster"})).expect("plan");
        println!("{p}");
        assert!(p.contains("iyakki-registry") && p.contains("plan only"));
        let a = t.call("cluster_setup_plan", &json!({"option": "system"}));
        println!("{a:?}");
    }

    #[test]
    fn registry_setup_help_and_cluster_setup_plan_are_read_only_and_never_write_or_run_anything_with_root() {
        let mut g = rig("regtools", true);
        g.t.registry_setting = "192.168.1.68:30500".into();
        let h = g.t.call("registry_setup_help", &json!({})).unwrap();
        assert!(h.contains("mirrors:\n  \"192.168.1.68:30500\":") && h.contains("sudo tee") && h.contains("hostname the nodes already trust"), "{h}");
        let h2 = g.t.call("registry_setup_help", &json!({"registry": "http://10.0.0.5:5000/"})).unwrap();
        assert!(h2.contains("\"http://10.0.0.5:5000\""), "{h2}");
        assert!(g.t.call("registry_setup_help", &json!({"registry": "nonsense"})).is_err());
        let p = g.t.call("cluster_setup_plan", &json!({"option": "local"})).unwrap();
        assert!(p.contains("podman run -d --restart=always") && p.contains("plan only"), "{p}");
        assert!(g.t.call("cluster_setup_plan", &json!({"option": "remote", "registry": "bad"})).is_err());
        assert!(!g.root.join("registries.yaml").exists());
        assert!(!g.r.lines().iter().any(|l| l.contains("podman run") || l.contains("sudo")), "{:?}", g.r.lines());
        for n in ["registry_setup_help", "cluster_setup_plan"] {
            assert_eq!(g.t.defs().iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Safe);
        }
    }

    #[test]
    fn the_screenshot_scenario_deploy_this_connects_builds_loads_and_applies_and_cannot_start_k3s() {
        use crate::agent::{start_with, RunEvent, RunLimits, Tools, Verdict};
        use crate::assistrun::Assist;
        use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall};

        // the model: status, build, load, apply, then it answers; at the start it also tries `k3s server` like the screenshot
        struct Script(Mutex<Vec<ModelTurn>>);
        impl ChatBackend for Script {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                Err("unused".into())
            }
            fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
                let mut v = self.0.lock().unwrap();
                if v.is_empty() { Ok(ModelTurn { text: "Deployed.".into(), calls: vec![] }) } else { Ok(v.remove(0)) }
            }
        }
        let call = |id: &str, name: &str, args: Value| ModelTurn { text: String::new(), calls: vec![ToolCall { id: id.into(), name: name.into(), args }] };
        let g = rig("flow", true);
        g.r.answer("version -o json", r#"{"serverVersion":{"gitVersion":"v1.36.5+k3s1"}}"#);
        g.r.answer("get namespace shop-dev", "namespace/shop-dev");
        g.r.answer("podman build", "Successfully tagged");
        g.r.answer("podman image exists", "");
        g.r.answer("podman save", "");
        g.r.answer("k3s ctr images import", "ok");
        g.r.answer("diff -f", "");
        g.r.answer("apply -f", "deployment.apps/web created\n");
        std::fs::write(g.root.join("deploy/app.yaml"), MANIFEST).unwrap();
        let tag = content_tag(&g.root.join("app"));
        let image = format!("localhost/web:{tag}");
        let manifest = MANIFEST.replace("localhost/web:abc", &image);
        std::fs::write(g.root.join("deploy/app.yaml"), manifest).unwrap();
        let script = Script(Mutex::new(vec![call("a", "k8s_status", json!({})), call("b", "deploy_build", json!({"dir": "app", "image": "web"})), call("c", "deploy_load", json!({"image": image})), call("d", "k8s_apply", json!({"manifest_path": "deploy/app.yaml"}))]));
        // the tools the chat gets: the project wrapper around a base that has run_command (stand-in) plus the k8s tools
        struct Base(Mutex<Vec<String>>);
        impl Tools for Base {
            fn defs(&self) -> Vec<ToolDef> {
                vec![ToolDef { spec: ToolSpec { name: "run_command".into(), description: "run".into(), schema: json!({"type": "object", "properties": {"program": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}}, "required": ["program"]}) }, risk: Risk::Ask, untrusted: true }]
            }
            fn call(&self, name: &str, args: &Value) -> Result<String, String> {
                self.0.lock().unwrap().push(format!("{name} {args}"));
                Ok("ran".into())
            }
            fn describe(&self, name: &str, _a: &Value) -> String {
                name.into()
            }
        }
        let base = Arc::new(Base(Mutex::new(vec![])));
        let tools: Arc<dyn Tools> = Arc::new(crate::boardtools::ProjectTools::new(base.clone(), &g.root, "Shop", &g.root.join("FEATURES.md")).with_k8s(Arc::new(g.t)));
        let n_asked = Arc::new(AtomicUsize::new(0));
        let h = start_with(Arc::new(script), tools, vec![Message::system("SYS"), Message::user("deploy this to the cluster")], RunLimits::default(), Assist::standard(crate::toolblock::ReminderCtx { agent_on: true, browser_policy: "off".into(), engine_installed: false, screen_available: false, loaded: None }));
        let mut events = vec![];
        let mut asked: Vec<(String, String, String)> = vec![];
        let t0 = std::time::Instant::now();
        loop {
            while let Some(e) = h.try_event() {
                match &e {
                    RunEvent::Approval { id, name, summary, detail } => {
                        n_asked.fetch_add(1, Ordering::SeqCst);
                        asked.push((name.clone(), summary.clone(), detail.clone()));
                        h.decide(id, Verdict::Approve);
                    }
                    _ => {}
                }
                let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
                events.push(e);
                if done {
                    break;
                }
            }
            if events.iter().any(|e| matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. })) || t0.elapsed() > Duration::from_secs(20) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(events.iter().any(|e| matches!(e, RunEvent::Final { text } if text == "Deployed.")), "{events:?}");
        // approvals: build, load, apply (status is read-only), each labelled with the workspace and showing the real thing
        assert_eq!(asked.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), vec!["deploy_build", "deploy_load", "k8s_apply"], "{asked:?}");
        assert!(asked.iter().all(|a| a.1.starts_with("[Shop] ")), "{asked:?}");
        assert!(asked[0].2.contains("podman build"), "{}", asked[0].2);
        assert!(asked[2].2.contains("namespace shop-dev") && asked[2].2.contains("manifest check"), "{}", asked[2].2);
        // the order of the commands, and no k3s server anywhere
        let lines = g.r.lines();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} in {lines:?}"));
        assert!(pos("podman build") < pos("podman save") && pos("podman save") < pos("k3s ctr images import") && pos("k3s ctr images import") < pos(" apply -f "), "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("k3s server") || l.contains("sudo")), "{lines:?}");
        assert!(base.0.lock().unwrap().is_empty(), "the base tools were not used");
    }

    #[test]
    fn the_command_tool_cannot_start_a_cluster_in_a_workspace_chat_with_a_connected_cluster() {
        use crate::agent::Tools;
        struct Base(Mutex<Vec<String>>);
        impl Tools for Base {
            fn defs(&self) -> Vec<ToolDef> {
                vec![ToolDef { spec: ToolSpec { name: "run_command".into(), description: "run".into(), schema: json!({"type": "object"}) }, risk: Risk::Ask, untrusted: true }]
            }
            fn call(&self, name: &str, args: &Value) -> Result<String, String> {
                self.0.lock().unwrap().push(format!("{name} {args}"));
                Ok("ran".into())
            }
            fn describe(&self, name: &str, _a: &Value) -> String {
                name.into()
            }
        }
        let g = rig("guard", true);
        let base = Arc::new(Base(Mutex::new(vec![])));
        let tools = crate::boardtools::ProjectTools::new(base.clone(), &g.root, "Shop", &g.root.join("FEATURES.md")).with_k8s(Arc::new(g.t));
        let server = json!({"program": "k3s", "args": ["server"]});
        // the model is told before it is asked: the refusal is the tool result, and no approval prompt is raised for it
        let why = tools.refusal("run_command", &server).expect("refused");
        assert!(why.contains("already running and connected") && why.contains("will not start another one"), "{why}");
        assert!(tools.call("run_command", &server).unwrap_err().contains("will not start another one"));
        assert!(tools.describe("run_command", &server).contains("REFUSED"));
        // reading and kubectl pass through
        for ok in [json!({"program": "kubectl", "args": ["get", "pods"]}), json!({"program": "k3s", "args": ["kubectl", "get", "pods"]}), json!({"program": "k3s", "args": ["ctr", "images", "ls"]})] {
            assert!(tools.refusal("run_command", &ok).is_none(), "{ok}");
            assert_eq!(tools.call("run_command", &ok).unwrap(), "ran");
        }
        assert_eq!(base.0.lock().unwrap().len(), 3, "only the allowed ones reached the base tool");
    }

    // ---------------------------------------------------------------------------------------- live (this machine's real k3s)

    fn real_tools(ns: &str, root: &Path) -> Option<K8sTools> {
        let run: Arc<dyn Runner> = Arc::new(crate::wsexec::System);
        let res = crate::devcluster::resolve(&run, &crate::devcluster::Env::real(), &crate::workspace::K8sConfig::default(), None, ns, false);
        println!("candidates:");
        for p in &res.profiles {
            println!("  {} — {} — {}", p.chip(), p.kubeconfig, p.server);
        }
        let p = res.reachable()?.clone();
        println!("chosen: {}", p.describe());
        Some(K8sTools::new(run, p, ns, "live-test", root, Arc::new(|a, d| println!("  audit k8s.{a} {d}"))))
    }

    /// READ-ONLY against the real cluster of this machine: status, namespaces, pods, the log of one pod. Run with
    /// `cargo test -p iyakki-agent --lib live_read_only -- --ignored --nocapture`.
    #[test]
    #[ignore = "talks to this machine's real k3s (read-only)"]
    fn live_read_only_status_namespaces_pods_and_one_log() {
        let root = tmp("live-ro");
        let t = real_tools("iyakki-live-readonly", &root).expect("a reachable cluster on this machine");
        println!("--- k8s_status\n{}", t.call("k8s_status", &json!({})).unwrap());
        println!("--- k8s_get namespaces\n{}", t.call("k8s_get", &json!({"kind": "namespaces"})).unwrap());
        let pods = t.call("k8s_get", &json!({"kind": "pods", "namespace": "kube-system"})).unwrap();
        println!("--- k8s_get pods (kube-system)\n{pods}");
        if let Some(pod) = pods.lines().nth(1).and_then(|l| l.split_whitespace().next()) {
            println!("--- k8s_logs {pod}\n{}", t.call("k8s_logs", &json!({"pod": pod, "namespace": "kube-system", "tail": 5})).unwrap_or_else(|e| format!("(error: {e})")));
        }
        println!("--- k8s_events (workspace namespace)\n{}", t.call("k8s_events", &json!({})).unwrap_or_else(|e| format!("(error: {e})")));
        println!("--- deploy_status\n{}", t.call("deploy_status", &json!({})).unwrap_or_else(|e| format!("(error: {e})")));
    }

    /// WRITES to the real cluster, only into a throwaway namespace `iyakki-test-<random>` that is deleted afterwards, and
    /// BYPASSES the approval prompts (the harness calls the tools directly). It applies one ConfigMap (no image is pulled).
    #[test]
    #[ignore = "WRITES to this machine's real k3s (throwaway namespace; approvals bypassed by the harness)"]
    fn live_apply_into_a_throwaway_namespace_then_delete_it() {
        let mut r = [0u8; 4];
        let _ = iyakki_sec::random_fill(&mut r);
        let ns = format!("iyakki-test-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let root = tmp("live-rw");
        let t = real_tools(&ns, &root).expect("a reachable cluster on this machine");
        struct Cleanup(Arc<dyn Runner>, ClusterProfile, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let kc = Kc::new(self.0.clone(), &self.1.kubeconfig, &self.1.context);
                let o = kc.kubectl(&["delete", "namespace", &self.2, "--wait=false"], 8, 60);
                println!("cleanup: kubectl delete namespace {} -> {:?} {}", self.2, o.code, o.combined().trim());
            }
        }
        let _cleanup = Cleanup(t.run.clone(), t.profile.clone(), ns.clone());
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: iyakki-live\ndata:\n  hello: world\n";
        println!("--- preview\n{}", t.preview("k8s_apply", &json!({"yaml": yaml})).unwrap());
        println!("--- apply\n{}", t.call("k8s_apply", &json!({"yaml": yaml})).unwrap());
        println!("--- get configmaps\n{}", t.call("k8s_get", &json!({"kind": "configmaps"})).unwrap());
        assert!(t.call("k8s_get", &json!({"kind": "configmaps"})).unwrap().contains("iyakki-live"));
        println!("--- delete\n{}", t.call("k8s_delete", &json!({"kind": "configmaps", "name": "iyakki-live"})).unwrap());
    }
}
