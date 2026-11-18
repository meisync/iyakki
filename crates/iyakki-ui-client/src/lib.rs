//! The contract client for a UI outside the repository (see the crate manifest), and the reference server for the other end.
//!
//! The transport is one request and one answer per line of JSON over any byte stream (a loopback TCP socket today, the developer-mode
//! channel or a local socket later; the typed calls do not change):
//!
//! ```text
//! -> {"id":1,"call":"settings.get","args":{"key":"display.dark"}}
//! <- {"id":1,"ok":{"type":"bool","value":false}}          or          {"id":1,"err":{CallError}}
//! ```
//!
//! A line is at most [`MAX_LINE`] bytes. Calls: `hello`, `settings.schema`, `settings.get`, `settings.set`, `settings.watch` (and `vault.status|lock|use` and
//! `flows.list|start|stop|status|events|approve`: every domain of the first contract that has a trait; `vault.unlock` and the answer to a
//! consent prompt are not on the transport at all, they need the machine's own seat). A call the machine does not know is answered
//! `unsupported` (never dropped), a damaged line is answered `invalid_input` with the reason and the connection goes on.

pub mod client;
pub mod server;
pub mod socketcore;

pub use iyakki_core_api::contract::{negotiate, CallContext, CallError, CallResult, ClientHello, ContractVersion, ErrorCode, EventBatch, Hello, IdempotencyKey, ActionOutcome, Approval, FlowEvent, FlowInfo, Flows, NamedAction, RunId, RunState, RunStatus, Vault, VaultStatus, Negotiated, Resume, Risk, Scope, SettingChange, SettingEvent, SettingKind, SettingSpec, SettingValue, Settings, Transport, CONTRACT_VERSION};

/// The longest line either side accepts.
pub const MAX_LINE: usize = 64 * 1024;
/// The most calls one connection may make (a UI that needs more opens another connection).
pub const MAX_CALLS_PER_CONNECTION: u64 = 100_000;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub call: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<CallError>,
}

impl Response {
    pub fn ok(id: u64, v: Value) -> Response {
        Response { id, ok: Some(v), err: None }
    }
    pub fn err(id: u64, e: CallError) -> Response {
        Response { id, ok: None, err: Some(e) }
    }
}
