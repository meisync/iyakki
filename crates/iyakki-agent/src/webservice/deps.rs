//! What the service needs from the rest of IYAKKI, as traits. The supervisor supplies [`real`](super::real) implementations over
//! the machine's `Services`; tests supply fakes. No handler names a kernel crate, a shell type or a device.

use crate::agent::Tools;
use crate::chat::ChatBackend;
use crate::runs::RunStore;
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
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
    /// Start a run of one of the person's own flows (the published version, or the draft); returns the run id. Questions in the run wait
    /// for the person's own approval; nothing here can approve them.
    fn flow_run(&self, _p: &Principal, _id: &str, _draft: bool, _inputs: &Value) -> ApiResult<String> {
        Err(ApiError::Unsupported("this machine does not run flows".into()))
    }
    /// The runs of one of the person's flows, newest first (summaries; secrets already redacted).
    fn flow_runs(&self, _p: &Principal, _id: &str) -> ApiResult<Vec<Value>> {
        Err(ApiError::Unsupported("this machine does not run flows".into()))
    }
    /// One run of one of the person's flows: summary and the state of each step.
    fn flow_run_get(&self, _p: &Principal, _id: &str, _run: &str) -> ApiResult<Value> {
        Err(ApiError::Unsupported("this machine does not run flows".into()))
    }
    /// The timed triggers of the person's own flows: `{flow, name, published, trigger, next, paused}`.
    fn schedules(&self, _p: &Principal) -> ApiResult<Vec<Value>> {
        Err(ApiError::Unsupported("this machine does not run schedules".into()))
    }
    /// Put (add or replace by id) a timed trigger on the person's flow; returns `{active, trigger, next}`.
    fn schedule_save(&self, _p: &Principal, _flow: &str, _trigger: &Value) -> ApiResult<Value> {
        Err(ApiError::Unsupported("this machine does not run schedules".into()))
    }
    /// Remove one trigger; returns `{removed, timed_left, active}`.
    fn schedule_delete(&self, _p: &Principal, _flow: &str, _trigger: &str) -> ApiResult<Value> {
        Err(ApiError::Unsupported("this machine does not run schedules".into()))
    }
    fn schedule_pause(&self, _p: &Principal, _flow: &str, _paused: bool) -> ApiResult<Value> {
        Err(ApiError::Unsupported("this machine does not run schedules".into()))
    }
    /// One data tool inside the person's own jail (`data_open`, `data_describe`, `data_query`).
    fn data_call(&self, p: &Principal, tool: &str, args: &Value) -> ApiResult<String>;
    /// The signed-in person's session moved from one tenant to another (`from` and `to` are tenant ids). Lock whatever the machine kept
    /// unlocked for them (their vault) so the next tenant starts locked. The default keeps nothing, so there is nothing to lock.
    fn tenant_switched(&self, _p: &Principal, _from: &str, _to: &str) {}
    /// Cloud profile only: start the tenant's background services (the scheduler) with nobody signed in. `stop` ends them.
    fn background_start(&self, _auth: Arc<dyn Authenticator>, _audit: Arc<dyn AuditSink>, _stop: Arc<std::sync::atomic::AtomicBool>) {}
    /// Wait for what `background_start` started to finish (the stop flag is already set).
    fn background_stop(&self) {}
    /// "Serve this desktop to browsers" (Settings -> Remote access): the switch, the port and what the listener reports.
    /// Unsupported where there is no desktop to serve (a cloud pod).
    fn web_access(&self, _p: &Principal) -> ApiResult<Value> {
        Err(ApiError::Unsupported("this machine has no desktop to serve to browsers".into()))
    }
    /// Switch it on or off and/or choose the port (`Some(0)`: the default). Returns what changed, as the audit line says it.
    fn set_web_access(&self, _p: &Principal, _on: Option<bool>, _port: Option<u16>) -> ApiResult<Vec<(String, String)>> {
        Err(ApiError::Unsupported("this machine has no desktop to serve to browsers".into()))
    }
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
