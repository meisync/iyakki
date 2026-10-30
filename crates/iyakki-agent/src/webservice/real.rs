//! The real implementations of the service's dependencies, over the machine's `Services`. The supervisor adds the accounts and the
//! audit log (they live in the shell crate today) through the `iyakki-core-api` traits, then calls [`assemble`].

use super::deps::{Deps, Extras, ToolSource};
use crate::agent::{Tools, UserTools, AGENT_SYSTEM_PROMPT};
use crate::chat::ChatBackend;
use crate::mcp::ServerSpec;
use crate::services::Services;
use crate::settings::ModelChoice;
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, HostApi, Principal};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ServicesTools {
    svc: Arc<Services>,
    host: Arc<dyn HostApi>,
}

impl ToolSource for ServicesTools {
    fn tools_for(&self, p: &Principal) -> Arc<dyn Tools> {
        // MCP servers are not passed: their tools are `mcp__*`, high risk, and not in any profile's list in this slice
        Arc::new(UserTools::new(&p.user, p.policy.clone(), self.svc.clone(), self.host.clone(), Vec::<ServerSpec>::new()))
    }
}

pub struct ServicesExtras {
    svc: Arc<Services>,
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io(e.to_string())
}

impl Extras for ServicesExtras {
    fn ready(&self) -> Result<(), String> {
        let probe = self.svc.dir().join(".service-ready");
        std::fs::write(&probe, b"ok").map_err(|e| format!("the data directory is not writable: {e}"))?;
        let _ = std::fs::remove_file(&probe);
        Ok(())
    }

    fn models(&self, _p: &Principal) -> Vec<Value> {
        self.svc.all_models().into_iter().map(|(provider, model)| json!({"provider": provider, "model": model})).collect()
    }

    fn backend_for(&self, _p: &Principal, provider: &str, model: &str) -> ApiResult<Arc<dyn ChatBackend>> {
        if !self.svc.all_models().iter().any(|(p, m)| p == provider && m == model) {
            return Err(ApiError::InvalidInput("that provider and model are not offered (see GET /api/v1/models)".into()));
        }
        Ok(self.svc.chat_backend_for(ModelChoice { provider: provider.into(), model: model.into() }))
    }

    fn flows(&self, p: &Principal) -> ApiResult<Vec<Value>> {
        let env = crate::flows::FlowEnv::build(&self.svc, &p.user, &p.policy, false);
        Ok(env.summaries().into_iter().map(|f| json!({"id": f.id, "name": f.name, "description": f.description, "published": f.published, "nodes": f.nodes, "triggers": f.triggers, "production": f.production})).collect())
    }

    fn flow_export(&self, p: &Principal, id: &str) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::build(&self.svc, &p.user, &p.policy, false);
        let text = env.export_json(id).map_err(|_| ApiError::NotFound("no such flow".into()))?;
        serde_json::from_str(&text).map_err(io)
    }

    fn data_call(&self, p: &Principal, tool: &str, args: &Value) -> ApiResult<String> {
        let cwd = crate::scope::user_home(&p.user);
        let cwd = if cwd.is_dir() { cwd } else { std::path::PathBuf::from("/") };
        crate::datatools::call(tool, args, &p.policy, &p.user, &cwd, self.svc.dir()).map_err(ApiError::InvalidInput)
    }
}

/// Everything the service needs, over the machine's services.
pub fn assemble(auth: Arc<dyn Authenticator>, audit: Arc<dyn AuditSink>, svc: Arc<Services>, host: Arc<dyn HostApi>) -> Arc<Deps> {
    Arc::new(Deps {
        auth,
        audit,
        store: svc.runs.clone(),
        chat: svc.chat_backend(),
        tools: Arc::new(ServicesTools { svc: svc.clone(), host }),
        extras: Arc::new(ServicesExtras { svc }),
        system_prompt: AGENT_SYSTEM_PROMPT.to_string(),
    })
}
