//! `flows.*` over one account's flows engine. The engine has no event log of its own, so this adapter derives the contract's run events
//! (started, step, approval_needed, approval_answered, finished) by comparing the runs it sees with what it saw before: every call
//! first catches up, then answers. Events are numbered from the moment the adapter is made; runs that already existed are the
//! baseline and produce no events.
//!
//! Mapping of states: scheduled, queued, running, retrying and healing runs are `running`; a run that `needs you` (an approval, a
//! proposed fix, an interrupted step) is `waiting_approval` and carries the approval only when one is open; failed is `failed`;
//! succeeded is `succeeded`; cancelled is `stopped`. No approval a flow can ask today is `local_only` (the camera question is a
//! permission of its own, not an approval node), so every open approval can be answered by a remote client holding `flows:approve`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::contract::*;
use iyakki_flow::engine::StartRequest;
use iyakki_flow::model::{NodeState, Run, RunStatus as EngineStatus};

use super::{guarded, now_ms, who, Audit, CoreStreams};
use crate::flows::FlowEnv;

fn state_of(s: EngineStatus) -> RunState {
    match s {
        EngineStatus::Scheduled | EngineStatus::Queued | EngineStatus::Running | EngineStatus::Retrying | EngineStatus::Healing => RunState::Running,
        EngineStatus::Blocked => RunState::WaitingApproval,
        EngineStatus::Failed => RunState::Failed,
        EngineStatus::Succeeded => RunState::Succeeded,
        EngineStatus::Cancelled => RunState::Stopped,
    }
}

/// The approval node a run waits at, as the contract's question.
fn open_approval(r: &Run) -> Option<Approval> {
    r.nodes.iter().find(|(_, n)| n.state == NodeState::Waiting && n.waiting.starts_with("approval")).map(|(id, n)| Approval {
        id: id.clone(),
        text_key: "flows.approval".into(),
        text: n.waiting.splitn(3, ':').nth(2).unwrap_or("").to_string(),
        local_only: !n.permission.is_empty(),
    })
}

fn current_step(r: &Run) -> String {
    r.nodes.iter().find(|(_, n)| matches!(n.state, NodeState::Running | NodeState::Retrying | NodeState::Waiting)).map(|(id, _)| id.clone()).unwrap_or_default()
}

struct Seen {
    step: String,
    approval: Option<String>,
    finished: bool,
}

struct Inner {
    log: EventLog<FlowEvent>,
    seen: HashMap<String, Seen>,
    idem: HashMap<IdempotencyKey, (String, RunId)>,
}

pub struct CoreFlows {
    env: Arc<FlowEnv>,
    streams: Arc<CoreStreams>,
    inner: Mutex<Inner>,
    audit: Arc<Audit>,
}

impl CoreFlows {
    pub fn new(env: Arc<FlowEnv>, streams: Arc<CoreStreams>) -> CoreFlows {
        let f = CoreFlows { env, streams, inner: Mutex::new(Inner { log: EventLog::new(512), seen: HashMap::new(), idem: HashMap::new() }), audit: Arc::new(Audit::default()) };
        // the baseline: runs that exist now produce no events
        {
            let mut inner = f.inner.lock().unwrap_or_else(|e| e.into_inner());
            for s in f.env.engine.summaries() {
                let (step, approval) = f.env.engine.get_run(&s.id).map(|r| (current_step(&r), open_approval(&r).map(|a| a.id))).unwrap_or_default();
                inner.seen.insert(s.id.clone(), Seen { step, approval, finished: s.status.is_terminal() });
            }
        }
        f
    }

    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }

    /// Hand a camera frame (or any picture) to a run: the bytes wait behind a stream handle and a `frame` event announces it. The
    /// handle is readable with `flows:read`. (The camera flow nodes call this once they migrate; the camera consent itself stays local.)
    pub fn hand_frame(&self, run: &RunId, media_type: &str, bytes: Vec<u8>) -> CallResult<StreamHandle> {
        let h = self.streams.put(StreamKind::CameraFrame, media_type, Scope::FLOWS_READ, bytes)?;
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).log.push("frame", now_ms(), FlowEvent::Frame { run: run.clone(), stream: h.clone() });
        Ok(h)
    }

    fn catch_up(&self, inner: &mut Inner) {
        let mut sums = self.env.engine.summaries();
        sums.reverse(); // oldest first
        for s in sums {
            let id = RunId(s.id.clone());
            let known = inner.seen.contains_key(&s.id);
            if !known {
                inner.log.push("started", now_ms(), FlowEvent::Started { run: id.clone(), flow: s.flow_id.clone() });
                inner.seen.insert(s.id.clone(), Seen { step: String::new(), approval: None, finished: false });
            } else if inner.seen[&s.id].finished {
                continue;
            }
            let Some(run) = self.env.engine.get_run(&s.id) else { continue };
            let (step, open) = (current_step(&run), open_approval(&run));
            let seen = inner.seen.get_mut(&s.id).expect("inserted above");
            let mut push: Vec<(&str, FlowEvent)> = Vec::new();
            if !step.is_empty() && step != seen.step {
                push.push(("step", FlowEvent::Step { run: id.clone(), name: step.clone() }));
            }
            seen.step = step;
            if let Some(a) = &open {
                if seen.approval.as_deref() != Some(a.id.as_str()) {
                    seen.approval = Some(a.id.clone());
                    push.push(("approval_needed", FlowEvent::ApprovalNeeded { run: id.clone(), approval: a.clone() }));
                }
            } else if let Some(node) = seen.approval.take() {
                let out = run.nodes.get(&node).map(|n| n.output.clone()).unwrap_or_default();
                let allowed = out.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);
                let by = out.get("by").and_then(|v| v.as_str()).unwrap_or("").to_string();
                push.push(("approval_answered", FlowEvent::ApprovalAnswered { run: id.clone(), approval_id: node, allowed, by }));
            }
            if run.status.is_terminal() {
                seen.finished = true;
                push.push(("finished", FlowEvent::Finished { run: id.clone(), state: state_of(run.status) }));
            }
            for (topic, ev) in push {
                inner.log.push(topic, now_ms(), ev);
            }
        }
    }

    fn status_of(&self, run: &RunId) -> CallResult<RunStatus> {
        let r = self.env.engine.get_run(&run.0).ok_or_else(|| CallError::new(ErrorCode::NotFound, "flows.unknown_run", "No such run."))?;
        Ok(RunStatus { run: run.clone(), flow: r.flow_id.clone(), state: state_of(r.status), pending_approval: open_approval(&r), step: current_step(&r) })
    }

    /// Only the person who owns the engine may use it locally (a remote call is checked by scope and acts on this account's flows).
    fn own(&self, ctx: &CallContext) -> CallResult<()> {
        if ctx.is_local() && ctx.identity.user != self.env.user {
            return Err(CallError::new(ErrorCode::Denied, "flows.not_yours", "These flows belong to another account."));
        }
        Ok(())
    }
}

impl Flows for CoreFlows {
    fn list(&self, ctx: &CallContext) -> CallResult<Vec<FlowInfo>> {
        guarded(&self.audit, ctx, "flows.list", Scope::FLOWS_READ, Risk::Read)?;
        self.own(ctx)?;
        Ok(self.env.summaries().into_iter().map(|s| FlowInfo { id: s.id, name: s.name, description: s.description }).collect())
    }

    fn start(&self, ctx: &CallContext, flow: &str, idem: &IdempotencyKey) -> CallResult<RunId> {
        guarded(&self.audit, ctx, "flows.start", Scope::FLOWS_RUN, Risk::Change)?;
        self.own(ctx)?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((f, run)) = inner.idem.get(idem) {
            if f == flow {
                self.audit.line(ctx, &format!("flows.start {flow}"), Risk::Change, "repeat: same run");
                return Ok(run.clone());
            }
            return Err(CallError::new(ErrorCode::Conflict, "core.idempotency_conflict", "That key was already used for a different request."));
        }
        self.catch_up(&mut inner);
        let req = StartRequest::new(flow).trigger("contract", &who(ctx)).key(idem.as_str());
        let out = self.env.engine.start_run(req).map_err(|m| {
            let (code, key) = if !self.env.store.exists(flow) { (ErrorCode::NotFound, "flows.unknown_flow") } else { (ErrorCode::InvalidInput, "flows.cannot_start") };
            self.audit.line(ctx, &format!("flows.start {flow}"), Risk::Change, &format!("refused: {}", code.as_str()));
            CallError::new(code, key, &m)
        })?;
        let run = RunId(out.run_id);
        inner.idem.insert(idem.clone(), (flow.to_string(), run.clone()));
        self.catch_up(&mut inner);
        self.audit.line(ctx, &format!("flows.start {flow}"), Risk::Change, if out.existing { "repeat: same run" } else { "ok" });
        Ok(run)
    }

    fn stop(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus> {
        guarded(&self.audit, ctx, "flows.stop", Scope::FLOWS_RUN, Risk::Change)?;
        self.own(ctx)?;
        let st = self.status_of(run)?;
        if !st.state.is_finished() {
            // a run that finished a moment ago is "already done", which is harmless
            let _ = self.env.engine.cancel(&run.0);
            self.env.engine.wait_for(&run.0, Duration::from_secs(5), |r| r.status.is_terminal());
        }
        self.catch_up(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()));
        self.audit.line(ctx, "flows.stop", Risk::Change, "ok");
        self.status_of(run)
    }

    fn status(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus> {
        guarded(&self.audit, ctx, "flows.status", Scope::FLOWS_READ, Risk::Read)?;
        self.own(ctx)?;
        // every read catches the event log up, so a state seen here is never missing from the events
        self.catch_up(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()));
        self.status_of(run)
    }

    fn events(&self, ctx: &CallContext, run: Option<&RunId>, resume: Resume, limit: usize) -> CallResult<EventBatch<FlowEvent>> {
        guarded(&self.audit, ctx, "flows.events", Scope::FLOWS_READ, Risk::Read)?;
        self.own(ctx)?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.catch_up(&mut inner);
        let mut b = inner.log.read(resume, None, limit);
        if let Some(r) = run {
            b.events.retain(|e| run_of(&e.body) == r);
        }
        Ok(b)
    }

    fn approve(&self, ctx: &CallContext, run: &RunId, approval_id: &str, allow: bool) -> CallResult<RunStatus> {
        guarded(&self.audit, ctx, "flows.approve", Scope::FLOWS_APPROVE, Risk::AskPerson)?;
        self.own(ctx)?;
        let st = self.status_of(run)?;
        let Some(a) = st.pending_approval.filter(|a| a.id == approval_id) else {
            return Err(CallError::new(ErrorCode::NotFound, "flows.no_such_approval", "That question is not waiting."));
        };
        if a.local_only && !ctx.is_local() {
            self.audit.line(ctx, "flows.approve", Risk::AskPerson, "refused: local_only");
            return Err(CallError::new(ErrorCode::NeedsConsent, "flows.approval_local_only", "Only a person at the machine can answer this question.").hint("Ask the person at the machine."));
        }
        self.env.engine.decide(&run.0, &a.id, allow, &who(ctx), "").map_err(|m| CallError::new(ErrorCode::NotFound, "flows.no_such_approval", &m))?;
        let node = a.id.clone();
        self.env.engine.wait_for(&run.0, Duration::from_secs(5), |r| r.nodes.get(&node).is_none_or(|n| n.state != NodeState::Waiting));
        self.catch_up(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()));
        self.audit.line(ctx, "flows.approve", Risk::AskPerson, if allow { "allowed" } else { "denied" });
        self.status_of(run)
    }
}

fn run_of(e: &FlowEvent) -> &RunId {
    match e {
        FlowEvent::Started { run, .. } | FlowEvent::Step { run, .. } | FlowEvent::ApprovalNeeded { run, .. } | FlowEvent::ApprovalAnswered { run, .. } | FlowEvent::Frame { run, .. } | FlowEvent::Finished { run, .. } => run,
    }
}
