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

pub struct RunManager {
    deps: Arc<Deps>,
    approval_timeout: Duration,
    states: Mutex<HashMap<String, Arc<Mutex<RunState>>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The event as the API shows it; `None` for what is for a screen only (partial text, thinking, timings).
fn shown(e: &RunEvent) -> Option<Value> {
    match e {
        RunEvent::Delta { .. } | RunEvent::Thinking { .. } | RunEvent::Budget { .. } | RunEvent::Timing { .. } => None,
        RunEvent::ToolResult { id, name, ok, text } => Some(json!({"type": "tool_result", "id": id, "name": name, "ok": ok, "text": text.chars().take(MAX_RESULT_TEXT).collect::<String>()})),
        other => serde_json::to_value(other).ok(),
    }
}

impl RunManager {
    pub fn new(deps: Arc<Deps>, approval_timeout: Duration) -> RunManager {
        RunManager { deps, approval_timeout, states: Mutex::new(HashMap::new()) }
    }

    pub fn going(&self) -> usize {
        lock(&self.states).values().filter(|s| !lock(s).done).count()
    }

    pub fn start(&self, p: &Principal, prompt: &str, model: Option<(&str, &str)>, allowed: BTreeSet<String>, max_concurrent: u32) -> ApiResult<String> {
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
        drop(states);
        let spawned = std::thread::Builder::new().name("iyakki-svc-run".into()).spawn(move || {
            while !session.is_done() {
                for e in session.poll() {
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
                std::thread::sleep(Duration::from_millis(40));
            }
            lock(&state).done = true;
        });
        if spawned.is_err() {
            return Err(ApiError::Busy("the service cannot start another run now".into()));
        }
        Ok(id)
    }

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
