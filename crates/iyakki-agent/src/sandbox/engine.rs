//! The facade: admit, wait for a slot, run, clean, audit. One [`Engine`] per tenant pod (its policy is read at start and again when
//! the configuration changes: build a new one).

use super::admit::{admit, Who};
use super::exec::Executor;
use super::limiter::Limiter;
use super::output::clean;
use super::policy::SandboxPolicy;
use super::spec::{Artifact, SandboxError, SandboxResult, SandboxSpec};
use crate::tenantconfig::api::ConfigAudit;
use std::sync::Arc;
use std::time::Duration;

pub const MAX_ARTIFACTS: usize = 20;
pub const MAX_ARTIFACT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ARTIFACTS_TOTAL: usize = 4 * 1024 * 1024;

pub struct Engine {
    pub policy: SandboxPolicy,
    executors: Vec<Arc<dyn Executor>>,
    limiter: Arc<Limiter>,
    audit: Arc<dyn ConfigAudit>,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    tick: Duration,
}

/// Audit lines go to the tenant's own audit file as well as the given sink.
pub struct FileAudit {
    pub dir: std::path::PathBuf,
    pub inner: Arc<dyn ConfigAudit>,
}

impl ConfigAudit for FileAudit {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.inner.record(who, event, detail);
        let _ = crate::cloudroles::append_audit(&crate::cloudroles::audit_path(&self.dir), crate::tenantconfig::now_secs(), &crate::cloudroles::Change(format!("{event} by={who} {detail}")));
    }
}

fn program_of(spec: &SandboxSpec) -> String {
    spec.argv.first().map(|a| a.rsplit('/').next().unwrap_or(a).chars().filter(|c| !c.is_control() && !c.is_whitespace()).take(40).collect()).unwrap_or_default()
}

impl Engine {
    pub fn new(policy: SandboxPolicy, executors: Vec<Arc<dyn Executor>>, audit: Arc<dyn ConfigAudit>) -> Engine {
        let limiter = Limiter::new(policy.per_person, policy.per_team, policy.queue_timeout_secs);
        Engine { policy, executors, limiter, audit, now: Arc::new(crate::tenantconfig::now_secs), tick: Duration::from_millis(100) }
    }

    /// Tests set the clock and the look interval.
    pub fn with_clock(mut self, now: Arc<dyn Fn() -> u64 + Send + Sync>, tick: Duration) -> Engine {
        self.now = now;
        self.tick = tick;
        self
    }

    pub fn limiter(&self) -> &Arc<Limiter> {
        &self.limiter
    }

    /// `approved_by`: somebody the approvals router has accepted for exactly this request.
    pub fn run(&self, spec: &SandboxSpec, who: &Who, approved_by: Option<&str>) -> Result<SandboxResult, SandboxError> {
        let l = &spec.labels;
        let adm = match admit(&self.policy, spec, who, approved_by) {
            Ok(a) => a,
            Err(e) => {
                let (event, why) = match &e {
                    SandboxError::NeedsApproval { reason, .. } => ("sandbox.needs_approval", reason.clone()),
                    other => ("sandbox.refused", other.to_string()),
                };
                self.audit.record(who.person, event, &format!("run={} team={} program={} {why}", l.run_id, l.team, program_of(spec)));
                return Err(e);
            }
        };
        let tier = adm.tier();
        let Some(ex) = self.executors.iter().find(|e| e.handles(tier)) else {
            let m = format!("no executor is connected for the {} tier", tier.id());
            self.audit.record(who.person, "sandbox.refused", &format!("run={} {m}", l.run_id));
            return Err(SandboxError::Infra(m));
        };
        let permit = match self.limiter.acquire(&l.person, &*self.now, self.tick) {
            Ok(p) => p,
            Err(e) => {
                self.audit.record(who.person, "sandbox.refused", &format!("run={} team={} the queue timed out", l.run_id, l.team));
                return Err(e);
            }
        };
        self.audit.record(
            who.person,
            "sandbox.start",
            &format!(
                "run={} tenant={} team={} tier={} program={} args={} image={} pinned={} net={} cpu_m={} mem_mib={} seconds={} secrets={} approved_by={}",
                l.run_id,
                l.tenant,
                l.team,
                tier.id(),
                program_of(spec),
                spec.argv.len().saturating_sub(1),
                super::admit::image_registry(&spec.image),
                adm.digest_pinned(),
                spec.network.id(),
                spec.limits.cpu_m,
                spec.limits.mem_mib,
                spec.limits.seconds,
                spec.secrets.len(),
                adm.approved_by().unwrap_or("-")
            ),
        );
        let raw = ex.run(&adm);
        drop(permit);
        let raw = match raw {
            Ok(r) => r,
            Err(e) => {
                self.audit.record(who.person, "sandbox.end", &format!("run={} error={e}", l.run_id));
                return Err(e);
            }
        };
        let held = &raw.held.0;
        let out = clean(&raw.stdout, spec.limits.output_kib, held);
        let err = clean(&raw.stderr, spec.limits.output_kib, held);
        let mut artifacts = Vec::new();
        let mut files = Vec::new();
        let mut total = 0usize;
        for (path, bytes) in raw.artifacts.iter().take(MAX_ARTIFACTS * 5) {
            let keep = files.len() < MAX_ARTIFACTS && bytes.len() <= MAX_ARTIFACT_BYTES && total + bytes.len() <= MAX_ARTIFACTS_TOTAL;
            if keep {
                total += bytes.len();
                files.push((path.clone(), bytes.clone()));
            }
            artifacts.push(Artifact { path: path.clone(), size: bytes.len() as u64, sha256: crate::ocipush::sha256_hex(bytes), returned: keep });
        }
        let res = SandboxResult {
            run_id: l.run_id.clone(),
            tier,
            exit_code: raw.exit_code,
            signal: raw.signal,
            timed_out: raw.timed_out,
            oom_killed: raw.oom_killed,
            stdout: out.text,
            stderr: err.text,
            stdout_truncated: out.truncated,
            stderr_truncated: err.truncated,
            redacted: out.redacted || err.redacted,
            artifacts,
            files,
            usage: raw.usage,
        };
        self.audit.record(
            who.person,
            "sandbox.end",
            &format!(
                "run={} tier={} exit={} signal={} timed_out={} oom={} truncated={} redacted={} wall_ms={} artifacts={}",
                l.run_id,
                tier.id(),
                res.exit_code.map_or("-".to_string(), |c| c.to_string()),
                res.signal.map_or("-".to_string(), |c| c.to_string()),
                res.timed_out,
                res.oom_killed,
                res.stdout_truncated || res.stderr_truncated,
                res.redacted,
                res.usage.wall_ms,
                res.artifacts.len()
            ),
        );
        Ok(res)
    }
}
