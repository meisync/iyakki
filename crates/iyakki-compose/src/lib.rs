//! The composition root (alpha.6 split F, docs/development/SPLIT-GROUPS.md): the ONLY crate that names both the agent (`iyakki-agent`) and the UI
//! (`iyakki-apps`, `iyakki-shell`). A program calls [`wire`] once, before it makes a shell environment or a shell; after that the UI reaches the agent
//! only through the handles installed here:
//!
//! * the agent facade (`iyakki_apps::install_agent`, the real `AgentApiImpl`: KI-1010, the first read of the process-wide handle must not see the fake),
//! * the host facts and the file scope (`install_host`, `install_file_scope`),
//! * the sound players and recorders the agent found (`install_players`, `install_recorders`),
//! * the core factory the shell calls for each shell (`iyakki_shell::wiring::install_core_factory`): the `AgentCore` over the environment's services.
//!
//! `wire` is idempotent; the first call wins (the installers are one-shot cells).

pub mod frames;
pub mod rpc;

use std::sync::Arc;

use iyakki_shell::wiring::CoreInputs;

/// Install the real agent and everything the UI is given from it.
pub fn wire() {
    let _ = iyakki_apps::install_agent(Arc::new(iyakki_agent::AgentApiImpl));
    assert!(iyakki_agent_api::AgentFacets::is_real(iyakki_apps::agent()), "the agent handle is not the real implementation");
    let _ = iyakki_apps::install_host(Arc::new(iyakki_agent::corehandle::LocalHost));
    let _ = iyakki_apps::install_file_scope(Arc::new(iyakki_agent::corehandle::LocalFileScope));
    let agent = iyakki_apps::agent_arc();
    if let Some(p) = agent.sound_probe().system_players() {
        let _ = iyakki_apps::install_players(p);
    }
    if let Some(r) = agent.sound_probe().system_recorders() {
        let _ = iyakki_apps::install_recorders(r);
    }
    let _ = iyakki_shell::wiring::install_core_factory(core_for);
    let _ = iyakki_shell::wiring::install_services_factory(open_services);
}

/// The real services of a data folder (what `Shell` environments are built over): the one place the UI's programs get a `Services` from.
pub fn open_services(dir: &std::path::Path, quick: Arc<dyn iyakki_proxy::Upstream>, slow: Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn iyakki_agent_api::ServicesHandle>, String> {
    let svc: Arc<dyn iyakki_agent_api::ServicesHandle> = iyakki_agent::services::Services::open(dir, quick, slow)?;
    Ok(svc)
}

/// The agent's `Services` behind the handle (the composition root is the one place that may name it).
pub fn real_services(h: &Arc<dyn iyakki_agent_api::ServicesHandle>) -> Arc<iyakki_agent::services::Services> {
    h.clone().as_any_arc().downcast::<iyakki_agent::services::Services>().expect("the services handle is the agent's Services")
}

/// The core over one shell environment's services (what each `Shell::new` asks for).
fn core_for(env: &iyakki_shell::shell::ShellEnv, inputs: CoreInputs) -> Arc<dyn iyakki_core_api::Core> {
    let CoreInputs { policies, audit, policy_of } = inputs;
    Arc::new(
        iyakki_agent::corehandle::AgentCore::new(real_services(&env.services))
            .with_policies(policies)
            .with_embedded_assistant(env.chat.clone(), policy_of.clone(), audit.clone())
            .with_media(audit, policy_of.clone())
            .with_embedded_cameras(policy_of),
    )
}

#[cfg(test)]
mod tests {
    /// `wire` installs the real agent handle and the core factory (a fake default must not be what production gets).
    #[test]
    fn wire_installs_the_real_agent_and_the_core_factory() {
        super::wire();
        super::wire();
        assert!(iyakki_agent_api::AgentFacets::is_real(iyakki_apps::agent()));
        assert!(iyakki_shell::wiring::is_wired());
    }
}
