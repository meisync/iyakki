//! The quiet values of the facets the core does not serve over the socket yet (D-SPLIT-03-HANDLE): an object of each trait that answers "not
//! served over the UI socket" and never a made-up success. The traits of `iyakki-agent-api` are generated (`gen_unserved.rs`); the few of other
//! crates are here.

/// A facet the core does not serve over the UI socket: every method answers the quiet value of its return type.
pub struct Unserved;

use std::sync::Arc;

use crate::Lost;

/// The cue output of a facet that is not served: nothing is played.
impl iyakki_core_api::soundtypes::CueOut for Unserved {
    fn ready(&self) -> Result<(), String> {
        Err(crate::gen_unserved::NOT_SERVED.into())
    }
    fn play(&self, _wav: &[u8]) -> Result<(), String> {
        Err(crate::gen_unserved::NOT_SERVED.into())
    }
}
impl Lost for Arc<dyn iyakki_core_api::soundtypes::CueOut> {
    fn lost(_: &str) -> Self {
        Arc::new(Unserved)
    }
}
/// The store facets of a core that serves none: the default of the trait group.
impl Lost for Arc<dyn iyakki_agent_api::StoreFacets> {
    fn lost(_: &str) -> Self {
        Arc::new(iyakki_agent_api::NoStoreFacets)
    }
}

// ---- the quiet values of the types of the unserved facets (the first state of each: nothing running, nothing found, the careful choice)
impl Lost for iyakki_shared::gpu_steps::Compute {
    fn lost(_: &str) -> Self {
        iyakki_shared::gpu_steps::Compute::Cpu
    }
}
impl Lost for iyakki_shared::ollama::OllamaState {
    fn lost(_: &str) -> Self {
        iyakki_shared::ollama::OllamaState::Stopped
    }
}
impl Lost for iyakki_shared::cluster::ClusterState {
    fn lost(_: &str) -> Self {
        iyakki_shared::cluster::ClusterState::Stopped
    }
}
impl Lost for iyakki_tile_api::radio::RadioState {
    fn lost(_: &str) -> Self {
        iyakki_tile_api::radio::RadioState::Idle
    }
}
impl Lost for iyakki_shared::hostengines::Report {
    fn lost(_: &str) -> Self {
        iyakki_shared::hostengines::Report::empty()
    }
}
impl Lost for iyakki_shared::k3sdev::Layout {
    fn lost(_: &str) -> Self {
        iyakki_shared::k3sdev::Layout::new(std::path::Path::new(""))
    }
}
impl Lost for iyakki_shared::k3sdev::Svc {
    fn lost(_: &str) -> Self {
        iyakki_shared::k3sdev::Svc::NotInstalled
    }
}
impl Lost for iyakki_shared::k3sdev::Situation {
    fn lost(why: &str) -> Self {
        iyakki_shared::k3sdev::Situation::Free { problems: vec![why.to_string()] }
    }
}
impl Lost for iyakki_shared::clusters::Status {
    fn lost(_: &str) -> Self {
        iyakki_shared::clusters::Status::unknown()
    }
}
/// The most careful policy: a guest's.
impl Lost for iyakki_sec::policy::Policy {
    fn lost(_: &str) -> Self {
        iyakki_sec::policy::Role::Guest.template()
    }
}
impl Lost for [f32; 28] {
    fn lost(_: &str) -> Self {
        [0.0; 28]
    }
}
impl crate::LostErr for iyakki_shared::authcodes::VaultProblem {
    fn lost(why: &str) -> Self {
        iyakki_shared::authcodes::VaultProblem::Other(why.to_string())
    }
}
impl Lost for iyakki_shared::study::StudyStore {
    fn lost(_: &str) -> Self {
        iyakki_shared::study::StudyStore::new(std::path::Path::new(""))
    }
}
impl Lost for std::sync::Mutex<iyakki_shared::mediaindex::MediaIndex> {
    fn lost(_: &str) -> Self {
        std::sync::Mutex::new(iyakki_shared::mediaindex::MediaIndex::in_memory(None))
    }
}
/// A git key store with no folder, an empty locked vault and no audit line: it finds no key and writes nothing.
impl Lost for iyakki_shared::gitkeys::GitKeys {
    fn lost(_: &str) -> Self {
        let store = std::sync::Arc::new(iyakki_shared::gitkeys::MemStore { locked: std::sync::Mutex::new(true), ..Default::default() });
        let audit: iyakki_shared::gitkeys::AuditFn = Arc::new(|_, _| {});
        iyakki_shared::gitkeys::GitKeys::new(std::path::Path::new(""), store, audit, Arc::new(iyakki_shared::gitkeys::Session { pass: Default::default(), approved: Default::default() }))
    }
}
/// A private-store report of a core that was not asked: not installed, nothing found.
impl Lost for iyakki_shared::privatemanage::Report {
    fn lost(_: &str) -> Self {
        iyakki_shared::privatemanage::Report { state: iyakki_shared::privatemanage::StoreState::NotInstalled, cluster: iyakki_shared::platform_types::ClusterRef { name: String::new(), kubeconfig: String::new(), context: String::new(), server: String::new(), source: iyakki_shared::devcluster::Source::Project }, namespace: String::new(), instance: String::new(), facts: Default::default(), record: None, elsewhere: None, source: None }
    }
}
