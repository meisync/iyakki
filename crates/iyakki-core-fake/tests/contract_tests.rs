//! The contract rules, proved against the in-memory fakes (docs/development/CORE-CONTRACT.md).

use iyakki_core_api::contract::catalog::{hello_for, DOMAINS};
use iyakki_core_api::contract::*;
use iyakki_core_api::Capabilities;
use iyakki_core_fake::contract::*;

fn local() -> CallContext {
    CallContext::local("tester", true)
}
fn relay(scopes: &[&str]) -> CallContext {
    CallContext::remote("cloud-ui", Transport::Relay, scopes)
}
fn key(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).unwrap()
}

#[test]
fn a_remote_principal_cannot_answer_a_consent_request() {
    let consents = FakeConsents::new();
    let id = consents.ask(ConsentKind::Camera, "tile.scan", "Use the camera?");
    for t in [Transport::DevMode, Transport::Relay] {
        // Even holding every scope there is, no remote context can become a seat.
        let remote = CallContext::remote("cloud-ui", t, &["consent:read", "local", "flows:approve", "vault:use"]);
        assert_eq!(LocalSeat::claim(&remote).unwrap_err().code, ErrorCode::Denied);
        // It can see the question.
        assert_eq!(consents.pending(&remote).unwrap().len(), 1);
    }
    assert!(consents.answered().is_empty());
    // A local person can.
    let seat = LocalSeat::claim(&local()).unwrap();
    consents.answer(&seat, &id, ConsentAnswer::Allow).unwrap();
    assert_eq!(consents.answered(), [(id, ConsentAnswer::Allow, "tester".to_string())]);
}

#[test]
fn a_flow_approval_that_touches_the_camera_is_answered_only_at_the_machine() {
    let flows = FakeFlows::sample();
    let run = flows.start(&local(), "look", &key("start-look-01")).unwrap();
    let remote = relay(&[Scope::FLOWS_READ, Scope::FLOWS_RUN, Scope::FLOWS_APPROVE]);
    let e = flows.approve(&remote, &run, "a2", true).unwrap_err();
    assert_eq!(e.code, ErrorCode::NeedsConsent);
    assert_eq!(flows.status(&remote, &run).unwrap().state, RunState::WaitingApproval);
    assert_eq!(flows.approve(&local(), &run, "a2", true).unwrap().state, RunState::Running);
}

#[test]
fn a_remote_client_may_answer_an_ordinary_approval_with_the_scope_only() {
    let flows = FakeFlows::sample();
    let run = flows.start(&local(), "ask", &key("start-ask-001")).unwrap();
    assert_eq!(flows.approve(&relay(&[Scope::FLOWS_READ]), &run, "a1", true).unwrap_err().code, ErrorCode::Denied);
    let st = flows.approve(&relay(&[Scope::FLOWS_APPROVE]), &run, "a1", true).unwrap();
    assert_eq!(st.state, RunState::Running);
}

#[test]
fn the_vault_trait_returns_no_secret_shaped_type() {
    // Every return type in the trait must be one of these. A method returning String, Vec<u8>, &str or a new type fails here and
    // needs the owner's decision, not a quiet edit of this list.
    let src = include_str!("../../iyakki-core-api/src/contract/vault.rs");
    let body = &src[src.find("pub trait Vault").unwrap()..];
    let allowed = ["CallResult<VaultStatus>", "CallResult<ActionOutcome>"];
    let mut methods = 0;
    for line in body.lines().filter(|l| l.trim_start().starts_with("fn ")) {
        methods += 1;
        let ret = line.split("->").nth(1).expect("every method returns something").trim().trim_end_matches(';').trim();
        assert!(allowed.contains(&ret), "vault method returns `{ret}`: {line}");
    }
    assert_eq!(methods, 4, "status, unlock, lock, use_secret");
    // The output types carry no free-form value field either.
    let types = &src[..src.find("pub trait Vault").unwrap()];
    assert!(!types.contains("secret:") && !types.contains("value:") && !types.contains("password:") && !types.contains("token:"), "VaultStatus/ActionOutcome must not grow a value field");
}

#[test]
fn the_fake_vault_never_lets_a_secret_out_through_any_call_or_the_audit() {
    const MARK: &str = "S3CRET-MARKER-VALUE";
    let vault = FakeVault::new("login-pass").with_secret("git-token", MARK, "sign_in:git");
    let remote = relay(&[Scope::VAULT_STATUS, Scope::VAULT_LOCK, Scope::VAULT_USE]);
    let mut seen = String::new();
    // Locked: using a secret is refused, with a hint for the person.
    let e = vault.use_secret(&remote, &NamedAction("sign_in:git".into()), None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Locked);
    seen += &format!("{e:?}");
    seen += &format!("{:?}", vault.status(&remote).unwrap());
    // A wrong login stays locked; the right one, from the local seat, unlocks.
    let seat = LocalSeat::claim(&local()).unwrap();
    assert_eq!(vault.unlock(&seat, "wrong").unwrap_err().code, ErrorCode::Denied);
    assert!(vault.status(&remote).unwrap().locked);
    assert!(!vault.unlock(&seat, "login-pass").unwrap().locked);
    let out = vault.use_secret(&remote, &NamedAction("sign_in:git".into()), None).unwrap();
    assert!(out.ok);
    seen += &format!("{out:?}");
    assert_eq!(vault.use_secret(&remote, &NamedAction("nope".into()), None).unwrap_err().code, ErrorCode::NotFound);
    seen += &format!("{:?}", vault.lock(&remote).unwrap());
    seen += &vault.audit().join("\n");
    assert_eq!(vault.used(), [("sign_in:git".to_string(), "git-token".to_string())], "the machine used the secret itself");
    assert!(!seen.contains(MARK) && !seen.contains("login-pass"), "{seen}");
    // Without the scope, nothing works.
    assert_eq!(vault.use_secret(&relay(&[]), &NamedAction("sign_in:git".into()), None).unwrap_err().code, ErrorCode::Denied);
}

#[test]
fn a_settings_watcher_resumes_from_its_last_sequence_and_gets_exactly_what_it_missed() {
    let s = FakeSettings::sample();
    let me = local();
    let remote = relay(&[Scope::SETTINGS_READ]);
    s.set(&me, "display.dark", SettingValue::Bool(true), None).unwrap();
    let first = s.watch(&remote, None, Resume::After(0), 100).unwrap();
    assert_eq!(first.events.len(), 1);
    // Link drops. Three changes happen.
    s.set(&me, "voice.speed", SettingValue::Text("fast".into()), None).unwrap();
    s.set(&me, "display.dark", SettingValue::Bool(false), None).unwrap();
    s.set(&me, "voice.speed", SettingValue::Text("slow".into()), None).unwrap();
    let missed = s.watch(&remote, None, Resume::After(first.next), 100).unwrap();
    let got: Vec<(u64, String, SettingValue)> = missed.events.iter().map(|e| (e.seq, e.body.key.clone(), e.body.value.clone())).collect();
    assert_eq!(got, [(2, "voice.speed".to_string(), SettingValue::Text("fast".into())), (3, "display.dark".to_string(), SettingValue::Bool(false)), (4, "voice.speed".to_string(), SettingValue::Text("slow".into()))]);
    assert!(!missed.gap);
    assert!(s.watch(&remote, None, Resume::After(missed.next), 100).unwrap().events.is_empty());
    // One key only.
    assert_eq!(s.watch(&remote, Some("display.dark"), Resume::After(0), 100).unwrap().events.len(), 2);
}

#[test]
fn settings_check_scope_schema_and_local_only_keys_and_audit_without_values() {
    let s = FakeSettings::sample();
    let reader = relay(&[Scope::SETTINGS_READ]);
    let writer = relay(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE]);
    assert_eq!(s.get(&reader, "display.dark").unwrap(), SettingValue::Bool(false));
    assert_eq!(s.get(&reader, "nope").unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(s.set(&reader, "display.dark", SettingValue::Bool(true), None).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(s.set(&writer, "display.dark", SettingValue::Int(3), None).unwrap_err().code, ErrorCode::InvalidInput);
    assert_eq!(s.set(&writer, "voice.speed", SettingValue::Text("warp".into()), None).unwrap_err().code, ErrorCode::InvalidInput);
    // A local-only key: even a remote writer cannot touch it; the person at the machine can.
    assert_eq!(s.set(&writer, "security.vault_timeout", SettingValue::Int(5), None).unwrap_err().code, ErrorCode::Denied);
    s.set(&local(), "security.vault_timeout", SettingValue::Int(5), None).unwrap();
    s.set(&writer, "display.dark", SettingValue::Bool(true), None).unwrap();
    let audit = s.audit().join("\n");
    assert!(audit.contains("cloud-ui via relay: settings.set display.dark [change] ok"), "{audit}");
    assert!(audit.contains("refused: denied"));
    assert!(!audit.contains("true"), "values stay out of the audit");
}

#[test]
fn a_setting_change_with_an_idempotency_key_is_applied_once() {
    let s = FakeSettings::sample();
    let w = relay(&[Scope::SETTINGS_WRITE, Scope::SETTINGS_READ]);
    let k = key("set-dark-0001");
    let a = s.set(&w, "display.dark", SettingValue::Bool(true), Some(&k)).unwrap();
    let b = s.set(&w, "display.dark", SettingValue::Bool(true), Some(&k)).unwrap();
    assert!(!a.repeated && b.repeated);
    assert_eq!(a.seq, b.seq);
    assert_eq!(s.watch(&w, None, Resume::After(0), 10).unwrap().events.len(), 1);
    assert_eq!(s.set(&w, "display.dark", SettingValue::Bool(false), Some(&k)).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn starting_a_flow_run_twice_with_the_same_key_gives_one_run() {
    let f = FakeFlows::sample();
    let c = relay(&[Scope::FLOWS_RUN, Scope::FLOWS_READ]);
    let k = key("start-tidy-001");
    let a = f.start(&c, "tidy", &k).unwrap();
    let b = f.start(&c, "tidy", &k).unwrap();
    assert_eq!(a, b);
    assert_eq!(f.runs_started(), 1);
    let started = f.events(&c, None, Resume::After(0), 100).unwrap().events.iter().filter(|e| matches!(e.body, FlowEvent::Started { .. })).count();
    assert_eq!(started, 1);
    assert_eq!(f.start(&c, "ask", &k).unwrap_err().code, ErrorCode::Conflict, "same key, different flow");
    let c2 = f.start(&c, "tidy", &key("start-tidy-002")).unwrap();
    assert_ne!(a, c2);
    assert_eq!(f.start(&c, "ghost", &key("start-ghost-01")).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn a_flow_runs_stops_and_streams_its_events_per_run_in_order() {
    let f = FakeFlows::sample();
    let me = local();
    let r1 = f.start(&me, "tidy", &key("start-tidy-001")).unwrap();
    let r2 = f.start(&me, "ask", &key("start-ask-0001")).unwrap();
    assert_eq!(f.list(&me).unwrap().len(), 3);
    f.hand_frame(&r1, StreamHandle { id: StreamId("frame-1".into()), kind: StreamKind::CameraFrame, total: Some(10), max_chunk: 4, media_type: "image/jpeg".into() });
    assert_eq!(f.stop(&me, &r1).unwrap().state, RunState::Stopped);
    assert_eq!(f.stop(&me, &r1).unwrap().state, RunState::Stopped, "stopping twice is harmless");
    let ev: Vec<String> = f.events(&me, Some(&r1), Resume::After(0), 100).unwrap().events.iter().map(|e| e.topic.clone()).collect();
    assert_eq!(ev, ["started", "step", "frame", "finished"]);
    let all = f.events(&me, None, Resume::After(0), 100).unwrap();
    assert!(all.events.windows(2).all(|w| w[0].seq + 1 == w[1].seq), "sequence numbers are gapless");
    assert_eq!(f.status(&me, &r2).unwrap().state, RunState::WaitingApproval);
    assert_eq!(f.approve(&me, &r2, "a1", false).unwrap().state, RunState::Stopped, "denying stops the run");
    assert_eq!(f.status(&me, &RunId("run-99".into())).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn hello_and_capability_negotiation_across_versions() {
    let machine = hello_for("machine-1", "test", Capabilities::all());
    assert!(machine.supports("flows", "start") && !machine.supports("flows", "teleport") && !machine.supports("chat", "send"));
    assert_eq!(machine.domains.len(), DOMAINS.len());

    // Same version: everything wanted that exists is usable, the rest is reported missing (hide the screen, do not fail).
    let n = negotiate(&machine, &ClientHello { contract: CONTRACT_VERSION, wants: vec!["settings".into(), "chat".into()] });
    assert_eq!(n.version, Some(CONTRACT_VERSION));
    assert_eq!((n.usable, n.missing), (vec!["settings".to_string()], vec!["chat".to_string()]));

    // A newer cloud UI (higher minor) talks to this older machine at the machine's minor.
    let newer = ClientHello { contract: ContractVersion { major: CONTRACT_VERSION.major, minor: CONTRACT_VERSION.minor + 5 }, wants: vec!["flows".into(), "files".into()] };
    let n = negotiate(&machine, &newer);
    assert_eq!(n.version, Some(CONTRACT_VERSION));
    assert_eq!((n.usable, n.missing), (vec!["flows".to_string()], vec!["files".to_string()]));

    // A different major refuses everything.
    let other = ClientHello { contract: ContractVersion { major: CONTRACT_VERSION.major + 1, minor: 0 }, wants: vec!["settings".into()] };
    let n = negotiate(&machine, &other);
    assert_eq!((n.version, n.usable, n.missing), (None, vec![], vec!["settings".to_string()]));

    // A client older than the machine still serves is refused; an older client within range gets only domains that existed then.
    let mut m2 = machine.clone();
    m2.contract.minor = 3;
    m2.oldest_minor = 2;
    m2.domains.push(DomainSupport { domain: "files".into(), since_minor: 3, calls: vec!["list".into()], events: vec![] });
    let old = ClientHello { contract: ContractVersion { major: 0, minor: 1 }, wants: vec!["settings".into()] };
    assert_eq!(negotiate(&m2, &old).version, None);
    let mid = ClientHello { contract: ContractVersion { major: 0, minor: 2 }, wants: vec!["settings".into(), "files".into()] };
    let n = negotiate(&m2, &mid);
    assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 2 }));
    assert_eq!((n.usable, n.missing), (vec!["settings".to_string()], vec!["files".to_string()]), "files appeared in minor 3");

    // The hello survives the wire.
    let json = serde_json::to_string(&machine).unwrap();
    assert_eq!(serde_json::from_str::<Hello>(&json).unwrap(), machine);
}

#[test]
fn the_catalogue_matches_what_the_fakes_enforce() {
    // Every remote-callable call refuses a caller with no scopes; local-only ones refuse a remote caller holding every scope.
    let everything: Vec<&str> = DOMAINS.iter().flat_map(|d| d.calls.iter().map(|c| c.scope)).collect();
    for d in DOMAINS {
        for c in d.calls {
            let none = relay(&[]);
            let all = CallContext::remote("cloud-ui", Transport::Relay, &everything);
            let denied_none = guard(&none, c.scope, c.risk).is_err();
            let denied_all = guard(&all, c.scope, c.risk).is_err();
            assert!(denied_none, "{}.{} must need a scope", d.name, c.name);
            assert_eq!(denied_all, c.risk == Risk::LocalOnly, "{}.{}", d.name, c.name);
        }
    }
}

#[test]
fn error_text_has_a_key_a_message_and_a_hint_and_survives_the_wire() {
    let e = FakeVault::new("x").use_secret(&relay(&[Scope::VAULT_USE]), &NamedAction("a".into()), None).unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::Locked, "vault.locked"));
    assert!(!e.message.is_empty() && !e.hint.is_empty());
    assert_eq!(serde_json::from_str::<CallError>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
}
