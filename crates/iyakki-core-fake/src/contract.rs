//! In-memory fakes of the first contract domains (settings, vault, flows, consent), for tests of tiles and of remote UIs.
//! They enforce the same guards as a real machine would: scopes, local-only calls, idempotency, audit lines, event sequence numbers.

use iyakki_core_api::contract::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// A clock that moves one millisecond per reading, so event times are ordered and tests are exact.
#[derive(Default)]
struct Tick(AtomicU64);
impl Tick {
    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<String>>);
impl Audit {
    fn line(&self, ctx: &CallContext, call: &str, risk: Risk, outcome: &str) {
        self.0.lock().unwrap().push(ctx.audit_line(call, risk, outcome));
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn denied_audit<T>(audit: &Audit, ctx: &CallContext, call: &str, risk: Risk, r: CallResult<T>) -> CallResult<T> {
    if let Err(e) = &r {
        audit.line(ctx, call, risk, &format!("refused: {}", e.code.as_str()));
    }
    r
}

// ---------------------------------------------------------------- settings

pub struct FakeSettings {
    specs: Vec<SettingSpec>,
    values: Mutex<HashMap<String, SettingValue>>,
    log: Mutex<EventLog<SettingEvent>>,
    idem: Mutex<HashMap<IdempotencyKey, (String, SettingValue, u64)>>,
    audit: Audit,
    tick: Tick,
}

impl FakeSettings {
    pub fn new(specs: Vec<SettingSpec>) -> FakeSettings {
        let values = specs.iter().map(|s| (s.key.clone(), s.default.clone())).collect();
        FakeSettings { specs, values: Mutex::new(values), log: Mutex::new(EventLog::new(256)), idem: Default::default(), audit: Audit::default(), tick: Tick::default() }
    }
    /// Three keys: a bool anyone with `settings:write` changes, a choice, and a `LocalOnly` one a remote caller can never write.
    pub fn sample() -> FakeSettings {
        let spec = |key: &str, kind, default, risk| SettingSpec {
            key: key.into(),
            kind,
            default,
            read_scope: Scope::SETTINGS_READ.into(),
            write_scope: Scope::SETTINGS_WRITE.into(),
            write_risk: risk,
            label_key: format!("settings.{key}"),
        };
        FakeSettings::new(vec![
            spec("display.dark", SettingKind::Bool, SettingValue::Bool(false), Risk::Change),
            spec("voice.speed", SettingKind::Choice { options: vec!["slow".into(), "normal".into(), "fast".into()] }, SettingValue::Text("normal".into()), Risk::Change),
            spec("security.vault_timeout", SettingKind::Int { min: 1, max: 120 }, SettingValue::Int(15), Risk::LocalOnly),
        ])
    }
    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    fn spec(&self, key: &str) -> CallResult<&SettingSpec> {
        self.specs.iter().find(|s| s.key == key).ok_or_else(|| CallError::new(ErrorCode::NotFound, "settings.unknown_key", &format!("There is no setting called {key}.")))
    }
}

impl Settings for FakeSettings {
    fn schema(&self, ctx: &CallContext) -> CallResult<Vec<SettingSpec>> {
        denied_audit(&self.audit, ctx, "settings.schema", Risk::Read, guard(ctx, Scope::SETTINGS_READ, Risk::Read))?;
        Ok(self.specs.clone())
    }
    fn get(&self, ctx: &CallContext, key: &str) -> CallResult<SettingValue> {
        let spec = self.spec(key)?;
        denied_audit(&self.audit, ctx, "settings.get", Risk::Read, guard(ctx, &spec.read_scope, Risk::Read))?;
        Ok(self.values.lock().unwrap()[key].clone())
    }
    fn set(&self, ctx: &CallContext, key: &str, value: SettingValue, idem: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        let spec = self.spec(key)?;
        let scope = if spec.write_risk == Risk::LocalOnly { Scope::LOCAL } else { spec.write_scope.as_str() };
        denied_audit(&self.audit, ctx, "settings.set", spec.write_risk, guard(ctx, scope, spec.write_risk))?;
        if !spec.kind.accepts(&value) {
            let e = CallError::new(ErrorCode::InvalidInput, "settings.bad_value", &format!("That value does not fit {key}.")).hint("Look at the schema for the allowed values.");
            self.audit.line(ctx, "settings.set", spec.write_risk, "refused: invalid_input");
            return Err(e);
        }
        if let Some(k) = idem {
            if let Some((ek, ev, seq)) = self.idem.lock().unwrap().get(k) {
                if ek == key && *ev == value {
                    return Ok(SettingChange { key: key.into(), value, seq: *seq, repeated: true });
                }
                return Err(CallError::new(ErrorCode::Conflict, "core.idempotency_conflict", "That key was already used for a different request."));
            }
        }
        self.values.lock().unwrap().insert(key.into(), value.clone());
        let seq = self.log.lock().unwrap().push("changed", self.tick.now(), SettingEvent { key: key.into(), value: value.clone(), by: ctx.identity.user.clone() });
        if let Some(k) = idem {
            self.idem.lock().unwrap().insert(k.clone(), (key.into(), value.clone(), seq));
        }
        // The audit line names the key, never the value.
        self.audit.line(ctx, &format!("settings.set {key}"), spec.write_risk, "ok");
        Ok(SettingChange { key: key.into(), value, seq, repeated: false })
    }
    fn watch(&self, ctx: &CallContext, key: Option<&str>, resume: Resume, limit: usize) -> CallResult<EventBatch<SettingEvent>> {
        denied_audit(&self.audit, ctx, "settings.watch", Risk::Read, guard(ctx, Scope::SETTINGS_READ, Risk::Read))?;
        let mut b = self.log.lock().unwrap().read(resume, Some("changed"), limit);
        if let Some(k) = key {
            b.events.retain(|e| e.body.key == k);
        }
        Ok(b)
    }
}

// ---------------------------------------------------------------- vault

pub struct FakeVault {
    login: String,
    locked: Mutex<bool>,
    secrets: HashMap<String, String>,
    /// action name -> secret name
    actions: HashMap<String, String>,
    used: Mutex<Vec<(String, String)>>,
    audit: Audit,
}

impl FakeVault {
    pub fn new(login: &str) -> FakeVault {
        FakeVault { login: login.into(), locked: Mutex::new(true), secrets: HashMap::new(), actions: HashMap::new(), used: Mutex::new(vec![]), audit: Audit::default() }
    }
    /// Seed a secret and the one named action that may use it (test setup, not part of the contract).
    pub fn with_secret(mut self, secret_name: &str, value: &str, action: &str) -> FakeVault {
        self.secrets.insert(secret_name.into(), value.into());
        self.actions.insert(action.into(), secret_name.into());
        self
    }
    /// (action, secret name) pairs that were performed locally.
    pub fn used(&self) -> Vec<(String, String)> {
        self.used.lock().unwrap().clone()
    }
    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    fn status_now(&self) -> VaultStatus {
        VaultStatus { locked: *self.locked.lock().unwrap(), entries: self.secrets.len() as u32 }
    }
}

impl Vault for FakeVault {
    fn status(&self, ctx: &CallContext) -> CallResult<VaultStatus> {
        denied_audit(&self.audit, ctx, "vault.status", Risk::Read, guard(ctx, Scope::VAULT_STATUS, Risk::Read))?;
        Ok(self.status_now())
    }
    fn unlock(&self, seat: &LocalSeat, login_password: &str) -> CallResult<VaultStatus> {
        if login_password != self.login {
            self.audit.0.lock().unwrap().push(format!("{} via local: vault.unlock [local_only] refused: denied", seat.user()));
            return Err(CallError::new(ErrorCode::Denied, "vault.wrong_login", "That is not the login password."));
        }
        *self.locked.lock().unwrap() = false;
        self.audit.0.lock().unwrap().push(format!("{} via local: vault.unlock [local_only] ok", seat.user()));
        Ok(self.status_now())
    }
    fn lock(&self, ctx: &CallContext) -> CallResult<VaultStatus> {
        denied_audit(&self.audit, ctx, "vault.lock", Risk::Change, guard(ctx, Scope::VAULT_LOCK, Risk::Change))?;
        *self.locked.lock().unwrap() = true;
        self.audit.line(ctx, "vault.lock", Risk::Change, "ok");
        Ok(self.status_now())
    }
    fn use_secret(&self, ctx: &CallContext, action: &NamedAction, _idem: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
        denied_audit(&self.audit, ctx, "vault.use_secret", Risk::Change, guard(ctx, Scope::VAULT_USE, Risk::Change))?;
        if *self.locked.lock().unwrap() {
            self.audit.line(ctx, "vault.use_secret", Risk::Change, "refused: locked");
            return Err(CallError::new(ErrorCode::Locked, "vault.locked", "The vault is locked.").hint("Ask the person at the machine to unlock it."));
        }
        let Some(secret) = self.actions.get(&action.0) else {
            return Err(CallError::new(ErrorCode::NotFound, "vault.unknown_action", "No such action is registered."));
        };
        self.used.lock().unwrap().push((action.0.clone(), secret.clone()));
        self.audit.line(ctx, &format!("vault.use_secret {}", action.0), Risk::Change, "ok");
        Ok(ActionOutcome { ok: true, summary: format!("{} done", action.0) })
    }
}

// ---------------------------------------------------------------- consent

#[derive(Default)]
pub struct FakeConsents {
    open: Mutex<Vec<ConsentRequest>>,
    answered: Mutex<Vec<(String, ConsentAnswer, String)>>,
}

impl FakeConsents {
    pub fn new() -> FakeConsents {
        FakeConsents::default()
    }
    /// A tile or a flow raises a question (test setup).
    pub fn ask(&self, kind: ConsentKind, subject: &str, text: &str) -> String {
        let mut open = self.open.lock().unwrap();
        let id = format!("c{}", open.len() + self.answered.lock().unwrap().len() + 1);
        open.push(ConsentRequest { id: id.clone(), kind, subject: subject.into(), text_key: format!("consent.{kind:?}").to_lowercase(), text: text.into(), created_ms: 1, expires_ms: 60_000 });
        id
    }
    pub fn answered(&self) -> Vec<(String, ConsentAnswer, String)> {
        self.answered.lock().unwrap().clone()
    }
}

impl Consents for FakeConsents {
    fn pending(&self, ctx: &CallContext) -> CallResult<Vec<ConsentRequest>> {
        guard(ctx, Scope::CONSENT_READ, Risk::Read)?;
        Ok(self.open.lock().unwrap().clone())
    }
    fn answer(&self, seat: &LocalSeat, id: &str, answer: ConsentAnswer) -> CallResult<()> {
        let mut open = self.open.lock().unwrap();
        let Some(i) = open.iter().position(|r| r.id == id) else {
            return Err(CallError::new(ErrorCode::NotFound, "consent.unknown", "No such question is waiting."));
        };
        open.remove(i);
        self.answered.lock().unwrap().push((id.into(), answer, seat.user().into()));
        Ok(())
    }
}

// ---------------------------------------------------------------- flows

/// A canned flow: optionally asks one approval right after it starts.
#[derive(Clone)]
pub struct FlowScript {
    pub info: FlowInfo,
    pub approval: Option<Approval>,
}

struct Run {
    flow: String,
    state: RunState,
    pending: Option<Approval>,
    step: String,
}

pub struct FakeFlows {
    scripts: Vec<FlowScript>,
    runs: Mutex<HashMap<RunId, Run>>,
    idem: Mutex<HashMap<IdempotencyKey, (String, RunId)>>,
    log: Mutex<EventLog<FlowEvent>>,
    started: AtomicU64,
    audit: Audit,
    tick: Tick,
}

impl FakeFlows {
    pub fn new(scripts: Vec<FlowScript>) -> FakeFlows {
        FakeFlows { scripts, runs: Default::default(), idem: Default::default(), log: Mutex::new(EventLog::new(512)), started: AtomicU64::new(0), audit: Audit::default(), tick: Tick::default() }
    }
    pub fn sample() -> FakeFlows {
        let info = |id: &str, name: &str| FlowInfo { id: id.into(), name: name.into(), description: format!("{name} flow") };
        FakeFlows::new(vec![
            FlowScript { info: info("tidy", "Tidy downloads"), approval: None },
            FlowScript { info: info("ask", "Ask before sending"), approval: Some(Approval { id: "a1".into(), text_key: "flows.send".into(), text: "Send the summary?".into(), local_only: false }) },
            FlowScript { info: info("look", "Look with the camera"), approval: Some(Approval { id: "a2".into(), text_key: "flows.camera".into(), text: "Use the camera?".into(), local_only: true }) },
        ])
    }
    /// How many runs were really started (an idempotent repeat does not count).
    pub fn runs_started(&self) -> u64 {
        self.started.load(Ordering::Relaxed)
    }
    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    /// Test helper: the run ends well.
    pub fn finish(&self, run: &RunId) {
        if let Some(r) = self.runs.lock().unwrap().get_mut(run) {
            r.state = RunState::Succeeded;
            r.pending = None;
        }
        self.log.lock().unwrap().push("finished", self.tick.now(), FlowEvent::Finished { run: run.clone(), state: RunState::Succeeded });
    }
    /// Test helper: a camera frame is handed to the run.
    pub fn hand_frame(&self, run: &RunId, stream: StreamHandle) {
        self.log.lock().unwrap().push("frame", self.tick.now(), FlowEvent::Frame { run: run.clone(), stream });
    }
    fn status_of(&self, run: &RunId) -> CallResult<RunStatus> {
        let runs = self.runs.lock().unwrap();
        let r = runs.get(run).ok_or_else(|| CallError::new(ErrorCode::NotFound, "flows.unknown_run", "No such run."))?;
        Ok(RunStatus { run: run.clone(), flow: r.flow.clone(), state: r.state, pending_approval: r.pending.clone(), step: r.step.clone() })
    }
}

fn run_of(e: &FlowEvent) -> &RunId {
    match e {
        FlowEvent::Started { run, .. } | FlowEvent::Step { run, .. } | FlowEvent::ApprovalNeeded { run, .. } | FlowEvent::ApprovalAnswered { run, .. } | FlowEvent::Frame { run, .. } | FlowEvent::Finished { run, .. } => run,
    }
}

impl Flows for FakeFlows {
    fn list(&self, ctx: &CallContext) -> CallResult<Vec<FlowInfo>> {
        denied_audit(&self.audit, ctx, "flows.list", Risk::Read, guard(ctx, Scope::FLOWS_READ, Risk::Read))?;
        Ok(self.scripts.iter().map(|s| s.info.clone()).collect())
    }
    fn start(&self, ctx: &CallContext, flow: &str, idem: &IdempotencyKey) -> CallResult<RunId> {
        denied_audit(&self.audit, ctx, "flows.start", Risk::Change, guard(ctx, Scope::FLOWS_RUN, Risk::Change))?;
        let script = self.scripts.iter().find(|s| s.info.id == flow).ok_or_else(|| CallError::new(ErrorCode::NotFound, "flows.unknown_flow", "No such flow."))?;
        let mut idems = self.idem.lock().unwrap();
        if let Some((f, run)) = idems.get(idem) {
            if f == flow {
                self.audit.line(ctx, &format!("flows.start {flow}"), Risk::Change, "repeat: same run");
                return Ok(run.clone());
            }
            return Err(CallError::new(ErrorCode::Conflict, "core.idempotency_conflict", "That key was already used for a different request."));
        }
        let n = self.started.fetch_add(1, Ordering::Relaxed) + 1;
        let run = RunId(format!("run-{n}"));
        let (state, pending) = match &script.approval {
            Some(a) => (RunState::WaitingApproval, Some(a.clone())),
            None => (RunState::Running, None),
        };
        self.runs.lock().unwrap().insert(run.clone(), Run { flow: flow.into(), state, pending: pending.clone(), step: "begin".into() });
        {
            let mut log = self.log.lock().unwrap();
            log.push("started", self.tick.now(), FlowEvent::Started { run: run.clone(), flow: flow.into() });
            log.push("step", self.tick.now(), FlowEvent::Step { run: run.clone(), name: "begin".into() });
            if let Some(a) = pending {
                log.push("approval_needed", self.tick.now(), FlowEvent::ApprovalNeeded { run: run.clone(), approval: a });
            }
        }
        idems.insert(idem.clone(), (flow.into(), run.clone()));
        self.audit.line(ctx, &format!("flows.start {flow}"), Risk::Change, "ok");
        Ok(run)
    }
    fn stop(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus> {
        denied_audit(&self.audit, ctx, "flows.stop", Risk::Change, guard(ctx, Scope::FLOWS_RUN, Risk::Change))?;
        let st = self.status_of(run)?;
        if !st.state.is_finished() {
            let mut runs = self.runs.lock().unwrap();
            let r = runs.get_mut(run).unwrap();
            r.state = RunState::Stopped;
            r.pending = None;
            self.log.lock().unwrap().push("finished", self.tick.now(), FlowEvent::Finished { run: run.clone(), state: RunState::Stopped });
        }
        self.audit.line(ctx, "flows.stop", Risk::Change, "ok");
        self.status_of(run)
    }
    fn status(&self, ctx: &CallContext, run: &RunId) -> CallResult<RunStatus> {
        denied_audit(&self.audit, ctx, "flows.status", Risk::Read, guard(ctx, Scope::FLOWS_READ, Risk::Read))?;
        self.status_of(run)
    }
    fn events(&self, ctx: &CallContext, run: Option<&RunId>, resume: Resume, limit: usize) -> CallResult<EventBatch<FlowEvent>> {
        denied_audit(&self.audit, ctx, "flows.events", Risk::Read, guard(ctx, Scope::FLOWS_READ, Risk::Read))?;
        let mut b = self.log.lock().unwrap().read(resume, None, limit);
        if let Some(r) = run {
            b.events.retain(|e| run_of(&e.body) == r);
        }
        Ok(b)
    }
    fn approve(&self, ctx: &CallContext, run: &RunId, approval_id: &str, allow: bool) -> CallResult<RunStatus> {
        denied_audit(&self.audit, ctx, "flows.approve", Risk::AskPerson, guard(ctx, Scope::FLOWS_APPROVE, Risk::AskPerson))?;
        let st = self.status_of(run)?;
        let Some(a) = st.pending_approval.filter(|a| a.id == approval_id) else {
            return Err(CallError::new(ErrorCode::NotFound, "flows.no_such_approval", "That question is not waiting."));
        };
        if a.local_only && !ctx.is_local() {
            self.audit.line(ctx, "flows.approve", Risk::AskPerson, "refused: local_only");
            return Err(CallError::new(ErrorCode::NeedsConsent, "flows.approval_local_only", "Only a person at the machine can answer this question.").hint("Ask the person at the machine."));
        }
        {
            let mut runs = self.runs.lock().unwrap();
            let r = runs.get_mut(run).unwrap();
            r.pending = None;
            r.state = if allow { RunState::Running } else { RunState::Stopped };
        }
        let mut log = self.log.lock().unwrap();
        log.push("approval_answered", self.tick.now(), FlowEvent::ApprovalAnswered { run: run.clone(), approval_id: a.id, allowed: allow, by: ctx.identity.user.clone() });
        if !allow {
            log.push("finished", self.tick.now(), FlowEvent::Finished { run: run.clone(), state: RunState::Stopped });
        }
        drop(log);
        self.audit.line(ctx, "flows.approve", Risk::AskPerson, if allow { "allowed" } else { "denied" });
        self.status_of(run)
    }
}
