//! Chat runs for the service: start (with a per-person cap), a worker thread that drains the run's events into a buffer the client
//! polls, stop, and the person's history. Approvals are not decided here (see the handler); the run loop itself asks, waits, and
//! refuses on timeout exactly as in the desktop chat.

use super::deps::Deps;
use super::tools::ProfileTools;
use crate::agent::{RunEvent, RunLimits};
use crate::chat::{ChatBackend, Message};
use crate::runs::RunSession;
use iyakki_core_api::{ApiError, ApiResult, Principal};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_EVENTS: usize = 2000;
const MAX_PROMPT: usize = 32 * 1024;
const MAX_RESULT_TEXT: usize = 8000;
const MAX_STATES: usize = 128;

pub struct RunState {
    pub user: String,
    pub prompt: String,
    pub started: u64,
    pub events: Vec<Value>,
    pub status: String,
    pub done: bool,
}

/// Tells the person's event stream that a run moved: `{type: run, data: {id, status, events}}` (the kind and the counts only).
pub type Notify = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct RunManager {
    deps: Arc<Deps>,
    approval_timeout: Duration,
    states: Arc<Mutex<HashMap<String, Arc<Mutex<RunState>>>>>,
    /// Running totals for `/metrics` that outlive the runs (a run is forgotten, a counter is not): tool calls by outcome and model tokens.
    totals: Arc<Totals>,
}

/// Counters kept since the service started (F-WEBSVC-10).
#[derive(Default)]
pub(super) struct Totals {
    pub tool_ok: std::sync::atomic::AtomicU64,
    pub tool_failed: std::sync::atomic::AtomicU64,
    pub tokens: std::sync::atomic::AtomicU64,
}

/// What reading a run needs, without the power to start or stop one: the saved runs and the live ones.
#[derive(Clone)]
pub struct Core {
    deps: Arc<Deps>,
    states: Arc<Mutex<HashMap<String, Arc<Mutex<RunState>>>>>,
}

impl Core {
    fn state(&self, p: &Principal, id: &str) -> Option<Arc<Mutex<RunState>>> {
        let s = lock(&self.states).get(id).cloned()?;
        let mine = lock(&s).user == p.user;
        mine.then_some(s)
    }

    /// Status and events after `after`; from memory while it is known, else from the saved record. Another person's run
    /// is "not found".
    pub fn view(&self, p: &Principal, id: &str, after: usize) -> ApiResult<(String, Vec<Value>, usize, bool)> {
        if let Some(s) = self.state(p, id) {
            let s = lock(&s);
            let ev: Vec<Value> = s.events.iter().skip(after).cloned().collect();
            return Ok((s.status.clone(), ev, s.events.len(), s.done));
        }
        let (log, _) = self.deps.store.load(&p.user, id).map_err(|_| ApiError::NotFound("no such run".into()))?;
        let all: Vec<Value> = log.events.iter().filter_map(shown).collect();
        let n = all.len();
        Ok((log.status.clone(), all.into_iter().skip(after).collect(), n, log.status != "running"))
    }

    pub fn list(&self, p: &Principal) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        let mut seen = BTreeSet::new();
        for (id, s) in lock(&self.states).iter() {
            let s = lock(s);
            if s.user == p.user {
                seen.insert(id.clone());
                out.push(json!({"id": id, "started": s.started, "prompt": s.prompt, "status": s.status}));
            }
        }
        for m in self.deps.store.list(&p.user) {
            if seen.insert(m.id.clone()) {
                out.push(json!({"id": m.id, "started": m.started, "prompt": m.prompt.chars().take(100).collect::<String>(), "status": m.status, "tool_calls": m.tool_calls}));
            }
        }
        out.sort_by(|a, b| b["started"].as_u64().cmp(&a["started"].as_u64()));
        out.truncate(200);
        out
    }
}

/// One person's read-only window on their runs (what a tile in the browser is given): their own runs only.
#[derive(Clone)]
pub struct RunReader {
    core: Core,
    who: Principal,
}

impl RunReader {
    pub fn list(&self) -> Vec<Value> {
        self.core.list(&self.who)
    }
    pub fn view(&self, id: &str, after: usize) -> ApiResult<(String, Vec<Value>, usize, bool)> {
        self.core.view(&self.who, id, after)
    }
}

impl RunManager {
    fn core(&self) -> Core {
        Core { deps: self.deps.clone(), states: self.states.clone() }
    }

    /// A reader for one person (their runs only).
    pub fn reader(&self, p: &Principal) -> RunReader {
        RunReader { core: self.core(), who: p.clone() }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The event as the API shows it; `None` for what is for a screen only (partial text, thinking, timings).
/// Counts one event into the running totals: a tool result by outcome, and the tokens a run has used since its last report (the
/// `Usage` event carries the run's total so far, so only the growth is added).
pub(super) fn count_event(t: &Totals, last_tokens: &mut u64, e: &RunEvent) {
    use std::sync::atomic::Ordering::Relaxed;
    match e {
        RunEvent::ToolResult { ok: true, .. } => {
            t.tool_ok.fetch_add(1, Relaxed);
        }
        RunEvent::ToolResult { ok: false, .. } => {
            t.tool_failed.fetch_add(1, Relaxed);
        }
        RunEvent::Usage { tokens, .. } => {
            t.tokens.fetch_add(tokens.saturating_sub(*last_tokens), Relaxed);
            *last_tokens = (*last_tokens).max(*tokens);
        }
        _ => {}
    }
}

fn shown(e: &RunEvent) -> Option<Value> {
    match e {
        RunEvent::Delta { .. } | RunEvent::Thinking { .. } | RunEvent::Budget { .. } | RunEvent::Timing { .. } => None,
        RunEvent::ToolResult { id, name, ok, text } => Some(json!({"type": "tool_result", "id": id, "name": name, "ok": ok, "text": text.chars().take(MAX_RESULT_TEXT).collect::<String>()})),
        other => serde_json::to_value(other).ok(),
    }
}

impl RunManager {
    /// How many runs the service holds (for `/metrics`).
    pub fn count(&self) -> usize {
        self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }


    pub fn new(deps: Arc<Deps>, approval_timeout: Duration) -> RunManager {
        RunManager { deps, approval_timeout, states: Arc::new(Mutex::new(HashMap::new())), totals: Arc::new(Totals::default()) }
    }

    /// The runs held now by state, and the tool results they hold (ok, failed): for `/metrics`.
    pub fn census(&self) -> ([(&'static str, usize); 4], usize, usize) {
        let mut by = [("running", 0usize), ("completed", 0), ("failed", 0), ("stopped", 0)];
        let (mut ok, mut bad) = (0, 0);
        for st in lock(&self.states).values() {
            let s = lock(st);
            let name = if !s.done { "running" } else { s.status.as_str() };
            if let Some(slot) = by.iter_mut().find(|(n, _)| *n == name) {
                slot.1 += 1;
            }
            for e in &s.events {
                if e["type"] == "tool_result" {
                    if e["ok"] == true {
                        ok += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
        }
        (by, ok, bad)
    }

    /// Tool calls that came back (ok, failed) and the model tokens used, counted since the service started.
    pub fn totals(&self) -> (u64, u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.totals.tool_ok.load(Relaxed), self.totals.tool_failed.load(Relaxed), self.totals.tokens.load(Relaxed))
    }

    /// Is this run one the service itself started and is following (it tells its own progress to the person's stream)?
    pub fn holds(&self, id: &str) -> bool {
        lock(&self.states).contains_key(id)
    }

    pub fn going(&self) -> usize {
        lock(&self.states).values().filter(|s| !lock(s).done).count()
    }

    pub fn start(&self, p: &Principal, prompt: &str, model: Option<(&str, &str)>, allowed: BTreeSet<String>, max_concurrent: u32, notify: Option<Notify>) -> ApiResult<String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(ApiError::InvalidInput("the prompt is empty".into()));
        }
        if prompt.len() > MAX_PROMPT {
            return Err(ApiError::InvalidInput("the prompt is too long".into()));
        }
        let backend: Arc<dyn ChatBackend> = match model {
            Some((pr, m)) => self.deps.extras.backend_for(p, pr, m)?,
            None => self.deps.chat.clone(),
        };
        self.start_with_backend(p, prompt, backend, allowed, max_concurrent, notify)
    }

    /// As [`RunManager::start`] with the backend already chosen (a team's own model host or provider: `tenantcfg::team_backend`).
    pub fn start_with_backend(&self, p: &Principal, prompt: &str, backend: Arc<dyn ChatBackend>, allowed: BTreeSet<String>, max_concurrent: u32, notify: Option<Notify>) -> ApiResult<String> {
        let prompt = prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT {
            return Err(ApiError::InvalidInput("the prompt is empty or too long".into()));
        }
        let tools = Arc::new(ProfileTools::new(self.deps.tools.tools_for(p), allowed));
        let history = vec![Message::system(self.deps.system_prompt.clone()), Message::user(prompt)];
        // approval stays per call: `auto` is false and nothing here can switch it on
        let limits = RunLimits { approval_timeout: self.approval_timeout, auto: false, ..RunLimits::for_budget(&p.policy.budget, 0) };
        let mut states = lock(&self.states);
        if states.len() >= MAX_STATES {
            states.retain(|_, s| !lock(s).done);
        }
        let mine = states.values().filter(|s| {
            let s = lock(s);
            s.user == p.user && !s.done
        });
        if mine.count() >= max_concurrent as usize {
            return Err(ApiError::Busy(format!("you already have {max_concurrent} runs going; wait for one to finish or stop one")));
        }
        let mut session = RunSession::start(Some(self.deps.store.clone()), &p.user, prompt, backend, tools, history, limits);
        let id = session.id().to_string();
        let state = Arc::new(Mutex::new(RunState { user: p.user.clone(), prompt: prompt.chars().take(100).collect(), started: session.log().started, events: vec![], status: "running".into(), done: false }));
        states.insert(id.clone(), state.clone());
        let totals_for_thread = self.totals.clone();
        drop(states);
        let run_id = id.clone();
        let tell = move |status: &str, n: usize| {
            if let Some(f) = &notify {
                f(crate::notifyhub::events::event("run", json!({"id": run_id, "status": status, "events": n})));
            }
        };
        tell("running", 0);
        let spawned = std::thread::Builder::new().name("iyakki-svc-run".into()).spawn(move || {
            let mut last_told = Instant::now();
            let totals = totals_for_thread;
            let mut last_tokens = 0u64;
            while !session.is_done() {
                let mut moved = false;
                for e in session.poll() {
                    moved = true;
                    count_event(&totals, &mut last_tokens, &e);
                    let mut s = lock(&state);
                    let status = match &e {
                        RunEvent::Final { .. } => Some("completed"),
                        RunEvent::Failed { .. } => Some("failed"),
                        RunEvent::Stopped { .. } => Some("stopped"),
                        _ => None,
                    };
                    if let Some(v) = shown(&e) {
                        if s.events.len() < MAX_EVENTS || status.is_some() {
                            s.events.push(v);
                        }
                    }
                    if let Some(st) = status {
                        s.status = st.into();
                        s.done = true;
                    }
                }
                // progress is told at most twice a second; the end is always told
                let (status, n, done) = {
                    let s = lock(&state);
                    (s.status.clone(), s.events.len(), s.done)
                };
                if done || (moved && last_told.elapsed() >= Duration::from_millis(500)) {
                    tell(&status, n);
                    last_told = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            lock(&state).done = true;
            let (status, n) = {
                let s = lock(&state);
                (s.status.clone(), s.events.len())
            };
            tell(&status, n);
        });
        if spawned.is_err() {
            return Err(ApiError::Busy("the service cannot start another run now".into()));
        }
        Ok(id)
    }

    fn state(&self, p: &Principal, id: &str) -> Option<Arc<Mutex<RunState>>> {
        self.core().state(p, id)
    }

    /// Status and events after `after`; from memory while it is known, else from the saved record. Another person's run
    /// is "not found".
    pub fn view(&self, p: &Principal, id: &str, after: usize) -> ApiResult<(String, Vec<Value>, usize, bool)> {
        self.core().view(p, id, after)
    }

    /// Wait up to `wait` for something after `after`.
    pub fn events(&self, p: &Principal, id: &str, after: usize, wait: Duration) -> ApiResult<(String, Vec<Value>, usize, bool)> {
        let t = Instant::now();
        loop {
            let v = self.view(p, id, after)?;
            if !v.1.is_empty() || v.3 || t.elapsed() >= wait {
                return Ok(v);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Whether the run (the caller's) has already recorded an answer for this call.
    pub fn decided(&self, p: &Principal, id: &str, call: &str) -> bool {
        match self.view(p, id, 0) {
            Ok((_, events, _, _)) => events.iter().any(|e| e["type"] == "decision" && e["id"] == call),
            Err(_) => false,
        }
    }

    pub fn stop(&self, p: &Principal, id: &str) -> ApiResult<()> {
        if self.state(p, id).is_none() && self.deps.store.load(&p.user, id).is_err() {
            return Err(ApiError::NotFound("no such run".into()));
        }
        self.deps.store.stop_live(id);
        Ok(())
    }

    pub fn stop_all(&self) {
        self.deps.store.stop_all_live(None);
    }

    pub fn list(&self, p: &Principal) -> Vec<Value> {
        self.core().list(p)
    }
}
