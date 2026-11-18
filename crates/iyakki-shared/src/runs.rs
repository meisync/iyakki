//! Types moved out of `iyakki_agent::runs` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMeta {
    pub id: String,
    pub started: u64,
    pub prompt: String,
    pub status: String,
    pub tool_calls: usize,
    /// Where the time went, in milliseconds (0 for runs recorded before this was measured).
    pub model_ms: u64,
    pub tools_ms: u64,
    /// Everything that was ours: building the request, repairing calls, queues, drawing hand-over.
    pub overhead_ms: u64,
    pub first_token_ms: u64,
    pub total_ms: u64,
    pub task_id: Option<String>,
    pub row: Option<String>,
    pub agent: Option<String>,
}

impl RunMeta {
    /// `Task t1a2b3 · Agent 2 · F-APP-01`, empty for a run that is not a task's.
    pub fn task_label(&self) -> String {
        let Some(t) = &self.task_id else { return String::new() };
        let mut s = format!("Task {t}");
        for p in [&self.agent, &self.row].into_iter().flatten().filter(|x| !x.is_empty()) {
            s.push_str(" · ");
            s.push_str(p);
        }
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Verified,
    Tampered(String),
}

/// A run that is going on now, for the SYSTEM tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub id: String,
    pub user: String,
    /// The start of what was asked.
    pub prompt: String,
    pub started: i64,
    /// Tool calls waiting for an answer.
    pub pending: usize,
    /// Events so far.
    pub steps: usize,
    /// Tokens used so far, when the run has reported them.
    pub tokens: u64,
    /// What happened last, in a few words.
    pub last: String,
}

/// A tool call that is waiting for the person's answer, as the INBOX shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub run_id: String,
    pub user: String,
    /// The start of what the run was asked.
    pub prompt: String,
    pub call_id: String,
    pub tool: String,
    /// One line on what the call will do.
    pub summary: String,
    /// What exactly will change, when there is more to show.
    pub detail: String,
    /// `low`, `medium` or `high`.
    pub risk: &'static str,
    /// When the call started waiting.
    pub since: i64,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::agent::RunEvent;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    pub id: String,
    pub user: String,
    pub started: u64,
    pub prompt: String,
    /// `running`, `completed`, `failed` or `stopped`.
    pub status: String,
    pub events: Vec<RunEvent>,
    /// The chat this run belongs to, so deleting the chat can delete its runs.
    #[serde(default)]
    pub session: Option<String>,
    /// A parallel task's run: the task, the board row and the agent's display name (`Agent 2`). Old logs have none.
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub row: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}
