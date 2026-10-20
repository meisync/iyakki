//! Autopilot: the assistant works toward a goal, step after step, without the person typing "ok" in between.
//!
//! [`start`] returns an ordinary [`RunHandle`] (so the chat, the run log and the approvals inbox work as for any run) whose
//! events are those of a series of ordinary runs, one per *step*:
//!
//! 1. step 1 asks the model to plan the goal on the feature board (`board_add`, one row per step) and start;
//! 2. after each step the harness reads the board (only the rows the assistant added, see [`crate::boardtrack`]) and either
//!    continues with a short internal message naming the next row, retries a row with "a different approach", marks a row
//!    `blocked` after N failed attempts, or runs a final verification and finishes with a summary;
//! 3. every step is bounded: steps, wall time, tokens and consecutive turns without any board change; the run never raises
//!    its own limits;
//! 4. Auto approvals are implied for the safe set, but `hard_ask` calls (delete, vault, outside the workspace, browser) still
//!    ask: the step waits on the approval card ("needs you") and goes on when it is answered. Denying one blocks that row;
//! 5. Pause (typing a message, Pause button), Stop and read-only workspaces end the run; the place is saved in
//!    `.iyakki/autopilot.json` so a restarted window can offer Resume.
//!
//! The step messages are short and carry only the current row and counts, never the whole board, so small local models stay
//! within their window.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{RunEvent, RunHandle, RunLimits, Tools, Verdict};
use crate::assistrun::{Assist, Prepare};
use crate::boardtrack::Tracker;
use crate::chat::{ChatBackend, Message};
use crate::featureboard::BoardFile;

/// What one step asks of whoever carries it out: the step message (goal, PROJECT STATE, current row, last diagnosis), the
/// context rebuild for the system prompt and skills, and the hard caps of the step.
pub struct StepCall {
    pub prompt: String,
    pub prepare: Option<Prepare>,
    pub limits: RunLimits,
    pub step: usize,
}

/// Carries out one step as a run. By default a step is an ordinary run of the chat's model; an external coding agent
/// (`crate::extagent`) supplies its own, resuming its own session, while the harness here keeps the board, the checks, the caps
/// and the summary.
pub type Executor = Arc<dyn Fn(StepCall) -> RunHandle + Send + Sync>;

/// The limits of one autopilot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub max_steps: usize,
    pub max_seconds: u64,
    /// Generated plus NEW prompt tokens (the resent history is not counted again). 0 takes the run limits' token budget, or
    /// 300,000 when that is unlimited too.
    pub max_tokens: u64,
    /// Steps in a row in which the board did not change.
    pub max_no_progress: usize,
    /// Attempts per row before it is marked blocked.
    pub retries: usize,
    /// Start rows although their dependencies (the board's Depends column) are not done. Only the person turns this on.
    pub ignore_deps: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { max_steps: 40, max_seconds: 30 * 60, max_tokens: 0, max_no_progress: 3, retries: 3, ignore_deps: false }
    }
}

/// What the board looks like after a step (only the rows the assistant added).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    /// Open rows in board order: (id, title, status).
    pub open: Vec<(String, String, String)>,
    pub total: usize,
    pub done: usize,
    pub blocked: usize,
    /// Rows set to done without the harness having seen a verified success (set by hand or forced): shown, not counted.
    pub claimed: usize,
    /// Rows the harness parked for the person's OK (work claimed, nothing to check it with): shown, never retried.
    pub review: Vec<(String, String)>,
    /// Bugs (B- rows) that are not done yet: id, title, status. They are fixed along the way, not worked as plan rows.
    pub bugs: Vec<(String, String, String)>,
    /// Open rows that wait for another row: id to what they wait for.
    pub open_deps: BTreeMap<String, String>,
    /// Rows waiting for something: (id, title, what they wait for).
    pub pending: Vec<(String, String, String)>,
    /// Ids of the verified done rows (a pending row whose dependency is among them comes back).
    pub done_ids: Vec<String>,
    /// For open rows whose notes name another row that is not done: id to that row.
    pub deps: BTreeMap<String, String>,
    /// Changes whenever any row's status changes or a row is added.
    pub sig: String,
    /// Open rows whose dependencies are not done, with those dependencies: not started until they are (unless the person said to).
    pub waiting: BTreeMap<String, Vec<String>>,
}

impl Rows {
    /// The next row to work on: a partial one first, then the first planned row whose dependency (a row id) is done or not a row at all.
    pub fn next_row(&self) -> Option<&(String, String, String)> {
        let ready = |r: &&(String, String, String)| self.open_deps.get(&r.0).is_none_or(|d| self.done_ids.iter().any(|x| d.contains(x.as_str())) || !self.open.iter().any(|o| d.contains(o.0.as_str())));
        // a row whose dependencies are not done is never picked: when only such rows are left, there is no next row
        let go = |r: &&(String, String, String)| !self.waiting.contains_key(&r.0);
        self.open.iter().filter(go).find(|r| r.2 == "partial").or_else(|| self.open.iter().filter(go).find(ready)).or_else(|| self.open.iter().find(go))
    }

    /// "row 12 of 63": how far along the plan is (rows settled one way or another, plus the one being worked on).
    pub fn position(&self) -> String {
        format!("row {} of {}", self.done + self.blocked + self.pending.len() + self.review.len() + 1, self.total)
    }

    pub fn read(root: &Path, features: &Path) -> Rows {
        Rows::read_with(root, features, false)
    }

    /// `ignore_deps`: the person allowed rows to start before their dependencies are done.
    pub fn read_with(root: &Path, features: &Path, ignore_deps: bool) -> Rows {
        let tracker = Tracker::new(root, features);
        let (owned, verified) = (tracker.owned_ids(), tracker.verified_ids());
        let Ok(bf) = BoardFile::load(features) else { return Rows::default() };
        let mut r = Rows::default();
        let all = bf.board.features();
        let graph = crate::boarddeps::Graph::build(&all);
        for f in all.iter().cloned().filter(|f| owned.contains(&f.id)) {
            if f.is_bug() {
                if f.status != "done" {
                    r.bugs.push((f.id.clone(), f.title.clone(), f.status.clone()));
                }
                continue;
            }
            r.total += 1;
            r.sig.push_str(&format!("{}:{};", f.id, f.status));
            match f.status.as_str() {
                "done" if verified.contains(&f.id) => {
                    r.done += 1;
                    r.done_ids.push(f.id.clone());
                }
                "pending" => r.pending.push((f.id.clone(), f.title.clone(), f.depends())),
                "review" => r.review.push((f.id.clone(), f.title.clone())),
                "done" => r.claimed += 1,
                "blocked" => r.blocked += 1,
                "planned" | "partial" => {
                    if let Some(d) = crate::boardtrack::dependency_in(&f.notes, &f.id) {
                        r.deps.insert(f.id.clone(), d);
                    }
                    if !f.depends().is_empty() {
                        r.open_deps.insert(f.id.clone(), f.depends());
                    }
                    if !ignore_deps {
                        if let Some(crate::boarddeps::DepState::Waiting(v) | crate::boarddeps::DepState::Blocked(v)) = graph.state(&f.id).filter(|s| !s.is_ready()) {
                            if !v.is_empty() {
                                r.waiting.insert(f.id.clone(), v.clone());
                            }
                        }
                    }
                    r.open.push((f.id.clone(), f.title.clone(), f.status.clone()))
                }
                _ => r.done += 1,
            }
        }
        r
    }
}

/// The running summary never exceeds this many characters (about 750 tokens).
pub const SUMMARY_CAP: usize = 3000;

/// How a step ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepEnd {
    Final,
    Failed(String),
    /// The inner run stopped by itself (its own step limit, a loop).
    Stopped(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Continue { msg: String, note: String },
    /// Mark the row blocked with the reason, then continue with whatever is next.
    Block { id: String, reason: String },
    /// Set the row to pending (it waits for `depends`) and move on.
    Pending { id: String, depends: String },
    /// A pending row comes back: set it to partial and work on it.
    Revisit { id: String, why: String },
    Finish,
    Stop(String),
    Pause(String),
}

/// The saved place of an autopilot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub goal: String,
    pub step: usize,
    pub started: i64,
    pub tokens: u64,
    pub no_progress: usize,
    pub last_sig: String,
    pub current_row: String,
    pub tries: BTreeMap<String, usize>,
    pub verified: bool,
    pub plan_tries: usize,
    /// Waiting for the person's choice of which rows to work through (many rows were found on the board).
    pub awaiting_scope: bool,
    /// Pending rows already brought back once.
    pub revisited: std::collections::BTreeSet<String>,
    /// The running summary pasted into every step (replaced, never appended; at most [`SUMMARY_CAP`] characters).
    pub summary: String,
    /// The last step did something (a file written, a command run, a build, load or apply): a real attempt at fixing the row.
    pub step_changed: bool,
    /// The final check may end with the single optional offer to tear the dev environment down (set by the harness from the cluster's state).
    pub offer_teardown: bool,
    /// Steps in a row on the same row that changed nothing (the model only described).
    pub stalls: usize,
    /// A failed check's diagnosis (row id, text) waiting to be handed to the model in the next step.
    pub diagnosis: Option<(String, String)>,
    /// The step number at the last (re)start: the step budget counts from here.
    pub base_step: usize,
    /// New tokens of the last step, for the footer and the step note.
    pub step_tokens: u64,
    /// running, paused, stopped or finished.
    pub status: String,
    pub reason: String,
    /// The step that just ended waited for a detached job (or one is still running): it was not a stall, so it counts neither
    /// as a step without progress nor as an attempt at the row. Set by the loop around `after_step`, never saved.
    pub waiting: bool,
}

impl State {
    pub fn new(goal: &str, now: i64) -> State {
        State { goal: goal.into(), step: 0, started: now, tokens: 0, no_progress: 0, last_sig: String::new(), current_row: String::new(), tries: BTreeMap::new(), verified: false, plan_tries: 0, awaiting_scope: false, revisited: Default::default(), summary: String::new(), step_changed: true, offer_teardown: false, stalls: 0, diagnosis: None, base_step: 0, step_tokens: 0, status: "running".into(), reason: String::new(), waiting: false }
    }

    pub fn to_json(&self) -> Value {
        json!({"goal": self.goal, "step": self.step, "started": self.started, "tokens": self.tokens, "no_progress": self.no_progress, "last_sig": self.last_sig, "current_row": self.current_row, "tries": self.tries, "verified": self.verified, "plan_tries": self.plan_tries, "awaiting_scope": self.awaiting_scope, "stalls": self.stalls, "revisited": self.revisited, "summary": self.summary, "base_step": self.base_step, "step_tokens": self.step_tokens, "status": self.status, "reason": self.reason})
    }

    pub fn from_json(v: &Value) -> Option<State> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(State {
            goal: s("goal"),
            step: n("step") as usize,
            started: v.get("started").and_then(Value::as_i64).unwrap_or(0),
            tokens: n("tokens"),
            no_progress: n("no_progress") as usize,
            last_sig: s("last_sig"),
            current_row: s("current_row"),
            tries: v.get("tries").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, x)| x.as_u64().map(|x| (k.clone(), x as usize))).collect()).unwrap_or_default(),
            verified: v.get("verified").and_then(Value::as_bool).unwrap_or(false),
            plan_tries: n("plan_tries") as usize,
            step_changed: true,
            offer_teardown: false,
            stalls: n("stalls") as usize,
            diagnosis: None,
            awaiting_scope: v.get("awaiting_scope").and_then(Value::as_bool).unwrap_or(false),
            summary: s("summary"),
            base_step: n("base_step") as usize,
            step_tokens: n("step_tokens"),
            revisited: v.get("revisited").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            status: s("status"),
            reason: s("reason"),
            waiting: false,
        })
        .filter(|st| !st.goal.is_empty())
    }

    pub fn load(path: &Path) -> Option<State> {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| State::from_json(&v))
    }

    pub fn save(&self, path: &Path) {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(path, self.to_json().to_string());
    }

    /// Fresh budgets for a resumed run (the place, the board and the summary are kept).
    pub fn reset_budgets(&mut self, now: i64) {
        self.tokens = 0;
        self.started = now;
        self.base_step = self.step;
        self.no_progress = 0;
    }

    /// Rewrite the running summary from what the harness knows (extractive, no model call): what is done, blocked and pending,
    /// the files and commands so far, the last error and the last answer. Capped at [`SUMMARY_CAP`] characters.
    pub fn update_summary(&mut self, rows: &Rows, titles: &BTreeMap<String, String>, files: &[String], commands: &[String], last_error: &str, last_text: &str) {
        let name = |id: &String| format!("{id} {}", crate::agent::truncate(titles.get(id).map(String::as_str).unwrap_or(""), 40).replace('\n', " "));
        let mut s = format!("Goal: {}. Step {}.", crate::agent::truncate(&self.goal, 160), self.step);
        s.push_str(&format!(" Rows: {} done, {} blocked, {} pending, {} open.", rows.done, rows.blocked, rows.pending.len(), rows.open.len()));
        if !rows.done_ids.is_empty() {
            s.push_str(&format!(" Done: {}.", rows.done_ids.iter().rev().take(8).map(name).collect::<Vec<_>>().join("; ")));
        }
        if !rows.pending.is_empty() {
            s.push_str(&format!(" Pending: {}.", rows.pending.iter().take(5).map(|p| format!("{} waits for {}", p.0, if p.2.is_empty() { "?" } else { &p.2 })).collect::<Vec<_>>().join("; ")));
        }
        if !files.is_empty() {
            s.push_str(&format!(" Files changed: {}.", files.iter().rev().take(12).cloned().collect::<Vec<_>>().join(", ")));
        }
        if !commands.is_empty() {
            s.push_str(&format!(" Commands: {}.", commands.iter().rev().take(8).cloned().collect::<Vec<_>>().join("; ")));
        }
        if !last_error.is_empty() {
            s.push_str(&format!(" Last error: {}.", crate::agent::truncate(last_error, 300).replace('\n', " ")));
        }
        if !last_text.trim().is_empty() {
            s.push_str(&format!(" Last answer: {}", crate::agent::truncate(last_text.trim(), 300).replace('\n', " ")));
        }
        self.summary = s.chars().take(SUMMARY_CAP).collect();
    }

    /// The message of step 1.
    pub fn first_message(&self) -> String {
        format!("AUTOPILOT goal: {}\nPlan it first: add one planned row per step with board_add (rows already on the board stay; `depends` names the rows one waits for), then start the first ready row. Record each lasting decision (technology, structure) in the project context with context_add. List the tests you write for each row (board_update `tests`, and the source files in `files`). Work with tools; do not ask me whether to continue. You can read and write files in the workspace and run tools: never ask me to open, edit, replace, create or save a file or to run or try a command; do it yourself with the tools and re-run the failing step.", self.goal)
    }

    fn row_message(&self, rows: &Rows, cfg: &Config) -> String {
        let (id, title, status) = &rows.next_row().cloned().unwrap_or_default();
        let tries = self.tries.get(id).copied().unwrap_or(0);
        format!(
            "AUTOPILOT step {}: {}/{} rows done{}. Work on {id} \"{}\" now ({status}).{} Use tools; call board_update when it really is done; do not ask me to continue. You can read and write files in the workspace and run tools: never ask me to open, edit, replace, create or save a file or to run or try a command; do it yourself with the tools and re-run the failing step.",
            self.step + 1,
            rows.done,
            rows.total,
            if rows.blocked > 0 { format!(", {} blocked", rows.blocked) } else { String::new() },
            crate::agent::truncate(title, 120),
            if tries > 0 { format!(" Attempt {} of {}: the last try did not finish, use a different approach.", tries + 1, cfg.retries) } else { String::new() }
        )
    }

    fn verify_message(&self) -> String {
        let ending = if self.offer_teardown {
            " If the goal was a deploy to the cluster and it succeeded, end by calling ask_user with the question \"Tear down the dev environment?\" and the options Keep it running, Stop it, Remove all (you never tear down by yourself: Stop uses k8s_stop_env, Remove all uses k8s_teardown, which asks and needs the person to type the namespace name). Ask nothing else."
        } else {
            " This is the last message: a summary only. Do not ask anything, offer options or write \"would you like\"."
        };
        format!("AUTOPILOT final check: verify the result for the goal \"{}\" with a tool (deploy_status, the tests, or ide_browser_check on the app's `reachable at:` address). Then write the summary: what is done, what is blocked and why, what I must do myself (for example commands that need root), files changed, commands run.{ending}", crate::agent::truncate(&self.goal, 200))
    }

    /// Decide what follows a step. `denied`: the person refused an approval during the step.
    pub fn after_step(&mut self, cfg: &Config, end: &StepEnd, rows: &Rows, step_tokens: u64, now: i64, denied: bool) -> Decision {
        self.tokens += step_tokens;
        self.step += 1;
        if let StepEnd::Failed(e) = end {
            return Decision::Pause(format!("the model failed: {e}"));
        }
        self.step_tokens = step_tokens;
        if self.step - self.base_step >= cfg.max_steps {
            return Decision::Stop(format!("step budget used ({} steps)", cfg.max_steps));
        }
        if (now - self.started).max(0) as u64 >= cfg.max_seconds {
            return Decision::Stop(format!("time budget used ({} min)", cfg.max_seconds / 60));
        }
        if cfg.max_tokens > 0 && self.tokens >= cfg.max_tokens {
            return Decision::Stop(format!("token budget used (about {})", self.tokens));
        }
        if rows.sig != self.last_sig {
            self.no_progress = 0;
            self.last_sig = rows.sig.clone();
        } else if !self.waiting {
            self.no_progress += 1;
            if self.no_progress >= cfg.max_no_progress {
                return Decision::Stop(format!("no progress in {} steps in a row", cfg.max_no_progress));
            }
        }
        if self.verified {
            return Decision::Finish;
        }
        self.plan_next(cfg, rows, denied)
    }

    /// The decision once the board is known (also used after a row was blocked).
    pub fn plan_next(&mut self, cfg: &Config, rows: &Rows, denied: bool) -> Decision {
        if rows.total == 0 {
            self.plan_tries += 1;
            // a model that will not use the board: its answer stands
            return if self.plan_tries >= 2 { Decision::Finish } else { Decision::Continue { msg: "AUTOPILOT: there is no plan on the board yet. Add one planned row per step with board_add, then start the first.".into(), note: "autopilot: planning".into() } };
        }
        // a pending row whose dependency is now done comes back first
        if let Some(p) = rows.pending.iter().find(|p| rows.done_ids.iter().any(|d| p.2.contains(d.as_str()))) {
            self.revisited.insert(p.0.clone());
            self.current_row.clear();
            return Decision::Revisit { id: p.0.clone(), why: format!("{} is done", p.2) };
        }
        let Some(cur) = rows.next_row() else {
            // nothing else is left: pending rows get one more try each, then the check and the summary
            if let Some(p) = rows.pending.iter().find(|p| !self.revisited.contains(&p.0)) {
                self.revisited.insert(p.0.clone());
                self.current_row.clear();
                return Decision::Revisit { id: p.0.clone(), why: "nothing else is left to do".into() };
            }
            self.verified = true;
            return Decision::Continue { msg: self.verify_message(), note: format!("autopilot: verifying · {}/{} done{}", rows.done, rows.total, if rows.blocked > 0 { format!(" · {} blocked", rows.blocked) } else { String::new() }) };
        };
        let id = cur.0.clone();
        if denied && self.current_row == id {
            return Decision::Block { id, reason: "the person did not approve a step it needed".into() };
        }
        if self.current_row == id && !self.step_changed {
            // the model only described what to do: not an attempt, a stall (unless it was waiting for a job)
            if !self.waiting {
                self.stalls += 1;
            }
            if self.stalls >= cfg.retries {
                return Decision::Block { id, reason: "the model kept describing fixes instead of making them".into() };
            }
        } else if self.current_row == id {
            self.stalls = 0;
            let t = self.tries.entry(id.clone()).or_insert(0);
            if !self.waiting {
                *t += 1;
            }
            if *t >= cfg.retries {
                // it waits for another row it names, or it is blocked
                return match rows.deps.get(&id) {
                    Some(d) => Decision::Pending { id, depends: d.clone() },
                    None => Decision::Block { id, reason: format!("not finished after {} attempts", cfg.retries) },
                };
            }
        } else {
            self.current_row = id.clone();
        }
        let msg = self.row_message(rows, cfg);
        let note = format!("autopilot: step {} · {} · {} {}", self.step + 1, rows.position(), cur.0, crate::agent::truncate(&cur.1, 60));
        Decision::Continue { msg, note }
    }
}

/// The longest an Autopilot step waits for detached jobs before going on (the job keeps running either way).
pub const JOB_WAIT_CAP: Duration = Duration::from_secs(6 * 3600);

pub struct Waited {
    pub waited: bool,
    pub secs: u64,
}

/// While detached jobs of this chat run, wait for them: the run is not stalled, it is waiting. Returns early when the person
/// stops or pauses the run (`Some("stop")` / `Some("pause")`) and after `cap`. A note says what is awaited about once a minute.
pub fn wait_for_jobs(tools: &dyn Tools, cancel: &AtomicBool, pause: &AtomicBool, emit: &dyn Fn(RunEvent), cap: Duration) -> (Waited, Option<&'static str>) {
    wait_for_jobs_until(tools, cancel, pause, &|| false, emit, cap)
}

/// [`wait_for_jobs`] that also returns (`Some("off")`) as soon as `off` is true: Autopilot was switched off, so the run does not
/// wait; the jobs keep running detached.
pub fn wait_for_jobs_until(tools: &dyn Tools, cancel: &AtomicBool, pause: &AtomicBool, off: &dyn Fn() -> bool, emit: &dyn Fn(RunEvent), cap: Duration) -> (Waited, Option<&'static str>) {
    let t0 = std::time::Instant::now();
    let mut noted: Option<std::time::Instant> = None;
    let mut waited = false;
    loop {
        let running = tools.jobs_running();
        if running.is_empty() || t0.elapsed() >= cap {
            break;
        }
        if off() {
            return (Waited { waited, secs: t0.elapsed().as_secs() }, Some("off"));
        }
        waited = true;
        if noted.is_none_or(|n| n.elapsed() >= Duration::from_secs(60)) {
            emit(RunEvent::Note { text: format!("autopilot: waiting for {} ({}s so far); it is not stalled", running.join(", "), t0.elapsed().as_secs()) });
            noted = Some(std::time::Instant::now());
        }
        if cancel.load(Ordering::SeqCst) {
            return (Waited { waited, secs: t0.elapsed().as_secs() }, Some("stop"));
        }
        if pause.load(Ordering::SeqCst) {
            return (Waited { waited, secs: t0.elapsed().as_secs() }, Some("pause"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    (Waited { waited, secs: t0.elapsed().as_secs() }, None)
}

/// Everything a run needs from its surroundings.
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf,
    pub features: PathBuf,
    pub state_path: PathBuf,
    pub goal: String,
    pub cfg: Config,
    pub read_only: bool,
    /// Continue from the saved place (a paused or stopped run) instead of starting over.
    pub resume: bool,
    /// What the person typed while the run was paused: it becomes part of the next step ("re-plan for this"), not a new goal.
    pub interjection: Option<String>,
    /// Rebuild each step's context from the system prompt, the goal, the board and the running summary instead of resending the
    /// conversation (default on). Off only for comparison in tests.
    pub compact: bool,
    /// The chat session this run belongs to (the teardown offer is made once per session).
    pub session: String,
    /// The PROJECT STATE block, rebuilt from the disk at every step (stack, board, bugs, deploy, last session).
    pub state: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// Workspace history (the knowledge base): steps are recorded in it and relevant earlier chunks are recalled into each step.
    pub history: Option<crate::history::History>,
    /// `autopilot.start|step|pause|stop` with the goal and budgets.
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    /// Leave an inbox item when the run finishes or pauses (title, text).
    pub notify: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    /// Carries out the steps (an external coding agent); `None`: the chat's model.
    pub executor: Option<Executor>,
    /// The workspace that holds the project notes (a task works in a copy: its notes are the workspace's). Default: `root`.
    pub notes_root: PathBuf,
    /// The key of this run in the agent registry (`assign`). A parallel task sets its task id (the task manager reports its state); empty: this
    /// run registers itself as `autopilot:<session>` and reports its own steps.
    pub agent_key: String,
    own_agent: bool,
}

impl Context {
    pub fn new(root: &Path, features: &Path, goal: &str) -> Context {
        Context { root: root.into(), features: features.into(), state_path: root.join(".iyakki/autopilot.json"), goal: goal.into(), cfg: Config::default(), read_only: false, resume: false, interjection: None, compact: true, session: String::new(), state: None, history: None, audit: None, notify: None, executor: None, notes_root: root.into(), agent_key: String::new(), own_agent: false }
    }
}

fn now() -> i64 {
    crate::clock::now_unix()
}

/// Command lines the model's answer suggests (a line starting with a known program, with or without a `$ ` or backticks) that were not
/// among the commands the harness ran. These are advice, never reported as done.
pub fn suggested_commands(text: &str, executed: &[String]) -> Vec<String> {
    const PROGS: [&str; 14] = ["kubectl", "docker", "podman", "helm", "npm", "npx", "node", "cargo", "git", "curl", "pip", "python3", "go", "make"];
    let looks = |t: &str| PROGS.contains(&t.split_whitespace().next().unwrap_or("")) && t.split_whitespace().count() >= 2;
    let mut out: Vec<String> = Vec::new();
    let add = |t: &str, out: &mut Vec<String>| {
        let t = t.trim();
        if looks(t) && !executed.iter().any(|e| e.trim() == t) && !out.iter().any(|o| o == t) && out.len() < 5 {
            out.push(crate::agent::truncate(t, 160));
        }
    };
    for l in text.lines() {
        let t = l.trim().trim_start_matches(['-', '*', '>', ' ']).trim_start_matches("$ ").trim_matches('`');
        add(t, &mut out);
        // inline `commands` in a sentence
        for (i, seg) in l.split('`').enumerate() {
            if i % 2 == 1 {
                add(seg.trim_start_matches("$ "), &mut out);
            }
        }
    }
    out
}

/// A goal that asks for the environment to be removed: "remove all", "tear down", "clean up", "delete the environment".
pub fn goal_is_teardown(goal: &str) -> bool {
    let l = goal.to_lowercase();
    ["remove all", "remove everything", "tear down", "teardown", "tear-down", "clean up", "cleanup", "delete the environment", "delete everything", "delete the namespace", "remove the environment", "remove the namespace"].iter().any(|w| l.contains(w))
}

/// May the final message offer to tear the dev environment down? Only after a deploy that succeeded and was verified, while the namespace still exists, not
/// for a teardown goal, and once per chat session.
fn offer_ok(ctx: &Context, tools: &Arc<dyn Tools>, commands: &[String], rows: &Rows, teardown_goal: bool) -> bool {
    if teardown_goal || !commands.iter().any(|c| c == "k8s_apply") || rows.done == 0 || rows.blocked > 0 || !rows.open.is_empty() {
        return false;
    }
    let key = if ctx.session.is_empty() { "run".to_string() } else { ctx.session.clone() };
    if crate::projectstate::noted(&ctx.root, "teardown_offered").as_deref() == Some(key.as_str()) {
        return false;
    }
    namespace_state(tools) == Some(true)
}

fn namespace_state(tools: &Arc<dyn Tools>) -> Option<bool> {
    // Some(true): the namespace exists; Some(false): it does not exist (any more); None: no cluster tools
    if !tools.defs().iter().any(|d| d.spec.name == "k8s_status") {
        return None;
    }
    let t = tools.call("k8s_status", &json!({})).ok()?;
    if t.contains("(exists)") {
        Some(true)
    } else if t.contains("does not exist") {
        Some(false)
    } else {
        None
    }
}

fn set_status(ctx: &Context, id: &str, status: &str, comment: &str, depends: Option<String>) {
    if let Ok(mut bf) = BoardFile::load(&ctx.features) {
        let u = crate::featureboard::Update { status: Some(status.into()), comment: Some(comment.into()), at: Some(now()), depends, ..Default::default() };
        if bf.board.update(id, &u).is_ok() && bf.save().is_ok() {
            if let Some(a) = &ctx.audit {
                a("board.auto", &format!("{id} -> {status} ({comment})"));
            }
        }
    }
}

/// Rows of the board that are open (planned or partial): (id, status, phase).
fn open_board_rows(ctx: &Context) -> Vec<(String, String, String)> {
    BoardFile::load(&ctx.features).map(|b| b.board.features().into_iter().filter(|f| matches!(f.status.as_str(), "planned" | "partial")).map(|f| (f.id, f.status, f.phase)).collect()).unwrap_or_default()
}

/// A goal about the board's own rows ("work on the features tasks", "do the backlog"): the rows already there are the plan.
fn goal_is_about_the_board(goal: &str) -> bool {
    let l = goal.to_lowercase();
    ["feature", "task", "row", "board", "backlog", "todo", "to-do", "continue", "resume", "carry on", "keep going", "pick up", "where we left", "next step"].iter().any(|w| l.contains(w))
}

/// The question with its numbered options (the chat shows a button for each), when the goal covers many rows.
fn scope_question(open: &[(String, String, String)]) -> String {
    let planned = open.iter().filter(|r| r.1 == "planned").count();
    let mut phases: BTreeMap<&str, usize> = BTreeMap::new();
    for r in open.iter().filter(|r| !r.2.is_empty()) {
        *phases.entry(r.2.as_str()).or_default() += 1;
    }
    let mut v = phases.into_iter().collect::<Vec<_>>();
    v.sort_by_key(|p| std::cmp::Reverse(p.1));
    let mut s = format!("The board has {} open rows. Which should I work through?\n\n1. Work through all {} rows\n2. Only the {planned} planned rows", open.len(), open.len());
    for (i, (p, n)) in v.iter().take(2).enumerate() {
        s.push_str(&format!("\n{}. Only phase {p} ({n} rows)", i + 3));
    }
    s
}

/// Take rows over as the plan, by the person's answer (all, planned only, or one phase). Returns how many.
fn adopt_rows(ctx: &Context, open: &[(String, String, String)], answer: &str) -> usize {
    let a = answer.to_lowercase();
    let phase = open.iter().map(|r| r.2.as_str()).filter(|p| !p.is_empty()).find(|p| a.contains(&format!("phase {}", p.to_lowercase())));
    let tracker = Tracker::new(&ctx.root, &ctx.features);
    let mut n = 0;
    for r in open {
        let take = match phase {
            Some(p) => r.2 == p,
            None => !(a.contains("only") && a.contains("planned")) || r.1 == "planned",
        };
        if take {
            tracker.own(&r.0);
            n += 1;
        }
    }
    n
}

fn mark_blocked(ctx: &Context, id: &str, reason: &str) {
    if let Ok(mut bf) = BoardFile::load(&ctx.features) {
        let notes = bf.board.get(id).map(|f| f.notes).unwrap_or_default();
        let base = notes.split(" · auto:").next().unwrap_or("").trim().to_string();
        let comment = if base.is_empty() { format!("autopilot: {reason}") } else { format!("{base} · autopilot: {reason}") };
        let u = crate::featureboard::Update { status: Some("blocked".into()), comment: Some(comment), at: Some(now()), ..Default::default() };
        if bf.board.update(id, &u).is_ok() && bf.save().is_ok() {
            if let Some(a) = &ctx.audit {
                a("board.auto", &format!("{id} -> blocked (autopilot: {reason})"));
            }
        }
    }
}

/// Start (or resume) an autopilot run. The first model request of each step is `base` plus the step message; `assist.prepare`
/// is wrapped so the person's chat context is rebuilt for every step and the step message comes last.
pub fn start(backend: Arc<dyn ChatBackend>, tools: Arc<dyn Tools>, base: Vec<Message>, mut limits: RunLimits, assist: Assist, ctx: Context) -> RunHandle {
    let (etx, erx) = channel::<RunEvent>();
    let (dtx, drx) = channel::<(String, Verdict)>();
    let cancel = Arc::new(AtomicBool::new(false));
    let pause = Arc::new(AtomicBool::new(false));
    let handle = RunHandle { events: erx, decisions: dtx, cancel: cancel.clone(), pause: Some(pause.clone()) };
    if ctx.read_only {
        let _ = etx.send(RunEvent::Failed { error: "Autopilot cannot start: this workspace is open read-only (another window has it).".into() });
        return handle;
    }
    // Auto approvals are implied; hard_ask calls still ask, and the card waits for the person
    limits.auto = true;
    limits.approval_timeout = Duration::from_secs(3600);
    std::thread::spawn(move || drive(backend, tools, base, limits, assist, ctx, etx, drx, cancel, pause));
    handle
}

#[allow(clippy::too_many_arguments)]
fn drive(backend: Arc<dyn ChatBackend>, tools: Arc<dyn Tools>, base: Vec<Message>, limits: RunLimits, assist: Assist, mut ctx: Context, etx: std::sync::mpsc::Sender<RunEvent>, drx: std::sync::mpsc::Receiver<(String, Verdict)>, cancel: Arc<AtomicBool>, pause: Arc<AtomicBool>) {
    // an Autopilot of the chat (not one of a task) is an agent of the workspace too: it shows in the Board's Agents strip
    ctx.own_agent = ctx.agent_key.is_empty();
    if ctx.own_agent {
        ctx.agent_key = format!("autopilot:{}", if ctx.session.is_empty() { "run" } else { &ctx.session });
        let engine = if ctx.executor.is_some() { "external agent" } else { "assistant" };
        crate::assign::update(&ctx.notes_root, |d| d.register(&ctx.agent_key, "autopilot", engine, &[], &ctx.session, now()));
    }
    let (own, key, nroot, spath) = (ctx.own_agent, ctx.agent_key.clone(), ctx.notes_root.clone(), ctx.state_path.clone());
    drive_run(backend, tools, base, limits, assist, ctx, etx, drx, cancel, pause);
    if own {
        let st = State::load(&spath);
        let (state, why) = match st.as_ref().map(|s| s.status.as_str()) {
            Some("finished") => (crate::assign::AState::Done, "finished".to_string()),
            Some("paused") => (crate::assign::AState::Paused, st.as_ref().map(|s| s.reason.clone()).unwrap_or_default()),
            Some("stopped") => (crate::assign::AState::Failed, st.as_ref().map(|s| s.reason.clone()).unwrap_or_default()),
            _ => (crate::assign::AState::Failed, "the run ended".to_string()),
        };
        crate::assign::update(&nroot, |d| d.set_state(&key, state, &why, now()));
    }
}

#[allow(clippy::too_many_arguments)]
fn drive_run(backend: Arc<dyn ChatBackend>, tools: Arc<dyn Tools>, base: Vec<Message>, limits: RunLimits, assist: Assist, ctx: Context, etx: std::sync::mpsc::Sender<RunEvent>, drx: std::sync::mpsc::Receiver<(String, Verdict)>, cancel: Arc<AtomicBool>, pause: Arc<AtomicBool>) {
    // a chat that went away (sign-out, window closed) ends the run
    let emit = |e: RunEvent| {
        if etx.send(e).is_err() {
            cancel.store(true, Ordering::SeqCst);
        }
    };
    let audit = |k: &str, d: &str| {
        if let Some(a) = &ctx.audit {
            a(k, d);
        }
    };
    let mut cfg = ctx.cfg.clone();
    if cfg.max_tokens == 0 {
        cfg.max_tokens = if limits.max_tokens > 0 { limits.max_tokens } else { 300_000 };
    }
    let budgets = format!("steps {}, {} min, tokens {}, no-progress {}, retries {}", cfg.max_steps, cfg.max_seconds / 60, cfg.max_tokens, cfg.max_no_progress, cfg.retries);
    let resumed = if ctx.resume { State::load(&ctx.state_path).filter(|s| matches!(s.status.as_str(), "paused" | "stopped" | "running")) } else { None };
    let mut st = match &resumed {
        Some(s) => {
            let mut s = s.clone();
            s.status = "running".into();
            s.reason.clear();
            s.reset_budgets(now());
            s
        }
        None => State::new(&ctx.goal, now()),
    };
    audit(if resumed.is_some() { "autopilot.resume" } else { "autopilot.start" }, &format!("goal: {} · budgets: {budgets}", crate::agent::truncate(&st.goal, 200)));
    emit(RunEvent::Note { text: format!("autopilot {}: {} · {budgets}", if resumed.is_some() { "resumed" } else { "on" }, crate::agent::truncate(&st.goal, 100)) });
    st.save(&ctx.state_path);
    // the board's own rows as the plan: adopted (after asking, when there are many) when the goal is about them
    let mut interjection = ctx.interjection.clone();
    let mut adopted = 0usize;
    let awaiting = resumed.as_ref().is_some_and(|s| s.awaiting_scope);
    if (resumed.is_none() || awaiting) && goal_is_about_the_board(&st.goal) && Tracker::new(&ctx.root, &ctx.features).owned_ids().is_empty() {
        let open = open_board_rows(&ctx);
        if awaiting {
            adopted = adopt_rows(&ctx, &open, interjection.as_deref().unwrap_or(""));
            interjection = None;
            st.awaiting_scope = false;
        } else if open.len() > 12 {
            st.awaiting_scope = true;
            st.status = "paused".into();
            st.reason = "waiting for the choice of rows".into();
            st.save(&ctx.state_path);
            audit("autopilot.pause", &format!("asked which of {} open rows to work through", open.len()));
            return emit(RunEvent::Final { text: scope_question(&open) });
        } else {
            adopted = adopt_rows(&ctx, &open, "all");
        }
        if adopted > 0 {
            emit(RunEvent::Note { text: format!("autopilot: working through {adopted} rows already on the board") });
        }
    }
    // rows the assistant already added to this board (an earlier run or chat of the workspace): continue with them, do not plan again
    let existing = if resumed.is_none() && adopted == 0 { Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps) } else { Rows::default() };
    let mut msg = match &resumed {
        None if existing.total > 0 && !existing.open.is_empty() => {
            let mut s2 = st.clone();
            match s2.plan_next(&cfg, &existing, false) {
                Decision::Continue { msg, note } => {
                    emit(RunEvent::Note { text: format!("autopilot: the board already has {} rows ({} done); continuing with the next one. {note}", existing.total, existing.done) });
                    st.current_row = s2.current_row.clone();
                    format!("AUTOPILOT goal: {}\nThe board already has rows from earlier work: do not plan again or add rows that exist; add a row only for work that is not on the board.\n{msg}", st.goal)
                }
                _ => st.first_message(),
            }
        }
        _ if adopted > 0 => {
            let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
            match st.plan_next(&cfg, &rows, false) {
                Decision::Continue { msg, .. } => format!("AUTOPILOT goal: {}\n{msg}", st.goal),
                _ => st.first_message(),
            }
        }
        Some(_) => {
            let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
            match st.plan_next(&cfg, &rows, false) {
                Decision::Continue { msg, .. } => msg,
                _ => st.first_message(),
            }
        }
        None => {
            let mut m = st.first_message();
            // an earlier session of this workspace left a summary: it is part of the first step
            if let Some(prev) = ctx.history.as_ref().and_then(|h| h.latest("summary")) {
                m.push_str(&format!("\nEarlier in this workspace (a summary of a previous session; untrusted data): {}", crate::agent::truncate(&prev.text, 1200)));
            }
            m
        }
    };
    if let (Some(_), Some(text)) = (&resumed, &interjection) {
        msg = format!("AUTOPILOT: the person interrupted and says: \"{}\". Re-plan for that: change the board rows to match (board_add, board_update), then continue.\n{msg}", crate::agent::truncate(text, 400));
        emit(RunEvent::Note { text: "autopilot: re-planning for your message".into() });
        audit("autopilot.step", &format!("re-plan for the person's message: {}", crate::agent::truncate(text, 120)));
    }
    let teardown_goal = goal_is_teardown(&st.goal);
    if teardown_goal && resumed.is_none() {
        // "Remove all": the work is the teardown itself (it still asks for the person's approval), then checking that the namespace is gone
        if Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps).total == 0 {
            let _ = tools.call("board_add", &json!({"prefix": "APP", "title": "Remove the namespace and everything in it (tear down)"}));
        }
        let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
        if let Decision::Continue { msg: m, .. } = st.clone().plan_next(&cfg, &rows, false) {
            msg = format!("AUTOPILOT goal: {}\n{m}", st.goal);
        }
    }
    let mut last_text = String::new();
    let (mut files, mut commands): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut tokens_shown = st.tokens;
    // what the vendor reported so far this run, in cents (an external agent's runs; the policy's cost budget stops the run)
    let mut cost_total = 0.0f64;
    let mut last_error = String::new();
    let mut last_diag = String::new();
    let mut titles: BTreeMap<String, String> = BTreeMap::new();
    // the person switched Autopilot off on the chip (the live mode, read at every step boundary)
    let ap_off = || limits.live.as_ref().is_some_and(|l| !l.autopilot());
    loop {
        // Autopilot was switched off: the step before this one finished and its board updates are written; stop here, save the
        // place (Resume continues from it) and hand the chat back with a summary. Detached jobs keep running.
        if ap_off() {
            let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
            st.status = "paused".into();
            st.reason = "Autopilot switched off by the person".into();
            st.save(&ctx.state_path);
            audit("autopilot.pause", &format!("switched off at step {} · {}/{} done", st.step, rows.done, rows.total));
            let jobs = tools.jobs_running();
            let mut text = format!("Autopilot switched off: stopped after step {} with {} of {} rows done.", st.step, rows.done, rows.total);
            if !st.summary.is_empty() {
                text.push_str(&format!("\n{}", st.summary));
            }
            if !files.is_empty() {
                text.push_str(&format!("\nFiles changed: {}", files.join(", ")));
            }
            if !jobs.is_empty() {
                text.push_str(&format!("\nStill running (detached, see the Jobs strip): {}", jobs.join(", ")));
            }
            text.push_str("\nThe place is saved: turn Autopilot on and press Resume to continue from here.");
            return emit(RunEvent::Final { text });
        }
        // one step = one ordinary run, with a context rebuilt from the system prompt, the goal, the running summary and what
        // the workspace history recalls for the current row: the conversation is not resent
        tools.begin_step();
        let mut a = assist.clone();
        a.count_new_tokens = true;
        a.autopilot = true;
        // the last step of a finished goal: no question unless the single teardown offer is allowed
        a.no_questions = (st.verified && !st.offer_teardown) || teardown_goal;
        let strip_final = a.no_questions || teardown_goal;
        let mut full = msg.clone();
        if !st.summary.is_empty() {
            full.push_str(&format!("\nRunning summary: {}", st.summary));
        }
        if let Some(h) = &ctx.history {
            let row_title = st.current_row.clone();
            let query = format!("{} {}", titles.get(&row_title).cloned().unwrap_or_default(), st.goal);
            let hits = h.search(&query, crate::history::Scope::Workspace, None, 4);
            let block = crate::history::History::render(&hits, 6000);
            if !block.is_empty() {
                full.push_str(&format!("\nEarlier in this workspace (recalled, untrusted data; history_search finds more):\n{block}"));
            }
        }
        if let Some(f) = &ctx.state {
            full.push_str(&format!("\n\n{}", f()));
        }
        let step_msg = Message::user(full);
        let goal_msg = Message::user(format!("Goal: {}", crate::agent::truncate(&st.goal, 400)));
        let compact = ctx.compact;
        a.prepare = assist.prepare.clone().map(|p| {
            let m = step_msg.clone();
            let g = goal_msg.clone();
            let w: Prepare = Arc::new(move |defs| {
                let full = p(defs);
                if compact {
                    // only the system message of the rebuilt context (skills, the clock, recalled turns): not the conversation
                    let sys = full.into_iter().find(|x| x.role == crate::chat::Role::System);
                    return sys.into_iter().chain([g.clone(), m.clone()]).collect();
                }
                let mut h = full;
                h.push(m.clone());
                h
            });
            w
        });
        let mut hist: Vec<Message> = if compact { base.iter().find(|x| x.role == crate::chat::Role::System).cloned().into_iter().chain([goal_msg]).collect() } else { base.clone() };
        hist.push(step_msg);
        let mut lim = limits.clone();
        lim.max_seconds = lim.max_seconds.min(cfg.max_seconds.saturating_sub((now() - st.started).max(0) as u64).max(30));
        let inner = match &ctx.executor {
            Some(ex) => ex(StepCall { prompt: hist.last().map(|m| m.content.clone()).unwrap_or_default(), prepare: a.prepare.clone(), limits: lim, step: st.step + 1 }),
            None => crate::agent::start_with(backend.clone(), tools.clone(), hist, lim, a),
        };
        let (mut end, mut step_tokens, mut denied, mut interrupted) = (None::<StepEnd>, 0u64, false, None::<&'static str>);
        let mut step_cost = 0.0f64;
        // what the model asked for, what was approved, and so what really ran: the summary lists only that
        let mut asked: std::collections::HashMap<String, (String, Value)> = Default::default();
        let mut approved: std::collections::HashSet<String> = Default::default();
        let mut step_changed = false;
        while end.is_none() {
            loop {
                match drx.try_recv() {
                    Ok((id, v)) => {
                        if v == Verdict::Deny {
                            denied = true;
                        }
                        inner.decide(&id, v);
                    }
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
            if interrupted.is_none() {
                if cancel.load(Ordering::SeqCst) {
                    interrupted = Some("stop");
                    inner.cancel();
                } else if pause.load(Ordering::SeqCst) {
                    interrupted = Some("pause");
                    inner.cancel();
                }
            }
            match inner.events.recv_timeout(Duration::from_millis(40)) {
                Ok(e) => match e {
                    RunEvent::Final { text } => {
                        let text = if strip_final { crate::assistant_guard::strip_closing_question(&text) } else { text };
                        last_text = text.clone();
                        if let Some(h) = &ctx.history {
                            h.record("turn", &st.current_row, &text);
                        }
                        if !text.trim().is_empty() {
                            emit(RunEvent::Model { text, calls: vec![] });
                        }
                        end = Some(StepEnd::Final);
                    }
                    RunEvent::Failed { error } => end = Some(StepEnd::Failed(error)),
                    RunEvent::Stopped { reason } => end = Some(StepEnd::Stopped(reason)),
                    RunEvent::Usage { tokens, cost_cents, files: f, seconds } => {
                        step_tokens = tokens;
                        step_cost = cost_cents;
                        // a vendor-reported run shows the cost of the whole autopilot run so far
                        emit(RunEvent::Usage { tokens: tokens_shown + tokens, cost_cents: if ctx.executor.is_some() { cost_total + cost_cents } else { cost_cents }, files: f, seconds });
                    }
                    RunEvent::Timing { .. } => {}
                    RunEvent::ToolResult { id, name, ok, text } => {
                        if !ok {
                            last_error = format!("{name}: {}", text.lines().next().unwrap_or(""));
                        }
                        if ctx.own_agent && !matches!(name.as_str(), "read_file" | "list_dir" | "board_list" | "board_depends" | "note_list" | "note_get") {
                            let args = asked.get(&id).map(|x| x.1.clone()).unwrap_or_else(|| json!({}));
                            let (kind, line) = crate::assign::classify(&name, ok, &args, &text);
                            let path = args.get("path").or_else(|| args.get("file_path")).and_then(Value::as_str).map(str::to_string);
                            let is_write = ok && matches!(name.as_str(), "write_file" | "Write" | "Edit" | "MultiEdit");
                            crate::assign::update(&ctx.notes_root, |d| {
                                d.step(&ctx.agent_key, kind, &line, now());
                                if let (true, Some(p)) = (is_write, path) {
                                    d.touch_file(&ctx.agent_key, &p, 0, 0, now());
                                }
                                if let (true, Some(id)) = (ok && name == "note_add", text.strip_prefix("added N-").map(|t| format!("N-{}", t.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("")))) {
                                    d.note_added(&ctx.agent_key, &id, args.get("title").and_then(Value::as_str).unwrap_or(""), now());
                                }
                            });
                        }
                        // only calls that were approved and produced a result count as done: never a call that was refused or only printed in prose
                        if approved.remove(&id) {
                            if let Some((n, args)) = asked.remove(&id) {
                                match n.as_str() {
                                    "write_file" if ok => {
                                        step_changed = true;
                                        if let Some(p) = args.get("path").and_then(Value::as_str) {
                                            if !files.iter().any(|f| f == p) {
                                                files.push(p.to_string());
                                            }
                                        }
                                    }
                                    // an external agent's own tools (Claude Code): what its permission prompts approved and it then ran
                                    "Write" | "Edit" | "MultiEdit" | "NotebookEdit" if ok => {
                                        step_changed = true;
                                        if let Some(p) = args.get("file_path").or_else(|| args.get("notebook_path")).and_then(Value::as_str) {
                                            if !files.iter().any(|f| f == p) {
                                                files.push(p.to_string());
                                            }
                                        }
                                    }
                                    "Bash" => {
                                        tools.note_external(&n, &args, ok);
                                        commands.push(args.get("command").and_then(Value::as_str).unwrap_or("").trim().chars().take(160).collect());
                                        step_changed = true;
                                    }
                                    "run_command" => {
                                        let prog = args.get("program").and_then(Value::as_str).unwrap_or("");
                                        let a: Vec<&str> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                                        commands.push(format!("{prog} {}", a.join(" ")).trim().to_string());
                                        step_changed = true;
                                    }
                                    "deploy_build" | "deploy_load" | "k8s_apply" | "k8s_rollout_restart" => {
                                        commands.push(n.clone());
                                        step_changed = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        // a digest goes to the workspace history: dropped from the context, never lost
                        if let Some(h) = &ctx.history {
                            h.record(if ok { "tool" } else { "error" }, &st.current_row, &format!("{name}: {}", crate::agent::truncate(&text, 500)));
                        }
                        emit(RunEvent::ToolResult { id, name, ok, text });
                    }
                    RunEvent::Approval { id, name, summary, detail } => {
                        if ctx.own_agent {
                            crate::assign::update(&ctx.notes_root, |d| d.set_state(&ctx.agent_key, crate::assign::AState::WaitingApproval, &format!("{name} {}", crate::agent::truncate(&summary, 100)), now()));
                        }
                        audit("autopilot.pause", &format!("needs you: {name}: {summary}"));
                        emit(RunEvent::Note { text: format!("autopilot paused: needs you ({name}). Approve, or deny to skip this row; Stop ends autopilot.") });
                        emit(RunEvent::Approval { id, name, summary, detail });
                    }
                    RunEvent::Model { text, calls } => {
                        for c in &calls {
                            asked.insert(c.id.clone(), (c.name.clone(), c.args.clone()));
                        }
                        emit(RunEvent::Model { text, calls });
                    }
                    RunEvent::Decision { id, name, approved: ok, by } => {
                        if ctx.own_agent {
                            crate::assign::update(&ctx.notes_root, |d| d.set_state(&ctx.agent_key, crate::assign::AState::Working, "", now()));
                        }
                        if ok {
                            approved.insert(id.clone());
                        }
                        emit(RunEvent::Decision { id, name, approved: ok, by });
                    }
                    other => emit(other),
                },
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => end = Some(StepEnd::Failed("the step ended without an answer".into())),
            }
        }
        tokens_shown = st.tokens + step_tokens;
        // a detached job is still running (the step started it and ended its turn): the run waits for it instead of calling
        // the step a stall, and the waiting time is not charged to the time budget
        let mut waited = false;
        if interrupted.is_none() && !matches!(end, Some(StepEnd::Failed(_))) {
            let (w, why) = wait_for_jobs_until(tools.as_ref(), &cancel, &pause, &ap_off, &emit, JOB_WAIT_CAP);
            if w.waited {
                waited = true;
                st.started += w.secs as i64;
            }
            // switched off: no waiting; the step is settled below and the loop stops at its top
            interrupted = why.filter(|k| *k != "off");
        }
        st.waiting = waited;
        if let Some(kind) = interrupted {
            st.tokens += step_tokens;
            st.status = if kind == "pause" { "paused".into() } else { "stopped".into() };
            st.reason = if kind == "pause" { "paused by the person".into() } else { "stopped by the person".into() };
            st.save(&ctx.state_path);
            audit(if kind == "pause" { "autopilot.pause" } else { "autopilot.stop" }, &format!("{} at step {}", st.reason, st.step + 1));
            return emit(RunEvent::Stopped { reason: format!("autopilot {} at step {}. {}", st.reason, st.step + 1, if kind == "pause" { "Resume continues from here." } else { "" }) });
        }
        // a tool that kept returning the same result stopped the step: the harness settles the row itself (verify it, or block it)
        if let Some(StepEnd::Stopped(r)) = &end {
            if r.contains("same arguments") {
                let before = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
                if let Some((id, title, _)) = before.open.iter().find(|r| r.2 == "partial").or(before.open.first()).cloned() {
                    let k = crate::boardtrack::kinds_of(&title);
                    let checkable = k.contains(&crate::boardtrack::Kind::Apply) || k.contains(&crate::boardtrack::Kind::Verify) || k.contains(&crate::boardtrack::Kind::Namespace);
                    let ready = checkable && tools.defs().iter().any(|d| d.spec.name == "deploy_status") && tools.call("deploy_status", &json!({})).is_ok_and(|t| crate::boardtrack::all_ready(&t));
                    if ready {
                        Tracker::new(&ctx.root, &ctx.features).mark_verified(&[id.clone()]);
                        set_status(&ctx, &id, "done", "autopilot: verified by deploy_status after a tool loop", None);
                        emit(RunEvent::Note { text: format!("autopilot: a tool kept repeating itself; {id} was verified by deploy_status and is done") });
                    } else {
                        mark_blocked(&ctx, &id, "a tool kept returning the same result and nothing verified the row");
                        emit(RunEvent::Note { text: format!("autopilot: a tool kept repeating itself; {id} is blocked and the run moves on") });
                    }
                    st.tries.remove(&id);
                    st.current_row.clear();
                }
            }
        }
        st.step_changed = step_changed;
        if let Some((id, d)) = tools.take_diagnosis() {
            last_diag = d.clone();
            st.diagnosis = Some((id, d));
        }
        let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
        if let Ok(bf) = BoardFile::load(&ctx.features) {
            for f in bf.board.features() {
                titles.insert(f.id.clone(), f.title.clone());
            }
        }
        let mut rows = rows;
        // "Remove all": the teardown is verified by the namespace being gone; then the goal is done, with no question
        let mut forced_finish = false;
        if teardown_goal && namespace_state(&tools) == Some(false) {
            for (id, _, _) in rows.open.clone() {
                Tracker::new(&ctx.root, &ctx.features).mark_verified(&[id.clone()]);
                set_status(&ctx, &id, "done", "autopilot: verified, the namespace no longer exists", None);
            }
            rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
            forced_finish = true;
        }
        if !st.verified {
            st.offer_teardown = offer_ok(&ctx, &tools, &commands, &rows, teardown_goal);
        }
        cost_total += step_cost;
        let mut d = st.after_step(&cfg, end.as_ref().expect("set above"), &rows, step_tokens, now(), denied);
        if ctx.executor.is_some() && limits.max_cost_cents > 0 && cost_total >= limits.max_cost_cents as f64 && !matches!(d, Decision::Stop(_) | Decision::Finish) {
            d = Decision::Stop(format!("cost budget used (${:.2} of ${:.2})", cost_total / 100.0, limits.max_cost_cents as f64 / 100.0));
        }
        if forced_finish {
            d = Decision::Finish;
        }
        for _ in 0..3 {
            match &d {
                Decision::Block { id, reason } => {
                    let why = if last_diag.is_empty() { reason.clone() } else { format!("{reason}; last diagnosis: {}", crate::agent::truncate(last_diag.lines().find(|l| l.contains("Diagnosis") || l.contains("Error") || l.contains("BackOff")).unwrap_or(last_diag.lines().next().unwrap_or("")), 160)) };
                    mark_blocked(&ctx, id, &why);
                    // a failure that stopped a row for good is worth remembering for every later step and agent: one note per incident
                    // (the same title is not added twice), nothing secret
                    let title = format!("{id} keeps failing");
                    if crate::notes::auto_note(&ctx.notes_root, "Autopilot", crate::notes::NewNote { source: id.clone(), ..crate::notes::NewNote::new(&title, &crate::agent::truncate(&crate::bugs::redact(&why), 500), crate::notes::Kind::Gotcha) }).is_some() {
                        emit(RunEvent::Note { text: format!("autopilot: noted why {id} failed (Context tab)") });
                    }
                    if let Some(h) = &ctx.history {
                        h.record("decision", id, &format!("{id} blocked: {reason}"));
                    }
                    emit(RunEvent::Note { text: format!("autopilot: {id} blocked ({reason}); moving on") });
                    st.tries.remove(id);
                    st.current_row.clear();
                }
                Decision::Pending { id, depends } => {
                    if let Some(h) = &ctx.history {
                        h.record("decision", id, &format!("{id} pending: waits for {depends}"));
                    }
                    set_status(&ctx, id, "pending", &format!("autopilot: waits for {depends}"), Some(depends.clone()));
                    emit(RunEvent::Note { text: format!("autopilot: {id} is pending (waits for {depends}); moving on and coming back later") });
                    st.tries.remove(id);
                    st.current_row.clear();
                }
                Decision::Revisit { id, why } => {
                    set_status(&ctx, id, "partial", &format!("autopilot: revisiting ({why})"), Some(String::new()));
                    emit(RunEvent::Note { text: format!("autopilot: back to {id} ({why})") });
                    st.tries.remove(id);
                }
                _ => break,
            }
            let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
            if !st.verified {
                st.offer_teardown = offer_ok(&ctx, &tools, &commands, &rows, teardown_goal);
            }
            d = st.plan_next(&cfg, &rows, false);
        }
        st.update_summary(&rows, &titles, &files, &commands, &last_error, &last_text);
        if let Some(h) = &ctx.history {
            h.record("summary", "", &st.summary);
        }
        audit("autopilot.step", &format!("step {} · {}/{} done · {}", st.step, rows.done, rows.total, match &d { Decision::Continue { note, .. } => note.clone(), other => format!("{other:?}") }));
        match d {
            Decision::Continue { msg: m, note } => {
                st.save(&ctx.state_path);
                if ctx.own_agent {
                    let cur: Vec<String> = if st.current_row.is_empty() { vec![] } else { vec![st.current_row.clone()] };
                    crate::assign::update(&ctx.notes_root, |d| {
                        d.set_rows(&ctx.agent_key, &cur);
                        d.step(&ctx.agent_key, crate::assign::StepKind::Plan, &note, now());
                        d.set_usage(&ctx.agent_key, st.tokens, 0.0);
                        d.set_progress(&ctx.agent_key, if rows.total == 0 { 0 } else { (rows.done * 100 / rows.total) as u32 });
                    });
                }
                emit(RunEvent::Note { text: format!("{note} · ~{}k tok last step", st.step_tokens.div_ceil(1000)) });
                msg = m;
                if teardown_goal {
                    msg.push_str("\nThis goal is a teardown: call k8s_teardown (the person approves it and types the namespace), then check with k8s_status that the namespace is gone, then write the summary. Do not plan other rows and ask nothing.");
                }
                // the one optional offer to tear down: noted now, so it is not made again in this chat
                if st.verified && st.offer_teardown {
                    crate::projectstate::note(&ctx.root, "teardown_offered", if ctx.session.is_empty() { "run" } else { &ctx.session });
                }
                // a check failed: the model gets the concrete diagnosis and is told to fix it, not to describe it
                // (and again in every following step until the row is done or parked, so a step that only described does not lose it)
                if let Some((id, d)) = st.diagnosis.clone().filter(|(id, _)| id.is_empty()) {
                    // a failed build belongs to whatever row is being worked on: shown once
                    let _ = id;
                    msg.push_str(&format!("\n{}", crate::agent::truncate(&d, 1800)));
                    st.diagnosis = None;
                }
                if let Some((id, d)) = st.diagnosis.clone() {
                    if !rows.open.iter().any(|r| r.0 == id) {
                        st.diagnosis = None;
                    } else {
                        msg.push_str(&format!("\nThe check for {id} FAILED. Diagnosis:\n{}\nChange the files or manifests that cause it, then rebuild and redeploy (write_file, deploy_build, deploy_load, k8s_apply). Do not describe a fix: make it with tools. Then claim the row done again.", crate::agent::truncate(&d, 1500)));
                    }
                }
            }
            Decision::Finish => {
                let rows = Rows::read_with(&ctx.root, &ctx.features, ctx.cfg.ignore_deps);
                st.status = "finished".into();
                st.save(&ctx.state_path);
                let mut text = if st.offer_teardown && st.verified { last_text.clone() } else { crate::assistant_guard::strip_closing_question(&last_text) };
                text.push_str(&format!("\n\nAutopilot finished after {} steps: {} of {} rows done (verified){}{}.", st.step, rows.done, rows.total, if rows.blocked > 0 { format!(", {} blocked (see the board; they need you)", rows.blocked) } else { String::new() }, if rows.claimed > 0 { format!(", {} marked done without a check (claimed, not counted)", rows.claimed) } else { String::new() }));
                if !rows.bugs.is_empty() {
                    text.push_str(&format!("\nOpen bugs (B-): {}", rows.bugs.iter().map(|b| format!("{} {} ({})", b.0, crate::agent::truncate(&b.1, 60), b.2)).collect::<Vec<_>>().join("; ")));
                }
                if !rows.review.is_empty() {
                    text.push_str(&format!("\nIn review (work claimed; needs your OK, one click on the board): {}", rows.review.iter().map(|r| r.0.clone()).collect::<Vec<_>>().join(", ")));
                }
                if !rows.pending.is_empty() {
                    text.push_str(&format!("\nPending: {}", rows.pending.iter().map(|p| format!("{} (waits for {})", p.0, if p.2.is_empty() { "something not named" } else { &p.2 })).collect::<Vec<_>>().join("; ")));
                }
                if !files.is_empty() {
                    text.push_str(&format!("\nFiles changed: {}", files.join(", ")));
                }
                if !commands.is_empty() {
                    text.push_str(&format!("\nCommands run: {}", commands.join("; ")));
                }
                let suggested = suggested_commands(&last_text, &commands);
                if !suggested.is_empty() {
                    text.push_str(&format!("\nSuggested, not run: {}", suggested.join("; ")));
                }
                if let Some(h) = &ctx.history {
                    h.record("summary", "", &format!("Session summary: {}", st.summary));
                }
                audit("autopilot.stop", &format!("finished · {}/{} done", rows.done, rows.total));
                if let Some(n) = &ctx.notify {
                    n("Autopilot finished", &format!("{}: {}/{} rows done{}.", crate::agent::truncate(&st.goal, 80), rows.done, rows.total, if rows.blocked > 0 { format!(", {} blocked", rows.blocked) } else { String::new() }));
                }
                return emit(RunEvent::Final { text });
            }
            Decision::Stop(why) | Decision::Pause(why) => {
                st.status = "stopped".into();
                st.reason = why.clone();
                st.save(&ctx.state_path);
                audit("autopilot.stop", &why);
                // a budget ending is not a failure: the state and a summary are written, and Resume starts again with fresh budgets and a fresh context
                if why.contains("budget") {
                    emit(RunEvent::Model { text: format!("Autopilot reached its {why}. Nothing is lost: the plan is on the board and the progress is saved.\n{}\nResume (fresh context) continues from this state with new budgets.", st.summary), calls: vec![] });
                    if let Some(h) = &ctx.history {
                        h.record("summary", "", &format!("Stopped at the {why}. {}", st.summary));
                    }
                }
                if let Some(n) = &ctx.notify {
                    n("Autopilot stopped", &format!("{}: {why}. Open the chat to resume.", crate::agent::truncate(&st.goal, 80)));
                }
                return emit(RunEvent::Stopped { reason: format!("autopilot stopped at step {}: {why}. Resume continues from here.", st.step) });
            }
            Decision::Block { .. } | Decision::Pending { .. } | Decision::Revisit { .. } => unreachable!("handled above"),
        }
    }
}
