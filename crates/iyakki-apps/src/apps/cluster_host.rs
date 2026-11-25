//! The cluster switcher in the shell: the host and its drawing are `iyakki_tile_k8s::cluster_host` (shared by the K8S tile, the AI IDE's Kubernetes
//! panel and deploy header and the Store -> Setup popup; alpha.8 E7). This file gives it the real [`ClusterHostPort`] (the fleet, the agent's cluster
//! registry and its GPU look, the audit log) and the one constructor the screens use.

use std::sync::Arc;

use crate::apps::AppCx;
use iyakki_cluster_model::clusters::{ClusterEntry, Dot, Status};
use iyakki_sec::policy::Role;

pub use iyakki_tile_k8s::cluster_host::*;

/// The real port: what the switcher reads from the machine, from the context of the screen that owns it.
pub struct CxHostPort {
    dir: std::path::PathBuf,
    user: String,
    role: Role,
    fleet: Arc<dyn iyakki_agent_api::FleetApi>,
    audit: Arc<dyn iyakki_agent_api::AuditApi>,
}

impl ClusterHostPort for CxHostPort {
    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
    fn user(&self) -> String {
        self.user.clone()
    }
    fn role(&self) -> Role {
        self.role
    }
    fn local_status(&self, e: &ClusterEntry) -> Status {
        self.fleet.status(e, false)
    }
    /// A remote cluster's dot from the last GPU look (which is one `kubectl get nodes`): answered, answered long ago, or not checked.
    fn remote_status(&self, e: &ClusterEntry) -> Status {
        let mut st = Status::unknown();
        if let Some(p) = iyakki_shared::aiprofile_remote::clusters_cache().get_key(&e.id) {
            st.dot = if p.stale { Dot::Stale } else { Dot::Running };
            st.last_error = p.error.clone();
        }
        st
    }
    fn refresh_gpus(&self, visible: &[ClusterEntry]) {
        if !crate::TESTING {
            let run: Arc<dyn iyakki_shared::wsexec::Runner> = crate::agent().exec().system();
            crate::agent().cluster_registry().refresh_gpus(&run, visible, true);
        }
    }
    fn gpu_summary(&self, id: &str) -> Option<String> {
        crate::agent().cluster_registry().gpu_summary(id)
    }
    fn switch_account(&self, id: &str) -> Result<ClusterEntry, String> {
        crate::agent().cluster_registry().switch(&self.dir, &self.dir, &self.user, self.role, id)
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.record(&self.user, action, detail);
    }
}

/// The switcher of this person on this screen. `base`: the event codes `base..base + 10 + rows` belong to it; `workspace`: the AI IDE workspace
/// (empty elsewhere).
pub fn new(base: usize, cx: &AppCx, workspace: &str) -> ClusterHost {
    ClusterHost::new(base, Arc::new(CxHostPort { dir: cx.data_dir().to_path_buf(), user: cx.session.user.clone(), role: cx.session.role, fleet: cx.fleet(), audit: cx.audit.clone() }), workspace)
}

#[cfg(test)]
#[path = "cluster_host_tests.rs"]
mod tests;
