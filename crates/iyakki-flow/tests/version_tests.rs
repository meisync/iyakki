mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use common::*;
use iyakki_flow::store::PublishReq;
use iyakki_flow::*;
use serde_json::json;

fn gated(id: &str, track: Track) -> Flow {
    let mut f = Flow::new(id, id);
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("gate", "approval", json!({"message": "go?"})));
    let mut b = Node::new("b", "http", json!({"url": "https://x.example/v1", "extract": {"n": "count"}}));
    b.track = track;
    f.nodes.push(b);
    f.nodes.push(Node::new("e", "end", json!({})));
    f.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "b").port("approved"), Edge::new("b", "e")];
    f
}

fn publish_change(e: &Env, id: &str, f: impl FnOnce(&mut Flow)) -> u32 {
    let mut d = e.store.get_draft(id).unwrap();
    f(&mut d);
    e.store.save_draft(&d, None).unwrap();
    e.store.publish(id, &PublishReq { by: "user:me", label: "", changelog: "change", approved_by: None }).unwrap()
}

fn last_url(e: &Env) -> String {
    e.host.http_calls.lock().unwrap().last().map(|c| c.url.clone()).unwrap_or_default()
}

#[test]
fn a_run_pins_the_version_it_started_with() {
    let e = env("pin");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(gated("v", Track::Pinned));
    let o = e.engine.start_run(StartRequest::new("v")).unwrap();
    e.finish(&o.run_id);
    publish_change(&e, "v", |f| f.node_mut("b").unwrap().config["url"] = json!("https://x.example/v2"));
    e.engine.decide(&o.run_id, "gate", true, "me", "").unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(last_url(&e), "https://x.example/v1", "pinned: the run goes on with its own version");
    assert_eq!(r.flow.meta.version, 1);
    let n = e.engine.start_run(StartRequest::new("v")).unwrap();
    assert_eq!(e.engine.get_run(&n.run_id).unwrap().flow.meta.version, 2, "a new run takes the latest published version");
    // and an older version can be run on purpose
    let o1 = e.engine.start_run(StartRequest { version: Some(1), ..StartRequest::new("v") }).unwrap();
    assert_eq!(e.engine.get_run(&o1.run_id).unwrap().flow.meta.version, 1);
    assert!(e.engine.start_run(StartRequest { version: Some(9), ..StartRequest::new("v") }).is_err());
}

#[test]
fn a_node_that_tracks_the_latest_published_version_takes_it_at_its_boundary() {
    let e = env("track-latest");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(gated("v", Track::LatestPublished));
    let o = e.engine.start_run(StartRequest::new("v")).unwrap();
    e.finish(&o.run_id);
    publish_change(&e, "v", |f| f.node_mut("b").unwrap().config["url"] = json!("https://x.example/v2"));
    e.engine.decide(&o.run_id, "gate", true, "me", "").unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(last_url(&e), "https://x.example/v2");
    assert!(r.nodes["b"].note.contains("tracks the latest published version"), "{}", r.nodes["b"].note);
    assert_eq!(r.flow.meta.version, 1, "the run is still the run of v1; only this node moved");
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.node.updated")));
}

#[test]
fn compatible_tracking_only_takes_a_new_node_that_keeps_its_ports_and_outputs() {
    for (change_outputs, want) in [(false, "https://x.example/v2"), (true, "https://x.example/v1")] {
        let e = env("track-compat");
        e.host.on_http(|_| ok_json("{}"));
        e.publish(gated("v", Track::Compatible));
        let o = e.engine.start_run(StartRequest::new("v")).unwrap();
        e.finish(&o.run_id);
        publish_change(&e, "v", |f| {
            let n = f.node_mut("b").unwrap();
            n.config["url"] = json!("https://x.example/v2");
            if change_outputs {
                n.config["extract"] = json!({"other": "x"});
            }
        });
        e.engine.decide(&o.run_id, "gate", true, "me", "").unwrap();
        e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
        assert_eq!(last_url(&e), want, "outputs changed: {change_outputs}");
    }
}

#[test]
fn node_updates_are_counted_and_applied_to_scheduled_runs_now_and_running_ones_at_the_boundary() {
    let e = env("apply");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(gated("v", Track::Pinned));
    let running = e.engine.start_run(StartRequest::new("v")).unwrap().run_id;
    e.finish(&running);
    let later = e.clock.now_ms() + 3_600_000;
    let scheduled = e.engine.start_run(StartRequest::new("v").at(later)).unwrap().run_id;
    assert!(e.engine.node_updates("v").is_empty(), "nothing is newer yet");
    publish_change(&e, "v", |f| f.node_mut("b").unwrap().config["url"] = json!("https://x.example/v2"));
    let ups = e.engine.node_updates("v");
    assert_eq!(ups.len(), 1);
    assert_eq!((ups[0].node.as_str(), ups[0].to_version, ups[0].compatible), ("b", 2, true));
    assert_eq!(ups[0].scheduled, vec![scheduled.clone()]);
    assert_eq!(ups[0].running, vec![running.clone()]);
    // only the scheduled run takes it now
    let rep = e.engine.apply_node_update("v", "b", ApplyScope::Scheduled, "user:me", false).unwrap();
    assert_eq!(rep.scheduled_updated, vec![scheduled.clone()]);
    assert!(rep.running_pending.is_empty());
    assert_eq!(e.engine.get_run(&scheduled).unwrap().flow.node("b").unwrap().config["url"], "https://x.example/v2");
    assert!(e.engine.get_run(&scheduled).unwrap().nodes["b"].note.contains("updated to v2"));
    assert_eq!(e.engine.get_run(&running).unwrap().flow.node("b").unwrap().config["url"], "https://x.example/v1");
    assert_eq!(e.engine.node_updates("v")[0].scheduled.len(), 0);
    // the running one is told, and takes it only when the node is about to begin
    let rep = e.engine.apply_node_update("v", "b", ApplyScope::Running, "user:me", false).unwrap();
    assert_eq!(rep.running_pending, vec![running.clone()]);
    assert!(e.engine.get_run(&running).unwrap().pending_updates.contains_key("b"));
    assert_eq!(e.engine.get_run(&running).unwrap().flow.node("b").unwrap().config["url"], "https://x.example/v1", "not yet");
    e.engine.decide(&running, "gate", true, "me", "").unwrap();
    let r = e.engine.wait_for(&running, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(last_url(&e), "https://x.example/v2");
    assert!(r.nodes["b"].note.contains("took the update to v2"), "{}", r.nodes["b"].note);
    assert!(r.pending_updates.is_empty());
    // a run that has already done the node is left alone
    let rep = e.engine.apply_node_update("v", "b", ApplyScope::Both, "user:me", false).unwrap();
    assert!(rep.skipped_started.contains(&running) || rep.skipped_started.is_empty());
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.node.apply")));
}

#[test]
fn an_update_is_never_applied_in_the_middle_of_a_node() {
    let e = env("midnode");
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (started_tx, go_rx) = (Mutex::new(started_tx), Mutex::new(go_rx));
    let first = Arc::new(AtomicBool::new(true));
    let f2 = first.clone();
    e.host.on_http(move |_| {
        if f2.swap(false, Ordering::SeqCst) {
            let _ = started_tx.lock().unwrap().send(());
            let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
        }
        ok_json("{}")
    });
    let mut f = Flow::new("m", "M");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("b", "http", json!({"url": "https://x.example/v1"})));
    f.edges.push(Edge::new("s", "b"));
    e.publish(f);
    let id = e.engine.start_run(StartRequest::new("m")).unwrap().run_id;
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    publish_change(&e, "m", |f| f.node_mut("b").unwrap().config["url"] = json!("https://x.example/v2"));
    assert!(e.engine.node_updates("m").is_empty(), "b already began: it is not offered");
    let rep = e.engine.apply_node_update("m", "b", ApplyScope::Both, "user:me", false).unwrap();
    assert_eq!(rep.skipped_started, vec![id.clone()]);
    assert!(rep.running_pending.is_empty() && rep.scheduled_updated.is_empty());
    go_tx.send(()).unwrap();
    let r = e.engine.wait_for(&id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(e.host.http_calls.lock().unwrap()[0].url, "https://x.example/v1", "the call in flight kept its definition");
}

#[test]
fn an_incompatible_update_needs_an_explicit_yes() {
    let e = env("incompat");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(gated("v", Track::Pinned));
    let id = e.engine.start_run(StartRequest::new("v").at(e.clock.now_ms() + 1_000_000)).unwrap().run_id;
    publish_change(&e, "v", |f| f.node_mut("b").unwrap().config["extract"] = json!({"renamed": "count"}));
    let ups = e.engine.node_updates("v");
    assert!(!ups[0].compatible, "the output names changed");
    let rep = e.engine.apply_node_update("v", "b", ApplyScope::Both, "user:me", false).unwrap();
    assert_eq!(rep.skipped_incompatible, vec![id.clone()]);
    assert!(rep.scheduled_updated.is_empty());
    let rep = e.engine.apply_node_update("v", "b", ApplyScope::Both, "user:me", true).unwrap();
    assert_eq!(rep.scheduled_updated, vec![id]);
}

#[test]
fn the_agent_can_pull_a_fixed_node_into_other_runs_with_the_same_approval_path() {
    // the heal fix goes to the draft; once it is published, other runs get it through apply_node_update (a hard-ask in the tools)
    let e = env("pull-fix");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(gated("v", Track::Pinned));
    let other = e.engine.start_run(StartRequest::new("v").at(e.clock.now_ms() + 1_000_000)).unwrap().run_id;
    let fixed = e.store.save_draft_fix("v", &{
        let mut f = e.store.get_draft("v").unwrap();
        f.node_mut("b").unwrap().timeout_ms = Some(9000);
        f
    }, "b", "agent:assistant", "timeout 9000 ms", true).unwrap();
    assert_eq!(fixed, 1);
    // a draft is not published: other runs are not offered it
    assert!(e.engine.node_updates("v").is_empty());
    assert!(e.store.publish("v", &PublishReq { by: "agent:assistant", label: "", changelog: "x", approved_by: None }).is_err(), "the agent cannot publish its own change");
    e.store.publish("v", &PublishReq { by: "user:me", label: "", changelog: "agent's fix", approved_by: Some("user:me") }).unwrap();
    let ups = e.engine.node_updates("v");
    assert_eq!(ups[0].scheduled, vec![other.clone()]);
    e.engine.apply_node_update("v", "b", ApplyScope::Scheduled, "user:me", false).unwrap();
    assert_eq!(e.engine.get_run(&other).unwrap().flow.node("b").unwrap().timeout_ms, Some(9000));
}

#[test]
fn library_nodes_are_shared_versioned_and_tracked() {
    let e = env("lib");
    e.host.on_http(|_| ok_json("{}"));
    let base = Node::new("x", "http", json!({"url": "https://x.example/lib1"}));
    assert_eq!(e.store.lib_publish("ping", &base, "user:me").unwrap(), 1);
    let mut f = Flow::new("uses", "Uses");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut n = Node::new("p", "http", json!({}));
    n.lib = Some(LibRef { id: "ping".into(), version: 1 });
    n.track = Track::LatestPublished;
    f.nodes.push(n);
    f.edges.push(Edge::new("s", "p"));
    e.publish(f);
    assert_eq!(e.store.lib_usage("ping").iter().filter(|u| u.2 == "published").count(), 1);
    e.run("uses");
    assert_eq!(last_url(&e), "https://x.example/lib1");
    let mut v2 = base.clone();
    v2.config["url"] = json!("https://x.example/lib2");
    e.store.lib_publish("ping", &v2, "user:me").unwrap();
    // a new run of the same published flow sees the latest library version for a node that tracks the latest
    e.run("uses");
    assert_eq!(last_url(&e), "https://x.example/lib2");
}
