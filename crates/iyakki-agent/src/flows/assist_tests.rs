// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! Tests of FLOWS assist on the agent's side: the assistant (the chat's tools) and imports can never switch assist on or to autopilot,
//! a new flow starts with assist off, and a failed run goes through the real host with a scripted model (NO real model, NO real
//! network: the upstream is dead, so the run fails and the trial of the proposed fix fails honestly).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_flow::assist::*;
use iyakki_flow::assist_store::Status;
use iyakki_flow::{AssistMode, AssistPolicy, RunStatus, StartRequest};
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};

use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Scripted {
    answers: Mutex<Vec<String>>,
    seen: Mutex<Vec<crate::chat::Message>>,
}

impl crate::chat::ChatBackend for Scripted {
    fn complete(&self, h: &[crate::chat::Message]) -> Result<String, String> {
        self.seen.lock().unwrap().extend(h.iter().cloned());
        Ok(self.answers.lock().unwrap().pop().unwrap_or_else(|| "{}".into()))
    }
}

struct W {
    env: Arc<FlowEnv>,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn world(tag: &str) -> W {
    let dir = std::env::temp_dir().join(format!("iyakki-assist-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("assisttester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &dir.join("home")));
    W { env, dir }
}

fn check_flow(assist: AssistPolicy) -> iyakki_flow::Flow {
    let mut f = build_flow(&json!({"name": "Watch", "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "https://api.example.com/old"}}]})).unwrap();
    f.defaults.assist = assist;
    f
}

#[test]
fn the_assistant_cannot_switch_assist_on_and_a_new_flow_starts_off() {
    let w = world("api");
    let call = |name: &str, args: Value| w.env.call(name, &args, None);
    let made = call("flow_create", json!({"name": "Plain", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}]})).unwrap();
    assert!(made.contains("Made the draft"), "{made}");
    assert_eq!(w.env.store.get_draft("plain").unwrap().defaults.assist.mode, AssistMode::Off);
    for v in [json!({"mode": "autopilot", "auto_publish": true}), json!({"mode": "copilot"}), json!({"auto_publish": true}), json!({"max_publish_per_day": 50})] {
        let e = call("flow_edit", json!({"flow": "plain", "ops": [{"op": "set_default", "assist": v}]})).unwrap_err();
        assert!(e.contains("only the person can switch Assist"), "{e}");
    }
    let e = call("flow_create", json!({"name": "Sneaky", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}], "defaults": {"assist": {"mode": "autopilot"}}})).unwrap_err();
    assert!(e.contains("only the person"), "{e}");
    assert!(!w.env.store.exists("sneaky"));
    assert_eq!(w.env.store.get_draft("plain").unwrap().defaults.assist, AssistPolicy::default());
    // the person's choices in the same call are still accepted
    call("flow_edit", json!({"flow": "plain", "ops": [{"op": "set_default", "timeout_ms": 5000}]})).unwrap();
}

#[test]
fn an_imported_flow_a_store_template_and_a_sample_arrive_with_assist_off() {
    let w = world("import");
    let mut f = check_flow(AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() });
    f.id = "imported".into();
    let g = w.env.import_json(&serde_json::to_string(&f).unwrap(), "user:t").unwrap();
    assert_eq!(g.defaults.assist.mode, AssistMode::Off);
    assert!(!g.defaults.assist.auto_publish);
    let mut t = iyakki_flow::templates::get("cam-sleep-check").unwrap();
    t.flow.defaults.assist = AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() };
    let back = iyakki_flow::templates::from_json(&serde_json::to_string(&t).unwrap()).unwrap();
    assert_eq!(back.flow.defaults.assist, AssistPolicy::default());
    for t in iyakki_flow::templates::all() {
        assert_eq!(t.flow.defaults.assist.mode, AssistMode::Off, "{} starts with assist off", t.id);
    }
}

#[test]
fn a_failed_run_through_the_real_host_makes_a_copilot_suggestion_from_the_scripted_model() {
    let w = world("e2e");
    let model = Arc::new(Scripted {
        answers: Mutex::new(vec![r#"{"diagnosis": "the address moved", "patch": {"node": "call", "reason": "moved", "ops": [{"op": "set_config", "path": "url", "value": "https://api.example.com/new"}]}}"#.into()]),
        seen: Mutex::new(vec![]),
    });
    *w.env.host.backend.lock().unwrap() = Some(model.clone());
    let f = check_flow(AssistPolicy { mode: AssistMode::Autopilot, auto_publish: true, ..Default::default() });
    let id = f.id.clone();
    w.env.create_draft(f, "user:t").unwrap();
    w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let secret = "tok_Zq8Wm3Xk9Vn2Bd7L";
    let o = w.env.engine.start_run(StartRequest::new(&id).inputs(json!({"api": secret}))).unwrap();
    let r = w.env.engine.wait_for(&o.run_id, Duration::from_secs(30), |r| r.status.is_terminal() || r.status == RunStatus::Blocked).unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    let lines = w.env.assist_tick();
    assert!(lines.iter().any(|l| l.contains("suggestion s1")), "{lines:?}");
    // the dead network means the trial of the new address fails, so even autopilot with auto_publish only asks
    let list = w.env.open_suggestions(&id);
    assert_eq!(list.len(), 1);
    let s = &list[0];
    assert_eq!(s.status, Status::Pending);
    assert!(matches!(s.trial, TrialState::Failed(_)), "{:?}", s.trial);
    assert!(s.asks.contains("trial"), "{}", s.asks);
    assert_eq!(w.env.store.published(&id).unwrap().meta.version, 1, "nothing was published");
    // the model saw the failure and no input value
    let seen: String = model.seen.lock().unwrap().iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(seen.contains("FAILING NODE call") && !seen.contains(secret), "{seen}");
    // one failure is answered once; the person's side works through the env
    assert!(w.env.assist_tick().iter().all(|l| !l.contains("suggestion s")));
    let a = w.env.with_assistant(|a| a.adopt(&id, "s1"));
    assert!(a.is_ok(), "{a:?}");
    let v = w.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "fix", approved_by: Some("user:t") }).unwrap();
    w.env.assist_after_publish(&id, v);
    assert_eq!(w.env.assist_store().get(&id, "s1").unwrap().status, Status::Approved);
    assert!(w.env.open_suggestions(&id).is_empty());
}

#[test]
fn flow_publish_and_rollback_still_ask_every_time() {
    let p = Role::Admin.template();
    let defs = super::tools::defs(&p);
    for name in ["flow_publish", "flow_rollback", "flow_apply_to_runs", "flow_delete", "flow_drop_version"] {
        let d = defs.iter().find(|d| d.spec.name == name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(d.risk, crate::agent::Risk::Ask, "{name}");
        assert!(crate::agent::hard_ask(name, &json!({})).is_some(), "{name} is hard-ask, also in Auto and Autopilot");
    }
}
