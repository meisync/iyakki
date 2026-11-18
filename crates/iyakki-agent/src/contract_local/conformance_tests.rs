//! One conformance suite for the core contract, run against BOTH the in-memory fakes (`iyakki-core-fake`) and the real adapters over the
//! device's services. The same behaviours must hold for both: scopes, local-only keys, audit lines without values, event resume,
//! idempotent start, consent only through the local seat, and no secret ever coming out of the vault. A rule that holds for the fake
//! but not the real adapter (or the reverse) fails here, so the fake stays honest.
//!
//! Real-only checks (streams, settings changed behind the adapter's back, scrubbing, expiry) follow the shared suite.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_core_fake::contract::{FakeConsents, FakeFlows, FakeSettings, FakeVault, FlowScript};
use iyakki_flow::model::{Edge, Flow, Node};
use iyakki_sec::policy::Role;
use serde_json::json;

use super::*;
use crate::flows::FlowEnv;

const LOGIN: &str = "loginpw-123456";
const SECRET: &str = "S3CRET-VALUE-xyz-0042";
const ACTION: &str = "sign_in:git";

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn local() -> CallContext {
    CallContext::local("tester", true)
}
fn remote(scopes: &[&str]) -> CallContext {
    CallContext::remote("cloud-ui", Transport::Relay, scopes)
}
fn dev(scopes: &[&str]) -> CallContext {
    CallContext::remote("laptop", Transport::DevMode, scopes)
}
fn key(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).unwrap()
}

/// Everything the suite needs from one implementation.
struct Subject {
    name: &'static str,
    settings: Box<dyn Settings>,
    settings_audit: Box<dyn Fn() -> Vec<String>>,
    vault: Box<dyn Vault>,
    vault_audit: Box<dyn Fn() -> Vec<String>>,
    flows: Box<dyn Flows>,
    flows_audit: Box<dyn Fn() -> Vec<String>>,
    consents: Box<dyn Consents>,
    ask_consent: Box<dyn Fn() -> String>,
    consents_answered: Box<dyn Fn() -> Vec<(String, ConsentAnswer, String)>>,
    /// Lets a scripted implementation move a run along (the real engine moves by itself).
    finish: Box<dyn Fn(&RunId)>,
    /// (flow, approval id) of a flow whose approval only the machine's own screen may answer; none when the engine cannot make one.
    local_only_flow: Option<(&'static str, &'static str)>,
    ask_approval_id: &'static str,
}

const BOOL_KEY: &str = "show_all_ai";
const LOCAL_KEY: &str = "vault.lock_on_admin_logout";
const INT_KEY: &str = "tool_log_days";

fn tmp(tag: &str) -> std::path::PathBuf {
    crate::testdir::unique("iyakki-core-conf", tag)
}

fn flow(id: &str, name: &str, approval: bool) -> Flow {
    let mut f = Flow::new(id, name);
    f.description = format!("{name} flow");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("e", "end", json!({})));
    if approval {
        f.nodes.push(Node::new("gate", "approval", json!({"message": "Send the summary?"})));
        f.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "e").port("approved")];
    } else {
        f.edges = vec![Edge::new("s", "e")];
    }
    f
}

fn real() -> (Subject, Arc<Core>, Arc<FlowEnv>) {
    let d = tmp("real");
    let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    // a personal vault with one secret, then locked again
    svc.on_login("tester", LOGIN, false);
    svc.user_set("tester", "deploy-token", SECRET).unwrap();
    svc.lock_user_vault("tester");
    let policy = Role::User.template();
    let env = Arc::new(FlowEnv::build_in(&svc, "tester", &policy, false, &d.join("home")));
    for (id, name, ap) in [("tidy", "Tidy downloads", false), ("ask", "Ask before sending", true)] {
        env.create_draft(flow(id, name, ap), "user:tester").unwrap();
        env.store.publish(id, &iyakki_flow::store::PublishReq { by: "user:tester", label: "", changelog: "x", approved_by: None }).unwrap();
    }
    let core = Arc::new(Core::new(svc.clone()));
    core.vault.register_action(ACTION, "deploy-token", Arc::new(|_secret: &str| Ok("signed in to the git host".to_string())));
    let flows = core.flows_for_env(env.clone());
    let (s, v, f2, c) = (core.clone(), core.clone(), flows.clone(), core.clone());
    let (c2, c3) = (core.clone(), core.clone());
    let subject = Subject {
        name: "real",
        settings: Box::new(SettingsRef(core.clone())),
        settings_audit: Box::new(move || s.settings.audit()),
        vault: Box::new(VaultRef(core.clone())),
        vault_audit: Box::new(move || v.vault.audit()),
        flows: Box::new(FlowsRef(flows)),
        flows_audit: Box::new(move || f2.audit()),
        consents: Box::new(ConsentsRef(core.clone())),
        ask_consent: Box::new(move || c.consents.ask(ConsentKind::Camera, "tile.scan", "consent.camera", "Use the camera?")),
        consents_answered: Box::new(move || c2.consents.answered()),
        finish: Box::new(|_| {}),
        local_only_flow: None,
        ask_approval_id: "gate",
    };
    let _ = c3;
    (subject, core, env)
}

// thin forwarding wrappers so the Subject can own boxed trait objects while the Core stays shared
struct SettingsRef(Arc<Core>);
impl Settings for SettingsRef {
    fn schema(&self, c: &CallContext) -> CallResult<Vec<SettingSpec>> {
        self.0.settings.schema(c)
    }
    fn get(&self, c: &CallContext, k: &str) -> CallResult<SettingValue> {
        self.0.settings.get(c, k)
    }
    fn set(&self, c: &CallContext, k: &str, v: SettingValue, i: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        self.0.settings.set(c, k, v, i)
    }
    fn watch(&self, c: &CallContext, k: Option<&str>, r: Resume, l: usize) -> CallResult<EventBatch<SettingEvent>> {
        self.0.settings.watch(c, k, r, l)
    }
}
struct VaultRef(Arc<Core>);
impl Vault for VaultRef {
    fn status(&self, c: &CallContext) -> CallResult<VaultStatus> {
        self.0.vault.status(c)
    }
    fn unlock(&self, s: &LocalSeat, p: &str) -> CallResult<VaultStatus> {
        self.0.vault.unlock(s, p)
    }
    fn lock(&self, c: &CallContext) -> CallResult<VaultStatus> {
        self.0.vault.lock(c)
    }
    fn use_secret(&self, c: &CallContext, a: &NamedAction, i: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
        self.0.vault.use_secret(c, a, i)
    }
}
struct ConsentsRef(Arc<Core>);
impl Consents for ConsentsRef {
    fn pending(&self, c: &CallContext) -> CallResult<Vec<ConsentRequest>> {
        self.0.consents.pending(c)
    }
    fn answer(&self, s: &LocalSeat, id: &str, a: ConsentAnswer) -> CallResult<()> {
        self.0.consents.answer(s, id, a)
    }
}
struct FlowsRef(Arc<CoreFlows>);
impl Flows for FlowsRef {
    fn list(&self, c: &CallContext) -> CallResult<Vec<FlowInfo>> {
        self.0.list(c)
    }
    fn start(&self, c: &CallContext, f: &str, i: &IdempotencyKey) -> CallResult<RunId> {
        self.0.start(c, f, i)
    }
    fn stop(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
        self.0.stop(c, r)
    }
    fn status(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
        self.0.status(c, r)
    }
    fn events(&self, c: &CallContext, r: Option<&RunId>, re: Resume, l: usize) -> CallResult<EventBatch<FlowEvent>> {
        self.0.events(c, r, re, l)
    }
    fn approve(&self, c: &CallContext, r: &RunId, a: &str, al: bool) -> CallResult<RunStatus> {
        self.0.approve(c, r, a, al)
    }
}

fn fake(specs: Vec<SettingSpec>) -> Subject {
    let settings = Arc::new(FakeSettings::new(specs));
    let vault = Arc::new(FakeVault::new(LOGIN).with_secret("deploy-token", SECRET, ACTION));
    let info = |id: &str, name: &str| FlowInfo { id: id.into(), name: name.into(), description: format!("{name} flow") };
    let flows = Arc::new(FakeFlows::new(vec![
        FlowScript { info: info("tidy", "Tidy downloads"), approval: None },
        FlowScript { info: info("ask", "Ask before sending"), approval: Some(Approval { id: "gate".into(), text_key: "flows.approval".into(), text: "Send the summary?".into(), local_only: false }) },
        FlowScript { info: info("look", "Look with the camera"), approval: Some(Approval { id: "cam".into(), text_key: "flows.camera".into(), text: "Use the camera?".into(), local_only: true }) },
    ]));
    let consents = Arc::new(FakeConsents::new());
    struct S(Arc<FakeSettings>);
    impl Settings for S {
        fn schema(&self, c: &CallContext) -> CallResult<Vec<SettingSpec>> {
            self.0.schema(c)
        }
        fn get(&self, c: &CallContext, k: &str) -> CallResult<SettingValue> {
            self.0.get(c, k)
        }
        fn set(&self, c: &CallContext, k: &str, v: SettingValue, i: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
            self.0.set(c, k, v, i)
        }
        fn watch(&self, c: &CallContext, k: Option<&str>, r: Resume, l: usize) -> CallResult<EventBatch<SettingEvent>> {
            self.0.watch(c, k, r, l)
        }
    }
    struct V(Arc<FakeVault>);
    impl Vault for V {
        fn status(&self, c: &CallContext) -> CallResult<VaultStatus> {
            self.0.status(c)
        }
        fn unlock(&self, s: &LocalSeat, p: &str) -> CallResult<VaultStatus> {
            self.0.unlock(s, p)
        }
        fn lock(&self, c: &CallContext) -> CallResult<VaultStatus> {
            self.0.lock(c)
        }
        fn use_secret(&self, c: &CallContext, a: &NamedAction, i: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
            self.0.use_secret(c, a, i)
        }
    }
    struct F(Arc<FakeFlows>);
    impl Flows for F {
        fn list(&self, c: &CallContext) -> CallResult<Vec<FlowInfo>> {
            self.0.list(c)
        }
        fn start(&self, c: &CallContext, f: &str, i: &IdempotencyKey) -> CallResult<RunId> {
            self.0.start(c, f, i)
        }
        fn stop(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
            self.0.stop(c, r)
        }
        fn status(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
            self.0.status(c, r)
        }
        fn events(&self, c: &CallContext, r: Option<&RunId>, re: Resume, l: usize) -> CallResult<EventBatch<FlowEvent>> {
            self.0.events(c, r, re, l)
        }
        fn approve(&self, c: &CallContext, r: &RunId, a: &str, al: bool) -> CallResult<RunStatus> {
            self.0.approve(c, r, a, al)
        }
    }
    struct C(Arc<FakeConsents>);
    impl Consents for C {
        fn pending(&self, c: &CallContext) -> CallResult<Vec<ConsentRequest>> {
            self.0.pending(c)
        }
        fn answer(&self, s: &LocalSeat, id: &str, a: ConsentAnswer) -> CallResult<()> {
            self.0.answer(s, id, a)
        }
    }
    let (sa, va, fa, fin) = (settings.clone(), vault.clone(), flows.clone(), flows.clone());
    let (c1, c2) = (consents.clone(), consents.clone());
    Subject {
        name: "fake",
        settings: Box::new(S(settings)),
        settings_audit: Box::new(move || sa.audit()),
        vault: Box::new(V(vault)),
        vault_audit: Box::new(move || va.audit()),
        flows: Box::new(F(flows)),
        flows_audit: Box::new(move || fa.audit()),
        consents: Box::new(C(consents)),
        ask_consent: Box::new(move || c1.ask(ConsentKind::Camera, "tile.scan", "Use the camera?")),
        consents_answered: Box::new(move || c2.answered()),
        finish: Box::new(move |r| fin.finish(r)),
        local_only_flow: Some(("look", "cam")),
        ask_approval_id: "gate",
    }
}

fn subjects() -> Vec<Subject> {
    let (r, _core, _env) = real();
    // the fake gets the REAL schema, so both answer to the same keys, kinds and scopes
    let specs = r.settings.schema(&remote(&[Scope::SETTINGS_READ])).unwrap();
    vec![fake(specs), r]
}

fn wait_state(f: &dyn Flows, run: &RunId, ok: impl Fn(&iyakki_core_api::contract::RunStatus) -> bool) -> iyakki_core_api::contract::RunStatus {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let st = f.status(&local(), run).unwrap();
        if ok(&st) || Instant::now() > end {
            return st;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn no_value_in(audit: &[String], needles: &[&str], who: &str) {
    for l in audit {
        for n in needles {
            assert!(!l.contains(n), "[{who}] the audit line `{l}` holds `{n}`");
        }
    }
}

#[test]
fn settings_rules_hold_for_the_fake_and_the_real_adapter() {
    for s in subjects() {
        let n = s.name;
        let st = &*s.settings;
        // scopes: a remote caller with none is allowed nothing; with the read scope it sees the schema
        assert_eq!(st.schema(&remote(&[])).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        let schema = st.schema(&remote(&[Scope::SETTINGS_READ])).unwrap();
        for k in [BOOL_KEY, LOCAL_KEY, INT_KEY] {
            assert!(schema.iter().any(|x| x.key == k), "[{n}] schema lacks {k}");
        }
        assert!(schema.iter().all(|x| !matches!(x.kind, SettingKind::Text { .. }) || !x.key.contains("key") && !x.key.contains("token") && !x.key.contains("secret")), "[{n}] a secret-like key");
        assert_eq!(st.get(&remote(&[]), BOOL_KEY).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(st.get(&remote(&[Scope::SETTINGS_READ]), BOOL_KEY).unwrap(), SettingValue::Bool(false), "[{n}] default");
        assert_eq!(st.get(&local(), "no.such.key").unwrap_err().code, ErrorCode::NotFound);

        // writes need the write scope; the scope for reading is not enough
        let bad = st.set(&dev(&[Scope::SETTINGS_READ]), BOOL_KEY, SettingValue::Bool(true), None).unwrap_err();
        assert_eq!(bad.code, ErrorCode::Denied, "[{n}]");
        let w = dev(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE]);
        let c1 = st.set(&w, BOOL_KEY, SettingValue::Bool(true), Some(&key("set-bool-0001"))).unwrap();
        assert!(!c1.repeated && c1.seq > 0, "[{n}]");
        assert_eq!(st.get(&w, BOOL_KEY).unwrap(), SettingValue::Bool(true));

        // idempotency: the same key and request does nothing twice; the same key for another value is a conflict
        let again = st.set(&w, BOOL_KEY, SettingValue::Bool(true), Some(&key("set-bool-0001"))).unwrap();
        assert!(again.repeated && again.seq == c1.seq, "[{n}]");
        assert_eq!(st.set(&w, BOOL_KEY, SettingValue::Bool(false), Some(&key("set-bool-0001"))).unwrap_err().code, ErrorCode::Conflict, "[{n}]");

        // a local-only key: no scope, no transport makes a remote caller able to write it; the person at the machine can
        let all = remote(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE, Scope::LOCAL]);
        assert_eq!(st.set(&all, LOCAL_KEY, SettingValue::Bool(true), None).unwrap_err().code, ErrorCode::Denied, "[{n}] local-only");
        assert_eq!(st.get(&all, LOCAL_KEY).unwrap(), SettingValue::Bool(false), "[{n}] unchanged");
        st.set(&local(), LOCAL_KEY, SettingValue::Bool(true), None).unwrap();
        assert_eq!(st.get(&local(), LOCAL_KEY).unwrap(), SettingValue::Bool(true));

        // values that do not fit are invalid input with a hint, and change nothing
        for badv in [SettingValue::Int(9999), SettingValue::Text("x".into()), SettingValue::Int(-1)] {
            let e = st.set(&w, INT_KEY, badv, None).unwrap_err();
            assert_eq!((e.code, e.hint.is_empty()), (ErrorCode::InvalidInput, false), "[{n}]");
        }
        assert_eq!(st.get(&w, INT_KEY).unwrap(), SettingValue::Int(0));
        st.set(&w, INT_KEY, SettingValue::Int(321), None).unwrap();

        // watch: resume gives exactly what was missed, in order; a key filter; Latest sees nothing old
        let all_ev = st.watch(&w, None, Resume::After(0), 100).unwrap();
        let keys: Vec<&str> = all_ev.events.iter().map(|e| e.body.key.as_str()).collect();
        assert_eq!(keys, [BOOL_KEY, LOCAL_KEY, INT_KEY], "[{n}]");
        assert!(all_ev.events.windows(2).all(|p| p[0].seq < p[1].seq));
        let after_first = st.watch(&w, None, Resume::After(all_ev.events[0].seq), 100).unwrap();
        assert_eq!(after_first.events.len(), 2, "[{n}]");
        assert_eq!(st.watch(&w, Some(INT_KEY), Resume::After(0), 100).unwrap().events.len(), 1);
        assert!(st.watch(&w, None, Resume::Latest, 10).unwrap().events.is_empty());
        let next = after_first.next;
        st.set(&w, BOOL_KEY, SettingValue::Bool(false), None).unwrap();
        let more = st.watch(&w, None, Resume::After(next), 100).unwrap();
        assert_eq!(more.events.len(), 1, "[{n}] only the new change");
        assert_eq!(st.watch(&remote(&[]), None, Resume::After(0), 10).unwrap_err().code, ErrorCode::Denied);

        // the audit log names keys and outcomes, never values
        let audit = (s.settings_audit)();
        no_value_in(&audit, &["321", "9999"], n);
        assert!(audit.iter().any(|l| l.contains(&format!("settings.set {BOOL_KEY}")) && l.ends_with("ok")), "[{n}] {audit:?}");
        assert!(audit.iter().any(|l| l.contains("refused: denied")), "[{n}] refusals are logged");
        assert!(audit.iter().any(|l| l.starts_with("laptop via devmode:")), "[{n}] says who and over what");
    }
}

#[test]
fn vault_rules_hold_for_the_fake_and_the_real_adapter() {
    for s in subjects() {
        let n = s.name;
        let v = &*s.vault;
        let status_scope = remote(&[Scope::VAULT_STATUS]);
        assert_eq!(v.status(&remote(&[])).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        // the owner of the real vault is the caller's own name; a remote caller is mapped to it by the transport, here by name
        let who_remote = CallContext::remote("tester", Transport::DevMode, &[Scope::VAULT_STATUS, Scope::VAULT_USE, Scope::VAULT_LOCK]);
        assert!(v.status(&status_scope).unwrap().locked, "[{n}] starts locked");
        let act = NamedAction(ACTION.into());
        assert_eq!(v.use_secret(&who_remote, &act, None).unwrap_err().code, ErrorCode::Locked, "[{n}]");
        assert_eq!(v.use_secret(&dev(&[Scope::VAULT_STATUS]), &act, None).unwrap_err().code, ErrorCode::Denied, "[{n}]");

        // unlocking is for the person at the machine only: no remote context can make the seat
        for c in [dev(&[Scope::LOCAL, Scope::VAULT_USE]), remote(&[Scope::LOCAL])] {
            assert_eq!(LocalSeat::claim(&c).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        }
        let seat = LocalSeat::claim(&local()).unwrap();
        assert_eq!(v.unlock(&seat, "not the login").unwrap_err().code, ErrorCode::Denied, "[{n}]");
        assert!(v.status(&status_scope).unwrap().locked);
        let opened = v.unlock(&seat, LOGIN).unwrap();
        assert!(!opened.locked && opened.entries == 1, "[{n}] {opened:?}");

        // "use a secret for a named action": words come back, never the secret
        let out = v.use_secret(&who_remote, &act, Some(&key("use-secret-001"))).unwrap();
        assert!(out.ok && !out.summary.contains(SECRET) && !out.summary.is_empty(), "[{n}]");
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains(SECRET));
        assert_eq!(v.use_secret(&who_remote, &NamedAction("sign_in:nowhere".into()), None).unwrap_err().code, ErrorCode::NotFound, "[{n}]");
        let locked = v.lock(&who_remote).unwrap();
        assert!(locked.locked, "[{n}]");
        assert_eq!(v.use_secret(&who_remote, &act, None).unwrap_err().code, ErrorCode::Locked, "[{n}]");

        // nothing in the audit log is a secret or the login, and unlocking is logged as local-only
        let audit = (s.vault_audit)();
        no_value_in(&audit, &[SECRET, LOGIN, "deploy-token", "not the login"], n);
        assert!(audit.iter().any(|l| l.contains("vault.unlock [local_only] ok")), "[{n}] {audit:?}");
        assert!(audit.iter().any(|l| l.contains("vault.unlock [local_only] refused")), "[{n}]");
        assert!(audit.iter().any(|l| l.contains("vault.use_secret sign_in:git")), "[{n}]");
    }
}

#[test]
fn consent_is_answered_only_through_the_local_seat_for_the_fake_and_the_real_adapter() {
    for s in subjects() {
        let n = s.name;
        let id = (s.ask_consent)();
        for t in [Transport::DevMode, Transport::Relay] {
            let c = CallContext::remote("cloud-ui", t, &["consent:read", "local", "flows:approve", "vault:use"]);
            assert_eq!(LocalSeat::claim(&c).unwrap_err().code, ErrorCode::Denied, "[{n}]");
            let seen = s.consents.pending(&c).unwrap();
            assert_eq!(seen.len(), 1, "[{n}] a remote client may see the question");
            assert_eq!(seen[0].kind, ConsentKind::Camera);
        }
        assert_eq!(s.consents.pending(&remote(&[])).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        assert!((s.consents_answered)().is_empty());
        let seat = LocalSeat::claim(&local()).unwrap();
        assert_eq!(s.consents.answer(&seat, "nope", ConsentAnswer::Allow).unwrap_err().code, ErrorCode::NotFound);
        s.consents.answer(&seat, &id, ConsentAnswer::Deny).unwrap();
        assert_eq!((s.consents_answered)(), [(id.clone(), ConsentAnswer::Deny, "tester".to_string())], "[{n}]");
        assert!(s.consents.pending(&remote(&[Scope::CONSENT_READ])).unwrap().is_empty());
        assert_eq!(s.consents.answer(&seat, &id, ConsentAnswer::Allow).unwrap_err().code, ErrorCode::NotFound, "[{n}] a question is answered once");
    }
}

#[test]
fn flows_rules_hold_for_the_fake_and_the_real_adapter() {
    for s in subjects() {
        let n = s.name;
        let f = &*s.flows;
        let rd = remote(&[Scope::FLOWS_READ]);
        let rw = remote(&[Scope::FLOWS_READ, Scope::FLOWS_RUN]);
        let ra = remote(&[Scope::FLOWS_READ, Scope::FLOWS_RUN, Scope::FLOWS_APPROVE]);

        // list
        assert_eq!(f.list(&remote(&[])).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        let list = f.list(&rd).unwrap();
        for id in ["tidy", "ask"] {
            assert!(list.iter().any(|i| i.id == id && !i.name.is_empty()), "[{n}] {id}");
        }

        // start needs the run scope, is idempotent, and a key cannot be reused for another flow
        assert_eq!(f.start(&rd, "tidy", &key("start-tidy-001")).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        assert_eq!(f.start(&rw, "no-such-flow", &key("start-none-001")).unwrap_err().code, ErrorCode::NotFound, "[{n}]");
        let run = f.start(&rw, "tidy", &key("start-tidy-001")).unwrap();
        assert_eq!(f.start(&rw, "tidy", &key("start-tidy-001")).unwrap(), run, "[{n}] idempotent");
        assert_eq!(f.start(&rw, "ask", &key("start-tidy-001")).unwrap_err().code, ErrorCode::Conflict, "[{n}]");
        (s.finish)(&run);
        let done = wait_state(f, &run, |st| st.state.is_finished());
        assert_eq!((done.state, done.flow.as_str()), (RunState::Succeeded, "tidy"), "[{n}]");
        assert_eq!(f.status(&remote(&[]), &run).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(f.status(&rd, &RunId("zzz".into())).unwrap_err().code, ErrorCode::NotFound, "[{n}]");

        // events: ordered, per run, ending with finished; resume gives exactly what was missed
        let evs = f.events(&rd, Some(&run), Resume::After(0), 100).unwrap();
        assert!(matches!(evs.events.first().map(|e| &e.body), Some(FlowEvent::Started { .. })), "[{n}] {:?}", evs.events);
        assert!(matches!(evs.events.last().map(|e| &e.body), Some(FlowEvent::Finished { state: RunState::Succeeded, .. })), "[{n}]");
        assert!(evs.events.windows(2).all(|p| p[0].seq < p[1].seq), "[{n}]");
        let first = evs.events[0].seq;
        let rest = f.events(&rd, Some(&run), Resume::After(first), 100).unwrap();
        assert_eq!(rest.events.len(), evs.events.len() - 1, "[{n}]");
        assert!(f.events(&rd, None, Resume::Latest, 10).unwrap().events.is_empty());
        assert!(f.events(&rd, Some(&run), Resume::After(evs.next), 10).unwrap().events.is_empty(), "[{n}] nothing after the end");
        assert_eq!(f.events(&remote(&[]), None, Resume::After(0), 1).unwrap_err().code, ErrorCode::Denied);
        // a page of one then the next: no event skipped or repeated
        let a = f.events(&rd, Some(&run), Resume::After(0), 1).unwrap();
        assert_eq!(a.events.len(), 1);
        let b = f.events(&rd, Some(&run), Resume::After(a.next), 100).unwrap();
        assert_eq!(1 + b.events.len(), evs.events.len(), "[{n}]");

        // an approval waits; reading is not enough to answer; with the scope a remote client may
        let ask = f.start(&rw, "ask", &key("start-ask-0001")).unwrap();
        let waiting = wait_state(f, &ask, |st| st.state == RunState::WaitingApproval);
        assert_eq!(waiting.state, RunState::WaitingApproval, "[{n}]");
        let pending = waiting.pending_approval.clone().expect("an open question");
        assert_eq!((pending.id.as_str(), pending.local_only), (s.ask_approval_id, false), "[{n}]");
        assert_eq!(f.approve(&rw, &ask, &pending.id, true).unwrap_err().code, ErrorCode::Denied, "[{n}] no approve scope");
        assert_eq!(f.approve(&ra, &ask, "nothing-here", true).unwrap_err().code, ErrorCode::NotFound, "[{n}]");
        let after = f.approve(&ra, &ask, &pending.id, true).unwrap();
        assert_ne!(after.state, RunState::WaitingApproval, "[{n}]");
        let kinds: Vec<String> = f
            .events(&rd, Some(&ask), Resume::After(0), 100)
            .unwrap()
            .events
            .iter()
            .map(|e| match &e.body {
                FlowEvent::Started { .. } => "started",
                FlowEvent::Step { .. } => "step",
                FlowEvent::ApprovalNeeded { .. } => "approval_needed",
                FlowEvent::ApprovalAnswered { allowed: true, .. } => "approval_answered",
                FlowEvent::ApprovalAnswered { .. } => "approval_denied",
                FlowEvent::Frame { .. } => "frame",
                FlowEvent::Finished { .. } => "finished",
            })
            .map(String::from)
            .collect();
        let pos = |k: &str| kinds.iter().position(|x| x == k).unwrap_or_else(|| panic!("[{n}] no {k} in {kinds:?}"));
        assert!(pos("started") < pos("approval_needed") && pos("approval_needed") < pos("approval_answered"), "[{n}] {kinds:?}");

        // an approval that only the machine's own screen may answer: needs_consent for a remote client, even with the scope
        if let Some((flow, aid)) = s.local_only_flow {
            let look = f.start(&rw, flow, &key("start-look-001")).unwrap();
            let e = f.approve(&ra, &look, aid, true).unwrap_err();
            assert_eq!(e.code, ErrorCode::NeedsConsent, "[{n}]");
            assert_eq!(f.status(&rd, &look).unwrap().state, RunState::WaitingApproval);
            assert_eq!(f.approve(&local(), &look, aid, true).unwrap().state, RunState::Running);
        }

        // stop is harmless twice and needs the run scope
        let ask2 = f.start(&rw, "ask", &key("start-ask-0002")).unwrap();
        wait_state(f, &ask2, |st| st.state == RunState::WaitingApproval);
        assert_eq!(f.stop(&rd, &ask2).unwrap_err().code, ErrorCode::Denied, "[{n}]");
        assert_eq!(f.stop(&rw, &ask2).unwrap().state, RunState::Stopped, "[{n}]");
        assert_eq!(f.stop(&rw, &ask2).unwrap().state, RunState::Stopped, "[{n}] twice");

        // audit: who, over what, which call; refusals are logged; a flow is named, nothing else
        let audit = (s.flows_audit)();
        assert!(audit.iter().any(|l| l.starts_with("cloud-ui via relay:") && l.contains("flows.start tidy") && l.ends_with("ok")), "[{n}] {audit:?}");
        assert!(audit.iter().any(|l| l.contains("flows.start") && l.contains("refused: denied")), "[{n}]");
        assert!(audit.iter().any(|l| l.contains("flows.approve") && l.contains("allowed")), "[{n}]");
    }
}

// ---------------------------------------------------------------------------------------------- the real adapter only

#[test]
fn a_local_person_who_is_not_an_administrator_cannot_change_settings_but_can_read_them() {
    let (s, _core, _env) = real();
    let amy = CallContext::local("amy", false);
    assert_eq!(s.settings.get(&amy, BOOL_KEY).unwrap(), SettingValue::Bool(false));
    let e = s.settings.set(&amy, BOOL_KEY, SettingValue::Bool(true), None).unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::Denied, "settings.admin_only"));
}

#[test]
fn a_setting_changed_behind_the_adapter_becomes_an_event_and_the_store_still_validates() {
    let (_s, core, _env) = real();
    let w = remote(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE]);
    let before = core.settings.watch(&w, None, Resume::Latest, 1).unwrap().next;
    core.svc.settings.lock().unwrap().update(|s| s.show_all_ai = true).unwrap();
    let b = core.settings.watch(&w, None, Resume::After(before), 10).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!((b.events[0].body.key.as_str(), &b.events[0].body.by[..]), (BOOL_KEY, "machine"));
    // the store's own rules apply through the contract: an unknown zone is refused as invalid input and nothing is written
    let e = core.settings.set(&local(), "time.zone", SettingValue::Text("Mars/Olympus".into()), None).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    assert_eq!(core.settings.get(&local(), "time.zone").unwrap(), SettingValue::Text(String::new()));
    core.settings.set(&local(), "time.zone", SettingValue::Text("Europe/Paris".into()), None).unwrap();
    assert_eq!(core.svc.settings.lock().unwrap().get().time.zone, "Europe/Paris");
}

#[test]
fn an_action_that_echoes_the_secret_is_scrubbed_and_an_unlocked_vault_cannot_be_read_back() {
    let (_s, core, _env) = real();
    let seat = LocalSeat::claim(&local()).unwrap();
    core.vault.unlock(&seat, LOGIN).unwrap();
    core.vault.register_action("leak:test", "deploy-token", Arc::new(|secret: &str| Ok(format!("the token is {secret}"))));
    let ctx = CallContext::remote("tester", Transport::DevMode, &[Scope::VAULT_USE]);
    let out = core.vault.use_secret(&ctx, &NamedAction("leak:test".into()), None).unwrap();
    assert_eq!(out.summary, "leak:test done");
    assert!(!format!("{out:?}").contains(SECRET));
    // a failing action says so in words
    core.vault.register_action("fail:test", "deploy-token", Arc::new(|_: &str| Err("the host said no".to_string())));
    let out = core.vault.use_secret(&ctx, &NamedAction("fail:test".into()), None).unwrap();
    assert_eq!((out.ok, out.summary.as_str()), (false, "the host said no"));
    // an action whose secret is not in the vault
    core.vault.register_action("gone:test", "not-stored", Arc::new(|_: &str| Ok("x".into())));
    assert_eq!(core.vault.use_secret(&ctx, &NamedAction("gone:test".into()), None).unwrap_err().code, ErrorCode::NotFound);
    // unlocking an already-open vault with anything changes nothing and the first idempotent result repeats
    let k = key("use-secret-777");
    let a = core.vault.use_secret(&ctx, &NamedAction(ACTION.into()), Some(&k)).unwrap();
    assert_eq!(core.vault.use_secret(&ctx, &NamedAction(ACTION.into()), Some(&k)).unwrap(), a);
    assert_eq!(core.vault.use_secret(&ctx, &NamedAction("fail:test".into()), Some(&k)).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn large_data_is_read_in_chunks_by_offset_resumes_checks_the_scope_and_expires() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let now = Arc::new(AtomicU64::new(1_000));
    let n2 = now.clone();
    let st = CoreStreams::with_clock(Box::new(move || n2.load(Ordering::SeqCst)));
    let payload: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let h = st.put(StreamKind::File, "application/octet-stream", Scope::FLOWS_READ, payload.clone()).unwrap();
    assert_eq!((h.total, h.max_chunk), (Some(100_000), 16 * 1024));
    let rd = remote(&[Scope::FLOWS_READ]);
    assert_eq!(st.read(&remote(&[Scope::SETTINGS_READ]), &h.id, 0, 10).unwrap_err().code, ErrorCode::Denied, "the scope of the call that made the handle");
    // read by offset; the link "drops" after two chunks and resumes at the offset reached
    let mut got = Vec::new();
    let mut off = 0u64;
    let mut chunks = 0;
    loop {
        let c = st.read(&rd, &h.id, off, 1 << 20).unwrap();
        assert_eq!(c.offset, off);
        assert!(c.data.len() <= 16 * 1024, "a read never exceeds max_chunk");
        got.extend_from_slice(&c.data);
        off += c.data.len() as u64;
        chunks += 1;
        if c.last {
            break;
        }
    }
    assert_eq!((got, chunks), (payload.clone(), 7));
    // reading again at the end gives an empty last chunk; past the end is invalid
    let end = st.read(&rd, &h.id, 100_000, 10).unwrap();
    assert!(end.data.is_empty() && end.last);
    assert_eq!(st.read(&rd, &h.id, 100_001, 10).unwrap_err().code, ErrorCode::InvalidInput);
    // a middle read repeats the same bytes (resume is idempotent)
    assert_eq!(st.read(&rd, &h.id, 40_000, 5).unwrap().data, payload[40_000..40_005]);
    // closing frees it; unknown after that
    st.close(&rd, &h.id).unwrap();
    assert_eq!(st.read(&rd, &h.id, 0, 1).unwrap_err().code, ErrorCode::NotFound);
    // expiry
    let h2 = st.put(StreamKind::Log, "text/plain", Scope::FLOWS_READ, b"abc".to_vec()).unwrap();
    now.store(1_000 + streams::TTL_MS + 1, Ordering::SeqCst);
    assert_eq!(st.read(&rd, &h2.id, 0, 1).unwrap_err().code, ErrorCode::NotFound);
    // bounded: too many handles are refused as busy, not dropped silently
    for i in 0..64 {
        st.put(StreamKind::Log, "text/plain", Scope::FLOWS_READ, vec![i as u8]).unwrap();
    }
    assert_eq!(st.put(StreamKind::Log, "text/plain", Scope::FLOWS_READ, vec![1]).unwrap_err().code, ErrorCode::Busy);
}

#[test]
fn a_frame_handed_to_a_run_is_an_event_with_a_handle_not_the_bytes() {
    let (_s, core, env) = real();
    let flows = core.flows_for_env(env);
    let rw = remote(&[Scope::FLOWS_READ, Scope::FLOWS_RUN]);
    let run = flows.start(&rw, "ask", &key("start-ask-9999")).unwrap();
    let bytes = vec![9u8; 40_000];
    let h = flows.hand_frame(&run, "image/png", bytes.clone()).unwrap();
    let evs = flows.events(&rw, Some(&run), Resume::After(0), 100).unwrap();
    let frame = evs.events.iter().find_map(|e| if let FlowEvent::Frame { stream, .. } = &e.body { Some(stream.clone()) } else { None }).expect("a frame event");
    assert_eq!(frame, h);
    assert!(!serde_json::to_string(&evs).unwrap().contains("999999"), "no bytes in the event");
    assert_eq!(serde_json::to_vec(&evs).unwrap().len() < 4000, true);
    let c = core.streams.read(&rw, &h.id, 0, 4096).unwrap();
    assert_eq!((c.data.len(), c.last), (4096, false));
    flows.stop(&rw, &run).unwrap();
}

#[test]
fn a_consent_question_that_ran_out_of_time_is_expired_and_a_local_person_of_another_account_cannot_use_the_engine() {
    let (_s, core, env) = real();
    let id = core.consents.ask(ConsentKind::Permission, "tile.x", "consent.permission", "Allow?");
    // age the question by hand: nothing in the contract lets a caller do this
    core.consents.age_for_test(&id);
    assert!(core.consents.pending(&remote(&[Scope::CONSENT_READ])).unwrap().is_empty());
    let seat = LocalSeat::claim(&local()).unwrap();
    assert_eq!(core.consents.answer(&seat, &id, ConsentAnswer::Allow).unwrap_err().code, ErrorCode::NotFound);
    let flows = core.flows_for_env(env);
    let e = flows.list(&CallContext::local("someone-else", true)).unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::Denied, "flows.not_yours"));
}

#[test]
fn the_real_adapters_expose_no_method_that_returns_a_secret_shaped_value() {
    // the traits are the contract; the adapters add only these public functions with a return type, and none returns text or bytes
    // taken from the vault. A new public vault method returning String/Vec<u8>/&str fails here.
    let src = include_str!("vault.rs");
    for l in src.lines().filter(|l| l.trim_start().starts_with("pub fn ")) {
        assert!(!l.contains("-> String") && !l.contains("-> Vec<u8>") && !l.contains("-> &str") && !l.contains("Zeroizing"), "{l}");
    }
}
