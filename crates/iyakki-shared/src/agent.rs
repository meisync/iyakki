//! Types moved out of `iyakki_agent::agent` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use serde::Deserialize;
use serde::Serialize;
use crate::chat::ToolSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Risk {
    Safe,
    Ask,
}

#[derive(Clone, Debug)]
pub struct ToolDef {
    pub spec: ToolSpec,
    pub risk: Risk,
    /// Output comes from outside (files, web, documents, other programs): label it untrusted.
    pub untrusted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLimits {
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub max_seconds: u64,
    pub max_result_chars: usize,
    /// How long to wait for the user to approve a tool call.
    pub approval_timeout: Duration,
    /// Budgets (0 = unlimited). Tokens are estimated (~4 characters each): prompt + reply, per model turn.
    /// They are checked before each model call against the next prompt's size, so a run can overshoot
    /// by at most one reply.
    pub max_tokens: u64,
    pub max_files: u32,
    pub max_cost_cents: u32,
    /// What a million tokens cost, in US cents (0 = cost not tracked).
    pub price_cents_per_mtok: u32,
    /// Approval mode Auto: calls that would ask run without asking, except those `hard_ask` names. Audited as `auto-approved`.
    pub auto: bool,
    /// The chat's session approvals ("allow for this session"), shared by every run of the chat; None: grants last one run.
    pub grants: Option<Arc<crate::grants::Grants>>,
    /// The chat's live approval mode ([`crate::livemode`]): when set it replaces `auto` and is read at every decision, so the
    /// person can switch modes while the run is going.
    pub live: Option<Arc<crate::livemode::LiveMode>>,
}

impl RunLimits {
    /// Auto approvals apply right now: the live mode when the run has one, else `auto` as it was at the start.
    pub fn auto_now(&self) -> bool {
        match &self.live {
            Some(l) => l.auto(),
            None => self.auto,
        }
    }

    /// Limits for a user: the standard step/tool caps plus their policy budget.
    pub fn for_budget(b: &iyakki_sec::policy::AgentBudget, price_cents_per_mtok: u32) -> RunLimits {
        let d = RunLimits::default();
        RunLimits { max_seconds: if b.seconds == 0 { u64::MAX / 4 } else { b.seconds }, max_tokens: b.tokens, max_files: b.files, max_cost_cents: b.cost_cents, price_cents_per_mtok, ..d }
    }
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_steps: 12, max_tool_calls: 24, max_seconds: 600, max_result_chars: 12_000, approval_timeout: Duration::from_secs(600), max_tokens: 0, max_files: 0, max_cost_cents: 0, price_cents_per_mtok: 0, auto: false, grants: None, live: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    /// Approve this and every later call to the same tool in this run.
    Always,
    /// Approve this and later calls of the same kind (same tool, same scope: [`crate::grants::scope_of`]) for the rest of the
    /// chat session. Calls that always ask (`Tools::grant_blocker`) are approved once only.
    Session,
    Deny,
}

pub trait Tools: Send + Sync {
    fn defs(&self) -> Vec<ToolDef>;
    /// Execute a call. Errors are returned to the model as the tool's result.
    fn call(&self, name: &str, args: &Value) -> Result<String, String>;
    /// One line describing what a call will do (shown in the approval prompt).
    fn describe(&self, name: &str, args: &Value) -> String;
    /// The tools this set wraps (`None`: it wraps nothing). Every method below that a wrapper does not decide itself goes to it, so a
    /// wrapper that implements `inner_tools` and only its own tools cannot hide what the tools underneath decide (an approval card, a
    /// refusal, a grant rule, open work, notes, running jobs). A wrapper that has an `Arc<dyn Tools>` field MUST implement it: the
    /// test `tools_wrappers_forward` reads the sources and fails for one that does not.
    fn inner_tools(&self) -> Option<&dyn Tools> {
        None
    }
    /// Optional detail shown with the approval prompt (for a file write: the actual diff).
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        self.inner_tools().and_then(|t| t.preview(name, args))
    }
    /// A call that policy refuses outright: the reason is given to the model as the tool's result and nobody is asked
    /// (the "deny" outcome: starting a second cluster, for one).
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        self.inner_tools().and_then(|t| t.refusal(name, args))
    }
    /// A call that would ask is already allowed (a time-limited grant the person gave earlier).
    fn pre_approved(&self, name: &str) -> bool {
        self.inner_tools().is_some_and(|t| t.pre_approved(name))
    }
    /// The person answered "always allow" for this tool: whether that lasts for the rest of the run (a tool may keep
    /// its own, shorter grant instead).
    fn approved_always(&self, name: &str) -> bool {
        self.inner_tools().is_none_or(|t| t.approved_always(name))
    }
    /// Why this call is never covered by an "allow for this session" grant (`None`: it may be). The default is the
    /// `hard_ask` list plus drive writes ([`crate::grants::blocker`]); a tool set adds its own (a remote cluster).
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        match self.inner_tools() {
            Some(t) => t.grant_blocker(name, args),
            None => crate::grants::blocker(name, args),
        }
    }
    /// What a session grant for this call covers ([`crate::grants::scope_of`]); a tool set adds what it knows (the cluster).
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        match self.inner_tools() {
            Some(t) => t.grant_scope(name, args),
            None => crate::grants::scope_of(name, args),
        }
    }
    /// Work the tools themselves track and that is still open at the end of a run (board rows the assistant added that are
    /// not done), one short label each. The run asks the model once to settle them when its final answer claims progress.
    fn unresolved_work(&self) -> Vec<String> {
        self.inner_tools().map_or_else(Vec::new, |t| t.unresolved_work())
    }
    /// The final answer says something about the tools' own state that is not true (a board row "marked done" that is not): one plain
    /// correction line to append to the answer, or none. It never changes anything but the text shown.
    fn correct_claims(&self, answer: &str) -> Option<String> {
        self.inner_tools().and_then(|t| t.correct_claims(answer))
    }
    /// An Autopilot step is starting: reset the per-step hard limits.
    fn begin_step(&self) {
        if let Some(t) = self.inner_tools() {
            t.begin_step()
        }
    }
    /// A tool decided the step must end now (a row was parked): the run stops after the current call with this text as its answer.
    fn should_end_run(&self) -> Option<String> {
        self.inner_tools().and_then(|t| t.should_end_run())
    }
    /// The diagnosis of the last failed verification (row id, text), handed over once to the next Autopilot step.
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner_tools().and_then(|t| t.take_diagnosis())
    }
    /// An external coding agent ran one of its own tools (`Write`, `Edit`, `Bash`) after IYAKKI approved it: tools that track
    /// the work (the feature board's checks) learn of it as they would of the same call made through them.
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        if let Some(t) = self.inner_tools() {
            t.note_external(name, args, ok)
        }
    }
    /// Things the assistant should be told before its next model turn (a detached job finished), each once. The run adds them
    /// to the conversation as they come.
    fn take_notes(&self) -> Vec<String> {
        self.inner_tools().map_or_else(Vec::new, |t| t.take_notes())
    }
    /// Labels of the detached jobs still running for this chat. Autopilot treats a step that ended with a job running as waiting,
    /// not as stalled, and waits for the job.
    fn jobs_running(&self) -> Vec<String> {
        self.inner_tools().map_or_else(Vec::new, |t| t.jobs_running())
    }
}


use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallInfo {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// An assistant turn: optional text and any tool requests.
    Model { text: String, calls: Vec<CallInfo> },
    /// Waiting for the user to decide.
    Approval {
        id: String,
        name: String,
        summary: String,
        /// What exactly will change (a diff for file writes); empty when there is nothing more to show.
        #[serde(default)]
        detail: String,
    },
    /// How a call was decided: `by` is `auto` (safe), `user`, `always` or `system` (timeout, limits).
    Decision { id: String, name: String, approved: bool, by: String },
    ToolResult { id: String, name: String, ok: bool, text: String },
    /// Resources used so far (after each model turn and each written file).
    Usage { tokens: u64, cost_cents: f64, files: u32, seconds: u64 },
    Final { text: String },
    Failed { error: String },
    Stopped { reason: String },
    /// A line from the assistant's safety net (a retry, the plain-text tool protocol): shown as a step in the chat.
    Note { text: String },
    /// The answer so far, while the model is still writing it (never recorded).
    Delta { text: String },
    /// The model's thinking so far and how many seconds it has been thinking (never recorded, never sent back to the model).
    Thinking { text: String, secs: u64 },
    /// Where the time went (milliseconds): the model, the tools, and everything else of ours (approval waits excluded).
    /// Where the tokens of the latest request went (never sent back to the model).
    Budget { bd: crate::tokenbudget::Breakdown },
    Timing { first_token_ms: u64, first_text_ms: u64, total_ms: u64, model_ms: u64, tools_ms: u64, overhead_ms: u64, pre_request_ms: u64, prepare_ms: u64, thinking_ms: u64, rounds: u32 },
}


pub struct RunHandle {
    pub events: Receiver<RunEvent>,
    pub decisions: Sender<(String, Verdict)>,
    pub cancel: Arc<AtomicBool>,
    /// Set by Autopilot runs: asks the run to stop after saving its place (see `crate::autopilot`).
    pub pause: Option<Arc<AtomicBool>>,
}

impl RunHandle {
    /// A handle around an event stream made elsewhere (a run that failed before it started).
    pub fn from_parts(events: Receiver<RunEvent>, decisions: Sender<(String, Verdict)>) -> RunHandle {
        RunHandle { events, decisions, cancel: Arc::new(AtomicBool::new(false)), pause: None }
    }

    pub fn decide(&self, call_id: &str, v: Verdict) {
        let _ = self.decisions.send((call_id.to_string(), v));
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Pause an Autopilot run: it stops after saving where it is, and can be resumed. Plain runs just stop.
    pub fn pause(&self) {
        match &self.pause {
            Some(p) => p.store(true, Ordering::SeqCst),
            None => self.cancel(),
        }
    }

    /// A way to answer approvals from somewhere else (the INBOX), besides [`RunHandle::decide`].
    pub fn decision_sender(&self) -> Sender<(String, Verdict)> {
        self.decisions.clone()
    }

    /// The switch that stops the run, to hand to something that can stop it from outside (the SYSTEM tile).
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn try_event(&self) -> Option<RunEvent> {
        self.events.try_recv().ok()
    }
}

/// For the "test" button and anything else that needs a `Tools` with nothing in it.
pub struct NoTools;

impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _args: &Value) -> Result<String, String> {
        Err(format!("no tool called {name}"))
    }
    fn describe(&self, name: &str, _args: &Value) -> String {
        name.into()
    }
}

/// A rough count of the tokens of a history (4 per message plus a quarter of the characters).
pub fn estimate_tokens(history: &[crate::chat::Message]) -> u64 {
    history.iter().map(|m| 4 + (m.content.len() + m.tool_calls.iter().map(|c| c.name.len() + c.args.to_string().len()).sum::<usize>()) as u64 / 4).sum()
}
