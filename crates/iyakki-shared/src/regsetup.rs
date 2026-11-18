//! Types moved out of `iyakki_agent::regsetup` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The local registry container.
pub const LOCAL_REGISTRY: &str = "localhost:5000";

/// The three choices of the setup dialog, best first for a cluster without root: (option word, name, one-line pros, root needs).
pub const CHOICES: [(&str, &str, &str, &str); 3] = [
    ("in-cluster", "A  Registry pod inside the cluster (recommended)", "No root and no node changes: pods pull localhost:<nodePort>; works for a local or a remote k3s.", "no root"),
    ("system", "B  System registry (container on this machine)", "Best for one local node: a registry:2 container with podman/docker on localhost:5000.", "root only to write the node's registries.yaml (commands given)"),
    ("remote", "C  An existing registry", "Use a registry you already have (host:port); nothing is installed.", "root only for plain-HTTP or untrusted-TLS registries (commands given)"),
];



/// What the K8S panel's Registry strip shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegStatus {
    /// `none`, `in-cluster`, `system` or `remote`.
    pub kind: String,
    pub push: String,
    pub pull: String,
    /// `GET /v2/` from this machine; None: not asked (no registry).
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

/// The namespace, Service name and default NodePort of the registry that runs inside the cluster.
pub const IN_NS: &str = "iyakki-registry";
pub const IN_NAME: &str = "registry";

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use std::path::Path;
use std::path::PathBuf;

pub const LOCAL_NAME: &str = "iyakki-registry";

/// The registry image, by tag AND digest: CNCF Distribution 2.8.3 (the tag `2` pointed at it on 2026-10-27), the OCI index (amd64 and
/// arm64) read from Docker Hub's registry API. Change the tag and the digest here only, and verify the digest against the registry.
pub const REGISTRY_IMAGE: &str = "docker.io/library/registry:2.8.3@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373";

pub const LOCAL_VOLUME: &str = "iyakki-registry";

pub const YAML_PATH: &str = "/etc/rancher/k3s/registries.yaml";

pub fn valid_hostport(s: &str) -> bool {
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

pub const RESTART: &str = "sudo systemctl restart k3s          # on the server node\nsudo systemctl restart k3s-agent    # on every agent node";

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

pub const DEFAULT_NODE_PORT: u16 = 30500;

pub const NODE_PORTS: std::ops::RangeInclusive<u16> = 30500..=32767;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryOption {
    None,
    /// A `registry:2` Deployment inside the cluster (namespace `iyakki-registry`, a PVC, a ClusterIP Service, a hostPort bound to 127.0.0.1). `node_port` 0: 30500 or the
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
    /// Our Service is still a NodePort Service (an older install): it is open to the network and must be converted.
    pub legacy_exposed: bool,
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

pub fn loopback(h: &str) -> bool {
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

/// The registry's Deployment, PVC (or `emptyDir`) and ClusterIP Service. The registry is published only as a hostPort bound to
/// 127.0.0.1 of the node (the same pattern as the Gitea loopback port): pods and the node pull `localhost:<port>`, this machine
/// pushes to `localhost:<port>`, and no other computer can reach it (it has no sign-in). No namespace in the objects: it is given with `-n`.
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
        "{pvc}apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {IN_NAME}\n  labels:\n    app: {IN_NAME}\nspec:\n  replicas: 1\n  strategy:\n    type: Recreate\n  selector:\n    matchLabels:\n      app: {IN_NAME}\n  template:\n    metadata:\n      labels:\n        app: {IN_NAME}\n    spec:\n      containers:\n      - name: {IN_NAME}\n        image: {REGISTRY_IMAGE}\n        ports:\n        - containerPort: 5000\n          hostPort: {node_port}\n          hostIP: 127.0.0.1\n        env:\n        - name: REGISTRY_STORAGE_DELETE_ENABLED\n          value: \"true\"\n        volumeMounts:\n        - name: data\n          mountPath: /var/lib/registry\n        resources:\n          requests:\n            cpu: 50m\n            memory: 64Mi\n          limits:\n            cpu: 500m\n            memory: 512Mi\n        securityContext:\n          allowPrivilegeEscalation: false\n      volumes:\n{volume}---\napiVersion: v1\nkind: Service\nmetadata:\n  name: {IN_NAME}\nspec:\n  type: ClusterIP\n  selector:\n    app: {IN_NAME}\n  ports:\n  - port: 5000\n    targetPort: 5000\n"
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
            if !loopback(&env.api_host) {
                return Err(format!("the in-cluster registry has no sign-in, so it is published on 127.0.0.1 of the node only and cannot be pushed to from another computer ({} is not this machine). Use an existing registry (option C, with its own sign-in) for a cluster on another machine", env.api_host));
            }
            let port = choose_port(*node_port, &env.facts)?;
            p.registry = format!("localhost:{port}");
            p.pull = format!("localhost:{port}");
            let size = if env.facts.size_gi == 0 { DEFAULT_SIZE_GI } else { env.facts.size_gi };
            p.manifest = Some(in_cluster_manifest_sized(port, env.facts.storage_class.as_deref(), size));
            match &env.facts.storage_class {
                Some(c) => p.notes.push(format!("The images live on a {size}Gi PersistentVolumeClaim (StorageClass {c}), so they survive pod restarts. With the local-path class the data sits on the node that ran the pod.")),
                None => p.notes.push("The cluster has no default StorageClass and no local-path class, so the registry uses an emptyDir: pushed images are lost when its pod restarts (push again with deploy_load).".into()),
            }
            if env.facts.existing_port.is_some() {
                p.notes.push(format!("The registry already exists in {IN_NS}; applying again keeps its port and only updates the objects."));
                if env.facts.legacy_exposed {
                    p.notes.push(format!("SECURITY: the existing registry is a NodePort Service, open to the whole network without a password. Applying turns the Service into ClusterIP and binds the registry to 127.0.0.1:{port} of the node only; the NodePort closes."));
                }
            } else if *node_port == 0 && port != DEFAULT_NODE_PORT {
                p.notes.push(format!("NodePort {DEFAULT_NODE_PORT} is taken by another Service, so {port} is used."));
            }
            p.notes.push(format!("Pods pull `{}/<namespace>/<image>:<tag>`: containerd allows plain HTTP for localhost and the registry is published as a hostPort on 127.0.0.1 of the node (not a NodePort: no other computer can reach it, there is no sign-in), so no node needs a registries.yaml or a restart. This works on a single-node cluster (the dev cluster); pods on another node cannot pull it. Pushes from this machine go to {}.", p.pull, p.registry));
            p.notes.push("The registry:2 image is pulled once from Docker Hub by the cluster when the pod first starts.".into());
        }
        RegistryOption::Local => {
            let engine = env.engine.clone().ok_or("a system registry needs podman or docker, and neither is installed")?;
            p.registry = LOCAL_REGISTRY.into();
            want(LOCAL_REGISTRY, http_mirror_yaml(LOCAL_REGISTRY), &mut p);
            let argv = [engine.as_str(), "run", "-d", "--restart=always", "--name", LOCAL_NAME, "-p", "127.0.0.1:5000:5000", "-v", &format!("{LOCAL_VOLUME}:/var/lib/registry"), REGISTRY_IMAGE];
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

/// One plain line: does this plan need root, and for what.
pub fn root_line(p: &SetupPlan) -> String {
    match (&p.file, p.env.is_root) {
        (Some((path, _)), true) => format!("Root: IYAKKI is running as root and will write {}.", path.display()),
        (Some((path, _)), false) => format!("Root: needed to write {} on each node; IYAKKI never uses sudo, so the commands are for you to run.", path.display()),
        (None, _) => "Root: not needed.".to_string(),
    }
}

/// The exact files and commands, for the approval and for `cluster_setup_plan`.
pub fn preview(p: &SetupPlan) -> String {
    let mut s = String::new();
    s.push_str(&match &p.option {
        RegistryOption::None => "Registry option: none\n".to_string(),
        RegistryOption::InCluster { .. } => format!("Registry option: A, a registry pod inside the cluster (namespace {IN_NS}, loopback port {})\n", p.registry.rsplit(':').next().unwrap_or("")),
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

/// The loopback hostPort of the registry container in a Deployment's JSON.
pub fn hostport_of(deployment: &serde_json::Value) -> Option<u16> {
    deployment["spec"]["template"]["spec"]["containers"].as_array()?.iter().flat_map(|c| c["ports"].as_array().cloned().unwrap_or_default()).find_map(|p| p["hostPort"].as_u64()).map(|n| n as u16)
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub fn tee_commands(yaml: &str) -> String {
    format!("sudo mkdir -p /etc/rancher/k3s\nsudo tee {YAML_PATH} >/dev/null <<'EOF'\n{yaml}EOF")
}

pub fn unsure(p: &SetupPlan) -> bool {
    p.file.is_some() && !p.env.is_root
}

pub fn kubectl_line(k: &KubeCtx, args: &str) -> String {
    let mut s = String::from("kubectl");
    if !k.kubeconfig.is_empty() {
        s.push_str(&format!(" --kubeconfig {}", k.kubeconfig));
    }
    if !k.context.is_empty() {
        s.push_str(&format!(" --context {}", k.context));
    }
    format!("{s} {args}")
}
