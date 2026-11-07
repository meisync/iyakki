//! A flow's `cli` and `browser` steps in a cloud pod, through the sandbox layer (`sandbox::wiring::exec_via_sandbox`,
//! `browser_via_sandbox`). The tenant's `sandbox.policy` and the team's cluster binding decide, exactly as for `run_command`; the
//! team is the one where the person has the highest rank (the flow has no team field). Installed by `real::start`.

use crate::cloudroles::Roles;
use crate::sandbox::admit::Who;
use crate::sandbox::engine::{Engine, FileAudit};
use crate::sandbox::exec::{ApplyGate, JobExecutor, K3sGate, SecretResolver};
use crate::sandbox::kube::KubeApi;
use crate::sandbox::policy::SandboxPolicy;
use crate::sandbox::spec::Labels;
use crate::tenantconfig::api::{Caller, ConfigAudit, Env};
use iyakki_flow::host::{BrowserRequest, BrowserResult, ExecRequest, ExecResult};
use iyakki_flow::NodeError;
use std::path::PathBuf;
use std::sync::Arc;

pub struct FlowSandbox {
    pub dir: PathBuf,
    pub tenant: String,
    pub kube: Arc<dyn KubeApi>,
    pub vault: Arc<dyn crate::tenantconfig::vault::SecretStore>,
    pub audit: Arc<dyn ConfigAudit>,
    pub secrets: Arc<dyn SecretResolver>,
}

fn role_of(rank: i32) -> &'static str {
    match rank {
        3.. => "tenant_admin",
        2 => "admin",
        1 => "editor",
        _ => "viewer",
    }
}

/// The team a person's flow steps are charged to: where their rank is highest.
pub fn best_team(roles: &Roles, user: &str) -> Option<(String, i32)> {
    let rank = |t: &str| Caller { user, roles: Some(roles) }.rank(t);
    roles.tenant_role(user)?;
    let team = roles.namespaces.keys().max_by_key(|t| (rank(t), std::cmp::Reverse((*t).clone()))).cloned()?;
    let r = rank(&team);
    (r >= 0).then_some((team, r))
}

impl FlowSandbox {
    fn engine(&self, roles: &Roles, team: &str, rank: i32) -> (Engine, SandboxPolicy) {
        let env = Env { dir: &self.dir, vault: self.vault.clone(), audit: self.audit.as_ref() };
        let policy = SandboxPolicy::load(&env, team);
        let gate: Arc<dyn ApplyGate> = Arc::new(K3sGate { dir: self.dir.clone(), vault: self.vault.clone(), audit: self.audit.clone(), roles: roles.clone(), rank, guest: false, environment: policy.k3s_environment.clone() });
        let job = Arc::new(JobExecutor::new(self.kube.clone(), policy.clone(), gate, self.secrets.clone()));
        let audit: Arc<dyn ConfigAudit> = Arc::new(FileAudit { dir: self.dir.clone(), inner: self.audit.clone() });
        (Engine::new(policy.clone(), vec![job], audit), policy)
    }

    fn labels(&self, team: &str, user: &str) -> Labels {
        let id = crate::cloudkeys::random::<6>().map(|b| crate::cloudkeys::hex(&b)).unwrap_or_default();
        Labels { tenant: self.tenant.clone(), team: team.into(), person: user.into(), run_id: format!("flow-{id}") }
    }

    fn context(&self, user: &str) -> Result<(Roles, String, i32), NodeError> {
        let roles = crate::cloudroles::load(&crate::cloudroles::path(&self.dir)).ok().flatten().ok_or_else(|| NodeError::config("the tenant's people lists cannot be read"))?;
        let (team, rank) = best_team(&roles, user).ok_or_else(|| NodeError::config("you are not in a team"))?;
        Ok((roles, team, rank))
    }

    pub fn exec(&self, user: &str, req: &ExecRequest) -> Result<ExecResult, NodeError> {
        let (roles, team, rank) = self.context(user)?;
        let (engine, policy) = self.engine(&roles, &team, rank);
        crate::sandbox::wiring::exec_via_sandbox(&engine, &Who { person: user, role: role_of(rank) }, req, &policy.cli_image, self.labels(&team, user), None)
    }

    pub fn browser(&self, user: &str, req: &BrowserRequest) -> Result<BrowserResult, NodeError> {
        let (roles, team, rank) = self.context(user)?;
        let (engine, _) = self.engine(&roles, &team, rank);
        crate::sandbox::wiring::browser_via_sandbox(&engine, &Who { person: user, role: role_of(rank) }, req, self.labels(&team, user))
    }
}
