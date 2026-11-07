//! Where the sandbox meets the rest of the program.
//!
//! 1. `run_command` of the agent ([`route`]): in a tenant pod (the manifest sets `IYAKKI_TENANT_DATA_DIR`) a command never runs on the
//!    pod. With a [`CloudRunner`] installed (at pod start, by the cloud listener) it goes through the sandbox; without one it is
//!    refused. Anywhere else it runs as before.
//! 2. The flows `cli` and `browser` nodes: [`exec_via_sandbox`] and [`browser_via_sandbox`] adapt the host-side request types of
//!    `iyakki-flow` (`ExecRequest`, `BrowserRequest`) to a [`SandboxSpec`] and the [`SandboxResult`] back.

use super::admit::Who;
use super::engine::Engine;
use super::exec::{ApplyGate, JobExecutor, K3sGate, SecretResolver};
use super::kube::KubeApi;
use super::policy::SandboxPolicy;
use super::spec::{Labels, NetMode, SandboxError, SandboxResult, SandboxSpec, Tier};
use crate::tenantconfig::api::{ConfigAudit, Env};
use std::sync::{Arc, Mutex, OnceLock};

/// What `run_command` calls in a tenant pod.
pub trait CloudRunner: Send + Sync {
    fn run_command(&self, user: &str, program: &str, args: &[String], seconds: u64) -> Result<String, String>;
}

fn hook() -> &'static Mutex<Option<Arc<dyn CloudRunner>>> {
    static H: OnceLock<Mutex<Option<Arc<dyn CloudRunner>>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

/// Connect the sandbox to `run_command` (called once at pod start).
pub fn install(r: Arc<dyn CloudRunner>) {
    *hook().lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

pub fn uninstall() {
    *hook().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub enum Route {
    /// Run it here, as before (not a tenant pod).
    Local,
    Sandbox(Arc<dyn CloudRunner>),
    Refuse(String),
}

/// Pure decision: a runner is connected, or this is a tenant pod without one (refuse), or neither.
pub fn route_of(runner: Option<Arc<dyn CloudRunner>>, tenant_pod: bool) -> Route {
    match (runner, tenant_pod) {
        (Some(r), _) => Route::Sandbox(r),
        (None, true) => Route::Refuse("commands do not run on the tenant's pod: the sandbox is not connected here yet, so this command was not started".into()),
        (None, false) => Route::Local,
    }
}

pub fn route() -> Route {
    let r = hook().lock().unwrap_or_else(|e| e.into_inner()).clone();
    route_of(r, crate::tenantconfig::enforce::data_dir_from_env().is_some())
}

// ------------------------------------------------------------------------------------------------------------------ run_command

/// The sandbox behind `run_command` of a tenant pod: reads the tenant's roles and `sandbox.policy` for each call (so a change takes
/// effect at once), asks the team's cluster binding, files an approval request when a pattern asks for one, and returns the output.
pub struct TenantRunner {
    pub dir: std::path::PathBuf,
    pub tenant: String,
    pub kube: Arc<dyn KubeApi>,
    pub vault: Arc<dyn crate::tenantconfig::vault::SecretStore>,
    pub audit: Arc<dyn ConfigAudit>,
    pub secrets: Arc<dyn SecretResolver>,
    pub counter: Mutex<u64>,
}

pub const ROLE_NAMES: [&str; 4] = ["viewer", "editor", "admin", "tenant_admin"];

impl TenantRunner {
    fn run_id(&self, user: &str) -> String {
        let mut n = self.counter.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        let r = crate::cloudkeys::random::<4>().map(|b| crate::cloudkeys::hex(&b)).unwrap_or_default();
        format!("{}-{}-{}", super::spec::label_value(user).chars().take(12).collect::<String>(), *n, r)
    }

    /// The command as a request: the person's best team (highest rank), the policy's CLI image, the proxy when allowed.
    pub fn spec_for(&self, policy: &SandboxPolicy, team: &str, user: &str, program: &str, args: &[String], seconds: u64) -> SandboxSpec {
        let labels = Labels { tenant: self.tenant.clone(), team: team.into(), person: user.into(), run_id: self.run_id(user) };
        let mut argv = vec![program.to_string()];
        argv.extend(args.iter().cloned());
        let mut spec = SandboxSpec::command(argv, &policy.cli_image, labels);
        spec.limits.seconds = (seconds as u32).clamp(1, policy.max.seconds.max(1));
        spec.limits.cpu_m = spec.limits.cpu_m.min(policy.max.cpu_m.max(1));
        spec.limits.mem_mib = spec.limits.mem_mib.min(policy.max.mem_mib.max(1));
        spec.limits.pids = spec.limits.pids.min(policy.max.pids.max(1));
        spec.limits.workspace_mib = spec.limits.workspace_mib.min(policy.max.workspace_mib.max(1));
        spec.network = if policy.network_modes.contains(&NetMode::Proxy) { NetMode::Proxy } else { NetMode::None };
        spec
    }
}

/// A short, stable name for one exact command, so an approval covers that command and no other.
pub fn approval_item(argv: &[String]) -> String {
    format!("cmd-{}", crate::tenantconfig::short_hash(argv.join("\u{1f}").as_bytes()))
}

impl CloudRunner for TenantRunner {
    fn run_command(&self, user: &str, program: &str, args: &[String], seconds: u64) -> Result<String, String> {
        let roles = crate::cloudroles::load(&crate::cloudroles::path(&self.dir)).ok().flatten().ok_or("the tenant's people lists cannot be read")?;
        roles.tenant_role(user).ok_or("you are not a member of this tenant")?;
        // the team the run is charged to: the one where the person has the highest rank
        let caller = |t: &str| crate::tenantconfig::api::Caller { user, roles: Some(&roles) }.rank(t);
        let team = roles.namespaces.keys().max_by_key(|t| (caller(t), std::cmp::Reverse((*t).clone()))).cloned().ok_or("you are not in a team")?;
        let rank = caller(&team);
        if rank < 0 {
            return Err("you are not in a team".into());
        }
        let env = Env { dir: &self.dir, vault: self.vault.clone(), audit: self.audit.as_ref() };
        let policy = SandboxPolicy::load(&env, &team);
        let spec = self.spec_for(&policy, &team, user, program, args, seconds);
        let role = ROLE_NAMES[(rank as usize).min(3)];
        let item = approval_item(&spec.argv);
        let now = crate::tenantconfig::now_secs();
        let approved = crate::tenantconfig::approvals::take_approval(&env, "sandbox.policy", &team, &item, user, now);
        let gate: Arc<dyn ApplyGate> = Arc::new(K3sGate { dir: self.dir.clone(), vault: self.vault.clone(), audit: self.audit.clone(), roles: roles.clone(), rank, guest: false, environment: policy.k3s_environment.clone() });
        let job = Arc::new(JobExecutor::new(self.kube.clone(), policy.clone(), gate, self.secrets.clone()));
        let audit: Arc<dyn ConfigAudit> = Arc::new(super::engine::FileAudit { dir: self.dir.clone(), inner: self.audit.clone() });
        let engine = Engine::new(policy, vec![job], audit);
        match engine.run(&spec, &Who { person: user, role }, approved.as_deref()) {
            Ok(r) => Ok(render(&r)),
            Err(SandboxError::NeedsApproval { reason, .. }) => {
                let p = crate::tenantconfig::approvals::request(&env, &roles, "sandbox.policy", &team, &item, user, false, false, now)?;
                Err(format!("{reason}. A request ({}) is waiting for an admin of team {team}; ask again after it is approved.", p.id))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// What the person or the model sees of a result.
pub fn render(r: &SandboxResult) -> String {
    let mut s = String::new();
    s.push_str(&r.stdout);
    if !r.stderr.is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&r.stderr);
    }
    let note = if r.oom_killed {
        "stopped: it used more than its memory limit".to_string()
    } else if r.timed_out {
        "stopped: it ran longer than its time limit".to_string()
    } else if let Some(sig) = r.signal {
        format!("stopped by signal {sig}")
    } else {
        match r.exit_code {
            Some(0) | None => String::new(),
            Some(c) => format!("exit status {c}"),
        }
    };
    if !note.is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&format!("[{note}]"));
    }
    if r.stdout_truncated || r.stderr_truncated {
        s.push_str("\n[output cut]");
    }
    if s.trim().is_empty() {
        "(no output, exit status 0)".into()
    } else {
        s
    }
}

// ------------------------------------------------------------------------------------------------------------------ flows

/// The `cli` node of a flow: its `ExecRequest` as a sandbox run. `image` is the node's, or the default tools image.
pub fn exec_spec(req: &iyakki_flow::host::ExecRequest, default_image: &str, labels: Labels) -> SandboxSpec {
    let image = if req.image.is_empty() { default_image } else { req.image.as_str() };
    let mut spec = SandboxSpec::command(req.argv.clone(), image, labels);
    spec.env = req.env.iter().map(|(k, v)| super::spec::EnvVar { name: k.clone(), value: v.clone() }).collect();
    spec.limits.seconds = ((req.timeout_ms / 1000) + u64::from(req.timeout_ms % 1000 > 0)).clamp(1, 86_400) as u32;
    spec.workdir = if req.cwd.is_empty() { String::new() } else { format!("/workspace/{}", req.cwd.trim_start_matches('/')) };
    spec.network = NetMode::Proxy;
    spec.tier = Some(Tier::Job);
    spec
}

/// Run a flow `cli` node in the sandbox and answer in the flow engine's own terms.
pub fn exec_via_sandbox(engine: &Engine, who: &Who, req: &iyakki_flow::host::ExecRequest, default_image: &str, labels: Labels, approved_by: Option<&str>) -> Result<iyakki_flow::host::ExecResult, iyakki_flow::NodeError> {
    let mut spec = exec_spec(req, default_image, labels);
    if !engine.policy.network_modes.contains(&NetMode::Proxy) {
        spec.network = NetMode::None;
    }
    match engine.run(&spec, who, approved_by) {
        Ok(r) => Ok(iyakki_flow::host::ExecResult { code: r.exit_code.or(r.signal.map(|s| 128 + s)), stdout: r.stdout, stderr: r.stderr, timed_out: r.timed_out, truncated: r.stdout_truncated || r.stderr_truncated, missing: false, ran_in: format!("sandbox {} {}", r.tier.id(), r.run_id) }),
        Err(SandboxError::Refused(m)) => Err(iyakki_flow::NodeError::config(m)),
        Err(SandboxError::NeedsApproval { reason, .. }) => Err(iyakki_flow::NodeError::config(format!("needs approval: {reason}"))),
        Err(SandboxError::QueueTimeout) => Err(iyakki_flow::NodeError::timeout("the sandbox queue timed out")),
        Err(SandboxError::Infra(m)) => Err(iyakki_flow::NodeError::new(m)),
    }
}

/// The `browser` node of a flow (target `sandbox`): a browser session in a pod of its own.
pub fn browser_spec(req: &iyakki_flow::host::BrowserRequest, image: &str, labels: Labels) -> SandboxSpec {
    let mut spec = SandboxSpec::browser(vec![], req.allow_domains.clone(), image, labels);
    if let Some(a) = req.steps.as_array().filter(|a| !a.is_empty()) {
        spec.inputs.push(super::spec::InputFile { name: "steps.json".into(), bytes: serde_json::to_vec(a).unwrap_or_default() });
    }
    spec.limits.seconds = ((req.timeout_ms / 1000).clamp(10, 3600)) as u32;
    spec
}

pub fn browser_via_sandbox(engine: &Engine, who: &Who, req: &iyakki_flow::host::BrowserRequest, labels: Labels) -> Result<iyakki_flow::host::BrowserResult, iyakki_flow::NodeError> {
    let spec = browser_spec(req, &engine.policy.browser_image, labels);
    match engine.run(&spec, who, None) {
        Ok(r) if r.ok() => Ok(iyakki_flow::host::BrowserResult { report: r.stdout, extracted: Default::default(), screenshots: r.artifacts.iter().filter(|a| a.path.ends_with(".png")).map(|a| a.path.clone()).collect(), url: String::new() }),
        Ok(r) => Err(iyakki_flow::NodeError { message: format!("the browser session failed: {}", r.stdout.lines().last().unwrap_or("")), timed_out: r.timed_out, ..Default::default() }),
        Err(SandboxError::Refused(m)) => Err(iyakki_flow::NodeError::config(m)),
        Err(SandboxError::NeedsApproval { reason, .. }) => Err(iyakki_flow::NodeError::config(format!("needs approval: {reason}"))),
        Err(SandboxError::QueueTimeout) => Err(iyakki_flow::NodeError::timeout("the sandbox queue timed out")),
        Err(SandboxError::Infra(m)) => Err(iyakki_flow::NodeError::new(m)),
    }
}

// ------------------------------------------------------------------------------------------------------------------ cloud flows

/// The seam with the cloud-flows scheduler (`cloudflows::job::JobSpec`): the same run as a sandbox request, so that the hardened
/// manifest of [`super::job`] (workspace, NetworkPolicy, ttl, runtime class) is the one builder. `secret_env` (references to
/// Kubernetes Secrets) has no place in a sandbox spec, which holds vault references only: such a run is refused here.
pub fn spec_from_flow_job(j: &crate::cloudflows::job::JobSpec, person: &str) -> Result<SandboxSpec, String> {
    if !j.secret_env.is_empty() {
        return Err("a Kubernetes Secret reference cannot be used in a sandbox run: use a vault reference".into());
    }
    let labels = Labels { tenant: j.tenant.clone(), team: j.team.clone(), person: person.into(), run_id: format!("{}-{}", j.flow, j.run) };
    let mut s = SandboxSpec::command(j.argv.clone(), &j.image, labels);
    s.env = j.env.iter().map(|(k, v)| super::spec::EnvVar { name: k.clone(), value: v.clone() }).collect();
    s.limits.cpu_m = j.cpu_m;
    s.limits.mem_mib = j.mem_mib;
    s.limits.seconds = j.deadline_secs.min(u64::from(u32::MAX)) as u32;
    s.network = NetMode::Proxy;
    s.tier = Some(Tier::Job);
    Ok(s)
}
