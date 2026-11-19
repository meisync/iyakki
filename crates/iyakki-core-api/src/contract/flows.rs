//! Flows: list, start, stop, status, a run event stream, approvals and the camera-frame handover.

use super::context::CallContext;
use super::error::{CallResult, IdempotencyKey};
use super::event::{EventBatch, Resume};
use super::stream::StreamHandle;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RunId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    WaitingApproval,
    Succeeded,
    Failed,
    Stopped,
}

impl RunState {
    pub fn is_finished(self) -> bool {
        matches!(self, RunState::Succeeded | RunState::Failed | RunState::Stopped)
    }
}

/// A question a run asks before a step. `local_only` approvals (camera, microphone, anything touching a consent kind) can be
/// answered only at the machine; a remote client with `flows:approve` may answer the others.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub text_key: String,
    pub text: String,
    pub local_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatus {
    pub run: RunId,
    pub flow: String,
    pub state: RunState,
    pub pending_approval: Option<Approval>,
    pub step: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlowEvent {
    Started { run: RunId, flow: String },
    Step { run: RunId, name: String },
    ApprovalNeeded { run: RunId, approval: Approval },
    ApprovalAnswered { run: RunId, approval_id: String, allowed: bool, by: String },
    /// A camera frame handed to the run; read it with [`super::Streams`].
    Frame { run: RunId, stream: StreamHandle },
    Finished { run: RunId, state: RunState },
}

pub trait Flows: Send + Sync {
    fn list(&self, ctx: &CallContext) -> CallResult<Vec<FlowInfo>>;
    /// Starting twice with the same idempotency key (and flow) returns the same run and starts nothing new. The same key for a
    /// different flow is a `conflict`.
    fn start(&self, ctx: &CallContext, flow: &str, idem: &IdempotencyKey) -> CallResult<RunId>;
    fn stop(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus>;
    fn status(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus>;
    /// Events of all runs, or of one run.
    fn events(&self, ctx: &CallContext, run: Option<&RunId>, resume: Resume, limit: usize) -> CallResult<EventBatch<FlowEvent>>;
    fn approve(&self, ctx: &CallContext, run: &RunId, approval_id: &str, allow: bool) -> CallResult<RunStatus>;
}
