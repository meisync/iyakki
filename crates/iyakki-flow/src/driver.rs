//! The work of one run: a driver thread that dispatches nodes to worker threads, applies what they report, waits for people and
//! for time, and runs the healing loop. One driver per active run; the run itself lives in the engine's table.

use std::collections::BTreeSet;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::engine::*;
use crate::graph;
use crate::heal::{self, ClassifyEnv, PatchOp};
use crate::host::{Cancel, Host, HealRequest, Notice, Redactor};
use crate::kinds::{NodeCx, NodeKind, NodeOutput, WaitFor};
use crate::model::*;
use crate::retry::{hash_parts, ErrClass, NodeError};

/// The first line of `s` cut at a word boundary to at most `n` characters, with an ellipsis when something was left out.
pub fn clip_words(s: &str, n: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= n && s.trim().lines().count() <= 1 {
        return line.to_string();
    }
    let cut: String = line.chars().take(n).collect();
    let cut = if line.chars().count() > n { cut.rsplit_once(char::is_whitespace).map_or(cut.as_str(), |x| x.0).to_string() } else { cut };
    format!("{}\u{2026}", cut.trim_end_matches([',', ';', ':', ' ', '.']))
}

pub(crate) const AGENT_WHO: &str = "agent:assistant";
/// The note of a card that only the person can settle (a consent or trust setting): the screen shows "Open flow settings" for it.
pub const CONSENT_NOTE: &str = "needs-your-permission";
/// The note of a card that waits for the person's choice (a vision model, say): the screen offers Settings -> Models and Refresh models.
pub const CHOICE_NOTE: &str = "needs-your-choice";

pub(crate) enum Final {
    Ok(NodeOutput),
    Failed { err: NodeError, class: ErrClass },
    Cancelled,
}

/// Fill in the templates of a node's configuration. Secrets only in the fields the kind names; each secret read is registered with
/// the redactor so it cannot appear in a log or an output.
pub(crate) fn resolve_config(host: &Arc<dyn Host>, red: &Redactor, kind: &dyn NodeKind, node: &Node, ctx: &Value) -> Result<Value, NodeError> {
    let run = crate::host::RunSecretCx::from_ctx(ctx);
    let secret = |name: &str| -> Result<String, String> {
        let v = host.secret_for_run(&run, name)?;
        red.add(&v);
        Ok(v)
    };
    let Value::Object(map) = &node.config else {
        return Ok(json!({}));
    };
    let mut out = serde_json::Map::new();
    for (k, v) in map {
        let r = if kind.raw_fields().contains(&k.as_str()) {
            v.clone()
        } else if kind.secret_fields().contains(&k.as_str()) {
            // a host that says "needs secret: ..." (a tenant that has not released the secret to this run) needs a person to fix it, not the
            // fixing agent: the same kind of stop as a missing consent
            crate::expr::resolve(v, ctx, Some(&secret)).map_err(|e| if e.0.contains("needs secret") { NodeError::consent(format!("`{k}`: {e}")) } else { NodeError::config(format!("`{k}`: {e}")) })?
        } else {
            crate::expr::resolve(v, ctx, None).map_err(|e| NodeError::config(format!("`{k}`: {e}")))?
        };
        out.insert(k.clone(), r);
    }
    Ok(Value::Object(out))
}

pub(crate) fn spawn(inner: &Arc<Inner>, id: &str) {
    let (tx, rx) = channel();
    {
        let mut g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        match g.get_mut(id) {
            Some(s) if s.tx.is_none() => {
                s.tx = Some(tx.clone());
                // counted as going from this moment, so the next `pump` does not start one more than the limit allows
                if s.run.status == RunStatus::Queued {
                    s.run.status = RunStatus::Running;
                }
            }
            _ => return,
        }
    }
    let inner2 = inner.clone();
    let id2 = id.to_string();
    let h = std::thread::Builder::new().name(format!("flow-run-{id}")).spawn(move || drive(inner2, id2, rx, tx, None));
    match h {
        Ok(h) => inner.threads.lock().unwrap_or_else(|e| e.into_inner()).push(h),
        Err(e) => {
            inner.update(id, |r| {
                r.status = RunStatus::Failed;
                r.error = format!("cannot start a thread: {e}");
            });
        }
    }
}

struct Driver {
    inner: Arc<Inner>,
    id: String,
    tx: Sender<Ctl>,
    in_flight: BTreeSet<String>,
    healing: BTreeSet<String>,
    dry: BTreeSet<String>,
    parent_cancel: Option<Cancel>,
    cancelled: bool,
    last_block: String,
}

pub(crate) fn drive(inner: Arc<Inner>, id: String, rx: Receiver<Ctl>, tx: Sender<Ctl>, parent_cancel: Option<Cancel>) {
    let mut d = Driver { inner: inner.clone(), id: id.clone(), tx, in_flight: BTreeSet::new(), healing: BTreeSet::new(), dry: BTreeSet::new(), parent_cancel, cancelled: false, last_block: String::new() };
    d.start();
    loop {
        if inner.halted() {
            return;
        }
        while let Ok(m) = rx.try_recv() {
            d.handle(m);
        }
        let own = inner.cancel_of(&id);
        if !d.cancelled && (own.is_cancelled() || d.parent_cancel.as_ref().is_some_and(Cancel::is_cancelled)) {
            own.cancel();
            d.cancelled = true;
            inner.log(&id, "", "cancelling");
        }
        if d.cancelled {
            if d.in_flight.is_empty() && d.healing.is_empty() && d.dry.is_empty() {
                d.finish_cancelled();
                return;
            }
            d.wait(&rx, false);
            continue;
        }
        d.timers();
        d.dispatch();
        d.refresh_status();
        if d.try_finalize() {
            return;
        }
        d.wait(&rx, true);
    }
}

impl Driver {
    fn start(&mut self) {
        let now = self.inner.now();
        self.inner.update(&self.id, |r| {
            if r.nodes.is_empty() {
                graph::init(r);
            }
            if r.started_ms == 0 {
                r.started_ms = now;
            }
            r.status = RunStatus::Running;
            graph::propagate(r, now);
        });
        self.inner.log(&self.id, "", "started");
    }

    fn run(&self) -> Option<Run> {
        self.inner.get(&self.id)
    }

    fn red(&self) -> Arc<Redactor> {
        self.inner.redactor(&self.id)
    }

    fn wait(&mut self, rx: &Receiver<Ctl>, timers: bool) {
        let now = self.inner.now();
        let next = if timers { self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).get(&self.id).and_then(|s| s.run.nodes.values().filter(|n| n.state == NodeState::Waiting).filter_map(|n| n.wake_ms).min()) } else { None };
        let idle = self.in_flight.is_empty() && self.healing.is_empty() && self.dry.is_empty();
        if idle && self.inner.clock.is_virtual() {
            if let Some(t) = next {
                // a virtual clock only moves when somebody sleeps: nothing else is going on, so move it to the wake-up
                self.inner.clock.sleep_ms((t - now).max(0) as u64, &Cancel::new());
                return;
            }
        }
        let ms = next.map_or(50, |t| ((t - now).max(0) as u64).min(50)).max(1);
        match rx.recv_timeout(Duration::from_millis(ms)) {
            Ok(m) => self.handle(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(5)),
        }
    }

    // ------------------------------------------------------------------------------------------ messages

    fn handle(&mut self, m: Ctl) {
        match m {
            Ctl::Done { node, result } => {
                self.in_flight.remove(&node);
                self.done(&node, result);
            }
            Ctl::Cancel | Ctl::Wake => {}
            Ctl::Decide { node, approved, by, note } => self.decide(&node, approved, &by, &note),
            Ctl::RetryNode { node } => self.retry_node(&node),
            Ctl::SkipNode { node, by } => self.skip_node(&node, &by),
            Ctl::RerunFrom { node } => self.rerun_from(&node),
            Ctl::Card { card, decision } => self.card(&card, decision),
            Ctl::Proposal { node, card, result } => {
                self.healing.remove(&node);
                self.proposal(&node, &card, result);
            }
            Ctl::DryRun { node, card, result } => {
                self.dry.remove(&node);
                self.dry_run_done(&node, &card, result);
            }
        }
    }

    fn kind_of(&self, run: &Run, node: &Node) -> Option<Arc<dyn NodeKind>> {
        let _ = run;
        self.inner.reg.get(&node.kind).cloned()
    }

    // ------------------------------------------------------------------------------------------ dispatch

    fn timers(&mut self) {
        let now = self.inner.now();
        let Some(run) = self.run() else { return };
        for (id, nr) in &run.nodes {
            if nr.state == NodeState::Waiting && nr.wake_ms.is_some_and(|t| t <= now) {
                if nr.waiting.starts_with("approval") {
                    let approve = nr.waiting.starts_with("approval:approve");
                    self.resume(id, json!({"approved": approve, "by": "timeout", "note": "", "timed_out": true}));
                } else {
                    self.resume(id, json!({"at_ms": nr.wake_ms}));
                }
            }
        }
    }

    fn resume(&mut self, node_id: &str, decision: Value) {
        let Some(run) = self.run() else { return };
        let Some(node) = run.flow.node(node_id).cloned() else { return };
        let Some(kind) = self.kind_of(&run, &node) else { return };
        let ctx = graph::ctx_for(&run, &graph::input_for(&run, node_id), &[]);
        let cancel = self.inner.cancel_of(&self.id);
        let red = self.red();
        let cx = NodeCx { run_id: &self.id, flow: &run.flow, node_id, attempt: 1, timeout_ms: 1000, ctx: &ctx, host: self.inner.host.as_ref(), cancel: &cancel, clock: self.inner.clock.as_ref(), redactor: &red, runner: RunnerKind::Local, sub: &NoSub2, logs: Mutex::new(vec![]) };
        let res = kind.resume(&cx, &node, &decision);
        match res {
            Ok(out) => self.complete(node_id, out),
            Err(e) => self.done(node_id, Final::Failed { err: e, class: ErrClass::Permanent }),
        }
    }

    fn dispatch(&mut self) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        if run.status.is_terminal() || !graph::failed_unrouted(&run).is_empty() {
            return;
        }
        let cap = run.flow.defaults.max_parallel.max(1).saturating_sub(self.in_flight.len());
        if cap == 0 {
            return;
        }
        let ready: Vec<String> = graph::ready(&run, &inner.reg).into_iter().take(cap).collect();
        if ready.is_empty() {
            return;
        }
        let latest = if ready.iter().any(|n| run.flow.node(n).is_some_and(|x| x.track != Track::Pinned)) { inner.source.get_published(&run.flow_id) } else { None };
        for node_id in ready {
            let now = inner.now();
            let reg = inner.reg.clone();
            let prepared = inner.update(&self.id, |r| {
                // a newer definition of this node, taken now because the node is about to begin
                let mut note = String::new();
                if let Some(pu) = r.pending_updates.remove(&node_id) {
                    let pos = r.flow.node(&node_id).map(|n| n.pos).unwrap_or_default();
                    if let Some(n) = r.flow.node_mut(&node_id) {
                        *n = pu.node;
                        n.pos = pos;
                    }
                    note = format!("took the update to v{} (asked by {}) at its start", pu.to_version, pu.by);
                } else if let (Some(l), Some(cur)) = (&latest, r.flow.node(&node_id)) {
                    if let Some(newn) = l.node(&node_id) {
                        if !cur.same_definition(newn) {
                            let take = match cur.track {
                                Track::Pinned => false,
                                Track::LatestPublished => true,
                                Track::Compatible => crate::store::compatible(cur, newn, &reg),
                            };
                            if take {
                                let pos = cur.pos;
                                note = format!("tracks the latest published version: took v{}", l.meta.version);
                                if let Some(n) = r.flow.node_mut(&node_id) {
                                    *n = newn.clone();
                                    n.pos = pos;
                                }
                            }
                        }
                    }
                }
                let node = r.flow.node(&node_id)?.clone();
                let input = graph::input_for(r, &node_id);
                let ctx = graph::ctx_for(r, &input, &[]);
                let attempts = r.flow.retry_of(&node).total_attempts();
                if let Some(nr) = r.nodes.get_mut(&node_id) {
                    nr.state = NodeState::Running;
                    nr.started_ms = now;
                    nr.ended_ms = 0;
                    nr.error.clear();
                    nr.max_attempts = attempts;
                    nr.input = cap_value(input.clone());
                    if !note.is_empty() {
                        nr.note = note.clone();
                    }
                }
                Some((node, ctx, r.flow.clone(), note))
            });
            let Some(Some((node, ctx, flow, note))) = prepared else { continue };
            if !note.is_empty() {
                inner.log(&self.id, &node_id, &note);
                inner.host.audit("flow.node.updated", &format!("{}/{node_id}: {note}", self.id));
            }
            // the redaction of the stored input
            let red = self.red();
            inner.with_run(&self.id, |r| {
                if let Some(nr) = r.nodes.get_mut(&node_id) {
                    nr.input = red.value(&nr.input);
                }
            });
            inner.persist(&self.id);
            self.in_flight.insert(node_id.clone());
            let (inner2, run_id, tx, cancel) = (inner.clone(), self.id.clone(), self.tx.clone(), inner.cancel_of(&self.id));
            let h = std::thread::Builder::new().name(format!("flow-node-{}-{node_id}", self.id)).spawn(move || {
                let nid = node.id.clone();
                let result = execute_node(&inner2, &run_id, &node, &flow, &ctx, &cancel, &tx);
                let _ = tx.send(Ctl::Done { node: nid, result });
            });
            match h {
                Ok(h) => inner.threads.lock().unwrap_or_else(|e| e.into_inner()).push(h),
                Err(e) => {
                    self.in_flight.remove(&node_id);
                    self.done(&node_id, Final::Failed { err: NodeError::new(format!("cannot start a worker: {e}")), class: ErrClass::Permanent });
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------ results

    fn done(&mut self, node_id: &str, result: Final) {
        let inner = self.inner.clone();
        match result {
            Final::Cancelled => {
                inner.update(&self.id, |r| {
                    if let Some(nr) = r.nodes.get_mut(node_id) {
                        nr.state = NodeState::Cancelled;
                    }
                });
            }
            Final::Ok(out) => match out.wait.clone() {
                Some(w) => self.wait_state(node_id, w),
                None => self.complete(node_id, out),
            },
            Final::Failed { err, class } => {
                let red = self.red();
                let msg = red.text(&err.message);
                let now = inner.now();
                let Some(run) = self.run() else { return };
                let Some(node) = run.flow.node(node_id).cloned() else { return };
                let routed = run.flow.outgoing(node_id).iter().any(|e| e.port == "error");
                inner.log(&self.id, node_id, &format!("failed ({}): {msg}", if class == ErrClass::Retryable { "after all attempts" } else { "not retryable" }));
                if routed {
                    inner.update(&self.id, |r| {
                        if let Some(nr) = r.nodes.get_mut(node_id) {
                            nr.state = NodeState::Failed;
                            nr.error = msg.clone();
                            nr.ended_ms = now;
                            nr.port = "error".into();
                            nr.output = json!({"error": msg});
                        }
                        graph::settle(r, node_id, now);
                    });
                } else if node.on_error == OnError::Continue {
                    inner.update(&self.id, |r| {
                        if let Some(nr) = r.nodes.get_mut(node_id) {
                            nr.state = NodeState::Succeeded;
                            nr.error = msg.clone();
                            nr.ended_ms = now;
                            nr.port = "out".into();
                            nr.output = json!({"error": msg, "continued": true});
                            nr.note = "failed, and the flow goes on (on_error: continue)".into();
                        }
                        graph::settle(r, node_id, now);
                    });
                } else {
                    inner.update(&self.id, |r| {
                        if let Some(nr) = r.nodes.get_mut(node_id) {
                            nr.state = NodeState::Failed;
                            nr.error = msg.clone();
                            nr.ended_ms = now;
                            nr.port.clear();
                            nr.needs_person = err.needs_person;
                            nr.permission = err.permission.clone();
                        }
                    });
                    if err.needs_person {
                        if !err.permission.is_empty() && err.permission != "choice" {
                            let (fname, ntitle) = (run.flow.name.clone(), node.title().to_string());
                            let blocked = run.flow.defaults.camera.blocked;
                            self.inner.host.permission_needed(&crate::host::PermissionNeed { flow_id: run.flow_id.clone(), flow_name: fname, run_id: self.id.clone(), node_id: node_id.to_string(), node_title: ntitle, permission: err.permission.clone(), message: msg.clone(), blocked });
                        }
                        self.consent_card(node_id, &msg, err.permission == "choice");
                    } else {
                        self.begin_heal(node_id, &msg);
                    }
                }
            }
        }
    }

    /// A refusal that only the person can lift (a consent or trust setting is off). The agent is not asked, no fix attempt or budget is
    /// spent and no change is proposed: the card says plainly what to switch on, and the screen offers to open the flow's settings.
    fn consent_card(&mut self, node_id: &str, msg: &str, choice: bool) {
        // with the agent's help off the run just fails, as for any other error (the failure card names the setting)
        if self.run().and_then(|r| r.flow.node(node_id).map(|n| r.flow.heal_of(n).mode)).is_none_or(|m| m == HealMode::Off) {
            return;
        }
        if choice {
            self.add_card(node_id, CardState::Proposed, format!("`{node_id}` needs your choice: {msg}"), CHOICE_NOTE);
            self.inner.host.audit("flow.heal.choice", &format!("{}/{node_id}: needs the person's choice, no agent fix tried", self.id));
            return;
        }
        self.add_card(node_id, CardState::Proposed, format!("`{node_id}` needs your permission: {msg}"), CONSENT_NOTE);
        self.inner.host.audit("flow.heal.consent", &format!("{}/{node_id}: needs the person, no agent fix tried", self.id));
        // the request itself (popup and inbox entry) is made by the host in `permission_needed`; no second note here
    }

    fn wait_state(&mut self, node_id: &str, w: WaitFor) {
        let inner = self.inner.clone();
        let now = inner.now();
        let Some(run) = self.run() else { return };
        let flow_name = run.flow.name.clone();
        match &w {
            WaitFor::Until { at_ms } => {
                inner.update(&self.id, |r| {
                    if let Some(nr) = r.nodes.get_mut(node_id) {
                        nr.state = NodeState::Waiting;
                        nr.wake_ms = Some(*at_ms);
                        nr.waiting = "until".into();
                    }
                });
            }
            WaitFor::Approval { message, timeout_s, on_timeout } => {
                let msg = self.red().text(message);
                inner.update(&self.id, |r| {
                    if let Some(nr) = r.nodes.get_mut(node_id) {
                        nr.state = NodeState::Waiting;
                        nr.wake_ms = timeout_s.map(|t| now + (t as i64) * 1000);
                        nr.waiting = format!("approval:{on_timeout}:{msg}");
                    }
                });
                inner.log(&self.id, node_id, &format!("waiting for approval: {msg}"));
                let _ = inner.host.notify(&Notice { title: format!("Approval needed: {flow_name}"), body: msg, level: "info".into(), flow_id: run.flow_id.clone(), run_id: self.id.clone(), needs_you: true });
            }
        }
    }

    fn decide(&mut self, node_id: &str, approved: bool, by: &str, note: &str) {
        let waiting = self.run().and_then(|r| r.nodes.get(node_id).map(|n| n.state == NodeState::Waiting && n.waiting.starts_with("approval"))).unwrap_or(false);
        if !waiting {
            return;
        }
        self.inner.log(&self.id, node_id, &format!("{} by {by}{}", if approved { "approved" } else { "rejected" }, if note.is_empty() { String::new() } else { format!(": {note}") }));
        self.resume(node_id, json!({"approved": approved, "by": by, "note": note, "timed_out": false}));
    }

    fn complete(&mut self, node_id: &str, out: NodeOutput) {
        let inner = self.inner.clone();
        let red = self.red();
        let now = inner.now();
        let value = cap_value(red.value(&out.value));
        let mut verified: Vec<(String, String)> = Vec::new();
        let run_before = self.run();
        inner.update(&self.id, |r| {
            let kind_end = r.flow.node(node_id).is_some_and(|n| n.kind == "end");
            if let Some(nr) = r.nodes.get_mut(node_id) {
                nr.state = NodeState::Succeeded;
                nr.output = value.clone();
                nr.port = out.port.clone();
                nr.ended_ms = now;
                nr.wake_ms = None;
                nr.waiting.clear();
                nr.artifacts = out.artifacts.clone();
            }
            if kind_end {
                if let Value::Object(m) = &value {
                    if !r.outputs.is_object() {
                        r.outputs = json!({});
                    }
                    for (k, v) in m {
                        r.outputs[k] = v.clone();
                    }
                }
            }
            for c in r.heal.iter_mut().filter(|c| c.node == node_id && c.state == CardState::Applied) {
                c.state = CardState::Verified;
                verified.push((c.id.clone(), c.patch.as_ref().map(|p| p.summary()).unwrap_or_default()));
            }
            graph::settle(r, node_id, now);
        });
        inner.log(&self.id, node_id, &format!("done ({})", out.port));
        for (cid, summary) in verified {
            inner.host.audit("flow.heal.verified", &format!("{}/{node_id} card {cid}: {summary}", self.id));
            inner.log(&self.id, node_id, &format!("the agent's fix worked: {summary}"));
            if let Some(r) = &run_before {
                let _ = inner.host.notify(&Notice { title: format!("Fixed: {} / {node_id}", r.flow.name), body: format!("{summary}. The change is a draft; publish it when you are happy with it."), level: "info".into(), flow_id: r.flow_id.clone(), run_id: self.id.clone(), needs_you: false });
            }
        }
    }

    fn retry_node(&mut self, node_id: &str) {
        let now = self.inner.now();
        self.inner.update(&self.id, |r| {
            if let Some(nr) = r.nodes.get_mut(node_id) {
                if matches!(nr.state, NodeState::Failed | NodeState::Interrupted | NodeState::Cancelled) {
                    let attempts = std::mem::take(&mut nr.attempts);
                    *nr = NodeRun { attempts, note: "retried by hand".into(), ..Default::default() };
                }
            }
            for e in r.flow.edges.iter().filter(|e| e.from == node_id) {
                r.edges.insert(e.key(), EdgeState::Open);
            }
            for c in r.heal.iter_mut().filter(|c| c.node == node_id && c.state == CardState::Proposed) {
                c.state = CardState::Rejected;
                c.note = "the step was retried by hand".into();
                c.decided_by = "user".into();
            }
            let _ = now;
        });
        self.inner.log(&self.id, node_id, "retry by hand");
    }

    fn skip_node(&mut self, node_id: &str, by: &str) {
        let now = self.inner.now();
        self.inner.update(&self.id, |r| {
            if let Some(nr) = r.nodes.get_mut(node_id) {
                if matches!(nr.state, NodeState::Failed | NodeState::Interrupted | NodeState::Waiting) {
                    nr.state = NodeState::Succeeded;
                    nr.output = json!({"skipped": true, "by": by});
                    nr.port = "out".into();
                    nr.ended_ms = now;
                    nr.wake_ms = None;
                    nr.waiting.clear();
                    nr.note = format!("skipped by {by}");
                }
            }
            for c in r.heal.iter_mut().filter(|c| c.node == node_id && c.state == CardState::Proposed) {
                c.state = CardState::Rejected;
                c.note = format!("the step was skipped by {by}");
                c.decided_by = by.to_string();
            }
            graph::settle(r, node_id, now);
        });
        self.inner.log(&self.id, node_id, &format!("skipped by {by}"));
    }

    fn rerun_from(&mut self, node_id: &str) {
        self.inner.update(&self.id, |r| {
            graph::reset_from(r, node_id);
            r.outputs = Value::Null;
            for c in r.heal.iter_mut().filter(|c| c.state == CardState::Proposed) {
                c.state = CardState::Rejected;
                c.note = "the run was started again from a step".into();
            }
        });
        self.inner.log(&self.id, node_id, "rerun from here");
    }

    // ------------------------------------------------------------------------------------------ status and the end

    fn refresh_status(&mut self) {
        let healing = !self.healing.is_empty() || !self.dry.is_empty();
        let mut notify: Option<(String, String)> = None;
        let last = self.last_block.clone();
        let res = self.inner.with_run(&self.id, |r| {
            if r.status.is_terminal() {
                return String::new();
            }
            let waiting: Vec<(&String, &NodeRun)> = r.nodes.iter().filter(|(_, n)| n.state == NodeState::Waiting && n.waiting.starts_with("approval")).collect();
            let interrupted: Vec<&String> = r.nodes.iter().filter(|(_, n)| n.state == NodeState::Interrupted).map(|(k, _)| k).collect();
            let cards = r.open_cards().into_iter().filter(|c| c.state == CardState::Proposed).count();
            let (status, why) = if let Some((id, nr)) = waiting.first() {
                (RunStatus::Blocked, format!("Approval needed at `{id}`: {}", nr.waiting.splitn(3, ':').nth(2).unwrap_or("")))
            } else if cards > 0 {
                let c = r.heal.iter().find(|c| c.state == CardState::Proposed).unwrap();
                (RunStatus::Blocked, match &c.patch {
                    Some(p) => format!("A fix is proposed for `{}`: {}", c.node, p.summary()),
                    None => format!("`{}` failed and needs you: {}", c.node, clip_words(&c.diagnosis, 120)),
                })
            } else if let Some(i) = interrupted.first() {
                (RunStatus::Blocked, format!("The program restarted while `{i}` was running: retry it, skip it or cancel the run"))
            } else if healing {
                (RunStatus::Healing, String::new())
            } else if r.nodes.values().any(|n| n.state == NodeState::Retrying) {
                (RunStatus::Retrying, String::new())
            } else {
                (RunStatus::Running, String::new())
            };
            r.status = status;
            r.blocked = why.clone();
            why
        });
        if let Some(why) = res {
            if !why.is_empty() && why != last {
                notify = Some((why.clone(), self.id.clone()));
            }
            self.last_block = why;
        }
        if let Some((why, id)) = notify {
            if let Some(r) = self.run() {
                if r.heal.iter().all(|c| c.state != CardState::Proposed) {
                    // an approval or an interruption: the card path sends its own note
                    let _ = self.inner.host.notify(&Notice { title: format!("{} needs you", r.flow.name), body: why, level: "info".into(), flow_id: r.flow_id.clone(), run_id: id, needs_you: true });
                }
            }
        }
        self.inner.persist(&self.id);
    }

    fn try_finalize(&mut self) -> bool {
        if !(self.in_flight.is_empty() && self.healing.is_empty() && self.dry.is_empty()) {
            return false;
        }
        let Some(run) = self.run() else { return true };
        if run.nodes.values().any(|n| matches!(n.state, NodeState::Waiting | NodeState::Interrupted)) || !run.open_cards().is_empty() {
            return false;
        }
        let failed = graph::failed_unrouted(&run);
        if failed.is_empty() && !graph::ready(&run, &self.inner.reg).is_empty() {
            return false;
        }
        if failed.is_empty() && run.nodes.values().any(|n| matches!(n.state, NodeState::Pending | NodeState::Running | NodeState::Retrying)) {
            // pending nodes that nothing will reach
            self.inner.with_run(&self.id, |r| graph::propagate(r, 0));
            let r2 = self.run().unwrap();
            if !graph::ready(&r2, &self.inner.reg).is_empty() {
                return false;
            }
        }
        let now = self.inner.now();
        let (status, error) = if let Some(f) = failed.first() {
            let msg = run.nodes.get(f).map(|n| n.error.clone()).unwrap_or_default();
            (RunStatus::Failed, format!("`{f}` failed: {msg}"))
        } else {
            (RunStatus::Succeeded, String::new())
        };
        self.finish(status, error, now);
        true
    }

    fn finish_cancelled(&mut self) {
        let now = self.inner.now();
        self.inner.update(&self.id, |r| {
            for n in r.nodes.values_mut() {
                if !n.state.is_done() {
                    n.state = NodeState::Cancelled;
                }
            }
        });
        self.finish(RunStatus::Cancelled, String::new(), now);
    }

    fn finish(&mut self, status: RunStatus, error: String, now: i64) {
        let inner = self.inner.clone();
        // the run's browser session (if its nodes opened one) ends with the run, whatever the way it ended, and before the run
        // is seen as finished
        inner.host.browser_end(&self.id);
        // the run's camera pictures are deleted with it (a node that asked to keep its picture keeps it)
        inner.host.cam_end(&self.id);
        // the failure notice goes out BEFORE the run reads as finished: whoever waits for the end (a caller, a test) then sees the notice already posted
        if status == RunStatus::Failed {
            if let Some(r) = self.run() {
                let _ = inner.host.notify(&Notice { title: format!("{} failed", r.flow.name), body: error.clone(), level: "error".into(), flow_id: r.flow_id.clone(), run_id: self.id.clone(), needs_you: false });
            }
        }
        inner.update(&self.id, |r| {
            r.status = status;
            r.ended_ms = now;
            r.error = error.clone();
            r.blocked.clear();
            if status == RunStatus::Succeeded && r.outputs.is_null() {
                r.outputs = json!({});
            }
            for n in r.nodes.values_mut() {
                if n.state == NodeState::Pending {
                    n.state = NodeState::Skipped;
                }
            }
        });
        inner.state.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&self.id).map(|s| s.tx = None);
        inner.log(&self.id, "", &format!("{}{}", status.label(), if error.is_empty() { String::new() } else { format!(": {error}") }));
        inner.host.audit("flow.run.end", &format!("{} {}", self.id, status.label()));
        let run = self.run();
        if let Some(r) = &run {
            if status == RunStatus::Failed {
                if r.parent.is_none() {
                    fire_on_failure(&inner, r);
                }
            }
            if r.parent.is_none() {
                let keep = r.flow.defaults.keep_runs;
                let sums = inner.summaries();
                for gone in inner.store.prune(&sums.into_iter().filter(|s| s.flow_id == r.flow_id).collect::<Vec<_>>(), keep) {
                    inner.state.lock().unwrap_or_else(|e| e.into_inner()).remove(&gone);
                }
            }
        }
        if run.as_ref().is_some_and(|r| r.parent.is_none()) {
            Engine { inner: inner.clone() }.pump();
        }
    }

    // ------------------------------------------------------------------------------------------ healing

    fn add_card(&mut self, node_id: &str, state: CardState, diagnosis: String, note: &str) -> String {
        let now = self.inner.now();
        let diag = self.red().text(&diagnosis);
        self.inner
            .update(&self.id, |r| {
                let id = format!("c{}", r.heal.len() + 1);
                r.heal.push(HealCard { id: id.clone(), node: node_id.to_string(), state, diagnosis: diag, patch: None, class: None, created_ms: now, decided_by: String::new(), note: note.to_string(), draft_rev: 0, cost_cents: 0 });
                id
            })
            .unwrap_or_default()
    }

    /// The classifier's verdict, plus what only the engine knows: a step setting that the kind does not have is refused (so a made-up key
    /// such as `camera_allowed` is never written into a step), and so is any change to a consent or trust setting.
    fn classify_checked(&self, patch: &heal::Patch, flow: &Flow, failing: &str) -> heal::Classification {
        let inner = &self.inner;
        let mut cl = heal::classify(patch, flow, failing, &ClassifyEnv { in_catalog: &|c| inner.host.cli_known(c) });
        let node = flow.node(&patch.node);
        let keys: Option<Vec<&'static str>> = node.and_then(|n| inner.reg.get(&n.kind)).map(|k| k.schema().iter().map(|f| f.key).collect());
        if let Some(keys) = keys {
            for (path, top) in heal::set_settings(patch) {
                if !top.is_empty() && !keys.contains(&top.as_str()) {
                    cl.class = heal::Class::Refused;
                    cl.reasons.push(format!("`{path}` is not a setting of this kind of step"));
                }
            }
        }
        cl
    }

    fn begin_heal(&mut self, node_id: &str, msg: &str) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        let Some(node) = run.flow.node(node_id).cloned() else { return };
        let pol = run.flow.heal_of(&node);
        if pol.mode == HealMode::Off {
            return;
        }
        let today = inner.heal_today(&run.flow_id, inner.now());
        if let Err(why) = heal::budget_check(&pol, run.heal_spent, run.heal_cents, today) {
            let diag = format!("I will not try more fixes on my own: {why}. The last error was: {msg}");
            self.add_card(node_id, CardState::Proposed, diag, "budget");
            inner.host.audit("flow.heal.budget", &format!("{}/{node_id}: {why}", self.id));
            self.evidence(node_id);
            self.notify_card(node_id, &format!("The agent stopped trying: {why}"));
            return;
        }
        if pol.mode == HealMode::AlwaysAsk {
            self.add_card(node_id, CardState::Proposed, format!("`{node_id}` failed: {msg}. Do you want the agent to look at it?"), "investigate");
            self.notify_card(node_id, "A step failed; ask the agent to look at it?");
            return;
        }
        let card = self.add_card(node_id, CardState::Investigating, format!("The agent is looking at `{node_id}`: {msg}"), "");
        self.spawn_proposal(node_id, &card);
    }

    fn notify_card(&self, node_id: &str, body: &str) {
        if let Some(r) = self.run() {
            let _ = self.inner.host.notify(&Notice { title: format!("{} / {node_id} needs you", r.flow.name), body: body.to_string(), level: "warn".into(), flow_id: r.flow_id.clone(), run_id: self.id.clone(), needs_you: true });
        }
    }

    fn spawn_proposal(&mut self, node_id: &str, card: &str) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        let Some(node) = run.flow.node(node_id).cloned() else { return };
        let mut clis = Vec::new();
        if let Some(c) = node.config.get("cli").and_then(|c| c.get("id")).and_then(Value::as_str) {
            clis.push((c.to_string(), inner.host.cli_info(c).filter(|i| i.installed).map(|i| i.version).unwrap_or_default()));
        } else if node.kind == "grpc" {
            clis.push(("grpcurl".to_string(), inner.host.cli_info("grpcurl").filter(|i| i.installed).map(|i| i.version).unwrap_or_default()));
        }
        let pol = run.flow.heal_of(&node);
        let ctx = self.red().text(&heal::failure_context(&run, node_id, &clis));
        let req = HealRequest { run_id: self.id.clone(), flow_id: run.flow_id.clone(), node_id: node_id.to_string(), context: format!("{}\n\n{ctx}", heal::HEAL_INSTRUCTIONS), agent: String::new(), max_cents: pol.cost_cap_cents.saturating_sub(run.heal_cents) };
        self.healing.insert(node_id.to_string());
        let (tx, host, cancel, node_s, card_s) = (self.tx.clone(), inner.host.clone(), inner.cancel_of(&self.id), node_id.to_string(), card.to_string());
        let h = std::thread::Builder::new().name(format!("flow-heal-{}", self.id)).spawn(move || {
            let result = host.propose_patch(&req, &cancel);
            let _ = tx.send(Ctl::Proposal { node: node_s, card: card_s, result });
        });
        if let Ok(h) = h {
            inner.threads.lock().unwrap_or_else(|e| e.into_inner()).push(h);
        } else {
            self.healing.remove(node_id);
        }
    }

    fn proposal(&mut self, node_id: &str, card_id: &str, result: Result<crate::host::HealProposal, String>) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        let red = self.red();
        match result {
            Err(e) => {
                let e = red.text(&e);
                inner.update(&self.id, |r| {
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.state = CardState::Proposed;
                        c.diagnosis = format!("The agent could not look at this: {e}");
                        c.note = "no proposal".into();
                    }
                });
                inner.host.audit("flow.heal.diagnosis", &format!("{}/{node_id}: no proposal ({e})", self.id));
                self.evidence(node_id);
                self.notify_card(node_id, &format!("The agent could not help: {e}"));
            }
            Ok(p) => {
                let diag = red.text(&p.diagnosis);
                let patch = p.patch.clone();
                let class = patch.as_ref().map(|pt| self.classify_checked(pt, &run.flow, node_id));
                let mode = run.flow.node(node_id).map(|n| run.flow.heal_of(n).mode).unwrap_or(HealMode::Off);
                let auto = mode == HealMode::AutoFixSimple && class.as_ref().is_some_and(|c| c.is_simple()) && !run.flow.production && patch.is_some();
                inner.update(&self.id, |r| {
                    r.heal_cents += p.cents;
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.state = CardState::Proposed;
                        c.diagnosis = diag.clone();
                        c.patch = patch.clone();
                        c.class = class.clone();
                        c.cost_cents = p.cents;
                        if patch.is_some() {
                            c.note = "proposed by the assistant, not applied until you approve".into();
                        }
                    }
                });
                match (&patch, &class) {
                    (Some(pt), Some(cl)) => {
                        inner.host.audit("flow.heal.proposed", &format!("{}/{node_id} {} [{:?}{}]", self.id, pt.summary(), cl.class, if cl.reasons.is_empty() { String::new() } else { format!(": {}", cl.reasons.join("; ")) }));
                        inner.log(&self.id, node_id, &format!("the agent proposes: {} ({:?})", pt.summary(), cl.class));
                        if cl.class == heal::Class::Refused {
                            inner.update(&self.id, |r| {
                                if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                                    c.patch = None;
                                    c.note = format!("refused: {}", cl.reasons.join("; "));
                                    c.diagnosis = format!("{diag}\n(The agent's proposed change was refused: {}.)", cl.reasons.join("; "));
                                }
                            });
                            inner.host.audit("flow.heal.rejected", &format!("{}/{node_id} refused by the harness: {}", self.id, cl.reasons.join("; ")));
                            self.evidence(node_id);
                            self.notify_card(node_id, "The agent's fix was refused by the safety rules; it needs you");
                        } else if auto {
                            self.apply_card(card_id, AGENT_WHO, true);
                        } else {
                            self.notify_card(node_id, &format!("The agent proposes: {}", pt.summary()));
                        }
                    }
                    _ => {
                        inner.host.audit("flow.heal.diagnosis", &format!("{}/{node_id}: {}", self.id, short(&diag, 200)));
                        self.evidence(node_id);
                        self.notify_card(node_id, &format!("The agent has no fix: {}", short(&diag, 160)));
                    }
                }
            }
        }
    }

    fn card(&mut self, card_id: &str, d: CardDecision) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        let Some(card) = run.heal.iter().find(|c| c.id == card_id).cloned() else { return };
        if card.state != CardState::Proposed {
            return;
        }
        match d {
            CardDecision::Reject { by, note } => {
                inner.update(&self.id, |r| {
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.state = CardState::Rejected;
                        c.decided_by = by.clone();
                        c.note = note.clone();
                    }
                });
                inner.host.audit("flow.heal.rejected", &format!("{}/{} by {by}: {note}", self.id, card.node));
                inner.log(&self.id, &card.node, &format!("fix rejected by {by}"));
            }
            CardDecision::Approve { by } => {
                inner.host.audit("flow.heal.approved", &format!("{}/{} by {by}", self.id, card.node));
                self.apply_card(card_id, &by, false);
            }
            CardDecision::Edit { by, patch } => {
                let cl = self.classify_checked(&patch, &run.flow, &card.node);
                if cl.class == heal::Class::Refused {
                    inner.log(&self.id, &card.node, &format!("your edited fix was refused: {}", cl.reasons.join("; ")));
                    return;
                }
                inner.update(&self.id, |r| {
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.patch = Some(patch.clone());
                        c.class = Some(cl.clone());
                        c.note = format!("edited by {by}");
                    }
                });
                inner.host.audit("flow.heal.approved", &format!("{}/{} by {by} (edited)", self.id, card.node));
                self.apply_card(card_id, &by, false);
            }
            CardDecision::Investigate { by } => {
                inner.update(&self.id, |r| {
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.state = CardState::Investigating;
                        c.note = format!("asked by {by}");
                    }
                });
                self.spawn_proposal(&card.node, card_id);
            }
        }
    }

    fn apply_card(&mut self, card_id: &str, by: &str, auto: bool) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        let Some(card) = run.heal.iter().find(|c| c.id == card_id).cloned() else { return };
        let Some(patch) = card.patch.clone() else { return };
        let node_id = card.node.clone();
        let simple = card.class.as_ref().is_some_and(|c| c.is_simple());
        let now = inner.now();
        let fail = |this: &mut Driver, why: String| {
            inner.update(&this.id, |r| {
                r.heal_spent += 1;
                if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                    c.state = CardState::Failed;
                    c.note = why.clone();
                    c.decided_by = by.to_string();
                }
            });
            inner.host.audit("flow.heal.failed", &format!("{}/{node_id}: {why}", this.id));
            inner.log(&this.id, &node_id, &format!("the fix could not be applied: {why}"));
            this.begin_heal(&node_id, &why);
        };
        for op in &patch.ops {
            if let PatchOp::CliUpgrade { cli, to } = op {
                inner.log(&self.id, &node_id, &format!("installing {cli} {to}"));
                if let Err(e) = inner.host.cli_upgrade(cli, to) {
                    fail(self, format!("could not install {cli} {to}: {e}"));
                    return;
                }
            }
        }
        let mut flow = run.flow.clone();
        let summary = match heal::apply(&mut flow, &patch, by, simple, now) {
            Ok(s) => s,
            Err(e) => {
                fail(self, e);
                return;
            }
        };
        let rev = inner.source.save_draft_fix(&run.flow_id, &flow, &node_id, by, &summary, simple).unwrap_or_else(|e| {
            inner.log(&self.id, &node_id, &format!("the draft could not be saved: {e}"));
            0
        });
        inner.update(&self.id, |r| {
            r.flow = flow.clone();
            r.heal_spent += 1;
            r.patches.push(AppliedPatch { card: card_id.to_string(), node: node_id.clone(), by: by.to_string(), at_ms: now, summary: summary.clone(), simple });
            if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                c.state = CardState::Applied;
                c.decided_by = by.to_string();
                c.draft_rev = rev;
            }
        });
        inner.host.audit("flow.heal.applied", &format!("{}/{node_id} by {by}{}: {summary} (draft r{rev})", self.id, if auto { " (automatic, simple)" } else { "" }));
        inner.log(&self.id, &node_id, &format!("{} applied: {summary}", if auto { "simple fix" } else { "fix" }));
        let Some(node) = flow.node(&node_id).cloned() else { return };
        let idem = inner.reg.get(&node.kind).is_some_and(|k| k.idempotent(&node));
        if idem {
            let Some(run2) = self.run() else { return };
            let input = run2.nodes.get(&node_id).map(|n| n.input.clone()).unwrap_or(Value::Null);
            let ctx = graph::ctx_for(&run2, &input, &[]);
            self.dry.insert(node_id.clone());
            let (inner2, tx, cancel, card_s, nid) = (inner.clone(), self.tx.clone(), inner.cancel_of(&self.id), card_id.to_string(), node_id.clone());
            let run_id = self.id.clone();
            let h = std::thread::Builder::new().name(format!("flow-dry-{}", self.id)).spawn(move || {
                let result = dry_run(&inner2, &run_id, &node, &flow, &ctx, &cancel);
                let _ = tx.send(Ctl::DryRun { node: nid, card: card_s, result });
            });
            match h {
                Ok(h) => inner.threads.lock().unwrap_or_else(|e| e.into_inner()).push(h),
                Err(_) => {
                    self.dry.remove(&node_id);
                    self.rerun_node(&node_id, by, &summary, rev);
                }
            }
        } else {
            self.rerun_node(&node_id, by, &summary, rev);
        }
    }

    fn dry_run_done(&mut self, node_id: &str, card_id: &str, result: Result<(), String>) {
        let inner = self.inner.clone();
        let Some(run) = self.run() else { return };
        match result {
            Ok(()) => {
                let (by, summary, rev) = run.heal.iter().find(|c| c.id == card_id).map(|c| (c.decided_by.clone(), c.patch.as_ref().map(|p| p.summary()).unwrap_or_default(), c.draft_rev)).unwrap_or_default();
                inner.log(&self.id, node_id, "the fixed step passed a test with the recorded input");
                self.rerun_node(node_id, &by, &summary, rev);
            }
            Err(e) => {
                let e = self.red().text(&e);
                inner.update(&self.id, |r| {
                    if let Some(c) = r.heal.iter_mut().find(|c| c.id == card_id) {
                        c.state = CardState::Failed;
                        c.note = format!("the test of the fixed step failed: {e}");
                    }
                });
                inner.host.audit("flow.heal.failed", &format!("{}/{node_id}: test of the fix failed: {e}", self.id));
                inner.log(&self.id, node_id, &format!("the fixed step failed its test: {e}"));
                self.begin_heal(node_id, &e);
            }
        }
    }

    fn rerun_node(&mut self, node_id: &str, by: &str, summary: &str, rev: u32) {
        // who proposed it and who decided are two different things: an assistant's proposal is "approved by" the person, never "fixed by" them
        let edited = self.run().is_some_and(|r| r.heal.iter().rev().find(|c| c.node == node_id).is_some_and(|c| c.note.starts_with("edited by")));
        let who = if by == AGENT_WHO {
            format!("fixed by {by}")
        } else if edited {
            format!("proposed by the assistant, edited and approved by {by}")
        } else {
            format!("proposed by the assistant, approved by {by}")
        };
        let note = format!("{who}: {summary}{}", if rev > 0 { format!(" (draft r{rev})") } else { String::new() });
        self.inner.update(&self.id, |r| {
            if let Some(nr) = r.nodes.get_mut(node_id) {
                let attempts = std::mem::take(&mut nr.attempts);
                *nr = NodeRun { attempts, note: note.clone(), ..Default::default() };
            }
            for e in r.flow.edges.iter().filter(|e| e.from == node_id) {
                r.edges.insert(e.key(), EdgeState::Open);
            }
        });
        self.inner.log(&self.id, node_id, "running it again with the fix");
    }

    /// The evidence of a failure that needs a person: the node, its attempts, the context and the log, in the flow's evidence folder.
    fn evidence(&self, node_id: &str) {
        let Some(run) = self.run() else { return };
        let Some(dir) = self.inner.host.evidence_dir(&run.flow_id, &self.id, node_id) else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let red = self.red();
        let w = |name: &str, text: String| {
            let _ = std::fs::write(dir.join(name), red.text(&text));
        };
        if let Some(n) = run.flow.node(node_id) {
            w("node.json", serde_json::to_string_pretty(n).unwrap_or_default());
        }
        if let Some(nr) = run.nodes.get(node_id) {
            w("attempts.json", serde_json::to_string_pretty(&nr.attempts).unwrap_or_default());
            w("node-log.txt", nr.log.join("\n"));
        }
        w("context.txt", heal::failure_context(&run, node_id, &[]));
        w("cards.json", serde_json::to_string_pretty(&run.heal).unwrap_or_default());
        w("patches.json", serde_json::to_string_pretty(&run.patches).unwrap_or_default());
        w("run-log.txt", self.inner.store.read_log(&self.id));
        self.inner.log(&self.id, node_id, &format!("evidence saved in {}", dir.display()));
    }
}

struct NoSub2;
impl crate::kinds::SubRunner for NoSub2 {
    fn run_sub(&self, _f: &str, _i: Value, _c: &Cancel) -> Result<Value, NodeError> {
        Err(NodeError::config("not available here"))
    }
}

fn fire_on_failure(inner: &Arc<Inner>, failed: &Run) {
    let e = Engine { inner: inner.clone() };
    for f in inner.source.list_published() {
        if f.id == failed.flow_id {
            continue;
        }
        for t in f.triggers.iter().filter(|t| t.enabled) {
            if let TriggerKind::OnFailure { flow } = &t.kind {
                if *flow == failed.flow_id {
                    let mut req = StartRequest::new(&f.id).trigger("on_failure", &failed.id);
                    req.inputs = json!({"failed_run": failed.id, "failed_flow": failed.flow_id, "error": failed.error});
                    req.idempotency_key = Some(format!("onfail-{}-{}", f.id, failed.id));
                    let _ = e.start_run(req);
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the worker

fn preview(v: &Value) -> String {
    let t = v.to_string();
    t.chars().take(300).collect()
}

/// Run a node with its retry policy: the loop of attempts, each with its own timeout and the wait before it.
fn execute_node(inner: &Arc<Inner>, run_id: &str, node: &Node, flow: &Flow, ctx: &Value, cancel: &Cancel, tx: &Sender<Ctl>) -> Final {
    let Some(kind) = inner.reg.get(&node.kind).cloned() else {
        return Final::Failed { err: NodeError::config(format!("`{}` is not a node kind", node.kind)), class: ErrClass::Permanent };
    };
    let red = inner.redactor(run_id);
    let policy = flow.retry_of(node);
    let attempts = policy.total_attempts();
    let base_timeout = flow.timeout_of(node);
    let first_start = inner.now();
    let sub = SubCtx { inner: inner.clone(), parent: run_id.to_string() };
    let mut retry_after: Option<u64> = None;
    let mut last_err = NodeError::new("no attempt was made");
    let mut last_class = ErrClass::Permanent;
    for n in 1..=attempts {
        if cancel.is_cancelled() {
            return Final::Cancelled;
        }
        let seed = hash_parts(&[run_id, &node.id, &n.to_string()]);
        let delay = policy.delay_before(n, retry_after, seed);
        if n > 1 {
            if !policy.within_deadline(first_start, inner.now(), delay) {
                last_err.message = format!("{} (the total time of {} ms for retries ran out)", last_err.message, policy.total_deadline_ms.unwrap_or(0));
                return Final::Failed { err: last_err, class: last_class };
            }
            let at = inner.now() + delay as i64;
            inner.update(run_id, |r| {
                if let Some(nr) = r.nodes.get_mut(&node.id) {
                    nr.state = NodeState::Retrying;
                    nr.next_retry_ms = Some(at);
                }
            });
            inner.log(run_id, &node.id, &format!("attempt {}/{attempts} in {delay} ms", n));
            let _ = tx.send(Ctl::Wake);
            if delay > 0 && !inner.clock.sleep_ms(delay, cancel) {
                return Final::Cancelled;
            }
        }
        let timeout = policy.timeout_for(n, base_timeout);
        let started = inner.now();
        inner.update(run_id, |r| {
            if let Some(nr) = r.nodes.get_mut(&node.id) {
                nr.state = NodeState::Running;
                nr.next_retry_ms = None;
                nr.attempts.push(Attempt { n, started_ms: started, timeout_ms: timeout, delay_before_ms: if n > 1 { delay } else { 0 }, ..Default::default() });
            }
        });
        let _ = tx.send(Ctl::Wake);
        let cfg = match resolve_config(&inner.host, &red, kind.as_ref(), node, ctx) {
            Ok(c) => c,
            Err(e) => {
                record_attempt(inner, run_id, &node.id, n, false, &e.message, "permanent", "", vec![], &red);
                return Final::Failed { err: e, class: ErrClass::Permanent };
            }
        };
        let cx = NodeCx { run_id, flow, node_id: &node.id, attempt: n, timeout_ms: timeout, ctx, host: inner.host.as_ref(), cancel, clock: inner.clock.as_ref(), redactor: &red, runner: flow.runner_of(node), sub: &sub, logs: Mutex::new(vec![]) };
        let res = kind.run(&cx, node, &cfg);
        let logs = cx.logs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for l in &logs {
            inner.log(run_id, &node.id, l);
        }
        match res {
            Ok(out) => {
                record_attempt(inner, run_id, &node.id, n, true, "", "", &preview(&out.value), logs, &red);
                return Final::Ok(out);
            }
            Err(e) => {
                if e.cancelled || cancel.is_cancelled() {
                    record_attempt(inner, run_id, &node.id, n, false, "cancelled", "permanent", "", logs, &red);
                    return Final::Cancelled;
                }
                let class = kind.classify_error(&e, node);
                let msg = red.text(&e.message);
                record_attempt(inner, run_id, &node.id, n, false, &msg, if class == ErrClass::Retryable { "retryable" } else { "permanent" }, &red.text(&short(&e.output, 300)), logs, &red);
                inner.log(run_id, &node.id, &format!("attempt {n}/{attempts} failed ({}): {msg}", if class == ErrClass::Retryable { "can be retried" } else { "will not be retried" }));
                retry_after = e.retry_after_ms;
                last_err = e;
                last_class = class;
                if class == ErrClass::Permanent {
                    return Final::Failed { err: last_err, class };
                }
            }
        }
    }
    Final::Failed { err: last_err, class: last_class }
}

#[allow(clippy::too_many_arguments)]
fn record_attempt(inner: &Arc<Inner>, run_id: &str, node_id: &str, n: u32, ok: bool, err: &str, class: &str, out: &str, logs: Vec<String>, red: &Redactor) {
    let now = inner.now();
    let (err, out) = (red.text(err), red.text(out));
    inner.update(run_id, |r| {
        if let Some(nr) = r.nodes.get_mut(node_id) {
            if let Some(a) = nr.attempts.iter_mut().rev().find(|a| a.n == n) {
                a.ended_ms = now;
                a.ok = ok;
                a.error = err;
                a.class = class.to_string();
                a.output_preview = out;
                a.logs = logs;
            }
        }
    });
}

/// One try of the fixed node with the input it had, to see that the fix works before the run goes on. Nothing is stored.
fn dry_run(inner: &Arc<Inner>, run_id: &str, node: &Node, flow: &Flow, ctx: &Value, cancel: &Cancel) -> Result<(), String> {
    let kind = inner.reg.get(&node.kind).cloned().ok_or("unknown node kind")?;
    let red = inner.redactor(run_id);
    let policy = flow.retry_of(node);
    let timeout = policy.timeout_for(1, flow.timeout_of(node));
    let cfg = resolve_config(&inner.host, &red, kind.as_ref(), node, ctx).map_err(|e| e.message)?;
    let sub = SubCtx { inner: inner.clone(), parent: run_id.to_string() };
    let cx = NodeCx { run_id, flow, node_id: &node.id, attempt: 1, timeout_ms: timeout, ctx, host: inner.host.as_ref(), cancel, clock: inner.clock.as_ref(), redactor: &red, runner: flow.runner_of(node), sub: &sub, logs: Mutex::new(vec![]) };
    kind.run(&cx, node, &cfg).map(|_| ()).map_err(|e| e.message)
}
