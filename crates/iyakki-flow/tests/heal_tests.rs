mod common;

use std::time::Duration;

use common::*;
use iyakki_flow::heal::{Class, Patch, PatchOp};
use iyakki_flow::host::{failed_exec, ok_exec, HealProposal};
use iyakki_flow::*;
use serde_json::json;

fn flow_with(mode: HealMode) -> Flow {
    let mut f = linear("h");
    f.nodes[1].retry = Some(RetryPolicy::fixed(2, 0));
    f.defaults.heal = HealPolicy { mode, ..Default::default() };
    f
}

fn patch(node: &str, ops: Vec<PatchOp>) -> Patch {
    Patch { node: node.into(), ops, reason: "because".into(), claimed_risk: "low".into(), touches: vec![] }
}

fn proposal(p: Patch) -> HealProposal {
    HealProposal { diagnosis: "the call is too slow for a 30 s limit".into(), patch: Some(p), cents: 2 }
}

/// An endpoint that only answers when the attempt may take at least 5 seconds.
fn slow_server(e: &Env) {
    e.host.on_http(|req| if req.timeout_ms >= 5000 { ok_json("{}") } else { Err(NodeError::timeout("timed out")) });
}

fn set_timeout_5s() -> Patch {
    patch("a", vec![PatchOp::SetTimeout { ms: 5000 }])
}

fn start(e: &Env, id: &str) -> String {
    e.engine.start_run(StartRequest::new(id)).unwrap().run_id
}

fn flow_timeout_default_small(mut f: Flow) -> Flow {
    f.defaults.timeout_ms = 1000;
    f
}

#[test]
fn a_simple_fix_is_applied_tested_and_verified_without_asking() {
    let e = env("heal-auto");
    slow_server(&e);
    e.host.propose(Ok(proposal(set_timeout_5s())));
    e.publish(flow_timeout_default_small(flow_with(HealMode::AutoFixSimple)));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.heal);
    assert_eq!(r.heal.len(), 1);
    let c = &r.heal[0];
    assert_eq!(c.state, CardState::Verified, "success marks the patch verified");
    assert!(c.class.as_ref().unwrap().is_simple());
    assert_eq!(c.decided_by, "agent:assistant");
    assert_eq!(c.draft_rev, 1);
    assert_eq!(r.heal_spent, 1);
    assert_eq!(r.heal_cents, 2);
    assert_eq!(r.patches.len(), 1);
    assert!(r.patches[0].simple);
    // 2 attempts failed, then a test of the fixed step with the recorded input, then the rerun
    assert_eq!(e.host.http_calls.lock().unwrap().len(), 4);
    let limits: Vec<u64> = e.host.http_calls.lock().unwrap().iter().map(|c| c.timeout_ms).collect();
    assert_eq!(limits, vec![1000, 1000, 5000, 5000]);
    // the fix is a DRAFT with a badge; the published version is untouched
    let dr = e.store.get_draft("h").unwrap();
    assert_eq!(dr.node("a").unwrap().timeout_ms, Some(5000));
    assert!(dr.node("a").unwrap().modified_by.as_ref().unwrap().by_agent());
    assert!(dr.meta.modified_by.iter().any(|c| c.by_agent() && c.simple));
    assert_eq!(e.store.published("h").unwrap().node("a").unwrap().timeout_ms, None);
    // never silent
    let audit = e.host.audit_lines();
    let pos = |s: &str| audit.iter().position(|l| l.starts_with(s)).unwrap_or_else(|| panic!("{s} in {audit:?}"));
    assert!(pos("flow.heal.proposed") < pos("flow.heal.applied") && pos("flow.heal.applied") < pos("flow.heal.verified"));
    assert!(audit.iter().any(|l| l.contains("automatic, simple")));
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.title.starts_with("Fixed:")));
    // publishing that draft still needs a person
    assert!(e.store.publish("h", &iyakki_flow::store::PublishReq { by: "agent:assistant", label: "", changelog: "x", approved_by: None }).unwrap_err().contains("approve"));
    // the run keeps the fixed node, and its node says why
    assert!(r.nodes["a"].note.contains("fixed by agent:assistant"));
}

#[test]
fn the_agent_gets_a_compact_redacted_context() {
    let e = env("heal-ctx");
    e.host.secret_is("api", "Sup3r-Secret-Key");
    e.host.on_http(|req| {
        let a = req.headers.iter().find(|(k, _)| k == "Authorization").map(|x| x.1.clone()).unwrap_or_default();
        Ok(iyakki_flow::host::HttpResponse { status: 500, body: format!("internal error for {a}"), ..Default::default() })
    });
    e.host.propose(Ok(HealProposal { diagnosis: "server error".into(), patch: None, cents: 0 }));
    let mut f = flow_with(HealMode::Suggest);
    f.nodes[1].config = json!({"url": "https://api.example.com/a", "headers": {"Authorization": "Bearer {{secret.api}}"}});
    e.publish(f);
    let id = start(&e, "h");
    e.finish(&id);
    let reqs = e.host.heal_requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    let ctx = &reqs[0].context;
    assert!(!ctx.contains("Sup3r-Secret-Key"), "the secret reached the agent: {ctx}");
    assert!(ctx.contains("FAILING NODE a") && ctx.contains("{{secret.api}}") && ctx.contains("***"));
    assert!(ctx.contains("Reply with ONE JSON object"));
    assert!(ctx.len() < 8000);
}

#[test]
fn suggest_mode_always_waits_for_you_even_for_a_simple_fix() {
    let e = env("heal-suggest");
    slow_server(&e);
    e.host.propose(Ok(proposal(set_timeout_5s())));
    e.publish(flow_timeout_default_small(flow_with(HealMode::Suggest)));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.status, RunStatus::Blocked);
    assert!(r.blocked.contains("A fix is proposed"), "{}", r.blocked);
    let c = &r.heal[0];
    assert_eq!(c.state, CardState::Proposed);
    assert!(c.class.as_ref().unwrap().is_simple(), "simple, but this mode never applies by itself");
    assert!(r.patches.is_empty());
    assert!(e.store.get_draft("h").unwrap().node("a").unwrap().timeout_ms.is_none());
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.needs_you && n.body.contains("proposes")));
    assert_eq!(e.engine.counts().needs_you, 1);
    e.engine.card_decide(&id, &c.id, CardDecision::Approve { by: "user:me".into() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.heal[0].state, CardState::Verified);
    assert_eq!(r.heal[0].decided_by, "user:me");
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.heal.approved") && l.contains("user:me")));
    // a decided card cannot be decided again
    assert!(e.engine.card_decide(&id, &r.heal[0].id, CardDecision::Approve { by: "x".into() }).is_err());
}

#[test]
fn a_fix_that_is_not_simple_waits_for_you_even_when_automatic_fixes_are_on() {
    let e = env("heal-ask");
    e.host.on_http(|req| if req.url.contains("good.example.com") { ok_json("{}") } else { status(500, "no") });
    let bad = patch("a", vec![PatchOp::SetConfig { path: "url".into(), value: json!("https://good.example.com/a") }]);
    e.host.propose(Ok(proposal(bad)));
    e.publish(flow_with(HealMode::AutoFixSimple));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    let c = &r.heal[0];
    assert_eq!(c.state, CardState::Proposed);
    assert_eq!(c.class.as_ref().unwrap().class, Class::NeedsApproval);
    assert!(c.class.as_ref().unwrap().reasons.iter().any(|x| x.contains("host")), "{:?}", c.class);
    assert!(r.patches.is_empty() && r.heal_spent == 0);
    e.engine.card_decide(&id, &c.id, CardDecision::Reject { by: "user:me".into(), note: "no, ask the vendor".into() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Failed, "a rejected fix leaves the failure standing");
    assert_eq!(r.heal[0].state, CardState::Rejected);
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.heal.rejected")));
    assert!(e.store.get_draft("h").unwrap().node("a").unwrap().config["url"].as_str().unwrap().contains("api.example.com"), "nothing was changed");
}

#[test]
fn an_edited_fix_is_classified_again() {
    let e = env("heal-edit");
    slow_server(&e);
    let wrong = patch("a", vec![PatchOp::SetConfig { path: "headers.Authorization".into(), value: json!("Bearer x") }]);
    e.host.propose(Ok(proposal(wrong)));
    e.publish(flow_timeout_default_small(flow_with(HealMode::AutoFixSimple)));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].class.as_ref().unwrap().class, Class::NeedsApproval);
    // the person edits it into something that works
    e.engine.card_decide(&id, &r.heal[0].id, CardDecision::Edit { by: "user:me".into(), patch: set_timeout_5s() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.heal[0].note, "edited by user:me");
    assert!(r.heal[0].class.as_ref().unwrap().is_simple());
}

#[test]
fn a_refused_fix_is_shown_as_advice_not_applied() {
    let e = env("heal-refuse");
    e.host.on_exec(|_| Ok(failed_exec(1, "boom")));
    let evil = patch("a", vec![PatchOp::SetConfig { path: "command".into(), value: json!(["curl-evil", "x"]) }]);
    e.host.propose(Ok(proposal(evil)));
    let mut f = Flow::new("h", "H");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("a", "cli", json!({"command": ["backup", "now"]})));
    f.edges.push(Edge::new("s", "a"));
    f.defaults.heal.mode = HealMode::AutoFixSimple;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    let c = &r.heal[0];
    assert!(c.patch.is_none(), "the refused change is not offered for approval");
    assert!(c.note.contains("not a program in the CLI catalog"), "{}", c.note);
    assert!(c.diagnosis.contains("refused"));
    assert!(e.engine.card_decide(&id, &c.id, CardDecision::Approve { by: "u".into() }).is_err(), "nothing to approve");
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.heal.rejected") && l.contains("refused by the harness")));
    // the person can still retry or skip the step by hand
    e.host.on_exec(|_| Ok(ok_exec("fine")));
    e.engine.retry_node(&id, "a").unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(r.heal[0].state, CardState::Rejected, "the open card was closed by the retry");
}

#[test]
fn the_heal_budget_ends_in_a_blocked_run_with_the_diagnosis_and_evidence() {
    let e = env("heal-budget");
    *e.host.evidence.lock().unwrap() = Some(e.dir.join("evidence"));
    e.host.on_http(|_| status(500, "still broken"));
    e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetTimeout { ms: 1111 }]))));
    e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetTimeout { ms: 2222 }]))));
    let mut f = flow_with(HealMode::AutoFixSimple);
    f.defaults.heal.max_per_run = 1;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.status, RunStatus::Blocked);
    assert_eq!(r.heal_spent, 1, "one fix was tried");
    assert_eq!(r.heal.len(), 2);
    assert_eq!(r.heal[0].state, CardState::Failed, "the first fix did not help");
    assert_eq!(r.heal[1].state, CardState::Proposed);
    assert!(r.heal[1].diagnosis.contains("I will not try more fixes"), "{}", r.heal[1].diagnosis);
    assert_eq!(r.heal[1].note, "budget");
    assert_eq!(e.host.heal_requests.lock().unwrap().len(), 1, "the agent was not asked again");
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.heal.budget")));
    // evidence on disk
    let dir = e.dir.join("evidence/h/evidence").join(&id).join("a");
    for f in ["node.json", "attempts.json", "context.txt", "cards.json", "run-log.txt"] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    assert!(std::fs::read_to_string(dir.join("context.txt")).unwrap().contains("FAILING NODE a"));
    // the person closes it
    e.engine.card_decide(&id, &r.heal[1].id, CardDecision::Reject { by: "user:me".into(), note: String::new() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Failed);
}

#[test]
fn the_daily_limit_counts_fixes_across_runs() {
    let e = env("heal-day");
    e.host.on_http(|_| status(500, "x"));
    for ms in [1111u64, 2222, 3333] {
        e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetTimeout { ms }]))));
    }
    let mut f = flow_with(HealMode::AutoFixSimple);
    f.defaults.heal.max_per_day = 2;
    f.defaults.heal.max_per_run = 5;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal_spent, 2);
    assert!(r.heal.last().unwrap().diagnosis.contains("daily limit"), "{}", r.heal.last().unwrap().diagnosis);
}

#[test]
fn off_means_no_agent_and_always_ask_waits_to_be_asked() {
    let e = env("heal-off");
    e.host.on_http(|_| status(500, "x"));
    e.publish(flow_with(HealMode::Off));
    let r = e.run("h");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.heal.is_empty() && e.host.heal_requests.lock().unwrap().is_empty());

    let e = env("heal-ask2");
    slow_server(&e);
    e.host.propose(Ok(proposal(set_timeout_5s())));
    e.publish(flow_timeout_default_small(flow_with(HealMode::AlwaysAsk)));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].note, "investigate");
    assert!(r.heal[0].patch.is_none());
    assert!(e.host.heal_requests.lock().unwrap().is_empty(), "the agent has not been called yet");
    e.engine.card_decide(&id, &r.heal[0].id, CardDecision::Investigate { by: "user:me".into() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.heal[0].patch.is_some() || r.status.is_terminal()).unwrap();
    assert!(r.heal[0].patch.is_some());
    // even a simple fix then waits for approval
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked && r.heal[0].state == CardState::Proposed).unwrap();
    assert!(r.patches.is_empty());
    e.engine.card_decide(&id, &r.heal[0].id, CardDecision::Approve { by: "user:me".into() }).unwrap();
    assert_eq!(e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap().status, RunStatus::Succeeded);
}

#[test]
fn a_production_flow_never_gets_an_automatic_fix() {
    let e = env("heal-prod");
    slow_server(&e);
    e.host.propose(Ok(proposal(set_timeout_5s())));
    let mut f = flow_timeout_default_small(flow_with(HealMode::AutoFixSimple));
    f.production = true;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].state, CardState::Proposed);
    assert!(r.heal[0].class.as_ref().unwrap().reasons.iter().any(|x| x.contains("production")));
    assert!(r.patches.is_empty());
}

#[test]
fn a_missing_agent_or_a_bad_answer_becomes_a_card_not_a_hang() {
    let e = env("heal-err");
    e.host.on_http(|_| status(500, "x"));
    e.host.propose(Err("no model is connected".into()));
    e.publish(flow_with(HealMode::Suggest));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert!(r.heal[0].diagnosis.contains("no model is connected"), "{}", r.heal[0].diagnosis);
    assert!(r.heal[0].patch.is_none());
    e.engine.skip_node(&id, "a", "user:me").unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "skipping the failed step lets the run go on");
    assert_eq!(out(&r, "a")["skipped"], true);
}

#[test]
fn a_cli_that_is_too_old_is_upgraded_through_the_catalog_and_the_step_reruns() {
    let e = env("heal-cli");
    e.host.cli_is("grpcurl", "1.7.0");
    e.host.on_exec(|req| if req.argv[0] == "grpcurl" { Ok(ok_exec("{\"ok\": true}")) } else { Ok(failed_exec(1, "?")) });
    e.host.propose(Ok(HealProposal { diagnosis: "grpcurl is older than this step needs".into(), patch: Some(patch("call", vec![PatchOp::CliUpgrade { cli: "grpcurl".into(), to: "1.9.0".into() }])), cents: 1 }));
    let mut f = Flow::new("h", "H");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("call", "grpc", json!({"address": "svc:50051", "method": "pkg.Svc/Do", "plaintext": true, "cli": {"min_version": "1.8.0"}})));
    f.edges.push(Edge::new("s", "call"));
    f.defaults.heal.mode = HealMode::AutoFixSimple;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.heal);
    assert_eq!(*e.host.upgrades.lock().unwrap(), vec![("grpcurl".to_string(), "1.9.0".to_string())]);
    assert_eq!(r.heal[0].state, CardState::Verified);
    assert!(r.nodes["call"].attempts[0].error.contains("older than"), "{:?}", r.nodes["call"].attempts);
    assert_eq!(e.store.get_draft("h").unwrap().node("call").unwrap().config["cli"]["min_version"], "1.9.0");
}

#[test]
fn a_cli_upgrade_for_a_cli_the_node_does_not_require_asks_first() {
    let e = env("heal-cli2");
    e.host.cli_is("grpcurl", "1.7.0");
    e.host.cli_is("kubectl", "1.20.0");
    e.host.on_exec(|_| Ok(failed_exec(1, "x")));
    e.host.propose(Ok(proposal(patch("call", vec![PatchOp::CliUpgrade { cli: "kubectl".into(), to: "1.30.0".into() }]))));
    let mut f = Flow::new("h", "H");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("call", "cli", json!({"command": ["grpcurl", "x"], "cli": {"id": "grpcurl", "min_version": "1.7.0"}})));
    f.edges.push(Edge::new("s", "call"));
    f.defaults.heal.mode = HealMode::AutoFixSimple;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].class.as_ref().unwrap().class, Class::NeedsApproval);
    assert!(e.host.upgrades.lock().unwrap().is_empty(), "nothing was installed");
}

#[test]
fn a_script_node_edit_inside_its_own_script_is_simple() {
    let e = env("heal-script");
    e.host.script_results.lock().unwrap().push_back(Err(NodeError::new("ReferenceError: b is not defined")));
    e.host.script_results.lock().unwrap().push_back(Ok(json!(3)));
    e.host.script_results.lock().unwrap().push_back(Ok(json!(3)));
    e.host.propose(Ok(proposal(patch("sc", vec![PatchOp::EditScript { find: "a + b".into(), replace: "a + 2".into() }]))));
    let mut f = Flow::new("h", "H");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("sc", "script", json!({"lang": "js", "source": "let a = 1;\nreturn a + b;"})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"r": "{{ nodes.sc.output.result }}"}})));
    f.edges = vec![Edge::new("s", "sc"), Edge::new("sc", "e")];
    f.defaults.heal.mode = HealMode::AutoFixSimple;
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.heal);
    assert!(r.flow.node("sc").unwrap().config["source"].as_str().unwrap().contains("a + 2"));
    assert!(e.store.get_draft("h").unwrap().node("sc").unwrap().config["source"].as_str().unwrap().contains("a + 2"));
    assert_eq!(r.outputs["r"], 3);
}

#[test]
fn a_restart_while_the_agent_was_looking_asks_again() {
    let e = env("heal-restart");
    e.host.on_http(|_| status(500, "x"));
    e.host.propose(Err("x".into()));
    e.publish(flow_with(HealMode::Suggest));
    let id = start(&e, "h");
    e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    e.engine.halt();
    let path = e.dir.join("engine/runs").join(&id).join("run.json");
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["heal"][0]["state"] = json!("investigating");
    v["status"] = json!("healing");
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let e2 = e.restart();
    e2.engine.recover();
    let r = e2.engine.wait_for(&id, Duration::from_secs(5), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].state, CardState::Proposed);
    assert!(r.heal[0].diagnosis.contains("interrupted by a restart"));
}

// ------------------------------------------------------------------------------------------------ consent is never "fixed" by the agent

fn camera_flow(mode: HealMode) -> Flow {
    let mut f = linear("h");
    f.nodes[1] = Node::new("a", "cam_photo", json!({}));
    f.defaults.heal = HealPolicy { mode, ..Default::default() };
    f
}

#[test]
fn a_step_refused_for_want_of_the_persons_permission_never_goes_to_the_agent_and_spends_no_attempt() {
    for mode in [HealMode::Suggest, HealMode::AutoFixSimple, HealMode::AlwaysAsk] {
        let e = env(&format!("heal-consent-{mode:?}"));
        e.host.cam_results.lock().unwrap().push_back(Err(NodeError::consent("This flow is not allowed to use the camera. Switch it on in the flow's settings.")));
        e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetConfig { path: "camera_allowed".into(), value: json!(true) }]))));
        e.publish(camera_flow(mode));
        let id = start(&e, "h");
        let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked || r.status.is_terminal()).unwrap();
        assert_eq!(r.status, RunStatus::Blocked, "{mode:?}");
        assert_eq!(e.host.heal_requests.lock().unwrap().len(), 0, "{mode:?}: the agent was not asked");
        assert_eq!(r.heal_spent, 0, "no attempt spent");
        assert_eq!(r.heal_cents, 0);
        assert_eq!(r.heal.len(), 1);
        let c = &r.heal[0];
        assert!(c.patch.is_none() && c.state == CardState::Proposed && c.note == iyakki_flow::driver::CONSENT_NOTE, "{c:?}");
        assert!(c.diagnosis.contains("needs your permission") && c.diagnosis.contains("not allowed to use the camera"), "{}", c.diagnosis);
        assert!(r.blocked.contains("needs you"), "{}", r.blocked);
        assert!(e.store.get_draft("h").unwrap().node("a").unwrap().config.get("camera_allowed").is_none(), "nothing was written");
    }
}

#[test]
fn a_proposal_that_touches_consent_or_invents_a_step_setting_is_refused_and_attributed_to_the_assistant() {
    let e = env("heal-bogus");
    // the assistant proposes a made-up step setting that would "allow" the camera (this is what the device showed)
    let mut f = camera_flow(HealMode::Suggest);
    f.defaults.camera.allowed = true;
    f.nodes[1].retry = Some(RetryPolicy::fixed(1, 0));
    e.host.cam_results.lock().unwrap().push_back(Err(NodeError::new("the camera gave no picture")));
    e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetConfig { path: "camera_allowed".into(), value: json!(true) }]))));
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked || r.status.is_terminal()).unwrap();
    let c = &r.heal[0];
    assert!(c.patch.is_none(), "refused patches are not offered: {c:?}");
    assert!(c.note.starts_with("refused:") && c.note.contains("camera_allowed"), "{}", c.note);
    assert!(e.store.get_draft("h").unwrap().node("a").unwrap().config.get("camera_allowed").is_none());
    // a step setting the kind does not have is refused even when its name looks harmless
    let e = env("heal-unknown");
    let mut f = camera_flow(HealMode::Suggest);
    f.defaults.camera.allowed = true;
    f.nodes[1].retry = Some(RetryPolicy::fixed(1, 0));
    e.host.cam_results.lock().unwrap().push_back(Err(NodeError::new("the camera gave no picture")));
    e.host.propose(Ok(proposal(patch("a", vec![PatchOp::SetConfig { path: "warm_up_seconds".into(), value: json!(2) }]))));
    e.publish(f);
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked || r.status.is_terminal()).unwrap();
    assert!(r.heal[0].note.contains("not a setting of this kind of step"), "{}", r.heal[0].note);
}

#[test]
fn an_approved_proposal_is_noted_as_the_assistants_and_a_pending_one_says_it_is_not_applied() {
    let e = env("heal-note");
    slow_server(&e);
    e.host.propose(Ok(proposal(set_timeout_5s())));
    e.publish(flow_timeout_default_small(flow_with(HealMode::Suggest)));
    let id = start(&e, "h");
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.heal[0].note, "proposed by the assistant, not applied until you approve");
    e.engine.card_decide(&id, &r.heal[0].id, CardDecision::Approve { by: "user:me".into() }).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    let note = &r.nodes["a"].note;
    assert!(note.starts_with("proposed by the assistant, approved by user:me"), "{note}");
    assert!(!note.contains("fixed by user"), "{note}");
}

#[test]
fn a_made_up_camera_key_on_a_step_is_an_error_and_never_grants_the_camera() {
    let reg = Registry::builtin();
    let mut f = camera_flow(HealMode::Off);
    f.nodes[1].config = json!({"camera_allowed": true});
    let issues = iyakki_flow::validate::validate_flow(&f, &reg);
    assert!(issues.iter().any(|i| i.level == iyakki_flow::kinds::Level::Error && i.msg.contains("camera_allowed")), "{issues:?}");
    // so the flow cannot even be published with it; the person removes it by clearing the step's settings (or discarding the draft)
    let e = env("heal-bogus-key");
    let id = f.id.clone();
    e.store.create(f.clone(), "user:me").unwrap();
    let err = e.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:me", label: "", changelog: "x", approved_by: None }).unwrap_err();
    assert!(err.contains("camera_allowed"), "{err}");
    assert!(!f.defaults.camera.allowed, "a step key never grants the camera: only the flow's own setting does");
}
