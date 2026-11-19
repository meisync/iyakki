//! Facets of alpha.6 lane split-e (F-SPLIT-03): the Quick install registry job and the direct-link (USB-C) machine. The real
//! implementation is `iyakki_agent::facets_b8`; the defaults here are the fake (a core that serves none of them).

use std::path::Path;
use std::sync::Arc;

use iyakki_shared::directlink::Env;
use iyakki_shared::jobs::Job;
use iyakki_shared::quickinstall::{Launch, RegistryArgs};

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B8: &str = "unavailable: this core does not serve this call";

/// What needs the process's services; implemented for `iyakki_agent::services::Services`.
pub trait ServicesB8 {
    /// Start the local image registry job for `a` (the managed cluster when no target is given: k3s is installed first when missing).
    /// Refuses while a registry job is running.
    fn qi_start_registry(&self, _owner: &str, _a: &RegistryArgs, _k3s_present: bool, _launch: &Launch) -> Result<Vec<Job>, String> {
        Err(UNAVAILABLE_B8.into())
    }
}

/// The direct-link machine (two computers on one cable): the engine and its pure types are `iyakki_shared::directlink`.
pub trait DirectLinkB8: Send + Sync {
    /// The real machine (sysfs, ioctls, raw ARP socket, UDP) for the data folder and the developer-mode client home;
    /// `None` when this core cannot serve it.
    fn real_env(&self, _data: &Path, _home: &Path) -> Option<Arc<dyn Env>> {
        None
    }
}

/// The default: no direct link.
pub struct NoB8;
impl DirectLinkB8 for NoB8 {}

/// The accessor (a supertrait of `AgentFacets`).
pub trait AgentFacetsB8: Send + Sync {
    fn direct_link_b8(&self) -> &dyn DirectLinkB8 {
        &NoB8
    }
}
impl AgentFacetsB8 for crate::FakeAgentApi {}
