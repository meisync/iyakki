//! What the shell is given by the composition root (`iyakki-compose`, the one crate that names both the agent and the UI) instead of building it from the agent
//! itself: the core handle every app talks to. The injected process-wide handles (the agent facade, the host facts, the file scope, the sound players and
//! recorders) are installed by the same crate through `iyakki_apps::install_*` before the first shell is made.
//!
//! A program calls `iyakki_compose::wire()` once, first. A unit test of this crate has no composition root above it, so `make_core` builds the real handles
//! itself under `cfg(test)` (the agent is a dev-dependency of the shell).

use std::sync::{Arc, OnceLock};

/// The people's policies as the core reads them (by user name).
pub type PolicyOf = Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>;

/// What the shell hands to the core factory: its accounts as a policy source, the audit log as a sink, the policy lookup.
pub struct CoreInputs {
    pub policies: Arc<dyn iyakki_core_api::PolicySource>,
    pub audit: Arc<dyn iyakki_core_api::AuditSink>,
    pub policy_of: PolicyOf,
}

/// Builds the core over the environment's services.
pub type CoreFactory = fn(&crate::shell::ShellEnv, CoreInputs) -> Arc<dyn iyakki_core_api::Core>;

static FACTORY: OnceLock<CoreFactory> = OnceLock::new();

/// Put the core factory in place; only the first call takes effect.
pub fn install_core_factory(f: CoreFactory) -> bool {
    FACTORY.set(f).is_ok()
}

/// Opens the person's services in a data folder (`quick` for tests and model lists, `slow` for generation).
pub type ServicesFactory = fn(&std::path::Path, Arc<dyn iyakki_proxy::Upstream>, Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn iyakki_agent_api::ServicesHandle>, String>;

static SERVICES: OnceLock<ServicesFactory> = OnceLock::new();

/// Put the services factory in place (the composition root builds the real `Services`; the shell never names the agent). Only the first call takes effect.
pub fn install_services_factory(f: ServicesFactory) -> bool {
    SERVICES.set(f).is_ok()
}

/// Whether a composition root has been installed.
pub fn is_wired() -> bool {
    FACTORY.get().is_some() && SERVICES.get().is_some()
}

/// Open the services through the installed factory. The handle must be the real one (KI-1010: a fake default is never what production gets).
pub(crate) fn open_services(dir: &std::path::Path, quick: Arc<dyn iyakki_proxy::Upstream>, slow: Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn iyakki_agent_api::ServicesHandle>, String> {
    let svc = if let Some(f) = SERVICES.get() {
        f(dir, quick, slow)?
    } else {
        #[cfg(test)]
        {
            test_wire::test_services(dir, quick, slow)?
        }
        #[cfg(not(test))]
        {
            panic!("no composition root: call iyakki_compose::wire() before the first shell environment")
        }
    };
    assert!(svc.is_real(), "the services handle is not the real implementation");
    Ok(svc)
}

pub(crate) fn make_core(env: &crate::shell::ShellEnv, inputs: CoreInputs) -> Arc<dyn iyakki_core_api::Core> {
    if let Some(f) = FACTORY.get() {
        return f(env, inputs);
    }
    #[cfg(test)]
    {
        test_wire::test_core(env, inputs)
    }
    #[cfg(not(test))]
    {
        panic!("no composition root: call iyakki_compose::wire() before the first shell")
    }
}

/// The unit tests of the shell: the same wiring `iyakki_compose::wire()` does, built here (the agent is a dev-dependency of the shell).
#[cfg(test)]
mod test_wire {
    use super::*;

    pub(super) fn test_services(dir: &std::path::Path, quick: Arc<dyn iyakki_proxy::Upstream>, slow: Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn iyakki_agent_api::ServicesHandle>, String> {
        init();
        let svc: Arc<dyn iyakki_agent_api::ServicesHandle> = iyakki_agent::services::Services::open(dir, quick, slow)?;
        Ok(svc)
    }

    pub(super) fn test_core(env: &crate::shell::ShellEnv, inputs: CoreInputs) -> Arc<dyn iyakki_core_api::Core> {
        init();
        let CoreInputs { policies, audit, policy_of } = inputs;
        Arc::new(
            iyakki_agent::corehandle::AgentCore::new(iyakki_agent::services::Services::from_handle(&env.services))
                .with_policies(policies)
                .with_embedded_assistant(env.chat.clone(), policy_of.clone(), audit.clone())
                .with_media(audit, policy_of.clone())
                .with_embedded_cameras(policy_of),
        )
    }

    /// Install the real handles and the test kit of `iyakki-apps` once. Runs before `main` of the test binary (see [`INIT`]), so the first read of the
    /// process-wide handle already sees the real one, as it does in a program after `iyakki_compose::wire()`.
    pub(super) fn init() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            iyakki_apps::testkit::install(iyakki_apps::testkit::TestKit {
                short_dir: iyakki_agent::testdir::short,
                open_services: |dir, quick, slow| {
                    let svc: Arc<dyn iyakki_agent_api::ServicesHandle> = iyakki_agent::services::Services::open(dir, quick, slow)?;
                    Ok(svc)
                },
                core: |svc, policies| Arc::new(iyakki_agent::corehandle::AgentCore::new(iyakki_agent::services::Services::from_handle(svc)).with_policies(policies)),
                agent: || Arc::new(iyakki_agent::AgentApiImpl),
                host: || Arc::new(iyakki_agent::corehandle::LocalHost),
                file_scope: || Arc::new(iyakki_agent::corehandle::LocalFileScope),
            });
            iyakki_apps::install_agent(Arc::new(iyakki_agent::AgentApiImpl));
            iyakki_apps::install_host(Arc::new(iyakki_agent::corehandle::LocalHost));
            iyakki_apps::install_file_scope(Arc::new(iyakki_agent::corehandle::LocalFileScope));
            let agent = iyakki_apps::agent_arc();
            if let Some(p) = agent.sound_probe().system_players() {
                iyakki_apps::install_players(p);
            }
            if let Some(r) = agent.sound_probe().system_recorders() {
                iyakki_apps::install_recorders(r);
            }
        });
    }

    extern "C" fn run_init() {
        init();
    }

    /// A load-time initialiser: the unit tests of the shell read the process-wide handles from any test, in any order, and the handles are one-shot.
    #[used]
    #[cfg_attr(any(target_os = "linux", target_os = "android"), link_section = ".init_array")]
    #[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
    static INIT: extern "C" fn() = run_init;
}
