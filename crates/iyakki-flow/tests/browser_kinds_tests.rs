//! The browser nodes in the engine, against a scripted host: every kind validates what it should, builds the request the host gets,
//! turns a failed check into a failed node (or a recorded one when soft), resolves `vault:NAME` at the moment it runs and keeps the
//! value out of the run, ends the run's browser session on every way a run can end, and the report is built from what ran.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use iyakki_flow::host::{BrowserOp, BrowserOpResult};
use iyakki_flow::kinds::browser_qa::{flow_hosts, KINDS};
use iyakki_flow::*;
use serde_json::{json, Value};

fn line(id: &str, nodes: Vec<Node>) -> Flow {
    let mut f = Flow::new(id, id);
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut prev = "s".to_string();
    for n in nodes {
        f.edges.push(Edge::new(&prev, &n.id));
        prev = n.id.clone();
        f.nodes.push(n);
    }
    f
}

fn issues(kind: &str, cfg: Value) -> Vec<String> {
    let f = {
        let mut f = Flow::new("t", "T");
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("n", kind, cfg));
        f.edges.push(Edge::new("s", "n"));
        f
    };
    iyakki_flow::validate::validate_flow(&f, &Registry::builtin()).into_iter().filter(|i| i.node.as_deref() == Some("n") && i.level == iyakki_flow::kinds::Level::Error).map(|i| i.msg).collect()
}

#[test]
fn there_are_twenty_browser_kinds_in_their_own_palette_group_each_with_a_schema() {
    let r = Registry::builtin();
    assert_eq!(KINDS.len(), 20);
    for k in KINDS {
        let kind = r.get(k).unwrap_or_else(|| panic!("{k}"));
        assert_eq!(kind.category(), "Browser", "{k}");
        assert!(!kind.schema().is_empty() || k == "br_download", "{k}");
        assert!(!kind.blurb().is_empty() && !kind.label().is_empty());
        assert_eq!(kind.ports(&Node::new("n", k, json!({}))), iyakki_flow::kinds::Ports::standard());
        assert!(kind.can_fail(), "{k} has an error port");
        // the list in palette order has them after the control kinds
        assert!(r.ids().iter().position(|i| i == k).unwrap() > r.ids().iter().position(|i| i == "foreach").unwrap());
    }
}

#[test]
fn every_kind_refuses_what_is_missing_or_unsafe_with_a_plain_message() {
    assert!(issues("br_open", json!({})).iter().any(|m| m.contains("address")));
    assert!(issues("br_open", json!({"url": "ftp://x/"})).iter().any(|m| m.contains("only http and https")));
    assert!(issues("br_open", json!({"url": "http://u:p@h/"})).iter().any(|m| m.contains("no user name or password")));
    assert!(issues("br_open", json!({"url": "https://example.org/"})).is_empty());
    assert!(issues("br_open", json!({"url": "{{ inputs.url }}"})).is_empty(), "a template is checked when it runs");
    assert!(issues("br_click", json!({})).iter().any(|m| m.contains("selector")));
    assert!(issues("br_click", json!({"selector": "a\nb"})).iter().any(|m| m.contains("not usable")));
    assert!(issues("br_click", json!({"selector": "#go"})).is_empty());
    assert!(issues("br_type", json!({"selector": "#a"})).iter().any(|m| m.contains("text")), "the text is required");
    assert!(issues("br_fill", json!({})).iter().any(|m| m.contains("map of selector")));
    assert!(issues("br_fill", json!({"fields": {}})).iter().any(|m| m.contains("map of selector")));
    assert!(issues("br_fill", json!({"fields": {"#a": "x"}})).is_empty());
    assert!(issues("br_key", json!({})).iter().any(|m| m.contains("key")));
    assert!(issues("br_select", json!({"selector": "s"})).iter().any(|m| m.contains("option")));
    assert!(issues("br_assert", json!({})).iter().any(|m| m.contains("what to check")));
    assert!(issues("br_assert", json!({"check": "exists"})).iter().any(|m| m.contains("selector")));
    assert!(issues("br_assert", json!({"check": "attr_equals", "selector": "a", "expected": "x"})).iter().any(|m| m.contains("attribute")));
    assert!(issues("br_assert", json!({"check": "text_equals", "selector": "a"})).iter().any(|m| m.contains("what is expected")));
    assert!(issues("br_assert", json!({"check": "text_matches", "selector": "a", "expected": "("})).iter().any(|m| m.contains("pattern")));
    assert!(issues("br_assert", json!({"check": "url_contains", "expected": "/x"})).is_empty(), "page checks need no selector");
    assert!(issues("br_wait", json!({"for": "time"})).iter().any(|m| m.contains("milliseconds")));
    assert!(issues("br_wait", json!({"for": "text"})).iter().any(|m| m.contains("what to wait for")));
    assert!(issues("br_extract", json!({})).iter().any(|m| m.contains("fields")));
    assert!(issues("br_extract", json!({"rows": {"selector": "tr", "columns": {"a": "td"}}})).is_empty());
    assert!(issues("br_js", json!({})).iter().any(|m| m.contains("expression")));
    assert!(issues("br_frame", json!({"action": "frame"})).iter().any(|m| m.contains("selector")));
    assert!(issues("br_frame", json!({"action": "main"})).is_empty());
    assert!(issues("br_viewport", json!({"width": 100, "height": 800})).iter().any(|m| m.contains("width")));
    assert!(issues("br_viewport", json!({"width": 1280, "height": 5000})).iter().any(|m| m.contains("height")));
    assert!(issues("br_viewport", json!({"preset": "phone 375x667"})).is_empty());
    assert!(issues("br_storage", json!({"kind": "cookie", "name": "session_id", "value": "abc123"})).iter().any(|m| m.contains("vault:NAME")), "a credential is never written into the flow");
    assert!(issues("br_storage", json!({"kind": "cookie", "name": "session_id", "value": "vault:sess"})).is_empty());
    assert!(issues("br_storage", json!({"kind": "local_storage", "name": "theme", "value": "dark"})).is_empty());
    assert!(issues("br_compare", json!({})).iter().any(|m| m.contains("baseline")));
    assert!(issues("br_screenshot", json!({})).is_empty() && issues("br_dom", json!({})).is_empty() && issues("br_download", json!({})).is_empty());
}

#[test]
fn the_hosts_a_flow_names_are_found_with_inputs_filled_in_and_nothing_made_up() {
    let mut f = line("h", vec![Node::new("a", "br_open", json!({"url": "https://shop.example/login?x=1"})), Node::new("b", "br_open", json!({"url": "{{ inputs.url }}"})), Node::new("c", "br_open", json!({"url": "{{ nodes.a.output.url }}"})), Node::new("d", "http", json!({"url": "https://other.example/"}))]);
    f.inputs.push(Param { name: "url".into(), ty: "string".into(), required: false, default: None, description: String::new() });
    let ctx = json!({"inputs": {"url": "http://127.0.0.1:8080/x"}, "nodes": {}, "vars": {}});
    assert_eq!(flow_hosts(&f, &ctx), vec!["shop.example", "127.0.0.1"], "the step that needs an earlier output adds nothing; other kinds do not count");
    assert_eq!(iyakki_flow::kinds::browser_qa::url_host("https://User:pw@Host.Example:8443/a"), "host.example");
}

#[test]
fn a_node_builds_its_request_the_host_gets_with_flow_settings_and_hosts() {
    let e = env("bq-req");
    let seen: Arc<Mutex<Vec<BrowserOp>>> = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    e.host.on_browser_op(move |op| {
        s2.lock().unwrap().push(op.clone());
        Ok(BrowserOpResult { value: json!({"ok": true, "url": "https://shop.example/"}), artifacts: vec![], log: vec!["did it".into()] })
    });
    let mut f = line("req", vec![Node::new("o", "br_open", json!({"url": "https://shop.example/login"})), Node::new("k", "br_click", json!({"selector": "#go", "index": "-1"}))]);
    f.defaults.browser = BrowserDefaults { trusted: true, profile: "person".into(), max_actions: 7, max_minutes: 3 };
    e.publish(f);
    let r = e.run("req");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    let ops = seen.lock().unwrap();
    assert_eq!(ops.iter().map(|o| o.op.as_str()).collect::<Vec<_>>(), vec!["open", "click"]);
    let o = &ops[0];
    assert_eq!((o.run_id.as_str(), o.flow_id.as_str(), o.node_id.as_str()), (r.id.as_str(), "req", "o"));
    assert!(o.trusted && o.profile == "person" && o.max_actions == 7 && o.max_minutes == 3);
    assert_eq!(o.hosts, vec!["shop.example"]);
    assert_eq!(o.timeout_ms, 30_000);
    assert_eq!(ops[1].args["selector"], "#go");
    assert_eq!(format!("{:?}", ops[1]), format!("BrowserOp(click in {}/k)", r.id), "the debug text never prints the arguments");
    assert!(r.nodes["o"].log.iter().any(|l| l == "did it"));
    // an untrusted flow with the default profile says so
    drop(ops);
    e.publish(line("req2", vec![Node::new("o", "br_open", json!({"url": "https://shop.example/"}))]));
    e.run("req2");
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert!(!last.trusted && last.profile == "isolated");
}

#[test]
fn a_failed_check_fails_the_node_with_its_message_unless_it_is_soft_and_a_retry_can_try_again() {
    let e = env("bq-assert");
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let n2 = n.clone();
    e.host.on_browser_op(move |op| {
        if op.op != "assert" {
            return Ok(BrowserOpResult { value: json!({"ok": true}), ..Default::default() });
        }
        let tries = n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let passed = op.args["check"] == "visible" && tries >= 2;
        Ok(BrowserOpResult { value: json!({"check": op.args["check"], "passed": passed, "message": if passed { "it is visible".to_string() } else { format!("expected \"#x\" to be visible but it is hidden (look {tries})") }}), ..Default::default() })
    });
    let mut retry = Node::new("v", "br_assert", json!({"check": "visible", "selector": "#x"}));
    retry.retry = Some(RetryPolicy::fixed(5, 10));
    e.publish(line("retry", vec![retry]));
    let r = e.run("retry");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.nodes["v"].attempts.len(), 3);
    assert!(r.nodes["v"].attempts[0].error.contains("but it is hidden (look 0)"));
    assert_eq!(out(&r, "v")["passed"], true);
    // a hard failure fails the run and the message is the node's error
    n.store(0, std::sync::atomic::Ordering::SeqCst);
    e.publish(line("hard", vec![Node::new("h", "br_assert", json!({"check": "exists", "selector": "#x"}))]));
    let r = e.run("hard");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.nodes["h"].error.contains("expected \"#x\" to be visible but it is hidden"), "{}", r.nodes["h"].error);
    // a soft one is recorded and the run goes on
    n.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut f = line("soft", vec![Node::new("h", "br_assert", json!({"check": "exists", "selector": "#x", "soft": true})), Node::new("after", "br_click", json!({"selector": "#go"}))]);
    f.nodes.push(Node::new("end", "end", json!({})));
    f.edges.push(Edge::new("after", "end"));
    e.publish(f);
    let r = e.run("soft");
    assert_eq!(r.status, RunStatus::Succeeded);
    assert_eq!((out(&r, "h")["passed"].clone(), out(&r, "h")["soft"].clone()), (json!(false), json!(true)));
    assert_eq!(node_state(&r, "after"), NodeState::Succeeded);
    assert!(r.nodes["h"].log.iter().any(|l| l.contains("soft check failed")));
    let rep = iyakki_flow::report::build(&r);
    assert!(rep.passed, "only a soft check failed");
    assert_eq!((rep.summary.checks, rep.summary.checks_failed, rep.summary.soft_failed), (1, 1, 1));
}

#[test]
fn vault_references_are_read_when_the_node_runs_and_the_value_reaches_no_stored_text() {
    let e = env("bq-vault");
    e.host.secret_is("qa-pass", "Zx9-Very-Secret");
    let got: Arc<Mutex<Vec<BrowserOp>>> = Arc::new(Mutex::new(vec![]));
    let g2 = got.clone();
    e.host.on_browser_op(move |op| {
        g2.lock().unwrap().push(op.clone());
        // a careless host echoing the value back: the engine still removes it from the stored output
        Ok(BrowserOpResult { value: json!({"ok": true, "echo": op.args.to_string(), "log_line": "typed Zx9-Very-Secret"}), log: vec!["typed Zx9-Very-Secret".into()], artifacts: vec![] })
    });
    e.publish(line("vault", vec![
        Node::new("t", "br_type", json!({"selector": "#pw", "text": "vault:qa-pass"})),
        Node::new("f", "br_fill", json!({"fields": {"#user": "qa", "#pw": "vault:qa-pass"}})),
        Node::new("c", "br_storage", json!({"kind": "cookie", "name": "session", "value": "vault:qa-pass"})),
    ]));
    let r = e.run("vault");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    let ops = got.lock().unwrap();
    assert_eq!(ops[0].args["text"], "Zx9-Very-Secret", "the host gets the value to type");
    assert_eq!(ops[1].args["fields"]["#pw"], "Zx9-Very-Secret");
    assert_eq!(ops[1].args["fields"]["#user"], "qa", "other values are untouched");
    assert_eq!(ops[2].args["value"], "Zx9-Very-Secret");
    assert_eq!(ops[0].secrets, vec!["Zx9-Very-Secret"], "and is told which values are secrets");
    drop(ops);
    let stored = serde_json::to_string(&r).unwrap();
    assert!(!stored.contains("Zx9-Very-Secret"), "the run (inputs, outputs, logs, attempts) never holds it");
    assert!(stored.contains("***"));
    let flow_file = serde_json::to_string(&e.store.get_draft("vault").unwrap()).unwrap();
    assert!(flow_file.contains("vault:qa-pass") && !flow_file.contains("Zx9-Very-Secret"));
    for f in std::fs::read_dir(e.dir.join("engine")).into_iter().flatten().flatten() {
        if let Ok(t) = std::fs::read_to_string(f.path()) {
            assert!(!t.contains("Zx9-Very-Secret"), "{}", f.path().display());
        }
    }
    // a reference to something that is not in the vault says so and does nothing
    e.publish(line("novault", vec![Node::new("t", "br_type", json!({"selector": "#pw", "text": "vault:missing"}))]));
    let before = got.lock().unwrap().len();
    let r = e.run("novault");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.nodes["t"].error.contains("no secret `missing`"), "{}", r.nodes["t"].error);
    assert_eq!(got.lock().unwrap().len(), before, "the host was never asked");
}

#[test]
fn the_runs_browser_session_is_ended_whatever_way_the_run_ends_and_before_it_counts_as_over() {
    let e = env("bq-end");
    let ended: Arc<Mutex<Vec<String>>> = e.host.browser_ended_arc();
    // success
    e.publish(line("ok", vec![Node::new("o", "br_open", json!({"url": "http://127.0.0.1/"}))]));
    let r = e.run("ok");
    assert!(ended.lock().unwrap().contains(&r.id));
    // failure
    e.host.on_browser_op(|_| Err(NodeError::new("the page broke")));
    e.publish(line("bad", vec![Node::new("o", "br_open", json!({"url": "http://127.0.0.1/"}))]));
    let r = e.run("bad");
    assert_eq!(r.status, RunStatus::Failed);
    assert!(ended.lock().unwrap().contains(&r.id));
    // a stop in the middle of a long wait
    e.host.on_browser_op(|_| {
        std::thread::sleep(Duration::from_millis(400));
        Ok(BrowserOpResult { value: json!({"ok": true}), ..Default::default() })
    });
    e.publish(line("stop", vec![Node::new("o", "br_open", json!({"url": "http://127.0.0.1/"})), Node::new("w", "br_wait", json!({"for": "time", "ms": 100}))]));
    let o = e.engine.start_run(StartRequest::new("stop")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    e.engine.cancel(&o.run_id).unwrap();
    let r = e.finish(&o.run_id);
    assert_eq!(r.status, RunStatus::Cancelled);
    assert!(ended.lock().unwrap().contains(&r.id), "ended after a stop");
}

#[test]
fn the_report_tells_what_ran_and_exports_one_page_and_data() {
    let e = env("bq-report");
    e.host.on_browser_op(|op| {
        Ok(match op.op.as_str() {
            "screenshot" => BrowserOpResult { value: json!({"path": "/nonexistent/1.png", "name": "p"}), artifacts: vec!["/nonexistent/1.png".into()], log: vec![] },
            "dom" => BrowserOpResult { value: json!({"url": "http://127.0.0.1/", "title": "T <b>", "text": "hello & <world>", "truncated": false, "bytes": 15, "console": [{"level": "error", "text": "boom"}, {"level": "log", "text": "fine"}]}), ..Default::default() },
            _ => BrowserOpResult { value: json!({"check": "exists", "passed": false, "message": "expected \"#a\" to exist but no element matches it"}), ..Default::default() },
        })
    });
    let mut soft = Node::new("c", "br_assert", json!({"check": "exists", "selector": "#a", "soft": true}));
    soft.name = "Soft check".into();
    e.publish(line("rep", vec![Node::new("sh", "br_screenshot", json!({})), Node::new("d", "br_dom", json!({"mode": "text", "selector": "h1"})), soft]));
    let r = e.run("rep");
    let rep = iyakki_flow::report::build(&r);
    assert!(rep.passed && rep.status == "succeeded");
    assert_eq!((rep.summary.nodes, rep.summary.succeeded, rep.summary.screenshots, rep.summary.console_errors, rep.summary.checks), (4, 4, 1, 1, 1));
    let names: Vec<&str> = rep.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(names, vec!["s", "sh", "d", "c"], "in the order they ran");
    let dom = rep.nodes.iter().find(|n| n.id == "d").unwrap();
    assert_eq!(dom.console_errors, vec!["error: boom"]);
    assert_eq!(dom.dom.as_ref().unwrap()["snippet"], "hello & <world>");
    let html = iyakki_flow::report::to_html(&rep, &|p| (p == "/nonexistent/1.png").then(|| vec![1, 2, 3]));
    assert!(html.starts_with("<!doctype html>") && html.contains("PASSED") && html.contains("data:image/png;base64,AQID"));
    assert!(html.contains("hello &amp; &lt;world&gt;") && !html.contains("hello & <world>"), "page text is escaped, never markup");
    assert!(html.contains("Soft check") && html.contains("(soft)") && html.contains("boom"));
    let missing = iyakki_flow::report::to_html(&rep, &|_| None);
    assert!(missing.contains("no longer there"));
    let v: Value = serde_json::from_str(&iyakki_flow::report::to_json(&rep)).unwrap();
    assert_eq!(v["gallery"][0]["path"], "/nonexistent/1.png");
    assert!(!html.contains("http://") || html.contains("http://127.0.0.1/"), "nothing is loaded from outside: the page is self-contained");
    assert!(!html.contains("<script") && !html.contains("src=\"http"), "no scripts, no remote pictures");
}
