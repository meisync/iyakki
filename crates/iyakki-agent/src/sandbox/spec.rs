//! The vocabulary: what is asked ([`SandboxSpec`]), what comes back ([`SandboxResult`]).

use std::collections::BTreeMap;

/// The isolation tiers, weakest isolation last. The ids are what `sandbox.policy.tiers` lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// T0: a module that compiles to WebAssembly, run by iyakki-wasm (fuel, memory cap, capability lists).
    Wasm,
    /// T1: user, mount, pid, uts, ipc and net namespaces inside the pod (`linuxapps_init`); an explicit opt-in of the tenant admin.
    Namespace,
    /// T2: one hardened Kubernetes Job and NetworkPolicy per run. The default in cloud mode.
    Job,
    /// T3: the IYAKKI browser (web engine helper) in a Job of its own; optional, off by default.
    Browser,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Wasm, Tier::Namespace, Tier::Job, Tier::Browser];
    pub fn id(self) -> &'static str {
        match self {
            Tier::Wasm => "wasm",
            Tier::Namespace => "namespace",
            Tier::Job => "job",
            Tier::Browser => "browser",
        }
    }
    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.id() == s)
    }
}

/// What a run may reach. `Direct` exists so that a request for it can be named and REFUSED; no policy can allow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetMode {
    /// No network at all (not even DNS).
    None,
    /// Only DNS and the tenant's egress proxy (host allow-list, key injection).
    Proxy,
    /// The internet directly. Always refused.
    Direct,
}

impl NetMode {
    pub fn id(self) -> &'static str {
        match self {
            NetMode::None => "none",
            NetMode::Proxy => "proxy",
            NetMode::Direct => "direct",
        }
    }
    pub fn parse(s: &str) -> Option<NetMode> {
        [NetMode::None, NetMode::Proxy, NetMode::Direct].into_iter().find(|m| m.id() == s)
    }
}

/// What kind of work this is; it decides which tiers can run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// A program and its arguments (run in a container image).
    Command,
    /// A WebAssembly module (`argv[0]` is the entry export, default `_start`; the module bytes are `wasm`).
    WasmModule,
    /// A browser session: the steps are an `iyakki browser-test` spec.
    Browser,
}

/// How a secret reaches the run. A secret is never a value in a spec: only a reference, resolved at the last moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The egress proxy adds it to requests for `host`; the run never holds it.
    ProxyInject { host: String },
    /// A file `/run/secrets/iyakki/<name>`, mode 0400, on a memory-backed volume that exists only for this run.
    TmpfsFile,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretUse {
    /// The file name, or the environment name the proxy rule is filed under.
    pub name: String,
    /// `vault:<ns>/<name>`.
    pub secret_ref: String,
    pub delivery: Delivery,
}

impl std::fmt::Debug for SecretUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretUse({} -> {})", self.name, self.secret_ref)
    }
}

/// An environment variable. The value is printed nowhere: not in `Debug`, not in the audit, not in the result.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for EnvVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnvVar({}=<hidden>)", self.name)
    }
}

/// A small input file placed read-only at `/inputs/<name>` (kept in a per-run ConfigMap, so small: 256 KiB together).
#[derive(Clone, PartialEq, Eq)]
pub struct InputFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for InputFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InputFile({}, {} bytes)", self.name, self.bytes.len())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub cpu_m: u32,
    pub mem_mib: u32,
    pub pids: u32,
    pub seconds: u32,
    pub workspace_mib: u32,
    /// Largest stdout and stderr kept, each.
    pub output_kib: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { cpu_m: 500, mem_mib: 256, pids: 64, seconds: 60, workspace_mib: 128, output_kib: 64 }
    }
}

/// Who and what a run belongs to; every label of every object and every audit line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    pub tenant: String,
    pub team: String,
    pub person: String,
    pub run_id: String,
}

/// Things a caller may ASK for that no policy grants. They exist in the spec only so that a request for one is refused by name
/// ([`crate::sandbox::admit::admit`]) before any manifest exists. A flow, an AI agent or a script cannot add a privilege by
/// setting a field: the field only makes the run be refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Escalations {
    pub privileged: bool,
    pub host_network: bool,
    pub host_pid: bool,
    pub host_ipc: bool,
    pub host_paths: Vec<String>,
    pub run_as_root: bool,
    pub add_capabilities: Vec<String>,
    pub allow_privilege_escalation: bool,
    pub writable_root: bool,
    pub service_account_token: bool,
    pub docker_socket: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxSpec {
    pub work: Work,
    /// The program and its arguments. No shell string: `["python3", "-c", code]`, never `"python3 -c ..."`.
    pub argv: Vec<String>,
    pub env: Vec<EnvVar>,
    pub secrets: Vec<SecretUse>,
    /// Absolute path inside the run; empty means `/workspace`.
    pub workdir: String,
    pub inputs: Vec<InputFile>,
    pub limits: Limits,
    pub network: NetMode,
    /// A tier the caller asks for; `None` lets [`crate::sandbox::admit::choose_tier`] decide.
    pub tier: Option<Tier>,
    /// Container image (a Job); a wasm module or a browser session may leave it empty for the policy's browser image.
    pub image: String,
    /// The WebAssembly module (`Work::WasmModule`).
    pub wasm: Vec<u8>,
    pub labels: Labels,
    /// Names of files under `/workspace/out` to return as artifacts (empty: all, up to the caps).
    pub artifacts: Vec<String>,
    /// Browser sessions: the domains the session may open (each must be on the tenant's browser allow-list).
    pub domains: Vec<String>,
    pub escalations: Escalations,
}

impl SandboxSpec {
    pub fn command(argv: Vec<String>, image: &str, labels: Labels) -> SandboxSpec {
        SandboxSpec {
            work: Work::Command,
            argv,
            env: vec![],
            secrets: vec![],
            workdir: String::new(),
            inputs: vec![],
            limits: Limits::default(),
            network: NetMode::None,
            tier: None,
            image: image.into(),
            wasm: vec![],
            labels,
            artifacts: vec![],
            domains: vec![],
            escalations: Escalations::default(),
        }
    }

    /// A browser session: `start` addresses to open, `domains` it may reach, optional steps in `steps.json` among `inputs`.
    pub fn browser(start: Vec<String>, domains: Vec<String>, image: &str, labels: Labels) -> SandboxSpec {
        let mut s = SandboxSpec::command(start, image, labels);
        s.work = Work::Browser;
        s.domains = domains;
        s.network = NetMode::Proxy;
        s.tier = Some(Tier::Browser);
        s
    }

    /// The domains of a browser session: the requested ones and the hosts of the start addresses.
    pub fn browser_domains(&self) -> Vec<String> {
        let mut v = self.domains.clone();
        for u in &self.argv {
            let host = u.split_once("://").map(|x| x.1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
            let host = host.rsplit_once(':').map_or(host, |x| x.0).to_string();
            if !host.is_empty() && !v.contains(&host) {
                v.push(host);
            }
        }
        v
    }
    pub fn workdir_or_default(&self) -> &str {
        if self.workdir.is_empty() {
            "/workspace"
        } else {
            &self.workdir
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// The bytes are in [`SandboxResult::files`]; false when a size cap left them out (the size and hash are still real).
    pub returned: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub max_mem_mib: u32,
    /// WebAssembly only.
    pub fuel_used: u64,
}

/// Values a run held (resolved secrets, environment values) so that the facade can remove them from the output. Never printed.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Hidden(pub Vec<String>);

impl std::fmt::Debug for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} hidden values>", self.0.len())
    }
}

/// What a run produced, before the facade redacts it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawOutcome {
    pub held: Hidden,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub oom_killed: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub artifacts: Vec<(String, Vec<u8>)>,
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxResult {
    pub run_id: String,
    pub tier: Tier,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub oom_killed: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// A credential was found and replaced in the output.
    pub redacted: bool,
    pub artifacts: Vec<Artifact>,
    /// The artifacts that fit the caps (20 files, 2 MiB each, 4 MiB together).
    pub files: Vec<(String, Vec<u8>)>,
    pub usage: Usage,
}

impl SandboxResult {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.oom_killed
    }
}

/// Why no result came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// Admission said no (the reason is for the person and never repeats a value).
    Refused(String),
    /// A person with the approver role must say yes first; the request is described for the approvals router.
    NeedsApproval { reason: String, approver_role: String },
    /// The queue timed out.
    QueueTimeout,
    /// The cluster, the namespace sandbox or the wasm engine failed.
    Infra(String),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::Refused(r) => write!(f, "refused: {r}"),
            SandboxError::NeedsApproval { reason, approver_role } => write!(f, "needs approval from a {approver_role}: {reason}"),
            SandboxError::QueueTimeout => write!(f, "the sandbox queue timed out"),
            SandboxError::Infra(e) => write!(f, "the sandbox failed: {e}"),
        }
    }
}

/// A map of the labels in the form Kubernetes wants.
pub fn label_map(l: &Labels, tier: Tier) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("app.kubernetes.io/name".into(), "iyakki-sandbox".into());
    m.insert("iyakki.dev/tenant".into(), label_value(&l.tenant));
    m.insert("iyakki.dev/team".into(), label_value(&l.team));
    m.insert("iyakki.dev/person".into(), label_value(&l.person));
    m.insert("iyakki.dev/run".into(), label_value(&l.run_id));
    m.insert("iyakki.dev/tier".into(), tier.id().into());
    m
}

/// A Kubernetes label value: lower case letters, digits, `-`, at most 63, starting and ending with a letter or digit.
pub fn label_value(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(63).collect();
    let t = t.trim_matches('-').to_string();
    if t.is_empty() {
        "x".into()
    } else {
        t
    }
}
