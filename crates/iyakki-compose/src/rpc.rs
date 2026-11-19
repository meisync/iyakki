//! The root objects the real core serves over its UI socket (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md section 2).
//!
//! `iyakki-ui-rpc` knows only traits. The composition root is the one place that knows which real object answers each trait, so it builds the
//! [`Host`] the core's `uiserve::served` hands to the transport (`rpc: Some`): the agent's facets (`AgentApiImpl`), the member stores of the real
//! services (through their accessors, so the core's one store answers and nothing is copied), the audit log, and the chat runs once a
//! [`RunStarter`] is given. A UI program therefore reaches exactly the roots listed here; a trait not registered answers "no such object".

use std::sync::Arc;

use iyakki_agent::contract_local::uiserve::RpcFactory;
use iyakki_agent_api::{ChatRunTable, RunStarter, ServicesHandle};
use iyakki_ui_rpc::{Host, Obj};

/// What the core serves as roots.
#[derive(Clone, Default)]
pub struct RpcRoots {
    audit: Option<Arc<iyakki_shell::accounts::AuditLog>>,
    runs: Option<Arc<dyn RunStarter>>,
}

impl RpcRoots {
    pub fn new() -> RpcRoots {
        RpcRoots::default()
    }

    /// Serve the audit log of the shell as the `AuditApi` root.
    pub fn with_audit(mut self, a: Arc<iyakki_shell::accounts::AuditLog>) -> RpcRoots {
        self.audit = Some(a);
        self
    }

    /// Serve chat runs through `starter` (the core's own tool chain).
    pub fn with_runs(mut self, s: Arc<dyn RunStarter>) -> RpcRoots {
        self.runs = Some(s);
        self
    }

    /// The names of the roots a host built over these services would register (what a UI can ask for), sorted.
    pub fn root_names(&self) -> Vec<&'static str> {
        let mut v = ROOTS.to_vec();
        if self.audit.is_some() {
            v.push("AuditApi");
        }
        if self.runs.is_some() {
            v.push("ChatRuns");
        }
        v.sort_unstable();
        v
    }

    /// The host for one set of real services.
    pub fn host(&self, svc: Arc<dyn ServicesHandle>) -> Arc<Host> {
        use iyakki_agent_api as api;
        let agent = Arc::new(iyakki_agent::AgentApiImpl);
        let mut h = Host::new()
            .with_root(Obj::AgentDisplay(agent.clone()))
            .with_root(Obj::AgentFiles(agent.clone()))
            .with_root(Obj::AgentStore(agent.clone()))
            .with_root(Obj::AgentFacetsB2(agent.clone()))
            .with_root(Obj::AgentFacetsB4(agent.clone()))
            .with_root(Obj::AgentFacetsB6(agent.clone()))
            .with_root(Obj::AgentFacetsB7(agent.clone()))
            .with_root(Obj::AgentFacetsB8(agent.clone()))
            .with_root(Obj::AgentFacetsS6B(agent))
            .with_root(Obj::InboxApi(Arc::new(api::InboxVia(svc.clone()))))
            .with_root(Obj::CalendarApi(Arc::new(api::CalendarVia(svc.clone()))))
            .with_root(Obj::StudyApi(Arc::new(api::StudyVia(svc.clone()))))
            .with_root(Obj::SecretsApi(Arc::new(api::SecretsVia(svc.clone()))))
            .with_root(Obj::DevWatchApi(Arc::new(api::DevWatchVia(svc.clone()))))
            .with_root(Obj::BrowserStateApi(Arc::new(api::BrowserStateVia(svc.clone()))))
            .with_root(Obj::ScreenCaptureApi(Arc::new(api::ScreenCaptureVia(svc.clone()))))
            .with_root(Obj::DevToolsApi(Arc::new(api::DevToolsVia(svc.clone()))))
            .with_root(Obj::SecretRequestsApi(Arc::new(api::SecretRequestsVia(svc.clone()))))
            .with_root(Obj::UsageLogApi(Arc::new(api::UsageLogVia(svc.clone()))))
            .with_root(Obj::ExtStatusApi(Arc::new(api::ExtStatusVia(svc))));
        if let Some(a) = &self.audit {
            h = h.with_root(Obj::AuditApi(a.clone()));
        }
        if let Some(s) = &self.runs {
            h = h.with_root(Obj::ChatRuns(Arc::new(ChatRunTable::new(s.clone()))));
        }
        Arc::new(h)
    }

    /// What `uiserve::watch` needs: one host per core, built over the core's own services.
    pub fn factory(self) -> RpcFactory {
        Arc::new(move |core| {
            let svc: Arc<dyn ServicesHandle> = core.svc.clone();
            let h: Arc<dyn iyakki_ui_client::server::RpcServe> = self.host(svc);
            h
        })
    }
}

/// The roots every host has (the audit log and chat runs join when given).
pub const ROOTS: [&str; 20] = [
    "AgentDisplay", "AgentFacetsB2", "AgentFacetsB4", "AgentFacetsB6", "AgentFacetsB7", "AgentFacetsB8", "AgentFacetsS6B", "AgentFiles", "AgentStore", "BrowserStateApi", "CalendarApi", "DevToolsApi", "DevWatchApi", "ExtStatusApi", "InboxApi", "ScreenCaptureApi", "SecretRequestsApi", "SecretsApi", "StudyApi", "UsageLogApi",
];
