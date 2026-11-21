//! The jobs of the FLOWS panel (F-SPLIT-03, D-S5-01): every call the tile makes goes through `FlowsJobs`, here against the real in-process
//! environment, with the wire types round-tripped as the core socket carries them. No network, no waiting beyond the engine's own wait.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::flows::{build_flow, FlowEnv};
use iyakki_agent::flowspanel::RealFlowsPanel;
use iyakki_agent_api::FlowsJobs;
use iyakki_flow::engine::{CardDecision, StartRequest};
use iyakki_flow::model::RunStatus;
use serde_json::json;

use super::tests::cx_with_admin_store;

struct Fx {
    env: Arc<FlowEnv>,
    jobs: Arc<dyn FlowsJobs>,
}

fn fx(tag: &str) -> Fx {
    let cx = cx_with_admin_store(&format!("flowsjobs-{tag}"));
    let home = std::env::temp_dir().join(format!("iyakki-flows-jobs-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let env = Arc::new(FlowEnv::build_in(&iyakki_agent::services::Services::from_handle(&cx.services), &cx.session.user, &cx.session.policy, false, &home));
    Fx { jobs: Arc::new(RealFlowsPanel(env.clone())), env }
}

fn notify_flow(f: &Fx, name: &str) -> String {
    let spec = json!({"name": name, "nodes": [{"id": "say", "kind": "notify", "config": {"title": "hello"}}], "edges": [{"from": "start", "to": "say"}]});
    f.jobs.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap().id
}

#[test]
fn drafts_are_made_changed_published_listed_rolled_back_exported_and_deleted_through_the_drafts_job() {
    let f = fx("drafts");
    let id = notify_flow(&f, "Greeter");
    assert!(f.jobs.summaries().iter().any(|s| s.id == id && s.name == "Greeter" && s.published == 0));
    assert!(f.jobs.published(&id).is_none());
    assert!(f.jobs.get_draft(&id).is_some());
    assert!(f.jobs.issues(&f.jobs.get_draft(&id).unwrap()).iter().all(|i| i.level != iyakki_flow::kinds::Level::Error));
    let before = f.jobs.draft_stamp(&id);
    assert_ne!(before, (0, 0), "a written draft has a stamp");
    let mut draft = f.jobs.get_draft(&id).unwrap();
    draft.description = "says hello".into();
    f.jobs.save_edit(&draft, "user:root", "describe", "say").unwrap();
    assert_ne!(f.jobs.draft_stamp(&id), before, "an edit moves the stamp");
    assert_eq!(f.jobs.draft_stamp("no-such-flow"), (0, 0));
    let v1 = f.jobs.publish(&id, "user:root", "1.0", "first", Some("user:root".into())).unwrap();
    assert_eq!(v1, 1);
    assert_eq!(f.jobs.meta_of(&id).published, 1);
    assert_eq!(f.jobs.versions(&id).len(), 1);
    assert_eq!(f.jobs.get_version(&id, 1).unwrap().description, "says hello");
    assert!(f.jobs.publish(&id, "user:root", "", "again", Some("user:root".into())).is_err(), "nothing changed since the published version");
    let mut again = f.jobs.get_draft(&id).unwrap();
    again.description = "says hello twice".into();
    f.jobs.save_edit(&again, "user:root", "describe", "say").unwrap();
    assert_eq!(f.jobs.publish(&id, "user:root", "", "second", Some("user:root".into())).unwrap(), 2);
    assert_eq!(f.jobs.rollback(&id, 1, "user:root").unwrap(), 3);
    assert_eq!(f.jobs.get_draft(&id).unwrap().description, "says hello");
    assert!(f.jobs.rollback(&id, 9, "user:root").is_err());
    let text = f.jobs.export_json(&id).unwrap();
    let copy = f.jobs.import_json(&text, "user:root").unwrap();
    assert_ne!(copy.id, id, "an import is a new flow");
    assert!(f.jobs.from_template("no-such-template", "x", "user:root").is_err());
    assert!(f.jobs.drop_version(&id, 3).is_err(), "the published version is never dropped");
    f.jobs.drop_version(&id, 1).unwrap();
    assert_eq!(f.jobs.versions(&id).len(), 2);
    f.jobs.delete_flow(&copy.id).unwrap();
    assert!(f.jobs.get_draft(&copy.id).is_none());
    assert!(f.jobs.flow_names().iter().any(|(i, n)| i == &id && n == "Greeter"));
}

#[test]
fn a_run_is_started_watched_counted_and_refused_for_unknown_ones_through_the_runs_job() {
    let f = fx("runs");
    let id = notify_flow(&f, "Runner");
    assert!(f.jobs.start_run(StartRequest::new(&id)).is_err() || f.jobs.published(&id).is_none(), "a flow never published has no version to run");
    f.jobs.publish(&id, "user:root", "", "v1", Some("user:root".into())).unwrap();
    let rev = f.jobs.revision();
    let out = f.jobs.start_run(StartRequest::new(&id).trigger("manual", "user:root")).unwrap();
    assert!(!out.existing);
    f.env.engine.wait_for(&out.run_id, Duration::from_secs(30), |r| r.status.is_terminal()).unwrap();
    let run = f.jobs.get_run(&out.run_id).unwrap();
    assert_eq!(run.status, RunStatus::Succeeded);
    assert!(f.jobs.revision() > rev, "a finished run moves the revision the tile polls");
    assert!(f.jobs.run_summaries().iter().any(|s| s.id == out.run_id));
    assert!(f.jobs.run_counts().done >= 1);
    assert!(f.jobs.get_run("no-such-run").is_none());
    assert!(f.jobs.cancel("no-such-run").is_err());
    assert!(f.jobs.retry_node("no-such-run", "say").is_err());
    assert!(f.jobs.decide("no-such-run", "say", true, "user:root", "").is_err());
    assert!(f.jobs.card_decide("no-such-run", "c1", CardDecision::Approve { by: "user:root".into() }).is_err());
    assert!(f.jobs.node_updates(&id).is_empty());
    assert!(f.jobs.browser_session(&out.run_id).is_none());
    assert_eq!(f.jobs.withdraw_permissions(&out.run_id), 0);
    let flow = f.jobs.published(&id).unwrap();
    let tested = f.jobs.test_node(&flow, "say", json!({}), json!({})).unwrap();
    assert!(tested.ok, "a step is tried without a run: {}", tested.error);
    assert!(f.jobs.next_runs(&flow, 3).is_empty(), "a flow with no schedule has no next run");
}

#[test]
fn the_core_installs_the_script_runner_and_the_audit_goes_to_the_core_not_to_the_tile() {
    let f = fx("core-side");
    // the tile no longer installs the in-house JavaScript engine: the environment has it from the start
    assert!(f.env.host.js.lock().unwrap().is_some());
    let js = f.env.host.js.lock().unwrap().clone().unwrap();
    assert_eq!(js("return args.a + 1;", &json!({"a": 41}), 2_000).unwrap(), json!(42));
    assert!(js("while(true){}", &json!({}), 100).is_err(), "a script that never ends is stopped by the wall limit");
    // a line written through the job reaches the environment's audit sink with the action and the detail
    let log = Arc::new(Mutex::new(String::new()));
    let sink = log.clone();
    *f.env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |_, action, detail| sink.lock().unwrap().push_str(&format!("{action} {detail}\n"))));
    f.jobs.audit("flow.publish", "x v1 by user:root");
    assert_eq!(log.lock().unwrap().as_str(), "flow.publish x v1 by user:root\n");
    assert_eq!(f.jobs.who(), "user:root");
    assert!(f.jobs.who().ends_with(&f.jobs.user()));
}

#[test]
fn the_assistant_book_the_message_policy_and_the_channel_switch_answer_through_their_jobs() {
    let f = fx("books");
    let id = notify_flow(&f, "Quiet");
    let g = f.jobs.assist_global();
    assert!(g.enabled);
    f.jobs.assist_set_enabled(false).unwrap();
    assert!(!f.jobs.assist_global().enabled);
    f.jobs.assist_set_global_cap(3).unwrap();
    assert_eq!(f.jobs.assist_global().max_publish_per_day, 3);
    f.jobs.assist_set_matrix(true, "!room:example").unwrap();
    assert!(f.jobs.assist_global().matrix);
    assert!(!f.jobs.assist_paused(&id));
    f.jobs.assist_set_paused(&id, true).unwrap();
    assert!(f.jobs.assist_paused(&id) && f.jobs.assist_book(&id).paused);
    assert!(f.jobs.assist_open_list(&id).is_empty());
    assert!(f.jobs.assist_watch(&id).is_none());
    assert!(f.jobs.assist_set_paused("../escape", true).is_err(), "a flow id is checked by the core");
    assert!(f.jobs.assist_now() > 0);
    let ro = f.jobs.assist_readout(&id, f.jobs.assist_now(), 10);
    assert_eq!(ro["paused"], true);
    assert!(f.jobs.assist_adopt(&id, "no-such-suggestion").is_err());

    let mut p = f.jobs.msg_policy_get();
    p.escalate_room = "!ops:example".into();
    let saved = f.jobs.msg_policy_set(p).unwrap();
    assert_eq!(f.jobs.msg_policy_get().escalate_room, saved.escalate_room);
    assert!(f.jobs.msg_held().is_empty() && f.jobs.msg_watching().is_empty());
    assert_eq!(f.jobs.msg_ack(&id), 0);
    f.jobs.chan_set(&id, false).unwrap();
    assert!(!f.jobs.chan_allowed(&id));
    f.jobs.chan_set(&id, true).unwrap();
    assert!(f.jobs.chan_allowed(&id));
    assert!(f.jobs.hook_views(Some(&id)).is_empty());
    assert!(f.jobs.hook_enable("no-such-hook", "user:root").is_err());
}

#[test]
fn every_type_the_jobs_carry_survives_the_wire_and_a_hook_secret_does_not_cross() {
    use iyakki_flow::engine::{ApplyScope, Counts};
    fn back<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(v: T) {
        let text = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&text).unwrap(), v, "{text}");
    }
    back(Counts { scheduled: 1, queued: 2, running: 3, needs_you: 4, failed: 5, done: 6 });
    back(ApplyScope::Both);
    back(CardDecision::Reject { by: "u".into(), note: "n".into() });
    let req = StartRequest::new("f").trigger("manual", "u").draft();
    let wire = serde_json::to_value(&req).unwrap();
    assert_eq!(serde_json::to_value(serde_json::from_value::<StartRequest>(wire.clone()).unwrap()).unwrap(), wire);
    back(iyakki_flow::engine::StartOutcome { run_id: "r".into(), existing: true });
    back(iyakki_flow::kinds::Issue::warn("n", "m"));
    back(iyakki_flow::assist_run::WholeTrial { tried: vec!["a".into()], refused: vec![("b".into(), "effect".into())], failed: vec![] });
    back(iyakki_flow::host::BrowserSessionInfo { run_id: "r".into(), ..Default::default() });
    let made = iyakki_flowsx::hookview::Created { id: "h".into(), flow: "f".into(), path: "/hooks/h/t".into(), secret: Some("sekrit".into()) };
    let wire = serde_json::to_string(&made).unwrap();
    assert!(!wire.contains("sekrit"), "a hook's shared secret never crosses the core socket: {wire}");
    let got: iyakki_flowsx::hookview::Created = serde_json::from_str(&wire).unwrap();
    assert_eq!((got.id.as_str(), got.path.as_str(), got.secret), ("h", "/hooks/h/t", None));
}
