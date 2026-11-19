//! The wiring an example needs: what `iyakki_compose::wire()` does for the programs (the shell cannot depend on the composition root above it).
#![allow(dead_code)]

use std::sync::Arc;

pub fn wire() {
    let _ = iyakki_apps::install_agent(Arc::new(iyakki_agent::AgentApiImpl));
    let _ = iyakki_apps::install_host(Arc::new(iyakki_agent::corehandle::LocalHost));
    let _ = iyakki_apps::install_file_scope(Arc::new(iyakki_agent::corehandle::LocalFileScope));
    let _ = iyakki_shell::wiring::install_services_factory(|dir, quick, slow| {
        let svc: Arc<dyn iyakki_agent_api::ServicesHandle> = iyakki_agent::services::Services::open(dir, quick, slow)?;
        Ok(svc)
    });
    let _ = iyakki_shell::wiring::install_core_factory(|env, i| {
        Arc::new(
            iyakki_agent::corehandle::AgentCore::new(iyakki_agent::services::Services::from_handle(&env.services))
                .with_policies(i.policies)
                .with_embedded_assistant(env.chat.clone(), i.policy_of.clone(), i.audit.clone())
                .with_media(i.audit, i.policy_of.clone())
                .with_embedded_cameras(i.policy_of),
        )
    });
}
