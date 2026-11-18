//! What a test needs from the agent and the core, handed in instead of named (alpha.6 split H: `iyakki-apps` no longer depends on `iyakki-agent`).
//!
//! * This crate's own tests (`cfg(test)`) build the real kit from `iyakki-agent`, a dev-dependency.
//! * With the `testing` feature (the shell's tests) the consumer installs a kit once, first: the shell's test wiring (`iyakki_shell::wiring`) does it for the
//!   shell's unit tests, and `iyakki_compose::wire()` is the production equivalent. Without a kit the process-wide handles keep their unwired defaults
//!   (the fake facade, no host facts), exactly what a production process gets before `wire()`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The pieces of the real wiring a test double of the UI needs.
pub struct TestKit {
    /// A short, unique folder under the system temp folder (`iyakki_agent::testdir::short`).
    pub short_dir: fn(&str, &str) -> PathBuf,
    /// The person's services in a data folder (`quick` for tests and lists, `slow` for generation).
    pub open_services: fn(&Path, Arc<dyn iyakki_proxy::Upstream>, Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn iyakki_agent_api::ServicesHandle>, String>,
    /// The core over those services, with the accounts' policies.
    pub core: fn(&Arc<dyn iyakki_agent_api::ServicesHandle>, Arc<dyn iyakki_core_api::PolicySource>) -> Arc<dyn iyakki_core_api::Core>,
    /// The real facade.
    pub agent: fn() -> Arc<dyn iyakki_agent_api::AgentApi>,
    /// The real local machine facts.
    pub host: fn() -> Arc<dyn iyakki_core_api::HostFacts>,
    /// The real local file scope.
    pub file_scope: fn() -> Arc<dyn iyakki_core_api::FileScope>,
}

static KIT: OnceLock<TestKit> = OnceLock::new();

/// Put the kit in place; only the first call takes effect.
pub fn install(kit: TestKit) -> bool {
    KIT.set(kit).is_ok()
}

/// The kit in use: this crate's tests always have the real one; a consumer of the `testing` feature has what it installed.
pub fn kit() -> Option<&'static TestKit> {
    #[cfg(test)]
    {
        Some(KIT.get_or_init(real::real_kit))
    }
    #[cfg(not(test))]
    {
        KIT.get()
    }
}

#[cfg(test)]
mod real {
    use super::*;

    pub(super) fn real_kit() -> TestKit {
        TestKit {
            short_dir: iyakki_agent::testdir::short,
            open_services: |dir, quick, slow| {
                let svc: Arc<dyn iyakki_agent_api::ServicesHandle> = iyakki_agent::services::Services::open(dir, quick, slow)?;
                Ok(svc)
            },
            core: |svc, policies| Arc::new(iyakki_agent::corehandle::AgentCore::new(iyakki_agent::services::Services::from_handle(svc)).with_policies(policies)),
            agent: || Arc::new(iyakki_agent::AgentApiImpl),
            host: || Arc::new(iyakki_agent::corehandle::LocalHost),
            file_scope: || Arc::new(iyakki_agent::corehandle::LocalFileScope),
        }
    }
}
