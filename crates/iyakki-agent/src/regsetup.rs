//! Container registries for k3s (docs/K3S-DEV.md): why a node cannot pull from one and the exact fix, and the registry option of the
//! cluster setup plan.
//!
//! * [`diagnose`] recognises the registry errors that end in `ErrImagePull` ("server gave HTTP response to HTTPS client", `x509`,
//!   "no basic auth credentials", "connection refused") and returns the cause and the fix. [`setup_help`] is the ready-to-copy
//!   `/etc/rancher/k3s/registries.yaml` with the commands for every node. Nothing here runs anything or edits `/etc`.
//! * [`plan`] / [`preview`] / [`apply`] are the setup: [`RegistryOption`] none, A a registry pod inside the cluster (namespace
//!   `iyakki-registry`, PVC, NodePort Service; pods pull `localhost:<nodePort>`, so no node needs configuring and no root is used),
//!   B the system registry (a `registry:2` container on `localhost:5000`), or C an existing remote registry. The plan lists the
//!   exact objects, file and commands; [`apply`] needs the approval flag, writes `registries.yaml` only when running as root
//!   (otherwise the commands are returned for the person to run), runs kubectl / the registry container through the [`Runner`], and
//!   records the choice in `.iyakki/project.json` (`k8s.registry` = where this machine pushes, `k8s.registry_pull` = what the pods
//!   pull, `k8s.registry_kind`) so `deploy_load` uses it. The IDE's K8S panel calls it (Set up registry…) and so does the
//!   `registry_setup` tool; `cluster_setup_plan` is the read-only preview. [`status`] feeds the panel's Registry chip.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::wsexec::{Out, Runner};

/// The local registry container.
pub const LOCAL_REGISTRY: &str = "localhost:5000";
pub const LOCAL_NAME: &str = "iyakki-registry";
pub const LOCAL_VOLUME: &str = "iyakki-registry";
pub const YAML_PATH: &str = "/etc/rancher/k3s/registries.yaml";

fn valid_hostport(s: &str) -> bool {
    let Some((h, p)) = s.rsplit_once(':') else { return false };
    !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit())
}

/// A `registries.yaml` that lets containerd pull `reg` over plain HTTP.
pub fn http_mirror_yaml(reg: &str) -> String {
    format!("mirrors:\n  \"{reg}\":\n    endpoint:\n      - \"http://{reg}\"\n")
}

/// An HTTPS registry whose certificate is not trusted (self-signed): skip verification (or point `ca_file` at its certificate).
pub fn insecure_tls_yaml(reg: &str) -> String {
    format!("mirrors:\n  \"{reg}\":\n    endpoint:\n      - \"https://{reg}\"\nconfigs:\n  \"{reg}\":\n    tls:\n      insecure_skip_verify: true\n")
}

fn tee_commands(yaml: &str) -> String {
    format!("sudo mkdir -p /etc/rancher/k3s\nsudo tee {YAML_PATH} >/dev/null <<'EOF'\n{yaml}EOF")
}

const RESTART: &str = "sudo systemctl restart k3s          # on the server node\nsudo systemctl restart k3s-agent    # on every agent node";

/// The ready-to-copy fix for a registry that serves plain HTTP: the file, the commands, the alternative.
pub fn setup_help(reg: &str) -> String {
    format!(
        "The nodes' containerd talks HTTPS to {reg}, but that registry serves plain HTTP. IYAKKI never edits /etc or logs in to nodes, so run this yourself on EACH node that must pull (the server and every k3s-agent node):\n\n\
         {}\n{RESTART}\n\n\
         The file (/etc/rancher/k3s/registries.yaml):\n{}\n\
         If the file already exists, add the `\"{reg}\"` entry under its existing `mirrors:` instead of replacing it. Restarting k3s restarts the containers on that node briefly; do one node at a time.\n\n\
         Alternative with nothing to change on the nodes: push to a registry hostname the nodes already trust (the one your existing workloads pull from) by setting \"registry\" under \"k8s\" in .iyakki/project.json to its host:port.",
        tee_commands(&http_mirror_yaml(reg)),
        http_mirror_yaml(reg)
    )
}

/// The registry named in an error ("https://192.168.1.68:30500/v2/…", "dial tcp 10.0.0.5:5000: …").
pub fn registry_in(text: &str) -> Option<String> {
    for key in ["https://", "http://", "dial tcp "] {
        let mut from = 0;
        while let Some(i) = text[from..].find(key) {
            let s = from + i + key.len();
            let host: String = text[s..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')).collect();
            let host = host.trim_end_matches(':').to_string();
            if valid_hostport(&host) && host != "127.0.0.1:443" && !host.ends_with(":6443") {
                return Some(host);
            }
            from = s;
        }
    }
    None
}

/// The cause and the fix for a registry pull error found in `text` (kubectl describe / events / a pod message); `fallback` is the
/// project's registry when the text names none. None when the text holds no known registry error.
pub fn diagnose(text: &str, fallback: &str) -> Option<String> {
    let l = text.to_lowercase();
    let reg = registry_in(text).or_else(|| (!fallback.is_empty()).then(|| fallback.to_string())).unwrap_or_else(|| "<registry host:port>".into());
    let pull = l.contains("pull") || l.contains("/v2/") || l.contains("registry");
    if l.contains("server gave http response to https client") {
        let first = match reg.rsplit_once(':') {
            Some((h, port)) if h != "localhost" && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => format!("First try, if this registry is a NodePort Service (no node changes, no root): containerd allows plain HTTP for localhost and a NodePort is reachable on every Ready node at localhost, so point the image at localhost:{port} instead, e.g.\n  kubectl set image deployment/<name> <container>=localhost:{port}/<namespace>/<image>:<tag> -n <namespace>\n(and use that form in the manifest). This works where kube-proxy exposes NodePorts on localhost (default iptables/ipvs). If it still fails, or the registry is not a NodePort, use the mirror below.\n\n"),
            _ => String::new(),
        };
        return Some(format!("Registry pull failed: {reg} serves plain HTTP but the nodes' containerd tries HTTPS, and no mirror/insecure entry for that address exists on the node.\n{first}{}", setup_help(&reg)));
    }
    if l.contains("x509") && pull {
        return Some(format!("Registry pull failed: the certificate of {reg} is not trusted by the node (x509). Fix on EACH node that must pull, then restart k3s (server) / k3s-agent (agents):\n{}\n{RESTART}\nBetter: add the registry's CA certificate (`configs: \"{reg}\": tls: ca_file: /path/ca.crt`) or use a certificate from a CA the nodes trust. Alternative: push to a registry hostname the nodes already trust.", tee_commands(&insecure_tls_yaml(&reg))));
    }
    if l.contains("no basic auth credentials") || (l.contains("unauthorized") && pull) {
        return Some(format!("Registry pull failed: {reg} requires a login and the pod has none (no basic auth credentials). Either give the nodes the login in /etc/rancher/k3s/registries.yaml:\n  configs:\n    \"{reg}\":\n      auth:\n        username: <user>\n        password: <password>\nthen restart k3s / k3s-agent on each node (keep the password out of chat and out of git), or create a pull secret in the namespace (kubectl create secret docker-registry regcred --docker-server={reg} --docker-username=<user> --docker-password=<password>) and reference it as imagePullSecrets in the manifest. Or use an anonymous registry the nodes already trust."));
    }
    if l.contains("connection refused") && pull {
        return Some(format!("Registry pull failed: nothing answers at {reg} from the node (connection refused). Check that the registry is running and listening on that address, that a NodePort/port is open in the node's firewall, and that the address is one the NODE can reach: `localhost` inside a pull means the node itself, so a registry on another machine needs that machine's address. Test from the node: `curl -sS http://{reg}/v2/`. If it answers over http, the node also needs the http mirror:\n{}", setup_help(&reg)));
    }
    None
}

// ------------------------------------------------------------------------------------------------------------------ setup plan

/// The namespace, Service name and default NodePort of the registry that runs inside the cluster.
pub const IN_NS: &str = "iyakki-registry";
pub const IN_NAME: &str = "registry";
pub const DEFAULT_NODE_PORT: u16 = 30500;
const NODE_PORTS: std::ops::RangeInclusive<u16> = 30500..=32767;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryOption {
    None,
    /// A `registry:2` Deployment inside the cluster (namespace `iyakki-registry`, a PVC, a NodePort Service). `node_port` 0: 30500 or the
    /// next free one.
    InCluster { node_port: u16 },
    /// The system registry: a `registry:2` container (podman or docker) on `localhost:5000` with a data volume.
    Local,
    /// An existing registry. `insecure`: plain HTTP, or HTTPS with a certificate nobody vouches for.
    Remote { hostport: String, https: bool, insecure: bool },
}

impl RegistryOption {
    /// From tool arguments: `none | in-cluster | system (local) | remote`, host:port, `http|https`, insecure.
    pub fn parse(option: &str, hostport: &str, scheme: &str, insecure: bool) -> Result<RegistryOption, String> {
        match option.trim().to_lowercase().as_str() {
            "" | "none" => Ok(RegistryOption::None),
            "local" | "system" => Ok(RegistryOption::Local),
            "in-cluster" | "incluster" | "in_cluster" | "cluster" | "pod" => {
                let port = match hostport.trim() {
                    "" => 0,
                    p => p.parse::<u16>().ok().filter(|p| NODE_PORTS.contains(p)).ok_or_else(|| format!("`{p}` is not a NodePort (30500 to 32767)"))?,
                };
                Ok(RegistryOption::InCluster { node_port: port })
            }
            "remote" | "existing" => {
                let h = hostport.trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
                if !valid_hostport(h) {
                    return Err(format!("`{hostport}` is not a registry address (host:port)"));
                }
                let https = scheme.trim().eq_ignore_ascii_case("https");
                Ok(RegistryOption::Remote { hostport: h.into(), https, insecure: insecure || !https })
            }
            o => Err(format!("unknown registry option `{o}` (none, in-cluster, system or remote)")),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            RegistryOption::None => "none",
            RegistryOption::InCluster { .. } => "in-cluster",
            RegistryOption::Local => "system",
            RegistryOption::Remote { .. } => "remote",
        }
    }
}

/// The three choices of the setup dialog, best first for a cluster without root: (option word, name, one-line pros, root needs).
pub const CHOICES: [(&str, &str, &str, &str); 3] = [
    ("in-cluster", "A  Registry pod inside the cluster (recommended)", "No root and no node changes: pods pull localhost:<nodePort>; works for a local or a remote k3s.", "no root"),
    ("system", "B  System registry (container on this machine)", "Best for one local node: a registry:2 container with podman/docker on localhost:5000.", "root only to write the node's registries.yaml (commands given)"),
    ("remote", "C  An existing registry", "Use a registry you already have (host:port); nothing is installed.", "root only for plain-HTTP or untrusted-TLS registries (commands given)"),
];

/// How to reach the cluster's kubectl.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KubeCtx {
    pub kubeconfig: String,
    pub context: String,
}

/// What the cluster says that the in-cluster plan depends on.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The StorageClass for the PVC: the default one, else `local-path`; None: use `emptyDir`.
    pub storage_class: Option<String>,
    pub taken_ports: Vec<u16>,
    /// The NodePort of our own Service when it already exists (re-running is idempotent).
    pub existing_port: Option<u16>,
    /// The size of the volume claim in Gi; 0: [`DEFAULT_SIZE_GI`].
    pub size_gi: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PlanEnv {
    pub is_root: bool,
    /// podman or docker, if installed.
    pub engine: Option<String>,
    pub registries_yaml: PathBuf,
    /// The workspace whose `.iyakki/project.json` records the choice.
    pub project_root: Option<PathBuf>,
    /// kubectl target for the in-cluster option.
    pub kube: Option<KubeCtx>,
    /// The host of the API server (`192.168.1.68`); empty or a loopback name: the cluster is on this machine.
    pub api_host: String,
    pub facts: Facts,
}

impl PlanEnv {
    pub fn real(engine: Option<&str>, project_root: Option<&Path>) -> PlanEnv {
        // SAFETY: geteuid has no preconditions.
        PlanEnv { is_root: unsafe { libc::geteuid() } == 0, engine: engine.map(str::to_string), registries_yaml: YAML_PATH.into(), project_root: project_root.map(Path::to_path_buf), ..Default::default() }
    }
}

#[derive(Clone, Debug)]
pub struct SetupPlan {
    pub option: RegistryOption,
    /// The address this machine pushes to; the value for `k8s.registry` (empty: unchanged).
    pub registry: String,
    /// The address the nodes pull from (`localhost:30500`); empty: the same as `registry`.
    pub pull: String,
    /// The mirror file to create, if one is needed and none exists yet.
    pub file: Option<(PathBuf, String)>,
    /// The registry container: engine and its run arguments.
    pub container: Option<Vec<String>>,
    /// The in-cluster manifest (namespaced objects only; the Namespace is created by its own approved step).
    pub manifest: Option<String>,
    pub env: PlanEnv,
    pub notes: Vec<String>,
}

fn loopback(h: &str) -> bool {
    h.is_empty() || h == "localhost" || h == "127.0.0.1" || h == "[::1]"
}

/// The NodePort to use: ours when it exists, else `want` (or 30500) if free, else the next free one.
pub fn choose_port(want: u16, f: &Facts) -> Result<u16, String> {
    if let Some(p) = f.existing_port {
        return Ok(p);
    }
    let first = if want == 0 { DEFAULT_NODE_PORT } else { want };
    if !f.taken_ports.contains(&first) {
        return Ok(first);
    }
    if want != 0 {
        return Err(format!("NodePort {want} is already used by another Service in the cluster; pick another (30500 to 32767) or leave it empty"));
    }
    NODE_PORTS.into_iter().find(|p| !f.taken_ports.contains(p)).ok_or_else(|| "every NodePort from 30500 to 32767 is taken".to_string())
}

/// The registry's Deployment, PVC (or `emptyDir`) and NodePort Service. No namespace in the objects: it is given with `-n`.
pub fn in_cluster_manifest(node_port: u16, storage_class: Option<&str>) -> String {
    in_cluster_manifest_sized(node_port, storage_class, DEFAULT_SIZE_GI)
}

/// The default size of the registry's volume claim, in Gi.
pub const DEFAULT_SIZE_GI: u32 = 10;

/// [`in_cluster_manifest`] with the volume claim's size (Gi) chosen.
pub fn in_cluster_manifest_sized(node_port: u16, storage_class: Option<&str>, size_gi: u32) -> String {
    let (volume, pvc) = match storage_class {
        Some(c) => ("      - name: data\n        persistentVolumeClaim:\n          claimName: registry-data\n".to_string(), format!("apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: registry-data\nspec:\n  accessModes:\n  - ReadWriteOnce\n  storageClassName: {c}\n  resources:\n    requests:\n      storage: {size_gi}Gi\n---\n")),
        None => ("      - name: data\n        emptyDir: {}\n".to_string(), String::new()),
    };
    format!(
        "{pvc}apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {IN_NAME}\n  labels:\n    app: {IN_NAME}\nspec:\n  replicas: 1\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n      app: {IN_NAME}\n  template:\n    metadata:\n      labels:\n        app: {IN_NAME}\n    spec:\n      containers:\n      - name: {IN_NAME}\n        image: docker.io/library/registry:2\n        ports:\n        - containerPort: 5000\n        env:\n        - name: REGISTRY_STORAGE_DELETE_ENABLED\n          value: \"true\"\n        volumeMounts:\n        - name: data\n          mountPath: /var/lib/registry\n        resources:\n          requests:\n            cpu: 50m\n            memory: 64Mi\n          limits:\n            cpu: 500m\n            memory: 512Mi\n        securityContext:\n          allowPrivilegeEscalation: false\n      volumes:\n{volume}---\napiVersion: v1\nkind: Service\nmetadata:\n  name: {IN_NAME}\nspec:\n  type: NodePort\n  selector:\n    app: {IN_NAME}\n  ports:\n  - port: 5000\n    targetPort: 5000\n    nodePort: {node_port}\n"
    )
}

pub fn plan(option: &RegistryOption, env: &PlanEnv) -> Result<SetupPlan, String> {
    let mut p = SetupPlan { option: option.clone(), registry: String::new(), pull: String::new(), file: None, container: None, manifest: None, env: env.clone(), notes: vec![] };
    let existing = std::fs::read_to_string(&env.registries_yaml).ok();
    let want = |reg: &str, yaml: String, p: &mut SetupPlan| match &existing {
        Some(t) if t.contains(&format!("\"{reg}\"")) => p.notes.push(format!("{} already has an entry for {reg}; it is left as it is.", env.registries_yaml.display())),
        Some(_) => p.notes.push(format!("{} already exists and is never overwritten: add this under its `mirrors:` / `configs:` yourself:\n{yaml}", env.registries_yaml.display())),
        None => p.file = Some((env.registries_yaml.clone(), yaml)),
    };
    match option {
        RegistryOption::None => {
            p.notes.push("No registry: images are imported into k3s (`k3s ctr images import`, needs root) or the cluster pulls from registries it can already reach. Any saved registry setting is cleared.".into());
        }
        RegistryOption::InCluster { node_port } => {
            if env.kube.is_none() {
                return Err("an in-cluster registry needs a connected cluster (kubectl); choose the cluster first".into());
            }
            let port = choose_port(*node_port, &env.facts)?;
            let host = if loopback(&env.api_host) { "localhost".to_string() } else { env.api_host.clone() };
            p.registry = format!("{host}:{port}");
            p.pull = format!("localhost:{port}");
            let size = if env.facts.size_gi == 0 { DEFAULT_SIZE_GI } else { env.facts.size_gi };
            p.manifest = Some(in_cluster_manifest_sized(port, env.facts.storage_class.as_deref(), size));
            match &env.facts.storage_class {
                Some(c) => p.notes.push(format!("The images live on a {size}Gi PersistentVolumeClaim (StorageClass {c}), so they survive pod restarts. With the local-path class the data sits on the node that ran the pod.")),
                None => p.notes.push("The cluster has no default StorageClass and no local-path class, so the registry uses an emptyDir: pushed images are lost when its pod restarts (push again with deploy_load).".into()),
            }
            if env.facts.existing_port.is_some() {
                p.notes.push(format!("The registry already exists in {IN_NS}; applying again keeps its NodePort and only updates the objects."));
            } else if *node_port == 0 && port != DEFAULT_NODE_PORT {
                p.notes.push(format!("NodePort {DEFAULT_NODE_PORT} is taken by another Service, so {port} is used."));
            }
            p.notes.push(format!("Pods pull `{}/<namespace>/<image>:<tag>`: containerd allows plain HTTP for localhost and a NodePort is reachable on every Ready node at localhost, so no node needs a registries.yaml or a restart. This works where kube-proxy exposes NodePorts on localhost (the default). Pushes from this machine go to {}.", p.pull, p.registry));
            p.notes.push("The registry:2 image is pulled once from Docker Hub by the cluster when the pod first starts.".into());
        }
        RegistryOption::Local => {
            let engine = env.engine.clone().ok_or("a system registry needs podman or docker, and neither is installed")?;
            p.registry = LOCAL_REGISTRY.into();
            want(LOCAL_REGISTRY, http_mirror_yaml(LOCAL_REGISTRY), &mut p);
            let argv = [engine.as_str(), "run", "-d", "--restart=always", "--name", LOCAL_NAME, "-p", "127.0.0.1:5000:5000", "-v", &format!("{LOCAL_VOLUME}:/var/lib/registry"), "docker.io/library/registry:2"];
            p.container = Some(argv.iter().map(|s| s.to_string()).collect());
            p.notes.push("The registry:2 image is pulled once from Docker Hub when the container is first created. --restart=always brings it back with the engine; with rootless podman it also needs the user's podman restart service (or `loginctl enable-linger`) to start at boot.".into());
            p.notes.push("Only a registry on THIS machine fits: the cluster's node must be this machine, because the node pulls localhost:5000.".into());
        }
        RegistryOption::Remote { hostport, https, insecure } => {
            if !valid_hostport(hostport) {
                return Err(format!("`{hostport}` is not a registry address (host:port)"));
            }
            p.registry = hostport.clone();
            match (https, insecure) {
                (false, _) if hostport.starts_with("localhost:") => p.notes.push(format!("{hostport} is a localhost address: containerd allows plain HTTP there, so no mirror file is needed.")),
                (false, _) => want(hostport, http_mirror_yaml(hostport), &mut p),
                (true, true) => want(hostport, insecure_tls_yaml(hostport), &mut p),
                (true, false) => p.notes.push(format!("{hostport} is HTTPS with a certificate the nodes trust: no mirror file is needed.")),
            }
            if p.file.is_some() {
                p.notes.push("If k3s is already running, restart it (below) so it reads the file; every node that pulls needs the same file. If the registry is a NodePort Service of this cluster, the in-cluster option needs none of this.".into());
            }
        }
    }
    Ok(p)
}

fn unsure(p: &SetupPlan) -> bool {
    p.file.is_some() && !p.env.is_root
}

/// One plain line: does this plan need root, and for what.
pub fn root_line(p: &SetupPlan) -> String {
    match (&p.file, p.env.is_root) {
        (Some((path, _)), true) => format!("Root: IYAKKI is running as root and will write {}.", path.display()),
        (Some((path, _)), false) => format!("Root: needed to write {} on each node; IYAKKI never uses sudo, so the commands are for you to run.", path.display()),
        (None, _) => "Root: not needed.".to_string(),
    }
}

fn kubectl_line(k: &KubeCtx, args: &str) -> String {
    let mut s = String::from("kubectl");
    if !k.kubeconfig.is_empty() {
        s.push_str(&format!(" --kubeconfig {}", k.kubeconfig));
    }
    if !k.context.is_empty() {
        s.push_str(&format!(" --context {}", k.context));
    }
    format!("{s} {args}")
}

/// The exact files and commands, for the approval and for `cluster_setup_plan`.
pub fn preview(p: &SetupPlan) -> String {
    let mut s = String::new();
    s.push_str(&match &p.option {
        RegistryOption::None => "Registry option: none\n".to_string(),
        RegistryOption::InCluster { .. } => format!("Registry option: A, a registry pod inside the cluster (namespace {IN_NS}, NodePort {})\n", p.registry.rsplit(':').next().unwrap_or("")),
        RegistryOption::Local => format!("Registry option: B, the system registry on {LOCAL_REGISTRY} (a registry:2 container with the data volume {LOCAL_VOLUME})\n"),
        RegistryOption::Remote { hostport, https, insecure } => format!("Registry option: C, existing registry {hostport} over {}{}\n", if *https { "https" } else { "http" }, if *insecure { " (insecure)" } else { "" }),
    });
    s.push_str(&format!("{}\n", root_line(p)));
    if let (Some(m), Some(k)) = (&p.manifest, &p.env.kube) {
        s.push_str(&format!("\nThe namespace {IN_NS} is a cluster-scoped object, which the manifest check refuses in a workspace manifest, so it gets its own step that needs your approval (this approval):\n{}\n", kubectl_line(k, &format!("create namespace {IN_NS}"))));
        s.push_str(&format!("\nThen the namespaced objects (checked like any manifest), applied with:\n{}\n{}\n\n{m}", kubectl_line(k, &format!("apply -f .iyakki/deploy/iyakki-registry.yaml -n {IN_NS}")), kubectl_line(k, &format!("rollout status deployment/{IN_NAME} -n {IN_NS} --timeout=120s"))));
    }
    if let Some((path, yaml)) = &p.file {
        s.push_str(&format!("\nFile {} (written BEFORE k3s first starts; k3s must be restarted if it already runs):\n{yaml}", path.display()));
        if unsure(p) {
            s.push_str(&format!("\nYou are not root, so IYAKKI will not write it. Run:\n{}\n{RESTART}\n", tee_commands(yaml)));
        }
    }
    if let Some(c) = &p.container {
        s.push_str(&format!("\nRegistry container (no root; started with the cluster and kept by --restart=always):\n{}\n", c.join(" ")));
    }
    if !p.registry.is_empty() {
        let pull = if p.pull.is_empty() || p.pull == p.registry { String::new() } else { format!(", k8s.registry_pull = \"{}\" (what the pods pull)", p.pull) };
        s.push_str(&format!("\nRecorded in the workspace's .iyakki/project.json: k8s.registry = \"{}\" (deploy_load pushes there){pull}.\n", p.registry));
    }
    for n in &p.notes {
        s.push_str(&format!("\nNote: {n}\n"));
    }
    s.trim_end().to_string()
}

#[derive(Debug, Default)]
pub struct Applied {
    pub done: Vec<String>,
    /// Commands the person must run (no root was taken).
    pub to_run: Vec<String>,
}

fn kubectl(run: &dyn Runner, k: &KubeCtx, args: &[&str], secs: u64) -> Out {
    let mut argv: Vec<String> = vec!["kubectl".into()];
    if !k.kubeconfig.is_empty() {
        argv.extend(["--kubeconfig".into(), k.kubeconfig.clone()]);
    }
    if !k.context.is_empty() {
        argv.extend(["--context".into(), k.context.clone()]);
    }
    argv.push("--request-timeout=20s".into());
    argv.extend(args.iter().map(|s| s.to_string()));
    let env: Vec<(String, String)> = if k.kubeconfig.is_empty() { vec![] } else { vec![("KUBECONFIG".into(), k.kubeconfig.clone())] };
    run.run(&std::env::temp_dir(), &argv, &env, Duration::from_secs(secs), 256 * 1024, None)
}

/// Read what the in-cluster plan depends on (read-only kubectl).
pub fn probe(run: &dyn Runner, k: &KubeCtx) -> Facts {
    let mut f = Facts::default();
    let sc = kubectl(run, k, &["get", "storageclass", "-o", "json"], 20);
    if sc.ok() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&sc.stdout) {
            let items = j["items"].as_array().cloned().unwrap_or_default();
            let name = |s: &serde_json::Value| s["metadata"]["name"].as_str().map(str::to_string);
            f.storage_class = items
                .iter()
                .find(|s| s["metadata"]["annotations"]["storageclass.kubernetes.io/is-default-class"].as_str() == Some("true"))
                .and_then(name)
                .or_else(|| items.iter().filter_map(name).find(|n| n == "local-path"));
        }
    }
    let sv = kubectl(run, k, &["get", "services", "-A", "-o", "json"], 20);
    if sv.ok() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&sv.stdout) {
            for s in j["items"].as_array().cloned().unwrap_or_default() {
                let ours = s["metadata"]["namespace"].as_str() == Some(IN_NS) && s["metadata"]["name"].as_str() == Some(IN_NAME);
                for port in s["spec"]["ports"].as_array().cloned().unwrap_or_default() {
                    if let Some(n) = port["nodePort"].as_u64().map(|n| n as u16) {
                        if ours {
                            f.existing_port = Some(n);
                        } else {
                            f.taken_ports.push(n);
                        }
                    }
                }
            }
        }
    }
    f
}

/// Do the plan. `approved` must be true (the person saw [`preview`]). Root is never taken: without it the mirror file is returned as
/// commands in `to_run`.
pub fn apply(p: &SetupPlan, run: &dyn Runner, approved: bool) -> Result<Applied, String> {
    if !approved {
        return Err("the registry setup was not approved".into());
    }
    let mut a = Applied::default();
    if let (Some(m), Some(k)) = (&p.manifest, &p.env.kube) {
        let lint = crate::deploylint::lint(m, IN_NS, &|_| false);
        if !lint.ok() {
            return Err(format!("the registry manifest did not pass the manifest check:\n{}", lint.report()));
        }
        let ns = kubectl(run, k, &["get", "namespace", IN_NS, "-o", "name"], 30);
        if !ns.ok() {
            let t = ns.combined();
            if !(t.contains("NotFound") || t.contains("not found")) {
                return Err(format!("could not check namespace {IN_NS}: {}", t.lines().next().unwrap_or("").trim()));
            }
            let c = kubectl(run, k, &["create", "namespace", IN_NS], 30);
            if !c.ok() {
                return Err(format!("could not create namespace {IN_NS}: {}", c.combined().lines().next().unwrap_or("").trim()));
            }
            a.done.push(format!("created namespace {IN_NS}"));
        }
        let dir = p.env.project_root.clone().unwrap_or_else(std::env::temp_dir).join(".iyakki/deploy");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join("iyakki-registry.yaml");
        std::fs::write(&file, &lint.text).map_err(|e| format!("{}: {e}", file.display()))?;
        let f = file.display().to_string();
        let o = kubectl(run, k, &["apply", "-f", &f, "-n", IN_NS], 90);
        if !o.ok() {
            return Err(format!("kubectl apply failed: {}", o.combined().trim()));
        }
        a.done.push(format!("applied the registry objects to {IN_NS}"));
        let r = kubectl(run, k, &["rollout", "status", &format!("deployment/{IN_NAME}"), "-n", IN_NS, "--timeout=120s"], 150);
        if r.ok() {
            a.done.push("the registry pod is running".into());
        } else {
            a.done.push(format!("the registry pod is not ready yet ({}); check with k8s_get pods namespace={IN_NS}", r.combined().lines().next().unwrap_or("").trim()));
        }
        if run.has("curl") {
            let up = run.run(&std::env::temp_dir(), &["curl".into(), "-fsS".into(), "--max-time".into(), "4".into(), format!("http://{}/v2/", p.registry)], &[], Duration::from_secs(8), 4096, None).ok();
            a.done.push(if up { format!("{} answers GET /v2/", p.registry) } else { format!("{} did not answer GET /v2/ from this machine yet (the pod may still be starting, or the NodePort is not reachable here)", p.registry) });
        }
    }
    if let Some((path, yaml)) = &p.file {
        if p.env.is_root {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(path, yaml).map_err(|e| format!("{}: {e}", path.display()))?;
            a.done.push(format!("wrote {}", path.display()));
        } else {
            a.to_run.push(tee_commands(yaml));
            a.to_run.push(RESTART.to_string());
        }
    }
    if let Some(argv) = &p.container {
        let engine = &argv[0];
        let cwd = std::env::temp_dir();
        let t = Duration::from_secs(600);
        let exists = run.run(&cwd, &[engine.clone(), "inspect".into(), LOCAL_NAME.into()], &[], Duration::from_secs(20), 4096, None).ok();
        let (what, args) = if exists { ("started the existing registry container", vec![engine.clone(), "start".into(), LOCAL_NAME.into()]) } else { ("created the registry container", argv.clone()) };
        let o = run.run(&cwd, &args, &[], t, 16 * 1024, None);
        if !o.ok() {
            return Err(format!("{} failed: {}", args.join(" "), o.combined().trim()));
        }
        a.done.push(what.into());
    }
    if let Some(root) = &p.env.project_root {
        if !p.registry.is_empty() || p.option == RegistryOption::None {
            let mut c = crate::workspace::ProjectConfig::load(root)?;
            c.k8s.registry = p.registry.clone();
            c.k8s.registry_pull = if p.pull == p.registry { String::new() } else { p.pull.clone() };
            c.k8s.registry_kind = p.option.kind().to_string();
            c.save(root)?;
            a.done.push(if p.registry.is_empty() { "cleared the registry setting in .iyakki/project.json".to_string() } else { format!("recorded k8s.registry = {} in .iyakki/project.json", p.registry) });
        }
    }
    Ok(a)
}

// ------------------------------------------------------------------------------------------------------------------ status

/// What the K8S panel's Registry strip shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegStatus {
    /// `none`, `in-cluster`, `system` or `remote`.
    pub kind: String,
    pub push: String,
    pub pull: String,
    /// `GET /v2/` from this machine; None: not asked (no registry, or no curl).
    pub reachable: Option<bool>,
}

impl RegStatus {
    /// From the saved choice alone (no network).
    pub fn saved(c: &crate::workspace::K8sConfig) -> RegStatus {
        let push = c.registry.trim().to_string();
        let kind = if !c.registry_kind.is_empty() {
            c.registry_kind.clone()
        } else if push.is_empty() {
            "none".into()
        } else if push == LOCAL_REGISTRY {
            "system".into()
        } else {
            "remote".into()
        };
        let pull = if c.registry_pull.trim().is_empty() { push.clone() } else { c.registry_pull.trim().to_string() };
        RegStatus { kind, push, pull, reachable: None }
    }

    pub fn chip(&self) -> String {
        let down = if self.reachable == Some(false) { " (not answering)" } else { "" };
        match self.kind.as_str() {
            "none" => "registry: none".into(),
            "in-cluster" if self.push.is_empty() => "registry: in-cluster (not set up yet)".into(),
            "in-cluster" => format!("registry: in-cluster at {}{down}", self.pull),
            "system" => format!("registry: system at {}{down}", self.push),
            _ => format!("registry: remote {}{down}", self.push),
        }
    }

    /// Green when usable: no registry is a plain state, not a fault.
    pub fn ok(&self) -> bool {
        self.kind == "none" || !(self.kind == "in-cluster" && self.push.is_empty()) && self.reachable != Some(false)
    }
}

/// The saved choice plus `GET /v2/` from this machine (http, then https without verification).
pub fn status(c: &crate::workspace::K8sConfig, run: &dyn Runner) -> RegStatus {
    let mut s = RegStatus::saved(c);
    if s.push.is_empty() || !run.has("curl") {
        return s;
    }
    let get = |scheme: &str, k: bool| {
        let mut argv: Vec<String> = vec!["curl".into(), "-fsS".into(), "--max-time".into(), "3".into()];
        if k {
            argv.push("-k".into());
        }
        argv.push(format!("{scheme}://{}/v2/", s.push));
        run.run(&std::env::temp_dir(), &argv, &[], Duration::from_secs(6), 4096, None).ok()
    };
    s.reachable = Some(get("http", false) || get("https", true));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsexec::FakeRunner;

    const ERR: &str = "Failed to pull image \"192.168.1.68:30500/shop-dev/web:abc\": failed to do request: Head \"https://192.168.1.68:30500/v2/shop-dev/web/manifests/abc\": http: server gave HTTP response to HTTPS client";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-regsetup-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env(d: &Path, root: bool) -> PlanEnv {
        PlanEnv { is_root: root, engine: Some("podman".into()), registries_yaml: d.join("etc/registries.yaml"), project_root: Some(d.join("ws")), ..Default::default() }
    }

    fn kube_env(d: &Path, root: bool, host: &str, facts: Facts) -> PlanEnv {
        PlanEnv { kube: Some(KubeCtx { kubeconfig: "/tmp/k.yaml".into(), context: "default".into() }), api_host: host.into(), facts, ..env(d, root) }
    }

    fn free() -> Facts {
        Facts { storage_class: Some("local-path".into()), ..Default::default() }
    }

    #[test]
    fn the_http_to_https_error_gives_the_cause_the_yaml_and_the_commands_for_every_node() {
        let t = diagnose(ERR, "").unwrap();
        assert!(t.contains("plain HTTP") && t.contains("192.168.1.68:30500"));
        assert!(t.contains("mirrors:\n  \"192.168.1.68:30500\":\n    endpoint:\n      - \"http://192.168.1.68:30500\""), "{t}");
        assert!(t.contains("kubectl set image") && t.contains("localhost:30500/<namespace>") && t.find("kubectl set image") < t.find("registries.yaml"), "{t}");
        assert!(t.contains("/etc/rancher/k3s/registries.yaml") && t.contains("sudo systemctl restart k3s ") && t.contains("k3s-agent") && t.contains("EACH node"), "{t}");
        assert!(t.contains("hostname the nodes already trust"), "{t}");
        assert!(!t.to_lowercase().contains("docker hub") && !t.contains("docker.io"), "never Docker Hub: {t}");
    }

    #[test]
    fn x509_auth_and_refused_errors_are_recognised_and_other_text_is_not() {
        let x = diagnose("failed to pull \"10.0.0.5:5000/a:1\": tls: failed to verify certificate: x509: certificate signed by unknown authority from https://10.0.0.5:5000/v2/", "").unwrap();
        assert!(x.contains("x509") && x.contains("insecure_skip_verify") && x.contains("10.0.0.5:5000"), "{x}");
        let a = diagnose("pull access denied ... failed to authorize: no basic auth credentials", "reg.lan:5000").unwrap();
        assert!(a.contains("reg.lan:5000") && a.contains("imagePullSecrets") && !a.contains("password: hunter"), "{a}");
        let c = diagnose("failed to pull image: dial tcp 10.0.0.9:30500: connect: connection refused", "").unwrap();
        assert!(c.contains("10.0.0.9:30500") && c.contains("curl"), "{c}");
        assert!(diagnose("back-off restarting failed container", "x:1").is_none());
        assert!(diagnose("dial tcp 10.0.0.9:30500: connection refused", "").is_none(), "a refused connection that is not a pull is not blamed on a registry");
    }

    #[test]
    fn the_local_option_plans_the_mirror_the_container_and_the_setting_and_never_takes_root() {
        let d = tmp("local");
        let p = plan(&RegistryOption::Local, &env(&d, false)).unwrap();
        let v = preview(&p);
        assert!(v.contains("localhost:5000") && v.contains("podman run -d --restart=always --name iyakki-registry") && v.contains("iyakki-registry:/var/lib/registry"), "{v}");
        assert!(v.contains("not root") && v.contains("sudo tee /etc/rancher/k3s/registries.yaml"), "{v}");
        let r = FakeRunner::new(&["podman"]);
        r.fail("inspect", 1, "no such container");
        r.answer("run -d", "abc");
        std::fs::create_dir_all(d.join("ws")).unwrap();
        assert!(apply(&p, &r, false).is_err(), "no approval, no action");
        assert!(r.calls().is_empty());
        let a = apply(&p, &r, true).unwrap();
        assert!(!d.join("etc/registries.yaml").exists(), "not root: nothing written");
        assert!(a.to_run.iter().any(|c| c.contains("sudo tee")) && a.to_run.iter().any(|c| c.contains("k3s-agent")));
        assert!(r.lines().iter().any(|l| l.starts_with("podman run -d")));
        assert_eq!(crate::workspace::ProjectConfig::load(&d.join("ws")).unwrap().k8s.registry, "localhost:5000");
    }

    #[test]
    fn as_root_the_file_is_written_before_anything_starts_and_an_existing_one_is_never_overwritten() {
        let d = tmp("root");
        let e = env(&d, true);
        let p = plan(&RegistryOption::parse("remote", "http://10.0.0.5:5000/", "http", false).unwrap(), &e).unwrap();
        let r = FakeRunner::new(&[]);
        let a = apply(&p, &r, true).unwrap();
        assert!(a.to_run.is_empty() && r.calls().is_empty());
        let t = std::fs::read_to_string(&e.registries_yaml).unwrap();
        assert_eq!(t, http_mirror_yaml("10.0.0.5:5000"));
        assert_eq!(crate::workspace::ProjectConfig::load(&d.join("ws")).unwrap().k8s.registry, "10.0.0.5:5000");
        // second plan: already there, and a different registry is not merged in silently
        let again = plan(&RegistryOption::Local, &e).unwrap();
        assert!(again.file.is_none() && preview(&again).contains("never overwritten"));
        let same = plan(&RegistryOption::parse("remote", "10.0.0.5:5000", "http", false).unwrap(), &e).unwrap();
        assert!(same.file.is_none() && preview(&same).contains("already has an entry"));
        assert_eq!(std::fs::read_to_string(&e.registries_yaml).unwrap(), t);
    }

    #[test]
    fn existing_container_is_started_and_options_are_validated() {
        let d = tmp("exists");
        let p = plan(&RegistryOption::Local, &env(&d, true)).unwrap();
        let r = FakeRunner::new(&["podman"]);
        r.answer("inspect", "{}");
        r.answer("start iyakki-registry", "iyakki-registry");
        let a = apply(&p, &r, true).unwrap();
        assert!(a.done.iter().any(|x| x.contains("started the existing")) && !r.lines().iter().any(|l| l.contains("run -d")));
        let mut no_engine = env(&d, true);
        no_engine.engine = None;
        assert!(plan(&RegistryOption::Local, &no_engine).unwrap_err().contains("podman or docker"));
        assert!(RegistryOption::parse("remote", "nonsense", "http", false).is_err());
        assert!(RegistryOption::parse("weird", "", "", false).is_err());
        assert_eq!(RegistryOption::parse("none", "", "", false).unwrap(), RegistryOption::None);
        let https = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", false).unwrap(), &env(&tmp("https"), true)).unwrap();
        assert!(https.file.is_none(), "a trusted https registry needs no mirror");
        let insecure = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", true).unwrap(), &env(&tmp("https2"), true)).unwrap();
        assert!(preview(&insecure).contains("insecure_skip_verify"));
    }

    #[test]
    fn the_in_cluster_plan_is_namespaced_objects_that_pass_the_manifest_check_and_needs_no_root() {
        let d = tmp("in-plan");
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", free())).unwrap();
        assert_eq!((p.registry.as_str(), p.pull.as_str()), ("localhost:30500", "localhost:30500"));
        let m = p.manifest.clone().unwrap();
        assert!(m.contains("kind: PersistentVolumeClaim") && m.contains("storageClassName: local-path") && m.contains("nodePort: 30500") && m.contains("type: NodePort") && m.contains("docker.io/library/registry:2"), "{m}");
        assert!(!m.contains("kind: Namespace") && !m.contains("hostPath") && !m.contains("emptyDir"), "{m}");
        let lint = crate::deploylint::lint(&m, IN_NS, &|_| false);
        assert!(lint.ok(), "{}", lint.report());
        let v = preview(&p);
        assert!(v.contains("create namespace iyakki-registry") && v.contains("cluster-scoped") && v.contains("Root: not needed") && v.contains("k8s.registry = \"localhost:30500\""), "{v}");
        assert!(v.contains("localhost:30500/<namespace>/<image>:<tag>") && v.contains("no node needs a registries.yaml"), "{v}");
        assert!(p.file.is_none() && p.container.is_none(), "no node file, no container, so nothing for root");
        // a remote cluster: pushes go to the API server's host, pods still pull localhost
        let r = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "192.168.1.68", free())).unwrap();
        assert_eq!((r.registry.as_str(), r.pull.as_str()), ("192.168.1.68:30500", "localhost:30500"));
        assert!(preview(&r).contains("k8s.registry_pull = \"localhost:30500\""));
    }

    #[test]
    fn the_in_cluster_plan_picks_a_free_port_falls_back_to_emptydir_and_validates() {
        let d = tmp("in-port");
        let busy = Facts { taken_ports: vec![30500, 30501], ..Default::default() };
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", busy.clone())).unwrap();
        assert_eq!(p.registry, "localhost:30502");
        let m = p.manifest.unwrap();
        assert!(m.contains("emptyDir: {}") && !m.contains("PersistentVolumeClaim") && m.contains("nodePort: 30502"), "{m}");
        assert!(preview(&plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", busy.clone())).unwrap()).contains("no default StorageClass"));
        assert!(plan(&RegistryOption::InCluster { node_port: 30501 }, &kube_env(&d, false, "", busy)).unwrap_err().contains("already used"));
        let again = Facts { existing_port: Some(30777), taken_ports: vec![30500], ..free() };
        assert_eq!(plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", again)).unwrap().registry, "localhost:30777", "re-running keeps our own port");
        assert!(plan(&RegistryOption::InCluster { node_port: 0 }, &env(&d, false)).unwrap_err().contains("connected cluster"));
        assert_eq!(RegistryOption::parse("in-cluster", "31000", "", false).unwrap(), RegistryOption::InCluster { node_port: 31000 });
        assert!(RegistryOption::parse("in-cluster", "80", "", false).is_err());
        assert!(RegistryOption::parse("in-cluster", "abc", "", false).is_err());
        assert_eq!(RegistryOption::parse("system", "", "", false).unwrap(), RegistryOption::Local);
    }

    #[test]
    fn applying_the_in_cluster_option_creates_the_namespace_once_applies_waits_and_records_both_addresses() {
        let d = tmp("in-apply");
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "192.168.1.68", free())).unwrap();
        let r = FakeRunner::new(&["kubectl", "curl"]);
        r.fail("get namespace iyakki-registry", 1, "Error from server (NotFound): namespaces \"iyakki-registry\" not found");
        r.answer("create namespace iyakki-registry", "namespace/iyakki-registry created");
        r.answer("apply -f", "deployment.apps/registry created\nservice/registry created");
        r.answer("rollout status", "successfully rolled out");
        r.answer("/v2/", "{}");
        assert!(apply(&p, &r, false).is_err());
        assert!(r.calls().is_empty(), "no approval, no kubectl");
        let a = apply(&p, &r, true).unwrap();
        let lines = r.lines();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} in {lines:?}"));
        assert!(pos("create namespace") < pos("apply -f") && pos("apply -f") < pos("rollout status") && pos("rollout status") < pos("/v2/"));
        assert!(lines.iter().all(|l| !l.contains("sudo") && !l.contains("tee ")), "{lines:?}");
        assert!(lines[pos("apply -f")].contains("-n iyakki-registry") && lines[pos("apply -f")].contains("--kubeconfig /tmp/k.yaml"));
        assert!(a.to_run.is_empty() && a.done.iter().any(|x| x.contains("created namespace")) && a.done.iter().any(|x| x.contains("answers GET /v2/")), "{a:?}");
        let ws = d.join("ws");
        assert!(ws.join(".iyakki/deploy/iyakki-registry.yaml").is_file());
        let c = crate::workspace::ProjectConfig::load(&ws).unwrap();
        assert_eq!((c.k8s.registry.as_str(), c.k8s.registry_pull.as_str(), c.k8s.registry_kind.as_str()), ("192.168.1.68:30500", "localhost:30500", "in-cluster"));
        // a second run finds the namespace: it is not created again
        let r2 = FakeRunner::new(&["kubectl"]);
        r2.answer("get namespace iyakki-registry", "namespace/iyakki-registry");
        r2.answer("apply -f", "unchanged");
        r2.answer("rollout status", "ok");
        apply(&p, &r2, true).unwrap();
        assert!(!r2.lines().iter().any(|l| l.contains("create namespace")));
        // a failing apply stops with kubectl's words and records nothing new
        let d2 = tmp("in-fail");
        let p2 = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d2, false, "", free())).unwrap();
        let r3 = FakeRunner::new(&["kubectl"]);
        r3.answer("get namespace iyakki-registry", "namespace/iyakki-registry");
        r3.fail("apply -f", 1, "error: forbidden");
        assert!(apply(&p2, &r3, true).unwrap_err().contains("forbidden"));
        assert!(!d2.join("ws/.iyakki/project.json").exists());
        let r4 = FakeRunner::new(&["kubectl"]);
        r4.fail("get namespace", 1, "Unable to connect to the server");
        assert!(apply(&p2, &r4, true).unwrap_err().contains("could not check namespace"));
    }

    #[test]
    fn probing_reads_the_default_storage_class_and_the_nodeports_in_use() {
        let r = FakeRunner::new(&["kubectl"]);
        r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"fast"}},{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#);
        r.answer("get services", r#"{"items":[{"metadata":{"name":"web","namespace":"a"},"spec":{"ports":[{"nodePort":30500}]}},{"metadata":{"name":"registry","namespace":"iyakki-registry"},"spec":{"ports":[{"nodePort":30600}]}}]}"#);
        let f = probe(&r, &KubeCtx::default());
        assert_eq!((f.storage_class.as_deref(), f.taken_ports.clone(), f.existing_port), (Some("local-path"), vec![30500], Some(30600)));
        let r2 = FakeRunner::new(&["kubectl"]);
        r2.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path"}}]}"#);
        r2.fail("get services", 1, "forbidden");
        let f2 = probe(&r2, &KubeCtx::default());
        assert_eq!(f2.storage_class.as_deref(), Some("local-path"), "local-path without being the default is still used");
        assert!(f2.taken_ports.is_empty());
        assert_eq!(probe(&FakeRunner::new(&["kubectl"]), &KubeCtx::default()).storage_class, None);
    }

    #[test]
    fn root_is_said_plainly_for_each_option_and_none_clears_the_setting() {
        let d = tmp("root-lines");
        let a = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", free())).unwrap();
        assert_eq!(root_line(&a), "Root: not needed.");
        let b = plan(&RegistryOption::Local, &env(&d, false)).unwrap();
        assert!(root_line(&b).contains("needed to write") && root_line(&b).contains("never uses sudo"), "{}", root_line(&b));
        let c_http = plan(&RegistryOption::parse("remote", "10.0.0.5:5000", "http", false).unwrap(), &env(&d, false)).unwrap();
        assert!(root_line(&c_http).contains("needed"));
        let c_tls = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", false).unwrap(), &env(&d, false)).unwrap();
        assert_eq!(root_line(&c_tls), "Root: not needed.");
        let c_local = plan(&RegistryOption::parse("remote", "localhost:31000", "http", false).unwrap(), &env(&d, false)).unwrap();
        assert_eq!(root_line(&c_local), "Root: not needed.", "localhost over http needs no mirror");
        let root = plan(&RegistryOption::Local, &env(&tmp("root-lines2"), true)).unwrap();
        assert!(root_line(&root).contains("running as root"));
        // none clears what an earlier choice saved
        let ws = d.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let mut cfg = crate::workspace::ProjectConfig::load(&ws).unwrap();
        cfg.k8s.registry = "localhost:30500".into();
        cfg.k8s.registry_pull = "localhost:30500".into();
        cfg.k8s.registry_kind = "in-cluster".into();
        cfg.save(&ws).unwrap();
        apply(&plan(&RegistryOption::None, &env(&d, false)).unwrap(), &FakeRunner::new(&[]), true).unwrap();
        let c = crate::workspace::ProjectConfig::load(&ws).unwrap();
        assert_eq!((c.k8s.registry.as_str(), c.k8s.registry_pull.as_str(), c.k8s.registry_kind.as_str()), ("", "", "none"));
    }

    #[test]
    fn the_status_chip_says_what_is_saved_and_whether_it_answers() {
        let mut c = crate::workspace::K8sConfig::default();
        assert_eq!(RegStatus::saved(&c).chip(), "registry: none");
        c.registry = "localhost:5000".into();
        assert_eq!(RegStatus::saved(&c).kind, "system");
        c.registry = "10.0.0.5:5000".into();
        assert_eq!(RegStatus::saved(&c).kind, "remote");
        c.registry = "192.168.1.68:30500".into();
        c.registry_pull = "localhost:30500".into();
        c.registry_kind = "in-cluster".into();
        let up = FakeRunner::new(&["curl"]);
        up.answer("http://192.168.1.68:30500/v2/", "{}");
        let s = status(&c, &up);
        assert_eq!((s.chip().as_str(), s.ok(), s.reachable), ("registry: in-cluster at localhost:30500", true, Some(true)));
        let down = FakeRunner::new(&["curl"]);
        let s = status(&c, &down);
        assert_eq!((s.chip().as_str(), s.ok()), ("registry: in-cluster at localhost:30500 (not answering)", false));
        assert_eq!(status(&c, &FakeRunner::new(&[])).reachable, None, "no curl: not asked");
    }
}
