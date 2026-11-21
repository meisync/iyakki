//! Types moved out of `iyakki_agent::assign` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AState {
    Working,
    WaitingApproval,
    /// Cannot go on: a file held by another agent, a dependency, a failing step it gave up on.
    Blocked,
    Paused,
    Idle,
    Failed,
    Done,
    /// The window closed while it ran.
    Interrupted,
}

impl AState {
    pub fn key(self) -> &'static str {
        match self {
            AState::Working => "working",
            AState::WaitingApproval => "waiting",
            AState::Blocked => "blocked",
            AState::Paused => "paused",
            AState::Idle => "idle",
            AState::Failed => "failed",
            AState::Done => "done",
            AState::Interrupted => "interrupted",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AState::Working => "working",
            AState::WaitingApproval => "waiting for approval",
            AState::Blocked => "blocked",
            AState::Paused => "paused",
            AState::Idle => "idle",
            AState::Failed => "failed",
            AState::Done => "done",
            AState::Interrupted => "interrupted",
        }
    }
    pub fn parse(s: &str) -> AState {
        match s {
            "working" => AState::Working,
            "waiting" => AState::WaitingApproval,
            "blocked" => AState::Blocked,
            "paused" => AState::Paused,
            "failed" => AState::Failed,
            "done" => AState::Done,
            "interrupted" => AState::Interrupted,
            _ => AState::Idle,
        }
    }
    /// Counts as an agent at work: it holds its rows and files.
    pub fn active(self) -> bool {
        matches!(self, AState::Working | AState::WaitingApproval | AState::Blocked | AState::Paused)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Plan,
    Tool,
    File,
    Test,
    Board,
    Note,
    Message,
    State,
}

impl StepKind {
    pub fn key(self) -> &'static str {
        match self {
            StepKind::Plan => "plan",
            StepKind::Tool => "tool",
            StepKind::File => "file",
            StepKind::Test => "test",
            StepKind::Board => "board",
            StepKind::Note => "note",
            StepKind::Message => "message",
            StepKind::State => "state",
        }
    }
    pub fn parse(s: &str) -> StepKind {
        match s {
            "plan" => StepKind::Plan,
            "file" => StepKind::File,
            "test" => StepKind::Test,
            "board" => StepKind::Board,
            "note" => StepKind::Note,
            "message" => StepKind::Message,
            "state" => StepKind::State,
            _ => StepKind::Tool,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub at: i64,
    pub kind: StepKind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touched {
    pub path: String,
    pub adds: usize,
    pub dels: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRun {
    /// What identifies the run: a task id, or `autopilot:<session>`.
    pub key: String,
    /// `Agent 2`, or the name the person set.
    pub name: String,
    /// A person-set name replaces the numbered one.
    pub custom_name: bool,
    pub number: u32,
    /// `Claude Code (sonnet)`, `built-in (qwen2.5)`.
    pub engine: String,
    /// `task`, `autopilot`, `external`.
    pub kind: String,
    pub rows: Vec<String>,
    pub state: AState,
    pub started: i64,
    pub updated: i64,
    pub ended: i64,
    pub steps: Vec<Step>,
    pub files: Vec<Touched>,
    pub tokens: u64,
    pub cost_cents: f64,
    pub notes: Vec<String>,
    /// The line shown in the strip: what it is doing now.
    pub last: String,
    /// Where the person finds the run: a task id (Tasks tab) or a session (RUNS).
    pub run_ref: String,
    /// Progress 0..=100 of its rows, as the board counts it.
    pub progress: u32,
    /// The latest run log of the agent (`runs::RunLog` id): what RUNS opens.
    pub run_id: String,
    /// The person pressed Stop on the board for an Autopilot of the chat: the run reads this at its next step, stops and clears it.
    pub stop_requested: bool,
}

impl AgentRun {
    /// `Agent 2 · Claude Code (sonnet)`
    pub fn title(&self) -> String {
        if self.engine.is_empty() { self.name.clone() } else { format!("{} · {}", self.name, self.engine) }
    }
    pub fn elapsed(&self, now: i64) -> i64 {
        if self.started == 0 {
            return 0;
        }
        (if self.state.active() || self.ended == 0 { now } else { self.ended } - self.started).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Warned; the second agent goes on (default).
    Pending,
    /// The second agent waits until the holder is done.
    Queue,
    /// The person looked and let it continue.
    Continue,
}

impl Choice {
    pub fn key(self) -> &'static str {
        match self {
            Choice::Pending => "pending",
            Choice::Queue => "queue",
            Choice::Continue => "continue",
        }
    }
    pub fn parse(s: &str) -> Choice {
        match s {
            "queue" => Choice::Queue,
            "continue" => Choice::Continue,
            _ => Choice::Pending,
        }
    }
}

use std::path::{Path, PathBuf};
use serde_json::{json, Value};

pub const MAX_STEPS: usize = 300;
pub const MAX_KEPT: usize = 30;











#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// The agent that touched it first (key and name).
    pub holder: String,
    pub holder_name: String,
    pub other: String,
    pub other_name: String,
    pub choice: Choice,
    pub at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub agents: Vec<AgentRun>,
    pub conflicts: Vec<Conflict>,
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

impl Doc {
    pub fn get(&self, key: &str) -> Option<&AgentRun> {
        self.agents.iter().find(|a| a.key == key)
    }
    pub fn by_name(&self, name: &str) -> Option<&AgentRun> {
        self.agents.iter().find(|a| a.name == name)
    }
    fn at(&mut self, key: &str) -> Option<&mut AgentRun> {
        self.agents.iter_mut().find(|a| a.key == key)
    }
    pub fn active(&self) -> impl Iterator<Item = &AgentRun> {
        self.agents.iter().filter(|a| a.state.active())
    }

    /// Make (or find) the agent of a run. The same key keeps its name; a new one gets the next free number.
    pub fn register(&mut self, key: &str, kind: &str, engine: &str, rows: &[String], run_ref: &str, now: i64) -> String {
        if let Some(a) = self.at(key) {
            a.engine = engine.to_string();
            a.rows = rows.to_vec();
            if !a.state.active() {
                a.state = AState::Working;
                a.ended = 0;
            }
            a.updated = now;
            return a.name.clone();
        }
        let number = self.agents.iter().map(|a| a.number).max().unwrap_or(0) + 1;
        let name = format!("Agent {number}");
        self.agents.push(AgentRun { key: key.into(), name: name.clone(), custom_name: false, number, engine: engine.into(), kind: kind.into(), rows: rows.to_vec(), state: AState::Working, started: now, updated: now, ended: 0, steps: vec![], files: vec![], tokens: 0, cost_cents: 0.0, notes: vec![], last: String::new(), run_ref: run_ref.into(), progress: 0, run_id: String::new(), stop_requested: false });
        self.prune();
        name
    }

    /// Finished agents are kept for the drill-down, the newest [`MAX_KEPT`].
    fn prune(&mut self) {
        let over = self.agents.iter().filter(|a| !a.state.active()).count().saturating_sub(MAX_KEPT);
        let mut left = over;
        self.agents.retain(|a| {
            if left > 0 && !a.state.active() {
                left -= 1;
                false
            } else {
                true
            }
        });
    }

    /// The person names an agent (`Reviewer`); an empty name goes back to the number.
    pub fn rename(&mut self, key: &str, name: &str) -> Result<(), String> {
        let name = clip(name.trim(), 24);
        if !name.is_empty() && self.agents.iter().any(|a| a.key != key && a.name.eq_ignore_ascii_case(&name)) {
            return Err(format!("another agent is already called {name}"));
        }
        let a = self.at(key).ok_or("no such agent")?;
        if name.is_empty() {
            a.name = format!("Agent {}", a.number);
            a.custom_name = false;
        } else {
            a.name = name;
            a.custom_name = true;
        }
        Ok(())
    }

    /// Stop on the board for an Autopilot of the chat (its run lives in the chat, so the board leaves it a request that the run reads at
    /// its next step). Only an Autopilot that is going on can be asked; a task is stopped by its own manager.
    pub fn request_stop(&mut self, key: &str) -> Result<(), String> {
        let a = self.at(key).ok_or("there is no such agent")?;
        if a.kind != "autopilot" {
            return Err("only an Autopilot of the chat is stopped this way".into());
        }
        if !a.state.active() {
            return Err("this Autopilot is not running".into());
        }
        a.stop_requested = true;
        Ok(())
    }

    /// Was a stop asked for on the board? (Read by the Autopilot's own run.)
    pub fn stop_asked(&self, key: &str) -> bool {
        self.get(key).is_some_and(|a| a.stop_requested)
    }

    pub fn clear_stop(&mut self, key: &str) {
        if let Some(a) = self.at(key) {
            a.stop_requested = false;
        }
    }

    pub fn set_state(&mut self, key: &str, st: AState, why: &str, now: i64) {
        let Some(a) = self.at(key) else { return };
        if a.state != st {
            a.steps.push(Step { at: now, kind: StepKind::State, text: clip(&format!("{}{}", st.label(), if why.is_empty() { String::new() } else { format!(": {why}") }), 200) });
        }
        a.state = st;
        a.updated = now;
        if !st.active() {
            a.ended = now;
        } else {
            a.ended = 0;
        }
        if !why.is_empty() {
            a.last = clip(why, 160);
        }
        if !st.active() {
            self.release(key);
        }
    }

    pub fn set_rows(&mut self, key: &str, rows: &[String]) {
        if let Some(a) = self.at(key) {
            a.rows = rows.to_vec();
        }
    }

    pub fn set_run(&mut self, key: &str, run_id: &str) {
        if let Some(a) = self.at(key) {
            a.run_id = run_id.to_string();
        }
    }

    pub fn set_progress(&mut self, key: &str, p: u32) {
        if let Some(a) = self.at(key) {
            a.progress = p.min(100);
        }
    }

    pub fn set_usage(&mut self, key: &str, tokens: u64, cost_cents: f64) {
        if let Some(a) = self.at(key) {
            a.tokens = tokens;
            a.cost_cents = cost_cents;
        }
    }

    pub fn step(&mut self, key: &str, kind: StepKind, text: &str, now: i64) {
        let Some(a) = self.at(key) else { return };
        let text = clip(&text.split_whitespace().collect::<Vec<_>>().join(" "), 240);
        if a.steps.last().is_some_and(|l| l.kind == kind && l.text == text) {
            return;
        }
        a.steps.push(Step { at: now, kind, text: text.clone() });
        if a.steps.len() > MAX_STEPS {
            let drop = a.steps.len() - MAX_STEPS;
            a.steps.drain(..drop);
        }
        a.updated = now;
        if matches!(kind, StepKind::Tool | StepKind::Plan | StepKind::Test | StepKind::Board) {
            a.last = clip(&text, 160);
        }
    }

    pub fn note_added(&mut self, key: &str, id: &str, title: &str, now: i64) {
        if let Some(a) = self.at(key) {
            if !a.notes.iter().any(|n| n == id) {
                a.notes.push(id.to_string());
            }
        }
        self.step(key, StepKind::Note, &format!("note {id}: {title}"), now);
    }

    /// An active agent already working on `row` (other than `key`).
    pub fn holder_of_row(&self, row: &str, key: &str) -> Option<&AgentRun> {
        self.active().find(|a| a.key != key && a.rows.iter().any(|r| r == row))
    }

    /// Give `row` to an agent. Refused when another active agent has it.
    pub fn claim_row(&mut self, key: &str, row: &str) -> Result<(), String> {
        if let Some(h) = self.holder_of_row(row, key) {
            return Err(format!("{row} is already being worked on by {}", h.name));
        }
        let a = self.at(key).ok_or("no such agent")?;
        if !a.rows.iter().any(|r| r == row) {
            a.rows.push(row.to_string());
        }
        Ok(())
    }

    /// The agent `key` touched `path` (adds/dels as far as known). If another active agent already holds the file the conflict is recorded
    /// and returned (once per pair and file).
    pub fn touch_file(&mut self, key: &str, path: &str, adds: usize, dels: usize, now: i64) -> Option<Conflict> {
        let path = path.trim_start_matches("./").to_string();
        let holder = self.agents.iter().find(|a| a.key != key && a.state.active() && a.files.iter().any(|f| f.path == path)).map(|a| (a.key.clone(), a.name.clone()));
        let (name, existing) = {
            let a = self.at(key)?;
            match a.files.iter_mut().find(|f| f.path == path) {
                Some(f) => {
                    f.adds = f.adds.max(adds);
                    f.dels = f.dels.max(dels);
                    (a.name.clone(), true)
                }
                None => {
                    a.files.push(Touched { path: path.clone(), adds, dels });
                    (a.name.clone(), false)
                }
            }
        };
        let _ = existing;
        let (hk, hn) = holder?;
        if self.conflicts.iter().any(|c| c.path == path && c.holder == hk && c.other == key) {
            return None;
        }
        let c = Conflict { path: path.clone(), holder: hk, holder_name: hn.clone(), other: key.to_string(), other_name: name.clone(), choice: Choice::Pending, at: now };
        self.conflicts.push(c.clone());
        self.step(key, StepKind::File, &format!("{path} is also being changed by {hn}"), now);
        Some(c)
    }

    /// The person decides what happens to the second agent of a conflict.
    pub fn resolve_conflict(&mut self, path: &str, other: &str, choice: Choice) -> bool {
        let mut hit = false;
        for c in self.conflicts.iter_mut().filter(|c| c.path == path && c.other == other) {
            c.choice = choice;
            hit = true;
        }
        hit
    }

    /// Must agent `key` wait before writing `path`? The holder's name, when the person chose to queue it and the holder is still working.
    pub fn must_wait(&self, key: &str, path: &str) -> Option<String> {
        let path = path.trim_start_matches("./");
        self.conflicts.iter().find(|c| c.other == key && c.path == path && c.choice == Choice::Queue && self.get(&c.holder).is_some_and(|h| h.state.active())).map(|c| c.holder_name.clone())
    }

    /// Conflicts that are still live (both agents active), for the badge.
    pub fn live_conflicts(&self) -> Vec<&Conflict> {
        self.conflicts.iter().filter(|c| self.get(&c.holder).is_some_and(|a| a.state.active()) && self.get(&c.other).is_some_and(|a| a.state.active())).collect()
    }

    /// An agent that is not at work lets go of its conflicts.
    fn release(&mut self, key: &str) {
        let keep: Vec<Conflict> = self.conflicts.iter().filter(|c| c.holder != key && c.other != key).cloned().collect();
        self.conflicts = keep;
    }

    /// The runs that are really alive (their keys): every other agent that claims to be at work was cut off by a restart.
    pub fn reconcile(&mut self, live: &[String], now: i64) {
        let dead: Vec<String> = self.agents.iter().filter(|a| a.state.active() && !live.contains(&a.key)).map(|a| a.key.clone()).collect();
        for k in dead {
            self.set_state(&k, AState::Interrupted, "the window closed while it ran", now);
        }
    }

    /// Agents that are at work: (working, waiting for approval, blocked).
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.agents.iter().filter(|a| a.state == AState::Working).count(), self.agents.iter().filter(|a| a.state == AState::WaitingApproval).count(), self.agents.iter().filter(|a| a.state == AState::Blocked).count())
    }

    /// The active agent on a row, for the card: ("Agent 2", state).
    pub fn assignee(&self, row: &str) -> Option<(&str, AState)> {
        self.active().find(|a| a.rows.iter().any(|r| r == row)).map(|a| (a.name.as_str(), a.state))
    }
}

// ------------------------------------------------------------------------------------------------ the file

pub fn path_of(root: &Path) -> PathBuf {
    root.join(".iyakki/board-assignments.json")
}

fn to_json(d: &Doc) -> Value {
    json!({
        "version": 1,
        "agents": d.agents.iter().map(|a| json!({
            "key": a.key, "name": a.name, "custom_name": a.custom_name, "number": a.number, "engine": a.engine, "kind": a.kind, "rows": a.rows,
            "state": a.state.key(), "started": a.started, "updated": a.updated, "ended": a.ended, "tokens": a.tokens, "cost_cents": a.cost_cents,
            "notes": a.notes, "last": a.last, "run_ref": a.run_ref, "progress": a.progress, "run_id": a.run_id,
            "stop": a.stop_requested,
            "steps": a.steps.iter().map(|s| json!({"at": s.at, "kind": s.kind.key(), "text": s.text})).collect::<Vec<_>>(),
            "files": a.files.iter().map(|f| json!({"path": f.path, "adds": f.adds, "dels": f.dels})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "conflicts": d.conflicts.iter().map(|c| json!({"path": c.path, "holder": c.holder, "holder_name": c.holder_name, "other": c.other, "other_name": c.other_name, "choice": c.choice.key(), "at": c.at})).collect::<Vec<_>>(),
    })
}

fn from_json(v: &Value) -> Doc {
    let mut d = Doc::default();
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |o: &Value, k: &str| o.get(k).and_then(Value::as_i64).unwrap_or(0);
    for a in v.get("agents").and_then(Value::as_array).into_iter().flatten().take(MAX_KEPT * 4) {
        let key = s(a, "key");
        if key.is_empty() || d.get(&key).is_some() {
            continue;
        }
        let strs = |k: &str| a.get(k).and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
        d.agents.push(AgentRun {
            key,
            name: if s(a, "name").is_empty() { format!("Agent {}", n(a, "number").max(1)) } else { s(a, "name") },
            custom_name: a.get("custom_name").and_then(Value::as_bool).unwrap_or(false),
            number: n(a, "number").max(1) as u32,
            engine: s(a, "engine"),
            kind: s(a, "kind"),
            rows: strs("rows"),
            state: AState::parse(&s(a, "state")),
            started: n(a, "started"),
            updated: n(a, "updated"),
            ended: n(a, "ended"),
            steps: a.get("steps").and_then(Value::as_array).map(|l| l.iter().map(|x| Step { at: n(x, "at"), kind: StepKind::parse(&s(x, "kind")), text: clip(&s(x, "text"), 240) }).collect()).unwrap_or_default(),
            files: a.get("files").and_then(Value::as_array).map(|l| l.iter().map(|x| Touched { path: s(x, "path"), adds: n(x, "adds") as usize, dels: n(x, "dels") as usize }).collect()).unwrap_or_default(),
            tokens: n(a, "tokens") as u64,
            cost_cents: a.get("cost_cents").and_then(Value::as_f64).unwrap_or(0.0),
            notes: strs("notes"),
            last: s(a, "last"),
            run_ref: s(a, "run_ref"),
            progress: n(a, "progress").clamp(0, 100) as u32,
            run_id: s(a, "run_id"),
            stop_requested: a.get("stop").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    for c in v.get("conflicts").and_then(Value::as_array).into_iter().flatten().take(100) {
        d.conflicts.push(Conflict { path: s(c, "path"), holder: s(c, "holder"), holder_name: s(c, "holder_name"), other: s(c, "other"), other_name: s(c, "other_name"), choice: Choice::parse(&s(c, "choice")), at: n(c, "at") });
    }
    d
}

/// Read the file (a missing or unreadable one is an empty document).
pub fn load(root: &Path) -> Doc {
    std::fs::read_to_string(path_of(root)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| from_json(&v)).unwrap_or_default()
}

/// Read, change and write the file under one process-wide lock (several task threads report at once). Failing to write is not an error
/// of the agent's work: the state is simply not shown.
pub fn update<R>(root: &Path, f: impl FnOnce(&mut Doc) -> R) -> R {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut d = load(root);
    let r = f(&mut d);
    let p = path_of(root);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, serde_json::to_string(&to_json(&d)).unwrap_or_default()).is_ok() && std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// The step kind and a short text for a finished tool call (what the timeline shows).
pub fn classify(tool: &str, ok: bool, args: &Value, output: &str) -> (StepKind, String) {
    let short = |s: &str| clip(&s.split_whitespace().collect::<Vec<_>>().join(" "), 120);
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    match tool {
        "board_update" | "board_add" | "board_link_tests" => (StepKind::Board, format!("{tool} {}: {}", arg("id"), short(output))),
        "write_file" | "edit_file" | "apply_patch" => (StepKind::File, format!("{} {}", if ok { "wrote" } else { "failed to write" }, arg("path"))),
        "note_add" | "note_update" | "note_archive" => (StepKind::Note, short(output)),
        "run_command" => {
            let cmd = format!("{} {}", arg("program").to_string().trim(), args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default());
            let low = format!("{cmd} {output}").to_lowercase();
            if cmd.contains("test") || low.contains("test result") || low.contains("passed") && low.contains("failed") {
                (StepKind::Test, format!("{} -> {}", short(&cmd), short(output)))
            } else {
                (StepKind::Tool, format!("{} -> {}", short(&cmd), short(output)))
            }
        }
        _ => (StepKind::Tool, format!("{tool}{} {}", if ok { "" } else { " failed" }, short(output))),
    }
}

