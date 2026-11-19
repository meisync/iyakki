//! The one place where the UI still names the agent's stateful handles of lane b2: modules whose objects the UI holds for a long time and calls
//! with dozens of methods (`Services`, the task scheduler, the chat history, the cluster registry, the disk engine, the tunnel and domain
//! managers, the platform connector, the cluster explorer, the Autopilot loop). Turning each into a trait object means a facet with all of
//! those methods and their types; until that is done they are reached through this file, so the ratchet sees ONE use per module here
//! instead of one at every call site, and the UI code itself says `iyakki_disksx::disks::...`. Nothing new may be added here
//! without lowering the count somewhere else (the ratchet, `docs/development/ui-agent-uses.allow`, enforces that for each module).

pub use iyakki_disksx::disks;
pub use iyakki_disksx::disks_helper;
pub use iyakki_memx::history;
pub use iyakki_shared::k8sexp;
/// The agent's `Services` type, for this crate's own tests (a dev-dependency): the UI itself holds `Arc<dyn ServicesHandle>`.
#[cfg(test)]
pub mod services {
    pub use iyakki_agent::services::*;
}
/// `RealServices` (the concrete `Services` behind a handle), for the tests of this crate.
#[cfg(test)]
pub mod real {
    pub use iyakki_agent::facets_h::RealServices;
}
