//! The FLOWS engine behind the narrow facets of `iyakki_agent_api::facets_s6b` (F-SPLIT-03, alpha.6 lane split-b): the CHAT tile and the
//! shell hold `Arc<dyn FlowChatEnv>` and never name `FlowEnv`.

use std::sync::Arc;

use iyakki_agent_api::{FlowChatEnv, FlowsFacet};
use iyakki_flowsx::spec::SharedOpen;
use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::browserconsent::{Answer, ConsentRequest};

use crate::flows::FlowEnv;
use crate::services::Services;

impl Services {
    /// The flows facet over these services (`cx.services.flows_facet()`).
    pub fn flows_facet(self: &Arc<Self>) -> Arc<dyn FlowsFacet> {
        Arc::new(RealFlows { svc: self.clone() })
    }
}

impl FlowEnv {
    /// This environment as the CHAT tile's handle.
    pub fn chat_handle(self: &Arc<Self>) -> Arc<dyn FlowChatEnv> {
        Arc::new(FlowChat(self.clone()))
    }
}

struct RealFlows {
    svc: Arc<Services>,
}

impl FlowsFacet for RealFlows {
    fn open(&self, user: &str, policy: &Policy) -> Result<Arc<dyn FlowChatEnv>, String> {
        FlowEnv::open(&self.svc, user, policy).map(|e| e.chat_handle())
    }
    fn answer_permission(&self, user: &str, policy: &Policy, req: &ConsentRequest, answer: Answer) -> Result<String, String> {
        FlowEnv::open(&self.svc, user, policy).and_then(|env| env.answer_permission(req, answer))
    }
    fn set_last_flow(&self, user: &str, flow: &str, name: &str) {
        crate::flows::set_last_flow(&self.svc, user, flow, name)
    }
    fn start_for_login(&self, user: &str, policy: &Policy) -> bool {
        FlowEnv::start_for_login(&self.svc, user, policy)
    }
}

struct FlowChat(Arc<FlowEnv>);

impl FlowChatEnv for FlowChat {
    fn prompt_rules(&self) -> &'static str {
        crate::flows::tools::PROMPT_RULES
    }
    fn context(&self, open: &SharedOpen) -> String {
        crate::flows::tools::FlowTools::context(&self.0, open)
    }
    fn draft_name(&self, flow: &str) -> Option<String> {
        self.0.store.get_draft(flow).map(|x| x.name)
    }
    fn newest_failed_run(&self, flow: &str) -> Option<String> {
        crate::flows::newest_failed_run(&self.0, flow)
    }
    fn last_run_failed(&self, flow: &str) -> bool {
        crate::flows::last_run_failed(&self.0, flow)
    }
    fn last_flow(&self) -> Option<(String, String)> {
        crate::flows::last_flow(&self.0)
    }
    fn wrap_tools(&self, inner: Arc<dyn Tools>, open: SharedOpen) -> Arc<dyn Tools> {
        Arc::new(crate::flows::tools::FlowTools::new(inner, self.0.clone(), open))
    }
}
