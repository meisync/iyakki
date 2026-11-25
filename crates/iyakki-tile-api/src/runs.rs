//! The plain data and the ports of the RUNS tile and of the Jobs strip (D-S1-E4-01, option (b); owner brief alpha.8 E5). The recorded runs
//! (what the model asked for, what the person approved, what each tool answered) and the detached jobs are plain mirrors of
//! `iyakki_shared::runs::{RunMeta, RunLog, Integrity}`, `iyakki_shared::agent::RunEvent` and `iyakki_shared::jobs::{Job, State}`; the
//! shell converts one to the other (a test keeps them in step). A mirror keeps only what the screens read: a tool call's arguments come
//! as their one-line JSON text, and the events the screens never draw (deltas, thinking, the token budget) are one `Hidden` variant.

use std::time::Duration;

/// How long a stopped job gets between TERM and KILL.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// The exit code of a step that ran out of time (the `timeout` program's convention).
pub const TIMED_OUT_EXIT: i32 = 124;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMeta {
    pub id: String,
    pub started: u64,
    pub prompt: String,
    pub status: String,
    pub tool_calls: usize,
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

/// One tool request of a model turn: the name and the arguments as one line of JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub args_text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunEvent {
    Model { text: String, calls: Vec<Call> },
    Approval { summary: String, detail: String },
    /// `by` is `auto`, `user`, `always`, `grant` or `system`.
    Decision { name: String, approved: bool, by: String },
    ToolResult { name: String, ok: bool, text: String },
    Usage { tokens: u64, cost_cents: f64, files: u32, seconds: u64 },
    Note { text: String },
    Timing { first_token_ms: u64, first_text_ms: u64, total_ms: u64, model_ms: u64, tools_ms: u64, overhead_ms: u64, thinking_ms: u64 },
    Final { text: String },
    Failed { error: String },
    Stopped { reason: String },
    /// An event the record screen never draws (the answer so far, the thinking, the token budget).
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunLog {
    pub id: String,
    pub prompt: String,
    pub events: Vec<RunEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Verified,
    Tampered(String),
}

/// What the RUNS tile asks of the rest of the system.
pub trait RunsPort {
    /// The recorded runs of the signed-in person, newest first.
    fn list(&self) -> Vec<RunMeta>;
    /// One run's record and whether its hash chain is intact.
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)>;
    /// Delete one of the person's runs.
    fn delete(&self, id: &str) -> Result<(), String>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Succeeded,
    /// Ended with this exit code (128 + n when a signal ended it).
    Failed(i32),
    Stopped,
    Lost,
}

impl JobState {
    pub fn label(&self) -> String {
        match self {
            JobState::Running => "running".into(),
            JobState::Succeeded => "succeeded".into(),
            JobState::Failed(TIMED_OUT_EXIT) => "timed out".into(),
            JobState::Failed(c) => format!("failed (exit {c})"),
            JobState::Stopped => "stopped".into(),
            JobState::Lost => "lost".into(),
        }
    }
}

/// A detached job as the strip and the list see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub label: String,
    /// The command line it runs (the shell tells install jobs apart by it).
    pub command: String,
    pub state: JobState,
    pub started_ms: u64,
    /// 0 while running.
    pub ended_ms: u64,
    /// 0..=100 when the output says how far it is, else None (indeterminate).
    pub progress: Option<f32>,
    /// The last line of output, cleaned and redacted.
    pub last_line: String,
    /// When the job last showed a sign of work, in ms (0 for a job that is not running).
    pub last_active_ms: u64,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == JobState::Running
    }
    /// Seconds since the running job last showed a sign of work (0 when it did just now or is not running).
    pub fn quiet_secs(&self, now_ms: u64) -> u64 {
        if !self.running() || self.last_active_ms == 0 {
            return 0;
        }
        now_ms.saturating_sub(self.last_active_ms) / 1000
    }
    /// Seconds it has run (to now while running).
    pub fn elapsed_secs(&self, now_ms: u64) -> u64 {
        let end = if self.ended_ms > 0 { self.ended_ms } else { now_ms };
        end.saturating_sub(self.started_ms) / 1000
    }
}

/// `3m 12s`, `45s`, `1h 05m`.
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// One line for the transcript when a job ended (the chat shows it as an event).
pub fn finish_line(j: &Job, now_ms: u64) -> String {
    let mark = match j.state {
        JobState::Succeeded => "finished",
        JobState::Failed(_) => "failed",
        JobState::Stopped => "was stopped",
        _ => "was lost",
    };
    format!("Job “{}” ({}) {mark}: {} after {}", j.label, j.id, j.state.label(), fmt_duration(j.elapsed_secs(now_ms)))
}

/// What the Jobs strip and the job list ask of the rest of the system (the same port serves the chat, the AI IDE and RUNS).
pub trait JobsPort {
    /// The jobs of the signed-in person: those of workspace `ws`, or every workspace when `None`.
    fn visible(&self, ws: Option<&str>) -> Vec<Job>;
    /// The lines for jobs of workspace `ws` that finished since the last look (each once).
    fn take_finished(&self, ws: &str) -> Vec<String>;
    /// How long the job's log is, in bytes.
    fn log_len(&self, id: &str) -> u64;
    /// The tail of the job's log (at most `max` bytes).
    fn log_text(&self, id: &str, max: usize) -> Result<String, String>;
    /// A job that stops job `id` (TERM, a pause, KILL): the tile runs it on a thread of its own, never on the drawing thread.
    fn stop_job(&self, id: &str) -> Box<dyn FnOnce() + Send>;
    /// Remove a finished job from the list.
    fn dismiss(&self, id: &str) -> Result<(), String>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}
