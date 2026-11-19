//! Types moved out of `iyakki_agent::autopilot` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use crate::agent::RunLimits;
use crate::assistrun::Prepare;

/// What one step asks of whoever carries it out: the step message (goal, PROJECT STATE, current row, last diagnosis), the
/// context rebuild for the system prompt and skills, and the hard caps of the step.
pub struct StepCall {
    pub prompt: String,
    pub prepare: Option<Prepare>,
    pub limits: RunLimits,
    pub step: usize,
}

/// Carries out the steps of an Autopilot run (an external coding agent); `None` in the context: the chat's model.
pub type Executor = std::sync::Arc<dyn Fn(StepCall) -> crate::agent::RunHandle + Send + Sync>;
