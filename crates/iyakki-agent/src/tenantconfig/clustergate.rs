//! A person's cluster that is bound to a team environment goes through the team's deploy rules (F-TCFG-10): the cluster tools that change a
//! cluster ask [`TeamGate::through`] around the change. A cluster with no binding is the person's own and is not gated here (the card
//! the person approves is all there is). A bound one files (or finds) an approval request when the environment wants a second person,
//! applies only on `Go`, and spends the approval with the one apply, all through [`super::runtime::deploy_through`].

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::api::{ConfigAudit, Env};
use super::enforce::DeployAsk;
use super::vault::SecretStore;

/// Which team environment a cluster of the registry belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub cluster_id: String,
    pub team: String,
    pub environment: String,
    /// The environment the same change was last deployed to (the promotion order wants prod to come from preprod); the person's tool says it.
    #[serde(default)]
    pub promoted_from: Option<String>,
}

/// Where the bindings are kept in the data folder.
pub const FILE: &str = "team-clusters.json";

/// The bindings of a data folder (a missing or unreadable file means none: nothing is gated by guessing).
pub fn load(dir: &std::path::Path) -> Vec<Binding> {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub trait TeamGate: Send + Sync {
    /// `None`: the cluster is not bound to a team. `Some(result)`: the team's rules decided; `apply` ran only when they said go.
    fn through(&self, cluster_id: &str, apply: &mut dyn FnMut(Option<&str>) -> Result<String, String>) -> Option<Result<String, String>>;
}

/// The real gate: the tenant's own rules, approvals and audit.
pub struct TenantGate {
    pub dir: PathBuf,
    pub vault: Arc<dyn SecretStore>,
    pub audit: Arc<dyn ConfigAudit>,
    pub roles: crate::cloudroles::Roles,
    pub bindings: Vec<Binding>,
    pub requester: String,
    /// The requester's rank in the team (`Caller::rank`); negative is no right to deploy.
    pub rank: i32,
    pub guest: bool,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl TeamGate for TenantGate {
    fn through(&self, cluster_id: &str, apply: &mut dyn FnMut(Option<&str>) -> Result<String, String>) -> Option<Result<String, String>> {
        let b = self.bindings.iter().find(|b| b.cluster_id == cluster_id)?;
        let env = Env { dir: &self.dir, vault: self.vault.clone(), audit: self.audit.as_ref() };
        let ask = DeployAsk { team: &b.team, environment: &b.environment, requester: &self.requester, requester_rank: self.rank, approval: None, promoted_from: b.promoted_from.as_deref(), registry: None, roles: &self.roles, guest: self.guest };
        Some(super::runtime::deploy_through(&env, &ask, (self.now)(), apply))
    }
}
