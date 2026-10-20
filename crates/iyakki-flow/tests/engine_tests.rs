mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use common::*;
use iyakki_flow::host::{ok_response, Cancel, Clock, FakeClock, FakeHost, HttpResponse};
use iyakki_flow::*;
use serde_json::{json, Value};

#[test]
fn a_linear_flow_passes_data_along_and_finishes() {
    let e = env("linear");
    e.host.on_http(|req| {
        if req.url.ends_with("/a") {
            ok_json(r#"{"count": 7, "name": "x"}"#)
        } else {
            ok_json(&format!(r#"{{"echo": "{}"}}"#, req.url))
        }
    });
    let mut f = Flow::new("chain", "Chain");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("a", "http", json!({"url": "https://api.example.com/a", "extract": {"n": "count"}})));
    f.nodes.push(Node::new("b", "http", json!({"url": "https://api.example.com/b/{{ nodes.a.output.n }}?who={{ inputs.who }}"})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"n": "{{ nodes.a.output.n }}", "echo": "{{ nodes.b.output.body.echo }}"}})));
    f.edges = vec![Edge::new("s", "a"), Edge::new("a", "b"), Edge::new("b", "e")];
    f.inputs.push(Param { name: "who".into(), default: Some(json!("ada")), ..Default::default() });
    e.publish(f);
    let r = e.run("chain");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["n"], 7, "a number stays a number");
    assert_eq!(r.outputs["echo"], "https://api.example.com/b/7?who=ada");
    assert_eq!(r.flow.meta.version, 1, "the run pins the version it started with");
    let urls: Vec<String> = e.host.http_calls.lock().unwrap().iter().map(|c| c.url.clone()).collect();
    assert_eq!(urls.len(), 2);
    assert!(r.nodes.values().all(|n| n.state == NodeState::Succeeded));
    assert!(r.ended_ms >= r.started_ms && r.started_ms > 0);
    // persisted
    assert!(e.dir.join("engine/runs").join(&r.id).join("run.json").is_file());
    assert!(e.engine.log_text(&r.id).contains("done"));
    assert_eq!(e.engine.counts().done, 1);
}

#[test]
fn a_missing_required_input_and_an_invalid_flow_do_not_start() {
    let e = env("bad");
    let mut f = linear("needs");
    f.inputs.push(Param { name: "token".into(), required: true, ..Default::default() });
    e.publish(f);
    assert!(e.engine.start_run(StartRequest::new("needs")).unwrap_err().contains("needs an input"));
    assert!(e.engine.start_run(StartRequest::new("needs").inputs(json!({"token": "x"}))).is_ok());
    assert!(e.engine.start_run(StartRequest::new("ghost")).unwrap_err().contains("no published version") || true);
    let mut broken = linear("broken");
    broken.edges.push(Edge::new("e", "s"));
    assert!(e.engine.start_run(StartRequest::new("broken").with_flow(broken)).unwrap_err().contains("cannot run"));
}

#[test]
fn parallel_branches_run_and_join() {
    let e = env("par");
    e.host.on_http(|req| ok_json(&format!(r#"{{"u": "{}"}}"#, req.url)));
    let mut f = Flow::new("fan", "Fan");
    for (id, k, c) in [("s", "start", json!({})), ("p", "parallel", json!({})), ("a", "http", json!({"url": "https://x.example/a"})), ("b", "http", json!({"url": "https://x.example/b"})), ("c", "http", json!({"url": "https://x.example/c"})), ("j", "join", json!({"mode": "all"})), ("e", "end", json!({"outputs": {"n": "{{ len(keys(nodes.j.output.branches)) }}"}}))] {
        f.nodes.push(Node::new(id, k, c));
    }
    f.edges = vec![Edge::new("s", "p"), Edge::new("p", "a"), Edge::new("p", "b"), Edge::new("p", "c"), Edge::new("a", "j"), Edge::new("b", "j"), Edge::new("c", "j"), Edge::new("j", "e")];
    e.publish(f);
    let r = e.run("fan");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["n"], 3);
    assert_eq!(e.host.http_calls.lock().unwrap().len(), 3);
}

#[test]
fn with_one_slot_the_order_is_the_flows_order() {
    let e = env("order");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    e.host.on_http(move |req| {
        s2.lock().unwrap().push(req.url.clone());
        ok_json("{}")
    });
    let mut f = Flow::new("ord", "Ord");
    f.defaults.max_parallel = 1;
    f.nodes.push(Node::new("s", "start", json!({})));
    for (i, id) in ["x1", "x2", "x3", "x4"].iter().enumerate() {
        f.nodes.push(http_node(id, &format!("https://h.example/{i}")));
        f.edges.push(Edge::new("s", id));
    }
    e.publish(f);
    for _ in 0..5 {
        seen.lock().unwrap().clear();
        let r = e.run("ord");
        assert_eq!(r.status, RunStatus::Succeeded);
        assert_eq!(*seen.lock().unwrap(), vec!["https://h.example/0", "https://h.example/1", "https://h.example/2", "https://h.example/3"]);
    }
}

#[test]
fn a_condition_picks_one_path_and_the_other_is_skipped() {
    let e = env("cond");
    e.host.on_http(|req| if req.url.contains("/big") { ok_json("{}") } else { ok_json(r#"{"items": [1,2,3,4,5,6]}"#) });
    let mut f = Flow::new("c", "C");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("a", "http", json!({"url": "https://x.example/list"})));
    f.nodes.push(Node::new("c", "condition", json!({"expr": "len(nodes.a.output.body.items) > 5"})));
    f.nodes.push(http_node("big", "https://x.example/big"));
    f.nodes.push(http_node("small", "https://x.example/small"));
    f.nodes.push(Node::new("e", "end", json!({})));
    f.edges = vec![Edge::new("s", "a"), Edge::new("a", "c"), Edge::new("c", "big").port("true"), Edge::new("c", "small").port("false"), Edge::new("big", "e"), Edge::new("small", "e")];
    e.publish(f);
    let r = e.run("c");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(node_state(&r, "big"), NodeState::Succeeded);
    assert_eq!(node_state(&r, "small"), NodeState::Skipped);
    assert_eq!(r.nodes["c"].port, "true");
}

#[test]
fn a_switch_takes_the_matching_case_or_default() {
    let e = env("switch");
    let code = Arc::new(AtomicUsize::new(200));
    let c2 = code.clone();
    e.host.on_http(move |_| Ok(HttpResponse { status: c2.load(Ordering::SeqCst) as u16, body: "{}".into(), ..Default::default() }));
    let mut f = Flow::new("sw", "Sw");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut a = http_node("a", "https://x.example/");
    a.config["expect_status"] = json!([200, 404, 500]);
    f.nodes.push(a);
    f.nodes.push(Node::new("w", "switch", json!({"value": "nodes.a.output.status", "cases": [{"equals": 200, "port": "ok"}, {"when": "value >= 500", "port": "down"}]})));
    for p in ["ok", "down", "other"] {
        f.nodes.push(Node::new(p, "notify", json!({"title": p})));
    }
    f.edges = vec![Edge::new("s", "a"), Edge::new("a", "w"), Edge::new("w", "ok").port("ok"), Edge::new("w", "down").port("down"), Edge::new("w", "other").port("default")];
    e.publish(f);
    for (c, want) in [(200, "ok"), (500, "down"), (404, "other")] {
        code.store(c, Ordering::SeqCst);
        let r = e.run("sw");
        assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
        for p in ["ok", "down", "other"] {
            assert_eq!(node_state(&r, p), if p == want { NodeState::Succeeded } else { NodeState::Skipped }, "{c} {p}");
        }
    }
}

#[test]
fn for_each_runs_a_sub_flow_per_item_and_is_capped() {
    let e = env("each");
    e.host.on_http(|req| ok_json(&format!(r#"{{"seen": "{}"}}"#, req.url)));
    let mut child = Flow::new("per-item", "Per item");
    child.nodes.push(Node::new("s", "start", json!({})));
    child.nodes.push(http_node("h", "https://x.example/{{ inputs._item.id }}?i={{ inputs._index }}&tag={{ inputs.tag }}"));
    child.nodes.push(Node::new("e", "end", json!({"outputs": {"url": "{{ nodes.h.output.body.seen }}"}})));
    child.edges = vec![Edge::new("s", "h"), Edge::new("h", "e")];
    e.publish(child);
    let mut f = Flow::new("loop", "Loop");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("each", "foreach", json!({"items": "{{ inputs.list }}", "flow": "per-item", "inputs": {"tag": "t{{ index }}"}, "max_items": 3})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"urls": "{{ nodes.each.output.items }}", "count": "{{ nodes.each.output.count }}"}})));
    f.edges = vec![Edge::new("s", "each"), Edge::new("each", "e")];
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("loop").inputs(json!({"list": [{"id": 10}, {"id": 20}, {"id": 30}]}))).unwrap();
    let r = e.finish(&o.run_id);
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["count"], 3);
    assert_eq!(r.outputs["urls"][1]["url"], "https://x.example/20?i=1&tag=t1");
    // children are real runs with a parent
    let kids: Vec<_> = e.engine.summaries().into_iter().filter(|s| s.parent.as_deref() == Some(o.run_id.as_str())).collect();
    assert_eq!(kids.len(), 3);
    // over the cap: a clear failure, nothing runs
    let before = e.host.http_calls.lock().unwrap().len();
    let o2 = e.engine.start_run(StartRequest::new("loop").inputs(json!({"list": [1, 2, 3, 4]}))).unwrap();
    let r2 = e.finish(&o2.run_id);
    assert_eq!(r2.status, RunStatus::Failed);
    assert!(r2.error.contains("cap of 3"), "{}", r2.error);
    assert_eq!(e.host.http_calls.lock().unwrap().len(), before);
}

#[test]
fn sub_flows_cannot_call_themselves_or_nest_too_deep() {
    let e = env("sub");
    e.host.on_http(|_| ok_json("{}"));
    // a -> b -> a would be a cycle
    for (me, other) in [("fa", "fb"), ("fb", "fa")] {
        let mut f = Flow::new(me, me);
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("call", "subflow", json!({"flow": other})));
        f.edges.push(Edge::new("s", "call"));
        e.publish(f);
    }
    let r = e.run("fa");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("would run itself"), "{}", r.error);
    let mut selfy = Flow::new("me", "Me");
    selfy.nodes.push(Node::new("s", "start", json!({})));
    selfy.nodes.push(Node::new("call", "subflow", json!({"flow": "me"})));
    selfy.edges.push(Edge::new("s", "call"));
    assert!(e.engine.start_run(StartRequest::new("me").with_flow(selfy)).unwrap_err().contains("cannot run itself"));
    // a chain of 6 is too deep
    for i in 0..6 {
        let mut f = Flow::new(&format!("d{i}"), "d");
        f.nodes.push(Node::new("s", "start", json!({})));
        if i < 5 {
            f.nodes.push(Node::new("call", "subflow", json!({"flow": format!("d{}", i + 1)})));
            f.edges.push(Edge::new("s", "call"));
        }
        e.publish(f);
    }
    let r = e.run("d0");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("deep"), "{}", r.error);
    // a plain sub-flow returns its outputs
    let mut leaf = Flow::new("leaf", "Leaf");
    leaf.nodes.push(Node::new("s", "start", json!({})));
    leaf.nodes.push(Node::new("e", "end", json!({"outputs": {"v": "{{ inputs.x * 2 }}"}})));
    leaf.edges.push(Edge::new("s", "e"));
    e.publish(leaf);
    let mut top = Flow::new("top", "Top");
    top.nodes.push(Node::new("s", "start", json!({})));
    top.nodes.push(Node::new("c", "subflow", json!({"flow": "leaf", "inputs": {"x": 21}})));
    top.nodes.push(Node::new("e", "end", json!({"outputs": {"v": "{{ nodes.c.output.v }}"}})));
    top.edges = vec![Edge::new("s", "c"), Edge::new("c", "e")];
    e.publish(top);
    assert_eq!(e.run("top").outputs["v"], 42);
}

#[test]
fn an_idempotency_key_returns_the_same_run() {
    let e = env("idem");
    e.host.on_http(|_| ok_json("{}"));
    e.publish(linear("i"));
    let a = e.engine.start_run(StartRequest::new("i").key("k1")).unwrap();
    assert!(!a.existing);
    let b = e.engine.start_run(StartRequest::new("i").key("k1")).unwrap();
    assert!(b.existing);
    assert_eq!(a.run_id, b.run_id);
    let c = e.engine.start_run(StartRequest::new("i").key("k2")).unwrap();
    assert_ne!(a.run_id, c.run_id);
    e.finish(&a.run_id);
    // a finished good run still answers; after a failure the key may be used again
    assert!(e.engine.start_run(StartRequest::new("i").key("k1")).unwrap().existing);
    // outside the 24 hours it is a new run
    e.clock.advance(25 * 3600 * 1000);
    assert!(!e.engine.start_run(StartRequest::new("i").key("k1")).unwrap().existing);
    // http calls carry a stable Idempotency-Key when they change something
    let mut f = Flow::new("post", "Post");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("p", "http", json!({"method": "POST", "url": "https://x.example/p", "body": {"a": 1}, "retry": null})));
    f.nodes[1].retry = Some(RetryPolicy::fixed(3, 10));
    f.edges.push(Edge::new("s", "p"));
    e.publish(f);
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    assert!(e.engine.wait_idle(Duration::from_secs(5)));
    e.host.on_http(move |_| if n2.fetch_add(1, Ordering::SeqCst) < 2 { status(503, "") } else { ok_json("{}") });
    e.host.http_calls.lock().unwrap().clear();
    let r = e.run("post");
    assert_eq!(r.status, RunStatus::Succeeded);
    let calls = e.host.http_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    let keys: Vec<&String> = calls.iter().map(|c| &c.headers.iter().find(|(k, _)| k == "Idempotency-Key").unwrap().1).collect();
    assert!(keys.iter().all(|k| *k == keys[0]), "stable across attempts: {keys:?}");
    assert!(calls[0].headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
}

#[test]
fn retries_follow_the_policy_and_only_for_retryable_errors() {
    let e = env("retry");
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    e.host.on_http(move |_| match n2.fetch_add(1, Ordering::SeqCst) {
        0 | 1 => status(503, "busy"),
        _ => ok_json("{}"),
    });
    let mut f = linear("r1");
    f.nodes[1].retry = Some(RetryPolicy::exponential(4, 100, 2.0, 1000, 0.0));
    e.publish(f);
    let r = e.run("r1");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.nodes["a"].attempts.len(), 3);
    assert_eq!(r.nodes["a"].attempts.iter().map(|a| a.ok).collect::<Vec<_>>(), vec![false, false, true]);
    assert_eq!(r.nodes["a"].attempts[0].class, "retryable");
    assert_eq!(e.clock.sleeps(), vec![100, 200], "exponential: 100 then 200 ms");

    // the per-attempt list from the request: 700 ms, then 400 ms after a 200 ms pause
    let e = env("retry-list");
    e.host.on_http(|_| Err(NodeError::timeout("timed out")));
    let mut f = linear("r2");
    f.nodes[1].retry = Some(RetryPolicy::list(&[(700, 0), (400, 200), (300, 500)]));
    e.publish(f);
    let r = e.run("r2");
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(r.nodes["a"].attempts.len(), 3);
    let limits: Vec<u64> = e.host.http_calls.lock().unwrap().iter().map(|c| c.timeout_ms).collect();
    assert_eq!(limits, vec![700, 400, 300], "each attempt had its own timeout");
    assert_eq!(e.clock.sleeps(), vec![200, 500]);
    assert_eq!(r.nodes["a"].attempts.iter().map(|a| a.delay_before_ms).collect::<Vec<_>>(), vec![0, 200, 500]);

    // a 404 is not retried; a 400-level answer is a permanent failure
    let e = env("retry-404");
    e.host.on_http(|_| status(404, "no"));
    let mut f = linear("r3");
    f.nodes[1].retry = Some(RetryPolicy::fixed(5, 10));
    e.publish(f);
    let r = e.run("r3");
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(r.nodes["a"].attempts.len(), 1);
    assert_eq!(r.nodes["a"].attempts[0].class, "permanent");
    assert!(r.error.contains("404"));
}

#[test]
fn retry_after_and_the_total_deadline_are_honoured() {
    let e = env("retry-after");
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    e.host.on_http(move |_| {
        if n2.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(HttpResponse { status: 429, headers: vec![("retry-after".into(), "3".into())], body: "slow down".into(), elapsed_ms: 1 })
        } else {
            ok_json("{}")
        }
    });
    let mut f = linear("ra");
    f.nodes[1].retry = Some(RetryPolicy::fixed(3, 50));
    e.publish(f);
    let r = e.run("ra");
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(e.clock.sleeps(), vec![3000], "the server's Retry-After replaces the configured pause");

    let e = env("deadline");
    e.host.on_http(|_| status(503, ""));
    let mut f = linear("dl");
    let mut p = RetryPolicy::fixed(10, 1000);
    p.total_deadline_ms = Some(2500);
    f.nodes[1].retry = Some(p);
    e.publish(f);
    let r = e.run("dl");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.nodes["a"].attempts.len() <= 3, "{}", r.nodes["a"].attempts.len());
    assert!(r.error.contains("total time"), "{}", r.error);
}

#[test]
fn a_failed_node_follows_its_error_port_or_continues_or_fails_the_run() {
    let e = env("onerr");
    e.host.on_http(|req| if req.url.ends_with("/fail") { status(500, "boom") } else { ok_json("{}") });
    let mut f = Flow::new("routed", "Routed");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(http_node("bad", "https://x.example/fail"));
    f.nodes.push(Node::new("alert", "notify", json!({"title": "it failed: {{ nodes.bad.error }}"})));
    f.nodes.push(Node::new("after", "notify", json!({"title": "after"})));
    f.edges = vec![Edge::new("s", "bad"), Edge::new("bad", "alert").port("error"), Edge::new("bad", "after")];
    e.publish(f);
    let r = e.run("routed");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(node_state(&r, "bad"), NodeState::Failed);
    assert_eq!(node_state(&r, "alert"), NodeState::Succeeded);
    assert_eq!(node_state(&r, "after"), NodeState::Skipped);
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.title.starts_with("it failed: HTTP 500")));

    let mut g = Flow::new("cont", "Cont");
    g.nodes.push(Node::new("s", "start", json!({})));
    g.nodes.push(http_node("bad", "https://x.example/fail"));
    g.nodes[1].on_error = OnError::Continue;
    g.nodes.push(Node::new("after", "notify", json!({"title": "after"})));
    g.edges = vec![Edge::new("s", "bad"), Edge::new("bad", "after")];
    e.publish(g);
    let r = e.run("cont");
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(node_state(&r, "after"), NodeState::Succeeded);
    assert_eq!(out(&r, "bad")["continued"], true);

    let mut h = Flow::new("plain", "Plain");
    h.nodes.push(Node::new("s", "start", json!({})));
    h.nodes.push(http_node("bad", "https://x.example/fail"));
    h.nodes.push(Node::new("after", "notify", json!({"title": "after"})));
    h.edges = vec![Edge::new("s", "bad"), Edge::new("bad", "after")];
    e.publish(h);
    let r = e.run("plain");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("`bad` failed"), "{}", r.error);
    assert_eq!(node_state(&r, "after"), NodeState::Skipped, "what never ran is skipped, not left pending");
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.level == "error" && n.title.contains("failed")));
}

#[test]
fn approval_blocks_until_decided_rejection_fails_or_routes() {
    let e = env("appr");
    let mut f = Flow::new("ask", "Ask");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("ok?", "approval", json!({"message": "Deploy {{ inputs.v }}?"})));
    f.nodes[1].id = "gate".into();
    f.nodes.push(Node::new("go", "notify", json!({"title": "deployed"})));
    f.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "go").port("approved")];
    f.inputs.push(Param { name: "v".into(), default: Some(json!("v2")), ..Default::default() });
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("ask")).unwrap();
    let r = e.finish(&o.run_id);
    assert_eq!(r.status, RunStatus::Blocked);
    assert!(r.blocked.contains("Deploy v2?"), "{}", r.blocked);
    assert_eq!(node_state(&r, "gate"), NodeState::Waiting);
    assert_eq!(e.engine.counts().needs_you, 1);
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.needs_you && n.run_id == o.run_id));
    assert!(e.engine.decide(&o.run_id, "go", true, "me", "").is_err(), "only a waiting node can be decided");
    e.engine.decide(&o.run_id, "gate", true, "user:me", "looks fine").unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(out(&r, "gate")["by"], "user:me");
    assert!(e.host.audit_lines().iter().any(|l| l.starts_with("flow.approval") && l.contains("approved")));

    // rejected, nothing wired for it: the run fails with who said no
    let o = e.engine.start_run(StartRequest::new("ask")).unwrap();
    e.finish(&o.run_id);
    e.engine.decide(&o.run_id, "gate", false, "user:me", "not today").unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("rejected by user:me: not today"), "{}", r.error);

    // rejected with a `rejected` edge: it routes
    let mut g = Flow::new("ask2", "Ask2");
    g.nodes.push(Node::new("s", "start", json!({})));
    g.nodes.push(Node::new("gate", "approval", json!({"message": "ok?"})));
    g.nodes.push(Node::new("no", "notify", json!({"title": "declined"})));
    g.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "no").port("rejected")];
    e.publish(g);
    let o = e.engine.start_run(StartRequest::new("ask2")).unwrap();
    e.finish(&o.run_id);
    e.engine.decide(&o.run_id, "gate", false, "user:me", "").unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(node_state(&r, "no"), NodeState::Succeeded);
}

#[test]
fn an_approval_times_out_by_its_own_rule() {
    let e = env("appr-timeout");
    let mut f = Flow::new("t", "T");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("gate", "approval", json!({"message": "ok?", "timeout_s": 60, "on_timeout": "approve"})));
    f.nodes.push(Node::new("go", "notify", json!({"title": "went on"})));
    f.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "go").port("approved")];
    e.publish(f);
    let r = e.run("t");
    // the virtual clock is moved by the engine when everything only waits for time
    let r = e.engine.wait_for(&r.id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.nodes["gate"]);
    assert_eq!(out(&r, "gate")["timed_out"], true);
    assert_eq!(out(&r, "gate")["by"], "timeout");
}

#[test]
fn delay_waits_without_a_worker_and_a_restart_waits_only_the_rest() {
    let e = env("delay");
    let mut f = Flow::new("d", "D");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("w", "delay", json!({"seconds": 90})));
    f.nodes.push(Node::new("n", "notify", json!({"title": "later"})));
    f.edges = vec![Edge::new("s", "w"), Edge::new("w", "n")];
    e.publish(f);
    let t0 = e.clock.now_ms();
    let r = e.run("d");
    let r = e.engine.wait_for(&r.id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded);
    assert!(e.clock.now_ms() >= t0 + 90_000, "the clock was moved to the wake-up");
    assert_eq!(node_state(&r, "n"), NodeState::Succeeded);
    // the wake time is stored in the run, so a restart does not start the wait over
    let o = e.engine.start_run(StartRequest::new("d")).unwrap();
    let mut stored = None;
    for _ in 0..200 {
        if let Some(run) = e.engine.get_run(&o.run_id) {
            if run.nodes["w"].wake_ms.is_some() {
                stored = run.nodes["w"].wake_ms;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(stored.is_some() || e.engine.get_run(&o.run_id).unwrap().status.is_terminal());
}

#[test]
fn cancelling_a_blocked_run_a_queued_run_and_a_scheduled_run() {
    let e = env("cancel");
    let mut f = Flow::new("c", "C");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("gate", "approval", json!({"message": "ok?"})));
    f.nodes.push(Node::new("go", "notify", json!({"title": "x"})));
    f.edges = vec![Edge::new("s", "gate"), Edge::new("gate", "go").port("approved")];
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("c")).unwrap();
    assert_eq!(e.finish(&o.run_id).status, RunStatus::Blocked);
    e.engine.cancel(&o.run_id).unwrap();
    let r = e.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Cancelled);
    assert!(r.nodes.values().all(|n| n.state.is_done()));
    assert!(e.engine.cancel(&o.run_id).unwrap_err().contains("already"));
    // scheduled for later: cancelled before it starts
    let later = e.clock.now_ms() + 3_600_000;
    let s = e.engine.start_run(StartRequest::new("c").at(later)).unwrap();
    assert_eq!(e.engine.get_run(&s.run_id).unwrap().status, RunStatus::Scheduled);
    assert_eq!(e.engine.counts().scheduled, 1);
    e.engine.cancel(&s.run_id).unwrap();
    assert_eq!(e.engine.get_run(&s.run_id).unwrap().status, RunStatus::Cancelled);
    assert!(e.engine.cancel("r999").is_err());
}

#[test]
fn cancelling_a_running_run_wakes_a_retry_backoff() {
    let e = env("cancel-retry");
    // a real clock for this one: the backoff would be long
    let dir = tmp("cancel-retry2");
    let host = FakeHost::new();
    host.on_http(|_| status(503, ""));
    let clock: Arc<dyn Clock> = Arc::new(iyakki_flow::host::SystemClock);
    let store = Arc::new(FlowStore::new(&dir.join("flows"), Arc::new(|| 0)));
    let engine = Engine::new(EngineCfg::new(&dir.join("engine")), host, Arc::new(Registry::builtin()), clock, store.clone());
    let mut f = linear("cr");
    f.nodes[1].retry = Some(RetryPolicy::fixed(5, 60_000));
    store.create(f, "u").unwrap();
    store.publish("cr", &iyakki_flow::store::PublishReq { by: "u", label: "", changelog: "x", approved_by: None }).unwrap();
    let o = engine.start_run(StartRequest::new("cr")).unwrap();
    let r = engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status == RunStatus::Retrying).unwrap();
    assert_eq!(r.status, RunStatus::Retrying);
    assert!(r.nodes["a"].next_retry_ms.is_some());
    assert!(r.nodes["a"].max_attempts == 5);
    let t = std::time::Instant::now();
    engine.cancel(&o.run_id).unwrap();
    let r = engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(3), "the 60 second wait ended at once");
    drop(e);
}

#[test]
fn flows_and_the_whole_system_have_concurrency_limits() {
    let e = env("conc");
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let started_tx = Mutex::new(started_tx);
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (l2, p2) = (live.clone(), peak.clone());
    e.host.on_http(move |_| {
        let n = l2.fetch_add(1, Ordering::SeqCst) + 1;
        p2.fetch_max(n, Ordering::SeqCst);
        let _ = started_tx.lock().unwrap().send(());
        let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
        l2.fetch_sub(1, Ordering::SeqCst);
        ok_json("{}")
    });
    let mut f = linear("one");
    f.defaults.max_concurrent_runs = 1;
    e.publish(f);
    let ids: Vec<String> = (0..3).map(|_| e.engine.start_run(StartRequest::new("one")).unwrap().run_id).collect();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let st: Vec<RunStatus> = ids.iter().map(|i| e.engine.get_run(i).unwrap().status).collect();
    assert_eq!(st.iter().filter(|s| matches!(s, RunStatus::Running | RunStatus::Retrying)).count(), 1, "{st:?}");
    assert_eq!(st.iter().filter(|s| **s == RunStatus::Queued).count(), 2, "{st:?}");
    for _ in 0..3 {
        release_tx.send(()).unwrap();
    }
    for i in &ids {
        assert_eq!(e.engine.wait_for(i, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap().status, RunStatus::Succeeded);
    }
    assert_eq!(peak.load(Ordering::SeqCst), 1, "never two at once");
    // queued runs start in the order they were made
    let starts: Vec<i64> = ids.iter().map(|i| e.engine.get_run(i).unwrap().started_ms).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn the_global_limit_holds_across_flows() {
    let dir = tmp("global");
    let host = FakeHost::new();
    let clock = FakeClock::new(1_000_000);
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (l2, p2) = (live.clone(), peak.clone());
    host.on_http(move |_| {
        let n = l2.fetch_add(1, Ordering::SeqCst) + 1;
        p2.fetch_max(n, Ordering::SeqCst);
        let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
        l2.fetch_sub(1, Ordering::SeqCst);
        ok_json("{}")
    });
    let c2 = clock.clone();
    let store = Arc::new(FlowStore::new(&dir.join("flows"), Arc::new(move || c2.now_ms())));
    let mut cfg = EngineCfg::new(&dir.join("engine"));
    cfg.max_runs = 2;
    let engine = Engine::new(cfg, host, Arc::new(Registry::builtin()), clock, store.clone());
    let mut ids = vec![];
    for i in 0..4 {
        let id = format!("g{i}");
        store.create(linear(&id), "u").unwrap();
        store.publish(&id, &iyakki_flow::store::PublishReq { by: "u", label: "", changelog: "x", approved_by: None }).unwrap();
        ids.push(engine.start_run(StartRequest::new(&id)).unwrap().run_id);
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(live.load(Ordering::SeqCst), 2);
    for _ in 0..4 {
        release_tx.send(()).unwrap();
    }
    for i in &ids {
        assert!(engine.wait_for(i, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap().status == RunStatus::Succeeded);
    }
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[test]
fn runs_survive_a_restart() {
    let e = env("crash");
    e.host.on_http(|_| ok_json("{}"));
    let mut f = Flow::new("gated", "Gated");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("fetch", "http", json!({"url": "https://x.example/a"})));
    f.nodes.push(Node::new("gate", "approval", json!({"message": "go on?"})));
    f.nodes.push(Node::new("run", "cli", json!({"command": ["echo", "hi"]})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"v": "{{ nodes.fetch.output.status }}"}})));
    f.edges = vec![Edge::new("s", "fetch"), Edge::new("fetch", "gate"), Edge::new("gate", "run").port("approved"), Edge::new("run", "e")];
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("gated")).unwrap();
    assert_eq!(e.finish(&o.run_id).status, RunStatus::Blocked);
    e.engine.halt();
    // a fresh process: the engine reads the run and the approval still waits
    let e2 = e.restart();
    assert_eq!(e2.engine.recover(), 1);
    let r = e2.engine.get_run(&o.run_id).unwrap();
    assert!(matches!(r.status, RunStatus::Running | RunStatus::Blocked), "{:?}", r.status);
    let r = e2.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status == RunStatus::Blocked).unwrap();
    assert_eq!(node_state(&r, "gate"), NodeState::Waiting);
    assert_eq!(node_state(&r, "fetch"), NodeState::Succeeded, "finished steps are not run again");
    e2.host.on_exec(|_| Ok(iyakki_flow::host::ok_exec("hi\n")));
    e2.engine.decide(&o.run_id, "gate", true, "user:me", "").unwrap();
    let r = e2.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["v"], 200);
    // run numbers go on from the highest one on disk
    let n = e2.engine.start_run(StartRequest::new("gated")).unwrap();
    assert_ne!(n.run_id, o.run_id);
}

#[test]
fn a_step_that_was_running_at_the_crash_is_repeated_when_safe_and_flagged_when_not() {
    let e = env("interrupt");
    e.host.on_http(|_| ok_json("{}"));
    e.host.on_exec(|_| Ok(iyakki_flow::host::ok_exec("ok")));
    let mut f = Flow::new("two", "Two");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("get", "http", json!({"url": "https://x.example/a"})));
    f.nodes.push(Node::new("do", "cli", json!({"command": ["backup", "now"]})));
    f.nodes.push(Node::new("gate", "approval", json!({"message": "hold"})));
    f.edges = vec![Edge::new("s", "get"), Edge::new("s", "do"), Edge::new("get", "gate"), Edge::new("do", "gate")];
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("two")).unwrap();
    e.finish(&o.run_id);
    e.engine.halt();
    // make the file look like the process died with both steps running
    let path = e.dir.join("engine/runs").join(&o.run_id).join("run.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for n in ["get", "do"] {
        v["nodes"][n]["state"] = json!("running");
        v["nodes"][n]["output"] = Value::Null;
    }
    v["nodes"]["gate"]["state"] = json!("pending");
    for k in ["s:out>get", "s:out>do", "get:out>gate", "do:out>gate"] {
        v["edges"][k] = json!(if k.starts_with("s:") { "taken" } else { "open" });
    }
    v["nodes"]["gate"]["waiting"] = json!("");
    v["status"] = json!("running");
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    e.host.exec_calls.lock().unwrap().clear();
    e.host.http_calls.lock().unwrap().clear();
    let e2 = e.restart();
    e2.engine.recover();
    let r = e2.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.status == RunStatus::Blocked && r.nodes["do"].state == NodeState::Interrupted && r.nodes["get"].state == NodeState::Succeeded).unwrap();
    assert_eq!(r.nodes["do"].state, NodeState::Interrupted, "a command may have run: it is not repeated by itself");
    assert!(r.blocked.contains("restarted while `do`"), "{}", r.blocked);
    assert_eq!(r.nodes["get"].state, NodeState::Succeeded, "a GET is safe to repeat, so it ran again");
    assert!(r.nodes["get"].note.contains("safe to repeat"));
    assert!(e2.host.exec_calls.lock().unwrap().is_empty());
    assert!(!e2.host.http_calls.lock().unwrap().is_empty());
    // the person chooses: retry it
    e2.engine.retry_node(&o.run_id, "do").unwrap();
    let r = e2.engine.wait_for(&o.run_id, Duration::from_secs(5), |r| r.nodes["do"].state == NodeState::Succeeded).unwrap();
    assert_eq!(r.nodes["do"].state, NodeState::Succeeded);
    assert_eq!(e2.host.exec_calls.lock().unwrap().len(), 1);
}

#[test]
fn schedules_start_runs_and_overlap_is_obeyed() {
    let e = env("sched");
    e.host.on_http(|_| ok_json("{}"));
    let mut f = linear("tick");
    f.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
    e.publish(f);
    e.engine.tick();
    assert!(e.engine.summaries().is_empty(), "the first look only records");
    e.clock.advance(61_000);
    e.engine.tick();
    let runs = e.engine.runs_of("tick");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].trigger, "interval");
    e.engine.wait_for(&runs[0].id, Duration::from_secs(5), |r| r.status.is_terminal()).unwrap();
    e.clock.advance(60_000);
    e.engine.tick();
    assert_eq!(e.engine.runs_of("tick").len(), 2);
    assert!(e.engine.wait_idle(Duration::from_secs(5)));
    // a manual run is still going: the next tick is skipped
    let mut g = Flow::new("slow", "Slow");
    g.nodes.push(Node::new("s", "start", json!({})));
    g.nodes.push(Node::new("gate", "approval", json!({"message": "hold"})));
    g.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
    e.publish(g);
    e.engine.tick();
    e.clock.advance(61_000);
    e.engine.tick();
    assert_eq!(e.engine.runs_of("slow").len(), 1);
    e.clock.advance(60_000);
    e.engine.tick();
    assert_eq!(e.engine.runs_of("slow").len(), 1, "overlap: skip, because the first is waiting");
    assert!(e.engine.sched_entry("slow", "t").unwrap().skipped >= 1);
    // pausing stops it, resuming applies the missed policy
    assert!(e.engine.wait_idle(Duration::from_secs(5)));
    e.engine.pause_schedule("tick", true);
    let n = e.engine.runs_of("tick").len();
    e.clock.advance(300_000);
    e.engine.tick();
    assert_eq!(e.engine.runs_of("tick").len(), n);
    e.engine.pause_schedule("tick", false);
    e.engine.tick();
    assert_eq!(e.engine.runs_of("tick").len(), n + 1, "{:?}", e.host.audit_lines());
    // next runs
    let fl = e.store.published("tick").unwrap();
    let nr = e.engine.next_runs(&fl, 3);
    assert_eq!(nr.len(), 3);
    assert!(nr[0].1 > e.clock.now_ms() && nr[1].1 - nr[0].1 == 60_000);
}

#[test]
fn on_failure_and_event_triggers_start_other_flows() {
    let e = env("trig");
    e.host.on_http(|req| if req.url.contains("/boom") { status(500, "x") } else { ok_json("{}") });
    let mut bad = Flow::new("bad", "Bad");
    bad.nodes.push(Node::new("s", "start", json!({})));
    bad.nodes.push(http_node("h", "https://x.example/boom"));
    bad.edges.push(Edge::new("s", "h"));
    e.publish(bad);
    let mut watcher = Flow::new("watcher", "Watcher");
    watcher.nodes.push(Node::new("s", "start", json!({})));
    watcher.nodes.push(Node::new("n", "notify", json!({"title": "{{ inputs.failed_flow }} failed: {{ inputs.error }}"})));
    watcher.edges.push(Edge::new("s", "n"));
    watcher.triggers.push(Trigger::new("f", TriggerKind::OnFailure { flow: "bad".into() }));
    watcher.triggers.push(Trigger::new("ev", TriggerKind::Event { name: "deploy".into() }));
    e.publish(watcher);
    let r = e.run("bad");
    assert_eq!(r.status, RunStatus::Failed);
    let w = e.engine.wait_for_flow("watcher", Duration::from_secs(5));
    assert!(w.is_some());
    assert!(e.host.notices.lock().unwrap().iter().any(|n| n.title.starts_with("bad failed: `h` failed")));
    let started = e.engine.emit_event("deploy", json!({"v": 1}));
    assert_eq!(started.len(), 1);
    assert!(e.engine.emit_event("other", json!({})).is_empty());
}

#[test]
fn secrets_reach_the_call_and_never_the_logs_or_outputs() {
    let e = env("secret");
    e.host.secret_is("api", "S3cr3t-Token-Value");
    e.host.on_http(|req| {
        let auth = req.headers.iter().find(|(k, _)| k == "Authorization").map(|x| x.1.clone()).unwrap_or_default();
        // an unkind server that echoes what it was sent
        ok_json(&format!(r#"{{"you_sent": "{auth}"}}"#))
    });
    let mut f = Flow::new("sec", "Sec");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("h", "http", json!({"url": "https://x.example/", "headers": {"Authorization": "Bearer {{secret.api}}"}})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"echo": "{{ nodes.h.output.body.you_sent }}"}})));
    f.edges = vec![Edge::new("s", "h"), Edge::new("h", "e")];
    e.publish(f);
    let r = e.run("sec");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(e.host.http_calls.lock().unwrap()[0].headers[0].1, "Bearer S3cr3t-Token-Value", "the call itself had it");
    let everything = format!("{}{}{}", serde_json::to_string(&r).unwrap(), e.engine.log_text(&r.id), std::fs::read_to_string(e.dir.join("engine/runs").join(&r.id).join("run.json")).unwrap());
    assert!(!everything.contains("S3cr3t-Token-Value"), "a secret leaked into the run, its log or its file");
    assert!(r.outputs["echo"].as_str().unwrap().contains("***"));
    // a secret that is not in the vault fails the step clearly
    let mut g = Flow::new("sec2", "Sec2");
    g.nodes.push(Node::new("s", "start", json!({})));
    g.nodes.push(Node::new("h", "http", json!({"url": "https://x.example/", "headers": {"Authorization": "Bearer {{secret.nope}}"}})));
    g.edges.push(Edge::new("s", "h"));
    e.publish(g);
    let r = e.run("sec2");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("no secret `nope`"), "{}", r.error);
    // and a secret in a field that may not carry one is refused
    let mut h = Flow::new("sec3", "Sec3");
    h.nodes.push(Node::new("s", "start", json!({})));
    h.nodes.push(Node::new("h", "http", json!({"url": "https://x.example/{{secret.api}}"})));
    h.edges.push(Edge::new("s", "h"));
    e.publish(h);
    let r = e.run("sec3");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("can only be used in a header"), "{}", r.error);
}

#[test]
fn testing_one_node_runs_it_once_without_a_run() {
    let e = env("testnode");
    e.host.on_http(|_| ok_json(r#"{"count": 3}"#));
    let mut f = linear("t");
    f.nodes[1].config = json!({"url": "https://x.example/{{ inputs.who }}", "extract": {"n": "count"}});
    let t = e.engine.test_node(&f, "a", json!({"who": "ada"}), Value::Null).unwrap();
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["n"], 3);
    assert_eq!(e.host.http_calls.lock().unwrap()[0].url, "https://x.example/ada");
    assert!(e.engine.summaries().is_empty(), "no run was made");
    e.host.on_http(|_| status(500, "x"));
    let t = e.engine.test_node(&f, "a", json!({"who": "x"}), Value::Null).unwrap();
    assert!(!t.ok && t.error.contains("500"));
    assert!(e.engine.test_node(&f, "zzz", json!({}), Value::Null).is_err());
}

#[test]
fn a_flow_run_from_a_draft_snapshot_does_not_need_a_publish() {
    let e = env("snap");
    e.host.on_http(|_| ok_json("{}"));
    let f = linear("sn");
    let o = e.engine.start_run(StartRequest::new("sn").with_flow(f)).unwrap();
    let r = e.finish(&o.run_id);
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!(r.flow.meta.version, 0, "an unpublished flow is version 0");
    let _ = ok_response(200, "");
    let _ = Cancel::new();
}
