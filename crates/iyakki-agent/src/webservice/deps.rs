//! What the service needs from the rest of IYAKKI, as traits. The supervisor supplies [`real`](super::real) implementations over
//! the machine's `Services`; tests supply fakes. No handler names a kernel crate, a shell type or a device.

use crate::agent::Tools;
use crate::chat::ChatBackend;
use crate::runs::RunStore;
use iyakki_core_api::{ApiResult, AuditSink, Authenticator, Principal};
use serde_json::Value;
use std::sync::Arc;

/// Builds the person's full tool set (the profile filter is applied on top, by name).
pub trait ToolSource: Send + Sync {
    fn tools_for(&self, p: &Principal) -> Arc<dyn Tools>;
}

/// The smaller services: models, flows, data, readiness.
pub trait Extras: Send + Sync {
    /// Ready to serve: the data directory is writable and the services started.
    fn ready(&self) -> Result<(), String>;
    /// `{provider, model}` objects; never keys or addresses.
    fn models(&self, p: &Principal) -> Vec<Value>;
    /// A backend that uses the named model instead of the machine's current chat model (the pair must be one `models` lists).
    fn backend_for(&self, p: &Principal, provider: &str, model: &str) -> ApiResult<Arc<dyn ChatBackend>>;
    fn flows(&self, p: &Principal) -> ApiResult<Vec<Value>>;
    fn flow_export(&self, p: &Principal, id: &str) -> ApiResult<Value>;
    /// One data tool inside the person's own jail (`data_open`, `data_describe`, `data_query`).
    fn data_call(&self, p: &Principal, tool: &str, args: &Value) -> ApiResult<String>;
}

pub struct Deps {
    pub auth: Arc<dyn Authenticator>,
    pub audit: Arc<dyn AuditSink>,
    pub store: Arc<RunStore>,
    pub chat: Arc<dyn ChatBackend>,
    pub tools: Arc<dyn ToolSource>,
    pub extras: Arc<dyn Extras>,
    pub system_prompt: String,
}
