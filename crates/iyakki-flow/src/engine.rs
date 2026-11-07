//! The engine: owns the runs, starts them (now, later, from a schedule), keeps them on disk, brings them back after a restart,
//! and answers the screen and the assistant. The work of one run is in `driver.rs`. No UI, no network, no process: the world is
//! the [`Host`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::graph;
use crate::heal::Patch;
use crate::host::{Cancel, Clock, Host, Redactor};
use crate::kinds::{NodeCx, NoSub, Registry, SubRunner};
use crate::model::*;
use crate::retry::NodeError;
use crate::runstore::RunStore;
use crate::sched::{Scheduler, TickEnv};
use crate::store::{compatible, FlowSource};

pub const MAX_LOG: usize = 400;
pub const MAX_NODE_LOG: usize = 200;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_SUB_DEPTH: usize = 4;
pub const IDEMPOTENCY_WINDOW_MS: i64 = 24 * 3600 * 1000;

#[derive(Clone, Debug)]
pub struct EngineCfg {
    /// The engine's folder: `runs/` and `sched.json` live in it.
    pub dir: PathBuf,
    /// Runs going at once, in all flows.
    pub max_runs: usize,
    /// Start the background thread that moves scheduled runs along and ticks the scheduler.
    pub auto_tick: bool,
    /// Who the engine works for (audit lines and inbox notes).
    pub user: String,
    /// Cloud tenants: who may lead and whether one more queued run may start (see `Gate`). `None` on a desktop.
    pub gate: Option<Gate>,
    /// Cloud tenants: the zone `local` means in cron triggers (the tenant's setting) instead of the host's own.
    pub local_tz: Option<String>,
}

/// The cloud tenant's hooks into the engine. `lead` says whether this process is the one that runs the tenant's schedules (a replica
/// without the lease still serves manual runs but starts nothing on a timer); `admit(flow, started_in_this_pass)` says whether one more
/// queued run may start now (tenant and person quotas); a refused run stays queued and is asked again at the next pass.
#[derive(Clone)]
pub struct Gate {
    pub lead: Arc<dyn Fn() -> bool + Send + Sync>,
    pub admit: Arc<dyn Fn(&str, usize) -> bool + Send + Sync>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gate")
    }
}

impl EngineCfg {
    pub fn new(dir: &std::path::Path) -> EngineCfg {
        EngineCfg { dir: dir.to_path_buf(), max_runs: 8, auto_tick: false, user: "user".into(), gate: None, local_tz: None }
    }
}

/// Messages to a run's driver.
pub(crate) enum Ctl {
    Done { node: String, result: crate::driver::Final },
    Cancel,
    Decide { node: String, approved: bool, by: String, note: String },
    RetryNode { node: String },
    SkipNode { node: String, by: String },
    RerunFrom { node: String },
    Card { card: String, decision: CardDecision },
    Proposal { node: String, card: String, result: Result<crate::host::HealProposal, String> },
    DryRun { node: String, card: String, result: Result<(), String> },
    Wake,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CardDecision {
    Approve { by: String },
    Reject { by: String, note: String },
    /// Approve with the person's own version of the patch.
    Edit { by: String, patch: Patch },
    /// Ask the agent to look (for `always_ask`).
    Investigate { by: String },
}

pub(crate) struct Slot {
    pub run: Run,
    pub tx: Option<Sender<Ctl>>,
    pub cancel: Cancel,
    pub redactor: Arc<Redactor>,
}

pub(crate) struct Inner {
    pub cfg: EngineCfg,
    pub host: Arc<dyn Host>,
    pub reg: Arc<Registry>,
    pub clock: Arc<dyn Clock>,
    pub source: Arc<dyn FlowSource>,
    pub store: RunStore,
    pub state: Mutex<BTreeMap<String, Slot>>,
    pub next_id: AtomicU64,
    pub sched: Mutex<Scheduler>,
    pub halting: AtomicBool,
    /// One write of a run at a time, and the copy taken inside it, so an older copy never lands after a newer one.
    pub persist_lock: Mutex<()>,
    pub last_tick_ms: AtomicI64,
    pub threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// The last tick was made as the leader (a gate that says "not me" makes the next "me" reload the scheduler's file).
    pub was_leading: AtomicBool,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

#[derive(Clone, Debug, Default)]
pub struct StartRequest {
    pub flow_id: String,
    /// A published version (None: the latest published).
    pub version: Option<u32>,
    /// Run the draft instead (for trying a change).
    pub draft: bool,
    /// Run this flow as given (a snapshot; tests and "run this open flow").
    pub flow: Option<Flow>,
    pub inputs: Value,
    pub trigger: TriggerInfo,
    /// Start at this time (a scheduled run).
    pub at_ms: Option<i64>,
    pub idempotency_key: Option<String>,
    pub parent: Option<String>,
    /// Permissions the person gave this run only (see [`Run::grants`]); set by the screen after an "Allow this time".
    pub grants: Vec<String>,
}

impl StartRequest {
    pub fn new(flow_id: &str) -> StartRequest {
        StartRequest { flow_id: flow_id.into(), trigger: TriggerInfo { kind: "manual".into(), ..Default::default() }, inputs: json!({}), ..Default::default() }
    }
    pub fn trigger(mut self, kind: &str, by: &str) -> StartRequest {
        self.trigger = TriggerInfo { kind: kind.into(), by: by.into(), ..Default::default() };
        self
    }
    pub fn inputs(mut self, v: Value) -> StartRequest {
        self.inputs = v;
        self
    }
    pub fn key(mut self, k: &str) -> StartRequest {
        self.idempotency_key = Some(k.into());
        self
    }
    pub fn at(mut self, ms: i64) -> StartRequest {
        self.at_ms = Some(ms);
        self
    }
    pub fn with_flow(mut self, f: Flow) -> StartRequest {
        self.flow = Some(f);
        self
    }
    pub fn draft(mut self) -> StartRequest {
        self.draft = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartOutcome {
    pub run_id: String,
    /// An idempotency key matched an earlier run: nothing new was started.
    pub existing: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub scheduled: usize,
    pub queued: usize,
    pub running: usize,
    pub needs_you: usize,
    pub failed: usize,
    pub done: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyScope {
    Scheduled,
    Running,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeUpdateInfo {
    pub flow_id: String,
    pub node: String,
    pub to_version: u32,
    /// Runs that have not started this node and use an older definition: waiting to start, and going.
    pub scheduled: Vec<String>,
    pub running: Vec<String>,
    pub compatible: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyReport {
    pub scheduled_updated: Vec<String>,
    /// Runs that will take the new definition at the node's next boundary.
    pub running_pending: Vec<String>,
    pub skipped_started: Vec<String>,
    pub skipped_incompatible: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeTest {
    pub ok: bool,
    pub output: Value,
    pub port: String,
    pub error: String,
    pub logs: Vec<String>,
    pub waiting: bool,
}

pub(crate) fn cap_value(v: Value) -> Value {
    let s = v.to_string();
    if s.len() <= MAX_OUTPUT_BYTES {
        return v;
    }
    let preview: String = s.chars().take(2000).collect();
    json!({"_truncated": true, "bytes": s.len(), "preview": preview})
}

pub(crate) fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}...")
    } else {
        t
    }
}

impl Inner {
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    pub fn halted(&self) -> bool {
        self.halting.load(Ordering::SeqCst)
    }

    /// Change a run in memory (the `rev` goes up) without writing it.
    pub fn with_run<R>(&self, id: &str, f: impl FnOnce(&mut Run) -> R) -> Option<R> {
        if self.halted() {
            return None;
        }
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let slot = g.get_mut(id)?;
        let r = f(&mut slot.run);
        slot.run.rev += 1;
        Some(r)
    }

    pub fn persist(&self, id: &str) {
        if self.halted() {
            return;
        }
        let _w = self.persist_lock.lock().unwrap_or_else(|e| e.into_inner());
        let run = self.state.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.run.clone());
        if let Some(r) = run {
            if let Err(e) = self.store.save(&r) {
                self.host.audit("flow.run.save_failed", &format!("{id}: {e}"));
            }
        }
    }

    /// Change and write.
    pub fn update<R>(&self, id: &str, f: impl FnOnce(&mut Run) -> R) -> Option<R> {
        let r = self.with_run(id, f);
        self.persist(id);
        r
    }

    pub fn get(&self, id: &str) -> Option<Run> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.run.clone())
    }

    pub fn redactor(&self, id: &str) -> Arc<Redactor> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.redactor.clone()).unwrap_or_default()
    }

    pub fn cancel_of(&self, id: &str) -> Cancel {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.cancel.clone()).unwrap_or_default()
    }

    pub fn send(&self, id: &str, m: Ctl) -> bool {
        let tx = self.state.lock().unwrap_or_else(|e| e.into_inner()).get(id).and_then(|s| s.tx.clone());
        tx.is_some_and(|t| t.send(m).is_ok())
    }

    /// One line in the run's log (screen copy, node copy and the file), with secrets removed.
    pub fn log(&self, id: &str, node: &str, text: &str) {
        let red = self.redactor(id);
        let line = red.text(&short(text, 600));
        let t = self.now();
        self.store.append_log(id, &format!("{} {}{}", t, if node.is_empty() { String::new() } else { format!("[{node}] ") }, line));
        self.with_run(id, |r| {
            r.log.push(LogLine { t_ms: t, node: node.to_string(), text: line.clone() });
            if r.log.len() > MAX_LOG {
                let drop = r.log.len() - MAX_LOG;
                r.log.drain(..drop);
            }
            if let Some(nr) = r.nodes.get_mut(node) {
                nr.log.push(line);
                if nr.log.len() > MAX_NODE_LOG {
                    let drop = nr.log.len() - MAX_NODE_LOG;
                    nr.log.drain(..drop);
                }
            }
        });
    }

    pub fn summaries(&self) -> Vec<RunSummary> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).values().map(|s| s.run.summary()).collect()
    }

    /// A run of this flow is queued or going (blocked and scheduled ones count: they will go on).
    pub fn flow_busy(&self, flow_id: &str) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).values().any(|s| s.run.flow_id == flow_id && s.run.parent.is_none() && !s.run.status.is_terminal() && s.run.status != RunStatus::Scheduled)
    }

    pub fn heal_today(&self, flow_id: &str, now: i64) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|s| s.run.flow_id == flow_id)
            .flat_map(|s| s.run.heal.iter())
            .filter(|c| now - c.created_ms < 24 * 3600 * 1000 && matches!(c.state, CardState::Applied | CardState::Verified | CardState::Failed))
            .count() as u32
    }
}

/// A run with every node pending and every edge open (the engine starts from it; tests drive the graph rules with it).
pub fn blank_run(id: &str, flow: Flow, inputs: Value, trigger: TriggerInfo, now: i64) -> Run {
    let mut r = Run {
        grants: vec![],
        id: id.to_string(),
        flow_id: flow.id.clone(),
        flow,
        trigger,
        status: RunStatus::Queued,
        inputs,
        outputs: Value::Null,
        nodes: BTreeMap::new(),
        edges: BTreeMap::new(),
        created_ms: now,
        scheduled_ms: None,
        started_ms: 0,
        ended_ms: 0,
        error: String::new(),
        blocked: String::new(),
        idempotency_key: None,
        parent: None,
        heal: vec![],
        patches: vec![],
        heal_spent: 0,
        heal_cents: 0,
        pending_updates: BTreeMap::new(),
        log: vec![],
        rev: 1,
        runner_note: String::new(),
    };
    graph::init(&mut r);
    r
}

/// Fill in defaults and check required inputs.
pub fn bind_inputs(flow: &Flow, given: &Value) -> Result<Value, String> {
    let mut m = given.as_object().cloned().unwrap_or_default();
    for p in &flow.inputs {
        if !m.contains_key(&p.name) {
            match &p.default {
                Some(d) => {
                    m.insert(p.name.clone(), d.clone());
                }
                None if p.required => return Err(format!("the flow needs an input `{}`", p.name)),
                None => {}
            }
        }
    }
    Ok(Value::Object(m))
}

impl Engine {
    pub fn new(cfg: EngineCfg, host: Arc<dyn Host>, reg: Arc<Registry>, clock: Arc<dyn Clock>, source: Arc<dyn FlowSource>) -> Engine {
        let local_tz = cfg.local_tz.clone();
        let sched = Scheduler::new(Some(cfg.dir.join("sched.json")), Arc::new({
            let h = host.clone();
            move |n: &str| match (&local_tz, n.trim()) {
                (Some(z), "") | (Some(z), "local") => h.tz(z),
                _ => h.tz(n),
            }
        }));
        let store = RunStore::new(&cfg.dir);
        let next = store.ids().iter().filter_map(|i| i.strip_prefix('r').and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0) + 1;
        let auto = cfg.auto_tick;
        let inner = Arc::new(Inner { cfg, host, reg, clock, source, store, state: Mutex::new(BTreeMap::new()), next_id: AtomicU64::new(next), sched: Mutex::new(sched), halting: AtomicBool::new(false), persist_lock: Mutex::new(()), last_tick_ms: AtomicI64::new(0), threads: Mutex::new(Vec::new()), was_leading: AtomicBool::new(false) });
        let e = Engine { inner };
        if auto {
            e.start_background();
        }
        e
    }

    /// The background tick is going (started with `auto_tick` and not halted).
    pub fn ticking(&self) -> bool {
        self.inner.cfg.auto_tick && !self.inner.halted()
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.inner.reg.clone()
    }

    fn start_background(&self) {
        let inner = self.inner.clone();
        let h = std::thread::Builder::new().name("flow-tick".into()).spawn(move || {
            let e = Engine { inner };
            while !e.inner.halted() {
                e.tick();
                // a short real pause; the scheduler itself looks at the clock it was given
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        if let Ok(h) = h {
            self.inner.threads.lock().unwrap_or_else(|e| e.into_inner()).push(h);
        }
    }

    /// Stop everything without ending the runs: they stay on disk as they are and `recover` picks them up in the next process.
    pub fn halt(&self) {
        self.inner.halting.store(true, Ordering::SeqCst);
        {
            let g = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            for s in g.values() {
                s.cancel.cancel();
                if let Some(tx) = &s.tx {
                    let _ = tx.send(Ctl::Wake);
                }
            }
        }
        let hs: Vec<_> = std::mem::take(&mut *self.inner.threads.lock().unwrap_or_else(|e| e.into_inner()));
        let end = Instant::now() + Duration::from_secs(5);
        for h in hs {
            while !h.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    // ------------------------------------------------------------------------------------------ starting

    pub fn start_run(&self, req: StartRequest) -> Result<StartOutcome, String> {
        let inner = &self.inner;
        let now = inner.now();
        if let Some(k) = &req.idempotency_key {
            let g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = g.values().find(|s| s.run.flow_id == req.flow_id && s.run.idempotency_key.as_deref() == Some(k.as_str()) && now - s.run.created_ms < IDEMPOTENCY_WINDOW_MS && !matches!(s.run.status, RunStatus::Failed | RunStatus::Cancelled)) {
                return Ok(StartOutcome { run_id: s.run.id.clone(), existing: true });
            }
        }
        let flow = match (&req.flow, req.draft, req.version) {
            (Some(f), _, _) => inner.source.resolve(f.clone()),
            (None, true, _) => inner.source.get_draft(&req.flow_id).ok_or_else(|| format!("there is no flow `{}`", req.flow_id))?,
            (None, false, Some(v)) => inner.source.get_version(&req.flow_id, v).ok_or_else(|| format!("flow `{}` has no version {v}", req.flow_id))?,
            (None, false, None) => inner.source.get_published(&req.flow_id).ok_or_else(|| format!("flow `{}` has no published version (publish it, or run the draft)", req.flow_id))?,
        };
        let issues = crate::validate::validate_flow(&flow, &inner.reg);
        let errs: Vec<String> = crate::validate::errors(&issues).iter().map(|i| format!("{}{}", i.node.as_ref().map(|n| format!("{n}: ")).unwrap_or_default(), i.msg)).collect();
        if !errs.is_empty() {
            return Err(format!("the flow cannot run: {}", errs.join("; ")));
        }
        let inputs = bind_inputs(&flow, &req.inputs)?;
        let id = format!("r{}", inner.next_id.fetch_add(1, Ordering::SeqCst));
        let mut run = blank_run(&id, flow, inputs, req.trigger.clone(), now);
        run.idempotency_key = req.idempotency_key.clone();
        run.parent = req.parent.clone();
        run.grants = req.grants.clone();
        if let Some(at) = req.at_ms.filter(|a| *a > now) {
            run.status = RunStatus::Scheduled;
            run.scheduled_ms = Some(at);
        }
        let desc = format!("{id} {} v{} by {} ({})", run.flow_id, run.flow.meta.version, run.trigger.by, run.trigger.kind);
        inner.state.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Slot { run, tx: None, cancel: Cancel::new(), redactor: Arc::new(Redactor::new()) });
        inner.persist(&id);
        inner.host.audit("flow.run.start", &desc);
        inner.log(&id, "", &format!("run created ({})", req.trigger.kind));
        self.pump();
        Ok(StartOutcome { run_id: id, existing: false })
    }

    /// Start queued runs while there is room (globally and per flow).
    pub fn pump(&self) {
        let inner = &self.inner;
        if inner.halted() {
            return;
        }
        let mut to_start = Vec::new();
        {
            let g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let active = |s: &&Slot| matches!(s.run.status, RunStatus::Running | RunStatus::Retrying | RunStatus::Healing) && s.run.parent.is_none();
            let mut total = g.values().filter(active).count();
            let mut per_flow: BTreeMap<String, usize> = BTreeMap::new();
            for s in g.values().filter(active) {
                *per_flow.entry(s.run.flow_id.clone()).or_default() += 1;
            }
            let mut queued: Vec<&Slot> = g.values().filter(|s| s.run.status == RunStatus::Queued && s.tx.is_none() && s.run.parent.is_none()).collect();
            queued.sort_by_key(|s| (s.run.created_ms, s.run.id.len(), s.run.id.clone()));
            for s in queued {
                let limit = s.run.flow.defaults.max_concurrent_runs.max(1);
                let n = per_flow.entry(s.run.flow_id.clone()).or_default();
                if total < inner.cfg.max_runs && *n < limit {
                    total += 1;
                    *n += 1;
                    to_start.push(s.run.id.clone());
                }
            }
        }
        let mut started = 0usize;
        for id in to_start {
            let flow = inner.get(&id).map(|r| r.flow_id).unwrap_or_default();
            if let Some(g) = &inner.cfg.gate {
                if !(g.admit)(&flow, started) {
                    break;
                }
            }
            started += 1;
            crate::driver::spawn(inner, &id);
        }
    }

    /// Move due scheduled runs to the queue, ask the scheduler what is due, start queued runs. Call every second or so.
    pub fn tick(&self) {
        let inner = &self.inner;
        if inner.halted() {
            return;
        }
        let now = inner.now();
        let due: Vec<String> = inner.state.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|s| s.run.status == RunStatus::Scheduled && s.run.scheduled_ms.is_some_and(|t| t <= now)).map(|s| s.run.id.clone()).collect();
        for id in due {
            inner.update(&id, |r| {
                r.status = RunStatus::Queued;
            });
        }
        let flows = inner.source.list_published();
        let host = inner.host.clone();
        let busy = |f: &str| inner.flow_busy(f);
        let fp = |p: &str| host.file_fingerprint(p);
        let cal = |m: &str, a: i64, b: i64| host.calendar_starts(m, a, b);
        let leading = inner.cfg.gate.as_ref().map_or(true, |g| (g.lead)());
        let fires = if leading {
            let mut s = inner.sched.lock().unwrap_or_else(|e| e.into_inner());
            if !inner.was_leading.swap(true, Ordering::SeqCst) && inner.cfg.gate.is_some() {
                // another process may have fired ticks while this one did not lead: start from what is on the volume
                s.reload();
            }
            s.tick(now, &flows, &TickEnv { busy: &busy, fingerprint: &fp, calendar: &cal })
        } else {
            inner.was_leading.store(false, Ordering::SeqCst);
            Vec::new()
        };
        for f in fires {
            let mut t = TriggerInfo { kind: "schedule".into(), by: format!("trigger {}", f.trigger), detail: if f.missed { "missed tick".into() } else { String::new() }, due_ms: Some(f.due_ms) };
            if let Some(fl) = flows.iter().find(|x| x.id == f.flow) {
                if let Some(tr) = fl.triggers.iter().find(|x| x.id == f.trigger) {
                    t.kind = tr.kind.label().replace(' ', "_");
                }
            }
            let mut req = StartRequest::new(&f.flow);
            req.trigger = t;
            req.inputs = if f.inputs.is_object() { f.inputs.clone() } else { json!({}) };
            req.at_ms = Some(f.at_ms);
            if f.overlap == Overlap::Queue || f.overlap == Overlap::Allow {
                req.idempotency_key = None;
            } else {
                req.idempotency_key = Some(format!("sched-{}-{}-{}", f.flow, f.trigger, f.due_ms));
            }
            if let Err(e) = self.start_run(req) {
                inner.host.audit("flow.schedule.failed", &format!("{}/{}: {e}", f.flow, f.trigger));
            }
        }
        self.pump();
        inner.last_tick_ms.store(now, Ordering::SeqCst);
    }

    /// Something happened that a flow's `event` trigger may be waiting for.
    pub fn emit_event(&self, name: &str, payload: Value) -> Vec<String> {
        let mut started = Vec::new();
        for f in self.inner.source.list_published() {
            for t in f.triggers.iter().filter(|t| t.enabled) {
                if let TriggerKind::Event { name: n } = &t.kind {
                    if n == name {
                        let mut req = StartRequest::new(&f.id).trigger("event", name);
                        req.inputs = json!({"event": name, "payload": payload});
                        if let Ok(o) = self.start_run(req) {
                            started.push(o.run_id);
                        }
                    }
                }
            }
        }
        started
    }

    // ------------------------------------------------------------------------------------------ commands

    pub fn cancel(&self, run_id: &str) -> Result<(), String> {
        let inner = &self.inner;
        let (status, has_driver) = {
            let g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let s = g.get(run_id).ok_or_else(|| format!("there is no run {run_id}"))?;
            (s.run.status, s.tx.is_some())
        };
        if status.is_terminal() {
            return Err(format!("run {run_id} is already {}", status.label()));
        }
        inner.cancel_of(run_id).cancel();
        inner.host.audit("flow.run.cancel", run_id);
        if !has_driver {
            let now = inner.now();
            inner.update(run_id, |r| {
                r.status = RunStatus::Cancelled;
                r.ended_ms = now;
                for n in r.nodes.values_mut() {
                    if !n.state.is_done() {
                        n.state = NodeState::Cancelled;
                    }
                }
            });
            inner.log(run_id, "", "cancelled before it started");
        } else {
            inner.send(run_id, Ctl::Cancel);
        }
        Ok(())
    }

    fn ensure_driver(&self, run_id: &str) {
        let (terminal_or_idle, has_driver) = {
            let g = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            match g.get(run_id) {
                Some(s) => (s.run.status.is_terminal(), s.tx.is_some()),
                None => return,
            }
        };
        if terminal_or_idle && !has_driver {
            self.inner.update(run_id, |r| {
                r.status = RunStatus::Running;
                r.ended_ms = 0;
                r.error.clear();
            });
            crate::driver::spawn(&self.inner, run_id);
        }
    }

    pub fn decide(&self, run_id: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String> {
        let ok = self.inner.get(run_id).and_then(|r| r.nodes.get(node).map(|n| n.state == NodeState::Waiting)).unwrap_or(false);
        if !ok {
            return Err(format!("`{node}` is not waiting for a decision in run {run_id}"));
        }
        self.inner.host.audit("flow.approval", &format!("{run_id}/{node} {} by {by}", if approved { "approved" } else { "rejected" }));
        self.ensure_driver(run_id);
        self.inner.send(run_id, Ctl::Decide { node: node.into(), approved, by: by.into(), note: note.into() });
        Ok(())
    }

    pub fn retry_node(&self, run_id: &str, node: &str) -> Result<(), String> {
        let st = self.inner.get(run_id).and_then(|r| r.nodes.get(node).map(|n| n.state)).ok_or("no such run or node")?;
        if !matches!(st, NodeState::Failed | NodeState::Interrupted | NodeState::Cancelled) {
            return Err(format!("`{node}` is {}: only a failed or interrupted step can be retried", st.label()));
        }
        self.inner.host.audit("flow.node.retry", &format!("{run_id}/{node}"));
        self.ensure_driver(run_id);
        self.inner.send(run_id, Ctl::RetryNode { node: node.into() });
        Ok(())
    }

    /// "Allow this time": give the run one permission and run its stopped steps again. Only the screen calls this, after the person's own answer.
    pub fn grant_run(&self, run_id: &str, permission: &str, by: &str) -> Result<(), String> {
        let r = self.inner.get(run_id).ok_or("no such run")?;
        if r.status.is_terminal() {
            return Err("that run is over".into());
        }
        let stopped: Vec<String> = r.nodes.iter().filter(|(_, n)| n.state == NodeState::Failed && n.needs_person && n.permission == permission).map(|(k, _)| k.clone()).collect();
        if stopped.is_empty() {
            return Err("no step of that run is waiting for this permission".into());
        }
        self.inner.update(run_id, |r| {
            if !r.grants.iter().any(|g| g == permission) {
                r.grants.push(permission.to_string());
            }
        });
        self.inner.host.audit("flow.permission.granted_once", &format!("{run_id} {permission} by {by}"));
        for n in stopped {
            self.retry_node(run_id, &n)?;
        }
        Ok(())
    }

    pub fn skip_node(&self, run_id: &str, node: &str, by: &str) -> Result<(), String> {
        let st = self.inner.get(run_id).and_then(|r| r.nodes.get(node).map(|n| n.state)).ok_or("no such run or node")?;
        if !matches!(st, NodeState::Failed | NodeState::Interrupted | NodeState::Waiting) {
            return Err(format!("`{node}` is {}: only a failed, interrupted or waiting step can be skipped", st.label()));
        }
        self.inner.host.audit("flow.node.skip", &format!("{run_id}/{node} by {by}"));
        self.ensure_driver(run_id);
        self.inner.send(run_id, Ctl::SkipNode { node: node.into(), by: by.into() });
        Ok(())
    }

    pub fn rerun_from(&self, run_id: &str, node: &str) -> Result<(), String> {
        let r = self.inner.get(run_id).ok_or("no such run")?;
        if r.flow.node(node).is_none() {
            return Err(format!("no node `{node}`"));
        }
        if matches!(r.status, RunStatus::Scheduled | RunStatus::Queued) {
            return Err("the run has not started".into());
        }
        let busy = r.nodes.values().any(|n| matches!(n.state, NodeState::Running | NodeState::Retrying));
        if busy {
            return Err("a step is running: cancel the run or wait for it".into());
        }
        self.inner.host.audit("flow.node.rerun_from", &format!("{run_id}/{node}"));
        self.ensure_driver(run_id);
        self.inner.send(run_id, Ctl::RerunFrom { node: node.into() });
        Ok(())
    }

    pub fn card_decide(&self, run_id: &str, card: &str, d: CardDecision) -> Result<(), String> {
        let r = self.inner.get(run_id).ok_or("no such run")?;
        let c = r.heal.iter().find(|c| c.id == card).ok_or_else(|| format!("no card {card}"))?;
        if c.state != CardState::Proposed {
            return Err(format!("card {card} is {:?}, not waiting for you", c.state));
        }
        if matches!(d, CardDecision::Approve { .. } | CardDecision::Edit { .. }) && c.patch.is_none() {
            return Err("this card has no proposed change to approve (retry, skip or edit the step yourself)".into());
        }
        self.ensure_driver(run_id);
        self.inner.send(run_id, Ctl::Card { card: card.into(), decision: d });
        Ok(())
    }

    // ------------------------------------------------------------------------------------------ reading

    pub fn get_run(&self, id: &str) -> Option<Run> {
        self.inner.get(id).or_else(|| self.inner.store.load(id))
    }

    pub fn summaries(&self) -> Vec<RunSummary> {
        let mut v = self.inner.summaries();
        v.sort_by_key(|s| std::cmp::Reverse((s.created_ms, s.id.len(), s.id.clone())));
        v
    }

    pub fn runs_of(&self, flow_id: &str) -> Vec<RunSummary> {
        self.summaries().into_iter().filter(|s| s.flow_id == flow_id).collect()
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for s in self.inner.summaries() {
            match s.status {
                RunStatus::Scheduled => c.scheduled += 1,
                RunStatus::Queued => c.queued += 1,
                RunStatus::Running | RunStatus::Retrying | RunStatus::Healing => c.running += 1,
                RunStatus::Blocked => c.needs_you += 1,
                RunStatus::Failed => c.failed += 1,
                RunStatus::Succeeded | RunStatus::Cancelled => c.done += 1,
            }
        }
        c
    }

    /// A number that changes whenever any run changes (for a screen to know it must redraw).
    pub fn revision(&self) -> u64 {
        let g = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        g.values().map(|s| s.run.rev).sum::<u64>() + g.len() as u64
    }

    pub fn log_text(&self, id: &str) -> String {
        self.inner.store.read_log(id)
    }

    pub fn next_runs(&self, flow: &Flow, n: usize) -> Vec<(String, i64)> {
        self.inner.sched.lock().unwrap_or_else(|e| e.into_inner()).next_runs(flow, self.inner.now(), n)
    }

    pub fn pause_schedule(&self, flow_id: &str, paused: bool) {
        let mut s = self.inner.sched.lock().unwrap_or_else(|e| e.into_inner());
        if paused {
            s.pause_flow(flow_id);
        } else {
            s.resume_flow(flow_id);
        }
        s.save();
    }

    pub fn schedule_paused(&self, flow_id: &str) -> bool {
        self.inner.sched.lock().unwrap_or_else(|e| e.into_inner()).is_paused(flow_id)
    }

    pub fn sched_entry(&self, flow: &str, trigger: &str) -> Option<crate::sched::Entry> {
        self.inner.sched.lock().unwrap_or_else(|e| e.into_inner()).entry(flow, trigger).cloned()
    }

    /// Wait until no run is active (tests, and "wait for it" in tools). Blocked and scheduled runs count as idle.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            let busy = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).values().any(|s| matches!(s.run.status, RunStatus::Running | RunStatus::Retrying | RunStatus::Healing | RunStatus::Queued));
            if !busy {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait until a run's status satisfies `f`.
    pub fn wait_for(&self, id: &str, timeout: Duration, f: impl Fn(&Run) -> bool) -> Option<Run> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(r) = self.inner.get(id) {
                if f(&r) {
                    return Some(r);
                }
            }
            if Instant::now() > end {
                return self.inner.get(id);
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    /// Wait until some run of the flow exists and has stopped (finished or waiting for a person).
    pub fn wait_for_flow(&self, flow_id: &str, timeout: Duration) -> Option<Run> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(r) = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).values().map(|s| &s.run).find(|r| r.flow_id == flow_id && (r.status.is_terminal() || r.status == RunStatus::Blocked)) {
                return Some(r.clone());
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    // ------------------------------------------------------------------------------------------ recovery

    /// Read the runs on disk. Call once at start-up, before ticking. A run that was going when the program stopped:
    /// steps that are safe to repeat go back to pending, the others become `interrupted` and the run needs you.
    pub fn recover(&self) -> usize {
        let inner = &self.inner;
        let now = inner.now();
        let mut n = 0;
        for id in inner.store.ids() {
            let Some(mut run) = inner.store.load(&id) else { continue };
            if inner.state.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&id) {
                continue;
            }
            if !run.status.is_terminal() && !matches!(run.status, RunStatus::Scheduled | RunStatus::Queued) {
                let flow = run.flow.clone();
                for node in &flow.nodes {
                    let idem = inner.reg.get(&node.kind).is_some_and(|k| k.idempotent(node));
                    let Some(nr) = run.nodes.get_mut(&node.id) else { continue };
                    match nr.state {
                        NodeState::Running | NodeState::Retrying => {
                            if idem {
                                nr.state = NodeState::Pending;
                                nr.note = "restarted while this ran; it is safe to repeat, so it ran again".into();
                            } else {
                                nr.state = NodeState::Interrupted;
                                nr.error = "the program restarted while this step was running".into();
                            }
                        }
                        _ => {}
                    }
                }
                for c in run.heal.iter_mut().filter(|c| c.state == CardState::Investigating) {
                    c.state = CardState::Proposed;
                    c.note = "the program restarted while the agent was looking".into();
                    c.diagnosis = "The agent was interrupted by a restart. Ask it again, retry the step or skip it.".into();
                }
                run.status = RunStatus::Running;
            }
            if run.status == RunStatus::Scheduled && run.scheduled_ms.is_none_or(|t| t <= now) {
                run.status = RunStatus::Queued;
            }
            let status = run.status;
            inner.state.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Slot { run, tx: None, cancel: Cancel::new(), redactor: Arc::new(Redactor::new()) });
            inner.persist(&id);
            if matches!(status, RunStatus::Running | RunStatus::Blocked | RunStatus::Retrying | RunStatus::Healing) {
                crate::driver::spawn(inner, &id);
            }
            n += 1;
        }
        inner.host.audit("flow.recover", &format!("{n} runs read"));
        self.pump();
        n
    }

    // ------------------------------------------------------------------------------------------ versions in running tasks

    /// Runs that use an older definition of a node than the flow's latest published version, grouped by node.
    pub fn node_updates(&self, flow_id: &str) -> Vec<NodeUpdateInfo> {
        let inner = &self.inner;
        let Some(latest) = inner.source.get_published(flow_id) else { return vec![] };
        let g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut by_node: BTreeMap<String, NodeUpdateInfo> = BTreeMap::new();
        for s in g.values().filter(|s| s.run.flow_id == flow_id && !s.run.status.is_terminal()) {
            for new in &latest.nodes {
                let Some(old) = s.run.flow.node(&new.id) else { continue };
                if old.same_definition(new) {
                    continue;
                }
                let state = s.run.nodes.get(&new.id).map(|n| n.state).unwrap_or_default();
                if state != NodeState::Pending {
                    continue;
                }
                let e = by_node.entry(new.id.clone()).or_insert_with(|| NodeUpdateInfo { flow_id: flow_id.into(), node: new.id.clone(), to_version: latest.meta.version, scheduled: vec![], running: vec![], compatible: true });
                if !compatible(old, new, &inner.reg) {
                    e.compatible = false;
                }
                if matches!(s.run.status, RunStatus::Scheduled | RunStatus::Queued) {
                    e.scheduled.push(s.run.id.clone());
                } else {
                    e.running.push(s.run.id.clone());
                }
            }
        }
        by_node.into_values().collect()
    }

    /// Give runs the latest published definition of `node`: a run that has not started takes it now; a running one takes it when
    /// the node is about to start (never in the middle of it); one that already ran the node is left alone.
    pub fn apply_node_update(&self, flow_id: &str, node: &str, scope: ApplyScope, by: &str, allow_incompatible: bool) -> Result<ApplyReport, String> {
        let inner = &self.inner;
        let latest = inner.source.get_published(flow_id).ok_or_else(|| format!("flow `{flow_id}` has no published version"))?;
        let new = latest.node(node).cloned().ok_or_else(|| format!("the latest version has no node `{node}`"))?;
        let mut rep = ApplyReport::default();
        let mut touched = Vec::new();
        {
            let mut g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            for s in g.values_mut().filter(|s| s.run.flow_id == flow_id && !s.run.status.is_terminal()) {
                let Some(old) = s.run.flow.node(node).cloned() else { continue };
                if old.same_definition(&new) {
                    continue;
                }
                let id = s.run.id.clone();
                if s.run.nodes.get(node).map(|n| n.state).unwrap_or_default() != NodeState::Pending {
                    rep.skipped_started.push(id);
                    continue;
                }
                if !allow_incompatible && !compatible(&old, &new, &inner.reg) {
                    rep.skipped_incompatible.push(id);
                    continue;
                }
                let waiting_to_start = matches!(s.run.status, RunStatus::Scheduled | RunStatus::Queued);
                match (waiting_to_start, scope) {
                    (true, ApplyScope::Scheduled | ApplyScope::Both) => {
                        let pos = old.pos;
                        if let Some(n) = s.run.flow.node_mut(node) {
                            *n = new.clone();
                            n.pos = pos;
                        }
                        if let Some(nr) = s.run.nodes.get_mut(node) {
                            nr.note = format!("updated to v{} by {by} before it started", latest.meta.version);
                        }
                        s.run.rev += 1;
                        rep.scheduled_updated.push(id.clone());
                        touched.push(id);
                    }
                    (false, ApplyScope::Running | ApplyScope::Both) => {
                        s.run.pending_updates.insert(node.to_string(), PendingUpdate { node: new.clone(), from_version: s.run.flow.meta.version, to_version: latest.meta.version, by: by.to_string() });
                        s.run.rev += 1;
                        rep.running_pending.push(id.clone());
                        touched.push(id);
                    }
                    _ => {}
                }
            }
        }
        for id in &touched {
            inner.persist(id);
            inner.log(id, node, &format!("node update to v{} accepted by {by}", latest.meta.version));
        }
        inner.host.audit("flow.node.apply", &format!("{flow_id}/{node} v{} scope {:?} by {by}: {} scheduled, {} running", latest.meta.version, scope, rep.scheduled_updated.len(), rep.running_pending.len()));
        Ok(rep)
    }

    // ------------------------------------------------------------------------------------------ single node

    /// Run one node once with a sample input, without making a run (the designer's "test this node"). It does what the node does:
    /// a command runs, a call is made. Call it from a worker thread.
    pub fn test_node(&self, flow: &Flow, node_id: &str, sample_inputs: Value, input: Value) -> Result<NodeTest, String> {
        let inner = &self.inner;
        let node = flow.node(node_id).ok_or_else(|| format!("no node `{node_id}`"))?.clone();
        let kind = inner.reg.get(&node.kind).ok_or_else(|| format!("`{}` is not a node kind", node.kind))?.clone();
        let mut run = blank_run("test", flow.clone(), sample_inputs, TriggerInfo { kind: "test".into(), ..Default::default() }, inner.now());
        run.id = "test".into();
        let ctx = graph::ctx_for(&run, &input, &[]);
        let cancel = Cancel::new();
        let red = Redactor::new();
        let policy = flow.retry_of(&node);
        let timeout = policy.timeout_for(1, flow.timeout_of(&node));
        let cfg = crate::driver::resolve_config(&inner.host, &red, kind.as_ref(), &node, &ctx).map_err(|e| e.message)?;
        let cx = NodeCx { run_id: "test", flow, node_id: &node.id, attempt: 1, timeout_ms: timeout, ctx: &ctx, host: inner.host.as_ref(), cancel: &cancel, clock: inner.clock.as_ref(), redactor: &red, runner: flow.runner_of(&node), sub: &NoSub, logs: Mutex::new(vec![]) };
        let res = kind.run(&cx, &node, &cfg);
        let logs = cx.logs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(match res {
            Ok(o) => NodeTest { ok: true, output: red.value(&o.value), port: o.port, error: String::new(), logs, waiting: o.wait.is_some() },
            Err(e) => NodeTest { ok: false, output: Value::Null, port: String::new(), error: red.text(&e.message), logs, waiting: false },
        })
    }
}

/// Runs another flow inline, on the calling thread, as a child run.
pub(crate) struct SubCtx {
    pub inner: Arc<Inner>,
    pub parent: String,
}

impl SubRunner for SubCtx {
    fn run_sub(&self, flow_ref: &str, inputs: Value, cancel: &Cancel) -> Result<Value, NodeError> {
        let inner = &self.inner;
        // the chain of parents: depth and cycles
        let mut chain = vec![];
        let mut cur = Some(self.parent.clone());
        while let Some(id) = cur {
            match inner.get(&id) {
                Some(r) => {
                    chain.push(r.flow_id.clone());
                    cur = r.parent.clone();
                }
                None => break,
            }
        }
        if chain.len() >= MAX_SUB_DEPTH {
            return Err(NodeError::config(format!("sub-flows are nested more than {MAX_SUB_DEPTH} deep")));
        }
        if chain.iter().any(|f| f == flow_ref) {
            return Err(NodeError::config(format!("`{flow_ref}` would run itself (it is already running higher up)")));
        }
        let flow = inner.source.get_published(flow_ref).ok_or_else(|| NodeError::config(format!("sub-flow `{flow_ref}` has no published version")))?;
        let issues = crate::validate::validate_flow(&flow, &inner.reg);
        if crate::validate::has_errors(&issues) {
            return Err(NodeError::config(format!("sub-flow `{flow_ref}` has problems and cannot run")));
        }
        let inputs = bind_inputs(&flow, &inputs).map_err(NodeError::config)?;
        let now = inner.now();
        let id = format!("r{}", inner.next_id.fetch_add(1, Ordering::SeqCst));
        let mut run = blank_run(&id, flow, inputs, TriggerInfo { kind: "sub-flow".into(), by: self.parent.clone(), ..Default::default() }, now);
        run.parent = Some(self.parent.clone());
        let child_cancel = Cancel::new();
        inner.state.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Slot { run, tx: None, cancel: child_cancel, redactor: Arc::new(Redactor::new()) });
        inner.persist(&id);
        let (tx, rx) = channel();
        inner.state.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&id).map(|s| s.tx = Some(tx.clone()));
        crate::driver::drive(inner.clone(), id.clone(), rx, tx, Some(cancel.clone()));
        let r = inner.get(&id).ok_or_else(|| NodeError::new("the sub-flow's run is gone"))?;
        match r.status {
            RunStatus::Succeeded => Ok(r.outputs.clone()),
            RunStatus::Cancelled => Err(NodeError::cancelled()),
            _ => Err(NodeError::new(format!("sub-flow `{flow_ref}` failed: {}", r.error))),
        }
    }
}
