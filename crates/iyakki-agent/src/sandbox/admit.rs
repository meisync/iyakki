//! Admission: the pure checks every request passes before any manifest, process or module exists. [`admit`] is the only way to make an
//! [`Admitted`], and every executor takes an `Admitted`; a refused request therefore has no manifest. Reasons name the rule and the
//! field, never a value that was sent (an environment value or a secret is never repeated).

use super::policy::SandboxPolicy;
use super::spec::{Delivery, NetMode, SandboxError, SandboxSpec, Tier, Work};
use crate::tenantconfig::resolve::covers;
use crate::tenantconfig::schema::{self, SecretRef};

pub const MAX_ARGS: usize = 64;
pub const MAX_ARG_LEN: usize = 8192;
pub const MAX_ENV: usize = 32;
pub const MAX_ENV_VALUE: usize = 8192;
pub const MAX_SECRETS: usize = 8;
pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_WASM_BYTES: usize = 8 * 1024 * 1024;

/// The person asking and their team role (`viewer`, `editor`, `admin`, `tenant_admin`).
#[derive(Clone, Copy, Debug)]
pub struct Who<'a> {
    pub person: &'a str,
    pub role: &'a str,
}

/// A request that passed. Only [`admit`] makes one.
#[derive(Clone, Debug)]
pub struct Admitted {
    spec: SandboxSpec,
    tier: Tier,
    approved_by: Option<String>,
    digest_pinned: bool,
}

impl Admitted {
    pub fn spec(&self) -> &SandboxSpec {
        &self.spec
    }
    pub fn tier(&self) -> Tier {
        self.tier
    }
    pub fn approved_by(&self) -> Option<&str> {
        self.approved_by.as_deref()
    }
    pub fn digest_pinned(&self) -> bool {
        self.digest_pinned
    }
    /// The Kubernetes name of everything made for this run.
    pub fn run_name(&self) -> String {
        let id = super::spec::label_value(&self.spec.labels.run_id);
        let n = format!("sbx-{id}");
        n.chars().take(52).collect::<String>().trim_end_matches('-').to_string()
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, SandboxError> {
    Err(SandboxError::Refused(why.into()))
}

/// Which tier runs this, or why none can. Pure; the table in the tests is the contract. `namespace` is never chosen automatically:
/// it needs the caller to ask for it, the policy to list it and the tenant admin to have confirmed that the pods allow user namespaces.
pub fn choose_tier(policy: &SandboxPolicy, spec: &SandboxSpec) -> Result<Tier, String> {
    match spec.work {
        Work::Browser => {
            if !policy.browser || !policy.allows_tier(Tier::Browser) {
                return Err("the browser sandbox is off for this tenant (sandbox.policy: browser)".into());
            }
            match spec.tier {
                Some(t) if t != Tier::Browser => Err(format!("a browser session runs in the browser tier, not {}", t.id())),
                _ => Ok(Tier::Browser),
            }
        }
        Work::WasmModule => {
            if let Some(t) = spec.tier {
                if t != Tier::Wasm {
                    return Err(format!("a WebAssembly module runs in the wasm tier, not {}", t.id()));
                }
            }
            if !policy.allows_tier(Tier::Wasm) {
                return Err("the wasm tier is not allowed by the tenant's sandbox policy".into());
            }
            Ok(Tier::Wasm)
        }
        Work::Command => match spec.tier {
            Some(Tier::Wasm) => Err("a command is not a WebAssembly module".into()),
            Some(Tier::Browser) => Err("a command does not run in the browser tier".into()),
            Some(Tier::Namespace) => {
                if !policy.allows_tier(Tier::Namespace) || !policy.userns_ack {
                    Err("the namespace tier is not enabled: the tenant admin must list it and confirm the pods allow user namespaces".into())
                } else {
                    Ok(Tier::Namespace)
                }
            }
            Some(Tier::Job) => {
                if policy.allows_tier(Tier::Job) {
                    Ok(Tier::Job)
                } else {
                    Err("the job tier is not allowed by the tenant's sandbox policy".into())
                }
            }
            None => {
                if policy.allows_tier(Tier::Job) {
                    Ok(Tier::Job)
                } else {
                    Err("no tier can run a command: the job tier is not allowed (the namespace tier is never chosen by itself)".into())
                }
            }
        },
    }
}

/// The registry part of an image reference (`docker.io` when none is written).
pub fn image_registry(image: &str) -> String {
    let name = image.split('@').next().unwrap_or(image);
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => first.to_string(),
        _ => "docker.io".into(),
    }
}

pub fn image_pinned(image: &str) -> bool {
    image.split_once("@sha256:").is_some_and(|(_, d)| d.len() == 64 && d.chars().all(|c| c.is_ascii_hexdigit()))
}

fn image_chars_ok(image: &str) -> bool {
    !image.is_empty() && image.len() <= 300 && image.chars().all(|c| c.is_ascii_alphanumeric() || "._:/@-".contains(c)) && !image.starts_with(['-', '/'])
}

fn role_rank(r: &str) -> i32 {
    schema::ROLES.iter().position(|x| *x == r).map_or(99, |i| i as i32)
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

const RESERVED_ENV: &[&str] = &["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY", "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "BASH_ENV", "ENV", "PYTHONSTARTUP", "NODE_OPTIONS"];

fn env_name_ok(n: &str) -> Result<(), String> {
    let first_ok = n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first_ok || n.len() > 64 || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("an environment name is letters, digits and _".into());
    }
    let up = n.to_ascii_uppercase();
    if RESERVED_ENV.contains(&up.as_str()) || up.starts_with("KUBERNETES_") || (up.starts_with("IYAKKI_") && !matches!(up.as_str(), "IYAKKI_FLOW" | "IYAKKI_RUN")) || up.starts_with("LD_") {
        return Err(format!("the environment name {n} is reserved"));
    }
    Ok(())
}

/// Why this request asks for more than any policy grants (the first thing admission checks).
pub fn escalation_reason(spec: &SandboxSpec) -> Option<String> {
    let e = &spec.escalations;
    let list: [(bool, &str); 11] = [
        (e.privileged, "privileged containers are never allowed"),
        (e.host_network, "the host network is never allowed"),
        (e.host_pid, "the host process namespace is never allowed"),
        (e.host_ipc, "the host IPC namespace is never allowed"),
        (!e.host_paths.is_empty(), "host folders (hostPath) are never mounted"),
        (e.run_as_root, "running as root is never allowed"),
        (!e.add_capabilities.is_empty(), "adding Linux capabilities is never allowed"),
        (e.allow_privilege_escalation, "allowing privilege escalation is never allowed"),
        (e.writable_root, "a writable root file system is never allowed; use /workspace"),
        (e.service_account_token, "a run never gets a service account token (it could reach the cluster API)"),
        (e.docker_socket, "the container runtime socket is never mounted"),
    ];
    list.iter().find(|(on, _)| *on).map(|(_, t)| (*t).to_string())
}

fn words_after<'a>(argv: &'a [String]) -> Vec<&'a str> {
    argv.iter().skip(1).map(String::as_str).filter(|a| !a.starts_with('-')).collect()
}

fn base(argv0: &str) -> &str {
    argv0.rsplit('/').next().unwrap_or(argv0)
}

fn pipes_into_interpreter(script: &str) -> bool {
    let l = script.to_ascii_lowercase();
    let fetch = l.contains("curl") || l.contains("wget") || l.contains("fetch ");
    if !fetch {
        return false;
    }
    if l.contains("<(curl") || l.contains("<(wget") || l.contains("$(curl") || l.contains("$(wget") {
        return true;
    }
    l.split('|').skip(1).any(|after| {
        let w = after.trim_start().split(|c: char| c.is_whitespace() || c == ';' || c == '&').next().unwrap_or("");
        matches!(base(w), "sh" | "bash" | "zsh" | "dash" | "ksh" | "python" | "python3" | "perl" | "ruby" | "node" | "sudo")
    })
}

fn leaves_workspace(a: &str, workdir: &str) -> bool {
    if a.starts_with('-') {
        return false;
    }
    if a.split('/').any(|p| p == "..") || a.starts_with('~') || a == "/" {
        return true;
    }
    a.starts_with('/') && !(a == workdir || a.starts_with(&format!("{workdir}/")) || a.starts_with("/workspace/") || a == "/workspace" || a.starts_with("/tmp/"))
}

/// Does this command always need a person's approval? The built-in patterns plus the tenant's. Returns the pattern that matched.
/// Patterns are written `program word word`: the program is argv[0] without its folder, the words must all appear among the
/// arguments that do not start with `-` (so `kubectl apply` matches `kubectl -n x apply -f y`).
pub fn needs_approval(argv: &[String], workdir: &str, tenant_patterns: &[String]) -> Option<String> {
    let first = base(argv.first()?);
    let args: Vec<&str> = words_after(argv);
    let flags: Vec<&str> = argv.iter().skip(1).map(String::as_str).filter(|a| a.starts_with('-')).collect();
    // built in: never left to a policy that forgot them
    if matches!(first, "sudo" | "su" | "doas" | "dd" | "mkfs" | "mount" | "umount" | "chroot" | "nsenter" | "shred" | "docker" | "podman" | "ctr" | "nerdctl" | "crictl") || first.starts_with("mkfs.") {
        return Some(format!("{first} (high-risk program)"));
    }
    if matches!(first, "ssh" | "scp" | "sftp" | "telnet" | "nc" | "ncat" | "socat") {
        return Some(format!("{first} (connecting to another machine)"));
    }
    if matches!(first, "kubectl" | "k3s" | "oc" | "helm" | "kustomize") {
        if let Some(v) = args.iter().find(|w| matches!(**w, "apply" | "delete" | "replace" | "patch" | "drain" | "uninstall" | "destroy" | "cordon" | "scale" | "edit" | "exec" | "cp" | "port-forward" | "create" | "annotate" | "label" | "taint")) {
            return Some(format!("{first} {v} (changes a cluster)"));
        }
    }
    if matches!(first, "terraform" | "tofu") && args.iter().any(|w| matches!(*w, "apply" | "destroy" | "import" | "taint")) {
        return Some(format!("{first} apply/destroy (changes infrastructure)"));
    }
    if first == "rm" || first == "rmdir" || first == "find" || first == "mv" {
        let recursive = flags.iter().any(|f| f.starts_with("--") && f.contains("recursive") || (!f.starts_with("--") && (f.contains('r') || f.contains('R')))) || first == "find";
        let force_delete = first == "find" && argv.iter().any(|a| a == "-delete" || a == "-exec");
        if (first == "rm" && recursive || first == "rmdir" || first == "mv" || force_delete) && argv.iter().skip(1).any(|a| leaves_workspace(a, workdir)) {
            return Some(format!("{first} (removes or moves files outside the workspace)"));
        }
    }
    if matches!(first, "sh" | "bash" | "dash" | "zsh" | "ksh" | "ash") {
        if let Some(i) = argv.iter().position(|a| a == "-c" || a == "-lc" || a == "-ec") {
            if argv.get(i + 1).is_some_and(|s| pipes_into_interpreter(s)) {
                return Some("a downloaded script piped into a shell (curl | sh)".into());
            }
        }
    }
    for pat in tenant_patterns {
        let mut w = pat.split_whitespace();
        let Some(prog) = w.next() else { continue };
        if prog != first {
            continue;
        }
        let rest: Vec<&str> = w.collect();
        let all_args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
        if rest.iter().all(|r| all_args.contains(r)) {
            return Some(pat.clone());
        }
    }
    None
}

/// Check a request against the policy. `approved_by` is somebody the approvals router has already accepted for this request.
pub fn admit(policy: &SandboxPolicy, spec: &SandboxSpec, who: &Who, approved_by: Option<&str>) -> Result<Admitted, SandboxError> {
    if !policy.enabled {
        return refuse("sandbox runs are not switched on for this tenant (sandbox.policy)");
    }
    if let Some(why) = escalation_reason(spec) {
        return refuse(why);
    }
    let l = &spec.labels;
    for (name, v) in [("tenant", &l.tenant), ("team", &l.team), ("person", &l.person), ("run id", &l.run_id)] {
        if v.is_empty() || v.len() > 128 || v.chars().any(|c| c.is_control()) {
            return refuse(format!("the {name} of the run is missing or not valid"));
        }
    }
    if role_rank(who.role) < role_rank(&policy.cli_role) {
        return refuse(format!("running commands needs the {} role", policy.cli_role));
    }
    let tier = choose_tier(policy, spec).map_err(SandboxError::Refused)?;
    // what is run
    match spec.work {
        Work::Command | Work::Browser => {
            if spec.work == Work::Command && (spec.argv.is_empty() || spec.argv[0].is_empty()) {
                return refuse("no command");
            }
            if spec.work == Work::Browser && spec.browser_domains().is_empty() {
                return refuse("a browser session needs the domains it may open");
            }
            if spec.argv.len() > MAX_ARGS || spec.argv.iter().any(|a| a.len() > MAX_ARG_LEN || a.contains('\0')) {
                return refuse(format!("at most {MAX_ARGS} arguments of {MAX_ARG_LEN} characters, no NUL"));
            }
        }
        Work::WasmModule => {
            if spec.wasm.is_empty() || spec.wasm.len() > MAX_WASM_BYTES {
                return refuse("the WebAssembly module is empty or over 8 MiB");
            }
        }
    }
    // environment and secrets
    if spec.env.len() > MAX_ENV {
        return refuse(format!("at most {MAX_ENV} environment variables"));
    }
    for e in &spec.env {
        if let Err(m) = env_name_ok(&e.name) {
            return refuse(m);
        }
        if e.value.len() > MAX_ENV_VALUE || e.value.contains('\0') {
            return refuse(format!("the value of {} is too long", e.name));
        }
        if crate::credguard::detect(&e.value).is_some() {
            return refuse(format!("the value of {} looks like a credential: put it in the vault and pass a secret reference", e.name));
        }
    }
    if spec.secrets.len() > MAX_SECRETS {
        return refuse(format!("at most {MAX_SECRETS} secrets"));
    }
    for s in &spec.secrets {
        if !plain(&s.name, 64) || SecretRef::parse(&s.secret_ref).is_none() {
            return refuse("a secret needs a plain name and a vault reference like vault:<ns>/<name>");
        }
        if let Delivery::ProxyInject { host } = &s.delivery {
            if !schema::host_ok(host) {
                return refuse("a proxy-injected secret names one host");
            }
            if spec.network != NetMode::Proxy {
                return refuse("a secret injected by the proxy needs the proxy network mode");
            }
        }
        if tier == Tier::Wasm || tier == Tier::Namespace {
            return refuse("this tier takes no secrets: use the job tier");
        }
    }
    // inputs and workdir
    if spec.inputs.iter().map(|i| i.bytes.len()).sum::<usize>() > MAX_INPUT_BYTES || spec.inputs.iter().any(|i| !plain(&i.name, 100)) {
        return refuse("input files: plain names, 256 KiB together");
    }
    let wd = spec.workdir_or_default();
    if !(wd == "/workspace" || wd.starts_with("/workspace/")) || wd.split('/').any(|p| p == "..") || wd.contains('\0') {
        return refuse("the working folder must be inside /workspace");
    }
    // network
    if spec.network == NetMode::Direct {
        return refuse("direct internet access is never allowed: use the proxy mode (host allow-list and key injection)");
    }
    if !policy.network_modes.contains(&spec.network) {
        return refuse(format!("the network mode {} is not allowed by the tenant's sandbox policy", spec.network.id()));
    }
    if matches!(tier, Tier::Wasm | Tier::Namespace) && spec.network != NetMode::None {
        return refuse("this tier has no network (the namespace tier would share the tenant pod's network): use the job tier");
    }
    if tier == Tier::Browser && spec.network != NetMode::Proxy {
        return refuse("a browser session reaches sites only through the proxy");
    }
    // image
    let mut digest_pinned = false;
    if matches!(tier, Tier::Job | Tier::Browser) {
        let image = if spec.image.is_empty() && tier == Tier::Browser { policy.browser_image.as_str() } else { spec.image.as_str() };
        if !image_chars_ok(image) {
            return refuse("the image reference is missing or not valid");
        }
        let reg = image_registry(image);
        if !policy.allowed_registries.iter().any(|p| covers(p, &reg)) {
            return refuse(format!("the image registry {reg} is not allowed by the tenant's sandbox policy"));
        }
        digest_pinned = image_pinned(image);
        if policy.require_digest && !digest_pinned {
            return refuse("the tenant requires images pinned by @sha256 digest");
        }
    }
    // limits
    let (a, m) = (&spec.limits, &policy.max);
    for (name, v, max) in [("cpu", a.cpu_m, m.cpu_m), ("memory", a.mem_mib, m.mem_mib), ("processes", a.pids, m.pids), ("time", a.seconds, m.seconds), ("workspace", a.workspace_mib, m.workspace_mib)] {
        if v == 0 {
            return refuse(format!("the {name} limit must be set"));
        }
        if v > max {
            return refuse(format!("the {name} limit is above the tenant's maximum"));
        }
    }
    if a.output_kib == 0 || a.output_kib > 1024 {
        return refuse("the output cap is 1 to 1024 KiB");
    }
    // browser domains
    if tier == Tier::Browser {
        if spec.argv.iter().any(|u| !u.starts_with("https://") && !u.starts_with("http://")) {
            return refuse("a browser session starts from http or https addresses");
        }
        for host in spec.browser_domains() {
            if !schema::host_pattern_ok(&host) || host == "*" || !policy.browser_domains.iter().any(|d| covers(d, &host)) {
                return refuse(format!("the domain {host} is not on the tenant's browser allow-list"));
            }
        }
    }
    // approval
    if matches!(tier, Tier::Job | Tier::Namespace) {
        if let Some(pat) = needs_approval(&spec.argv, wd, &policy.approval_patterns) {
            if approved_by.is_none() {
                return Err(SandboxError::NeedsApproval { reason: format!("this command always needs approval: {pat}"), approver_role: "admin".into() });
            }
        }
    }
    Ok(Admitted { spec: spec.clone(), tier, approved_by: approved_by.map(String::from), digest_pinned })
}
