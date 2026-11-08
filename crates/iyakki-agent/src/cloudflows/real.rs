//! The wiring: the real [`Backend`] over the accounts' flow environments, the engine hook (lease gate, quotas, tenant time zone) and
//! the thread that calls the service every few seconds. Started by the cloud profile of the web service (`Extras::background_start`);
//! never on a desktop. Not exercised against a real pod yet (see the plan).

use super::approval::{Resolver, Waits};
use super::notify::NotifySink;
use super::quota::Quota;
use super::service::{Backend, Counters, RunInfo, TenantScheduler};
use crate::services::Services;
use iyakki_core_api::Authenticator;
use iyakki_flow::model::RunStatus;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const STEP_SECS: u64 = 5;

static STATUS: std::sync::OnceLock<Mutex<Option<serde_json::Value>>> = std::sync::OnceLock::new();

/// The latest status of the tenant scheduler in this process (what the status route shows); `None` before the first step.
pub fn last_status() -> Option<serde_json::Value> {
    STATUS.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// One line in the tenant's audit file for a schedule change (never a secret).
pub fn audit_line(data: &Path, who: &str, event: &str, detail: &str) {
    let clean: String = detail.chars().filter(|c| !c.is_control()).take(400).collect();
    let _ = crate::cloudroles::append_audit(&crate::cloudroles::audit_path(data), iyakki_schema::now_secs(), &crate::cloudroles::Change(format!("{event} by={who} {clean}")));
}

pub struct RealBackend {
    pub svc: Arc<Services>,
    pub auth: Arc<dyn Authenticator>,
}

impl RealBackend {
    fn env(&self, user: &str) -> Result<Arc<crate::flows::FlowEnv>, String> {
        let p = self.auth.lookup(user).ok_or_else(|| format!("no account {user}"))?;
        crate::flows::FlowEnv::open(&self.svc, user, &p.policy)
    }
}

impl Backend for RealBackend {
    fn accounts(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.svc.dir().join("flows")) else { return vec![] };
        let mut v: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(String::from))
            .filter(|u| iyakki_sec::policy::valid_username(u) && self.auth.lookup(u).is_some())
            .filter(|u| std::fs::read_dir(crate::scope::home_root(u, self.svc.dir()).join("Flows")).map(|rd| rd.flatten().any(|e| e.path().join("flow.json").is_file())).unwrap_or(false))
            .collect();
        v.sort();
        v
    }

    fn start(&self, user: &str) -> Result<(), String> {
        self.env(user).map(|_| ())
    }

    fn stop_all(&self) {
        crate::flows::halt_all();
    }

    fn runs(&self, user: &str) -> Vec<RunInfo> {
        let Ok(env) = self.env(user) else { return vec![] };
        let mut all = env.engine.summaries();
        all.sort_by_key(|r| std::cmp::Reverse(r.created_ms));
        all.truncate(60);
        all.into_iter()
            .map(|r| {
                let (due_ms, missed) = if r.parent.is_none() { env.engine.get_run(&r.id).map_or((None, false), |x| (x.trigger.due_ms, x.trigger.detail.contains("missed"))) } else { (None, false) };
                RunInfo { scheduled: !matches!(r.trigger.as_str(), "manual" | "chat" | "api" | "subflow" | "event"), status: status_word(r.status).into(), id: r.id, flow: r.flow_id, created_ms: r.created_ms, started_ms: r.started_ms, due_ms, missed, error: r.error, blocked: r.blocked }
            })
            .collect()
    }

    fn cancel(&self, user: &str, run: &str) -> Result<(), String> {
        self.env(user)?.engine.cancel(run)
    }

    fn pause(&self, user: &str, flow: &str, paused: bool) {
        if let Ok(e) = self.env(user) {
            e.engine.pause_schedule(flow, paused);
        }
    }
}

fn status_word(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Scheduled => "scheduled",
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Blocked => "blocked",
        RunStatus::Retrying => "retrying",
        RunStatus::Healing => "healing",
        RunStatus::Failed => "failed",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Cancelled => "cancelled",
    }
}

impl Resolver for RealBackend {
    fn decide(&self, user: &str, run: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String> {
        self.env(user)?.engine.decide(run, node, approved, by, note)
    }
    fn fail(&self, user: &str, run: &str, node: &str, reason: &str) -> Result<(), String> {
        // a node that must fail: reject it (the node's own reject path ends the run as failed unless the flow handles it)
        self.env(user)?.engine.decide(run, node, false, "timeout", reason)
    }
}

/// Install the engine hook once: the gate follows `leading`, quotas are re-read every 30 s, `local` means the tenant's zone.
pub fn install_hook(data: &Path, leading: Arc<AtomicBool>, counters: Arc<Counters>) {
    let data = data.to_path_buf();
    let cache: Arc<Mutex<(Instant, Quota)>> = Arc::new(Mutex::new((Instant::now() - Duration::from_secs(3600), Quota::default())));
    let quota = {
        let (data, cache) = (data.clone(), cache.clone());
        move || -> Quota {
            let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
            if g.0.elapsed() > Duration::from_secs(30) {
                *g = (Instant::now(), Quota::load(&data, None));
            }
            g.1.clone()
        }
    };
    let last_refusal = Arc::new(AtomicU64::new(0));
    let hook: crate::flows::CloudHook = Arc::new(move |_user: &str, cfg: &mut iyakki_flow::engine::EngineCfg| {
        let q = quota();
        cfg.max_runs = q.max_runs_person;
        cfg.local_tz = Some(tenant_zone(&data));
        // finished runs are pruned by the tenant's history_keep and history_days (asked again at every pass)
        cfg.retention = Some(iyakki_flow::engine::RetentionSource({
            let q = quota.clone();
            Arc::new(move || {
                let q = q();
                iyakki_flow::runstore::Retention { keep: q.history_keep, days: q.history_days }
            })
        }));
        let (lead, counters, last, q2) = (leading.clone(), counters.clone(), last_refusal.clone(), quota.clone());
        cfg.gate = Some(iyakki_flow::engine::Gate {
            lead: Arc::new(move || lead.load(Ordering::Relaxed)),
            admit: Arc::new(move |_flow: &str, started: usize| {
                let q = q2();
                let running = crate::flows::running_all().unwrap_or(0) + started;
                let ok = running < q.max_runs_tenant;
                if !ok {
                    let now = iyakki_schema::now_secs();
                    if now > last.swap(now, Ordering::Relaxed) + 10 {
                        counters.quota_refused.fetch_add(1, Ordering::Relaxed);
                    }
                }
                ok
            }),
        });
    });
    let _ = crate::flows::CLOUD_HOOK.set(hook);
}

/// The tenant's `schedule.timezone` (default UTC).
pub fn tenant_zone(data: &Path) -> String {
    use crate::tenantconfig::{resolve::resolve, schema, store::Store};
    let Some(spec) = schema::kind("schedule.timezone") else { return "UTC".into() };
    let Ok(cfg) = Store::new(data).load() else { return "UTC".into() };
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "schedule.timezone", None, "default"));
    if eff.exists && eff.enabled { eff.values.get("timezone").and_then(|v| v.as_str()).unwrap_or("UTC").to_string() } else { "UTC".into() }
}

struct AuditShim(Arc<dyn iyakki_core_api::AuditSink>);
impl crate::tenantconfig::api::ConfigAudit for AuditShim {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.record(who, event, detail);
    }
}

/// Start the tenant scheduler thread; it stops (and hands the lease over) when `stop` is set.
pub fn start(svc: Arc<Services>, auth: Arc<dyn Authenticator>, audit: Arc<dyn iyakki_core_api::AuditSink>, stop: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
    let data: PathBuf = svc.dir().to_path_buf();
    let tenant = crate::notifyhub::service::hub_for(&svc.dir().to_path_buf()).tenant.clone();
    let holder = format!("{}-{}", std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "pod".into()), std::process::id());
    let flows_dir = super::dir(&data);
    // the tenant's notification hub: durable, deduplicated, with the tenant's channels; its delivery worker runs here, on its own lease
    let hub = crate::notifyhub::service::hub_for(&data);
    let sink: Arc<dyn NotifySink> = hub.clone();
    let mut sched = TenantScheduler::new(&data, &tenant, &holder, super::lease::DEFAULT_TTL_SECS, STEP_SECS, sink.clone());
    {
        // a flow's `notify` step in the cloud goes to the hub (a step that waits for an approval is announced by the approvals file)
        let (sink, tenant, data2) = (sink.clone(), tenant.clone(), data.clone());
        let hook: crate::flows::CloudNotify = Arc::new(move |user: &str, n: &iyakki_flow::host::Notice, to: &serde_json::Value| {
            if n.needs_you {
                return;
            }
            let team = crate::cloudroles::load(&crate::cloudroles::path(&data2)).ok().flatten().and_then(|r| r.namespaces.iter().find(|(_, m)| m.contains_key(user)).map(|(t, _)| t.clone())).unwrap_or_default();
            let _ = sink.emit(&super::notify::from_node_notice_to(&tenant, &team, user, n, to));
        });
        let _ = crate::flows::CLOUD_NOTIFY.set(hook);
    }
    let leading = Arc::new(AtomicBool::new(false));
    install_hook(&data, leading.clone(), sched.counters.clone());
    {
        // a run that nobody started by hand reads only the secrets a team admin marked for scheduled runs
        let (data2, flows2) = (data.clone(), flows_dir.clone());
        let hook: crate::flows::CloudSecret = Arc::new(move |user: &str, run: &iyakki_flow::host::RunSecretCx, name: &str| {
            let vault = crate::tenantconfig::vault::for_dir(&data2);
            super::identity::unattended_secret(&data2, vault.as_ref(), &super::identity::Registry::new(&flows2), user, run, name)
        });
        let _ = crate::flows::CLOUD_SECRET.set(hook);
    }
    // the sandbox: without it a tenant pod refuses commands (fail closed); with it `run_command` and a flow's `cli` and `browser` steps
    // go to a hardened Job on the team's cluster binding
    {
        use crate::sandbox::{exec::SecretResolver, kube::{KubeApi, Kubectl}, wiring::TenantRunner};
        let vault = crate::tenantconfig::vault::for_dir(&data);
        let cfg_audit: Arc<dyn crate::tenantconfig::api::ConfigAudit> = Arc::new(AuditShim(audit.clone()));
        let kube: Arc<dyn KubeApi> = Arc::new(Kubectl::new(None));
        let secrets: Arc<dyn SecretResolver> = Arc::new(vault.clone());
        crate::sandbox::wiring::install(Arc::new(TenantRunner { dir: data.clone(), tenant: tenant.clone(), kube: kube.clone(), vault: vault.clone(), audit: cfg_audit.clone(), secrets: secrets.clone(), counter: Mutex::new(0) }));
        let fs = Arc::new(super::sandboxed::FlowSandbox { dir: data.clone(), tenant: tenant.clone(), kube, vault, audit: cfg_audit, secrets });
        let (f1, f2) = (fs.clone(), fs);
        let _ = crate::flows::CLOUD_EXEC.set(Arc::new(move |u: &str, r: &iyakki_flow::host::ExecRequest| f1.exec(u, r)));
        let _ = crate::flows::CLOUD_BROWSER.set(Arc::new(move |u: &str, r: &iyakki_flow::host::BrowserRequest| f2.browser(u, r)));
    }
    let backend = Arc::new(RealBackend { svc: svc.clone(), auth });
    let waits = Waits::new(&flows_dir);
    let net = crate::notifyhub::transport::RealNet { dir: data.clone(), ca_files: vec![] };
    let mut ticks = 0u64;
    std::thread::Builder::new()
        .name("cloud-flows".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let now = iyakki_schema::now_secs();
                let roles = crate::cloudroles::load(&crate::cloudroles::path(&data)).ok().flatten().unwrap_or_default();
                let q = Quota::load(&data, None);
                let rep = sched.step(now, backend.as_ref(), &roles, &q);
                leading.store(sched.is_leader(), Ordering::Relaxed);
                *STATUS.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()) = Some(sched.status(now));
                if sched.is_leader() {
                    let shim = AuditShim(audit.clone());
                    let env = crate::tenantconfig::api::Env { dir: &data, vault: crate::tenantconfig::vault::for_user(&data, "_flows"), audit: &shim };
                    scan_new_waits(&waits, &env, &roles, &tenant, backend.as_ref(), &sched, now);
                    for l in waits.poll(&env, &roles, sink.as_ref(), backend.as_ref(), now) {
                        audit.record("_flows", "flow.approval.resolved", &l);
                    }
                }
                ticks += 1;
                if ticks % 2 == 0 {
                    hub.tick(now, &net);
                }
                for p in rep.paused {
                    audit.record("_flows", "flow.schedule.paused", &format!("{p} owner lost access"));
                }
                for _ in 0..STEP_SECS * 10 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            sched.shutdown(backend.as_ref());
        })
        .ok()
}

/// A blocked run with an `approval` node that has no request yet gets one in the router. Where the flow belongs to no team the owner
/// is in, or nobody can approve, the step is rejected (default deny).
#[allow(clippy::too_many_arguments)]
fn scan_new_waits(waits: &Waits, env: &crate::tenantconfig::api::Env, roles: &crate::cloudroles::Roles, tenant: &str, b: &RealBackend, sched: &TenantScheduler, now: u64) {
    let known: Vec<(String, String)> = waits.waiting().into_iter().map(|w| (w.run, w.node)).collect();
    for user in b.accounts() {
        let Ok(fe) = b.env(&user) else { continue };
        for r in b.runs(&user).into_iter().filter(|r| r.status == "blocked") {
            let Some(run) = fe.engine.get_run(&r.id) else { continue };
            for (node, nr) in run.nodes.iter().filter(|(_, n)| n.state == iyakki_flow::model::NodeState::Waiting && n.waiting.starts_with("approval:")) {
                if known.iter().any(|(rr, nn)| *rr == r.id && nn == node) {
                    continue;
                }
                let mut parts = nr.waiting.splitn(3, ':');
                let (_, on_timeout, message) = (parts.next(), parts.next().unwrap_or("reject"), parts.next().unwrap_or(""));
                let timeout_s = nr.wake_ms.map(|w| ((w - now as i64 * 1000).max(0) / 1000) as u64);
                let team = sched.registry.get(&user, &r.flow).map(|s| s.team).or_else(|| roles.namespaces.iter().find(|(_, m)| m.contains_key(&user)).map(|(t, _)| t.clone())).unwrap_or_default();
                if team.is_empty() || waits.file_wait(env, roles, tenant, &team, &user, &r.flow, &r.id, node, message, timeout_s, on_timeout, now).is_err() {
                    let _ = fe.engine.decide(&r.id, node, false, "system", "nobody can approve this step");
                }
            }
        }
    }
}
