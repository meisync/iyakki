//! The `remote_flow` node in the engine against a scripted host: it validates a saved machine's name (never an address), builds the
//! request the host gets, turns a run that failed, waits for a person or is still going over there into a failed step, and keeps
//! the sign-in out of everything (the node holds no token at all: the host holds it).

mod common;

use common::*;
use iyakki_flow::host::{RemoteFlowResult, FakeHost};
use iyakki_flow::*;
use serde_json::{json, Value};

fn issues(cfg: Value) -> Vec<String> {
    let mut f = Flow::new("t", "T");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("n", "remote_flow", cfg));
    f.edges.push(Edge::new("s", "n"));
    iyakki_flow::validate::validate_flow(&f, &Registry::builtin()).into_iter().filter(|i| i.node.as_deref() == Some("n") && i.level == iyakki_flow::kinds::Level::Error).map(|i| i.msg).collect()
}

fn flow_with(cfg: Value) -> Flow {
    let mut f = Flow::new("f", "F");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("r", "remote_flow", cfg));
    f.edges.push(Edge::new("s", "r"));
    f
}

#[test]
fn the_node_is_in_the_palette_and_checks_its_settings() {
    let r = Registry::builtin();
    let k = r.get("remote_flow").unwrap();
    assert_eq!(k.category(), "Call");
    assert!(!k.blurb().is_empty() && !k.schema().is_empty());
    assert!(issues(json!({"machine": "lab", "flow": "nightly"})).is_empty());
    assert!(issues(json!({"machine": "https://evil.example", "flow": "nightly"})).iter().any(|m| m.contains("saved name")));
    assert!(issues(json!({"machine": "lab", "flow": "../x"})).iter().any(|m| m.contains("flow id")));
    assert!(!issues(json!({"machine": "", "flow": ""})).is_empty());
    assert!(issues(json!({"machine": "lab", "flow": "n", "wait_s": 0})).iter().any(|m| m.contains("wait")));
    assert!(issues(json!({"machine": "lab", "flow": "n", "wait_s": 99999})).iter().any(|m| m.contains("wait")));
    assert!(issues(json!({"machine": "lab", "flow": "n", "inputs": [1]})).iter().any(|m| m.contains("object")));
}

#[test]
fn a_run_that_succeeds_over_there_gives_its_run_status_and_steps() {
    let e = env("rf-ok");
    e.host.remote_results.lock().unwrap().push_back(Ok(RemoteFlowResult { run: "r9".into(), status: "succeeded".into(), ended: true, steps: vec![("a".into(), "succeeded".into()), ("b".into(), "succeeded".into())], ..Default::default() }));
    e.publish(flow_with(json!({"machine": "lab", "flow": "nightly", "inputs": {"x": 1}, "wait_s": 60})));
    let run = e.run("f");
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    let out = &run.nodes["r"].output;
    assert_eq!((out["run"].as_str(), out["status"].as_str(), out["machine"].as_str()), (Some("r9"), Some("succeeded"), Some("lab")));
    assert_eq!(out["steps"].as_array().unwrap().len(), 2);
    let calls = e.host.remote_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].machine.as_str(), calls[0].flow.as_str(), calls[0].wait_s), ("lab", "nightly", 60));
    assert_eq!(calls[0].inputs, json!({"x": 1}));
}

#[test]
fn a_failed_waiting_or_unfinished_run_over_there_fails_the_step_in_plain_words() {
    for (res, want) in [
        (RemoteFlowResult { run: "r1".into(), status: "failed".into(), ended: true, error: "disk full".into(), ..Default::default() }, "failed: disk full"),
        (RemoteFlowResult { run: "r1".into(), status: "needs you".into(), waiting: "asks: delete the old files?".into(), ..Default::default() }, "waits for a person there"),
        (RemoteFlowResult { run: "r1".into(), status: "running".into(), ..Default::default() }, "still running after"),
    ] {
        let e = env("rf-bad");
        e.host.remote_results.lock().unwrap().push_back(Ok(res));
        let mut f = flow_with(json!({"machine": "lab", "flow": "nightly"}));
        f.nodes[1].retry = None;
        e.publish(f);
        let run = e.run("f");
        assert_eq!(run.status, RunStatus::Failed, "{want}: {run:?}");
        let msg = format!("{:?}", run.nodes["r"]);
        assert!(msg.contains(want), "{want}: {msg}");
    }
}

#[test]
fn a_host_that_cannot_do_it_says_so_and_nothing_is_sent() {
    // the trait default: a host that does not run flows on other machines
    struct Plain;
    impl iyakki_flow::host::Host for Plain {}
    let r = iyakki_flow::host::Host::remote_flow(&Plain, &iyakki_flow::host::RemoteFlowRequest::default(), &Cancel::new());
    assert!(r.unwrap_err().message.contains("not available"));
    let _ = FakeHost::new();
}
