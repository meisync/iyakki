//! The plain data of the platform services (plans, options, installed records, connection plans, local programs), moved out of
//! `iyakki_agent::platform` for F-SPLIT-03; the agent re-exports it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::devcluster::ClusterProfile;


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
            Exposure::NodePort => "opens the chosen ports on every network address of the nodes: anyone on your network can reach it",
            Exposure::LoadBalancer => "the cluster's load balancer gives it an external address: possibly reachable from outside your network",
            Exposure::Ingress => "reachable through the cluster's ingress controller at the host name you give",
        }
    }
}

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


/// A cluster the person may pick, with the reason it cannot be used (None: usable).
#[derive(Clone, Debug)]
pub struct ListedProfile {
    pub id: String,
    pub profile: ClusterProfile,
    pub refusal: Option<String>,
}

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

#[derive(Debug, Clone)]
pub struct BackupResult {
    /// The file inside the pod (on the PVC).
    pub remote: String,
    pub local: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct RemovePlan {
    pub inst: Installed,
    pub delete_data: bool,
}


#[derive(Clone, Debug)]
pub struct WsInfo {
    pub name: String,
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ssh,
}

#[derive(Clone, Debug)]
pub struct ConnectOpts {
    /// Empty: from the workspace name.
    pub repo: String,
    pub private: bool,
    pub transport: Transport,
    /// Replace an `origin` that points somewhere else (shown in the plan).
    pub replace_origin: bool,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts { repo: String::new(), private: true, transport: Transport::Https, replace_origin: false }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectPlan {
    pub inst: Installed,
    pub ws: WsInfo,
    pub repo: String,
    pub private: bool,
    pub transport: Transport,
    pub branch: String,
    pub origin: Option<String>,
    pub replace_origin: bool,
    pub commits: usize,
    pub base: String,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Connected {
    pub owner: String,
    pub repo_url: String,
    pub remote_url: String,
    pub created: bool,
    pub origin_set: bool,
}

#[derive(Clone, Debug)]
pub struct PushPlan {
    pub inst: Installed,
    pub ws: WsInfo,
    pub branch: String,
    pub remote: String,
    pub commits: usize,
    pub transport: Transport,
    pub blockers: Vec<String>,
}

/// A pull request from the workspace's current branch into `base`, opened through the service's API. Nothing is pushed here: the branch
/// must already be on the service (its own approval, [`push`]), so the request is for what the service holds.
#[derive(Clone, Debug)]
pub struct PrPlan {
    pub inst: Installed,
    pub ws: WsInfo,
    pub owner: String,
    pub repo: String,
    pub head: String,
    pub base_branch: String,
    pub title: String,
    pub body: String,
    pub api_base: String,
    pub blockers: Vec<String>,
}


/// How to reach the cluster a service lives in (a reference; it is re-checked, never trusted, before anything is run).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterRef {
    pub name: String,
    pub kubeconfig: String,
    pub context: String,
    pub server: String,
    pub source: crate::devcluster::Source,
}

impl ClusterRef {
    pub fn of(p: &crate::devcluster::ClusterProfile) -> ClusterRef {
        ClusterRef { name: p.name.clone(), kubeconfig: p.kubeconfig.clone(), context: p.context.clone(), server: p.server.clone(), source: p.source }
    }
    pub fn line(&self) -> String {
        format!("{} ({}{})", if self.name.is_empty() { "cluster" } else { &self.name }, if self.server.is_empty() { "server unknown" } else { &self.server }, if self.context.is_empty() { String::new() } else { format!(", context {}", self.context) })
    }
}

/// A workspace connected to a git service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsLink {
    pub workspace: String,
    pub repo_url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    /// The instance: `<service>-<namespace>` (made unique across clusters).
    pub id: String,
    pub service: String,
    pub title: String,
    pub version: String,
    pub kind: String,
    pub cluster: Option<ClusterRef>,
    pub namespace: String,
    /// IYAKKI created the namespace (so Remove with data may delete it).
    pub created_namespace: bool,
    /// An existing service the person connected: IYAKKI deployed nothing and removes nothing.
    pub connected: bool,
    pub options: Options,
    pub urls: Vec<UrlInfo>,
    /// Vault entry names (`svc.<instance>.<key>`); the values are in the person's vault.
    pub vault: Vec<String>,
    pub installed_at: i64,
    pub updated_at: i64,
    pub workspaces: Vec<WsLink>,
    pub note: String,
}

impl Default for Installed {
    fn default() -> Self {
        Installed { id: String::new(), service: String::new(), title: String::new(), version: String::new(), kind: String::new(), cluster: None, namespace: String::new(), created_namespace: false, connected: false, options: Options::default(), urls: vec![], vault: vec![], installed_at: 0, updated_at: 0, workspaces: vec![], note: String::new() }
    }
}

impl Installed {
    pub fn web_url(&self) -> Option<&str> {
        self.urls.iter().find(|u| u.kind == "web").map(|u| u.url.as_str())
    }
    pub fn token_vault(&self) -> Option<&str> {
        self.vault.iter().find(|v| v.ends_with(".token")).map(String::as_str)
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// From a Kubernetes node's `architecture` (`amd64`, `arm64`) or a `uname -m` word.
    pub fn parse(s: &str) -> Option<Arch> {
        match s.trim() {
            "amd64" | "x86_64" | "x86-64" => Some(Arch::X86_64),
            "arm64" | "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }
    pub fn host() -> Arch {
        Arch::parse(std::env::consts::ARCH).unwrap_or(Arch::X86_64)
    }
    /// `x86_64` / `aarch64`: the folder and file-name word.
    pub fn uname(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
    /// `amd64` / `arm64`: the container and Kubernetes word.
    pub fn node(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }
    pub fn triple(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-unknown-linux-musl",
            Arch::Aarch64 => "aarch64-unknown-linux-musl",
        }
    }
    pub fn from_machine(m: u16) -> Option<Arch> {
        match m {
            62 => Some(Arch::X86_64),
            183 => Some(Arch::Aarch64),
            _ => None,
        }
    }
}

/// What the headers of an executable say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElfInfo {
    pub machine: u16,
    pub arch: Option<Arch>,
    /// No program interpreter and no needed library: it can run in an image with nothing else in it.
    pub is_static: bool,
    /// Why it is not static (the interpreter or the first needed library).
    pub dynamic_because: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    Unreadable(String),
    NotElf,
    Truncated,
    Unsupported(String),
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfError::Unreadable(e) => write!(f, "cannot be read ({e})"),
            ElfError::NotElf => write!(f, "is not a Linux executable (no ELF header)"),
            ElfError::Truncated => write!(f, "is cut short (the ELF headers are incomplete)"),
            ElfError::Unsupported(w) => write!(f, "is not supported ({w})"),
        }
    }
}

/// A program a service builds its image from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalBinary {
    /// The file name, e.g. `iyakki-store-server`.
    pub file: &'static str,
    /// The cargo package that builds it.
    pub package: &'static str,
    /// The folder under the data directory (the cache) and under `/usr/share/iyakki/` (shipped).
    pub sub: &'static str,
    /// The name an older cache used for the same program (any static `iyakki` works for the store).
    pub legacy: &'static [&'static str],
    /// What to read.
    pub docs: &'static str,
}

/// Where to look; built from the machine ([`Places::real`]) or by a test.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// Folders holding shipped copies, in order. Each is searched as `<dir>/<arch>/<file>`, `<dir>/<file>-<arch>` and `<dir>/<file>`.
    pub shipped: Vec<PathBuf>,
    /// The cache folder (`<data>/<sub>`).
    pub cache: PathBuf,
    /// The source tree, if found.
    pub source: Option<PathBuf>,
    /// Replaces the build command (tests give a harmless one); the output must still be at `target/<triple>/release/<file>`.
    pub build_override: Option<Vec<String>>,
}

impl Places {
    pub fn real(data: &Path, lb: &LocalBinary) -> Places {
        let mut shipped = vec![PathBuf::from("/usr/share/iyakki").join(lb.sub)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                shipped.push(d.join(lb.sub));
                shipped.push(d.to_path_buf());
            }
        }
        Places { shipped, cache: data.join(lb.sub), source: find_source(data, lb), build_override: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Shipped,
    Cache,
    Source,
}

impl Origin {
    pub fn word(self) -> &'static str {
        match self {
            Origin::Shipped => "shipped with this install",
            Origin::Cache => "kept in your IYAKKI data folder",
            Origin::Source => "built in the source tree",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub origin: Origin,
    pub arch: Arch,
}

/// How to build it here, or what stops that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildState {
    Ready(BuildPlan),
    /// Each entry is one missing thing, with the exact way to get it.
    Missing(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildPlan {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Where the finished file will be.
    pub output: PathBuf,
    pub arch: Arch,
}

#[derive(Clone, Debug)]
pub struct Discovery {
    pub want: Arch,
    pub found: Option<Found>,
    /// Files that exist but cannot be used, with the reason (dynamic, wrong CPU, not ELF).
    pub rejected: Vec<(PathBuf, String)>,
    /// Static files for another CPU: the person may have the wrong one.
    pub other_arch: Vec<(PathBuf, Arch)>,
    /// Every place looked at, in order.
    pub searched: Vec<PathBuf>,
    pub build: BuildState,
}

impl Discovery {
    /// The plain statement of what is missing and the ways out, for the wizard card and the assistant.
    pub fn problem(&self, lb: &LocalBinary, cache: &Path) -> String {
        let mut t = format!("No static {} for {} was found.", lb.file, self.want.uname());
        for (_, why) in &self.rejected {
            t.push_str(&format!("\n- not used: {why}"));
        }
        for (p, a) in &self.other_arch {
            t.push_str(&format!("\n- {} is a static {} build, but this cluster needs {}: build or download the {} one", p.display(), a.uname(), self.want.uname(), self.want.uname()));
        }
        match &self.build {
            BuildState::Ready(b) => t.push_str(&format!("\nIt can be built here: `{}` (Build now).", b.argv.join(" "))),
            BuildState::Missing(m) => {
                t.push_str("\nBuilding it here is not possible yet. Missing:");
                for x in m {
                    t.push_str(&format!("\n- {x}"));
                }
            }
        }
        t.push_str(&format!(
            "\nOr: download {}-{} from the release page (it is signed with the release) and choose it, or put it at {}; or build the image yourself from dist/store-server/Dockerfile ({}).",
            lb.file,
            self.want.uname(),
            cache.join(format!("{}-{}", lb.file, self.want.uname())).display(),
            lb.docs
        ));
        t
    }
}


/// A value to show once. Its `Debug` never prints the value; the memory is wiped on drop.
pub struct Reveal {
    pub label: String,
    pub vault: String,
    pub value: Zeroizing<String>,
}

impl std::fmt::Debug for Reveal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reveal({}, uvault:{}, {} characters)", self.label, self.vault, self.value.len())
    }
}


/// The source tree: `$IYAKKI_SOURCE`, else the folder written in `<data>/<sub>/source` (the word `none` turns the search off), else the
/// first folder above the running program or the current folder that holds the package.
pub fn find_source(data: &Path, lb: &LocalBinary) -> Option<PathBuf> {
    let is_root = |d: &Path| d.join("Cargo.toml").is_file() && d.join("crates").join(lb.package).join("Cargo.toml").is_file();
    if let Some(s) = std::env::var_os("IYAKKI_SOURCE").map(PathBuf::from) {
        if is_root(&s) {
            return Some(s);
        }
    }
    if let Ok(t) = std::fs::read_to_string(data.join(lb.sub).join("source")) {
        let t = t.trim();
        if t == "none" {
            return None;
        }
        let p = expand_typed(t);
        if is_root(&p) {
            return Some(p);
        }
    }
    let starts = [std::env::current_exe().ok(), std::env::current_dir().ok()];
    starts.into_iter().flatten().find_map(|s| s.ancestors().skip(1).take(8).find(|d| is_root(d)).map(Path::to_path_buf))
}

/// `~/x` and relative-free paths typed by a person.
pub fn expand_typed(t: &str) -> PathBuf {
    let t = t.trim().trim_matches('"').trim_matches('\'');
    match t.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| PathBuf::from(t)),
        None => PathBuf::from(t),
    }
}

/// How long `create` waits for the registry to answer from this machine.
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

/// `http(s)://host[:port]` without a path, credentials or query.
pub fn normalize_url(url: &str) -> Result<String, String> {
    let u = url.trim().trim_end_matches('/');
    let (scheme, rest) = u.split_once("://").ok_or("the address must start with http:// or https://")?;
    if scheme != "http" && scheme != "https" {
        return Err("the address must start with http:// or https://".into());
    }
    if rest.is_empty() || rest.contains(['@', '?', '#', ' ']) || rest.contains('/') && rest.split_once('/').is_some_and(|x| !x.1.is_empty()) {
        return Err("give the address without a path, user name or query (such as http://192.168.1.10:3000)".into());
    }
    let host = rest.trim_end_matches('/');
    if !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']')) {
        return Err(format!("`{host}` is not a host name or address"));
    }
    Ok(format!("{scheme}://{host}"))
}

/// A DNS label (a namespace or service name part): lower-case letters, digits and `-`, up to 63, not starting or ending with `-`.
pub fn dns_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}
