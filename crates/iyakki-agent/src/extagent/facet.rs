//! The external coding agents behind `iyakki_agent_api::ExtAgentFacet` (F-SPLIT-03): the UI hands over a plain [`ExtRunSpec`] and gets a
//! run handle; [`super::run::RunRequest`] stays here.

use std::sync::Arc;

use iyakki_agent_api::{ExtAgentFacet, ExtRunSpec};
use iyakki_shared::agent::{RunEvent, RunHandle};

use crate::services::Services;

impl Services {
    /// The external agents facet over these services (`cx.services.extagent_facet()`).
    pub fn extagent_facet(self: &Arc<Self>) -> Arc<dyn ExtAgentFacet> {
        Arc::new(RealExtAgents { svc: self.clone() })
    }
}

struct RealExtAgents {
    svc: Arc<Services>,
}

impl ExtAgentFacet for RealExtAgents {
    fn entries(&self) -> Vec<iyakki_extagentx::Entry> {
        super::entries(&self.svc.agents_cfg(), &self.svc.ext_status)
    }

    fn start(&self, s: ExtRunSpec) -> RunHandle {
        let Some(agent) = self.svc.agents_cfg().agent(&s.agent) else {
            let (tx, rx) = std::sync::mpsc::channel();
            let (dtx, _drx) = std::sync::mpsc::channel();
            let _ = tx.send(RunEvent::Failed { error: "That agent is no longer configured. Pick a model again (Ctrl+P).".into() });
            return RunHandle::from_parts(rx, dtx);
        };
        super::run::start(super::run::RunRequest {
            agent: Arc::from(agent),
            workspace: s.workspace,
            prompt: s.prompt,
            system: s.system,
            model: s.model,
            resume: s.resume,
            tools: s.tools,
            limits: s.limits,
            allow_auto: s.allow_auto,
            key: s.key,
            audit: s.audit,
            exe: s.exe,
            on_session: s.on_session,
            on_usage: s.on_usage,
            no_tools: s.no_tools,
        })
    }
}
