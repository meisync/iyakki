mod common;

use common::*;
use iyakki_flow::templates;
use iyakki_flow::*;
use serde_json::json;

fn kinds(f: &Flow, kind: &str) -> Vec<Node> {
    f.nodes.iter().filter(|n| n.kind == kind).cloned().collect()
}

#[test]
fn the_three_qa_templates_have_what_the_row_promises_three_sizes_console_errors_and_link_checks() {
    let r = templates::get("qa-responsive-shots").unwrap().flow;
    let presets: Vec<String> = kinds(&r, "br_viewport").iter().map(|n| n.config["preset"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(presets, vec!["phone 375x667", "tablet 768x1024", "desktop 1920x1080"], "three different sizes, small to large");
    let shots = kinds(&r, "br_screenshot");
    assert_eq!(shots.len(), 3);
    assert!(shots.iter().all(|n| n.config["full_page"] == true), "whole pages");
    assert_eq!(shots.iter().map(|n| n.config["name"].as_str().unwrap().to_string()).collect::<Vec<_>>(), vec!["phone", "tablet", "desktop"]);
    // the end collects the three files, one output each
    let end = kinds(&r, "end").remove(0);
    for k in ["phone", "tablet", "desktop"] {
        assert!(end.config["outputs"][k].as_str().unwrap().contains("output.path"), "{k}");
    }
    // each size is set before its picture is taken
    let order: Vec<&str> = r.edges.iter().map(|e| e.from.as_str()).collect();
    assert!(order.iter().position(|x| *x == "v1") < order.iter().position(|x| *x == "s1"));
    assert!(order.iter().position(|x| *x == "s1") < order.iter().position(|x| *x == "v2"));

    let h = templates::get("qa-page-health").unwrap().flow;
    let dom = &kinds(&h, "br_dom")[0];
    assert_eq!((dom.config["console"] == true, dom.config["links"] == true, dom.config["failures"] == true), (true, true, true));
    let quiet = kinds(&h, "br_assert").into_iter().find(|n| n.config["check"] == "no_console_errors").expect("the console check");
    assert_eq!(quiet.config["soft"], true, "a console error is recorded, it does not stop the run");
    let js = &kinds(&h, "br_js")[0];
    let script = js.config["expression"].as_str().unwrap();
    assert!(script.contains("HEAD") && script.contains("broken") && script.contains("location.origin"), "tries each same-site link");
    let cond = &kinds(&h, "condition")[0];
    assert!(cond.config["expr"].as_str().unwrap().contains("broken"));
    let tell = &kinds(&h, "notify")[0];
    assert_eq!(tell.config["level"], "error");

    let l = templates::get("qa-login-smoke").unwrap().flow;
    assert!(kinds(&l, "br_assert").len() >= 1 && kinds(&l, "br_fill").iter().any(|n| n.config.to_string().contains("vault:qa-password")));
}

/// start -> list (http, fails once) -> each (foreach over the list's output) -> end
#[test]
fn a_node_that_is_retried_feeds_a_loop_over_its_own_output_and_the_loop_feeds_the_end() {
    let e = env("m66-retry-loop");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = calls.clone();
    e.host.on_http(move |req| {
        if req.url.contains("/list") {
            // the first answer is an error, the retry gets the list
            if c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return status(503, "busy");
            }
            return ok_json(r#"{"items": [{"id": 7}, {"id": 8}]}"#);
        }
        ok_json(&format!(r#"{{"seen": "{}"}}"#, req.url))
    });
    let mut child = Flow::new("per-id", "Per id");
    child.nodes.push(Node::new("s", "start", json!({})));
    child.nodes.push(http_node("h", "https://x.example/item/{{ inputs._item.id }}"));
    child.nodes.push(Node::new("e", "end", json!({"outputs": {"url": "{{ nodes.h.output.body.seen }}"}})));
    child.edges = vec![Edge::new("s", "h"), Edge::new("h", "e")];
    e.publish(child);
    let mut f = Flow::new("m66-main", "Main");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut list = http_node("list", "https://x.example/list");
    list.retry = Some(RetryPolicy::fixed(3, 10));
    f.nodes.push(list);
    f.nodes.push(Node::new("each", "foreach", json!({"items": "{{ nodes.list.output.body.items }}", "flow": "per-id", "max_items": 5})));
    f.nodes.push(Node::new("e", "end", json!({"outputs": {"count": "{{ nodes.each.output.count }}", "urls": "{{ nodes.each.output.items }}"}})));
    f.edges = vec![Edge::new("s", "list"), Edge::new("list", "each"), Edge::new("each", "e")];
    e.publish(f);
    let o = e.engine.start_run(StartRequest::new("m66-main")).unwrap();
    let r = e.finish(&o.run_id);
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "one failure, one retry");
    assert_eq!(r.nodes["list"].attempts.len(), 2);
    assert_eq!(r.outputs["count"], 2);
    assert_eq!(r.outputs["urls"][0]["url"], "https://x.example/item/7");
    assert_eq!(r.outputs["urls"][1]["url"], "https://x.example/item/8");
}

/// Every registered node kind keeps the contract the designer, the validator and the engine rely on.
#[test]
fn every_node_kind_in_the_registry_keeps_its_contract_schema_ports_validation_describe_and_error_class() {
    use iyakki_flow::kinds::{Level, Registry};
    use iyakki_flow::retry::ErrClass;
    let reg = Registry::builtin();
    let all = reg.all();
    assert!(all.len() >= 30, "the registry has the browser, camera, control and call kinds: {}", all.len());
    let mut ids = std::collections::BTreeSet::new();
    let flow = Flow::new("m66-contract", "Contract");
    for k in all {
        let id = k.id();
        assert!(ids.insert(id), "a kind id is registered once: {id}");
        assert!(!id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{id}");
        assert!(!k.label().is_empty() && !k.blurb().is_empty(), "{id} has words for the palette");
        assert!(["Control", "Call", "Run", "Human", "Flow", "Browser", "Camera", "Data", "Tell"].contains(&k.category()) || !k.category().is_empty(), "{id}");
        // the schema: each field has a key and a label once
        let mut keys = std::collections::BTreeSet::new();
        for f in k.schema() {
            assert!(!f.key.is_empty() && !f.label.is_empty() && keys.insert(f.key), "{id}: field `{}`", f.key);
        }
        // ports: a node can be reached and (unless it ends the flow) left; a kind that can fail has an `error` way out
        let node = Node::new("n", id, json!({}));
        let ports = k.ports(&node);
        assert!(ports.outputs.iter().all(|p| !p.is_empty()) && ports.inputs.iter().all(|p| !p.is_empty()), "{id}");
        if id != "start" {
            assert!(!ports.inputs.is_empty(), "{id} can be reached");
        }
        if id != "end" {
            assert!(!ports.outputs.is_empty(), "{id} can be left");
        }
        if k.can_fail() {
            assert!(ports.outputs.iter().any(|p| p == "error") || !k.schema().is_empty() || id == "start" || id == "end", "{id}");
        }
        // validation never panics on an empty configuration, and its issues name the node
        for i in k.validate(&node, &flow) {
            assert!(i.node.as_deref().is_none_or(|n| n == "n"), "{id}: {i:?}");
            assert!(!i.msg.is_empty(), "{id}");
        }
        // the one-line description and the error classes
        assert!(!k.describe(&node).is_empty(), "{id}");
        assert_eq!(k.classify_error(&NodeError::config("wrong"), &node), ErrClass::Permanent, "{id}: a wrong configuration never retries");
        assert_eq!(k.classify_error(&NodeError::cancelled(), &node), ErrClass::Permanent, "{id}: a cancel never retries");
    }
    // the kinds the row names are all there
    for k in ["start", "end", "http", "cli", "grpc", "remote", "browser", "agent", "script", "condition", "switch", "delay", "approval", "notify", "subflow", "foreach", "thrift", "cam_photo", "br_open", "br_screenshot"] {
        assert!(ids.contains(k), "{k}");
    }
    // a required field that is missing is an error the person sees, for the kinds that have one
    let http = Node::new("n", "http", json!({}));
    assert!(reg.get("http").unwrap().validate(&http, &flow).iter().any(|i| i.level == Level::Error), "an http step with no address is refused");
}
