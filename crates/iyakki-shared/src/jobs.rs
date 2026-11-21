//! Types moved out of `iyakki_agent::jobs` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::PathBuf;
use std::time::Duration;
use serde::Deserialize;
use serde::Serialize;

pub const STOP_GRACE: Duration = Duration::from_secs(3);

/// The exit code of a step that ran out of time (the `timeout` program's convention).
pub const TIMED_OUT_EXIT: i32 = 124;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Running,
    Succeeded,
    /// Ended with this exit code (128 + n when a signal ended it).
    Failed(i32),
    Stopped,
    Lost,
}

impl State {
    pub fn finished(&self) -> bool {
        *self != State::Running
    }
    pub fn label(&self) -> String {
        match self {
            State::Running => "running".into(),
            State::Succeeded => "succeeded".into(),
            State::Failed(TIMED_OUT_EXIT) => "timed out".into(),
            State::Failed(c) => format!("failed (exit {c})"),
            State::Stopped => "stopped".into(),
            State::Lost => "lost".into(),
        }
    }
}

/// Who is told about a finished job: the chat (a visible event), the assistant (a note at its next turn) and the inbox are told once each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Ui,
    Agent,
    /// A note in the person's INBOX (the bell), so a job that ends while they are in another app is not missed.
    Inbox,
}

/// What to run: a shell line (`sh -c`), or a program with its arguments (no shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Shell(String),
    Argv(Vec<String>),
}

impl Cmd {
    pub fn text(&self) -> String {
        match self {
            Cmd::Shell(s) => s.clone(),
            Cmd::Argv(a) => crate::exec::command_line(a.first().map_or("", String::as_str), &a.iter().skip(1).cloned().collect::<Vec<_>>()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub label: String,
    pub cmd: Cmd,
    pub cwd: PathBuf,
    /// The workspace the job belongs to (its name), empty outside the IDE.
    pub workspace: String,
}

/// A job as the tools and the screen see it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub owner: String,
    pub workspace: String,
    pub pid: u32,
    pub state: State,
    pub started_ms: u64,
    /// 0 while running.
    pub ended_ms: u64,
    /// 0..=100 when the output says how far it is, else None (indeterminate).
    pub progress: Option<f32>,
    /// The last line of output, cleaned and redacted.
    pub last_line: String,
    /// When the job last showed a sign of work, in ms: new output in its log, or CPU time used by any process of its group (checked
    /// each time the job is read). 0 for a job that is not running. The screen's spinner moves only while this is recent.
    pub last_active_ms: u64,
    pub dismissed: bool,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == State::Running
    }
    /// Seconds it has run (to now while running).
    /// Seconds since the running job last showed a sign of work (0 when it did just now or is not running).
    pub fn quiet_secs(&self, now_ms: u64) -> u64 {
        if !self.running() || self.last_active_ms == 0 {
            return 0;
        }
        now_ms.saturating_sub(self.last_active_ms) / 1000
    }

    pub fn elapsed_secs(&self, now_ms: u64) -> u64 {
        let end = if self.ended_ms > 0 { self.ended_ms } else { now_ms };
        end.saturating_sub(self.started_ms) / 1000
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `3m 12s`, `45s`, `1h 05m`.
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// The same facts on one line, for the chat's visible event.
pub fn finish_line(j: &Job) -> String {
    let mark = match j.state {
        State::Succeeded => "finished",
        State::Failed(_) => "failed",
        State::Stopped => "was stopped",
        _ => "was lost",
    };
    format!("Job “{}” ({}) {mark}: {} after {}", j.label, j.id, j.state.label(), fmt_duration(j.elapsed_secs(now_ms())))
}

/// Does a process with this id exist (any user)? Signal 0 only checks; "not permitted" still means it exists.
pub fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 sends nothing.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}
