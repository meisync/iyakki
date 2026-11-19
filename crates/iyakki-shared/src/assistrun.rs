//! Types moved out of `iyakki_agent::assistrun` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::sync::Arc;
use crate::agent::ToolDef;
use crate::chat::Message;

/// Builds the model's history on the run's own thread (knowledge-base recall, skills, condensing), given the tools that exist.
/// The chat uses it so nothing slow happens on the UI thread.
pub type Prepare = Arc<dyn Fn(&[ToolDef]) -> Vec<Message> + Send + Sync>;

// ---- moved from iyakki_agent::assistrun (F-SPLIT-03) ----

use std::collections::BTreeSet;
use std::time::Duration;
use crate::toolblock::{Group, ReminderCtx};


/// What the loop adds around the model. Cheap to clone.
#[derive(Clone)]
pub struct Assist {
    /// Replaces the history given to the run (see [`Prepare`]).
    pub prepare: Option<Prepare>,
    /// The "TOOLS AVAILABLE NOW" block for the request (None = none).
    pub reminder: Option<ReminderCtx>,
    /// Retry once when the model denies a capability it has or asks permission for a read-only lookup.
    pub guard: bool,
    /// Repair tool calls; check arguments against the schema; text protocol fallback.
    pub repair: bool,
    /// Offer only the tool groups the message suggests (plus `more_tools`).
    pub lazy_groups: bool,
    /// Groups the chat's context needs from the first request (a flow open, a dashboard, an IDE workspace, a browser page).
    pub context_groups: BTreeSet<Group>,
    /// Run independent read-only calls of one model turn at the same time.
    pub parallel: bool,
    /// Ask the backend for the answer as it is written (`RunEvent::Delta`).
    pub stream: bool,
    /// Emit `RunEvent::Timing` at the end.
    pub timing: bool,
    /// A time limit per tool call (the table in [`tool_timeout`]) instead of none.
    pub tool_timeouts: bool,
    /// Keep the request under this many (estimated) tokens by shortening old tool results (0 = no cap).
    pub cap_tokens: u64,
    /// Longest tool result kept (0 = the run limits' value).
    pub max_result_chars: usize,
    /// Tell Ollama models not to think before answering (seconds saved per turn; see `providers::set_thread_reasoning`).
    pub fast_reasoning: bool,
    /// Autopilot's final step when no question may be asked (the goal is complete): a closing question is sent back once and `ask_user` is refused.
    pub no_questions: bool,
    /// Set by Autopilot: a step that ends with advice instead of a tool call, while rows are still open, is nudged to act.
    pub autopilot: bool,
    /// Count only what is new: generated tokens plus the part of each prompt that was not already sent (the resent history is not
    /// counted again). Autopilot sets it, so its token budget measures work, not the length of the conversation.
    pub count_new_tokens: bool,
    /// Pure thinking (no answer, no tool call) longer than this abandons the model call: retried once with shorter thinking, then stopped.
    pub think_limit: Duration,
    /// No token of any kind for this long abandons the model call (the same retry).
    pub silence_limit: Duration,
    /// Never wait longer than this for one tool (tests; None = the table in [`tool_timeout`]).
    pub tool_timeout_cap: Option<Duration>,
}

impl Assist {
    pub fn off() -> Assist {
        Assist { prepare: None, reminder: None, guard: false, repair: false, lazy_groups: false, context_groups: BTreeSet::new(), parallel: false, stream: false, timing: false, tool_timeouts: false, cap_tokens: 0, max_result_chars: 0, fast_reasoning: false, no_questions: false, autopilot: false, count_new_tokens: false, think_limit: Duration::from_secs(60), silence_limit: Duration::from_secs(120), tool_timeout_cap: None }
    }

    pub fn standard(ctx: ReminderCtx) -> Assist {
        Assist { prepare: None, reminder: Some(ctx), guard: true, repair: true, lazy_groups: true, context_groups: BTreeSet::new(), parallel: true, stream: true, timing: true, tool_timeouts: true, cap_tokens: 6000, max_result_chars: 6000, fast_reasoning: std::env::var("IYAKKI_THINK").is_err(), no_questions: false, autopilot: false, count_new_tokens: false, think_limit: Duration::from_secs(60), silence_limit: Duration::from_secs(120), tool_timeout_cap: None }
    }
}
