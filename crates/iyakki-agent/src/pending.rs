//! Waiting that actually waits (F-AI-16, `docs/development/AGENT-WAIT-PLAN.md`): the pending-work ledger.
//!
//! A language model cannot wait: it answers once and nothing runs until something sends it another turn. The ledger is the
//! program-side memory of "what is still going on" for one person: every asynchronous piece of work (a detached job, a parallel
//! task, a file that must appear, a port that must open, a producer that reports through [`Ledger::report`]) is a **ticket**.
//!
//! * The ledger is persistent (`<root>/<person>.json`, written atomically), so it survives a restart. A ticket that was running
//!   when IYAKKI stopped and has no producer that can say what became of it is marked `lost` and reported, never dropped.
//! * Jobs and parallel tasks are mirrored, not owned: [`Ledger::sync`] opens a ticket for each running job or task and follows its
//!   state. Their own stores stay the truth.
//! * [`Ledger::take_wake`] is the waker's half: when something finished that the assistant has not been told about, it returns one
//!   batched synthetic turn (text for the chat to start a run with), at most once per ticket, within a daily chain limit.
//! * [`promises_to_report`] is the model-free phrase test the end-of-run gate (`assistrun.rs`) uses.
//! * A ticket may carry an acceptance `check` (a command run in the job's folder when it ends): finished means the check
//!   passed, not that the exit code was 0.
//!
//! The ledger stores labels, states and short result lines only (redacted), never command output beyond one line.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::jobs::{fmt_duration, now_ms, Jobs, State as JobState};
use crate::tasks::{Status as TaskStatus, Task};

/// At most this many tickets are kept per person (finished and told ones go first).
const MAX_TICKETS: usize = 200;
/// How long an acceptance check may run.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a single registered wait may last (six hours, like Autopilot's cap).
pub const MAX_WAIT_SECS: u64 = 6 * 3600;
/// The waker looks at the ledger this often.
pub const WAKE_POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Job,
    Task,
    Install,
    Pull,
    Flow,
    Rollout,
    Browser,
    Git,
    /// A file, port or other condition the assistant asked to be told about (`wait_for`).
    Probe,
    External,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Job => "job",
            Source::Task => "task",
            Source::Install => "install",
            Source::Pull => "pull",
            Source::Flow => "flow",
            Source::Rollout => "rollout",
            Source::Browser => "browser",
            Source::Git => "git",
            Source::Probe => "wait",
            Source::External => "external",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    FinishedOk,
    FinishedFailed,
    Stopped,
    Lost,
    TimedOut,
}

impl State {
    pub fn finished(self) -> bool {
        self != State::Running
    }
    pub fn label(self) -> &'static str {
        match self {
            State::Running => "running",
            State::FinishedOk => "finished",
            State::FinishedFailed => "failed",
            State::Stopped => "stopped",
            State::Lost => "lost",
            State::TimedOut => "timed out",
        }
    }
}

/// A condition the ledger itself can test: no producer is needed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Probe {
    /// The file exists.
    File { path: String },
    /// This machine's port accepts a connection (loopback only).
    Port { host: String, port: u16 },
}

impl Probe {
    pub fn describe(&self) -> String {
        match self {
            Probe::File { path } => format!("file {path} exists"),
            Probe::Port { host, port } => format!("{host}:{port} accepts connections"),
        }
    }

    /// Whether the condition holds now.
    pub fn holds(&self) -> bool {
        match self {
            Probe::File { path } => Path::new(path).exists(),
            Probe::Port { host, port } => {
                use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
                let Ok(addrs) = (host.as_str(), *port).to_socket_addrs() else { return false };
                addrs.into_iter().any(|a: SocketAddr| TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_ok())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ticket {
    pub id: String,
    pub source: Source,
    /// The producer's own id (`j3` for a job, a task id), empty for a probe.
    pub key: String,
    pub label: String,
    pub workspace: String,
    pub started_ms: u64,
    /// 0 = no deadline of its own (the settings' maximum still applies).
    pub deadline_ms: u64,
    pub ended_ms: u64,
    pub state: State,
    /// A short result line (redacted).
    pub detail: String,
    /// What the model said it would do when this lands (one line it wrote itself).
    pub on_done: String,
    pub probe: Option<Probe>,
    /// An acceptance command, run in `check_cwd` when the producer finishes with success.
    pub check: String,
    pub check_cwd: String,
    /// The check has run; `check_ok` is its verdict.
    pub checked: bool,
    pub check_ok: bool,
    pub check_line: String,
    /// The assistant knows the final state (a note in a run, or a wake).
    pub told: bool,
    /// Which process opened the ticket (producers without a store cannot outlive it).
    pub boot: u64,
}

impl Default for Ticket {
    fn default() -> Ticket {
        Ticket { id: String::new(), source: Source::External, key: String::new(), label: String::new(), workspace: String::new(), started_ms: 0, deadline_ms: 0, ended_ms: 0, state: State::Running, detail: String::new(), on_done: String::new(), probe: None, check: String::new(), check_cwd: String::new(), checked: false, check_ok: false, check_line: String::new(), told: false, boot: 0 }
    }
}

impl Ticket {
    pub fn running(&self) -> bool {
        self.state == State::Running
    }

    /// Ended and not yet told, and not waiting for its acceptance check to run.
    fn to_tell(&self) -> bool {
        let awaiting_check = self.source == Source::Job && self.state == State::FinishedOk && !self.check.is_empty() && !self.checked;
        self.state.finished() && !self.told && !awaiting_check
    }

    /// `build (job j3)`: one running entry for the "Waiting for" line.
    pub fn short(&self) -> String {
        if self.key.is_empty() { format!("{} ({})", self.label, self.id) } else { format!("{} ({} {})", self.label, self.source.label(), self.key) }
    }

    pub fn line(&self, now: u64) -> String {
        let end = if self.running() { now } else { self.ended_ms.max(self.started_ms) };
        let mut s = format!("{} · {} · {} · {}", self.id, self.label, self.source.label(), self.state.label());
        s.push_str(&format!(" · {}", fmt_duration(end.saturating_sub(self.started_ms) / 1000)));
        if !self.detail.is_empty() {
            s.push_str(&format!(" · {}", self.detail));
        }
        if self.checked {
            s.push_str(if self.check_ok { " · check passed" } else { " · check FAILED" });
        } else if !self.check.is_empty() && self.running() {
            s.push_str(" · check on end");
        }
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Wake the chat when work ends (off: the note waits for the person's next message, the old behaviour).
    pub auto_continue: bool,
    /// Wakes per day (the chain limit).
    pub max_wakes_per_day: u32,
    /// The longest any wait lasts, in seconds.
    pub max_wait_secs: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { auto_continue: true, max_wakes_per_day: 20, max_wait_secs: MAX_WAIT_SECS }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    version: u32,
    next: u64,
    tickets: Vec<Ticket>,
    settings: Settings,
    wakes_day: u64,
    wakes_today: u32,
}

/// A process-wide token: tickets opened by an earlier process that no producer can vouch for are `lost`.
fn boot_token() -> u64 {
    static T: OnceLock<u64> = OnceLock::new();
    *T.get_or_init(|| now_ms().max(1))
}

fn clean_owner(owner: &str) -> String {
    let s: String = owner.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    if s.is_empty() || s.starts_with('.') { "invalid".into() } else { s }
}

/// One person's ledger. Cheap to clone and to open again: the file is the state.
#[derive(Clone)]
pub struct Ledger {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

/// What the waker hands the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Start a turn with this text.
    Turn { text: String, ids: Vec<String> },
    /// The daily chain limit was reached: say so plainly, start nothing.
    Limit { text: String },
}

/// Where the mirrored producers are read from (a chat's or workspace's view of the person's work).
#[derive(Clone)]
pub struct Feed {
    pub jobs: Option<Jobs>,
    pub tasks: Option<Arc<dyn Fn() -> Vec<Task> + Send + Sync>>,
    pub user: String,
    pub workspace: String,
}

impl Feed {
    pub fn new(user: &str, workspace: &str) -> Feed {
        Feed { jobs: None, tasks: None, user: user.into(), workspace: workspace.into() }
    }
}

impl Ledger {
    /// The ledger of `owner` under `root` (the folder is made when something is written).
    pub fn open(root: &Path, owner: &str) -> Ledger {
        let l = Ledger { path: root.join(format!("{}.json", clean_owner(owner))), lock: shared_lock(root) };
        l.recover();
        l
    }

    fn load(&self) -> File {
        let mut f: File = fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        f.version = 1;
        f
    }

    fn save(&self, f: &File) {
        let mut f = File { version: f.version, next: f.next, tickets: f.tickets.clone(), settings: f.settings.clone(), wakes_day: f.wakes_day, wakes_today: f.wakes_today };
        if f.tickets.len() > MAX_TICKETS {
            // finished and told tickets go first, oldest first
            let mut drop = f.tickets.len() - MAX_TICKETS;
            f.tickets.retain(|t| {
                if drop > 0 && t.state.finished() && t.told {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(b) = serde_json::to_vec(&f) {
            if fs::write(&tmp, b).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tickets an earlier process opened and no producer or probe can follow became `lost`: said once, never silently dropped.
    fn recover(&self) {
        let _g = self.guard();
        let mut f = self.load();
        let me = boot_token();
        let mut changed = false;
        for t in f.tickets.iter_mut().filter(|t| t.running() && t.boot != me) {
            // jobs and tasks are re-read from their own stores, probes are simply tested again
            if matches!(t.source, Source::Job | Source::Task | Source::Probe) {
                t.boot = me;
                changed = true;
                continue;
            }
            t.state = State::Lost;
            t.ended_ms = now_ms();
            t.detail = "was still running when IYAKKI stopped; its result is unknown".into();
            changed = true;
        }
        if changed {
            self.save(&f);
        }
    }

    pub fn settings(&self) -> Settings {
        let _g = self.guard();
        self.load().settings
    }

    pub fn set_settings(&self, s: Settings) {
        let _g = self.guard();
        let mut f = self.load();
        f.settings = Settings { max_wakes_per_day: s.max_wakes_per_day.clamp(1, 500), max_wait_secs: s.max_wait_secs.clamp(60, 7 * 24 * 3600), ..s };
        self.save(&f);
    }

    /// Open a ticket. A running ticket for the same producer item is returned instead of a second one.
    pub fn register(&self, mut t: Ticket) -> Ticket {
        let _g = self.guard();
        let mut f = self.load();
        if !t.key.is_empty() {
            if let Some(old) = f.tickets.iter_mut().find(|x| x.source == t.source && x.key == t.key && x.running()) {
                if !t.on_done.is_empty() {
                    old.on_done = t.on_done.clone();
                }
                if t.deadline_ms > 0 {
                    old.deadline_ms = t.deadline_ms;
                }
                let r = old.clone();
                self.save(&f);
                return r;
            }
        }
        f.next += 1;
        t.id = format!("p{}", f.next);
        t.state = State::Running;
        t.boot = boot_token();
        if t.started_ms == 0 {
            t.started_ms = now_ms();
        }
        let cap = f.settings.max_wait_secs.saturating_mul(1000);
        let latest = t.started_ms + cap;
        t.deadline_ms = if t.deadline_ms == 0 { latest } else { t.deadline_ms.min(latest) };
        t.label = crate::bugs::redact(&t.label).chars().take(120).collect();
        t.on_done = crate::bugs::redact(&t.on_done).chars().take(240).collect();
        f.tickets.push(t.clone());
        self.save(&f);
        t
    }

    /// A producer without a store of its own (an install, a pull, a flow, a rollout) says how its work stands. The ticket is
    /// opened on the first report.
    pub fn report(&self, source: Source, key: &str, label: &str, workspace: &str, state: State, detail: &str) {
        let existing = {
            let _g = self.guard();
            self.load().tickets.iter().any(|t| t.source == source && t.key == key && t.running())
        };
        if !existing && state.finished() {
            return;
        }
        if !existing {
            self.register(Ticket { source, key: key.into(), label: label.into(), workspace: workspace.into(), ..Ticket::default() });
        }
        if state.finished() {
            self.finish(source, key, state, detail);
        }
    }

    fn finish(&self, source: Source, key: &str, state: State, detail: &str) {
        let _g = self.guard();
        let mut f = self.load();
        if let Some(t) = f.tickets.iter_mut().find(|t| t.source == source && t.key == key && t.running()) {
            t.state = state;
            t.ended_ms = now_ms();
            t.detail = crate::bugs::redact(detail).chars().take(200).collect();
            self.save(&f);
        }
    }

    pub fn get(&self, id: &str) -> Option<Ticket> {
        let _g = self.guard();
        self.load().tickets.into_iter().find(|t| t.id == id)
    }

    /// The tickets of one workspace (empty name: those of no workspace), newest last.
    pub fn list(&self, workspace: &str) -> Vec<Ticket> {
        let _g = self.guard();
        self.load().tickets.into_iter().filter(|t| t.workspace == workspace).collect()
    }

    /// Labels of the running tickets, for the "Waiting for" line and for Autopilot.
    pub fn running_labels(&self, workspace: &str, skip_jobs: bool) -> Vec<String> {
        self.list(workspace).iter().filter(|t| t.running() && !(skip_jobs && t.source == Source::Job)).map(Ticket::short).collect()
    }

    /// The person or the assistant stops waiting. The producer is not touched here (the caller stops a job).
    pub fn cancel(&self, id: &str) -> Result<Ticket, String> {
        let _g = self.guard();
        let mut f = self.load();
        let t = f.tickets.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no ticket {id}"))?;
        if !t.running() {
            return Err(format!("ticket {id} is not running ({})", t.state.label()));
        }
        t.state = State::Stopped;
        t.ended_ms = now_ms();
        t.told = true;
        t.detail = "cancelled".into();
        let r = t.clone();
        self.save(&f);
        Ok(r)
    }

    /// Attach an acceptance command to a running ticket.
    pub fn set_check(&self, id: &str, command: &str, cwd: &str) -> Result<Ticket, String> {
        let _g = self.guard();
        let mut f = self.load();
        let t = f.tickets.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no ticket {id}"))?;
        if !t.running() {
            return Err(format!("ticket {id} already ended ({})", t.state.label()));
        }
        t.check = command.trim().to_string();
        t.check_cwd = cwd.to_string();
        let r = t.clone();
        self.save(&f);
        Ok(r)
    }

    /// Bring the ledger up to date with the producers and the clock: open tickets for running jobs and tasks, follow their
    /// states, test probes, run acceptance checks of what just ended, time out what passed its deadline.
    pub fn sync(&self, feed: &Feed) {
        let now = now_ms();
        let ws = feed.workspace.as_str();
        let jobs = feed.jobs.as_ref().map(|j| j.visible(&feed.user, ws)).unwrap_or_default();
        let tasks = feed.tasks.as_ref().map(|f| f()).unwrap_or_default();
        let told: Vec<(String, bool)> = match &feed.jobs {
            Some(j) => jobs.iter().filter(|x| x.state.finished()).map(|x| (x.id.clone(), j.agent_told(&feed.user, &x.id))).collect(),
            None => vec![],
        };
        // phase 1: states, under the lock
        let mut to_check: Vec<(String, String, String)> = Vec::new();
        {
            let _g = self.guard();
            let mut f = self.load();
            // a job still running, or one that ended in the last day without the assistant being told (it ended between two looks,
            // or while the chat was closed), gets a ticket; the state pass below moves it to its end
            let recent = now.saturating_sub(24 * 3600 * 1000);
            for j in jobs.iter().filter(|j| j.running() || (j.ended_ms > recent && !told.iter().any(|(id, was)| *id == j.id && *was))) {
                let known = if j.running() { f.tickets.iter().any(|t| t.source == Source::Job && t.key == j.id && t.running()) } else { f.tickets.iter().any(|t| t.source == Source::Job && t.key == j.id) };
                if !known {
                    f.next += 1;
                    f.tickets.push(Ticket { id: format!("p{}", f.next), source: Source::Job, key: j.id.clone(), label: j.label.clone(), workspace: ws.into(), started_ms: j.started_ms, deadline_ms: now + f.settings.max_wait_secs * 1000, check_cwd: j.cwd.clone(), boot: boot_token(), ..Ticket::default() });
                }
            }
            for t in tasks.iter().filter(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running | TaskStatus::NeedsYou | TaskStatus::Paused)) {
                if !f.tickets.iter().any(|x| x.source == Source::Task && x.key == t.id && x.running()) {
                    f.next += 1;
                    let label = if t.title.is_empty() { t.goal.chars().take(60).collect() } else { t.title.clone() };
                    f.tickets.push(Ticket { id: format!("p{}", f.next), source: Source::Task, key: t.id.clone(), label, workspace: ws.into(), started_ms: now, deadline_ms: now + f.settings.max_wait_secs * 1000, boot: boot_token(), ..Ticket::default() });
                }
            }
            for t in f.tickets.iter_mut().filter(|t| t.workspace == ws) {
                match t.source {
                    Source::Job => {
                        if let Some(j) = jobs.iter().find(|j| j.id == t.key) {
                            if t.running() && j.state.finished() {
                                t.state = match j.state {
                                    JobState::Succeeded => State::FinishedOk,
                                    JobState::Failed(_) => State::FinishedFailed,
                                    JobState::Stopped => State::Stopped,
                                    _ => State::Lost,
                                };
                                t.ended_ms = if j.ended_ms > 0 { j.ended_ms } else { now };
                                t.detail = match j.state {
                                    JobState::Failed(c) => format!("exit code {c}; {}", j.last_line),
                                    JobState::Succeeded => if t.check.is_empty() { format!("exited 0, not verified; {}", j.last_line) } else { j.last_line.clone() },
                                    _ => j.last_line.clone(),
                                };
                                if t.state == State::FinishedOk && !t.check.is_empty() && !t.checked {
                                    to_check.push((t.id.clone(), t.check.clone(), t.check_cwd.clone()));
                                }
                            }
                            // the assistant already has a plain job's result (a note, a wait, a stop): nothing to wake it for
                            if t.state.finished() && !t.told && t.check.is_empty() && told.iter().any(|(id, was)| *id == j.id && *was) {
                                t.told = true;
                            }
                        } else if t.running() && !jobs.iter().any(|j| j.id == t.key) && feed.jobs.is_some() {
                            // dismissed or removed while running: nothing more to follow
                            t.state = State::Lost;
                            t.ended_ms = now;
                            t.detail = "the job is gone from the list".into();
                        }
                    }
                    Source::Task => {
                        if let Some(k) = tasks.iter().find(|k| k.id == t.key) {
                            let st = match k.status {
                                TaskStatus::Done | TaskStatus::Merged => Some(State::FinishedOk),
                                TaskStatus::Failed => Some(State::FinishedFailed),
                                TaskStatus::Discarded => Some(State::Stopped),
                                TaskStatus::Interrupted => Some(State::Lost),
                                _ => None,
                            };
                            if let (true, Some(s)) = (t.running(), st) {
                                t.state = s;
                                t.ended_ms = now;
                                let line = if !k.error.is_empty() { &k.error } else if !k.summary.is_empty() { &k.summary } else { &k.last };
                                t.detail = crate::bugs::redact(line).chars().take(200).collect();
                            }
                        }
                    }
                    Source::Probe => {
                        if t.running() {
                            if let Some(p) = &t.probe {
                                if p.holds() {
                                    t.state = State::FinishedOk;
                                    t.ended_ms = now;
                                    t.detail = p.describe();
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if t.running() && t.deadline_ms > 0 && now >= t.deadline_ms {
                    t.state = State::TimedOut;
                    t.ended_ms = now;
                    t.detail = "the time allowed for this wait ran out".into();
                }
            }
            self.save(&f);
        }
        // phase 2: acceptance checks, outside the lock (a command may take a while)
        let results: Vec<(String, bool, String)> = to_check.iter().map(|(id, cmd, cwd)| {
            let (ok, line) = run_check(cmd, cwd);
            (id.clone(), ok, line)
        }).collect();
        if !results.is_empty() {
            let _g = self.guard();
            let mut f = self.load();
            for (id, ok, line) in results {
                if let Some(t) = f.tickets.iter_mut().find(|t| t.id == id) {
                    t.checked = true;
                    t.check_ok = ok;
                    t.check_line = line;
                    if !ok {
                        t.state = State::FinishedFailed;
                        t.detail = format!("finished but the check failed: {}", t.check_line);
                    }
                }
            }
            self.save(&f);
        }
    }

    /// Notes for a run that is starting a model turn: tickets that ended and the assistant was not told about, each once. A
    /// plain job is left to the job tools (they note it); everything else, and any ticket with a check, is noted here.
    pub fn take_notes(&self, workspace: &str) -> Vec<String> {
        let _g = self.guard();
        let mut f = self.load();
        let mut out = Vec::new();
        for t in f.tickets.iter_mut().filter(|t| t.workspace == workspace && t.to_tell()) {
            if t.source == Source::Job && t.check.is_empty() {
                continue;
            }
            t.told = true;
            out.push(note_text(t, false));
        }
        if !out.is_empty() {
            self.save(&f);
        }
        out
    }

    /// Whether anything ended that the assistant has not been told about (the waker's cheap test).
    pub fn has_untold(&self, workspace: &str) -> bool {
        let _g = self.guard();
        self.load().tickets.iter().any(|t| t.workspace == workspace && t.to_tell())
    }

    /// The waker's decision for an idle session: one batched turn for everything that ended and is not yet told, once per
    /// ticket, within the daily chain limit. `opened_ms` is when this session was opened (anything that ended earlier happened
    /// "while you were away"). Auto-continue off: nothing is woken, the notes wait for the person's next message.
    pub fn take_wake(&self, workspace: &str, opened_ms: u64, jobs: Option<(&Jobs, &str)>) -> Option<Wake> {
        let _g = self.guard();
        let mut f = self.load();
        if !f.settings.auto_continue {
            return None;
        }
        let ids: Vec<usize> = f.tickets.iter().enumerate().filter(|(_, t)| t.workspace == workspace && t.to_tell()).map(|(i, _)| i).collect();
        if ids.is_empty() {
            return None;
        }
        let day = now_ms() / 86_400_000;
        if f.wakes_day != day {
            f.wakes_day = day;
            f.wakes_today = 0;
        }
        if f.wakes_today >= f.settings.max_wakes_per_day {
            // said once: the tickets count as told so the message does not repeat every poll
            for &i in &ids {
                f.tickets[i].told = true;
            }
            let n = f.settings.max_wakes_per_day;
            self.save(&f);
            return Some(Wake::Limit { text: format!("Work ended, but I have already continued by myself {n} times today (the limit in the assistant settings), so I stopped. Ask me to look at the results when you want.") });
        }
        f.wakes_today += 1;
        let mut parts = Vec::new();
        let mut done = Vec::new();
        for &i in &ids {
            let away = f.tickets[i].ended_ms > 0 && f.tickets[i].ended_ms < opened_ms;
            f.tickets[i].told = true;
            parts.push(note_text(&f.tickets[i], away));
            done.push(f.tickets[i].id.clone());
            if f.tickets[i].source == Source::Job {
                if let Some((j, user)) = jobs {
                    // the job tools must not note the same ending again in the woken turn
                    j.mark_announced(user, &f.tickets[i].key, crate::jobs::Channel::Agent);
                }
            }
        }
        self.save(&f);
        let head = if parts.len() == 1 { "[IYAKKI] Work you were waiting for has ended.".to_string() } else { format!("[IYAKKI] {} pieces of work you were waiting for have ended.", parts.len()) };
        Some(Wake::Turn { text: format!("{head}\n\n{}\n\nContinue from here: report the outcome to the person in plain words, and do what you said you would. If something still needs time, register it with wait_for or job_start instead of promising to check back.", parts.join("\n\n")), ids: done })
    }

    /// Wakes still allowed today.
    pub fn wakes_left(&self) -> u32 {
        let _g = self.guard();
        let f = self.load();
        if f.wakes_day != now_ms() / 86_400_000 { f.settings.max_wakes_per_day } else { f.settings.max_wakes_per_day.saturating_sub(f.wakes_today) }
    }
}

fn shared_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<Vec<(PathBuf, Arc<Mutex<()>>)>>> = OnceLock::new();
    let m = LOCKS.get_or_init(|| Mutex::new(Vec::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, l)) = g.iter().find(|(p, _)| p == root) {
        return l.clone();
    }
    let l = Arc::new(Mutex::new(()));
    g.push((root.to_path_buf(), l.clone()));
    l
}

/// The text the assistant is given about an ended ticket.
pub fn note_text(t: &Ticket, away: bool) -> String {
    let took = fmt_duration(t.ended_ms.saturating_sub(t.started_ms) / 1000);
    let mut s = format!("{} {} ({}) {} after {took}.", if away { "WHILE YOU WERE AWAY:" } else { "PENDING WORK ENDED:" }, t.label, t.short_ref(), t.state.label());
    if !t.detail.is_empty() {
        s.push_str(&format!(" {}", t.detail));
    }
    if t.checked {
        s.push_str(&format!(" Acceptance check: {} ({}).", if t.check_ok { "passed" } else { "FAILED" }, t.check_line));
    } else if t.state == State::FinishedOk && t.source != Source::Probe && t.check.is_empty() && !t.detail.contains("not verified") {
        s.push_str(" Not verified: no check was set.");
    }
    if !t.on_done.is_empty() {
        s.push_str(&format!(" You said you would: {}", t.on_done));
    }
    s
}

impl Ticket {
    fn short_ref(&self) -> String {
        if self.key.is_empty() { self.id.clone() } else { format!("{} {}", self.source.label(), self.key) }
    }
}

/// Run an acceptance command in `cwd`: exit 0 passes. One redacted line of its output comes back.
pub fn run_check(command: &str, cwd: &str) -> (bool, String) {
    let log = std::env::temp_dir().join(format!("iyakki-check-{}-{}.log", std::process::id(), now_ms()));
    let out = match fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return (false, format!("could not run the check: {e}")),
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return (false, format!("could not run the check: {e}")),
    };
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    if Path::new(cwd).is_dir() {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_file(&log);
            return (false, format!("could not run the check: {e}"));
        }
    };
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t0.elapsed() >= CHECK_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    let mut raw = Vec::new();
    if let Ok(mut f) = fs::File::open(&log) {
        let _ = f.by_ref().take(16_384).read_to_end(&mut raw);
    }
    let _ = fs::remove_file(&log);
    let text = crate::jobs::clean_output(&raw);
    let last = crate::bugs::redact(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")).chars().take(160).collect::<String>();
    match status {
        Some(s) if s.success() => (true, if last.is_empty() { "exit 0".into() } else { last }),
        Some(s) => (false, format!("exit {}{}", s.code().unwrap_or(-1), if last.is_empty() { String::new() } else { format!(": {last}") })),
        None => (false, "the check did not finish in time".into()),
    }
}

// ------------------------------------------------------------------------------------------------ the waker

/// The waker thread of a chat or workspace session: keeps the ledger current and raises a flag when something ended that the
/// assistant has not been told about. The chat, when its run is idle, asks [`Ledger::take_wake`] for the turn to start. It is
/// a thread of the session (not of the model) and stops when dropped.
pub struct Waker {
    pub ledger: Ledger,
    pub feed: Feed,
    pub opened_ms: u64,
    ready: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Waker {
    pub fn start(ledger: Ledger, feed: Feed) -> Waker {
        Self::start_every(ledger, feed, WAKE_POLL)
    }

    pub fn start_every(ledger: Ledger, feed: Feed, every: Duration) -> Waker {
        let (ready, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (l2, f2, r2, s2) = (ledger.clone(), feed.clone(), ready.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                l2.sync(&f2);
                r2.store(l2.has_untold(&f2.workspace), Ordering::SeqCst);
                let t0 = Instant::now();
                while t0.elapsed() < every && !s2.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        Waker { ledger, feed, opened_ms: now_ms(), ready, stop }
    }

    /// Something ended and the assistant has not been told (set by the thread; cheap to read every frame).
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// The turn to start now (call only when no run is in progress).
    pub fn take(&self) -> Option<Wake> {
        let w = self.ledger.take_wake(&self.feed.workspace, self.opened_ms, self.feed.jobs.as_ref().map(|j| (j, self.feed.user.as_str())));
        self.ready.store(self.ledger.has_untold(&self.feed.workspace), Ordering::SeqCst);
        w
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------------------------------------ the gate's phrase test

/// Does the answer promise to wait, check back or report later? A sentence that asks, or that says it cannot, is not a promise.
pub fn promises_to_report(text: &str) -> bool {
    const PROMISES: &[&str] = &[
        "report back", "let you know", "get back to you", "check back", "check in again", "keep you posted", "keep you updated", "notify you", "update you when", "update you once", "tell you when", "tell you once", "follow up", "come back to you", "come back with", "i'll wait", "i will wait", "i am waiting", "i'm waiting", "i'll monitor", "i will monitor", "i'll keep an eye", "i will keep an eye", "i'll watch", "i will watch", "once it finishes", "when it finishes", "when it completes", "once it completes", "when it is done", "when it's done", "once it's done", "once it is done",
    ];
    const DENIALS: &[&str] = &["can't", "cannot", "can not", "won't", "unable", "not able", "no way to", "don't have a way", "do not have a way", "if you want", "if you'd like", "if you like", "should i", "shall i", "would you like", "do you want", "you can ask"];
    let l = text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    l.split(['.', '!', '\n', ';']).any(|s| {
        let s = s.trim();
        !s.is_empty() && !s.ends_with('?') && PROMISES.iter().any(|p| s.contains(p)) && !DENIALS.iter().any(|d| s.contains(d))
    })
}

/// The request-only note when the answer promises to report back and nothing is registered.
pub const NUDGE_NOTHING_PENDING: &str = "SYSTEM NOTE: you said you would wait or report back, but nothing is pending, so nothing will wake you. Register what you are waiting for (`job_start` for a command, `wait_for` for a file, a port or an existing job), or finish now and say plainly what you did not verify. Do not promise to check back.";

/// Added to an answer that still promises after the nudge, so the person is not left waiting for a report that cannot come.
pub const NO_REPORT_NOTICE: &str = "\n\nNote from IYAKKI: nothing is registered to wait for, so no report will follow by itself. Ask me when you want an update.";

/// The line shown in the chat while a run ends with work pending.
pub fn waiting_note(labels: &[String]) -> String {
    let shown: Vec<&str> = labels.iter().take(5).map(String::as_str).collect();
    format!("Waiting for: {}{}. I will continue when it ends.", shown.join(", "), if labels.len() > 5 { " and more" } else { "" })
}

impl Ledger {
    /// For a long-poll (`wait_for` with `block_seconds`, the external agents' way to wait): everything that landed and the
    /// caller was not told about, each once, without the wake counter (the caller is already inside a turn).
    pub fn take_landed(&self, workspace: &str, jobs: Option<(&Jobs, &str)>) -> Vec<String> {
        let _g = self.guard();
        let mut f = self.load();
        let mut out = Vec::new();
        for t in f.tickets.iter_mut().filter(|t| t.workspace == workspace && t.to_tell()) {
            t.told = true;
            out.push(note_text(t, false));
            if let (Source::Job, Some((j, user))) = (t.source, jobs) {
                j.mark_announced(user, &t.key, crate::jobs::Channel::Agent);
            }
        }
        if !out.is_empty() {
            self.save(&f);
        }
        out
    }
}
