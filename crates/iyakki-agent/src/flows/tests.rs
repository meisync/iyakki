//! Tests of the real host and the assistant's tools: HTTP against a local listener, commands through a fake runner, the sandbox Job
//! through the real `K8sTools` with a fake runner, tool risks, redaction. Nothing here touches the network beyond 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_flow::host::*;
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};

use super::tools::*;
use super::*;
use crate::agent::{hard_ask, Risk, Tools};
use crate::devcluster::{ClusterProfile, Source};
use crate::k8stools::K8sTools;
use crate::wsexec::FakeRunner;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-flows-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn policy(hosts: &[&str]) -> Policy {
    let mut p = Role::User.template();
    p.net_hosts = hosts.iter().map(|s| s.to_string()).collect();
    p
}

fn env_with(tag: &str, p: Policy) -> (Arc<FlowEnv>, PathBuf) {
    let d = tmp(tag);
    let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("flowtester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    (Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &d.join("home"))), d)
}

/// A one-thread HTTP server answering each connection with the next canned response (`Connection: close`).
fn serve(responses: Vec<String>, delay_first_ms: u64) -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for (i, resp) in responses.into_iter().enumerate() {
            let Ok((mut c, _)) = l.accept() else { return };
            let mut buf = [0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = c.read(&mut buf).unwrap_or(0);
                got.extend_from_slice(&buf[..n]);
                if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            s2.lock().unwrap().push(String::from_utf8_lossy(&got).into_owned());
            if i == 0 && delay_first_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_first_ms));
            }
            let _ = c.write_all(resp.as_bytes());
        }
    });
    (port, seen)
}

fn http_resp(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let mut s = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s
}

fn req(url: &str, timeout_ms: u64) -> HttpRequest {
    HttpRequest { method: "GET".into(), url: url.into(), timeout_ms, follow_redirects: true, ..Default::default() }
}

#[test]
fn http_goes_through_the_real_transport_with_headers_and_the_hosts_policy() {
    let (env, _d) = env_with("http1", policy(&["127.0.0.1"]));
    let (port, seen) = serve(vec![http_resp(200, &[("X-Thing", "abc"), ("Content-Type", "application/json")], r#"{"ok":true}"#)], 0);
    let mut r = req(&format!("http://127.0.0.1:{port}/a?b=1"), 3000);
    r.headers.push(("X-Test".into(), "yes".into()));
    let resp = env.host.http(&r, &Cancel::new()).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, r#"{"ok":true}"#);
    assert_eq!(resp.header("x-thing"), Some("abc"), "response headers come back");
    let sent = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(sent.starts_with("get /a?b=1 ") && sent.contains("x-test: yes"), "{sent}");
    // a host the account may not reach is refused before anything is sent
    let (env2, _d2) = env_with("http2", policy(&["example.org"]));
    let e = env2.host.http(&req(&format!("http://127.0.0.1:{port}/"), 1000), &Cancel::new()).unwrap_err();
    assert!(e.config && e.message.contains("may not reach 127.0.0.1"), "{e:?}");
}

#[test]
fn http_errors_map_to_timeouts_and_connection_problems_and_redirects_are_checked() {
    let (env, _d) = env_with("http3", policy(&["127.0.0.1"]));
    // a server that answers too late
    let (port, _) = serve(vec![http_resp(200, &[], "late")], 1500);
    let t = std::time::Instant::now();
    let e = env.host.http(&req(&format!("http://127.0.0.1:{port}/"), 300), &Cancel::new()).unwrap_err();
    assert!(e.timed_out, "{e:?}");
    assert!(t.elapsed() < Duration::from_millis(1400), "the per-attempt limit of 300 ms was kept");
    // nobody listens
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = env.host.http(&req(&format!("http://127.0.0.1:{dead}/"), 1000), &Cancel::new()).unwrap_err();
    assert!(e.connection && !e.timed_out, "{e:?}");
    // a redirect inside the policy is followed; one to a host outside it is refused
    let (p2, _) = serve(vec![http_resp(302, &[("Location", "/next")], ""), http_resp(200, &[], "arrived")], 0);
    let r = env.host.http(&req(&format!("http://127.0.0.1:{p2}/start"), 2000), &Cancel::new()).unwrap();
    assert_eq!(r.body, "arrived");
    let (p3, _) = serve(vec![http_resp(302, &[("Location", "http://evil.example.net/x")], "")], 0);
    let e = env.host.http(&req(&format!("http://127.0.0.1:{p3}/"), 2000), &Cancel::new()).unwrap_err();
    assert!(e.config && e.message.contains("evil.example.net"), "{e:?}");
    let (p4, _) = serve(vec![http_resp(302, &[("Location", "/x")], "")], 0);
    let mut r4 = req(&format!("http://127.0.0.1:{p4}/"), 2000);
    r4.follow_redirects = false;
    assert_eq!(env.host.http(&r4, &Cancel::new()).unwrap().status, 302);
}

#[test]
fn a_flow_retries_against_a_real_server_and_honours_retry_after() {
    let (env, _d) = env_with("http4", policy(&["127.0.0.1"]));
    let (port, seen) = serve(vec![http_resp(503, &[("Retry-After", "1")], "busy"), http_resp(200, &[("Content-Type", "application/json")], r#"{"n": 5}"#)], 0);
    let mut f = Flow::new("live", "Live");
    f.nodes.push(Node::new("start", "start", json!({})));
    let mut a = Node::new("a", "http", json!({"url": format!("http://127.0.0.1:{port}/x"), "extract": {"n": "n"}}));
    a.retry = Some(RetryPolicy::fixed(3, 10));
    f.nodes.push(a);
    f.nodes.push(Node::new("end", "end", json!({"outputs": {"n": "{{ nodes.a.output.n }}"}})));
    f.edges = vec![Edge::new("start", "a"), Edge::new("a", "end")];
    env.create_draft(f, "user:t").unwrap();
    env.store.publish("live", &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let t = std::time::Instant::now();
    let o = env.engine.start_run(StartRequest::new("live")).unwrap();
    let r = env.engine.wait_for(&o.run_id, Duration::from_secs(15), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["n"], 5);
    assert!(t.elapsed() >= Duration::from_millis(950), "the server's Retry-After of 1 s replaced the 10 ms pause");
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert!(r.nodes["a"].attempts[0].error.contains("503"));
}

fn fake_runner_env(tag: &str) -> (Arc<FlowEnv>, Arc<FakeRunner>, PathBuf) {
    let (env, d) = env_with(tag, policy(&["*"]));
    let r = Arc::new(FakeRunner::new(&["tar"]));
    // the host is shared: replace its runner
    let host = Arc::new({
        let mut h = AgentHost::new(&env.svc, &env.user, &env.policy, &env.data, &env.dir);
        h.runner = r.clone();
        h
    });
    let e2 = FlowEnv { user: env.user.clone(), policy: env.policy.clone(), svc: env.svc.clone(), store: env.store.clone(), engine: Engine::new(EngineCfg::new(&env.data), host.clone(), env.reg.clone(), Arc::new(SystemClock), env.store.clone()), host, reg: env.reg.clone(), dir: env.dir.clone(), data: env.data.clone() };
    (Arc::new(e2), r, d)
}

fn exec_req(argv: &[&str], cwd: &str) -> ExecRequest {
    ExecRequest { argv: argv.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), env: vec![("A".into(), "1".into())], timeout_ms: 5000, runner: RunnerKind::Local, flow_id: "f1".into(), run_id: "r1".into(), node_id: "n1".into(), ..Default::default() }
}

#[test]
fn commands_run_by_argument_list_in_the_workspace_with_the_cli_tiles_programs_on_the_path() {
    let (env, r, _d) = fake_runner_env("exec1");
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |_, a, d| a2.lock().unwrap().push(format!("{a} {d}"))));
    r.answer("tar -czf out.tgz docs", "packed");
    let out = env.host.exec(&exec_req(&["tar", "-czf", "out.tgz", "docs"], "sub/dir"), &Cancel::new()).unwrap();
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, "packed");
    let call = r.calls().pop().unwrap();
    assert_eq!(call.argv, vec!["tar", "-czf", "out.tgz", "docs"], "an argument list, untouched");
    assert!(call.cwd.ends_with("work/f1/sub/dir"), "{}", call.cwd);
    assert!(std::path::Path::new(&call.cwd).is_dir(), "the folder was made inside the workspace");
    let path = call.env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
    assert!(path.starts_with(&env.svc.clis.bin_dir().display().to_string()), "installed CLIs come first: {path}");
    assert!(call.env.contains(&("A".to_string(), "1".to_string())));
    // programs that change the machine, and ways out of the workspace
    for bad in [exec_req(&["sudo", "ls"], ""), exec_req(&["mkfs.ext4", "/dev/x"], ""), exec_req(&["/bin/ls"], ""), exec_req(&["../x"], ""), exec_req(&["ls"], "../..")] {
        let n = r.calls().len();
        let e = env.host.exec(&bad, &Cancel::new()).unwrap_err();
        assert!(e.config || e.missing, "{:?}: {e:?}", bad.argv);
        assert_eq!(r.calls().len(), n, "nothing was started for {:?}", bad.argv);
    }
    // a workspace per flow
    let mut other = exec_req(&["tar"], "");
    other.flow_id = "f2".into();
    env.host.exec(&other, &Cancel::new()).unwrap();
    assert!(r.calls().pop().unwrap().cwd.ends_with("work/f2"));
    // a remote runner is a stub that says so
    let mut rr = exec_req(&["tar"], "");
    rr.runner = RunnerKind::Remote;
    assert!(env.host.exec(&rr, &Cancel::new()).unwrap_err().message.contains("another machine"));
    assert!(audits.lock().unwrap().iter().any(|l| l.contains("flow.exec") && l.contains("f1/n1 tar")), "{:?}", audits.lock().unwrap());
}

fn profile(name: &str) -> ClusterProfile {
    ClusterProfile { name: name.into(), kubeconfig: "/home/u/.kube/k3s.yaml".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::K3s, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: "v1.36.5+k3s1".into(), can_i: "get pods yes".into(), problem: String::new(), fix: String::new() }
}

const JOB_OK: &str = r#"{"status":{"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]}}"#;
const JOB_FAILED: &str = r#"{"status":{"failed":1,"conditions":[{"type":"Failed","status":"True"}]}}"#;
const POD_DONE: &str = r#"{"items":[{"metadata":{"name":"flow-f1-n1-abcde"},"status":{"phase":"Succeeded","containerStatuses":[{"state":{"terminated":{"exitCode":0,"reason":"Completed"}}}]}}]}"#;
const POD_FAILED: &str = r#"{"items":[{"metadata":{"name":"flow-f1-n1-abcde"},"status":{"phase":"Failed","containerStatuses":[{"state":{"terminated":{"exitCode":3,"reason":"Error"}}}]}}]}"#;

fn k8s_env(tag: &str, prof: ClusterProfile, job: &str, pod: &str) -> (Arc<FlowEnv>, Arc<FakeRunner>) {
    let (e, r, _d) = k8s_env_d(tag, prof, job, pod);
    (e, r)
}

fn k8s_env_d(tag: &str, prof: ClusterProfile, job: &str, pod: &str) -> (Arc<FlowEnv>, Arc<FakeRunner>, PathBuf) {
    let (env, d) = env_with(tag, policy(&["*"]));
    let r = Arc::new(FakeRunner::new(&["kubectl"]));
    r.answer("get services", r#"{"items":[]}"#);
    r.answer("get namespace", "namespace/flows-x");
    r.answer("diff -f", "");
    r.answer("apply -f", "job.batch/flow-f1-n1 created");
    r.answer("get job flow-f1-n1", job);
    r.answer("get pods", pod);
    r.answer("logs flow-f1-n1-abcde", "hello from the job\n");
    r.answer("delete job", "deleted");
    let (r2, root) = (r.clone(), d.join("jobs"));
    std::fs::create_dir_all(&root).unwrap();
    *env.host.k8s.lock().unwrap() = Some(Arc::new(move |_ns: &str| Ok(Arc::new(K8sTools::new(r2.clone(), prof.clone(), "flows-x", "flows", &root, Arc::new(|_, _| {}))))));
    (env, r, d)
}

fn job_req() -> ExecRequest {
    ExecRequest { argv: vec!["make".into(), "test".into()], env: vec![("MODE".into(), "ci".into())], timeout_ms: 90_000, runner: RunnerKind::K3sJob, image: "localhost/tools:abc".into(), flow_id: "f1".into(), run_id: "r1".into(), node_id: "n1".into(), ..Default::default() }
}

#[test]
fn a_node_can_run_as_a_sandbox_job_by_the_same_code_as_deploy_run_job() {
    let (env, r, _d) = k8s_env_d("k3s1", profile("k3s"), JOB_OK, POD_DONE);
    let out = env.host.exec(&job_req(), &Cancel::new()).unwrap();
    assert_eq!((out.code, out.timed_out), (Some(0), false));
    assert_eq!(out.stdout, "hello from the job\n");
    assert!(out.ran_in.contains("flow-f1-n1") && out.ran_in.contains("flows-x"), "{}", out.ran_in);
    let lines = r.lines();
    assert!(lines.iter().any(|l| l.contains("apply -f")) && lines.iter().any(|l| l.contains("delete job")), "the Job was made and cleaned up: {lines:?}");
    // the manifest is the linted, non-root, limited one
    let yaml = std::fs::read_dir(_d.join("jobs/.iyakki/jobs")).map(|d| d.flatten().map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>()).unwrap_or_default();
    for want in ["kind: Job", "runAsNonRoot: true", "activeDeadlineSeconds: 90", "image: localhost/tools:abc", "limits:", "backoffLimit: 0", "iyakki.dev/job: integration-test"] {
        assert!(yaml.contains(want), "{want} in {yaml}");
    }
    assert!(!yaml.contains("privileged") && !yaml.contains("hostPath"));
    assert!(yaml.contains("make") && yaml.contains("MODE"));
}

#[test]
fn a_failing_job_carries_its_exit_code_and_a_production_cluster_is_refused() {
    let (env, _r) = k8s_env("k3s2", profile("k3s"), JOB_FAILED, POD_FAILED);
    let out = env.host.exec(&job_req(), &Cancel::new()).unwrap();
    assert_eq!(out.code, Some(3));
    assert_eq!(out.stdout, "hello from the job\n");
    // production
    let (env, r) = k8s_env("k3s3", profile("prod-eu"), JOB_OK, POD_DONE);
    let e = env.host.exec(&job_req(), &Cancel::new()).unwrap_err();
    assert!(e.config && e.message.contains("production"), "{e:?}");
    assert!(!r.lines().iter().any(|l| l.contains("apply -f")), "nothing was applied");
    // a secret never goes into a Job's file
    let (env, r) = k8s_env("k3s4", profile("k3s"), JOB_OK, POD_DONE);
    let mut s = job_req();
    s.secrets = true;
    let e = env.host.exec(&s, &Cancel::new()).unwrap_err();
    assert!(e.config && e.message.contains("secret"), "{e:?}");
    assert!(r.calls().is_empty());
    // no image, no cluster
    let mut noimg = job_req();
    noimg.image.clear();
    assert!(env.host.exec(&noimg, &Cancel::new()).unwrap_err().message.contains("image"));
    let (env2, _d) = env_with("k3s5", policy(&["*"]));
    *env2.host.k8s.lock().unwrap() = Some(Arc::new(|_ns: &str| Err("no cluster is connected: start the dev cluster".into())));
    assert!(env2.host.exec(&job_req(), &Cancel::new()).unwrap_err().message.contains("no cluster"));
}

#[test]
fn the_node_kind_rules_reach_the_runner_through_a_real_flow() {
    // a cli node marked as sandbox runs through the K3s runner and its exit code becomes the node's error
    let (env, _r) = k8s_env("k3s6", profile("k3s"), JOB_FAILED, POD_FAILED);
    let mut f = Flow::new("f1", "F1");
    f.nodes.push(Node::new("start", "start", json!({})));
    let mut n = Node::new("n1", "cli", json!({"command": ["make", "test"], "image": "localhost/tools:abc"}));
    n.runner = Some(RunnerKind::K3sJob);
    f.nodes.push(n);
    f.edges.push(Edge::new("start", "n1"));
    env.create_draft(f, "user:t").unwrap();
    env.store.publish("f1", &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let o = env.engine.start_run(StartRequest::new("f1")).unwrap();
    let r = env.engine.wait_for(&o.run_id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("exited with code 3"), "{}", r.error);
}

#[test]
fn the_catalog_offers_grpcurl_and_the_host_reads_cli_versions_from_it() {
    let d = crate::clis::def("grpcurl").expect("grpcurl is in the CLI catalog");
    assert_eq!(d.bins[0].0, "grpcurl");
    assert!(d.checksum.is_some() && d.hosts.contains(&"github.com"));
    let (env, _d) = env_with("cli1", policy(&["*"]));
    assert_eq!(env.host.cli_info("grpcurl"), Some(CliInfo { installed: false, version: String::new() }));
    assert!(env.host.cli_info("not-in-the-catalog").is_none() && !env.host.cli_known("not-in-the-catalog") && env.host.cli_known("kubectl"));
    crate::clis::write_fake_installed(&env.svc.clis, "grpcurl", "#!/bin/sh\necho grpcurl 1.9.1\n");
    let i = env.host.cli_info("grpcurl").unwrap();
    assert!(i.installed);
    assert!(env.host.cli_upgrade("not-in-the-catalog", "1").unwrap_err().contains("not in the CLI catalog"));
}

// ------------------------------------------------------------------------------------------ the tools

fn call(env: &FlowEnv, name: &str, args: Value) -> Result<String, String> {
    env.call(name, args.as_object().map(|_| &args).unwrap_or(&args), None)
}

fn spec() -> Value {
    json!({"name": "Check the shop", "description": "pings it", "nodes": [{"id": "ping", "kind": "http", "config": {"url": "https://shop.example/health", "headers": {"Authorization": "Bearer {{secret.shop}}"}}, "retry": {"attempts": 3, "backoff": {"kind": "fixed", "delay_ms": 500}}}, {"id": "tell", "kind": "notify", "config": {"title": "up"}}], "triggers": [{"kind": "interval", "every_s": 300}]})
}

#[test]
fn the_tools_make_edit_and_read_flows_and_mark_the_agents_changes() {
    let (env, _d) = env_with("tools1", policy(&["*"]));
    assert!(call(&env, "flow_list", json!({})).unwrap().contains("no flows yet"));
    let made = call(&env, "flow_create", spec()).unwrap();
    assert!(made.contains("Made the draft `check-the-shop`") && made.contains("nothing runs until it is published"), "{made}");
    let list = call(&env, "flow_list", json!({})).unwrap();
    assert!(list.contains("check-the-shop") && list.contains("agent changed") && list.contains("published v0"), "{list}");
    let f = env.store.get_draft("check-the-shop").unwrap();
    assert!(f.nodes[0].kind == "start", "a start node was added");
    assert_eq!(f.edges.len(), 2, "chained in order");
    assert!(f.meta.modified_by.iter().any(Change::by_agent));
    assert!(f.nodes.iter().all(|n| n.pos != Pos::default() || n.kind == "start"), "laid out");
    // reading it back: credentials are not shown, references are
    let g = call(&env, "flow_get", json!({"flow": "check-the-shop"})).unwrap();
    assert!(g.contains("node ping [http]") && g.contains("retry(3 attempts, 500 ms apart)") && g.contains("every 300 s"), "{g}");
    assert!(g.contains("{{secret.shop}}") && g.contains("Validation"));
    // an edit: structured ops, merged config, a new node after another
    let e = call(&env, "flow_edit", json!({"flow": "check-the-shop", "ops": [
        {"op": "update_node", "id": "ping", "config": {"expect_status": [200, 204]}, "timeout_ms": 3000, "retry": {"attempts": 4, "backoff": {"kind": "list", "steps": [{"timeout_ms": 700}, {"timeout_ms": 400, "delay_ms": 200}]}}},
        {"op": "add_node", "id": "log", "kind": "notify", "config": {"title": "again"}, "after": "tell"}
    ]})).unwrap();
    assert!(e.contains("changed node ping") && e.contains("added node log"), "{e}");
    let f = env.store.get_draft("check-the-shop").unwrap();
    let ping = f.node("ping").unwrap();
    assert_eq!(ping.config["url"], "https://shop.example/health", "the other keys stay");
    assert_eq!(ping.config["expect_status"], json!([200, 204]));
    assert_eq!(ping.timeout_ms, Some(3000));
    assert_eq!(ping.retry.as_ref().unwrap().timeout_for(2, 0), 400);
    assert!(f.edges.iter().any(|e| e.from == "tell" && e.to == "log"));
    assert!(f.node("log").unwrap().pos.x > f.node("tell").unwrap().pos.x, "placed to the right of its parent");
    // a bad operation changes nothing
    let before = env.store.get_draft("check-the-shop").unwrap();
    assert!(call(&env, "flow_edit", json!({"flow": "check-the-shop", "ops": [{"op": "remove_node", "id": "log"}, {"op": "update_node", "id": "ghost"}]})).unwrap_err().contains("ghost"));
    assert_eq!(env.store.get_draft("check-the-shop").unwrap(), before);
    // the preview of an edit is a diff
    let pv = env.preview("flow_edit", &json!({"flow": "check-the-shop", "ops": [{"op": "update_node", "id": "ping", "timeout_ms": 9000}]}), None).unwrap();
    assert!(pv.contains("~ node ping") && pv.contains("timeout_ms"), "{pv}");
    assert!(call(&env, "flow_get", json!({"flow": "ghost"})).is_err());
}

#[test]
fn a_template_makes_a_quiet_draft() {
    let (env, _d) = env_with("tools2", policy(&["*"]));
    let o = call(&env, "flow_create", json!({"template": "health-check", "name": "Shop health"})).unwrap();
    assert!(o.contains("`shop-health`"), "{o}");
    let f = env.store.get_draft("shop-health").unwrap();
    assert!(f.triggers.iter().all(|t| !t.enabled));
    assert!(call(&env, "flow_create", json!({"template": "nope"})).unwrap_err().contains("health-check"));
    // the same name again gets a free id
    let o2 = call(&env, "flow_create", json!({"template": "health-check", "name": "Shop health"})).unwrap();
    assert!(o2.contains("`shop-health-2`"), "{o2}");
}

#[test]
fn publish_rollback_apply_and_heal_policy_are_hard_ask_and_the_rest_ask_or_read() {
    for (name, hard) in [("flow_publish", true), ("flow_rollback", true), ("flow_apply_to_runs", true)] {
        assert!(hard_ask(name, &json!({})).is_some() == hard, "{name}");
    }
    assert!(hard_ask("flow_set_heal_policy", &json!({"mode": "auto_fix_simple"})).is_some());
    assert!(hard_ask("flow_set_heal_policy", &json!({"mode": "suggest"})).is_some());
    assert!(hard_ask("flow_set_heal_policy", &json!({"mode": "off"})).is_none(), "narrowing the agent's say needs no extra ask");
    for n in ["flow_list", "flow_get", "flow_runs", "flow_run_status", "flow_logs", "flow_explain_failure", "flow_create", "flow_edit", "flow_run", "flow_cancel"] {
        assert!(hard_ask(n, &json!({})).is_none(), "{n} follows the normal rules");
    }
    let d = defs(&Role::User.template());
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).unwrap().risk;
    for n in ["flow_list", "flow_get", "flow_runs", "flow_run_status", "flow_logs", "flow_explain_failure", "flow_assist_activity"] {
        assert_eq!(risk(n), Risk::Safe, "{n}");
    }
    for n in ["flow_create", "flow_edit", "flow_run", "flow_cancel", "flow_publish", "flow_rollback", "flow_apply_to_runs", "flow_set_heal_policy", "flow_prune_history", "flow_failure_help", "flow_assist_stop_all", "flow_outside_messages_set", "flow_outside_messages_policy", "flow_assist_settings", "flow_trial"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    assert_eq!(risk("flow_outside_messages"), Risk::Safe);
    assert_eq!(d.len(), 35, "the four remote machine tools, 16 flow tools, flow_trial, flow_assist_activity, flow_prune_history, flow_failure_help, flow_assist_stop_all, flow_assist_settings, the three outside-message tools and the six webhook tools");
    assert!(d.iter().all(|x| is_flow_tool(&x.spec.name)));
    // no tools for an account without the app, and the wrapped chat never offers them without it
    let mut p = Role::User.template();
    p.apps.remove(&iyakki_sec::policy::App::Flows);
    assert!(defs(&p).is_empty());
    let (env, _d) = env_with("tools3", p);
    assert!(env.call("flow_list", &json!({}), None).is_err());
}

#[test]
fn publishing_through_the_tool_needs_the_person_and_approves_the_agents_changes_with_them() {
    let (env, _d) = env_with("tools4", policy(&["*"]));
    call(&env, "flow_create", spec()).unwrap();
    // the agent's own attempt through the store is refused: a person has to approve what it changed
    let e = env.store.publish("check-the-shop", &iyakki_flow::store::PublishReq { by: AGENT, label: "", changelog: "x", approved_by: None }).unwrap_err();
    assert!(e.contains("approve"), "{e}");
    // the tool call itself is what the person approved (hard-ask), so it publishes as them
    let o = call(&env, "flow_publish", json!({"flow": "check-the-shop", "changelog": "first version", "label": "1.0.0"})).unwrap();
    assert!(o.contains("version 1"), "{o}");
    let v = env.store.versions("check-the-shop");
    assert_eq!(v[0].label, "1.0.0");
    assert_eq!(v[0].approved_by, format!("user:{}", env.user));
    assert!(v[0].changes.iter().any(Change::by_agent), "the history still says the agent made the changes");
    // edit again, roll back
    call(&env, "flow_edit", json!({"flow": "check-the-shop", "ops": [{"op": "update_node", "id": "ping", "timeout_ms": 9}]})).unwrap();
    call(&env, "flow_publish", json!({"flow": "check-the-shop", "changelog": "short timeout"})).unwrap();
    let rb = call(&env, "flow_rollback", json!({"flow": "check-the-shop", "version": 1})).unwrap();
    assert!(rb.contains("version 3"), "{rb}");
    let pv = env.preview("flow_publish", &json!({"flow": "check-the-shop"}), None);
    assert!(pv.is_some());
    assert!(call(&env, "flow_rollback", json!({"flow": "check-the-shop", "version": 9})).is_err());
}

#[test]
fn heal_policy_changes_go_to_the_draft_and_the_audit_log() {
    let (env, _d) = env_with("tools5", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |u, a, d| a2.lock().unwrap().push(format!("{u} {a} {d}"))));
    call(&env, "flow_create", spec()).unwrap();
    let o = call(&env, "flow_set_heal_policy", json!({"flow": "check-the-shop", "mode": "auto_fix_simple", "max_per_run": 1, "cost_cap_cents": 5})).unwrap();
    assert!(o.contains("auto-fix simple"), "{o}");
    let f = env.store.get_draft("check-the-shop").unwrap();
    assert_eq!(f.defaults.heal.mode, HealMode::AutoFixSimple);
    assert_eq!((f.defaults.heal.max_per_run, f.defaults.heal.cost_cap_cents), (1, 5));
    call(&env, "flow_set_heal_policy", json!({"flow": "check-the-shop", "node": "ping", "mode": "always_ask"})).unwrap();
    assert_eq!(env.store.get_draft("check-the-shop").unwrap().node("ping").unwrap().heal.as_ref().unwrap().mode, HealMode::AlwaysAsk);
    assert!(call(&env, "flow_set_heal_policy", json!({"flow": "check-the-shop", "mode": "sometimes"})).is_err());
    assert!(audits.lock().unwrap().iter().any(|l| l.contains("flow.heal_policy") && l.contains("auto-fix simple")), "{:?}", audits.lock().unwrap());
}

#[test]
fn outside_messages_are_read_and_switched_per_flow_by_chat_and_audited() {
    let (env, _d) = env_with("tools-chan", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |u, a, d| a2.lock().unwrap().push(format!("{u} {a} {d}"))));
    call(&env, "flow_create", spec()).unwrap();
    assert!(call(&env, "flow_outside_messages", json!({"flow": "check-the-shop"})).unwrap().contains("ON"), "a new flow may send");
    env.host.chan.block_new("check-the-shop");
    assert!(call(&env, "flow_outside_messages", json!({"flow": "check-the-shop"})).unwrap().contains("OFF"));
    assert!(call(&env, "flow_outside_messages", json!({})).unwrap().contains("check-the-shop: off"));
    assert!(call(&env, "flow_outside_messages", json!({"flow": "nope"})).unwrap_err().contains("no flow"));
    let r = call(&env, "flow_outside_messages_set", json!({"flow": "check-the-shop", "on": true})).unwrap();
    assert!(r.contains("ON"), "{r}");
    assert!(env.host.chan.allowed("check-the-shop"));
    call(&env, "flow_outside_messages_set", json!({"flow": "check-the-shop", "on": false})).unwrap();
    assert!(!env.host.chan.allowed("check-the-shop"));
    assert!(call(&env, "flow_outside_messages_set", json!({"flow": "check-the-shop"})).is_err(), "on is required");
    assert!(call(&env, "flow_outside_messages_set", json!({"flow": "nope", "on": true})).unwrap_err().contains("no flow"));
    assert!(audits.lock().unwrap().iter().filter(|l| l.contains("flow.outside_messages")).count() >= 2);
}

#[test]
fn the_failure_help_offer_is_set_per_flow_or_in_general_by_chat_and_is_audited() {
    use super::failfix::FailBook;
    let (env, _d) = env_with("tools7", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |u, a, d| a2.lock().unwrap().push(format!("{u} {a} {d}"))));
    call(&env, "flow_create", spec()).unwrap();
    assert!(FailBook::open(&env.data).offer_for("check-the-shop"));
    let o = call(&env, "flow_failure_help", json!({"flow": "check-the-shop", "on": false})).unwrap();
    assert!(o.contains("not offered"), "{o}");
    let b = FailBook::open(&env.data);
    assert!(!b.offer_for("check-the-shop") && b.offer(), "only this flow");
    assert!(call(&env, "flow_failure_help", json!({"flow": "no-such-flow", "on": false})).unwrap_err().contains("no flow"));
    assert!(call(&env, "flow_failure_help", json!({"flow": "check-the-shop"})).is_err(), "on is required");
    call(&env, "flow_failure_help", json!({"all": true, "on": false})).unwrap();
    assert!(!FailBook::open(&env.data).offer());
    let back = call(&env, "flow_failure_help", json!({"flow": "check-the-shop", "on": true})).unwrap();
    assert!(back.contains("general switch is off"), "{back}");
    call(&env, "flow_failure_help", json!({"all": true, "on": true})).unwrap();
    assert!(FailBook::open(&env.data).offer_for("check-the-shop"));
    assert!(audits.lock().unwrap().iter().filter(|l| l.contains("flow.failure_help")).count() >= 4);
}

#[test]
fn stopping_all_assist_by_chat_is_one_way_and_audited() {
    let (env, _d) = env_with("tools8", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |u, a, d| a2.lock().unwrap().push(format!("{u} {a} {d}"))));
    assert!(env.assist_store().global().enabled);
    let o = call(&env, "flow_assist_stop_all", json!({})).unwrap();
    assert!(o.contains("Only the person can switch it on again"), "{o}");
    assert!(!env.assist_store().global().enabled);
    assert!(call(&env, "flow_assist_stop_all", json!({})).unwrap().contains("already stopped"));
    // there is no tool argument or second tool that switches it on again
    assert!(!super::tools::defs(&policy(&["*"])).iter().any(|d| d.spec.name.contains("assist") && d.spec.schema.to_string().contains("enable")));
    assert_eq!(audits.lock().unwrap().iter().filter(|l| l.contains("flow.assist.stop_all")).count(), 1);
    env.assist_store().set_enabled(true).unwrap();
}

#[test]
fn the_message_policy_and_assist_settings_tools_set_validate_audit_and_show() {
    let (env, _d) = env_with("tools9", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |u, a, d| a2.lock().unwrap().push(format!("{u} {a} {d}"))));
    let shown = call(&env, "flow_outside_messages_policy", json!({})).unwrap();
    assert!(shown.contains("no quiet hours") && shown.contains("no escalation"), "{shown}");
    let o = call(&env, "flow_outside_messages_policy", json!({"quiet_start": "22:00", "quiet_end": "07:30", "escalate_minutes": 30, "escalate_room": "ops"})).unwrap();
    assert!(o.contains("quiet hours 22:00 to 07:30") && o.contains("after 30 min") && o.contains("ops"), "{o}");
    assert!(call(&env, "flow_outside_messages_policy", json!({"quiet_start": "25:00", "quiet_end": "07:30"})).is_err());
    assert!(call(&env, "flow_outside_messages_policy", json!({"escalate_minutes": 0})).unwrap().contains("no escalation"));
    assert!(call(&env, "flow_outside_messages_policy", json!({"quiet_off": true})).unwrap().contains("no quiet hours"));
    assert!(audits.lock().unwrap().iter().any(|l| l.contains("flow.outside_messages.policy")));
    // settings of the fixing agent
    let o = call(&env, "flow_assist_settings", json!({"matrix": true, "matrix_room": "ops", "models": [{"name": "lan/small", "local": true, "timeout_seconds": 30}, {"name": "cloud/big"}], "fallback": true})).unwrap();
    assert!(o.contains("Matrix notice on") && o.contains("room ops") && o.contains("lan/small (local) 30s") && o.contains("cloud/big"), "{o}");
    let g = env.assist_store().global();
    assert!(g.matrix && g.profile.models.len() == 2 && g.profile.fallback);
    assert!(call(&env, "flow_assist_settings", json!({"models": [{"name": ""}]})).is_err());
    assert!(call(&env, "flow_assist_settings", json!({})).unwrap().contains("Matrix notice on"), "no arguments only shows");
    assert!(audits.lock().unwrap().iter().any(|l| l.contains("flow.assist.settings")));
}

#[test]
fn runs_are_listed_explained_and_logged_without_secrets() {
    let (env, _d) = env_with("tools6", policy(&["127.0.0.1"]));
    let (port, _) = serve(vec![http_resp(500, &[], "the server fell over; token=sk-ABCDEFGHIJKLMNOPQRSTUVWX"), http_resp(500, &[], "again")], 0);
    let mut spec = spec();
    spec["nodes"][0]["config"] = json!({"url": format!("http://127.0.0.1:{port}/h")});
    spec["nodes"][0]["retry"] = json!({"attempts": 2, "backoff": {"kind": "fixed", "delay_ms": 5}});
    spec["triggers"] = json!([]);
    call(&env, "flow_create", spec).unwrap();
    call(&env, "flow_publish", json!({"flow": "check-the-shop", "changelog": "v1"})).unwrap();
    let o = call(&env, "flow_run", json!({"flow": "check-the-shop", "key": "k1"})).unwrap();
    let run_id = o.split_whitespace().nth(2).unwrap().trim_end_matches('.').to_string();
    assert!(o.starts_with("Started run"), "{o}");
    assert!(call(&env, "flow_run", json!({"flow": "check-the-shop", "key": "k1"})).unwrap().contains("already exists"));
    env.engine.wait_for(&run_id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
    let runs = call(&env, "flow_runs", json!({"status": "failed"})).unwrap();
    assert!(runs.contains(&run_id) && runs.contains("failed"), "{runs}");
    assert_eq!(call(&env, "flow_runs", json!({"status": "succeeded"})).unwrap(), "No runs match.");
    let st = call(&env, "flow_run_status", json!({"run": run_id})).unwrap();
    assert!(st.contains("ping [http] failed attempts 2/2"), "{st}");
    let ex = call(&env, "flow_explain_failure", json!({"run": run_id})).unwrap();
    assert!(ex.contains("attempt 1") && ex.contains("attempt 2") && ex.contains("retryable") && ex.contains("HTTP 5") && ex.contains("recover"), "{ex}");
    assert!(!ex.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWX"), "a credential in a server's answer must not reach the model: {ex}");
    let logs = call(&env, "flow_logs", json!({"run": run_id, "node": "ping", "tail": 5})).unwrap();
    assert!(logs.contains("attempt"), "{logs}");
    assert!(call(&env, "flow_logs", json!({"run": "r999"})).is_err());
    // cancel something that is over
    assert!(call(&env, "flow_cancel", json!({"run": run_id})).unwrap_err().contains("already"));
    // the context for the tile's chat: compact, with the open flow, never a secret
    let open = Open { flow: Some("check-the-shop".into()), node: Some("ping".into()), run: Some(run_id.clone()), tab: "design".into() };
    let ctx = context_text(&env, &open);
    assert!(ctx.len() <= 5200 && ctx.contains("selected node ping") && ctx.contains("SELECTED RUN") && ctx.contains("check-the-shop"), "{ctx}");
    assert!(!ctx.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWX"));
}

#[test]
fn a_flow_run_can_be_scheduled_and_the_when_text_is_understood() {
    assert_eq!(parse_when("in 10m", 1000).unwrap(), 1000 + 600_000);
    assert_eq!(parse_when("+2h", 0).unwrap(), 7_200_000);
    assert_eq!(parse_when("in 3 days", 0).unwrap(), 3 * 86_400_000);
    assert_eq!(parse_when("now", 5).unwrap(), 5);
    assert!(parse_when("in 5 fortnights", 0).is_err() && parse_when("whenever", 0).is_err());
    assert!(parse_when("2030-01-02 03:04", 0).unwrap() > 1_800_000_000_000);
    let (env, _d) = env_with("tools7", policy(&["*"]));
    call(&env, "flow_create", json!({"name": "Later", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    call(&env, "flow_publish", json!({"flow": "later", "changelog": "v1"})).unwrap();
    let o = call(&env, "flow_run", json!({"flow": "later", "at": "in 1h"})).unwrap();
    assert!(o.starts_with("Scheduled run"), "{o}");
    assert_eq!(env.engine.counts().scheduled, 1);
    let id = o.split_whitespace().nth(2).unwrap().trim_end_matches('.').to_string();
    call(&env, "flow_cancel", json!({"run": id})).unwrap();
    assert_eq!(env.engine.counts().scheduled, 0);
    // an unpublished flow cannot be run on a schedule, but its draft can be tried
    call(&env, "flow_create", json!({"name": "Draft only", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    assert!(call(&env, "flow_run", json!({"flow": "draft-only"})).unwrap_err().contains("no published version"));
    assert!(call(&env, "flow_run", json!({"flow": "draft-only", "draft": true})).unwrap().starts_with("Started run"));
}

#[test]
fn building_and_editing_flows_from_data_is_checked() {
    assert!(build_flow(&json!({})).is_err());
    assert!(build_flow(&json!({"name": "x"})).unwrap_err().contains("at least one node"));
    assert!(build_flow(&json!({"name": "x", "nodes": [{"kind": "notify"}]})).is_err());
    assert!(build_flow(&json!({"name": "x", "nodes": [{"id": "a", "kind": "notify"}], "triggers": [{"kind": "hourly"}]})).unwrap_err().contains("trigger kind"));
    let f = build_flow(&json!({"name": "Two", "nodes": [{"id": "a", "kind": "notify", "config": {"title": "a"}}, {"id": "b", "kind": "notify", "config": {"title": "b"}}], "edges": [{"from": "a", "to": "b"}], "triggers": [{"kind": "cron", "expr": "0 8 * * *", "tz": "+05:30"}, {"kind": "interval", "every_minutes": 5}]})).unwrap();
    assert_eq!(f.nodes.len(), 3);
    assert_eq!(f.edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect::<Vec<_>>(), vec![("a", "b"), ("start", "a")], "the start feeds what nothing else feeds");
    assert!(matches!(&f.triggers[1].kind, TriggerKind::Interval { every_s: 300, .. }));
    let mut g = f.clone();
    assert!(apply_ops(&mut g, &[json!({"op": "add_edge", "from": "a", "to": "zzz"})]).is_err());
    assert!(apply_ops(&mut g, &[json!({"op": "teleport"})]).unwrap_err().contains("is not an operation"));
    assert!(apply_ops(&mut g, &[json!({"op": "add_node", "id": "a", "kind": "notify"})]).unwrap_err().contains("already a node"));
    apply_ops(&mut g, &[json!({"op": "remove_node", "id": "b"}), json!({"op": "remove_trigger", "id": "interval"}), json!({"op": "set_default", "timeout_ms": 5000, "retry": {"attempts": 2}}), json!({"op": "set_input", "name": "who", "default": "me"}), json!({"op": "set_meta", "name": "Renamed"})]).unwrap();
    assert!(g.node("b").is_none() && g.edges.iter().all(|e| e.to != "b"));
    assert_eq!((g.defaults.timeout_ms, g.defaults.retry.attempts, g.name.as_str(), g.inputs.len(), g.triggers.len()), (5000, 2, "Renamed", 1, 1));
}

#[test]
fn what_the_model_is_told_hides_credentials() {
    let mut f = Flow::new("c", "C");
    f.nodes.push(Node::new("a", "http", json!({"url": "https://x.example", "headers": {"X-Key": "sk-ABCDEFGHIJKLMNOPQRSTUVWX", "Authorization": "Bearer {{secret.t}}"}, "password": "hunter2"})));
    let b = flow_brief(&f, true);
    assert!(!b.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWX") && !b.contains("hunter2"), "{b}");
    assert!(b.contains("{{secret.t}}"), "references are fine: {b}");
    assert_ne!(chat_key("amy"), chat_key("bob"));
    assert!(chat_key("amy").starts_with("flows_"));
}

#[test]
fn an_imported_flow_is_a_safe_draft() {
    let (env, _d) = env_with("import", policy(&["*"]));
    let mut f = Flow::new("imp", "Imported");
    f.production = true;
    f.nodes.push(Node::new("s", "start", json!({})));
    f.defaults.heal.mode = HealMode::AutoFixSimple;
    f.defaults.heal.auto_publish_simple = true;
    f.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
    f.meta.version = 7;
    let got = env.import_json(&f.to_json(), "user:me").unwrap();
    assert!(!got.production && got.defaults.heal.mode == HealMode::Suggest && !got.defaults.heal.auto_publish_simple);
    assert!(got.triggers.iter().all(|t| !t.enabled) && got.meta.version == 0);
    assert!(env.import_json("{not json", "user:me").is_err());
    let back = env.export_json("imp").unwrap();
    assert!(Flow::from_json(&back).is_ok());
    // a second import of the same id gets a free one
    assert_eq!(env.import_json(&f.to_json(), "user:me").unwrap().id, "imp-2");
}

#[test]
fn the_tools_are_wrapped_for_the_tile_chat_and_the_hard_ask_ones_are_never_approved_for_good() {
    let (env, _d) = env_with("wrap", policy(&["*"]));
    struct Inner;
    impl Tools for Inner {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _a: &Value) -> Result<String, String> {
            Ok(format!("inner {n}"))
        }
        fn describe(&self, n: &str, _a: &Value) -> String {
            n.to_string()
        }
    }
    let open: SharedOpen = Arc::new(Mutex::new(Open::default()));
    let t = FlowTools::new(Arc::new(Inner), env.clone(), open.clone());
    assert_eq!(t.defs().len(), 36, "35 flow tools (with the webhook tools) and ask_form");
    assert_eq!(t.call("something_else", &json!({})).unwrap(), "inner something_else");
    // with a flow open, `flow` may be left out
    call(&env, "flow_create", json!({"name": "Open one", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    assert!(t.call("flow_get", &json!({})).unwrap_err().contains("give `flow`"));
    open.lock().unwrap().flow = Some("open-one".into());
    assert!(t.call("flow_get", &json!({})).unwrap().contains("Open one"));
    t.call("flow_edit", &json!({"ops": [{"op": "update_node", "id": "n", "config": {"title": "changed"}}]})).unwrap();
    assert_eq!(env.store.get_draft("open-one").unwrap().node("n").unwrap().config["title"], "changed");
    for n in ["flow_publish", "flow_rollback", "flow_apply_to_runs", "flow_set_heal_policy"] {
        assert!(!t.approved_always(n), "{n} is decided each time");
    }
    assert!(t.approved_always("flow_edit"));
    assert!(t.describe("flow_publish", &json!({"changelog": "x"})).contains("PUBLISH"));
    assert!(t.describe("flow_run", &json!({"flow": "a", "at": "in 5m"})).contains("in 5m"));
    assert!(PROMPT_RULES.contains("Never publish by yourself"));
    assert!(FlowTools::context(&env, &open).contains("open-one"));
}

#[test]
fn a_store_template_becomes_a_safe_draft() {
    let (env, _d) = env_with("store-tpl", policy(&["*"]));
    let mut t = templates::get("health-check").unwrap();
    t.flow.production = true;
    t.flow.defaults.heal.mode = HealMode::AutoFixSimple;
    let f = env.from_store_template(&serde_json::to_string(&t).unwrap(), "user:me").unwrap();
    assert!(!f.production && f.defaults.heal.mode == HealMode::Suggest && f.triggers.iter().all(|x| !x.enabled));
    assert!(env.from_store_template("nonsense", "user:me").is_err());
    assert!(crate::store::manifest::EXT_TARGETS.contains(&"flows"));
}

#[test]
fn commands_follow_an_allowlist_that_an_admin_can_extend() {
    let (env, r, _d) = fake_runner_env("allow");
    r.answer("terraform", "ok");
    let e = env.host.exec(&exec_req(&["terraform", "plan"], ""), &Cancel::new()).unwrap_err();
    assert!(e.config && e.message.contains(super::programs::NOT_ALLOWED), "{e:?}");
    assert!(r.calls().is_empty(), "nothing was started");
    // a user without admin rights cannot allow it; an admin can, and it is remembered
    let mut p = env.policy.clone();
    p.manage_settings = false;
    let user = FlowEnv { policy: p, user: env.user.clone(), svc: env.svc.clone(), store: env.store.clone(), engine: env.engine.clone(), host: env.host.clone(), reg: env.reg.clone(), dir: env.dir.clone(), data: env.data.clone() };
    assert!(user.allow_program("terraform").is_err());
    let mut ap = env.policy.clone();
    ap.manage_settings = true;
    let admin = FlowEnv { policy: ap, user: env.user.clone(), svc: env.svc.clone(), store: env.store.clone(), engine: env.engine.clone(), host: env.host.clone(), reg: env.reg.clone(), dir: env.dir.clone(), data: env.data.clone() };
    assert!(admin.allow_program("sudo").is_err());
    admin.allow_program("terraform").unwrap();
    assert!(env.host.exec(&exec_req(&["terraform", "plan"], ""), &Cancel::new()).is_ok());
    // still refused whatever the list says
    admin.allow_program("rm").unwrap();
    assert!(env.host.exec(&exec_req(&["rm", "-rf", "/"], ""), &Cancel::new()).is_err());
    assert!(env.host.exec(&exec_req(&["rm", "-rf", "build"], ""), &Cancel::new()).is_ok());
}

#[test]
fn the_sandbox_namespace_is_the_flows_own_or_a_workspaces_dev_one() {
    let (env, _d) = env_with("ns", policy(&["*"]));
    let own = format!("flows-{}", env.user);
    assert_eq!(env.host.sandbox_namespace("").unwrap(), own);
    assert_eq!(env.host.sandbox_namespace("workspace:Shop").unwrap(), "shop-dev");
    assert_eq!(env.host.sandbox_namespace("billing-dev").unwrap(), "billing-dev");
    for bad in ["kube-system", "default", "prod", "Bad_Name", "workspace:", "-dev-"] {
        assert!(env.host.sandbox_namespace(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_scheduler_starts_at_sign_in_only_for_accounts_with_flows_and_stops_at_sign_out() {
    let (env, d) = env_with("login", policy(&["*"]));
    let svc = env.svc.clone();
    let p = env.policy.clone();
    // the hub keys on the services folder, so this account is its own
    let user = "rootlogin";
    let flows = crate::scope::home_root_for_writes(user, svc.dir()).join("Flows");
    let _ = std::fs::remove_dir_all(&flows);
    assert!(!FlowEnv::start_for_login(&svc, user, &p), "no flows, nothing started");
    assert!(!FlowEnv::scheduler_running(&svc, user));
    let f = {
        let tmp = FlowEnv::build_in(&svc, user, &p, false, flows.parent().unwrap());
        tmp.create_draft(build_flow(&json!({"name": "A", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}]})).unwrap(), "user:t").unwrap()
    };
    assert!(f.id == "a");
    assert!(FlowEnv::start_for_login(&svc, user, &p));
    assert!(FlowEnv::scheduler_running(&svc, user));
    FlowEnv::stop_for_logout(&svc, user);
    assert!(!FlowEnv::scheduler_running(&svc, user));
    let mut denied = p.clone();
    denied.apps.remove(&iyakki_sec::policy::App::Flows);
    assert!(!FlowEnv::start_for_login(&svc, user, &denied));
    let _ = d;
}

#[test]
fn the_flows_chat_leads_with_flow_tools_hides_app_creation_and_asks_for_missing_input_with_a_form() {
    let (env, _d) = env_with("formwrap", policy(&["*"]));
    struct Inner;
    impl Tools for Inner {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            ["list_dir", "app_scaffold", "tile_scaffold", "tile_pack", "tile_ship", "ask_user"]
                .iter()
                .map(|n| crate::agent::ToolDef { spec: crate::chat::ToolSpec { name: (*n).into(), description: String::new(), schema: json!({"type": "object"}) }, risk: crate::agent::Risk::Safe, untrusted: false })
                .collect()
        }
        fn call(&self, n: &str, _a: &Value) -> Result<String, String> {
            Ok(format!("inner {n}"))
        }
        fn describe(&self, n: &str, _a: &Value) -> String {
            n.to_string()
        }
    }
    let open: SharedOpen = Arc::new(Mutex::new(Open::default()));
    let t = FlowTools::new(Arc::new(Inner), env.clone(), open);
    let names: Vec<String> = t.defs().into_iter().map(|d| d.spec.name).collect();
    assert_eq!(names[0], "flow_list");
    assert!(names[..17].iter().all(|n| n.starts_with("flow_")));
    assert!(names.contains(&"ask_form".to_string()) && names.contains(&"list_dir".to_string()) && names.contains(&"ask_user".to_string()));
    for n in ["app_scaffold", "tile_scaffold", "tile_pack", "tile_ship"] {
        assert!(!names.contains(&n.to_string()), "{n} is not offered in FLOWS");
        assert!(t.call(n, &json!({})).unwrap_err().contains("Make a flow"));
    }
    assert!(PROMPT_RULES.contains("a FLOW (flow_create)") && PROMPT_RULES.contains("never an IYAKKI tile app") && PROMPT_RULES.contains("ask_form"));
    // ask_form validates, and a bad spec is an error the model can fix
    assert!(t.call("ask_form", &json!({"title": "x", "fields": [{"name": "a"}]})).unwrap().contains("Stop now"));
    assert!(t.call("ask_form", &json!({"fields": []})).is_err());
    assert_eq!(crate::agent::Risk::Safe, t.defs().into_iter().find(|d| d.spec.name == "ask_form").unwrap().risk);
    // flow_run names the missing inputs and hands over ready fields for the form
    call(&env, "flow_create", json!({"name": "Needs input", "inputs": [{"name": "target", "type": "string", "required": true}, {"name": "key", "type": "secret", "required": true}], "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    let e = t.call("flow_run", &json!({"flow": "needs-input", "draft": true, "inputs": {"target": "x"}})).unwrap_err();
    assert!(e.contains("ask_form") && e.contains("\"key\"") && !e.contains("\"target\""), "{e}");
}

#[test]
fn deleting_a_flow_and_dropping_a_version_are_hard_ask_named_audited_and_refused_when_unsafe() {
    let (env, _d) = env_with("deldrop", policy(&["*"]));
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    *env.host.audit_sink.lock().unwrap() = Some(Arc::new(move |_, a, d| a2.lock().unwrap().push(format!("{a} {d}"))));
    call(&env, "flow_create", spec()).unwrap();
    call(&env, "flow_publish", json!({"flow": "check-the-shop", "changelog": "one"})).unwrap();
    call(&env, "flow_edit", json!({"flow": "check-the-shop", "ops": [{"op": "update_node", "id": "ping", "timeout_ms": 9}]})).unwrap();
    call(&env, "flow_publish", json!({"flow": "check-the-shop", "changelog": "two"})).unwrap();
    // the card names the flow and what is lost
    let p = env.preview("flow_drop_version", &json!({"flow": "check-the-shop", "version": 1}), None).unwrap();
    assert!(p.contains("Check the shop") && p.contains("version 1 is lost"), "{p}");
    assert!(describe("flow_delete", &json!({"flow": "check-the-shop"})).contains("DELETE"));
    assert!(crate::agent::hard_ask("flow_delete", &json!({})).is_some() && crate::agent::hard_ask("flow_drop_version", &json!({})).is_some());
    // the current version is never dropped, and that is said
    let e = call(&env, "flow_drop_version", json!({"flow": "check-the-shop", "version": 2})).unwrap_err();
    assert!(e.contains("current published version"), "{e}");
    assert!(call(&env, "flow_drop_version", json!({"flow": "check-the-shop", "version": 9})).is_err());
    // an older one goes
    call(&env, "flow_drop_version", json!({"flow": "check-the-shop", "version": 1})).unwrap();
    assert!(env.store.get_version("check-the-shop", 1).is_none() && env.store.get_version("check-the-shop", 2).is_some());
    assert_eq!(env.store.versions("check-the-shop").len(), 1);
    assert!(audits.lock().unwrap().iter().any(|l| l.starts_with("flow.version.drop check-the-shop v1")), "{:?}", audits.lock().unwrap());
    let e = call(&env, "flow_drop_version", json!({"flow": "check-the-shop", "version": 2})).unwrap_err();
    assert!(e.contains("only version"), "{e}");
    // a flow with an unfinished run is not deleted
    let o = call(&env, "flow_run", json!({"flow": "check-the-shop"})).unwrap();
    assert!(o.contains("run"), "{o}");
    let e = call(&env, "flow_delete", json!({"flow": "check-the-shop"})).unwrap_err();
    assert!(e.contains("not finished"), "{e}");
    assert!(env.store.exists("check-the-shop"));
    let rid = env.engine.summaries()[0].id.clone();
    env.engine.cancel(&rid).unwrap();
    assert!(!audits.lock().unwrap().iter().any(|l| l.starts_with("flow.delete")), "a refused delete is not audited as done");
    // the cancel is handled by the engine's own thread: wait until the delete is no longer refused
    let t0 = std::time::Instant::now();
    loop {
        match call(&env, "flow_delete", json!({"flow": "check-the-shop"})) {
            Ok(_) => break,
            Err(e) => assert!(e.contains("not finished") && t0.elapsed().as_secs() < 10, "{e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!env.store.exists("check-the-shop"));
    let a = audits.lock().unwrap().clone();
    assert!(a.iter().any(|l| l.starts_with("flow.delete check-the-shop") && l.contains("Check the shop")), "{a:?}");
}

#[test]
fn with_a_flow_open_an_update_edits_that_flow_and_shows_what_changed_and_with_none_open_it_must_create() {
    let (env, _d) = env_with("openedit", policy(&["*"]));
    call(&env, "flow_create", json!({"name": "Alpha", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "a"}}]})).unwrap();
    call(&env, "flow_create", json!({"name": "Beta", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "b"}}]})).unwrap();
    struct Inner;
    impl Tools for Inner {
        fn defs(&self) -> Vec<crate::agent::ToolDef> { vec![] }
        fn call(&self, n: &str, _a: &Value) -> Result<String, String> { Ok(n.into()) }
        fn describe(&self, n: &str, _a: &Value) -> String { n.into() }
    }
    let open: SharedOpen = Arc::new(Mutex::new(Open::default()));
    let t = FlowTools::new(Arc::new(Inner), env.clone(), open.clone());
    let ops = json!({"ops": [{"op": "add_node", "id": "extra", "kind": "notify", "config": {"title": "new"}, "after": "n"}]});
    // none open: the edit needs a flow id (the model creates instead)
    assert!(t.call("flow_edit", &ops).unwrap_err().contains("give `flow`"));
    open.lock().unwrap().flow = Some("beta".into());
    let out = t.call("flow_edit", &ops).unwrap();
    assert!(out.contains("What changed") && out.contains("extra"), "{out}");
    assert!(env.store.get_draft("beta").unwrap().node("extra").is_some());
    assert!(env.store.get_draft("alpha").unwrap().node("extra").is_none(), "the other flow is untouched");
    assert!(PROMPT_RULES.contains("apply to THAT flow"));
}

// ------------------------------------------------------------------------------------------------ the open flow and its failure

fn failed_flow(env: &FlowEnv, name: &str, token: &str) -> (String, String) {
    let spec = json!({"name": name, "inputs": [{"name": "api", "type": "text", "required": true}], "nodes": [{"id": "call", "kind": "http", "name": "Call the site", "config": {"url": "http://127.0.0.1:1/x"}, "retry": {"attempts": 2}}], "edges": [{"from": "start", "to": "call"}], "triggers": [{"kind": "interval", "every_s": 3600}]});
    let flow = env.create_draft(build_flow(&spec).unwrap(), "user:root").unwrap();
    env.store.publish(&flow.id, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "v1", approved_by: Some("user:root") }).unwrap();
    let mut req = iyakki_flow::StartRequest::new(&flow.id).trigger("manual", "me");
    req.inputs = json!({"api": token});
    let run = env.engine.start_run(req).unwrap().run_id;
    env.engine.wait_for(&run, Duration::from_secs(20), |r| r.status.is_terminal()).unwrap();
    (flow.id, run)
}

#[test]
fn the_context_names_the_open_flow_its_state_inputs_runs_and_the_newest_failure_without_values() {
    let (env, _d) = env_with("ctx-fail", policy(&["*"]));
    let token = format!("{}{}", "ghp_", "Z9y8X7w6V5u4T3s2R1q0P9o8N7m6L5k4J3i2");
    let (flow, run) = failed_flow(&env, "Nightly check", &token);
    let open = Open { flow: Some(flow.clone()), tab: "design".into(), ..Default::default() };
    let c = context_text(&env, &open);
    assert!(c.contains(&format!("OPEN ON SCREEN: flow `{flow}` \"Nightly check\", published as version 1")), "{c}");
    assert!(c.contains("node call [http]") && c.contains("inputs (names only): api (required)") && c.contains("trigger"), "{c}");
    assert!(c.contains("RUNS OF THIS FLOW") && c.contains(&format!("NEWEST FAILED RUN {run}")), "{c}");
    assert!(c.contains("failed step `call` (Call the site) [http], attempt 2 of 2"), "{c}");
    assert!(c.contains("run inputs (names only): api"), "{c}");
    assert!(!c.contains(&token), "the input value never reaches the model");
    assert!(c.chars().count() <= 5200, "bounded: {}", c.chars().count());
    assert!(last_run_failed(&env, &flow) && newest_failed_run(&env, &flow).as_deref() == Some(run.as_str()));
    // without a flow open, no failure is described
    let none = context_text(&env, &Open { tab: "board".into(), ..Default::default() });
    assert!(!none.contains("NEWEST FAILED RUN") && none.contains("ALL FLOWS"));
}

#[test]
fn the_last_open_flow_is_kept_per_person_bounded_and_cleared_on_delete_and_sign_out() {
    let (env, _d) = env_with("last-flow", policy(&["*"]));
    assert!(!last_flow_recorded(&env.svc, &env.user) && last_flow(&env).is_none());
    call(&env, "flow_create", json!({"name": "Keep me", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    // bad ids are refused, long names are cut to one line
    set_last_flow(&env.svc, &env.user, "../x", "bad");
    assert!(!last_flow_recorded(&env.svc, &env.user));
    set_last_flow(&env.svc, &env.user, "keep-me", &format!("Keep\nme{}", "x".repeat(300)));
    let (id, name) = last_flow(&env).unwrap();
    assert_eq!(id, "keep-me");
    assert!(name.chars().count() <= 80 && !name.contains('\n'));
    // another person has none
    assert!(!last_flow_recorded(&env.svc, "someone-else"));
    // deleting the flow clears it
    env.delete_flow("keep-me").unwrap();
    assert!(last_flow(&env).is_none() && !last_flow_recorded(&env.svc, &env.user));
    // sign-out clears it
    call(&env, "flow_create", json!({"name": "Again", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    set_last_flow(&env.svc, &env.user, "again", "Again");
    assert!(last_flow(&env).is_some());
    FlowEnv::stop_for_logout(&env.svc, &env.user);
    assert!(!last_flow_recorded(&env.svc, &env.user));
}

#[test]
fn the_last_open_flow_ends_after_four_hours_and_not_a_minute_before() {
    let (env, _d) = env_with("last-flow-ttl", policy(&["*"]));
    assert_eq!(LAST_FLOW_TTL_SECS, 4 * 3600);
    call(&env, "flow_create", json!({"name": "Old one", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    let key = last_key(&env.svc, &env.user);
    let age = |secs: i64| {
        set_last_flow(&env.svc, &env.user, "old-one", "Old one");
        let mut m = last_slot().lock().unwrap_or_else(|e| e.into_inner());
        m.get_mut(&key).unwrap().2 = crate::clock::now_unix() - secs;
    };
    // a minute short of four hours: still attached
    age(LAST_FLOW_TTL_SECS - 60);
    assert_eq!(last_flow(&env).map(|(id, _)| id).as_deref(), Some("old-one"));
    // a minute over: gone, and the record is dropped so the chat shows no chip
    age(LAST_FLOW_TTL_SECS + 60);
    assert!(last_flow(&env).is_none());
    assert!(!last_flow_recorded(&env.svc, &env.user));
}

#[test]
fn pruning_the_history_by_the_tool_lists_first_asks_always_and_spares_the_newest_and_the_unfinished() {
    let (env, _d) = env_with("prunehist", policy(&["*"]));
    call(&env, "flow_create", json!({"name": "Only tell", "nodes": [{"id": "tell", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    call(&env, "flow_publish", json!({"flow": "only-tell", "changelog": "one"})).unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let o = env.engine.start_run(iyakki_flow::engine::StartRequest::new("only-tell")).unwrap();
        env.engine.wait_for(&o.run_id, Duration::from_secs(10), |r| r.status.is_terminal()).unwrap();
        ids.push(o.run_id);
    }
    // always asked, with the list on the card
    assert!(crate::agent::hard_ask("flow_prune_history", &json!({})).is_some());
    assert!(describe("flow_prune_history", &json!({"keep": 2})).contains("REMOVE"));
    let card = env.preview("flow_prune_history", &json!({"keep": 2, "days": 0}), None).unwrap();
    assert!(card.contains("Would remove 2 run(s)") && card.contains(&ids[0]) && !card.contains(&ids[3]), "{card}");
    // the dry run removes nothing
    let dry = call(&env, "flow_prune_history", json!({"keep": 2, "days": 0, "dry_run": true})).unwrap();
    assert!(dry.contains("nothing is removed yet"), "{dry}");
    assert_eq!(env.engine.summaries().len(), 4);
    // the real call removes exactly the listed runs and is audited
    let done = call(&env, "flow_prune_history", json!({"keep": 2, "days": 0})).unwrap();
    assert!(done.starts_with("Removed 2 run(s)"), "{done}");
    let left: Vec<String> = env.engine.summaries().into_iter().map(|s| s.id).collect();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&ids[2]) && left.contains(&ids[3]));
    // the newest finished run stays even for keep 1 and days 1 with nothing older than a day
    let again = call(&env, "flow_prune_history", json!({"keep": 1, "days": 1})).unwrap();
    assert!(again.starts_with("Removed 1 run(s)"), "{again}");
    assert_eq!(env.engine.summaries().len(), 1);
    assert!(call(&env, "flow_prune_history", json!({"keep": 1, "days": 1})).unwrap().contains("nothing to remove"));
}

#[test]
fn saved_machines_are_managed_by_chat_with_a_vault_name_only_and_a_flow_runs_there_through_the_real_transport() {
    let (env, _d) = env_with("rmach", policy(&["127.0.0.1", "*"]));
    assert!(call(&env, "remote_machines", json!({})).unwrap().contains("No machines"));
    // the token has to be in the vault first, and only its name is given
    let e = call(&env, "remote_machine_add", json!({"name": "lab", "url": "http://127.0.0.1:9", "token_vault": "lab-token"})).unwrap_err();
    assert!(e.contains("no such entry"), "{e}");
    let e = call(&env, "remote_machine_add", json!({"name": "lab", "url": "http://127.0.0.1:9", "token_vault": "a pasted token value!"})).unwrap_err();
    assert!(!e.contains("pasted"), "{e}");
    env.svc.on_login(&env.user, "a-long-test-passphrase", true);
    env.svc.user_set(&env.user, "lab-token", "tok-secret-12345").unwrap();
    // the other machine: a real socket that answers the two calls
    let (port, seen) = serve(vec![http_resp(200, &[("Content-Type", "application/json")], r#"{"run":"r5","flow":"nightly"}"#), http_resp(200, &[("Content-Type", "application/json")], r#"{"status":"succeeded","nodes":[{"node":"a","state":"succeeded"}]}"#)], 0);
    let url = format!("http://127.0.0.1:{port}");
    let ok = call(&env, "remote_machine_add", json!({"name": "lab", "url": url, "token_vault": "uvault:lab-token"})).unwrap();
    assert!(ok.contains("Saved") && !ok.contains("tok-secret"), "{ok}");
    assert!(call(&env, "remote_machines", json!({})).unwrap().contains(&format!("lab {url}")));
    let raw = std::fs::read_to_string(env.data.join("remote-machines.json")).unwrap();
    assert!(!raw.contains("tok-secret"), "the file holds the vault name only: {raw}");
    let r = call(&env, "remote_flow_run", json!({"machine": "lab", "flow": "nightly", "inputs": {"x": 1}, "wait_s": 20})).unwrap();
    assert!(r.contains("Run r5 on lab: succeeded") && r.contains("a: succeeded"), "{r}");
    let sent = seen.lock().unwrap().clone();
    assert!(sent[0].starts_with("POST /api/v1/flows/nightly/run ") && sent[0].to_ascii_lowercase().contains("authorization: bearer tok-secret-12345"), "{}", sent[0]);
    assert!(sent[1].starts_with("GET /api/v1/flows/nightly/runs/r5 "), "{}", sent[1]);
    assert!(!r.contains("tok-secret"));
    // an unknown name is refused before anything is sent, and a machine can be forgotten
    assert!(call(&env, "remote_flow_run", json!({"machine": "nope", "flow": "n"})).unwrap_err().contains("no saved machine"));
    assert!(call(&env, "remote_machine_remove", json!({"name": "lab"})).unwrap().contains("Forgot"));
    assert!(call(&env, "remote_machine_remove", json!({"name": "lab"})).unwrap().contains("No machine"));
    // every one of these asks first except the list
    let d = defs(&policy(&["*"]));
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).unwrap().risk;
    assert_eq!(risk("remote_machines"), Risk::Safe);
    for n in ["remote_machine_add", "remote_machine_remove", "remote_flow_run"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    // a host the account may not reach is refused
    let (env2, _d2) = env_with("rmach2", policy(&["example.org"]));
    env2.svc.on_login(&env2.user, "a-long-test-passphrase", true);
    env2.svc.user_set(&env2.user, "t", "tok-secret-12345").unwrap();
    call(&env2, "remote_machine_add", json!({"name": "far", "url": "http://127.0.0.1:9", "token_vault": "t"})).unwrap();
    assert!(call(&env2, "remote_flow_run", json!({"machine": "far", "flow": "n"})).unwrap_err().contains("may not reach"));
}

// ---- the flows runner's import, validate and stop over the real flow environment (F-WEBSVC-12)

mod real_runner {
    use super::*;
    use crate::webservice::real::assemble;
    use iyakki_core_api::{ApiError, AuditSink, Authenticator, Principal};

    struct NoAuth;
    impl Authenticator for NoAuth {
        fn login(&self, _: &str, _: &str) -> iyakki_core_api::ApiResult<Principal> {
            Err(ApiError::Denied("no".into()))
        }
        fn lookup(&self, _: &str) -> Option<Principal> {
            None
        }
    }
    struct NoAudit;
    impl AuditSink for NoAudit {
        fn record(&self, _: &str, _: &str, _: &str) {}
    }

    fn principal(user: &str) -> Principal {
        Principal { user: user.into(), role: "user".into(), admin: false, policy: Role::User.template() }
    }

    #[test]
    fn a_flow_is_validated_imported_as_a_safe_draft_run_and_stopped_through_the_real_extras() {
        let d = tmp("runner-real");
        let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        let deps = assemble(Arc::new(NoAuth), Arc::new(NoAudit), svc, Arc::new(crate::cli::tests::MockHost::default()));
        let x = &deps.extras;
        let p = principal("runnerann");
        // a flow without a start step is an error; one with a start and an end is fine
        let mut f = Flow::new("imp", "Imported");
        f.production = true;
        f.triggers.push(Trigger::new("t", TriggerKind::Interval { every_s: 60, anchor_ms: None }));
        let broken = serde_json::from_str::<serde_json::Value>(&f.to_json()).unwrap();
        let issues = x.flow_validate(&p, None, Some(&broken)).unwrap();
        assert!(issues.iter().any(|i| i["level"] == "error"), "{issues:?}");
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("w", "delay", json!({"seconds": 30})));
        f.nodes.push(Node::new("e", "end", json!({})));
        f.edges.push(Edge::new("s", "w"));
        f.edges.push(Edge::new("w", "e"));
        let good = serde_json::from_str::<serde_json::Value>(&f.to_json()).unwrap();
        assert!(!x.flow_validate(&p, None, Some(&good)).unwrap().iter().any(|i| i["level"] == "error"));
        assert!(x.flow_validate(&p, None, Some(&json!("not a flow"))).is_err());
        assert!(matches!(x.flow_validate(&p, Some("nope"), None), Err(ApiError::NotFound(_))));
        // import: a safe draft, a second import gets a free id, and it is the person's own (another person has none)
        let a = x.flow_import(&p, &good).unwrap();
        assert_eq!(a["id"], "imp");
        assert_eq!(x.flow_import(&p, &good).unwrap()["id"], "imp-2");
        assert!(x.flow_import(&p, &json!({"nodes": 5})).is_err());
        assert!(x.flows(&p).unwrap().iter().any(|v| v["id"] == "imp"));
        assert!(x.flows(&principal("runnerbob")).unwrap().is_empty());
        let back = x.flow_export(&p, "imp").unwrap();
        assert_eq!(back["production"], false, "an import is never production");
        assert!(back["triggers"].as_array().unwrap().iter().all(|t| t["enabled"] == false), "its triggers are off");
        assert!(x.flow_validate(&p, Some("imp"), None).is_ok());
        // run the draft (a 30 second wait), stop it, and the run ends cancelled; another person cannot stop it
        let run = x.flow_run(&p, "imp", true, &json!({})).unwrap();
        assert!(matches!(x.flow_run_stop(&principal("runnerbob"), "imp", &run), Err(ApiError::NotFound(_))));
        assert!(matches!(x.flow_run_stop(&p, "imp", "no-such-run"), Err(ApiError::NotFound(_))));
        x.flow_run_stop(&p, "imp", &run).unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(15);
        let mut status = String::new();
        while std::time::Instant::now() < end {
            status = x.flow_run_get(&p, "imp", &run).unwrap()["status"].as_str().unwrap_or("").to_string();
            if status == "cancelled" {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(status, "cancelled", "the stopped run ended");
    }

    fn wait_status(x: &std::sync::Arc<crate::webservice::deps::Deps>, p: &Principal, flow: &str, run: &str, want: &str) -> String {
        let end = std::time::Instant::now() + Duration::from_secs(20);
        let mut status = String::new();
        while std::time::Instant::now() < end {
            status = x.extras.flow_run_get(p, flow, run).map(|v| v["status"].as_str().unwrap_or("").to_string()).unwrap_or_default();
            if status == want {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        status
    }

    #[test]
    fn a_step_that_waits_for_a_person_is_answered_once_by_that_person_and_nobody_else() {
        let d = tmp("runner-decide");
        let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        let deps = assemble(Arc::new(NoAuth), Arc::new(NoAudit), svc, Arc::new(crate::cli::tests::MockHost::default()));
        let (p, other) = (principal("deciderann"), principal("deciderbob"));
        let mut f = Flow::new("ask", "Ask");
        f.nodes.push(Node::new("s", "start", json!({})));
        f.nodes.push(Node::new("a", "approval", json!({"message": "Continue?"})));
        f.nodes.push(Node::new("e", "end", json!({})));
        f.edges.push(Edge::new("s", "a"));
        f.edges.push(Edge::new("a", "e").port("approved"));
        deps.extras.flow_import(&p, &serde_json::from_str::<serde_json::Value>(&f.to_json()).unwrap()).unwrap();
        // the run reaches the step and waits ("needs you")
        let run = deps.extras.flow_run(&p, "ask", true, &json!({})).unwrap();
        assert_eq!(wait_status(&deps, &p, "ask", &run, "needs you"), "needs you");
        // not theirs, no such step, not a step that waits: not found
        assert!(matches!(deps.extras.flow_run_decide(&other, "ask", &run, "a", true), Err(ApiError::NotFound(_))));
        assert!(matches!(deps.extras.flow_run_decide(&p, "ask", &run, "nope", true), Err(ApiError::NotFound(_))));
        assert!(matches!(deps.extras.flow_run_decide(&p, "ask", &run, "s", true), Err(ApiError::NotFound(_))), "a step that is not waiting");
        assert!(matches!(deps.extras.flow_run_decide(&p, "other-flow", &run, "a", true), Err(ApiError::NotFound(_))));
        // the person approves: the run goes on and ends; a second answer finds nothing waiting
        deps.extras.flow_run_decide(&p, "ask", &run, "a", true).unwrap();
        assert_eq!(wait_status(&deps, &p, "ask", &run, "succeeded"), "succeeded");
        assert!(deps.extras.flow_run_decide(&p, "ask", &run, "a", true).is_err());
        // a no ends the run without the rest
        let run2 = deps.extras.flow_run(&p, "ask", true, &json!({})).unwrap();
        assert_eq!(wait_status(&deps, &p, "ask", &run2, "needs you"), "needs you");
        deps.extras.flow_run_decide(&p, "ask", &run2, "a", false).unwrap();
        assert_eq!(wait_status(&deps, &p, "ask", &run2, "failed"), "failed");
    }
}

// ------------------------------------------------------------------------------------------ real runs (no fakes)

/// A flow made of real steps: a real program through the local runner (`printf`, `false`, `sleep`), a real HTTP call to a loopback
/// server, the engine's real scheduling, retries and persistence in a temp folder.
fn run_flow(env: &FlowEnv, f: Flow, secs: u64) -> iyakki_flow::model::Run {
    let id = f.id.clone();
    env.create_draft(f, "user:t").unwrap();
    env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let o = env.engine.start_run(StartRequest::new(&id)).unwrap();
    env.engine.wait_for(&o.run_id, Duration::from_secs(secs), |r| r.status.is_terminal()).unwrap()
}

fn chain(id: &str, nodes: Vec<Node>) -> Flow {
    let mut f = Flow::new(id, id);
    f.nodes.push(Node::new("start", "start", json!({})));
    let mut prev = "start".to_string();
    for n in nodes {
        f.edges.push(Edge::new(&prev, &n.id));
        prev = n.id.clone();
        f.nodes.push(n);
    }
    f
}

#[test]
fn a_real_run_starts_a_real_program_parses_its_json_and_posts_it_to_a_real_server() {
    let (env, _d) = env_with("realrun1", policy(&["127.0.0.1"]));
    let (port, seen) = serve(vec![http_resp(200, &[("Content-Type", "application/json")], r#"{"stored":true}"#)], 0);
    let mut http = Node::new("send", "http", json!({"method": "POST", "url": format!("http://127.0.0.1:{port}/in"), "body": {"n": "{{ nodes.make.output.data.n }}", "who": "{{ nodes.make.output.data.who }}"}, "extract": {"stored": "stored"}}));
    http.timeout_ms = Some(3000);
    let end = Node::new("e", "end", json!({"outputs": {"stored": "{{ nodes.send.output.stored }}", "line": "{{ nodes.make.output.stdout }}"}}));
    let make = Node::new("make", "cli", json!({"command": ["printf", "{\"n\": 41, \"who\": \"ada\"}"], "parse": "json"}));
    let r = run_flow(&env, chain("realrun1", vec![make, http, end]), 20);
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert!(r.nodes.values().all(|n| n.state == NodeState::Succeeded));
    assert_eq!(r.outputs["stored"], true);
    assert!(r.outputs["line"].as_str().unwrap().contains("\"who\": \"ada\""), "{:?}", r.outputs);
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.starts_with("POST /in "), "{sent}");
    assert!(std::fs::read_dir(&env.data).unwrap().flatten().any(|e| e.path().is_dir()), "state is on disk");
}

#[test]
fn a_real_failing_program_fails_the_run_with_its_exit_code_and_a_refused_program_never_starts() {
    let (env, _d) = env_with("realrun2", policy(&["*"]));
    let r = run_flow(&env, chain("realrun2", vec![Node::new("bad", "cli", json!({"command": ["false"]}))]), 20);
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("exited with code 1"), "{}", r.error);
    let r = run_flow(&env, chain("realrun3", vec![Node::new("no", "cli", json!({"command": ["sudo", "id"]}))]), 20);
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("sudo") && (r.error.contains("never allowed") || r.error.contains(super::programs::NOT_ALLOWED)), "{}", r.error);
    let r = run_flow(&env, chain("realrun4", vec![{
        let mut n = Node::new("slow", "cli", json!({"command": ["sleep", "5"]}));
        n.timeout_ms = Some(300);
        n
    }]), 20);
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.error.contains("took longer than 300 ms"), "{}", r.error);
}

#[test]
fn the_health_check_and_backup_templates_run_for_real_against_a_loopback_server_and_a_real_tar() {
    let (env, _d) = env_with("realtpl", policy(&["127.0.0.1"]));
    // health check: a server that answers 200, then one that answers 503 three times (the retry policy) and takes the alert port
    let (ok_port, _) = serve(vec![http_resp(200, &[], "fine")], 0);
    let mut f = templates::instantiate(&templates::get("health-check").unwrap(), "hc-ok", "");
    f.triggers.clear();
    f.inputs[0].default = Some(json!(format!("http://127.0.0.1:{ok_port}/health")));
    let r = run_flow(&env, f, 30);
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert_eq!(r.outputs["status"], 200);
    let (bad_port, seen) = serve(vec![http_resp(503, &[], "down"), http_resp(503, &[], "down"), http_resp(503, &[], "down")], 0);
    let mut f = templates::instantiate(&templates::get("health-check").unwrap(), "hc-bad", "");
    f.triggers.clear();
    f.inputs[0].default = Some(json!(format!("http://127.0.0.1:{bad_port}/health")));
    let r = run_flow(&env, f, 30);
    assert_eq!(seen.lock().unwrap().len(), 3, "three tries");
    assert!(r.nodes.get("alert").is_some_and(|n| n.state == NodeState::Succeeded), "the alert step ran: {:?}", r.nodes.keys().collect::<Vec<_>>());
    // nightly backup: a real tar of a folder in the flow's workspace, then a missing folder takes the failure path
    let mut f = templates::instantiate(&templates::get("nightly-backup").unwrap(), "bk-ok", "");
    f.triggers.clear();
    let ws = env.data.join("work").join("bk-ok");
    std::fs::create_dir_all(ws.join("Documents")).unwrap();
    std::fs::write(ws.join("Documents/a.txt"), "hello").unwrap();
    let r = run_flow(&env, f, 60);
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    assert!(ws.join("backup-latest.tgz").metadata().unwrap().len() > 20, "the archive is real");
    let mut f = templates::instantiate(&templates::get("nightly-backup").unwrap(), "bk-missing", "");
    f.triggers.clear();
    f.nodes.iter_mut().find(|n| n.id == "pack").unwrap().retry = None;
    let r = run_flow(&env, f, 60);
    assert!(r.nodes.get("failed").is_some_and(|n| n.state == NodeState::Succeeded), "the failure path ran: {:?}", r.nodes.iter().map(|(k, n)| (k.clone(), n.state)).collect::<Vec<_>>());
}

#[test]
fn a_machine_can_be_saved_with_a_private_authority_certificate_by_chat_and_a_key_is_refused() {
    let (env, _d) = env_with("rmachca", policy(&["*"]));
    env.svc.on_login(&env.user, "a-long-test-passphrase", true);
    env.svc.user_set(&env.user, "lab-token", "tok-secret-12345").unwrap();
    let pem = "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUQ0FUZXN0\n-----END CERTIFICATE-----";
    let ok = call(&env, "remote_machine_add", json!({"name": "lab", "url": "https://lab.local:8443", "token_vault": "lab-token", "ca_pem": pem})).unwrap();
    assert!(ok.contains("private authority"), "{ok}");
    assert!(call(&env, "remote_machines", json!({})).unwrap().contains("(private CA)"));
    let key = "-----BEGIN PRIVATE KEY-----\nSECRETSECRET\n-----END PRIVATE KEY-----";
    let e = call(&env, "remote_machine_add", json!({"name": "k", "url": "https://k.local:8443", "token_vault": "lab-token", "ca_pem": key})).unwrap_err();
    assert!(e.contains("private key") && !e.contains("SECRETSECRET"), "{e}");
    // the approval card says a certificate is part of the request
    let d = defs(&policy(&["*"]));
    assert!(d.iter().find(|x| x.spec.name == "remote_machine_add").unwrap().spec.description.contains("ca_pem"));
}
