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
    assert!(env.host.exec(&rr, &Cancel::new()).unwrap_err().message.contains("planned"));
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
    for n in ["flow_list", "flow_get", "flow_runs", "flow_run_status", "flow_logs", "flow_explain_failure"] {
        assert_eq!(risk(n), Risk::Safe, "{n}");
    }
    for n in ["flow_create", "flow_edit", "flow_run", "flow_cancel", "flow_publish", "flow_rollback", "flow_apply_to_runs", "flow_set_heal_policy"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    assert_eq!(d.len(), 14);
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
    assert!(ctx.len() <= 3500 && ctx.contains("OPEN ON SCREEN: selected node ping") && ctx.contains("SELECTED RUN") && ctx.contains("check-the-shop"), "{ctx}");
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
    assert_eq!(t.defs().len(), 14);
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
    let flows = crate::scope::home_root(user, svc.dir()).join("Flows");
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
