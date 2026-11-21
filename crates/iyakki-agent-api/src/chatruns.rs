//! Chat runs as one core-side object (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md section 1).
//!
//! A chat run, its tool chain and its approvals belong to the core: the UI sends a [`RunRequest`] (plain data), polls the events of the run and answers
//! an approval card. Nothing that cannot cross a socket (`Arc<dyn Tools>`, `Arc<dyn ChatBackend>`, closures) is in the interface; the core builds the
//! tool chain itself through a [`RunStarter`] it was given when it was wired. [`ChatRunTable`] is the object: it keeps the runs of every person apart
//! (a run belongs to the user who started it; another user's calls on its id are refused), and [`FakeRunStarter`] drives it in tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use iyakki_shared::agent::{RunEvent, Verdict};
use iyakki_shared::chat::Message;
use serde::{Deserialize, Serialize};

use crate::facets_b4::ChatRun;

/// What a UI asks for when it starts a run: data only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunRequest {
    pub user: String,
    pub prompt: String,
    /// The chat session the run belongs to (empty: none yet).
    pub session: String,
    /// Earlier messages of the conversation.
    pub history: Vec<Message>,
    /// The open project `(name, root)` of an AI IDE chat.
    pub project: Option<(String, String)>,
    /// The working folder of the run.
    pub workdir: Option<String>,
    /// An external coding agent chosen in the chat.
    pub ext: Option<String>,
    /// The model chosen with the external agent (none: the one of its settings).
    pub ext_model: Option<String>,
    /// Continue a paused Autopilot series.
    pub resume: bool,
    /// An Autopilot series toward the prompt as the goal.
    pub autopilot: bool,
    /// Approvals are answered by the run's own rules (Auto mode).
    pub auto_approve: bool,
    /// The states a run reads, given by the name the core handed out when the UI attached them (an empty name: none). The core resolves a name for the
    /// user who sent it only; the dashboard, the flow, the task list and the cluster of a workspace stay on their side of the socket until they move.
    pub dash: Option<String>,
    pub flows: Option<String>,
    pub tasks: Option<String>,
    pub cluster: Option<String>,
}

/// The core's way to build the tool chain and begin a run for a request (the real one is wired in `iyakki-compose`).
pub trait RunStarter: Send + Sync {
    fn start(&self, req: &RunRequest) -> Result<Box<dyn ChatRun>, String>;
}

/// The object the UI talks to.
pub trait ChatRuns: Send + Sync {
    /// Begin a run; the answer is the run's id.
    fn start(&self, req: RunRequest) -> Result<String, String>;
    /// New events since the last call (empty for an unknown run or another user's run).
    fn poll(&self, user: &str, run: &str) -> Vec<RunEvent>;
    fn is_done(&self, user: &str, run: &str) -> bool;
    /// Answer the approval card of tool call `call_id`.
    fn decide(&self, user: &str, run: &str, call_id: &str, verdict: Verdict) -> Result<(), String>;
    fn cancel(&self, user: &str, run: &str) -> Result<(), String>;
    fn pause(&self, user: &str, run: &str) -> Result<(), String>;
    /// Drop a finished run's state (the UI calls it after it drew the last event).
    fn forget(&self, user: &str, run: &str);
}

struct Slot {
    user: String,
    run: Box<dyn ChatRun>,
}

/// The runs of one core, by id.
pub struct ChatRunTable {
    starter: Arc<dyn RunStarter>,
    runs: Mutex<HashMap<String, Slot>>,
}

/// The most runs a core keeps at once (finished ones must be forgotten).
pub const MAX_RUNS: usize = 64;

impl ChatRunTable {
    pub fn new(starter: Arc<dyn RunStarter>) -> ChatRunTable {
        ChatRunTable { starter, runs: Mutex::new(HashMap::new()) }
    }

    fn with<T>(&self, user: &str, run: &str, f: impl FnOnce(&mut dyn ChatRun) -> T) -> Result<T, String> {
        let mut all = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        match all.get_mut(run) {
            Some(s) if s.user == user => Ok(f(s.run.as_mut())),
            _ => Err(format!("there is no run {run} for this user")),
        }
    }
}

impl ChatRuns for ChatRunTable {
    fn start(&self, req: RunRequest) -> Result<String, String> {
        if req.user.is_empty() {
            return Err("a run needs a user".into());
        }
        if self.runs.lock().unwrap_or_else(PoisonError::into_inner).len() >= MAX_RUNS {
            return Err("too many runs are open; forget the finished ones".into());
        }
        let run = self.starter.start(&req)?;
        let id = run.id();
        self.runs.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), Slot { user: req.user, run });
        Ok(id)
    }

    fn poll(&self, user: &str, run: &str) -> Vec<RunEvent> {
        self.with(user, run, |r| r.poll()).unwrap_or_default()
    }

    fn is_done(&self, user: &str, run: &str) -> bool {
        self.with(user, run, |r| r.is_done()).unwrap_or(true)
    }

    fn decide(&self, user: &str, run: &str, call_id: &str, verdict: Verdict) -> Result<(), String> {
        self.with(user, run, |r| r.decide(call_id, verdict))
    }

    fn cancel(&self, user: &str, run: &str) -> Result<(), String> {
        self.with(user, run, |r| r.cancel())
    }

    fn pause(&self, user: &str, run: &str) -> Result<(), String> {
        self.with(user, run, |r| r.pause())
    }

    fn forget(&self, user: &str, run: &str) {
        let mut all = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        if all.get(run).is_some_and(|s| s.user == user) {
            all.remove(run);
        }
    }
}

/// A starter for tests: each run replays the events it was given and records the verdicts.
pub struct FakeRunStarter {
    pub events: Vec<RunEvent>,
    pub decided: Arc<Mutex<Vec<(String, Verdict)>>>,
    next: Mutex<u32>,
    pub requests: Mutex<Vec<RunRequest>>,
}

impl FakeRunStarter {
    pub fn new(events: Vec<RunEvent>) -> FakeRunStarter {
        FakeRunStarter { events, decided: Arc::default(), next: Mutex::new(0), requests: Mutex::new(Vec::new()) }
    }
}

struct FakeRun {
    id: String,
    left: Vec<RunEvent>,
    decided: Arc<Mutex<Vec<(String, Verdict)>>>,
    cancelled: bool,
}

impl ChatRun for FakeRun {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn is_done(&self) -> bool {
        self.cancelled || self.left.is_empty()
    }
    fn poll(&mut self) -> Vec<RunEvent> {
        std::mem::take(&mut self.left)
    }
    fn decide(&mut self, call_id: &str, v: Verdict) {
        self.decided.lock().unwrap_or_else(PoisonError::into_inner).push((call_id.to_string(), v));
    }
    fn cancel(&self) {}
    fn pause(&self) {}
    fn in_session(self: Box<Self>, _session: &str) -> Box<dyn ChatRun> {
        self
    }
}

impl RunStarter for FakeRunStarter {
    fn start(&self, req: &RunRequest) -> Result<Box<dyn ChatRun>, String> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(req.clone());
        let mut n = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        *n += 1;
        Ok(Box::new(FakeRun { id: format!("run-{n}"), left: self.events.clone(), decided: self.decided.clone(), cancelled: false }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(user: &str) -> RunRequest {
        RunRequest { user: user.into(), prompt: "hello".into(), ..Default::default() }
    }

    #[test]
    fn a_run_is_started_polled_decided_and_forgotten_and_belongs_to_its_user() {
        let events = vec![RunEvent::Model { text: "hi".into(), calls: vec![] }];
        let starter = Arc::new(FakeRunStarter::new(events.clone()));
        let t = ChatRunTable::new(starter.clone());
        let id = t.start(req("ann")).unwrap();
        assert!(t.poll("bob", &id).is_empty(), "another user sees nothing");
        assert!(t.decide("bob", &id, "c1", Verdict::Approve).is_err());
        assert_eq!(t.poll("ann", &id), events);
        assert!(t.is_done("ann", &id));
        t.decide("ann", &id, "c1", Verdict::Session).unwrap();
        assert_eq!(*starter.decided.lock().unwrap(), vec![("c1".to_string(), Verdict::Session)]);
        t.forget("bob", &id);
        assert!(t.cancel("ann", &id).is_ok(), "bob could not forget it");
        t.forget("ann", &id);
        assert!(t.cancel("ann", &id).is_err());
        assert!(t.is_done("ann", &id), "an unknown run counts as done");
    }

    #[test]
    fn the_table_is_bounded_and_refuses_an_empty_user() {
        let t = ChatRunTable::new(Arc::new(FakeRunStarter::new(vec![])));
        assert!(t.start(req("")).is_err());
        for _ in 0..MAX_RUNS {
            t.start(req("ann")).unwrap();
        }
        assert!(t.start(req("ann")).is_err());
    }

    #[test]
    fn the_request_and_verdict_cross_the_wire_as_json() {
        let r = RunRequest { project: Some(("shop".into(), "/w/shop".into())), autopilot: true, ..req("ann") };
        let back: RunRequest = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        assert_eq!(serde_json::to_string(&Verdict::Always).unwrap(), "\"Always\"");
    }
}
