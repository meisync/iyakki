//! The real implementation of `iyakki_agent_api::facets_b8` (F-SPLIT-03, lane alpha.6/split-e).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{DirectLinkB8, ServicesB8};
use iyakki_shared::directlink::Env;
use iyakki_shared::jobs::Job;
use iyakki_shared::quickinstall::{Launch, RegistryArgs};

use crate::services::Services;

impl ServicesB8 for Services {
    fn qi_start_registry(&self, owner: &str, a: &RegistryArgs, k3s_present: bool, launch: &Launch) -> Result<Vec<Job>, String> {
        crate::quickinstall::start_registry(&self.jobs, owner, a, k3s_present, launch)
    }
}

/// The real direct-link machine.
pub struct RealDirectLinkB8;

impl DirectLinkB8 for RealDirectLinkB8 {
    fn real_env(&self, data: &Path, home: &Path) -> Option<Arc<dyn Env>> {
        Some(Arc::new(crate::directlink::real::RealEnv::new(data, home)))
    }
}

impl iyakki_agent_api::AgentFacetsB8 for crate::AgentApiImpl {
    fn direct_link_b8(&self) -> &dyn DirectLinkB8 {
        &RealDirectLinkB8
    }
}
