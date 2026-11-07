//! Where a run executes. An [`Executor`] looks at a run request and says what happens: it runs in this process (with the quota's
//! limits), it becomes a Job on the team's cluster binding (manifest as text), it waits for an approval, or it is refused.
//!
//! The Job path asks the tenant configuration (`tenantconfig::enforce::k3s_decide`) exactly as a deploy does: dev, preprod and prod
//! rules, the promotion order, four eyes and the approval router apply to a scheduled run in an environment that needs approval. A
//! scheduled run never approves itself: it files a request (the owner is the requester) and waits; the next look finds the approval
//! (used once) or the request expired. Nothing here starts a cluster operation; applying the manifest is the cluster manager's job.

use super::job::{self, bundle_text, JobSpec};
use crate::sandbox::policy::SandboxPolicy;
use super::quota::{admit_run, Quota};
use crate::cloudroles::Roles;
use crate::tenantconfig::api::{Caller, Env};
use crate::tenantconfig::enforce::{k3s_approval_params, k3s_decide, Deploy, DeployAsk};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRequest {
    pub tenant: String,
    pub team: String,
    pub environment: String,
    /// The delegated identity: the owner of the schedule (never a session).
    pub owner: String,
    pub flow: String,
    pub run: String,
    pub image: String,
    pub argv: Vec<String>,
    pub tenant_running: usize,
    pub person_running: usize,
    /// The environment the artifact was last deployed to successfully (promotion order).
    pub promoted_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    InProcess { deadline_secs: u64 },
    Job { namespace: String, manifest: String },
    /// Waiting for an approval; the run stays queued and holds no worker.
    Wait { approval_id: String, approver_role: String },
    Refused { code: String, reason: String },
}

pub trait Executor {
    fn plan(&self, r: &RunRequest, now: u64) -> Decision;
}

pub struct InProcess {
    pub quota: Quota,
}

impl Executor for InProcess {
    fn plan(&self, r: &RunRequest, _now: u64) -> Decision {
        match admit_run(&self.quota, r.tenant_running, r.person_running) {
            Ok(()) => Decision::InProcess { deadline_secs: self.quota.max_run_secs },
            Err(f) => Decision::Refused { code: f.code.into(), reason: f.reason },
        }
    }
}

pub struct K3sJob<'a> {
    pub env: &'a Env<'a>,
    pub roles: &'a Roles,
    pub quota: Quota,
    pub cpu_m: u32,
    pub mem_mib: u32,
}

/// The namespace a team's environment lives in (a guess: the binding has no namespace field yet).
pub fn namespace_of(team: &str, environment: &str) -> String {
    format!("{team}-{environment}").to_ascii_lowercase().replace('_', "-")
}

fn dns(s: &str) -> String {
    s.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').chars().take(40).collect()
}

impl Executor for K3sJob<'_> {
    fn plan(&self, r: &RunRequest, now: u64) -> Decision {
        if let Err(f) = admit_run(&self.quota, r.tenant_running, r.person_running) {
            return Decision::Refused { code: f.code.into(), reason: f.reason };
        }
        let who = Caller { user: &r.owner, roles: Some(self.roles) };
        let guest = self.roles.tenant_role(&r.owner).is_none();
        let registry = job::registry_of(&r.image);
        let rank = who.rank(&r.team);
        fn ask<'a>(r: &'a RunRequest, rank: i32, registry: &'a str, roles: &'a Roles, guest: bool, approval: Option<&'a str>) -> DeployAsk<'a> {
            DeployAsk { team: &r.team, environment: &r.environment, requester: &r.owner, requester_rank: rank, approval, promoted_from: r.promoted_from.as_deref(), registry: Some(registry), roles, guest }
        }
        let mut d = k3s_decide(self.env, &ask(r, rank, &registry, self.roles, guest, None));
        if matches!(d, Deploy::NeedsApproval { .. }) {
            if let Some(by) = crate::tenantconfig::approvals::take_approval(self.env, "k3s.cluster", &r.team, &r.environment, &r.owner, now) {
                d = k3s_decide(self.env, &ask(r, rank, &registry, self.roles, guest, Some(&by)));
            }
        }
        match d {
            Deploy::Allow => {
                let spec = JobSpec {
                    name: format!("run-{}-{}", dns(&r.flow), dns(&r.run)).chars().take(63).collect::<String>().trim_end_matches('-').to_string(),
                    namespace: namespace_of(&r.team, &r.environment),
                    tenant: r.tenant.clone(),
                    team: r.team.clone(),
                    environment: r.environment.clone(),
                    flow: r.flow.clone(),
                    run: r.run.clone(),
                    image: r.image.clone(),
                    argv: r.argv.clone(),
                    env: vec![("IYAKKI_FLOW".into(), r.flow.clone()), ("IYAKKI_RUN".into(), r.run.clone())],
                    secret_env: vec![],
                    cpu_m: self.cpu_m,
                    mem_mib: self.mem_mib,
                    deadline_secs: self.quota.max_run_secs,
                    ttl_after_finish_secs: 3600,
                };
                // the tenant's sandbox policy decides what a Job may be (registries, limits, network, the role that may run it)
                let policy = SandboxPolicy::load(self.env, &r.team);
                let role = match rank {
                    3.. => "tenant_admin",
                    2 => "admin",
                    1 => "editor",
                    _ => "viewer",
                };
                let namespace = if policy.namespace.is_empty() { spec.namespace.clone() } else { policy.namespace.clone() };
                match bundle_text(&spec, &r.owner, role, &policy, &namespace) {
                    Ok(manifest) => Decision::Job { namespace, manifest },
                    Err(e) => Decision::Refused { code: "sandbox".into(), reason: e },
                }
            }
            Deploy::NeedsApproval { approver_role } => {
                let (tenant_only, role_only) = k3s_approval_params(self.env, self.roles, &r.team, &r.environment);
                match crate::tenantconfig::approvals::request(self.env, self.roles, "k3s.cluster", &r.team, &r.environment, &r.owner, tenant_only, role_only, now) {
                    Ok(p) => Decision::Wait { approval_id: p.id, approver_role },
                    Err(e) => Decision::Refused { code: "approval".into(), reason: e },
                }
            }
            Deploy::Deny(reason) => Decision::Refused { code: "denied".into(), reason },
        }
    }
}
