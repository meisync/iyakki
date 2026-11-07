//! The executors: one trait, three implementations (`JobExecutor` for tiers `job` and `browser`, `LocalNsExecutor`, `WasmExecutor`).
//! An executor takes only an [`Admitted`], so nothing here re-checks policy; it carries out what admission allowed and reports
//! what happened. Redaction, caps and audit are the facade's ([`super::engine`]).

use super::admit::{image_registry, Admitted};
use super::job;
use super::kube::KubeApi;
use super::policy::SandboxPolicy;
use super::spec::{label_value, Delivery, Hidden, RawOutcome, SandboxError, Tier, Usage};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub trait Executor: Send + Sync {
    fn tier(&self) -> Tier;
    /// Whether this executor carries out runs of tier `t` (the Job executor serves the job and browser tiers).
    fn handles(&self, t: Tier) -> bool {
        t == self.tier()
    }
    fn run(&self, adm: &Admitted) -> Result<RawOutcome, SandboxError>;
}

// ------------------------------------------------------------------------------------------------------------------ secrets, gate

/// Resolves a vault reference at the last moment. The real one is the tenant vault's "use a secret for a named action".
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret_ref: &str, action: &str) -> Result<zeroize::Zeroizing<String>, String>;
}

impl SecretResolver for Arc<dyn crate::tenantconfig::vault::SecretStore> {
    fn resolve(&self, secret_ref: &str, action: &str) -> Result<zeroize::Zeroizing<String>, String> {
        let r = crate::tenantconfig::schema::SecretRef::parse(secret_ref).ok_or("not a vault reference")?;
        let mut got: Option<zeroize::Zeroizing<String>> = None;
        self.use_secret(&r, action, &mut |v| {
            got = Some(zeroize::Zeroizing::new(v.to_string()));
            Ok(String::new())
        })?;
        got.ok_or_else(|| "the vault gave no value".to_string())
    }
}

/// Rules for the proxy: "for this run, requests to `host` get the secret in a header". The egress proxy has no run-scoped control
/// interface yet (F-SEC-04 reads its rules from its own configuration), so [`NoInjector`] refuses.
pub trait Injector: Send + Sync {
    fn register(&self, run: &str, name: &str, host: &str, secret_ref: &str) -> Result<(), String>;
    fn unregister(&self, run: &str);
}

pub struct NoInjector;
impl Injector for NoInjector {
    fn register(&self, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("injecting a secret at the proxy is not wired yet: deliver it as a file".into())
    }
    fn unregister(&self, _: &str) {}
}

/// Last check before anything is applied to the cluster: the team's cluster binding (`k3s_decide`).
pub trait ApplyGate: Send + Sync {
    fn allow(&self, adm: &Admitted, namespace: &str) -> Result<(), SandboxError>;
}

pub struct OpenGate;
impl ApplyGate for OpenGate {
    fn allow(&self, _: &Admitted, _: &str) -> Result<(), SandboxError> {
        Ok(())
    }
}

/// `k3s_decide` of the team's binding for the policy's environment (first of the chain, so no promotion is asked).
pub struct K3sGate {
    pub dir: std::path::PathBuf,
    pub vault: Arc<dyn crate::tenantconfig::vault::SecretStore>,
    pub audit: Arc<dyn crate::tenantconfig::api::ConfigAudit>,
    pub roles: crate::cloudroles::Roles,
    pub rank: i32,
    pub guest: bool,
    pub environment: String,
}

impl ApplyGate for K3sGate {
    fn allow(&self, adm: &Admitted, _ns: &str) -> Result<(), SandboxError> {
        use crate::tenantconfig::enforce::{k3s_decide, Deploy, DeployAsk};
        let env = crate::tenantconfig::api::Env { dir: &self.dir, vault: self.vault.clone(), audit: self.audit.as_ref() };
        let spec = adm.spec();
        let reg = image_registry(&if spec.image.is_empty() { String::new() } else { spec.image.clone() });
        let ask = DeployAsk {
            team: &spec.labels.team,
            environment: &self.environment,
            requester: &spec.labels.person,
            requester_rank: self.rank,
            approval: adm.approved_by(),
            promoted_from: None,
            registry: if spec.image.is_empty() { None } else { Some(reg.as_str()) },
            roles: &self.roles,
            guest: self.guest,
        };
        match k3s_decide(&env, &ask) {
            Deploy::Allow => Ok(()),
            Deploy::NeedsApproval { approver_role } => Err(SandboxError::NeedsApproval { reason: "the team's cluster binding asks for approval before anything is applied".into(), approver_role }),
            Deploy::Deny(why) => Err(SandboxError::Refused(format!("the team's cluster binding does not allow it: {why}"))),
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ job

pub struct JobExecutor {
    pub kube: Arc<dyn KubeApi>,
    pub policy: SandboxPolicy,
    pub gate: Arc<dyn ApplyGate>,
    pub secrets: Arc<dyn SecretResolver>,
    pub injector: Arc<dyn Injector>,
    pub poll: Duration,
    /// How long after the run's own time limit the executor keeps waiting (image pull, scheduling).
    pub grace: Duration,
    /// An image that cannot start is reported after this long.
    pub image_fail_after: Duration,
}

impl JobExecutor {
    pub fn new(kube: Arc<dyn KubeApi>, policy: SandboxPolicy, gate: Arc<dyn ApplyGate>, secrets: Arc<dyn SecretResolver>) -> JobExecutor {
        JobExecutor { kube, policy, gate, secrets, injector: Arc::new(NoInjector), poll: Duration::from_millis(1500), grace: Duration::from_secs(240), image_fail_after: Duration::from_secs(20) }
    }

    /// The namespace the Jobs run in.
    pub fn namespace(&self, adm: &Admitted) -> String {
        if self.policy.namespace.is_empty() {
            format!("sbx-{}", label_value(&adm.spec().labels.tenant))
        } else {
            self.policy.namespace.clone()
        }
    }
}

struct Cleanup<'a> {
    kube: &'a dyn KubeApi,
    inj: &'a dyn Injector,
    ns: String,
    run: String,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.kube.delete_run(&self.ns, &self.run);
        self.inj.unregister(&self.run);
    }
}

impl Executor for JobExecutor {
    fn tier(&self) -> Tier {
        Tier::Job
    }

    fn handles(&self, t: Tier) -> bool {
        matches!(t, Tier::Job | Tier::Browser)
    }

    fn run(&self, adm: &Admitted) -> Result<RawOutcome, SandboxError> {
        if !matches!(adm.tier(), Tier::Job | Tier::Browser) {
            return Err(SandboxError::Refused("the job executor runs the job and browser tiers only".into()));
        }
        let ns = self.namespace(adm);
        // the team's cluster binding decides before anything is applied
        self.gate.allow(adm, &ns)?;
        let spec = adm.spec();
        let name = adm.run_name();
        let _cleanup = Cleanup { kube: self.kube.as_ref(), inj: self.injector.as_ref(), ns: ns.clone(), run: name.clone() };
        let infra = |e: String| SandboxError::Infra(e);
        // secrets: by proxy rule (the run never holds them) or as a file in a Secret that lives as long as the run
        let mut held: Vec<String> = spec.env.iter().map(|e| e.value.clone()).collect();
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        for s in &spec.secrets {
            match &s.delivery {
                Delivery::ProxyInject { host } => self.injector.register(&name, &s.name, host, &s.secret_ref).map_err(|e| SandboxError::Refused(e))?,
                Delivery::TmpfsFile => {
                    let v = self.secrets.resolve(&s.secret_ref, "sandbox.run").map_err(|e| SandboxError::Refused(format!("secret {} cannot be used: {e}", s.name)))?;
                    held.push(v.to_string());
                    files.insert(s.name.clone(), v.to_string());
                }
            }
        }
        let mut objects = job::bundle(adm, &self.policy, &ns);
        let job_obj = objects.pop().ok_or_else(|| infra("no Job".into()))?;
        for o in &objects {
            self.kube.apply(o).map_err(infra)?;
        }
        if !files.is_empty() {
            self.kube.apply(&job::secret_object(adm, &ns, &files)).map_err(infra)?;
        }
        self.kube.apply(&job_obj).map_err(infra)?;

        let seconds = if adm.tier() == Tier::Browser { spec.limits.seconds.min(self.policy.browser_session_secs) } else { spec.limits.seconds };
        let started = Instant::now();
        let give_up = started + Duration::from_secs(u64::from(seconds)) + self.grace;
        let mut timed_out = false;
        let mut errors = 0;
        let state = loop {
            match self.kube.state(&ns, &name) {
                Ok(st) => {
                    errors = 0;
                    if st.succeeded || st.failed {
                        break st;
                    }
                    if st.waiting_reason.as_deref().is_some_and(|r| matches!(r, "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName" | "CreateContainerConfigError" | "CreateContainerError")) && started.elapsed() > self.image_fail_after {
                        return Err(infra(format!("the image could not be started ({})", st.waiting_reason.unwrap_or_default())));
                    }
                    if st.deadline_exceeded {
                        break st;
                    }
                }
                Err(e) => {
                    errors += 1;
                    if errors >= 5 {
                        return Err(infra(format!("the cluster could not be asked: {e}")));
                    }
                }
            }
            if Instant::now() > give_up {
                timed_out = true;
                break Default::default();
            }
            std::thread::sleep(self.poll);
        };
        let cap = spec.limits.output_kib as usize * 1024 * 2;
        let logs = self.kube.logs(&ns, &name, cap).unwrap_or_default();
        let artifacts = self.kube.artifacts(&ns, &name).unwrap_or_default();
        let code = if state.succeeded { Some(0) } else { state.exit_code };
        let (exit_code, signal) = match code {
            Some(c) if (129..=159).contains(&c) => (None, Some(c - 128)),
            other => (other, None),
        };
        Ok(RawOutcome {
            held: Hidden(held),
            exit_code,
            signal,
            timed_out: timed_out || state.deadline_exceeded,
            oom_killed: state.oom_killed,
            stdout: logs,
            stderr: vec![],
            artifacts,
            usage: Usage { wall_ms: started.elapsed().as_millis() as u64, max_mem_mib: state.max_mem_mib, ..Default::default() },
        })
    }
}

// ------------------------------------------------------------------------------------------------------------------ namespace (T1)

/// What the namespace sandbox printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NsOutput {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Starts `iyakki linuxapp-init <args>` (the program is this one) and waits for it.
pub trait NsLauncher: Send + Sync {
    fn launch(&self, args: &[String], timeout: Duration, cap: usize) -> Result<NsOutput, String>;
}

/// T1: user, mount, pid, uts, ipc and net namespaces inside the pod, with `linuxapps_init`'s limits, capability drop, no_new_privs and
/// seccomp deny-list. No network (it would be the pod's), no secrets. The tenant admin has opted in (`userns_ack`).
pub struct LocalNsExecutor {
    pub launcher: Arc<dyn NsLauncher>,
    /// The root folder the sandbox pivots into (an unpacked image, read-only).
    pub rootfs: std::path::PathBuf,
    /// Where per-run workspaces are made (removed after the run).
    pub scratch: std::path::PathBuf,
}

impl LocalNsExecutor {
    /// The argument list for one run (pure, so tests can read it).
    pub fn plan(&self, adm: &Admitted, workspace: &std::path::Path, inputs: Option<&std::path::Path>) -> crate::linuxapps_init::Spec {
        use crate::linuxapps_init::{Bind, IdMap, Spec};
        let s = adm.spec();
        let name: String = adm.run_name().chars().take(40).collect::<String>().trim_end_matches('-').to_string();
        let mut sp = Spec::new(&self.rootfs.display().to_string(), &name, s.argv.clone());
        sp.uid = job::RUN_UID as u32;
        sp.gid = job::RUN_UID as u32;
        sp.idmap = IdMap::Single;
        sp.net = false;
        sp.binds = vec![Bind { src: workspace.display().to_string(), dst: "/workspace".into(), rw: true }];
        if let Some(i) = inputs {
            sp.binds.push(Bind { src: i.display().to_string(), dst: "/inputs".into(), rw: false });
        }
        sp.tmp_mb = 64;
        sp.cwd = s.workdir_or_default().to_string();
        sp.env = s.env.iter().map(|e| (e.name.clone(), e.value.clone())).collect();
        sp.env.push(("HOME".into(), "/workspace".into()));
        sp.mem_mb = s.limits.mem_mib;
        sp.cpu_pct = (s.limits.cpu_m / 10).max(1);
        sp.pids = s.limits.pids;
        sp.seccomp = true;
        sp
    }
}

impl Executor for LocalNsExecutor {
    fn tier(&self) -> Tier {
        Tier::Namespace
    }

    fn run(&self, adm: &Admitted) -> Result<RawOutcome, SandboxError> {
        if adm.tier() != Tier::Namespace {
            return Err(SandboxError::Refused("the namespace executor runs the namespace tier only".into()));
        }
        let s = adm.spec();
        let dir = self.scratch.join(adm.run_name());
        let ws = dir.join("workspace");
        let inp = dir.join("inputs");
        let make = |p: &std::path::Path| std::fs::create_dir_all(p).map_err(|e| SandboxError::Infra(format!("{}: {e}", p.display())));
        make(&ws)?;
        let inputs = if s.inputs.is_empty() {
            None
        } else {
            make(&inp)?;
            for f in &s.inputs {
                std::fs::write(inp.join(&f.name), &f.bytes).map_err(|e| SandboxError::Infra(e.to_string()))?;
            }
            Some(inp.as_path())
        };
        let sp = self.plan(adm, &ws, inputs);
        sp.validate().map_err(SandboxError::Refused)?;
        let started = Instant::now();
        let r = self.launcher.launch(&sp.to_args(), Duration::from_secs(u64::from(s.limits.seconds)), s.limits.output_kib as usize * 1024 * 2);
        let _ = std::fs::remove_dir_all(&dir);
        let o = r.map_err(SandboxError::Infra)?;
        Ok(RawOutcome {
            held: Hidden(s.env.iter().map(|e| e.value.clone()).collect()),
            exit_code: o.code,
            signal: o.signal,
            timed_out: o.timed_out,
            oom_killed: o.signal == Some(9) && !o.timed_out && s.limits.mem_mib > 0 && String::from_utf8_lossy(&o.stderr).to_ascii_lowercase().contains("memory"),
            stdout: o.stdout,
            stderr: o.stderr,
            artifacts: vec![],
            usage: Usage { wall_ms: started.elapsed().as_millis() as u64, ..Default::default() },
        })
    }
}

/// The real launcher: this program with the `linuxapp-init` subcommand.
pub struct SelfLauncher;

impl NsLauncher for SelfLauncher {
    fn launch(&self, args: &[String], timeout: Duration, cap: usize) -> Result<NsOutput, String> {
        use std::io::Read;
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe).args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        let (mut so, mut se) = (child.stdout.take().ok_or("no pipe")?, child.stderr.take().ok_or("no pipe")?);
        let t1 = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut so).take(cap as u64).read_to_end(&mut b);
            b
        });
        let t2 = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut se).take(cap as u64).read_to_end(&mut b);
            b
        });
        let end = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() > end => {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().map_err(|e| e.to_string())?;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(e.to_string()),
            }
        };
        Ok(NsOutput { code: status.code(), signal: status.signal(), timed_out, stdout: t1.join().unwrap_or_default(), stderr: t2.join().unwrap_or_default() })
    }
}

// ------------------------------------------------------------------------------------------------------------------ wasm (T0)

/// T0: a module run by iyakki-wasm. Fuel from CPU share and time, memory from the limit, wall time from the limit; no files, no network,
/// no capabilities at all (a tool that needs files or hosts is a `pkg_` tool with a manifest, not a sandbox run).
pub struct WasmExecutor;

impl WasmExecutor {
    /// Instructions a run may execute: 100 000 per millisecond of CPU share.
    pub fn fuel_for(cpu_m: u32, seconds: u32) -> u64 {
        u64::from(cpu_m) * u64::from(seconds) * 100
    }
}

impl Executor for WasmExecutor {
    fn tier(&self) -> Tier {
        Tier::Wasm
    }

    fn run(&self, adm: &Admitted) -> Result<RawOutcome, SandboxError> {
        use iyakki_sec::CapabilityStore;
        use iyakki_vfs::MemFs;
        use iyakki_wasm::{Sandbox, SandboxConfig, WasmError};
        use std::sync::Mutex;
        if adm.tier() != Tier::Wasm {
            return Err(SandboxError::Refused("the wasm executor runs the wasm tier only".into()));
        }
        let s = adm.spec();
        let fuel = Self::fuel_for(s.limits.cpu_m, s.limits.seconds);
        let sb = Sandbox::new(SandboxConfig { fuel, max_memory_bytes: (s.limits.mem_mib as usize) << 20 }).map_err(|e| SandboxError::Infra(e.to_string()))?.with_wall_time(Duration::from_secs(u64::from(s.limits.seconds)));
        let vfs = Arc::new(Mutex::new(MemFs::with_quota((s.limits.workspace_mib as usize) << 20)));
        let entry = s.argv.first().map(String::as_str).filter(|e| !e.is_empty()).unwrap_or("_start");
        let started = Instant::now();
        let r = sb.run(&s.wasm, entry, vfs, CapabilityStore::new(), vec![]);
        let wall_ms = started.elapsed().as_millis() as u64;
        let mut o = RawOutcome { usage: Usage { wall_ms, ..Default::default() }, ..Default::default() };
        match r {
            Ok(out) => {
                o.exit_code = Some(out.result.unwrap_or(0));
                o.stdout = out.log;
                o.usage.fuel_used = out.fuel_used;
            }
            Err(WasmError::OutOfFuel) => {
                o.timed_out = true;
                o.stderr = b"stopped: the module used up its CPU budget".to_vec();
            }
            Err(WasmError::Timeout) => {
                o.timed_out = true;
                o.stderr = b"stopped: the module ran longer than its time limit".to_vec();
            }
            Err(WasmError::Trap(m)) => {
                o.exit_code = Some(1);
                o.oom_killed = m.to_ascii_lowercase().contains("memory");
                o.stderr = format!("trap: {m}").into_bytes();
            }
            Err(WasmError::Load(m)) => return Err(SandboxError::Refused(format!("the module cannot be loaded: {m}"))),
            Err(WasmError::NoEntry(e)) => return Err(SandboxError::Refused(format!("the module has no entry `{e}`"))),
        }
        Ok(o)
    }
}
