//! Types moved out of `iyakki_agent::tasks` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// Never more than this many tasks run at once, whatever the setting says.
pub const HARD_CAP: usize = 6;

/// Tasks one `task_spawn` call may make.
pub const MAX_PER_CALL: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    /// An approval or a question waits for the person.
    NeedsYou,
    Paused,
    Failed,
    Done,
    Merged,
    Discarded,
    /// The window closed while it ran; Resume continues from the saved place.
    Interrupted,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::NeedsYou => "needs you",
            Status::Paused => "paused",
            Status::Failed => "failed",
            Status::Done => "done",
            Status::Merged => "merged",
            Status::Discarded => "discarded",
            Status::Interrupted => "interrupted",
        }
    }
    /// Takes a worker thread.
    pub fn active(self) -> bool {
        matches!(self, Status::Running | Status::NeedsYou)
    }
    /// Over for good: the rows are free and the tree is gone.
    pub fn finished(self) -> bool {
        matches!(self, Status::Merged | Status::Discarded)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Git,
    Copy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budgets {
    pub max_steps: usize,
    pub max_minutes: u64,
    /// 0: Autopilot's own default.
    pub max_tokens: u64,
    /// 0: not capped.
    pub max_cost_cents: u64,
}

impl Default for Budgets {
    fn default() -> Budgets {
        Budgets { max_steps: 40, max_minutes: 30, max_tokens: 0, max_cost_cents: 0 }
    }
}

/// What a task is made from.
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    pub title: String,
    pub goal: String,
    pub rows: Vec<String>,
    pub agent: String,
    pub model: String,
    pub budgets: Option<Budgets>,
    /// Start although the rows depend on rows that are not done (only the person says so, in the Tasks tab).
    pub override_deps: bool,
    /// Start from this commit when the workspace has it (a task sent by a fleet planner names the commit it was planned on); empty: the last commit.
    pub base_commit: String,
    /// The fleet member that sent the task (empty for a task of the person's own).
    pub from_member: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub task: String,
    pub call_id: String,
    pub name: String,
    pub summary: String,
    pub detail: String,
}

/// The answer of the dry run before a merge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeCheck {
    pub clean: bool,
    pub conflicts: Vec<String>,
    /// Files the task changed that have uncommitted changes in the main workspace.
    pub blocking: Vec<String>,
    pub files: usize,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatWho {
    /// The request given to the task (the goal, or a follow-up).
    You,
    /// What the agent said.
    Agent,
    /// A tool the agent called, with what it answered (one short line).
    Tool,
    /// The harness: an approval asked for or decided, a note, the end of the run.
    Note,
}

/// What to do with rows that become ready when an agent finishes one: nothing, offer them, or start tasks for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAssign {
    Off,
    Ask,
    On,
}

impl AutoAssign {
    pub fn key(self) -> &'static str {
        match self {
            AutoAssign::Off => "off",
            AutoAssign::Ask => "ask",
            AutoAssign::On => "on",
        }
    }
    pub fn parse(s: &str) -> AutoAssign {
        match s.trim().to_lowercase().as_str() {
            "off" => AutoAssign::Off,
            "on" => AutoAssign::On,
            _ => AutoAssign::Ask,
        }
    }
    pub fn next(self) -> AutoAssign {
        match self {
            AutoAssign::Off => AutoAssign::Ask,
            AutoAssign::Ask => AutoAssign::On,
            AutoAssign::On => AutoAssign::Off,
        }
    }
}

// ---- moved from iyakki_agent::tasks (F-SPLIT-03) ----

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub rows: Vec<String>,
    /// `builtin`, `claude-code`, `copilot`, or a custom agent's id.
    pub agent: String,
    pub model: String,
    pub status: Status,
    pub mode: Mode,
    pub branch: String,
    pub base: String,
    pub tree: PathBuf,
    pub session: String,
    pub namespace: String,
    pub port: u16,
    pub created: i64,
    pub started: i64,
    pub ended: i64,
    pub tokens: u64,
    pub cost_cents: f64,
    pub progress_done: usize,
    pub progress_total: usize,
    pub last: String,
    pub summary: String,
    pub error: String,
    pub note: String,
    pub budgets: Budgets,
    /// Every row of the task was verified in its tree.
    pub verified: bool,
    pub kept: bool,
    pub tests: String,
    /// What each row's status was before the task touched it (restored on discard).
    pub row_prev: BTreeMap<String, String>,
    /// Start again from the saved place (a resume) rather than from the goal.
    pub resume: bool,
    pub interjection: String,
    /// The person allowed this task to start although a row it takes waits for rows that are not done.
    pub override_deps: bool,
    /// The id of the run log of the task's latest run (`runs::RunLog`, listed in RUNS with the task's name); new at every start.
    pub run_id: String,
    /// The fleet member that sent this task (empty for the person's own tasks): the Tasks tab marks it.
    #[serde(default)]
    pub from_member: String,
}

impl Default for Task {
    fn default() -> Task {
        Task { id: String::new(), title: String::new(), goal: String::new(), rows: vec![], agent: "builtin".into(), model: String::new(), status: Status::Queued, mode: Mode::Git, branch: String::new(), base: String::new(), tree: PathBuf::new(), session: String::new(), namespace: String::new(), port: 0, created: 0, started: 0, ended: 0, tokens: 0, cost_cents: 0.0, progress_done: 0, progress_total: 0, last: String::new(), summary: String::new(), error: String::new(), note: String::new(), budgets: Budgets::default(), verified: false, kept: false, tests: String::new(), row_prev: BTreeMap::new(), resume: false, interjection: String::new(), override_deps: false, run_id: String::new(), from_member: String::new() }
    }
}

impl Task {
    /// What the person (and the assistant's footer) reads: "running · 2/5 rows".
    pub fn progress_text(&self) -> String {
        if self.progress_total > 0 {
            format!("{}/{} rows", self.progress_done, self.progress_total)
        } else {
            "no plan yet".into()
        }
    }

    /// The seconds it has been (or was) working.
    pub fn elapsed(&self, now: i64) -> i64 {
        if self.started == 0 {
            return 0;
        }
        let end = if self.status.active() || self.ended == 0 { now } else { self.ended };
        (end - self.started).max(0)
    }

    pub fn dir_of(root: &Path, id: &str) -> PathBuf {
        root.join(".iyakki/tasks").join(id)
    }
}

/// What the runner needs to start (or resume) a task's run.
#[derive(Clone, Debug)]
pub struct RunSpec {
    pub task: Task,
    pub root: PathBuf,
    pub tree: PathBuf,
    pub dir: PathBuf,
    /// The task's own board (outside the tree).
    pub features: PathBuf,
    pub state_path: PathBuf,
    /// The goal text of the first step, with the rows, the port and the rules of a task.
    pub prompt: String,
    pub resume: bool,
    pub interjection: Option<String>,
    /// The agent's display name (`Agent 2`): the key of its entry in the agent registry is the task id.
    pub agent_name: String,
    /// The task's live approval mode (Autopilot at every start; the person may switch it while the task runs).
    pub live: Option<Arc<crate::livemode::LiveMode>>,
}

/// One line of a task's embedded chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub who: ChatWho,
    pub text: String,
}


/// What a task's run looked like so far, for the run log.
#[derive(Clone, Debug)]
pub struct RunRecord {
    pub run_id: String,
    pub task: Task,
    /// `Agent 2`
    pub agent: String,
    pub events: Vec<crate::agent::RunEvent>,
    /// `running`, `completed`, `failed` or `stopped`.
    pub status: String,
    pub started: i64,
}

/// Runs one task: the spec in, the events out.
pub type Runner = Arc<dyn Fn(RunSpec) -> crate::agent::RunHandle + Send + Sync>;

// ---- moved from iyakki-agent (F-SPLIT-03) ----

/// The question to ask before the IDE closes, or `None` when nothing is going on.
pub fn close_question(tasks: &[Task]) -> Option<String> {
    let running: Vec<&Task> = tasks.iter().filter(|t| t.status.active()).collect();
    if running.is_empty() {
        return None;
    }
    let names = running.iter().map(|t| format!("{} ({})", t.title, t.status.label())).collect::<Vec<_>>().join(", ");
    Some(format!("{} task(s) are still working: {names}. Closing stops them; each one shows as interrupted next time and can be resumed. Close anyway?", running.len()))
}
