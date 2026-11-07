//! Parallel tasks of the AI IDE: isolated pieces of work that run at the same time, each in its own copy of the workspace, and
//! are reviewed and merged one at a time. See `docs/PARALLEL-TASKS.md`.
//!
//! * [`isolate`] gives a task its own tree: a `git worktree` on a new branch for a git workspace, else a copy of the files
//!   with a private throwaway git repository inside it (the diff and merge engine). The user's branches are never committed to.
//! * [`Manager`] is the scheduler: a queue, at most N running (hard cap [`HARD_CAP`]), one worker thread per task, status and
//!   usage from the run's events, budgets, approvals of every task in one list, persistence in
//!   `.iyakki/tasks/<id>/task.json`, restart recovery (`interrupted`).
//! * What a run does is injected ([`Runner`]): the shell supplies the Autopilot machinery (own tools rooted at the tree, own
//!   board, own session); tests supply scripted fakes.
//! * Review and merge: [`Manager::diff`], [`Manager::merge_check`] (a dry run), [`Manager::merge`] (one at a time),
//!   [`Manager::rebase`], [`Manager::discard`], [`Manager::keep`].
//! * The main board has one writer ([`with_board`], under one lock); a row assigned to an unfinished task cannot be given to
//!   another.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::agent::{RunEvent, RunHandle, Verdict};
use crate::featureboard::{BoardFile, NewFeature, Update};

/// Never more than this many tasks run at once, whatever the setting says.
pub const HARD_CAP: usize = 6;
pub const DEFAULT_CONCURRENCY: usize = 3;
/// Unfinished tasks (queued, running, waiting, paused, done but not merged or discarded) per workspace.
pub const MAX_OPEN: usize = 12;
/// Tasks one `task_spawn` call may make.
pub const MAX_PER_CALL: usize = 6;
/// Board rows one task may take.
pub const MAX_ROWS: usize = 12;
/// A copy-mode tree refuses a workspace bigger than this.
pub const COPY_MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const COPY_MAX_FILES: usize = 20_000;
const LOG_CAP: usize = 400;

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
}

impl Default for Task {
    fn default() -> Task {
        Task { id: String::new(), title: String::new(), goal: String::new(), rows: vec![], agent: "builtin".into(), model: String::new(), status: Status::Queued, mode: Mode::Git, branch: String::new(), base: String::new(), tree: PathBuf::new(), session: String::new(), namespace: String::new(), port: 0, created: 0, started: 0, ended: 0, tokens: 0, cost_cents: 0.0, progress_done: 0, progress_total: 0, last: String::new(), summary: String::new(), error: String::new(), note: String::new(), budgets: Budgets::default(), verified: false, kept: false, tests: String::new(), row_prev: BTreeMap::new(), resume: false, interjection: String::new(), override_deps: false, run_id: String::new() }
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

pub type Runner = Arc<dyn Fn(RunSpec) -> RunHandle + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub task: String,
    pub call_id: String,
    pub name: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Clone, Default)]
pub struct Hooks {
    /// `task.spawn|start|stop|merge|discard`, detail.
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    /// (title, text): an inbox item when a task finishes or needs the person.
    pub notify: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    /// A task is over (merged or discarded): tear its namespace down.
    pub cleanup: Option<Arc<dyn Fn(&Task) + Send + Sync>>,
    /// The run of a task, to be kept as a run log (called when it starts, every few dozen events and when it ends).
    pub record: Option<Arc<dyn Fn(RunRecord) + Send + Sync>>,
}

/// What a task's run looked like so far, for the run log.
#[derive(Clone, Debug)]
pub struct RunRecord {
    pub run_id: String,
    pub task: Task,
    /// `Agent 2`
    pub agent: String,
    pub events: Vec<RunEvent>,
    /// `running`, `completed`, `failed` or `stopped`.
    pub status: String,
    pub started: i64,
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

// ------------------------------------------------------------------------------------------------ git

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "user.name=IYAKKI", "-c", "user.email=iyakki@localhost", "-c", "commit.gpgsign=false", "-c", "core.quotepath=off"]);
    c
}

/// (success, stdout, stderr)
fn git_run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    match git_cmd(dir).args(args).output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (false, String::new(), format!("git could not be run: {e}")),
    }
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String, String> {
    let (ok, out, err) = git_run(dir, args);
    if ok {
        Ok(out)
    } else {
        Err(format!("git {}: {}", args.first().unwrap_or(&""), if err.trim().is_empty() { out.trim() } else { err.trim() }))
    }
}

pub fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Is the workspace folder itself the top of a git repository with a commit?
pub fn is_git_root(root: &Path) -> bool {
    let (ok, out, _) = git_run(root, &["rev-parse", "--show-toplevel"]);
    if !ok {
        return false;
    }
    let (Ok(top), Ok(me)) = (std::fs::canonicalize(out.trim()), std::fs::canonicalize(root)) else { return false };
    top == me && git_run(root, &["rev-parse", "--verify", "-q", "HEAD"]).0
}

/// Stage everything except `.iyakki` (a task's own bookkeeping may sit in the tree). An exclude pathspec makes git fail when
/// `.iyakki` is also in `.gitignore`, so the folder is unstaged afterwards instead.
fn stage_all(tree: &Path) -> Result<(), String> {
    git_ok(tree, &["add", "-A", "--", "."])?;
    let _ = git_run(tree, &["reset", "-q", "--", ".iyakki"]);
    Ok(())
}

fn status_lines(dir: &Path) -> Vec<String> {
    git_run(dir, &["status", "--porcelain", "-uno"]).1.lines().filter(|l| !l.get(3..).unwrap_or("").starts_with(".iyakki/")).map(str::to_string).collect()
}

fn slug_of(title: &str) -> String {
    let s: String = crate::workspace::slug(title).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "task".into() } else { s }
}

fn valid_id(id: &str) -> bool {
    id.len() >= 3 && id.len() <= 12 && id.starts_with('t') && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn new_id(root: &Path) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let h = (t ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ std::process::id() as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
        let id = format!("t{:05x}", (h >> 20) & 0xf_ffff);
        if !Task::dir_of(root, &id).exists() {
            return id;
        }
    }
}

/// What [`isolate`] made.
#[derive(Clone, Debug)]
pub struct Made {
    pub mode: Mode,
    pub branch: String,
    pub base: String,
    pub tree: PathBuf,
    pub note: String,
}

const SKIP_DIRS: [&str; 10] = [".git", ".iyakki", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", "vendor"];

fn copy_tree(src: &Path, dst: &Path, skip: &[String], count: &mut (u64, usize)) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let rd = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() || SKIP_DIRS.contains(&name.as_str()) || skip.contains(&name) {
            continue;
        }
        let (from, to) = (e.path(), dst.join(&name));
        if ft.is_dir() {
            copy_tree(&from, &to, skip, count)?;
        } else if ft.is_file() {
            let len = e.metadata().map_or(0, |m| m.len());
            count.0 += len;
            count.1 += 1;
            if count.0 > COPY_MAX_BYTES || count.1 > COPY_MAX_FILES {
                return Err(format!("The workspace is too big to copy for a task ({} MB or {} files is the limit). Put big folders in .gitignore, or make the workspace a git repository so tasks can use a worktree.", COPY_MAX_BYTES / 1024 / 1024, COPY_MAX_FILES));
            }
            std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn add_exclude(root: &Path, line: &str) {
    let (ok, out, _) = git_run(root, &["rev-parse", "--git-path", "info/exclude"]);
    if !ok {
        return;
    }
    let p = PathBuf::from(out.trim());
    let p = if p.is_absolute() { p } else { root.join(p) };
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    if !text.lines().any(|l| l.trim() == line) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, format!("{text}{}{line}\n", if text.is_empty() || text.ends_with('\n') { "" } else { "\n" }));
    }
}

/// Give a task its own tree under `<root>/.iyakki/tasks/<id>/tree`.
pub fn isolate(root: &Path, id: &str, title: &str) -> Result<Made, String> {
    if !valid_id(id) {
        return Err("invalid task id".into());
    }
    if !git_available() {
        return Err("Tasks need git (it makes the diff and does the merge). Install git, then try again.".into());
    }
    let tree = Task::dir_of(root, id).join("tree");
    std::fs::create_dir_all(Task::dir_of(root, id)).map_err(|e| format!("{}: {e}", Task::dir_of(root, id).display()))?;
    let branch = format!("iyakki/task-{id}-{}", slug_of(title));
    if is_git_root(root) {
        add_exclude(root, ".iyakki/tasks/");
        let base = git_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
        let tree_s = tree.to_string_lossy().into_owned();
        git_ok(root, &["worktree", "add", "-q", "-b", &branch, &tree_s, &base])?;
        let dirty = status_lines(root).len();
        let note = if dirty > 0 { format!("The main workspace has {dirty} uncommitted change(s); the task starts from the last commit and does not have them.") } else { String::new() };
        Ok(Made { mode: Mode::Git, branch, base, tree, note })
    } else {
        let skip = crate::workspace::gitignored_names(root);
        let mut count = (0u64, 0usize);
        if let Err(e) = copy_tree(root, &tree, &skip, &mut count) {
            let _ = std::fs::remove_dir_all(Task::dir_of(root, id));
            return Err(e);
        }
        let r = (|| -> Result<String, String> {
            git_ok(&tree, &["init", "-q", "-b", &branch])?;
            add_exclude(&tree, ".iyakki/");
            git_ok(&tree, &["add", "-A", "--", "."])?;
            git_ok(&tree, &["commit", "-q", "--allow-empty", "-m", "base: the workspace as the task found it"])?;
            Ok(git_ok(&tree, &["rev-parse", "HEAD"])?.trim().to_string())
        })();
        match r {
            Ok(base) => Ok(Made { mode: Mode::Copy, branch, base, tree, note: "A copy of the workspace (not a git repository here): the merge applies a patch. Dependency folders are not copied; the task reinstalls them.".into() }),
            Err(e) => {
                let _ = std::fs::remove_dir_all(Task::dir_of(root, id));
                Err(e)
            }
        }
    }
}

/// Stage everything the task changed (never `.iyakki`) and commit it on the task's own branch. Ok(true): something was committed.
fn commit_tree(tree: &Path, title: &str) -> Result<bool, String> {
    stage_all(tree)?;
    if git_run(tree, &["diff", "--cached", "--quiet"]).0 {
        return Ok(false);
    }
    git_ok(tree, &["commit", "-q", "-m", &format!("IYAKKI task: {}", title.chars().take(100).collect::<String>())])?;
    Ok(true)
}

/// The board as one writer sees it: loaded, changed and saved under one process-wide lock, so two tasks finishing together never
/// lose an update.
pub fn with_board<R>(features: &Path, f: impl FnOnce(&mut BoardFile) -> R) -> Result<R, String> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut bf = BoardFile::load(features)?;
    let r = f(&mut bf);
    bf.save()?;
    Ok(r)
}

// ------------------------------------------------------------------------------------------------ manager

enum Cmd {
    Stop,
    Pause,
    Shutdown,
    Decide(String, Verdict),
}

struct Entry {
    task: Task,
    tx: Option<Sender<Cmd>>,
    join: Option<std::thread::JoinHandle<()>>,
    log: VecDeque<String>,
    approvals: Vec<Approval>,
    /// The live approval mode of the current run (None: not running).
    live: Option<Arc<crate::livemode::LiveMode>>,
}

struct State {
    entries: Vec<Entry>,
    max_concurrency: usize,
    /// What one task is expected to need, in MB (the memory-aware cap divides the free memory by it).
    task_mb: u64,
    per_agent: BTreeMap<String, usize>,
    auto_assign: AutoAssign,
    /// Guidance for a running task, delivered when its pause lands.
    guidance: BTreeMap<String, String>,
}

struct Inner {
    root: PathBuf,
    features: PathBuf,
    state: Mutex<State>,
    runner: Mutex<Option<Runner>>,
    hooks: Mutex<Hooks>,
    merge_lock: Mutex<()>,
    /// One git operation on a task's tree at a time: the review thread of the UI and a merge must not fight over `index.lock`.
    git_lock: Mutex<()>,
    /// Free memory in bytes (tests give a fake; `None` inside: unknown). Default: `/proc/meminfo`.
    mem_source: Mutex<Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>>,
    /// The fan-out plan whose later waves wait for earlier ones to be merged.
    plan: Mutex<Option<plan::PlanRun>>,
    /// (key, text) of the last lead summary.
    summary: Mutex<(String, String)>,
    last_poll: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

fn now() -> i64 {
    crate::clock::now_unix()
}

/// `Claude Code (sonnet)`, `built-in (qwen2.5)`: what the Agents strip shows next to an agent's name.
pub fn engine_of(t: &Task) -> String {
    let name = match t.agent.as_str() {
        "builtin" | "" => "built-in".to_string(),
        "claude-code" => "Claude Code".to_string(),
        "copilot" => "Copilot".to_string(),
        other => other.to_string(),
    };
    if t.model.is_empty() { name } else { format!("{name} ({})", t.model) }
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

impl Manager {
    /// The tasks of a workspace, read from `.iyakki/tasks/*/task.json`. A task that was running when the window closed is
    /// `interrupted`.
    pub fn open(root: &Path, features: &Path) -> Manager {
        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root.join(".iyakki/tasks")) {
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                if !valid_id(&id) {
                    continue;
                }
                let Some(mut t) = std::fs::read_to_string(e.path().join("task.json")).ok().and_then(|s| serde_json::from_str::<Task>(&s).ok()) else { continue };
                if t.id != id {
                    continue;
                }
                if t.status.active() {
                    t.status = Status::Interrupted;
                    t.last = "interrupted: the window closed while it ran; Resume continues from the saved place".into();
                    let _ = write_task(root, &t);
                }
                entries.push(Entry { task: t, tx: None, join: None, log: VecDeque::new(), approvals: vec![], live: None });
            }
        }
        entries.sort_by_key(|e| (e.task.created, e.task.id.clone()));
        let cfg = std::fs::read_to_string(root.join(".iyakki/tasks/config.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        let max = cfg.as_ref().and_then(|v| v.get("max_concurrency").and_then(|n| n.as_u64())).map_or(DEFAULT_CONCURRENCY, |n| (n as usize).clamp(1, HARD_CAP));
        let auto = cfg.as_ref().and_then(|v| v.get("auto_assign").and_then(|n| n.as_str()).map(str::to_string)).unwrap_or_default();
        // agents that claim to be at work were cut off by the restart (the tasks above are already `interrupted`)
        crate::assign::update(root, |d| d.reconcile(&[], now()));
        Manager { inner: Arc::new(Inner { root: root.into(), features: features.into(), state: Mutex::new(State { entries, max_concurrency: max, task_mb: cfg.as_ref().and_then(|v| v.get("task_memory_mb").and_then(|n| n.as_u64())).map_or(plan::DEFAULT_TASK_MB, |n| n.clamp(100, 64_000)), per_agent: BTreeMap::new(), auto_assign: AutoAssign::parse(&auto), guidance: BTreeMap::new() }), runner: Mutex::new(None), hooks: Mutex::new(Hooks::default()), merge_lock: Mutex::new(()), git_lock: Mutex::new(()), mem_source: Mutex::new(None), plan: Mutex::new(plan::PlanRun::load(root)), summary: Mutex::new((String::new(), String::new())), last_poll: Mutex::new(None) }) }
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn set_runner(&self, r: Runner) {
        *self.inner.runner.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        self.pump();
    }

    pub fn set_hooks(&self, h: Hooks) {
        *self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner()) = h;
    }

    /// At most `n` at once (clamped to 1..=[`HARD_CAP`]).
    pub fn set_concurrency(&self, n: usize) {
        let n = n.clamp(1, HARD_CAP);
        self.st().max_concurrency = n;
        // remembered for this workspace
        self.write_config("max_concurrency", serde_json::json!(n));
        self.pump();
    }

    /// One key of `.iyakki/tasks/config.json`, the others kept.
    fn write_config(&self, key: &str, value: serde_json::Value) {
        let dir = self.inner.root.join(".iyakki/tasks");
        if std::fs::create_dir_all(&dir).is_ok() {
            let mut v = std::fs::read_to_string(dir.join("config.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(serde_json::Value::is_object).unwrap_or_else(|| serde_json::json!({}));
            v[key] = value;
            let _ = std::fs::write(dir.join("config.json"), v.to_string());
        }
    }

    /// What happens to rows that become ready (default: ask).
    pub fn auto_assign(&self) -> AutoAssign {
        self.st().auto_assign
    }

    pub fn set_auto_assign(&self, a: AutoAssign) {
        self.st().auto_assign = a;
        self.write_config("auto_assign", serde_json::json!(a.key()));
    }

    fn reg<R>(&self, f: impl FnOnce(&mut crate::assign::Doc) -> R) -> R {
        crate::assign::update(&self.inner.root, f)
    }

    fn board_features(&self) -> Vec<crate::featureboard::Feature> {
        BoardFile::load(&self.inner.features).map(|b| b.board.features()).unwrap_or_default()
    }

    /// The rows a task waits for before it may start: dependencies of its rows (other than its own rows) that are not done. Empty when the
    /// person allowed it to start anyway.
    pub fn waiting_on(&self, id: &str) -> Vec<String> {
        let Some(t) = self.get(id) else { return vec![] };
        self.waiting_on_with(&t, &self.board_features())
    }

    fn waiting_on_with(&self, t: &Task, feats: &[crate::featureboard::Feature]) -> Vec<String> {
        if t.override_deps || t.rows.is_empty() {
            return vec![];
        }
        let g = crate::boarddeps::Graph::build(feats);
        let open = |d: &str| feats.iter().find(|f| f.id == d).is_some_and(|f| !matches!(f.status.as_str(), "done" | "deprecated"));
        let mut out: Vec<String> = Vec::new();
        for r in &t.rows {
            for d in g.deps.get(r).into_iter().flatten() {
                if !t.rows.contains(d) && open(d) && !out.contains(d) {
                    out.push(d.clone());
                }
            }
        }
        out
    }

    /// Rows nobody has and that can start now (planned, every dependency done, no unfinished task takes them): the offer after a row is done.
    pub fn ready_offers(&self) -> Vec<String> {
        let feats = self.board_features();
        let owned: std::collections::BTreeSet<String> = self.st().entries.iter().filter(|e| !e.task.status.finished()).flat_map(|e| e.task.rows.clone()).collect();
        crate::boarddeps::ready_rows(&feats).into_iter().filter(|r| !owned.contains(r) && feats.iter().any(|f| &f.id == r && f.status == "planned")).collect()
    }

    /// After a row is done: with `ask` the person is told which rows are ready, with `on` a task is started for each (as many as there are free
    /// places), with `off` nothing happens. Returns the rows offered or started.
    pub fn offer_ready(&self) -> Vec<String> {
        let mode = self.auto_assign();
        if mode == AutoAssign::Off {
            return vec![];
        }
        let offers = self.ready_offers();
        if offers.is_empty() {
            return offers;
        }
        if mode == AutoAssign::Ask {
            self.notify("Rows are ready", &format!("{} can start now (their dependencies are done). Assign them in the Tasks tab or turn on auto-assign.", offers.join(", ")));
            return offers;
        }
        let free = {
            let s = self.st();
            s.max_concurrency.min(HARD_CAP).saturating_sub(s.entries.iter().filter(|e| matches!(e.task.status, Status::Running | Status::NeedsYou | Status::Queued)).count())
        };
        let feats = self.board_features();
        let mut started = Vec::new();
        for r in offers.into_iter().take(free) {
            let title = feats.iter().find(|f| f.id == r).map(|f| f.title.clone()).unwrap_or_else(|| r.clone());
            if self.spawn(NewTask { title: crate::agent::truncate(&title, 90), goal: title, rows: vec![r.clone()], ..Default::default() }).is_ok() {
                started.push(r);
            }
        }
        if !started.is_empty() {
            self.audit("task.autoassign", &started.join(","));
        }
        started
    }

    /// Send guidance to an agent: a running task is paused at the end of its step and continues with the message; a stopped one is resumed with it.
    pub fn guide(&self, id: &str, message: &str) -> Result<(), String> {
        let msg = message.trim();
        if msg.is_empty() {
            return Err("write the message first".into());
        }
        let t = self.get(id).ok_or("no such task")?;
        let now_t = now();
        self.reg(|d| d.step(id, crate::assign::StepKind::Message, &format!("you: {msg}"), now_t));
        if t.status.active() {
            self.st().guidance.insert(id.to_string(), msg.to_string());
            self.pause(id)
        } else {
            self.resume(id, msg)
        }
    }

    pub fn concurrency(&self) -> usize {
        self.st().max_concurrency
    }

    /// At most `n` running tasks of one agent at the same time (0 removes the limit).
    pub fn set_agent_limit(&self, agent: &str, n: usize) {
        let mut s = self.st();
        if n == 0 {
            s.per_agent.remove(agent);
        } else {
            s.per_agent.insert(agent.into(), n);
        }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hooks(&self) -> Hooks {
        self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn audit(&self, kind: &str, detail: &str) {
        if let Some(a) = self.hooks().audit {
            a(kind, detail);
        }
    }

    fn notify(&self, title: &str, text: &str) {
        if let Some(n) = self.hooks().notify {
            n(title, text);
        }
    }

    // ---- reading

    pub fn list(&self) -> Vec<Task> {
        self.st().entries.iter().map(|e| e.task.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.st().entries.iter().find(|e| e.task.id == id).map(|e| e.task.clone())
    }

    pub fn log(&self, id: &str) -> Vec<String> {
        self.st().entries.iter().find(|e| e.task.id == id).map(|e| e.log.iter().cloned().collect()).unwrap_or_default()
    }

    /// Every task's waiting approvals, oldest first (the one queue).
    pub fn approvals(&self) -> Vec<Approval> {
        self.st().entries.iter().flat_map(|e| e.approvals.clone()).collect()
    }

    /// (running, needing you, queued) for the "N running · M need you" chip.
    pub fn counts(&self) -> (usize, usize, usize) {
        let s = self.st();
        let n = |f: &dyn Fn(Status) -> bool| s.entries.iter().filter(|e| f(e.task.status)).count();
        (n(&|x| x == Status::Running), n(&|x| x == Status::NeedsYou), n(&|x| x == Status::Queued))
    }

    /// Unfinished tasks, for the cap and for "something is running" questions.
    pub fn open_count(&self) -> usize {
        self.st().entries.iter().filter(|e| !e.task.status.finished()).count()
    }

    /// A row's owner: the unfinished task that has it.
    pub fn owner_of(&self, row: &str) -> Option<String> {
        self.st().entries.iter().find(|e| !e.task.status.finished() && e.task.rows.iter().any(|r| r == row)).map(|e| e.task.id.clone())
    }

    // ---- making

    /// Make a task: its tree, its board, its saved record. It starts when a place is free.
    pub fn spawn(&self, n: NewTask) -> Result<String, String> {
        let title = n.title.trim().to_string();
        let goal = n.goal.trim().to_string();
        if title.is_empty() && goal.is_empty() {
            return Err("a task needs a title or a goal".into());
        }
        let title = if title.is_empty() { goal.chars().take(60).collect() } else { title };
        let goal = if goal.is_empty() { title.clone() } else { goal };
        if n.rows.len() > MAX_ROWS {
            return Err(format!("a task takes at most {MAX_ROWS} board rows; split the work into several tasks"));
        }
        {
            let s = self.st();
            if s.entries.iter().filter(|e| !e.task.status.finished()).count() >= MAX_OPEN {
                return Err(format!("there are already {MAX_OPEN} unfinished tasks: merge or discard some first"));
            }
            for r in &n.rows {
                if let Some(e) = s.entries.iter().find(|e| !e.task.status.finished() && e.task.rows.contains(r)) {
                    return Err(format!("row {r} already belongs to task {} ({}); a row can only be in one task at a time", e.task.id, e.task.title));
                }
            }
        }
        // the rows must exist and not be done
        let mut seeds: Vec<NewFeature> = Vec::new();
        if !n.rows.is_empty() {
            let bf = BoardFile::load(&self.inner.features).map_err(|e| format!("the board cannot be read: {e}"))?;
            for r in &n.rows {
                let f = bf.board.get(r).ok_or_else(|| format!("there is no board row {r}"))?;
                if f.status == "done" {
                    return Err(format!("row {r} is already done"));
                }
                seeds.push(NewFeature { id: f.id.clone(), title: f.title.clone(), phase: f.phase.clone(), status: "planned".into(), priority: f.priority.clone(), notes: String::new() });
            }
        }
        let id = new_id(&self.inner.root);
        let made = isolate(&self.inner.root, &id, &title)?;
        let dir = Task::dir_of(&self.inner.root, &id);
        // the task's own board: only its rows
        let board_path = dir.join("FEATURES.md");
        let mut bf = BoardFile::new_empty(&board_path);
        for s in &seeds {
            let _ = bf.board.add(s);
        }
        if let Err(e) = bf.save() {
            let _ = remove_tree(&self.inner.root, &Task { id: id.clone(), ..Default::default() }, &made.tree, &made.branch, made.mode, true);
            return Err(e);
        }
        let ns = {
            let cfg = crate::workspace::ProjectConfig::load(&self.inner.root).unwrap_or_default();
            let base = cfg.namespace(&self.inner.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            format!("{base}-dev-{id}")
        };
        let count = self.st().entries.len();
        let task = Task {
            id: id.clone(),
            title,
            goal,
            rows: n.rows.clone(),
            agent: if n.agent.is_empty() { "builtin".into() } else { n.agent },
            model: n.model,
            status: Status::Queued,
            mode: made.mode,
            branch: made.branch,
            base: made.base,
            tree: made.tree,
            session: format!("task-{id}"),
            namespace: ns,
            port: 20_000 + 10 * (count % 500) as u16,
            created: now(),
            budgets: n.budgets.unwrap_or_default(),
            override_deps: n.override_deps,
            note: made.note,
            last: "waiting for a free place".into(),
            ..Default::default()
        };
        write_task(&self.inner.root, &task)?;
        self.audit("task.spawn", &format!("{id} {} · agent {} · rows {}", task.title, task.agent, task.rows.join(",")));
        self.st().entries.push(Entry { task, tx: None, join: None, log: VecDeque::new(), approvals: vec![], live: None });
        self.pump();
        Ok(id)
    }

    // ---- scheduling

    /// Start queued tasks while places are free.
    pub fn pump(&self) {
        if self.inner.runner.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return;
        }
        self.advance_plan();
        #[allow(unused_assignments)]
        let mut memory_held = false;
        loop {
            let (avail, per_task) = (self.free_memory(), self.st().task_mb);
            // a task whose rows wait for rows that are not done stays queued (it starts when they are done, or when the person says so)
            let feats = self.board_features();
            let waits: Vec<(String, Vec<String>)> = self.list().iter().filter(|t| t.status == Status::Queued).map(|t| (t.id.clone(), self.waiting_on_with(t, &feats))).filter(|w| !w.1.is_empty()).collect();
            for (id, w) in &waits {
                self.with_entry(id, |e| e.task.last = format!("waiting for {} to be done", w.join(", ")));
            }
            let held: std::collections::BTreeSet<String> = waits.into_iter().map(|w| w.0).collect();
            let pick = {
                let s = self.st();
                let running = s.entries.iter().filter(|e| e.task.status.active()).count();
                let cap = plan::memory_cap(s.max_concurrency, running, avail, per_task);
                if running >= cap {
                    memory_held = running < s.max_concurrency.min(HARD_CAP) && s.entries.iter().any(|e| e.task.status == Status::Queued);
                    if memory_held {
                        let msg = format!("waiting for memory ({} free; each task is expected to need {} MB)", plan::gb(avail), per_task);
                        drop(s);
                        self.hold_note(&msg);
                    }
                    None
                } else {
                    s.entries
                        .iter()
                        .filter(|e| e.task.status == Status::Queued && !held.contains(&e.task.id))
                        .find(|e| s.per_agent.get(&e.task.agent).is_none_or(|lim| s.entries.iter().filter(|x| x.task.status.active() && x.task.agent == e.task.agent).count() < *lim))
                        .map(|e| e.task.id.clone())
                }
            };
            let Some(id) = pick else { break };
            self.start(&id);
        }
        self.maybe_summarize();
    }

    fn start(&self, id: &str) {
        let (tx, rx) = channel::<Cmd>();
        let spec = {
            let mut s = self.st();
            let Some(e) = s.entries.iter_mut().find(|e| e.task.id == id) else { return };
            e.task.status = Status::Running;
            if e.task.started == 0 || !e.task.resume {
                e.task.started = now();
            }
            e.task.ended = 0;
            e.task.error.clear();
            e.task.run_id = crate::runs::new_run_id();
            e.task.last = "starting".into();
            e.tx = Some(tx);
            // every start (and every resume) works the plan: Autopilot, until the person switches it
            let live = crate::livemode::LiveMode::new(crate::livemode::Mode::Autopilot);
            e.live = Some(live.clone());
            let t = e.task.clone();
            let dir = Task::dir_of(&self.inner.root, id);
            let prompt = if t.interjection.is_empty() || !t.resume { task_prompt(&t) } else { t.interjection.clone() };
            let agent_name = crate::assign::update(&self.inner.root, |d| {
                let n = d.register(&t.id, "task", &engine_of(&t), &t.rows, &t.id, now());
                d.set_run(&t.id, &t.run_id);
                n
            });
            RunSpec { root: self.inner.root.clone(), tree: t.tree.clone(), features: dir.join("FEATURES.md"), state_path: dir.join("autopilot.json"), dir, resume: t.resume, interjection: (!t.interjection.is_empty()).then(|| t.interjection.clone()), prompt, agent_name, task: t, live: Some(live) }
        };
        self.save(id);
        self.report_start(&spec.task);
        self.audit("task.start", &format!("{id} {}", spec.task.title));
        let me = self.clone();
        let sid = id.to_string();
        let h = std::thread::spawn(move || me.worker(&sid, spec, rx));
        if let Some(e) = self.st().entries.iter_mut().find(|e| e.task.id == id) {
            e.join = Some(h);
        }
    }

    fn save(&self, id: &str) {
        let t = self.get(id);
        if let Some(t) = t {
            let _ = write_task(&self.inner.root, &t);
        }
    }

    fn with_entry<R>(&self, id: &str, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        self.st().entries.iter_mut().find(|e| e.task.id == id).map(f)
    }

    fn say(&self, id: &str, line: String) {
        let line = line.replace('\n', " ");
        self.with_entry(id, |e| {
            e.task.last = line.chars().take(160).collect();
            e.log.push_back(line.chars().take(400).collect());
            while e.log.len() > LOG_CAP {
                e.log.pop_front();
            }
        });
    }

    fn progress(&self, id: &str, spec: &RunSpec) {
        let rows = crate::autopilot::Rows::read(&spec.tree, &spec.features);
        if rows.total > 0 {
            self.with_entry(id, |e| {
                e.task.progress_done = rows.done;
                e.task.progress_total = rows.total;
            });
            self.reg(|d| d.set_progress(id, (rows.done * 100 / rows.total) as u32));
        }
        // the files the agent holds, with the size of the change (from git, so an external agent's own edits count too)
        let stats = self.stats(id);
        if !stats.is_empty() {
            self.reg(|d| {
                for (p, a, dl) in &stats {
                    d.touch_file(id, p, *a, *dl, now());
                }
            });
        }
    }

    fn worker(&self, id: &str, spec: RunSpec, rx: std::sync::mpsc::Receiver<Cmd>) {
        let runner = self.inner.runner.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(runner) = runner else { return self.finish(id, &spec, Outcome::Failed("no runner is set".into())) };
        let handle = runner(spec.clone());
        let started = Instant::now();
        let began = now();
        let mut events: Vec<RunEvent> = Vec::new();
        self.record_run(&spec, &events, "running", began);
        let (mut tokens_base, mut last_tokens, mut cost_base, mut last_cost) = (self.get(id).map_or(0, |t| t.tokens), 0u64, self.get(id).map_or(0.0, |t| t.cost_cents), 0.0f64);
        let (mut stop, mut pause, mut shutdown) = (false, false, false);
        let mut over: Option<String> = None;
        let (mut final_text, mut failed, mut stopped): (Option<String>, Option<String>, Option<String>) = (None, None, None);
        let mut sd_deadline: Option<Instant> = None;
        loop {
            match handle.events.recv_timeout(Duration::from_millis(100)) {
                Ok(ev) => {
                    if !matches!(ev, RunEvent::Delta { .. } | RunEvent::Thinking { .. }) && events.len() < 5000 {
                        events.push(ev.clone());
                        if events.len() % 40 == 0 {
                            self.record_run(&spec, &events, "running", began);
                        }
                    }
                    match &ev {
                        RunEvent::Approval { id: cid, name, summary, detail } => {
                            let a = Approval { task: id.to_string(), call_id: cid.clone(), name: name.clone(), summary: summary.clone(), detail: detail.clone() };
                            let title = self.with_entry(id, |e| {
                                e.approvals.push(a);
                                e.task.status = Status::NeedsYou;
                                e.task.title.clone()
                            });
                            if let Some(t) = title {
                                self.notify(&format!("Task needs you: {t}"), &format!("{name}: {}", crate::agent::truncate(summary, 160)));
                            }
                            self.reg(|d| d.set_state(id, crate::assign::AState::WaitingApproval, &format!("{name} {}", crate::agent::truncate(summary, 100)), now()));
                            self.say(id, format!("needs you: {name} {summary}"));
                        }
                        RunEvent::Decision { id: cid, name, approved, .. } => {
                            self.with_entry(id, |e| {
                                e.approvals.retain(|a| a.call_id != *cid);
                                if e.approvals.is_empty() && e.task.status == Status::NeedsYou {
                                    e.task.status = Status::Running;
                                }
                            });
                            if self.get(id).is_some_and(|t| t.status == Status::Running) {
                                self.reg(|d| d.set_state(id, crate::assign::AState::Working, "", now()));
                            }
                            self.say(id, format!("{name}: {}", if *approved { "approved" } else { "denied" }));
                        }
                        RunEvent::ToolResult { name, ok, text, .. } => self.say(id, format!("{name} {}: {}", if *ok { "ok" } else { "failed" }, crate::agent::truncate(text, 120))),
                        RunEvent::Model { text, calls } => {
                            let t = text.trim();
                            if !t.is_empty() {
                                self.say(id, crate::agent::truncate(t, 160));
                            } else if let Some(c) = calls.first() {
                                self.say(id, format!("calling {}", c.name));
                            }
                        }
                        RunEvent::Note { text } => {
                            self.reg(|d| d.step(id, crate::assign::StepKind::Plan, text, now()));
                            self.say(id, text.clone());
                            self.progress(id, &spec);
                        }
                        RunEvent::Usage { tokens, cost_cents, .. } => {
                            if *tokens < last_tokens {
                                tokens_base += last_tokens;
                            }
                            if *cost_cents < last_cost {
                                cost_base += last_cost;
                            }
                            last_tokens = *tokens;
                            last_cost = *cost_cents;
                            let (t, c) = (tokens_base + last_tokens, cost_base + last_cost);
                            self.with_entry(id, |e| {
                                e.task.tokens = t;
                                e.task.cost_cents = c;
                            });
                            self.reg(|d| d.set_usage(id, t, c));
                        }
                        RunEvent::Final { text } => final_text = Some(text.clone()),
                        RunEvent::Failed { error } => failed = Some(error.clone()),
                        RunEvent::Stopped { reason } => stopped = Some(reason.clone()),
                        _ => {}
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            loop {
                match rx.try_recv() {
                    Ok(Cmd::Stop) => {
                        stop = true;
                        handle.cancel();
                    }
                    Ok(Cmd::Pause) => {
                        pause = true;
                        handle.pause();
                    }
                    Ok(Cmd::Shutdown) => {
                        shutdown = true;
                        sd_deadline = Some(Instant::now() + Duration::from_secs(5));
                        handle.pause();
                    }
                    Ok(Cmd::Decide(cid, v)) => handle.decide(&cid, v),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            if over.is_none() && !stop && !pause && !shutdown {
                if let Some(t) = self.get(id) {
                    let b = &t.budgets;
                    let mins = started.elapsed().as_secs() / 60;
                    let why = if b.max_minutes > 0 && mins >= b.max_minutes {
                        Some(format!("over its time budget ({} min)", b.max_minutes))
                    } else if b.max_tokens > 0 && t.tokens >= b.max_tokens {
                        Some(format!("over its token budget ({})", b.max_tokens))
                    } else if b.max_cost_cents > 0 && t.cost_cents >= b.max_cost_cents as f64 {
                        Some(format!("over its cost budget ({} cents)", b.max_cost_cents))
                    } else {
                        None
                    };
                    if let Some(w) = why {
                        over = Some(w);
                        handle.cancel();
                    }
                }
            }
            if sd_deadline.is_some_and(|d| Instant::now() > d) {
                handle.cancel();
                break;
            }
        }
        self.progress(id, &spec);
        let saved = crate::autopilot::State::load(&spec.state_path);
        let outcome = if shutdown {
            Outcome::Interrupted
        } else if let Some(w) = over {
            Outcome::Failed(w)
        } else if stop {
            Outcome::Failed("stopped by you".into())
        } else if pause {
            Outcome::Paused
        } else if let Some(e) = failed {
            Outcome::Failed(e)
        } else {
            match saved.as_ref().map(|s| s.status.as_str()) {
                Some("paused") => Outcome::Paused,
                Some("stopped") => Outcome::Failed(saved.as_ref().map(|s| s.reason.clone()).filter(|r| !r.is_empty()).or(stopped).unwrap_or_else(|| "stopped".into())),
                Some("finished") => Outcome::Done(final_text.unwrap_or_default()),
                _ => match (final_text, stopped) {
                    (Some(t), _) => Outcome::Done(t),
                    (None, Some(r)) => Outcome::Failed(r),
                    _ => Outcome::Failed("the run ended without a result".into()),
                },
            }
        };
        let status = match &outcome {
            Outcome::Done(_) => "completed",
            Outcome::Failed(_) => "failed",
            Outcome::Paused | Outcome::Interrupted => "stopped",
        };
        self.record_run(&spec, &events, status, began);
        self.finish(id, &spec, outcome);
    }

    fn record_run(&self, spec: &RunSpec, events: &[RunEvent], status: &str, started: i64) {
        let Some(f) = self.hooks().record else { return };
        let task = self.get(&spec.task.id).unwrap_or_else(|| spec.task.clone());
        f(RunRecord { run_id: task.run_id.clone(), task, agent: spec.agent_name.clone(), events: events.to_vec(), status: status.to_string(), started });
    }

    fn finish(&self, id: &str, spec: &RunSpec, outcome: Outcome) {
        let title = self.get(id).map(|t| t.title).unwrap_or_default();
        let rows = crate::autopilot::Rows::read(&spec.tree, &spec.features);
        let verified = rows.total > 0 && rows.done == rows.total && rows.blocked == 0;
        match &outcome {
            Outcome::Done(text) => {
                let committed = {
                    let _g = self.glock();
                    commit_tree(&spec.tree, &title)
                };
                let note = match committed {
                    Ok(_) => String::new(),
                    Err(e) => format!("could not commit the task's files: {e}"),
                };
                self.with_entry(id, |e| {
                    e.task.status = Status::Done;
                    e.task.summary = text.clone();
                    e.task.verified = verified;
                    e.task.ended = now();
                    e.task.error = note;
                    e.task.last = "finished".into();
                    e.task.resume = false;
                    e.task.interjection.clear();
                });
                self.reg(|d| d.set_state(id, crate::assign::AState::Done, "finished; waiting for your review and merge", now()));
                self.report_rows(id, "review", &format!("task {id} finished; merge it to complete the row"));
                self.notify(&format!("Task finished: {title}"), &format!("{}. Review and merge it in the Tasks tab.", crate::agent::truncate(text, 200)));
            }
            Outcome::Failed(e) => {
                self.with_entry(id, |en| {
                    en.task.status = Status::Failed;
                    en.task.error = e.clone();
                    en.task.ended = now();
                    en.task.last = format!("failed: {}", crate::agent::truncate(e, 120));
                    en.task.resume = true;
                });
                self.reg(|d| d.set_state(id, crate::assign::AState::Failed, &crate::agent::truncate(e, 120), now()));
                self.audit("task.stop", &format!("{id} failed: {}", crate::agent::truncate(e, 160)));
                self.notify(&format!("Task failed: {title}"), e);
            }
            Outcome::Paused => {
                self.with_entry(id, |e| {
                    e.task.status = Status::Paused;
                    e.task.last = "paused".into();
                    e.task.resume = true;
                });
                self.reg(|d| d.set_state(id, crate::assign::AState::Paused, "", now()));
            }
            Outcome::Interrupted => {
                self.with_entry(id, |e| {
                    e.task.status = Status::Interrupted;
                    e.task.last = "interrupted: Resume continues from the saved place".into();
                    e.task.resume = true;
                });
                self.reg(|d| d.set_state(id, crate::assign::AState::Interrupted, "Resume continues from the saved place", now()));
            }
        }
        self.with_entry(id, |e| {
            e.approvals.clear();
            e.tx = None;
        });
        self.save(id);
        // guidance sent while it ran: the pause landed, so it continues with the message
        let guide = self.st().guidance.remove(id);
        if let (Some(g), Outcome::Paused) = (guide, &outcome) {
            let _ = self.resume(id, &g);
        }
        self.pump();
    }

    // ---- row reporting (the single writer of the main board)

    fn report_start(&self, t: &Task) {
        if t.rows.is_empty() {
            return;
        }
        let mut prev = BTreeMap::new();
        let res = with_board(&self.inner.features, |bf| {
            for r in &t.rows {
                if let Some(f) = bf.board.get(r) {
                    if f.status != "partial" {
                        prev.insert(r.clone(), f.status.clone());
                    }
                    let u = Update { status: Some("partial".into()), comment: Some(format!("task {} running: {}", t.id, t.title)), at: Some(now()), ..Default::default() };
                    let _ = bf.board.update(r, &u);
                }
            }
        });
        if res.is_ok() {
            self.with_entry(&t.id, |e| {
                for (k, v) in prev {
                    e.task.row_prev.entry(k).or_insert(v);
                }
            });
        }
    }

    fn report_rows(&self, id: &str, status: &str, comment: &str) {
        let Some(t) = self.get(id) else { return };
        if t.rows.is_empty() {
            return;
        }
        let _ = with_board(&self.inner.features, |bf| {
            for r in &t.rows {
                if bf.board.get(r).is_some() {
                    let u = Update { status: Some(status.into()), comment: Some(comment.into()), at: Some(now()), ..Default::default() };
                    let _ = bf.board.update(r, &u);
                }
            }
        });
    }

    fn restore_rows(&self, t: &Task) {
        if t.rows.is_empty() {
            return;
        }
        let _ = with_board(&self.inner.features, |bf| {
            for r in &t.rows {
                let Some(f) = bf.board.get(r) else { continue };
                // only rows still in the state this task put them in
                if matches!(f.status.as_str(), "partial" | "review") {
                    let back = t.row_prev.get(r).cloned().unwrap_or_else(|| "planned".into());
                    let u = Update { status: Some(back), comment: Some(format!("task {} was discarded", t.id)), at: Some(now()), ..Default::default() };
                    let _ = bf.board.update(r, &u);
                }
            }
        });
    }

    // ---- controls

    fn send(&self, id: &str, c: Cmd) -> bool {
        self.st().entries.iter().find(|e| e.task.id == id).and_then(|e| e.tx.as_ref()).is_some_and(|tx| tx.send(c).is_ok())
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        let t = self.get(id).ok_or("no such task")?;
        self.audit("task.stop", &format!("{id} by the person"));
        match t.status {
            Status::Queued => {
                self.with_entry(id, |e| {
                    e.task.status = Status::Failed;
                    e.task.error = "stopped by you".into();
                    e.task.last = "stopped before it started".into();
                });
                self.save(id);
                Ok(())
            }
            s if s.active() => {
                if self.send(id, Cmd::Stop) {
                    Ok(())
                } else {
                    Err("the task is not running any more".into())
                }
            }
            _ => Err(format!("the task is {}", t.status.label())),
        }
    }

    /// The live approval mode of a running task (`None`: it is not running).
    pub fn mode(&self, id: &str) -> Option<crate::livemode::Mode> {
        self.st().entries.iter().find(|e| e.task.id == id && e.task.status.active()).and_then(|e| e.live.as_ref().map(|l| l.get()))
    }

    /// Switch a running task's approval mode, live: Auto-approve safe approves its waiting card (unless it always asks), Ask first
    /// makes its next risky call ask, and leaving Autopilot ends its plan loop after the current step (paused with a summary;
    /// Resume works the plan again). Audited with who switched it.
    pub fn set_mode(&self, id: &str, m: crate::livemode::Mode, by: &str) -> Result<(), String> {
        let (live, run) = self.st().entries.iter().find(|e| e.task.id == id && e.task.status.active()).and_then(|e| e.live.clone().map(|l| (l, e.task.run_id.clone()))).ok_or("the task is not running")?;
        if live.set(m) {
            self.audit("task.mode", &format!("{id}: approvals mode changed to {} during run {run} by {by}", m.label()));
            self.say(id, format!("approvals mode changed to {} during this run", m.label()));
        }
        Ok(())
    }

    /// Autopilot -> Ask first -> Auto-approve safe -> Autopilot, for the task's chip.
    pub fn cycle_mode(&self, id: &str, by: &str) -> Result<crate::livemode::Mode, String> {
        use crate::livemode::Mode;
        let next = match self.mode(id).ok_or("the task is not running")? {
            Mode::Autopilot => Mode::Ask,
            Mode::Ask => Mode::Auto,
            Mode::Auto => Mode::Autopilot,
        };
        self.set_mode(id, next, by)?;
        Ok(next)
    }

    pub fn pause(&self, id: &str) -> Result<(), String> {
        let t = self.get(id).ok_or("no such task")?;
        if !t.status.active() {
            return Err(format!("the task is {}", t.status.label()));
        }
        self.send(id, Cmd::Pause);
        Ok(())
    }

    /// Put a paused, interrupted, failed or finished task back in the queue. With a message the run re-plans for it.
    pub fn resume(&self, id: &str, message: &str) -> Result<(), String> {
        let t = self.get(id).ok_or("no such task")?;
        if !matches!(t.status, Status::Paused | Status::Interrupted | Status::Failed | Status::Done) {
            return Err(format!("the task is {}", t.status.label()));
        }
        if !t.tree.exists() {
            return Err("the task's folder is gone".into());
        }
        let state = crate::autopilot::State::load(&Task::dir_of(&self.inner.root, id).join("autopilot.json")).filter(|s| matches!(s.status.as_str(), "paused" | "stopped" | "running"));
        self.with_entry(id, |e| {
            e.task.status = Status::Queued;
            e.task.resume = state.is_some();
            e.task.interjection = message.trim().to_string();
            e.task.last = "waiting for a free place".into();
            e.task.error.clear();
        });
        self.save(id);
        self.pump();
        Ok(())
    }

    pub fn retry(&self, id: &str) -> Result<(), String> {
        self.resume(id, "")
    }

    /// Answer a waiting approval of that task. Only that task's run receives it.
    pub fn answer(&self, id: &str, call_id: &str, v: Verdict) -> Result<(), String> {
        let known = self.st().entries.iter().find(|e| e.task.id == id).is_some_and(|e| e.approvals.iter().any(|a| a.call_id == call_id));
        if !known {
            return Err("that approval is not waiting in this task".into());
        }
        if self.send(id, Cmd::Decide(call_id.to_string(), v)) {
            Ok(())
        } else {
            Err("the task is not running any more".into())
        }
    }

    /// Stop every running task, saving its place; they show `interrupted` after a restart. Waits a few seconds.
    pub fn shutdown(&self) {
        let ids: Vec<String> = self.st().entries.iter().filter(|e| e.tx.is_some()).map(|e| e.task.id.clone()).collect();
        for id in &ids {
            self.send(id, Cmd::Shutdown);
        }
        let joins: Vec<_> = {
            let mut s = self.st();
            s.entries.iter_mut().filter_map(|e| e.join.take()).collect()
        };
        for j in joins {
            let _ = j.join();
        }
        // queued ones are not started by a closing window
        *self.inner.runner.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    // ---- review

    fn tree_of(&self, id: &str) -> Result<Task, String> {
        let t = self.get(id).ok_or("no such task")?;
        if t.status.finished() || !t.tree.exists() {
            return Err("the task's folder is gone (merged or discarded)".into());
        }
        Ok(t)
    }

    /// Stage the task's files (never `.iyakki`) so the diff sees them; a stopped task's work is committed on its own branch.
    fn snapshot(&self, t: &Task) -> Result<(), String> {
        if t.status.active() {
            stage_all(&t.tree)
        } else {
            commit_tree(&t.tree, &t.title).map(|_| ())
        }
    }

    /// Changed files: (status letter, path), against the base.
    pub fn files(&self, id: &str) -> Result<Vec<(char, String)>, String> {
        let _g = self.glock();
        self.files_u(id)
    }

    fn glock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner.git_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn files_u(&self, id: &str) -> Result<Vec<(char, String)>, String> {
        let t = self.tree_of(id)?;
        self.snapshot(&t)?;
        let args: Vec<&str> = if t.status.active() { vec!["diff", "--cached", "--name-status", &t.base] } else { vec!["diff", "--name-status", &t.base, "HEAD"] };
        let out = git_ok(&t.tree, &args)?;
        Ok(out.lines().filter_map(|l| {
            let mut p = l.split('\t');
            let s = p.next()?.chars().next()?;
            let path = p.last()?.to_string();
            Some((s, path))
        }).filter(|(_, p)| !p.starts_with(".iyakki/")).collect())
    }

    /// Lines added and removed per changed file of a task (path, adds, dels), against its base: the numbers of the agent's drill-down panel.
    /// While the task runs this only reads (`--no-optional-locks`, no staging), so it cannot make a git command of the agent fail on a lock.
    pub fn stats(&self, id: &str) -> Vec<(String, usize, usize)> {
        let Some(t) = self.get(id) else { return vec![] };
        if t.status.finished() || !t.tree.exists() {
            return vec![];
        }
        let parse = |out: &str| -> Vec<(String, usize, usize)> {
            out.lines()
                .filter_map(|l| {
                    let mut p = l.split('\t');
                    let (a, d, path) = (p.next()?, p.next()?, p.next()?);
                    Some((path.to_string(), a.parse().unwrap_or(0), d.parse().unwrap_or(0)))
                })
                .filter(|(p, _, _)| !p.starts_with(".iyakki/"))
                .collect()
        };
        if !t.status.active() {
            let (ok, out, _) = git_run(&t.tree, &["--no-optional-locks", "diff", "--numstat", &t.base, "HEAD"]);
            return if ok { parse(&out) } else { vec![] };
        }
        let (ok, out, _) = git_run(&t.tree, &["--no-optional-locks", "diff", "--numstat", &t.base]);
        let mut v = if ok { parse(&out) } else { vec![] };
        // files git does not know yet: all their lines are new
        let (ok, out, _) = git_run(&t.tree, &["--no-optional-locks", "ls-files", "--others", "--exclude-standard"]);
        if ok {
            for path in out.lines().filter(|p| !p.starts_with(".iyakki/")).take(200) {
                let abs = t.tree.join(path);
                if abs.metadata().is_ok_and(|m| m.is_file() && m.len() < 512 * 1024) {
                    let n = std::fs::read(&abs).map(|b| b.iter().filter(|c| **c == b'\n').count().max(usize::from(!b.is_empty()))).unwrap_or(0);
                    v.push((path.to_string(), n, 0));
                }
            }
        }
        v
    }

    /// The unified diff against the base.
    pub fn diff(&self, id: &str) -> Result<String, String> {
        let _g = self.glock();
        let t = self.tree_of(id)?;
        self.snapshot(&t)?;
        let args: Vec<&str> = if t.status.active() { vec!["diff", "--cached", "--no-color", "--no-ext-diff", &t.base] } else { vec!["diff", "--no-color", "--no-ext-diff", &t.base, "HEAD"] };
        let out = git_ok(&t.tree, &args)?;
        const CAP: usize = 2 * 1024 * 1024;
        Ok(if out.len() > CAP { format!("{}\n… the diff is longer than 2 MB; the rest is not shown\n", &out[..out.floor_char_boundary(CAP)]) } else { out })
    }

    /// The dry run: would the merge go through, and what blocks it.
    pub fn merge_check(&self, id: &str) -> Result<MergeCheck, String> {
        let _g = self.glock();
        self.merge_check_u(id)
    }

    fn merge_check_u(&self, id: &str) -> Result<MergeCheck, String> {
        let t = self.tree_of(id)?;
        if t.status.active() {
            return Err("the task is still running".into());
        }
        if !matches!(t.status, Status::Done | Status::Failed | Status::Paused | Status::Interrupted) {
            return Err(format!("the task is {}", t.status.label()));
        }
        self.snapshot(&t)?;
        let files = self.files_u(id)?;
        let mut mc = MergeCheck { files: files.len(), ..Default::default() };
        if files.is_empty() {
            mc.message = "the task changed nothing, so there is nothing to merge".into();
            return Ok(mc);
        }
        match t.mode {
            Mode::Git => {
                let head = git_ok(&self.inner.root, &["symbolic-ref", "-q", "--short", "HEAD"]).map_err(|_| "the main workspace is on a detached HEAD: check out a branch first".to_string())?;
                let (ok, out, err) = git_run(&self.inner.root, &["merge-tree", "--write-tree", "--name-only", "HEAD", &t.branch]);
                if !ok && out.trim().is_empty() {
                    return Err(format!("the dry run failed: {}", err.trim()));
                }
                if !ok {
                    let mut it = out.lines();
                    it.next();
                    for l in it {
                        if l.trim().is_empty() {
                            break;
                        }
                        mc.conflicts.push(l.trim().to_string());
                    }
                    mc.conflicts.sort();
                    mc.conflicts.dedup();
                }
                let dirty: Vec<String> = status_lines(&self.inner.root).iter().filter_map(|l| l.get(3..)).map(|p| p.rsplit(" -> ").next().unwrap_or(p).trim_matches('"').to_string()).collect();
                mc.blocking = files.iter().map(|f| f.1.clone()).filter(|f| dirty.contains(f)).collect();
                mc.clean = mc.conflicts.is_empty() && mc.blocking.is_empty();
                mc.message = if !mc.conflicts.is_empty() {
                    format!("{} file(s) conflict with {head}: {}", mc.conflicts.len(), mc.conflicts.join(", "))
                } else if !mc.blocking.is_empty() {
                    format!("you have uncommitted changes in {}: commit or stash them first", mc.blocking.join(", "))
                } else {
                    format!("{} file(s) merge cleanly into {head}", files.len())
                };
            }
            Mode::Copy => {
                let patch = self.write_patch(&t)?;
                let (ok, _, err) = apply_patch(&self.inner.root, &patch, true);
                if !ok {
                    mc.conflicts = patch_failures(&err);
                    if mc.conflicts.is_empty() {
                        mc.conflicts.push(err.trim().lines().next().unwrap_or("the patch does not apply").to_string());
                    }
                }
                mc.clean = ok;
                mc.message = if ok { format!("{} file(s) apply cleanly", files.len()) } else { format!("the patch does not apply: {}", mc.conflicts.join(", ")) };
            }
        }
        Ok(mc)
    }

    fn write_patch(&self, t: &Task) -> Result<PathBuf, String> {
        let out = git_ok(&t.tree, &["diff", "--binary", "--no-color", "--no-ext-diff", &t.base, "HEAD"])?;
        let p = Task::dir_of(&self.inner.root, &t.id).join("task.patch");
        std::fs::write(&p, out).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(p)
    }

    /// Apply the task to the main workspace. One merge at a time; the dry run is repeated against the workspace as it is now.
    /// The caller has the person's confirmation (a merge is never automatic).
    pub fn merge(&self, id: &str) -> Result<String, String> {
        let r = self.merge_locked(id);
        if r.is_ok() {
            // rows that were waiting for these may be ready now
            self.pump();
            self.offer_ready();
        }
        r
    }

    fn merge_locked(&self, id: &str) -> Result<String, String> {
        let _g = self.inner.merge_lock.lock().unwrap_or_else(|e| e.into_inner());
        let _h = self.glock();
        let mc = self.merge_check_u(id)?;
        if mc.files == 0 {
            return Err(mc.message);
        }
        if !mc.clean {
            return Err(mc.message);
        }
        let t = self.tree_of(id)?;
        let summary = match t.mode {
            Mode::Git => {
                let msg = format!("Merge task {}: {}", t.id, t.title.chars().take(100).collect::<String>());
                if let Err(e) = git_ok(&self.inner.root, &["merge", "--no-ff", "-m", &msg, &t.branch]) {
                    let _ = git_run(&self.inner.root, &["merge", "--abort"]);
                    return Err(e);
                }
                format!("merged {} file(s) of {} into the main branch", mc.files, t.branch)
            }
            Mode::Copy => {
                let patch = self.write_patch(&t)?;
                let (ok, _, err) = apply_patch(&self.inner.root, &patch, false);
                if !ok {
                    return Err(format!("the patch did not apply: {}", err.trim()));
                }
                format!("applied {} file(s) to the workspace", mc.files)
            }
        };
        let _ = remove_tree(&self.inner.root, &t, &t.tree, &t.branch, t.mode, false);
        self.with_entry(id, |e| {
            e.task.status = Status::Merged;
            e.task.ended = now();
            e.task.last = "merged".into();
        });
        let all = t.verified;
        self.report_rows(id, if all { "done" } else { "review" }, &format!("merged from task {id}{}", if all { " (verified in the task tree)" } else { "; not verified by a check, please review" }));
        self.save(id);
        self.audit("task.merge", &format!("{id} {}: {summary}", t.title));
        if let Some(c) = self.hooks().cleanup {
            c(&t);
        }
        Ok(summary)
    }

    /// Move the task onto the main branch's latest commit (git mode). A rebase that conflicts is aborted and reported.
    pub fn rebase(&self, id: &str) -> Result<String, String> {
        let _g = self.glock();
        let t = self.tree_of(id)?;
        if t.mode != Mode::Git {
            return Err("only a task made in a git workspace can be rebased; discard it and run it again to start from the latest files".into());
        }
        if t.status.active() {
            return Err("the task is still running".into());
        }
        self.snapshot(&t)?;
        let head = git_ok(&self.inner.root, &["rev-parse", "HEAD"])?.trim().to_string();
        let (ok, out, err) = git_run(&t.tree, &["rebase", &head]);
        if !ok {
            let files: Vec<String> = git_run(&t.tree, &["diff", "--name-only", "--diff-filter=U"]).1.lines().map(str::to_string).collect();
            let _ = git_run(&t.tree, &["rebase", "--abort"]);
            return Err(format!("the rebase conflicts in {}; the task is unchanged. {}", if files.is_empty() { "some files".into() } else { files.join(", ") }, err.lines().next().unwrap_or(out.lines().next().unwrap_or(""))));
        }
        self.with_entry(id, |e| e.task.base = head.clone());
        self.save(id);
        Ok("the task now starts from the latest commit".into())
    }

    /// Leave the branch for later: the tree goes, the branch stays (git mode).
    pub fn keep(&self, id: &str) -> Result<String, String> {
        let _g = self.glock();
        let t = self.tree_of(id)?;
        if t.status.active() {
            return Err("the task is still running".into());
        }
        if t.mode != Mode::Git {
            return Err("a copy-mode task has no branch to keep; merge it or save its diff".into());
        }
        self.snapshot(&t)?;
        let tree = t.tree.to_string_lossy().into_owned();
        git_ok(&self.inner.root, &["worktree", "remove", "--force", &tree])?;
        let _ = std::fs::remove_dir_all(&t.tree);
        self.report_rows(id, "review", &format!("task {id} kept on branch {}", t.branch));
        self.with_entry(id, |e| {
            e.task.status = Status::Discarded;
            e.task.kept = true;
            e.task.last = format!("kept on branch {}", t.branch);
        });
        self.save(id);
        Ok(format!("kept: the work stays on the branch {}", t.branch))
    }

    /// Remove the task's tree and branch. The caller has asked the person first.
    pub fn discard(&self, id: &str) -> Result<(), String> {
        let _g = self.glock();
        let t = self.get(id).ok_or("no such task")?;
        if t.status.active() {
            return Err("stop the task first".into());
        }
        if t.status.finished() {
            return Err(format!("the task is already {}", t.status.label()));
        }
        remove_tree(&self.inner.root, &t, &t.tree, &t.branch, t.mode, true)?;
        self.restore_rows(&t);
        self.with_entry(id, |e| {
            e.task.status = Status::Discarded;
            e.task.ended = now();
            e.task.last = "discarded".into();
        });
        self.save(id);
        self.audit("task.discard", &format!("{id} {}", t.title));
        if let Some(c) = self.hooks().cleanup {
            c(&t);
        }
        // a plan that waited for this task stops, and the place it held is free
        self.pump();
        Ok(())
    }

    /// Run the workspace's test command inside the task's tree; the result is kept on the task.
    pub fn run_tests(&self, id: &str, program: &str, args: &[String]) -> Result<String, String> {
        let t = self.tree_of(id)?;
        if t.status.active() {
            return Err("the task is still running".into());
        }
        let out = Command::new(program).args(args).current_dir(&t.tree).output().map_err(|e| format!("{program}: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let tail: String = text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        let res = format!("{} (exit {})\n{tail}", if out.status.success() { "tests passed" } else { "tests failed" }, out.status.code().unwrap_or(-1));
        self.with_entry(id, |e| e.task.tests = res.clone());
        self.save(id);
        Ok(res)
    }
}

enum Outcome {
    Done(String),
    Failed(String),
    Paused,
    Interrupted,
}

fn write_task(root: &Path, t: &Task) -> Result<(), String> {
    let dir = Task::dir_of(root, &t.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(t).map_err(|e| e.to_string())?;
    let tmp = dir.join("task.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join("task.json")).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Remove a task's worktree (or copy) and, when `branch_too`, its branch. Only ever inside `<root>/.iyakki/tasks/<id>/`.
fn remove_tree(root: &Path, t: &Task, tree: &Path, branch: &str, mode: Mode, branch_too: bool) -> Result<(), String> {
    let dir = Task::dir_of(root, &t.id);
    if !valid_id(&t.id) || !tree.starts_with(&dir) {
        return Err("refusing to remove a folder outside the task's own".into());
    }
    if mode == Mode::Git {
        let tree_s = tree.to_string_lossy().into_owned();
        let _ = git_run(root, &["worktree", "remove", "--force", &tree_s]);
        let _ = git_run(root, &["worktree", "prune"]);
        if branch_too && branch.starts_with("iyakki/task-") {
            let _ = git_run(root, &["branch", "-D", branch]);
        } else if branch.starts_with("iyakki/task-") {
            let _ = git_run(root, &["branch", "-d", branch]);
        }
    }
    if tree.exists() {
        std::fs::remove_dir_all(tree).map_err(|e| format!("{}: {e}", tree.display()))?;
    }
    for f in ["FEATURES.md", "task.patch", "autopilot.json"] {
        if branch_too {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
    Ok(())
}

fn apply_patch(root: &Path, patch: &Path, check: bool) -> (bool, String, String) {
    let ceiling = root.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    let mut c = git_cmd(root);
    c.env("GIT_CEILING_DIRECTORIES", ceiling).arg("apply");
    if check {
        c.arg("--check");
    }
    c.arg("--").arg(patch);
    match c.output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

fn patch_failures(err: &str) -> Vec<String> {
    let mut v: Vec<String> = err
        .lines()
        .filter_map(|l| {
            let l = l.strip_prefix("error: ")?;
            if let Some(r) = l.strip_prefix("patch failed: ") {
                return Some(r.rsplit_once(':').map_or(r, |x| x.0).to_string());
            }
            l.strip_suffix(": patch does not apply").or_else(|| l.strip_suffix(": already exists in working directory")).map(str::to_string)
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The first message of a task's run: the goal, its rows, and the rules of working in a task.
pub fn task_prompt(t: &Task) -> String {
    let mut s = String::new();
    if t.rows.is_empty() {
        s.push_str(&format!("TASK: {}\n", t.goal));
    } else {
        s.push_str(&format!("TASK: {}\nWork through the board rows of this task's board (they are the plan): {}.\n", t.goal, t.rows.join(", ")));
    }
    s.push_str(&format!(
        "You work in your own isolated copy of the project (your working folder). Only files in it can be changed; the main workspace and its board are not yours. Do not deploy or touch a cluster: build and test locally. If you start a local server, use port {} (or the next ones). Stop when the goal is done and checked; your work is reviewed and merged by the person.",
        t.port
    ));
    s
}

/// What the standard runner needs from its surroundings (the shell fills it in; tests give a scripted model).
#[derive(Clone)]
pub struct RunnerParts {
    pub backend: Arc<dyn crate::chat::ChatBackend>,
    /// The person's tools for one task: working folder and file scope are the task's tree, nothing else.
    pub tools: Arc<dyn Fn(&RunSpec) -> Arc<dyn crate::agent::Tools> + Send + Sync>,
    pub limits: crate::agent::RunLimits,
    pub assist: crate::assistrun::Assist,
    /// An external coding agent that carries out the steps (its session is the task's own).
    pub executor: Option<Arc<dyn Fn(&RunSpec, Arc<dyn crate::agent::Tools>) -> Option<crate::autopilot::Executor> + Send + Sync>>,
    pub history: Option<Arc<dyn Fn(&RunSpec) -> Option<crate::history::History> + Send + Sync>>,
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

/// The standard way to run a task: the Autopilot machinery with its own tools (rooted at the tree), board, state file, session,
/// budgets and project state.
pub fn autopilot_runner(p: RunnerParts) -> Runner {
    Arc::new(move |spec: RunSpec| {
        let base = (p.tools)(&spec);
        let mut pt = crate::boardtools::ProjectTools::new(base, &spec.tree, &spec.task.title, &spec.features).with_notes(&spec.agent_name, &spec.root).with_agent(&spec.task.id).with_project_test_command(&[&spec.tree, &spec.root]);
        if let Some(a) = &p.audit {
            let a = a.clone();
            pt = pt.with_audit(Arc::new(move |k, d| a(k, d)));
        }
        let tools: Arc<dyn crate::agent::Tools> = Arc::new(pt);
        let mut ctx = crate::autopilot::Context::new(&spec.tree, &spec.features, &spec.prompt);
        ctx.state_path = spec.state_path.clone();
        ctx.notes_root = spec.root.clone();
        ctx.resume = spec.resume;
        ctx.interjection = spec.interjection.clone();
        ctx.session = spec.task.session.clone();
        let b = &spec.task.budgets;
        ctx.cfg = crate::autopilot::Config { max_steps: b.max_steps.max(1), max_seconds: b.max_minutes.max(1) * 60, max_tokens: b.max_tokens, ..Default::default() };
        ctx.audit = p.audit.clone();
        {
            let (root, tree, features, name) = (spec.root.clone(), spec.tree.clone(), spec.features.clone(), spec.task.title.clone());
            ctx.state = Some(Arc::new(move || {
                let cfg = crate::workspace::ProjectConfig::load(&root).unwrap_or_default();
                crate::projectstate::build_with_notes(&crate::projectstate::Input { name: &name, root: &tree, features: &features, stack_override: &cfg.stack, test: &cfg.commands.test, run: &cfg.commands.run, build: &cfg.commands.build, cluster: None, history: None }, &root)
            }));
        }
        ctx.history = p.history.as_ref().and_then(|h| h(&spec));
        if let Some(x) = &p.executor {
            ctx.executor = x(&spec, tools.clone());
        }
        let mut limits = p.limits.clone();
        limits.max_seconds = limits.max_seconds.min(b.max_minutes.max(1) * 60);
        limits.live = spec.live.clone();
        crate::autopilot::start(p.backend.clone(), tools, vec![crate::chat::Message::user(&spec.prompt)], limits, p.assist.clone(), ctx)
    })
}

mod plan;
pub use plan::{memory_cap, split_plan, summary_text, PlanStep, Report, Split, DEFAULT_TASK_MB, RESERVE_MB};
