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
    /// A state a newer release added; a caller shows the run as not finished and asks again later.
    #[serde(other)]
    Unknown,
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
    /// An event of a type a newer release added; a caller skips it.
    #[serde(other)]
    Unknown,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_succeeded_failed_and_stopped_runs_count_as_finished() {
        assert!(RunState::Succeeded.is_finished());
        assert!(RunState::Failed.is_finished());
        assert!(RunState::Stopped.is_finished());
        assert!(!RunState::Running.is_finished());
        assert!(!RunState::WaitingApproval.is_finished());
    }

    #[test]
    fn a_run_state_from_a_newer_release_is_not_finished_so_a_caller_asks_again_later() {
        assert!(!RunState::Unknown.is_finished());
        let s: RunState = serde_json::from_str("\"paused_by_a_newer_release\"").unwrap();
        assert_eq!(s, RunState::Unknown);
        assert!(!s.is_finished());
    }

    #[test]
    fn the_known_run_states_read_from_their_snake_case_names() {
        assert_eq!(serde_json::from_str::<RunState>("\"waiting_approval\"").unwrap(), RunState::WaitingApproval);
        assert_eq!(serde_json::from_str::<RunState>("\"succeeded\"").unwrap(), RunState::Succeeded);
    }
}
