//! Platform services (docs/PLATFORM-SERVICES.md): services IYAKKI deploys into a cluster the person picked (Gitea, a private store, ...),
//! through the same executor, manifest linter and namespace guards as the dev-cluster tools, with a plan the person approves first.
//!
//! * [`ServiceDef`] describes one service: pinned images, an options schema, the manifest builder, secrets to generate, health, URLs,
//!   upgrade path, backup. [`register`] adds a service (the private store registers itself this way); [`specs`] lists them.
//! * [`engine`] holds `plan` / `preview` / `apply` / `status` / `stop` / `start` / `upgrade` / `remove` / `backup`. Nothing runs without
//!   the approval flag; the cluster is a profile the person selected (hidden, unreachable, production-looking and read-only profiles
//!   are refused); a namespace is created only as a visible step and carries `app.kubernetes.io/managed-by=iyakki` and
//!   `iyakki.dev/service=<id>`.
//! * [`secrets`]: generated passwords exist only as a Kubernetes Secret and a vault entry (`uvault:svc.<instance>.<key>`); the value is
//!   returned once to the caller that shows the masked reveal, never logged, never in an argument, never given to the model.
//! * [`installed`]: the per-user registry `installed-services-<user>.json`.
//! * [`gitea`]: the Gitea service, detection of an existing Gitea or Forgejo and "connect to existing".
//! * [`connect`]: connect a workspace to a git service (create the repository, set `origin`, push after approval).
//! * [`tools`]: the assistant's tools (`platform_list`, `platform_status`, `platform_deploy`, `platform_upgrade`, `platform_remove`,
//!   `git_service_status`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

pub mod connect;
pub mod engine;
pub mod gitea;
pub mod installed;
pub mod localbin;
pub mod secrets;
pub mod tools;

#[cfg(test)]
mod tests;

pub use crate::k8stools::LABEL_MANAGED;
pub use engine::*;
pub use installed::{ClusterRef, Installed};
pub use secrets::{MemVault, Reveal, VaultApi};
pub use zeroize::Zeroizing;

/// `iyakki.dev/service=<id>`: on the namespace IYAKKI created and on every object of a service.
pub const LABEL_SERVICE: &str = "iyakki.dev/service";
/// The label every object of one install carries, so remove and status find exactly those.
pub const LABEL_INSTANCE: &str = "iyakki.dev/instance";

// ------------------------------------------------------------------------------------------------ exposure

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exposure {
    ClusterIP,
    NodePort,
    LoadBalancer,
    Ingress,
}

impl Exposure {
    pub const ALL: [Exposure; 4] = [Exposure::NodePort, Exposure::ClusterIP, Exposure::LoadBalancer, Exposure::Ingress];

    pub fn parse(s: &str) -> Result<Exposure, String> {
        match s.trim().to_lowercase().as_str() {
            "clusterip" | "cluster-ip" | "cluster_ip" => Ok(Exposure::ClusterIP),
            "" | "nodeport" | "node-port" | "node_port" => Ok(Exposure::NodePort),
            "loadbalancer" | "load-balancer" | "load_balancer" | "lb" => Ok(Exposure::LoadBalancer),
            "ingress" => Ok(Exposure::Ingress),
            o => Err(format!("`{o}` is not an exposure (ClusterIP, NodePort, LoadBalancer or Ingress)")),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Exposure::ClusterIP => "ClusterIP",
            Exposure::NodePort => "NodePort",
            Exposure::LoadBalancer => "LoadBalancer",
            Exposure::Ingress => "Ingress",
        }
    }

    /// What this exposure means for who can reach the service, in plain words.
    pub fn explain(self) -> &'static str {
        match self {
            Exposure::ClusterIP => "only pods inside the cluster can reach it (use kubectl port-forward from your machine)",
            Exposure::NodePort => "reachable on every node's address at the chosen ports: anyone who can reach the nodes can reach it",
            Exposure::LoadBalancer => "the cluster's load balancer gives it an external address: possibly reachable from outside your network",
            Exposure::Ingress => "reachable through the cluster's ingress controller at the host name you give",
        }
    }
}

// ------------------------------------------------------------------------------------------------ options

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptKind {
    Text,
    /// `10Gi`, `500Mi`.
    Size,
    Choice(Vec<String>),
    Bool,
    /// A port number.
    Port,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionDef {
    pub key: String,
    pub label: String,
    pub help: String,
    pub kind: OptKind,
    pub default: String,
}

impl OptionDef {
    pub fn new(key: &str, label: &str, help: &str, kind: OptKind, default: &str) -> OptionDef {
        OptionDef { key: key.into(), label: label.into(), help: help.into(), kind, default: default.into() }
    }
}

/// The values of the options form, by key. Unknown keys are refused by [`validate_options`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options(pub BTreeMap<String, String>);

impl Options {
    pub fn get(&self, k: &str) -> &str {
        self.0.get(k).map(String::as_str).unwrap_or("")
    }
    pub fn set(&mut self, k: &str, v: &str) {
        self.0.insert(k.into(), v.trim().into());
    }
    pub fn flag(&self, k: &str) -> bool {
        matches!(self.get(k), "true" | "yes" | "1" | "on")
    }
    pub fn from_json(v: &serde_json::Value) -> Options {
        let mut o = Options::default();
        if let Some(m) = v.as_object() {
            for (k, x) in m {
                let t = match x {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                o.set(k, &t);
            }
        }
        o
    }
}

/// The options every service has; a service adds its own with [`ServiceDef::options`].
pub fn common_options(def: &dyn ServiceDef) -> Vec<OptionDef> {
    let versions = def.versions();
    let mut v = vec![
        OptionDef::new("namespace", "Namespace", "Where the service lives. A new namespace is created (and labelled as managed by IYAKKI) as a visible step; an existing one must already carry app.kubernetes.io/managed-by=iyakki.", OptKind::Text, &def.default_namespace()),
        OptionDef::new("version", "Version", "A pinned version known to IYAKKI.", OptKind::Choice(versions.clone()), versions.last().map(String::as_str).unwrap_or("")),
        OptionDef::new("storage_size", "Storage size", "Size of the PersistentVolumeClaim.", OptKind::Size, "10Gi"),
        OptionDef::new("storage_class", "Storage class", "Empty: the cluster's default class (or local-path).", OptKind::Text, ""),
        OptionDef::new("exposure", "Exposure", "How it is reachable: NodePort (default), ClusterIP, LoadBalancer or Ingress.", OptKind::Choice(Exposure::ALL.iter().map(|e| e.word().to_string()).collect()), "NodePort"),
        OptionDef::new("ingress_host", "Ingress host", "Only for Ingress: the host name, such as git.example.lan.", OptKind::Text, ""),
        OptionDef::new("admin_user", "Admin user", "The first administrator's login.", OptKind::Text, "iyakki-admin"),
        OptionDef::new("mirror", "Mirror images", "Copy the images through the in-cluster registry (for an air-gapped cluster). Needs the registry set up and docker or podman here.", OptKind::Bool, "false"),
    ];
    v.extend(def.options());
    v
}

pub fn dns_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}

fn dns_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(dns_label)
}

fn size_ok(s: &str) -> bool {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let unit = &s[digits.len()..];
    !digits.is_empty() && digits.len() <= 5 && digits.parse::<u32>().is_ok_and(|n| n > 0) && matches!(unit, "Mi" | "Gi" | "Ti")
}

pub const NODE_PORTS: std::ops::RangeInclusive<u16> = 30000..=32767;

/// Fill the defaults, check every value and refuse anything the form does not know. The result holds every key of the schema.
pub fn validate_options(def: &dyn ServiceDef, given: &Options) -> Result<Options, String> {
    let schema = common_options(def);
    for k in given.0.keys() {
        if !schema.iter().any(|d| &d.key == k) {
            return Err(format!("`{k}` is not an option of {} (known: {})", def.title(), schema.iter().map(|d| d.key.as_str()).collect::<Vec<_>>().join(", ")));
        }
    }
    let mut out = Options::default();
    for d in &schema {
        let v = match given.0.get(&d.key).map(|s| s.trim()) {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => d.default.clone(),
        };
        let bad = |why: &str| Err(format!("{}: {why}", d.label));
        match &d.kind {
            OptKind::Size if !size_ok(&v) => return bad(&format!("`{v}` is not a size like 10Gi or 500Mi")),
            OptKind::Choice(c) if !c.iter().any(|x| x.eq_ignore_ascii_case(&v)) => return bad(&format!("`{v}` is not one of {}", c.join(", "))),
            OptKind::Bool if !matches!(v.as_str(), "true" | "false" | "yes" | "no" | "1" | "0" | "on" | "off") => return bad(&format!("`{v}` is not true or false")),
            OptKind::Port if !v.is_empty() && !v.parse::<u16>().is_ok_and(|p| NODE_PORTS.contains(&p)) => return bad(&format!("`{v}` is not a NodePort (30000 to 32767)")),
            _ => {}
        }
        let v = match &d.kind {
            OptKind::Choice(c) => c.iter().find(|x| x.eq_ignore_ascii_case(&v)).cloned().unwrap_or(v),
            OptKind::Bool => (if matches!(v.as_str(), "true" | "yes" | "1" | "on") { "true" } else { "false" }).to_string(),
            _ => v,
        };
        out.0.insert(d.key.clone(), v);
    }
    let ns = out.get("namespace").to_string();
    if !dns_label(&ns) {
        return Err(format!("Namespace: `{ns}` is not a valid namespace name (lowercase letters, digits and -, up to 63)"));
    }
    if crate::k8stools::protected_namespace(&ns) {
        return Err(format!("Namespace: `{ns}` is a protected namespace (default, kube-* and iyakki-registry); IYAKKI never deploys services there"));
    }
    let sc = out.get("storage_class");
    if !sc.is_empty() && !dns_name(sc) {
        return Err(format!("Storage class: `{sc}` is not a valid class name"));
    }
    let exposure = Exposure::parse(out.get("exposure"))?;
    let host = out.get("ingress_host");
    if exposure == Exposure::Ingress && !dns_name(host) {
        return Err("Ingress host: give the host name the ingress answers to (such as git.example.lan)".into());
    }
    if exposure != Exposure::Ingress && !host.is_empty() {
        return Err("Ingress host: only used with the Ingress exposure; leave it empty".into());
    }
    let user = out.get("admin_user");
    if !(3..=31).contains(&user.len()) || !user.chars().next().is_some_and(|c| c.is_ascii_lowercase()) || !user.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') || matches!(user, "admin" | "root" | "administrator" | "api" | "user") {
        return Err("Admin user: 3 to 31 lowercase letters, digits, - or _, starting with a letter, and not a name services reserve (admin, root, administrator, api, user)".into());
    }
    def.check_options(&out)?;
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ what a service gives the engine

/// One container image a service runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    /// `docker.io/gitea/gitea:1.24.5-rootless`.
    pub image: String,
    /// `sha256:...` when known; the reference then is `image@digest` (the tag stays for people).
    pub digest: Option<String>,
    /// CPU architectures the image is published for (`amd64`, `arm64`).
    pub archs: Vec<&'static str>,
}

impl ImageRef {
    pub fn new(image: &str, archs: &[&'static str]) -> ImageRef {
        ImageRef { image: image.into(), digest: None, archs: archs.to_vec() }
    }
    /// What a manifest names: the digest form when there is a digest.
    pub fn reference(&self) -> String {
        match &self.digest {
            Some(d) => {
                let base = match self.image.rsplit_once(':') {
                    Some((b, t)) if !t.contains('/') => b,
                    _ => self.image.as_str(),
                };
                format!("{base}@{d}")
            }
            None => self.image.clone(),
        }
    }
    /// `gitea/gitea:1.24.5-rootless` without the registry host, for a mirror.
    pub fn repo_and_tag(&self) -> String {
        let s = self.image.strip_prefix("docker.io/").unwrap_or(&self.image);
        s.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    Token,
}

/// One generated value: the key in the Kubernetes Secret, the vault entry name and what the person sees it called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretEntry {
    pub key: String,
    /// The vault entry (`svc.<instance>.<name>`), stored in the person's vault as `uvault:<name>`.
    pub vault: String,
    pub label: String,
    pub kind: SecretKind,
    /// Show it once in the masked reveal after the deploy.
    pub reveal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretSpec {
    pub name: String,
    pub entries: Vec<SecretEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlInfo {
    pub label: String,
    pub url: String,
    /// `web`, `api`, `clone-http`, `clone-ssh`.
    pub kind: String,
}

/// A Deployment the service runs, for waiting, stopping, status and upgrades.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    pub name: String,
    pub container: String,
    /// Which of the service's images it runs (index into [`ServiceDef::images`]).
    pub image: usize,
}

/// Everything [`ServiceDef::build`] may use.
#[derive(Clone, Debug)]
pub struct BuildCtx {
    pub namespace: String,
    pub opts: Options,
    pub version: String,
    /// The image references to put in the manifest (mirrored ones when the images are mirrored), in [`ServiceDef::images`] order.
    pub images: Vec<String>,
    /// The StorageClass of the PVC (None: leave it to the cluster's default).
    pub storage_class: Option<String>,
    /// The address people reach nodes at: `localhost`, or the API server's host.
    pub host: String,
    /// The NodePorts chosen for the HTTP and the SSH Service (0: not used).
    pub http_port: u16,
    pub ssh_port: u16,
    /// `svc.<instance>`: the prefix of every vault entry.
    pub vault_prefix: String,
    /// The instance label value (`gitea-iyakki-gitea`).
    pub instance: String,
}

#[derive(Clone, Debug, Default)]
pub struct Built {
    /// The namespaced objects (no Namespace, no Secret values), block-style YAML for the manifest linter.
    pub manifest: String,
    pub secrets: Vec<SecretSpec>,
    pub urls: Vec<UrlInfo>,
    pub notes: Vec<String>,
}

/// What a probe saw of the cluster, for planning and for detecting an existing service.
#[derive(Clone, Debug, Default)]
pub struct ClusterFacts {
    pub nodes: Vec<NodeFact>,
    pub storage_class: Option<String>,
    pub storage_classes: Vec<String>,
    pub taken_ports: Vec<u16>,
    /// `(namespace, name, images)` of every Deployment and StatefulSet.
    pub workloads: Vec<(String, String, Vec<String>)>,
    /// `(namespace, name, type, nodePorts)` of every Service.
    pub services: Vec<(String, String, String, Vec<u16>)>,
    /// The NodePorts of this install's own Services (found by the instance label), by Service name: kept when it is deployed again.
    pub own_ports: Vec<(String, u16)>,
    /// Running pods in the cluster and the pod capacity of the Ready nodes.
    pub pods: usize,
    pub pod_capacity: usize,
    pub namespace: NamespaceFact,
    /// `host:port` pods pull from, when the in-cluster registry is set up.
    pub registry_pull: Option<String>,
    pub registry_push: Option<String>,
    pub api_host: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeFact {
    pub name: String,
    pub ready: bool,
    pub arch: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NamespaceFact {
    #[default]
    Missing,
    /// Exists and carries `managed-by=iyakki`; the service label (if any).
    Managed { service: Option<String> },
    /// Exists without our label.
    Foreign,
}

/// A service already running in the cluster that does the same job (a Gitea or Forgejo, for Gitea).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Existing {
    pub namespace: String,
    pub name: String,
    pub image: String,
    /// `http://<node>:<nodePort>` when a NodePort Service was found, else empty.
    pub url_hint: String,
    pub product: String,
}

/// One service IYAKKI can deploy.
pub trait ServiceDef: Send + Sync {
    fn id(&self) -> &'static str;
    fn title(&self) -> String;
    fn description(&self) -> String;
    /// `git`, `store`, `registry`, ...
    fn kind(&self) -> &'static str;
    /// Pinned versions, oldest first; the last is the default. Upgrades only go forward in this list.
    fn versions(&self) -> Vec<String>;
    fn images(&self, version: &str) -> Vec<ImageRef>;
    /// Options beyond [`common_options`].
    fn options(&self) -> Vec<OptionDef> {
        vec![]
    }
    /// Extra checks of the validated options.
    fn check_options(&self, _o: &Options) -> Result<(), String> {
        Ok(())
    }
    fn default_namespace(&self) -> String {
        format!("iyakki-{}", self.id())
    }
    fn build(&self, c: &BuildCtx) -> Result<Built, String>;
    fn workloads(&self) -> Vec<Workload>;
    /// A path under the first web URL that answers 200 when the service is healthy.
    fn health_path(&self) -> Option<String> {
        None
    }
    fn how_to_use(&self, urls: &[UrlInfo], o: &Options) -> String;
    fn backup_hint(&self) -> String;
    /// Commands for a backup; None when the service has none.
    fn backup(&self, _w: &Workload) -> Option<BackupPlan> {
        None
    }
    /// What the service exposes, in plain words, for the plan.
    fn exposes(&self, o: &Options) -> Vec<String>;
    fn detect_existing(&self, _f: &ClusterFacts) -> Vec<Existing> {
        vec![]
    }
    /// Steps after the workloads are ready (creating the admin user and the API token). Lines go to the result.
    fn post_deploy(&self, _p: &mut PostCtx<'_>) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    /// A NetworkPolicy to show as an example (never applied).
    fn network_policy_example(&self, _ns: &str) -> Option<String> {
        None
    }
    /// Versions an upgrade from `current` may go to.
    fn upgrade_path(&self, current: &str) -> Vec<String> {
        let v = self.versions();
        match v.iter().position(|x| x == current) {
            Some(i) => v[i + 1..].to_vec(),
            None => vec![],
        }
    }
    /// A service whose image is built on this machine (no public image exists) always goes through the in-cluster registry.
    fn always_mirror(&self) -> bool {
        false
    }
    /// Make the images available on this machine before they are pushed to the in-cluster registry (a local build).
    fn prepare_images(&self, _pl: &Platform, _version: &str, _arch: &str, _progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        Ok(())
    }
    /// Build the image in memory and push it to the registry with no container engine (a one-file image, see `ocipush.rs`). `registry`
    /// is `host:port` as this machine reaches it, `repo_tag` is `repo:tag`. Only a service whose image is one static program supports it
    /// (`Err` otherwise); the plan then offers it when neither podman nor docker is installed.
    fn push_builtin(&self, _pl: &Platform, _http: &dyn crate::ocipush::RegistryHttp, _version: &str, _arch: &str, _registry: &str, _repo_tag: &str, _progress: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err("this service's image cannot be built without podman or docker".into())
    }
    /// The program this service's local image is built from, if any ([`localbin`]): the wizard then finds it, offers to build it
    /// and lets the person choose a file, instead of giving up with a command to type.
    fn local_binary(&self) -> Option<localbin::LocalBinary> {
        None
    }
    /// A one-line card for the STORE or Settings: what it is for.
    fn card(&self) -> String {
        self.description()
    }
}

/// How to take a backup of a running service: commands run in the service's pod, then optionally copy the file out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupPlan {
    /// `kubectl exec` arguments after the pod: the command line to run in the container.
    pub exec: Vec<String>,
    /// The file the command writes inside the container, and its name.
    pub remote_file: String,
    pub name: String,
    pub note: String,
}

/// What a service's `post_deploy` may do: run a command in its pod and store a value in the vault.
pub struct PostCtx<'a> {
    pub p: &'a Platform,
    pub kc: crate::devcluster::Kc,
    pub namespace: String,
    pub instance: String,
    pub vault_prefix: String,
    pub opts: Options,
    pub urls: Vec<UrlInfo>,
    pub workload: Workload,
    /// Vault entries the step wrote (added to the install's record).
    pub added_vault: Vec<String>,
}

// ------------------------------------------------------------------------------------------------ the registry of services

static EXTRA: OnceLock<Mutex<Vec<Arc<dyn ServiceDef>>>> = OnceLock::new();

fn extra() -> &'static Mutex<Vec<Arc<dyn ServiceDef>>> {
    EXTRA.get_or_init(|| Mutex::new(vec![]))
}

/// Add a service (the private store does this at start). A second service with the same id replaces the first.
pub fn register(def: Arc<dyn ServiceDef>) {
    let mut v = extra().lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|d| d.id() != def.id());
    v.push(def);
}

pub fn unregister(id: &str) {
    extra().lock().unwrap_or_else(|e| e.into_inner()).retain(|d| d.id() != id);
}

/// The built-in services, then the registered ones.
pub fn specs() -> Vec<Arc<dyn ServiceDef>> {
    let mut v: Vec<Arc<dyn ServiceDef>> = vec![Arc::new(gitea::Gitea), Arc::new(crate::store::privatestore::PrivateStore), Arc::new(crate::indic::service::IndicTts)];
    v.extend(extra().lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
    v
}

pub fn spec(id: &str) -> Option<Arc<dyn ServiceDef>> {
    specs().into_iter().find(|d| d.id() == id)
}

// ------------------------------------------------------------------------------------------------ the context of every call

/// Everything the engine needs from the host: the executor, the person's directory and vault, the audit log. Tests give fakes.
pub struct Platform {
    pub run: Arc<dyn crate::wsexec::Runner>,
    /// The services directory (`installed-services-<user>.json` and the platform folder live here).
    pub dir: std::path::PathBuf,
    pub user: String,
    pub vault: Arc<dyn VaultApi>,
    /// `(action, detail)`: never a value.
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// Where kubeconfigs are looked for.
    pub env: crate::devcluster::Env,
    /// A folder in RAM for the 0600 files that carry a secret to a child process.
    pub tmp: std::path::PathBuf,
    pub http: Arc<dyn connect::Http>,
    /// Makes a random secret of `n` characters (tests plant known values here).
    pub gen: Arc<dyn Fn(usize) -> String + Send + Sync>,
    /// Live output of a running step, for the progress view.
    pub live: Option<crate::wsexec::Sink>,
}

impl Platform {
    /// The real thing for one person: the system runner, the person's vault, the real kubeconfig places.
    pub fn real(svc: &Arc<crate::services::Services>, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Platform {
        let run: Arc<dyn crate::wsexec::Runner> = Arc::new(crate::wsexec::System);
        Platform {
            run: run.clone(),
            dir: svc.dir().to_path_buf(),
            user: user.into(),
            vault: Arc::new(secrets::ServicesVault { svc: svc.clone(), user: user.into() }),
            audit,
            env: crate::devcluster::Env::real(),
            tmp: crate::kubeprofiles::temp_dir().unwrap_or_else(std::env::temp_dir),
            http: Arc::new(connect::CurlHttp { run, tmp: crate::kubeprofiles::temp_dir().unwrap_or_else(std::env::temp_dir) }),
            gen: Arc::new(secrets::random_secret),
            live: None,
        }
    }

    /// The same context with live output going to `live` (for one running step's progress view).
    pub fn with_live(&self, live: crate::wsexec::Sink) -> Platform {
        Platform { run: self.run.clone(), dir: self.dir.clone(), user: self.user.clone(), vault: self.vault.clone(), audit: self.audit.clone(), env: self.env.clone(), tmp: self.tmp.clone(), http: self.http.clone(), gen: self.gen.clone(), live: Some(live) }
    }

    pub(crate) fn audit(&self, action: &str, detail: &str) {
        (self.audit)(&format!("platform.{action}"), detail);
    }
}
