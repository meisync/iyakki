mod common;

use std::time::Duration;

use common::*;
use iyakki_flow::host::{failed_exec, ok_exec, AgentResult, BrowserResult, ExecResult, HttpResponse};
use iyakki_flow::*;
use serde_json::{json, Value};

fn one(kind: &str, cfg: Value) -> Flow {
    let mut f = Flow::new("t", "T");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("n", kind, cfg));
    f.edges.push(Edge::new("s", "n"));
    f
}

fn test_node(e: &Env, f: &Flow) -> iyakki_flow::engine::NodeTest {
    e.engine.test_node(f, "n", json!({}), Value::Null).unwrap()
}

fn class_of(kind: &str, node: &Node, err: NodeError) -> ErrClass {
    Registry::builtin().get(kind).unwrap().classify_error(&err, node)
}

fn node(kind: &str, cfg: Value) -> Node {
    Node::new("n", kind, cfg)
}

#[test]
fn http_builds_the_request_and_reads_the_answer() {
    let e = env("k-http");
    e.host.secret_is("tok", "T0k3n-Value");
    e.host.on_http(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "https://api.example.com/items?x=1");
        assert_eq!(String::from_utf8_lossy(&req.body), r#"{"a":1,"b":"two"}"#);
        assert!(req.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
        assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer T0k3n-Value"));
        assert_eq!(req.timeout_ms, 30_000);
        assert!(req.follow_redirects && !req.tls.insecure);
        Ok(HttpResponse { status: 201, headers: vec![("Content-Type".into(), "application/json".into()), ("X-Id".into(), "9".into())], body: r#"{"data": {"items": [{"id": 5}, {"id": 6}], "total": 2}}"#.into(), elapsed_ms: 12 })
    });
    let f = one("http", json!({"method": "post", "url": "https://api.example.com/items?x=1", "headers": {"Authorization": "Bearer {{secret.tok}}"}, "body": {"a": 1, "b": "two"}, "expect_status": [201], "extract": {"first": "data.items[0].id", "total": "data.total", "xid": "headers.x-id", "code": "status"}}));
    let t = test_node(&e, &f);
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["first"], 5);
    assert_eq!(t.output["total"], 2);
    assert_eq!(t.output["xid"], "9");
    assert_eq!(t.output["code"], 201);
    assert_eq!(t.output["body"]["data"]["total"], 2);
    assert!(!format!("{t:?}").contains("T0k3n"), "no secret in the result");
    let t2 = format!("{:?}", t.logs);
    assert!(!t2.contains("T0k3n-Value"));
}

#[test]
fn http_status_expectations_and_errors() {
    let e = env("k-http2");
    e.host.on_http(|_| Ok(HttpResponse { status: 503, headers: vec![("retry-after".into(), "7".into())], body: "try later".into(), elapsed_ms: 1 }));
    let t = test_node(&e, &one("http", json!({"url": "https://x.example/"})));
    assert!(!t.ok && t.error.contains("HTTP 503") && t.error.contains("try later"));
    e.host.on_http(|_| status(404, "nope"));
    assert!(test_node(&e, &one("http", json!({"url": "https://x.example/", "expect_status": [404]}))).ok, "an expected 404 is a success");
    let t = test_node(&e, &one("http", json!({"url": "ftp://x"})));
    assert!(!t.ok && t.error.contains("http://"));
    e.host.on_http(|_| Ok(HttpResponse { status: 200, body: "plain text".into(), ..Default::default() }));
    let t = test_node(&e, &one("http", json!({"url": "https://x.example/"})));
    assert_eq!(t.output["body"], "plain text");
}

#[test]
fn http_errors_are_classified_by_what_is_worth_retrying() {
    let n = node("http", json!({}));
    for s in [408u16, 425, 429, 500, 502, 503, 504] {
        assert_eq!(class_of("http", &n, NodeError { http_status: Some(s), ..Default::default() }), ErrClass::Retryable, "{s}");
    }
    for s in [400u16, 401, 403, 404, 409, 422, 501, 505] {
        assert_eq!(class_of("http", &n, NodeError { http_status: Some(s), ..Default::default() }), ErrClass::Permanent, "{s}");
    }
    assert_eq!(class_of("http", &n, NodeError::timeout("t")), ErrClass::Retryable);
    assert_eq!(class_of("http", &n, NodeError { connection: true, ..Default::default() }), ErrClass::Retryable, "connection reset or refused");
    assert_eq!(class_of("http", &n, NodeError::config("bad url")), ErrClass::Permanent);
    assert_eq!(class_of("remote", &node("remote", json!({})), NodeError { http_status: Some(503), ..Default::default() }), ErrClass::Retryable);
}

#[test]
fn a_remote_call_signs_in_from_the_vault() {
    let e = env("k-remote");
    e.host.secret_is("peer", "P33r-Secret");
    e.host.on_http(|req| {
        let auth = req.headers.iter().find(|(k, _)| k == "Authorization").map(|x| x.1.clone()).unwrap_or_default();
        ok_json(&format!(r#"{{"auth": "{}"}}"#, if auth.starts_with("Bearer P33r") { "bearer-ok" } else { &auth }))
    });
    let t = test_node(&e, &one("remote", json!({"url": "https://other.iyakki.local/api/run", "auth": {"kind": "bearer", "secret": "peer"}})));
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["body"]["auth"], "bearer-ok");
    let t = test_node(&e, &one("remote", json!({"url": "https://other/", "auth": {"kind": "basic", "secret": "peer", "user": "bob"}})));
    assert!(t.ok);
    let basic = e.host.http_calls.lock().unwrap().last().unwrap().headers.iter().find(|(k, _)| k == "Authorization").unwrap().1.clone();
    assert!(basic.starts_with("Basic "), "{basic}");
    let t = test_node(&e, &one("remote", json!({"url": "https://other/", "auth": {"kind": "header", "secret": "peer", "header": "X-Key"}})));
    assert!(t.ok);
    assert!(e.host.http_calls.lock().unwrap().last().unwrap().headers.iter().any(|(k, v)| k == "X-Key" && v == "P33r-Secret"));
    let t = test_node(&e, &one("remote", json!({"url": "https://other/", "auth": {"kind": "bearer", "secret": "missing"}})));
    assert!(!t.ok && t.error.contains("no secret `missing`"));
    assert!(!t.error.contains("P33r"));
}

#[test]
fn a_command_is_an_argument_list_in_the_workspace_with_its_output_parsed() {
    let e = env("k-cli");
    e.host.secret_is("pw", "Pa55word!");
    e.host.on_exec(|req| {
        assert_eq!(req.argv, vec!["backup", "--to", "s3://b/x y", "--flag"]);
        assert_eq!(req.cwd, "data/out");
        assert!(req.env.contains(&("PW".to_string(), "Pa55word!".to_string())));
        assert_eq!(req.runner, RunnerKind::Local);
        assert_eq!(req.timeout_ms, 30_000);
        Ok(ok_exec("{\"files\": 12, \"bytes\": 300}\n"))
    });
    let f = one("cli", json!({"command": ["backup", "--to", "{{ inputs.dest }}", "--flag"], "cwd": "data/out", "env": {"PW": "{{secret.pw}}"}, "parse": "json"}));
    let t = e.engine.test_node(&f, "n", json!({"dest": "s3://b/x y"}), Value::Null).unwrap();
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["data"]["files"], 12);
    assert_eq!(t.output["code"], 0);
    assert!(!format!("{t:?}").contains("Pa55word"));
    // lines and regex
    e.host.on_exec(|_| Ok(ok_exec("a\nb\n\nc\n")));
    assert_eq!(test_node(&e, &one("cli", json!({"command": ["x"], "parse": "lines"}))).output["data"], json!(["a", "b", "c"]));
    e.host.on_exec(|_| Ok(ok_exec("version 4.12.3 build 9")));
    let t = test_node(&e, &one("cli", json!({"command": ["x"], "parse": "regex", "pattern": "(\\d+)\\.(\\d+)"})));
    assert_eq!((t.output["data"]["1"].clone(), t.output["data"]["2"].clone()), (json!("4"), json!("12")));
    assert!(!test_node(&e, &one("cli", json!({"command": ["x"], "parse": "json"}))).ok, "text is not JSON");
}

#[test]
fn a_command_never_goes_through_a_shell_or_out_of_its_folder() {
    let e = env("k-cli2");
    e.host.on_exec(|_| Ok(ok_exec("x")));
    for bad in [json!({"command": "ls -l"}), json!({"command": ["sh", "-c", "ls"]}), json!({"command": ["/bin/bash", "-c", "x"]}), json!({"command": ["ls"], "cwd": "../etc"}), json!({"command": ["ls"], "cwd": "/etc"}), json!({"command": []}), json!({"command": ["x"], "stdin": "data"})] {
        let f = one("cli", bad.clone());
        assert!(validate::has_errors(&validate::validate_flow(&f, &Registry::builtin())), "{bad}");
        let t = test_node(&e, &f);
        assert!(!t.ok, "{bad}");
    }
    assert!(e.host.exec_calls.lock().unwrap().is_empty(), "nothing was started");
}

#[test]
fn command_failures_and_their_classes() {
    let e = env("k-cli3");
    e.host.on_exec(|_| Ok(failed_exec(75, "temporary failure, try again")));
    let t = test_node(&e, &one("cli", json!({"command": ["job"]})));
    assert!(!t.ok && t.error.contains("exited with code 75") && t.error.contains("temporary failure"));
    let n = node("cli", json!({"command": ["job"], "retry_exit_codes": [75, 111], "retry_patterns": ["Connection reset", "try AGAIN"]}));
    assert_eq!(class_of("cli", &n, NodeError { exit_code: Some(75), ..Default::default() }), ErrClass::Retryable);
    assert_eq!(class_of("cli", &n, NodeError { exit_code: Some(2), ..Default::default() }), ErrClass::Permanent);
    assert_eq!(class_of("cli", &n, NodeError { exit_code: Some(2), output: "read: connection reset by peer".into(), ..Default::default() }), ErrClass::Retryable, "pattern, any case");
    assert_eq!(class_of("cli", &n, NodeError { exit_code: Some(2), output: "please try again later".into(), ..Default::default() }), ErrClass::Retryable);
    assert_eq!(class_of("cli", &n, NodeError::timeout("slow")), ErrClass::Retryable);
    assert_eq!(class_of("cli", &n, NodeError { missing: true, ..Default::default() }), ErrClass::Permanent);
    // a timeout and a missing program
    e.host.on_exec(|_| Ok(ExecResult { timed_out: true, ..Default::default() }));
    let t = test_node(&e, &one("cli", json!({"command": ["slow"]})));
    assert!(t.error.contains("took longer than 30000 ms"));
    e.host.on_exec(|_| Ok(ExecResult { missing: true, ..Default::default() }));
    assert!(test_node(&e, &one("cli", json!({"command": ["nope"]}))).error.contains("was not found"));
}

#[test]
fn the_cli_a_step_needs_must_be_installed_and_new_enough() {
    let e = env("k-cli4");
    e.host.on_exec(|_| Ok(ok_exec("ok")));
    let f = one("cli", json!({"command": ["kubectl", "version"], "cli": {"id": "kubectl", "min_version": "1.28"}}));
    // not installed
    e.host.clis.lock().unwrap().insert("kubectl".into(), iyakki_flow::host::CliInfo { installed: false, version: String::new() });
    let t = test_node(&e, &f);
    assert!(!t.ok && t.error.contains("not installed"), "{}", t.error);
    // too old
    e.host.cli_is("kubectl", "1.26.0");
    let t = test_node(&e, &f);
    assert!(!t.ok && t.error.contains("older than the 1.28"), "{}", t.error);
    // fine
    e.host.cli_is("kubectl", "1.30.1");
    assert!(test_node(&e, &f).ok);
    // the class: never retried, and the error says what is needed (the healing agent reads this)
    let n = node("cli", f.node("n").unwrap().config.clone());
    let err = NodeError { needs_cli: Some(("kubectl".into(), "1.28".into(), "1.26.0".into())), ..Default::default() };
    assert_eq!(class_of("cli", &n, err), ErrClass::Permanent);
}

#[test]
fn a_node_can_run_in_a_sandbox_job_and_says_so() {
    let e = env("k-sandbox");
    e.host.on_exec(|req| {
        assert_eq!(req.runner, RunnerKind::K3sJob);
        assert_eq!(req.image, "localhost/tools:abc");
        Ok(ExecResult { code: Some(0), stdout: "in job".into(), ran_in: "job flow-t-n in namespace shop-dev".into(), ..Default::default() })
    });
    let mut f = one("cli", json!({"command": ["echo", "x"], "image": "localhost/tools:abc"}));
    f.nodes[1].runner = Some(RunnerKind::K3sJob);
    let t = test_node(&e, &f);
    assert!(t.ok, "{}", t.error);
    assert!(t.logs.iter().any(|l| l.contains("sandbox (k3s)")), "{:?}", t.logs);
    assert_eq!(t.output["ran_in"], "job flow-t-n in namespace shop-dev");
}

#[test]
fn grpc_goes_through_grpcurl_with_a_deadline_and_maps_status_codes() {
    let e = env("k-grpc");
    e.host.cli_is("grpcurl", "1.9.0");
    e.host.on_exec(|req| {
        assert_eq!(req.argv[0], "grpcurl");
        let has = |a: &str| req.argv.iter().any(|x| x == a);
        assert!(has("-plaintext") && has("-max-time") && has("-d"));
        let i = req.argv.iter().position(|x| x == "-max-time").unwrap();
        assert_eq!(req.argv[i + 1], "0.7", "a deadline of 700 ms");
        let j = req.argv.iter().position(|x| x == "-d").unwrap();
        assert_eq!(req.argv[j + 1], r#"{"name":"ada"}"#);
        let k = req.argv.iter().position(|x| x == "-H").unwrap();
        assert_eq!(req.argv[k + 1], "authorization: Bearer G-Secret");
        assert_eq!(&req.argv[req.argv.len() - 2..], ["svc.example:50051", "pkg.Greeter/SayHello"]);
        Ok(ok_exec("{\"message\": \"hi ada\"}"))
    });
    e.host.secret_is("g", "G-Secret");
    let f = one("grpc", json!({"address": "svc.example:50051", "method": "pkg.Greeter/SayHello", "plaintext": true, "data": {"name": "ada"}, "deadline_ms": 700, "metadata": {"authorization": "Bearer {{secret.g}}"}, "cli": {"min_version": "1.8.0"}}));
    let t = test_node(&e, &f);
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["data"]["message"], "hi ada");
    assert_eq!(t.output["code"], "OK");
    // proto files and import paths
    e.host.on_exec(|req| {
        let p = req.argv.iter().position(|x| x == "-proto").unwrap();
        assert_eq!(req.argv[p + 1], "api.proto");
        assert!(req.argv.windows(2).any(|w| w[0] == "-import-path" && w[1] == "protos"));
        Ok(ok_exec("{}"))
    });
    assert!(test_node(&e, &one("grpc", json!({"address": "h:1", "method": "a.B/c", "proto_path": "api.proto", "import_paths": ["protos"]}))).ok);
}

#[test]
fn grpc_status_codes_decide_what_is_retried() {
    let n = node("grpc", json!({}));
    for c in ["UNAVAILABLE", "DEADLINE_EXCEEDED", "RESOURCE_EXHAUSTED", "ABORTED"] {
        assert_eq!(class_of("grpc", &n, NodeError { grpc_code: Some(c.into()), ..Default::default() }), ErrClass::Retryable, "{c}");
    }
    for c in ["INVALID_ARGUMENT", "NOT_FOUND", "PERMISSION_DENIED", "UNAUTHENTICATED", "UNIMPLEMENTED", "ALREADY_EXISTS", "INTERNAL"] {
        assert_eq!(class_of("grpc", &n, NodeError { grpc_code: Some(c.into()), timed_out: false, ..Default::default() }), ErrClass::Permanent, "{c}");
    }
    let e = env("k-grpc2");
    e.host.cli_is("grpcurl", "1.9.0");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = calls.clone();
    e.host.on_exec(move |_| match c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
        0 => Ok(failed_exec(64, "ERROR:\n  Code: Unavailable\n  Message: connection error")),
        1 => Ok(ExecResult { timed_out: true, ..Default::default() }),
        _ => Ok(ok_exec("{\"ok\": true}")),
    });
    let mut f = Flow::new("g", "G");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut c = Node::new("c", "grpc", json!({"address": "h:1", "method": "a.B/c", "plaintext": true}));
    c.retry = Some(RetryPolicy::list(&[(700, 0), (400, 100), (300, 200)]));
    f.nodes.push(c);
    f.edges.push(Edge::new("s", "c"));
    e.publish(f);
    let r = e.run("g");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.nodes["c"].attempts.len(), 3);
    assert!(r.nodes["c"].attempts[0].error.contains("UNAVAILABLE") && r.nodes["c"].attempts[1].error.contains("DEADLINE_EXCEEDED"));
    // a permanent status stops at once
    let e = env("k-grpc3");
    e.host.cli_is("grpcurl", "1.9.0");
    e.host.on_exec(|_| Ok(failed_exec(64, "ERROR:\n  Code: InvalidArgument\n  Message: name is required")));
    let mut f = Flow::new("g", "G");
    f.nodes.push(Node::new("s", "start", json!({})));
    let mut c = Node::new("c", "grpc", json!({"address": "h:1", "method": "a.B/c"}));
    c.retry = Some(RetryPolicy::fixed(4, 0));
    f.nodes.push(c);
    f.edges.push(Edge::new("s", "c"));
    e.publish(f);
    let r = e.run("g");
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(r.nodes["c"].attempts.len(), 1);
    assert!(r.error.contains("INVALID_ARGUMENT") && r.error.contains("name is required"));
}

#[test]
fn grpc_asks_for_grpcurl_when_it_is_missing() {
    let e = env("k-grpc4");
    e.host.clis.lock().unwrap().insert("grpcurl".into(), iyakki_flow::host::CliInfo { installed: false, version: String::new() });
    e.host.on_exec(|_| Ok(ok_exec("{}")));
    let t = test_node(&e, &one("grpc", json!({"address": "h:1", "method": "a.B/c"})));
    assert!(!t.ok && t.error.contains("grpcurl is not installed"), "{}", t.error);
    assert!(e.host.exec_calls.lock().unwrap().is_empty());
    let f = one("grpc", json!({"address": "no-port", "method": "x"}));
    assert!(validate::has_errors(&validate::validate_flow(&f, &Registry::builtin())));
}

#[test]
fn thrift_is_planned_and_says_so() {
    let reg = Registry::builtin();
    let k = reg.get("thrift").unwrap();
    assert!(matches!(k.status(), kinds::KindStatus::Planned(_)));
    let f = one("thrift", json!({"endpoint": "h:9090", "service": "S", "method": "m"}));
    let issues = validate::validate_flow(&f, &reg);
    assert!(issues.iter().any(|i| i.level == kinds::Level::Error && i.msg.contains("planned")));
    let e = env("k-thrift");
    let t = test_node(&e, &f);
    assert!(!t.ok && t.error.contains("planned"));
    assert!(e.engine.start_run(StartRequest::new("t").with_flow(f)).is_err(), "a flow with a thrift node cannot run");
}

#[test]
fn the_browser_node_keeps_the_consent_levels_and_maps_steps() {
    let e = env("k-browser");
    e.host.browser_results.lock().unwrap().push_back(Ok(BrowserResult { report: "3 steps passed".into(), extracted: [("title".to_string(), json!("Example Domain"))].into_iter().collect(), screenshots: vec!["/tmp/shot.png".into()], url: "https://example.com/".into() }));
    let f = one("browser", json!({"steps": [{"open": "https://example.com"}, {"wait": {"text": "Example"}}, {"assert": {"text": "Domain"}}, {"screenshot": "home"}], "allow_domains": ["example.com"], "extract": {"title": "title"}}));
    let t = test_node(&e, &f);
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["title"], "Example Domain");
    let req = e.host.browser_calls.lock().unwrap()[0].clone();
    assert_eq!(req.level, "read_only", "read-only is the default");
    assert_eq!(req.target, "headless");
    assert_eq!(req.steps[0], json!({"goto": "https://example.com"}));
    assert_eq!(req.steps[1], json!({"wait_for": {"text": "Example"}}));
    assert_eq!(req.steps[2], json!({"expect": {"text": "Domain"}}));
    assert_eq!(req.extract, vec![("title".to_string(), "title".to_string(), String::new())]);
    // interacting steps need the interactive level, which the consent system then asks about per site
    let bad = one("browser", json!({"steps": [{"click": {"role": "button", "name": "Buy"}}], "allow_domains": ["shop.example"]}));
    assert!(validate::has_errors(&validate::validate_flow(&bad, &Registry::builtin())));
    let ok = one("browser", json!({"steps": [{"click": {"role": "button", "name": "Go"}}], "allow_domains": ["shop.example"], "level": "interactive"}));
    assert!(!validate::has_errors(&validate::validate_flow(&ok, &Registry::builtin())));
    let none = one("browser", json!({"steps": [{"open": "https://x"}]}));
    assert!(validate::validate_flow(&none, &Registry::builtin()).iter().any(|i| i.msg.contains("allow_domains")));
    // classes: navigation trouble retries, a failed assertion does not
    let n = node("browser", json!({}));
    assert_eq!(class_of("browser", &n, NodeError { navigation: true, ..Default::default() }), ErrClass::Retryable);
    assert_eq!(class_of("browser", &n, NodeError::timeout("nav")), ErrClass::Retryable);
    assert_eq!(class_of("browser", &n, NodeError::new("assertion failed: text not found")), ErrClass::Permanent);
    assert_eq!(class_of("browser", &n, NodeError::config("consent refused")), ErrClass::Permanent);
    // a read-only page check can be repeated safely after a restart
    assert!(Registry::builtin().get("browser").unwrap().idempotent(&node("browser", json!({}))));
    assert!(!Registry::builtin().get("browser").unwrap().idempotent(&node("browser", json!({"level": "interactive"}))));
}

#[test]
fn the_agent_node_checks_its_answer_against_the_schema() {
    let e = env("k-agent");
    e.host.agent_results.lock().unwrap().push_back(Ok(AgentResult { text: "Here: {\"summary\": \"all fine\", \"score\": 9}".into(), cents: 3 }));
    let f = one("agent", json!({"prompt": "Summarise {{ inputs.topic }}", "tools": ["web_search"], "output_schema": {"required": ["summary"]}, "max_cents": 5}));
    let t = e.engine.test_node(&f, "n", json!({"topic": "tides"}), Value::Null).unwrap();
    assert!(t.ok, "{}", t.error);
    assert_eq!(t.output["summary"], "all fine");
    assert_eq!(t.output["json"]["score"], 9);
    let req = e.host.agent_calls.lock().unwrap()[0].clone();
    assert_eq!(req.prompt, "Summarise tides");
    assert_eq!(req.tools, vec!["web_search"]);
    assert_eq!(req.max_cents, 5);
    e.host.agent_results.lock().unwrap().push_back(Ok(AgentResult { text: "{\"other\": 1}".into(), cents: 1 }));
    let t = test_node(&e, &f);
    assert!(!t.ok && t.error.contains("no `summary`"));
    e.host.agent_results.lock().unwrap().push_back(Ok(AgentResult { text: "no json at all".into(), cents: 1 }));
    assert!(!test_node(&e, &f).ok);
    // a model that wrote the wrong shape may do better next time
    let n = node("agent", json!({}));
    assert_eq!(class_of("agent", &n, NodeError { transient: Some(true), ..Default::default() }), ErrClass::Retryable);
    assert_eq!(class_of("agent", &n, NodeError::new("budget stop")), ErrClass::Permanent);
}

#[test]
fn script_and_notify_nodes_use_the_host() {
    let e = env("k-script");
    e.host.script_results.lock().unwrap().push_back(Ok(json!({"sum": 3})));
    let t = test_node(&e, &one("script", json!({"lang": "js", "source": "return {sum: args.a + args.b}", "args": {"a": 1, "b": 2}})));
    assert!(t.ok);
    assert_eq!(t.output["result"]["sum"], 3);
    // a script's `{{` is code, not a template
    e.host.script_results.lock().unwrap().push_back(Ok(json!(1)));
    assert!(test_node(&e, &one("script", json!({"lang": "js", "source": "if (a) {{ return 1 }}"}))).ok);
    let t = test_node(&e, &one("notify", json!({"title": "Hello {{ inputs.who }}", "body": "b", "level": "warn"})));
    assert!(t.ok);
    let n = e.host.notices.lock().unwrap().last().unwrap().clone();
    assert_eq!((n.title.as_str(), n.level.as_str()), ("Hello ", "warn"));
    assert!(validate::has_errors(&validate::validate_flow(&one("script", json!({"lang": "ruby", "source": "x"})), &Registry::builtin())));
    assert!(validate::has_errors(&validate::validate_flow(&one("script", json!({"lang": "wasm"})), &Registry::builtin())));
}

#[test]
fn every_kind_describes_its_form_and_ports() {
    let reg = Registry::builtin();
    for k in reg.all() {
        assert!(!k.label().is_empty() && !k.blurb().is_empty() && !k.category().is_empty(), "{}", k.id());
        let ports = k.ports(&Node::new("x", k.id(), json!({})));
        if k.id() != "start" {
            assert!(!ports.inputs.is_empty(), "{}", k.id());
        }
        let keys: Vec<&str> = k.schema().iter().map(|f| f.key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys.len(), sorted.len(), "{} has a duplicate field", k.id());
        for key in k.secret_fields().iter().chain(k.raw_fields()) {
            assert!(keys.contains(key), "{}: {key} is not in the schema", k.id());
        }
    }
    assert!(reg.get("condition").unwrap().ports(&node("condition", json!({}))).outputs == vec!["true", "false"]);
    let sw = node("switch", json!({"cases": [{"equals": 1, "port": "a"}, {"equals": 2, "port": "b"}]}));
    assert_eq!(reg.get("switch").unwrap().ports(&sw).outputs, vec!["a", "b", "default"]);
    let _ = Duration::from_secs(1);
}
