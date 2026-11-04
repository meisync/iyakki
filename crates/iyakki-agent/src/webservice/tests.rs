//! Tests of the web service on fakes: accounts and audit from `iyakki-core-fake`, a scripted model, a fake tool set. Most go
//! through `server::handle` (headers are visible there); the TLS ones use a real listener on 127.0.0.1:0 and the pinning client
//! of developer mode.

use super::config::{Checked, ServiceConfig};
use super::deps::{Deps, Extras, ToolSource};
use super::routes::{Access, ROUTES};
use super::server::{self, Service};
use super::{openapi, routes, tls, tools, Profile};
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
use crate::httpmini::{Request, Response};
use crate::runs::RunStore;
use iyakki_core_api::{ApiError, ApiResult, Principal};
use iyakki_core_fake::FakeAccounts;
use iyakki_sec::policy::Role;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------ fakes

struct Script {
    turns: Mutex<VecDeque<ModelTurn>>,
}

impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("plain chat is not scripted".into())
    }
    fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        Ok(self.turns.lock().unwrap().pop_front().unwrap_or(ModelTurn { text: "done".into(), calls: vec![] }))
    }
}

fn call_turn(name: &str) -> ModelTurn {
    ModelTurn { text: String::new(), calls: vec![ToolCall { id: format!("c-{name}"), name: name.into(), args: json!({"url": "https://example.org"}) }] }
}

fn final_turn(t: &str) -> ModelTurn {
    ModelTurn { text: t.into(), calls: vec![] }
}

struct FakeTools {
    executed: Arc<Mutex<Vec<String>>>,
}

impl Tools for FakeTools {
    fn defs(&self) -> Vec<ToolDef> {
        let t = |n: &str, risk| ToolDef { spec: ToolSpec { name: n.into(), description: n.into(), schema: json!({"type": "object"}) }, risk, untrusted: true };
        // `web_fetch` asks here so approvals can be tested with a tool that is in the profile; `run_command` is not in any profile
        vec![t("now", Risk::Safe), t("web_fetch", Risk::Ask), t("run_command", Risk::Ask), t("radio_play", Risk::Safe)]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        self.executed.lock().unwrap().push(name.into());
        Ok(format!("{name} ok"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        format!("call {name}")
    }
    // a desktop grant that must NOT count over the API
    fn pre_approved(&self, _: &str) -> bool {
        true
    }
}

struct FakeSource {
    executed: Arc<Mutex<Vec<String>>>,
}

impl ToolSource for FakeSource {
    fn tools_for(&self, _: &Principal) -> Arc<dyn Tools> {
        Arc::new(FakeTools { executed: self.executed.clone() })
    }
}

struct FakeExtras {
    ready: AtomicBool,
}

impl Extras for FakeExtras {
    fn ready(&self) -> Result<(), String> {
        if self.ready.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("not yet".into())
        }
    }
    fn models(&self, _: &Principal) -> Vec<Value> {
        vec![json!({"provider": "local", "model": "tiny"})]
    }
    fn backend_for(&self, _: &Principal, _: &str, _: &str) -> ApiResult<Arc<dyn ChatBackend>> {
        Err(ApiError::Unsupported("no other model in tests".into()))
    }
    fn flows(&self, _: &Principal) -> ApiResult<Vec<Value>> {
        Ok(vec![json!({"id": "f1", "name": "Nightly"})])
    }
    fn flow_export(&self, _: &Principal, id: &str) -> ApiResult<Value> {
        if id == "f1" {
            Ok(json!({"id": "f1", "nodes": []}))
        } else {
            Err(ApiError::NotFound("no such flow".into()))
        }
    }
    fn data_call(&self, p: &Principal, tool: &str, args: &Value) -> ApiResult<String> {
        // a `path` argument is echoed so tests can see where the service pointed it
        Ok(format!("{tool} for {}{}", p.user, args["path"].as_str().map(|x| format!(" at {x}")).unwrap_or_default()))
    }
}

struct H {
    svc: Service,
    accounts: Arc<FakeAccounts>,
    executed: Arc<Mutex<Vec<String>>>,
    script: Arc<Script>,
    dir: std::path::PathBuf,
}

fn accounts() -> Arc<FakeAccounts> {
    Arc::new(FakeAccounts::new(&[("root", "root-password-1", Role::Admin), ("ann", "ann-password-1", Role::User), ("bob", "bob-password-1", Role::User), ("gus", "gus-password-1", Role::Guest)]))
}

fn harness(profile: &str, tweak: impl FnOnce(&mut ServiceConfig)) -> H {
    let dir = crate::testdir::unique("webservice", &format!("{profile}-{}", std::thread::current().name().unwrap_or("t")));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = ServiceConfig { profile: profile.into(), listen: "127.0.0.1:0".into(), data_dir: dir.to_string_lossy().into(), ..ServiceConfig::default() };
    tweak(&mut cfg);
    let chk = cfg.check().expect("config");
    let accounts = accounts();
    let executed = Arc::new(Mutex::new(vec![]));
    let script = Arc::new(Script { turns: Mutex::new(VecDeque::new()) });
    let deps = Arc::new(Deps {
        auth: accounts.clone(),
        audit: accounts.clone(),
        store: Arc::new(RunStore::new(&dir.join("runs"))),
        chat: script.clone(),
        tools: Arc::new(FakeSource { executed: executed.clone() }),
        extras: Arc::new(FakeExtras { ready: AtomicBool::new(true) }),
        system_prompt: "test".into(),
    });
    let id = tls::self_signed(&dir.join("tls"), &[]).expect("certificate");
    let svc = server::start(&chk, deps, id).expect("start");
    H { svc, accounts, executed, script, dir }
}

fn req(method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> Request {
    let (path, q) = path.split_once('?').unwrap_or((path, ""));
    let mut headers = HashMap::new();
    headers.insert("host".to_string(), "svc.test".to_string());
    if let Some(t) = token {
        headers.insert("authorization".into(), format!("Bearer {t}"));
    }
    let body = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
    if !body.is_empty() {
        headers.insert("content-type".into(), "application/json".into());
    }
    let query = q.split('&').filter(|p| !p.is_empty()).filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Request { method: method.into(), path: path.into(), query, headers, body }
}

impl H {
    fn call(&self, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value, Response) {
        self.call_req(req(method, path, token, body))
    }
    fn call_req(&self, r: Request) -> (u16, Value, Response) {
        let resp = server::handle(self.svc.shared(), &r, "10.1.1.1".parse().unwrap());
        let v = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        (resp.status, v, resp)
    }
    fn login(&self, name: &str, pw: &str) -> String {
        let (s, v, _) = self.call("POST", "/api/v1/login", None, Some(json!({"name": name, "password": pw})));
        assert_eq!(s, 200, "{v}");
        v["token"].as_str().unwrap().to_string()
    }
    fn ann(&self) -> String {
        self.login("ann", "ann-password-1")
    }
    fn root(&self) -> String {
        self.login("root", "root-password-1")
    }
    /// Events of a run until `pred` matches one (or 10 s).
    fn wait_event(&self, tok: &str, run: &str, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut after = 0;
        let mut all = vec![];
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            let (s, v, _) = self.call("GET", &format!("/api/v1/chat/runs/{run}/events?after={after}&wait=1"), Some(tok), None);
            assert_eq!(s, 200, "{v}");
            after = v["next"].as_u64().unwrap() as usize;
            all.extend(v["events"].as_array().unwrap().iter().cloned());
            if all.iter().any(&pred) || v["done"] == true {
                break;
            }
        }
        all
    }
    fn start_run(&self, tok: &str, turns: Vec<ModelTurn>) -> String {
        self.script.turns.lock().unwrap().extend(turns);
        let (s, v, _) = self.call("POST", "/api/v1/chat/runs", Some(tok), Some(json!({"prompt": "hello there"})));
        assert_eq!(s, 200, "{v}");
        v["id"].as_str().unwrap().to_string()
    }
}

impl Drop for H {
    fn drop(&mut self) {
        self.svc.shutdown(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ------------------------------------------------------------------------------------------------ route table and OpenAPI

#[test]
fn the_route_table_is_consistent() {
    let mut seen = std::collections::HashSet::new();
    for r in ROUTES {
        assert!(seen.insert((r.method, r.path)), "duplicate {} {}", r.method, r.path);
        assert!(seen.insert(("id", r.id)), "duplicate id {}", r.id);
        assert!(!r.summary.is_empty() && r.path.starts_with('/'));
        assert!(!r.response.is_empty() && openapi::schemas().contains_key(r.response), "{}: response schema", r.id);
        assert!(r.request.is_empty() || openapi::schemas().contains_key(r.request), "{}: request schema", r.id);
        if r.method != "GET" && r.access != Access::Public {
            assert!(r.audit, "{} changes state or starts work and must be audited", r.id);
        }
        assert_eq!(r.method == "POST" && !r.request.is_empty(), r.method == "POST" && r.request != "", "{}", r.id);
    }
    // only these are reachable without signing in
    let public: Vec<&str> = ROUTES.iter().filter(|r| r.access == Access::Public).map(|r| r.id).collect();
    assert_eq!(public, ["healthz", "readyz", "version", "login"]);
    // the cloud profile is the agent-api profile plus more
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::AgentApi)) {
        assert!(r.in_profile(Profile::Cloud), "{}", r.id);
    }
    assert!(ROUTES.iter().any(|r| r.in_profile(Profile::Cloud) && !r.in_profile(Profile::AgentApi)));
}

#[test]
fn the_openapi_document_has_every_route_of_the_profile_and_nothing_else() {
    for p in Profile::ALL {
        let doc = openapi::document(p);
        assert_eq!(doc["openapi"], "3.1.0");
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        let mut documented = 0;
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                documented += 1;
                let r = ROUTES.iter().find(|r| r.path == path && r.method.eq_ignore_ascii_case(method)).unwrap_or_else(|| panic!("{method} {path} is documented but not a route"));
                assert!(r.in_profile(p));
                assert_eq!(op["operationId"], r.id);
                for s in [op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"].as_str(), op["requestBody"]["content"]["application/json"]["schema"]["$ref"].as_str()].into_iter().flatten() {
                    assert!(schemas.contains_key(s.rsplit('/').next().unwrap()), "{s}");
                }
                assert_eq!(op["security"] == json!([]), r.access == Access::Public, "{}", r.id);
            }
        }
        assert_eq!(documented, ROUTES.iter().filter(|r| r.in_profile(p)).count(), "every route of {} is documented", p.name());
    }
    // the agent-api document does not mention the cloud-only routes
    assert!(openapi::document(Profile::AgentApi)["paths"].get("/api/v1/flows").is_none());
    assert!(openapi::document(Profile::Cloud)["paths"].get("/api/v1/flows").is_some());
}

#[test]
fn every_route_of_the_table_has_a_handler_and_is_served() {
    for p in Profile::ALL {
        let h = harness(p.name(), |_| {});
        let tok = h.root();
        for r in ROUTES.iter().filter(|r| r.in_profile(p)) {
            let path = r.path.replace("{id}", "abc123abc123").replace("{run}", "abc123abc123").replace("{call}", "call1");
            let (s, v, _) = h.call(r.method, &path, Some(&tok), Some(json!({"x": 1})).filter(|_| r.method == "POST"));
            assert!(!(s == 501 && v["error"]["message"] == "not implemented"), "{}: no handler", r.id);
            assert!(s != 405, "{}: the table says {} {}", r.id, r.method, r.path);
        }
    }
}

#[test]
fn the_openapi_route_serves_the_running_profile() {
    let h = harness("agent-api", |_| {});
    let t = h.ann();
    let (s, v, _) = h.call("GET", "/api/v1/openapi.json", Some(&t), None);
    assert_eq!(s, 200);
    assert!(v["paths"].get("/api/v1/chat/runs").is_some() && v["paths"].get("/api/v1/data/call").is_none());
    assert_eq!(h.call("GET", "/api/v1/openapi.json", None, None).0, 401, "the document is for signed-in people");
}

// ------------------------------------------------------------------------------------------------ authentication, roles, 404

#[test]
fn no_token_is_401_a_bad_token_is_401_and_public_routes_need_none() {
    let h = harness("agent-api", |_| {});
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::AgentApi) && r.access != Access::Public) {
        let path = r.path.replace("{id}", "abc123abc123").replace("{run}", "abc123abc123").replace("{call}", "c");
        let (s, v, resp) = h.call(r.method, &path, None, None);
        assert_eq!(s, 401, "{} {}", r.method, r.path);
        assert_eq!(v["error"]["code"], "unauthorized");
        assert!(resp.headers.iter().any(|(k, v)| k == "WWW-Authenticate" && v == "Bearer"));
        let (s, _, _) = h.call(r.method, &path, Some("iyk_00000000000000000000000000000000000000000000000000000000000000000"), None);
        assert_eq!(s, 401);
        let (s, _, _) = h.call(r.method, &path, Some("not-a-token"), None);
        assert_eq!(s, 401);
    }
    assert_eq!(h.call("GET", "/healthz", None, None).0, 200);
    assert_eq!(h.call("GET", "/readyz", None, None).0, 200);
    let (s, v, _) = h.call("GET", "/api/v1/version", None, None);
    assert_eq!((s, v["profile"].as_str()), (200, Some("agent-api")));
}

#[test]
fn there_is_no_default_account_and_a_wrong_password_is_401_with_one_answer() {
    let h = harness("agent-api", |_| {});
    for (n, p) in [("admin", "admin"), ("root", ""), ("root", "wrong"), ("nobody", "x"), ("", "")] {
        let (s, v, _) = h.call("POST", "/api/v1/login", None, Some(json!({"name": n, "password": p})));
        assert_eq!(s, 401);
        assert_eq!(v["error"]["message"], "invalid username or password");
    }
    // a body that is not the login shape
    assert_eq!(h.call("POST", "/api/v1/login", None, Some(json!({"name": "root"}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/login", None, Some(json!({"name": "root", "password": "x", "extra": 1}))).0, 400);
}

#[test]
fn the_wrong_role_is_403() {
    let h = harness("cloud", |_| {});
    let gus = h.login("gus", "gus-password-1"); // a guest has no agent
    for (m, p) in [("GET", "/api/v1/models"), ("GET", "/api/v1/chat/runs"), ("POST", "/api/v1/chat/runs"), ("GET", "/api/v1/approvals"), ("GET", "/api/v1/flows"), ("POST", "/api/v1/data/call")] {
        assert_eq!(h.call(m, p, Some(&gus), Some(json!({"prompt": "x"})).filter(|_| m == "POST")).0, 403, "{m} {p}");
    }
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&gus), None).0, 200, "whoami needs no app");
    let ann = h.ann();
    assert_eq!(h.call("GET", "/api/v1/admin/status", Some(&ann), None).0, 403);
    let root = h.root();
    assert_eq!(h.call("GET", "/api/v1/admin/status", Some(&root), None).0, 200);
}

#[test]
fn a_route_or_tool_outside_the_profile_is_404() {
    let h = harness("agent-api", |_| {});
    let t = h.root();
    // cloud-only routes do not exist in agent-api
    for p in ["/api/v1/flows", "/api/v1/flows/f1"] {
        assert_eq!(h.call("GET", p, Some(&t), None).0, 404, "{p}");
    }
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&t), Some(json!({"tool": "data_open", "args": {}}))).0, 404);
    // things that never exist, with or without a token
    for p in ["/", "/api/v1", "/api/v1/camera", "/api/v1/audio", "/api/v1/usb", "/api/v1/firmware", "/api/v1/firewall", "/api/v1/shell", "/v1/dev/install", "/api/v1/dev/install", "/api/v1/system/power", "/metrics", "/ws", "/api/v1/chat/runs/abc/../../x"] {
        assert_eq!(h.call("GET", p, Some(&t), None).0, 404, "{p}");
        assert_eq!(h.call("GET", p, None, None).0, 404, "{p} without a token");
    }
    // a tool outside the profile cannot be asked for, and one the model calls is refused and never runs
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&t), Some(json!({"prompt": "x", "tools": ["run_command"]})));
    assert_eq!(s, 400, "{v}");
    let (s, _, _) = h.call("POST", "/api/v1/chat/runs", Some(&t), Some(json!({"prompt": "x", "tools": ["radio_play"]})));
    assert_eq!(s, 400);
    let run = h.start_run(&t, vec![call_turn("run_command"), final_turn("ok")]);
    let ev = h.wait_event(&t, &run, |e| e["type"] == "final");
    let res = ev.iter().find(|e| e["type"] == "tool_result").expect("a refusal came back to the model");
    assert_eq!(res["ok"], false);
    assert!(h.executed.lock().unwrap().is_empty(), "the tool never ran: {:?}", h.executed.lock().unwrap());
    // the data tool route of the cloud profile only takes the three read tools
    let c = harness("cloud", |_| {});
    let ct = c.root();
    assert_eq!(c.call("POST", "/api/v1/data/call", Some(&ct), Some(json!({"tool": "data_export", "args": {}}))).0, 404);
    assert_eq!(c.call("POST", "/api/v1/data/call", Some(&ct), Some(json!({"tool": "run_command", "args": {}}))).0, 404);
    let (s, v, _) = c.call("POST", "/api/v1/data/call", Some(&ct), Some(json!({"tool": "data_query", "args": {}})));
    assert_eq!((s, v["result"].as_str()), (200, Some("data_query for root")));
    assert_eq!(c.call("GET", "/api/v1/flows/nope", Some(&ct), None).0, 404);
    assert_eq!(c.call("GET", "/api/v1/flows/f1", Some(&ct), None).0, 200);
}

#[test]
fn a_wrong_method_is_405_and_a_removed_account_loses_its_tokens() {
    let h = harness("agent-api", |_| {});
    let t = h.ann();
    assert_eq!(h.call("DELETE", "/api/v1/whoami", Some(&t), None).0, 405);
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&t), None).0, 200);
    h.accounts.remove("ann");
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&t), None).0, 401);
    assert_eq!(h.svc.shared().tokens.count(), 0);
}

// ------------------------------------------------------------------------------------------------ approvals

#[test]
fn a_risky_tool_waits_for_the_owner_and_runs_once_when_approved() {
    let h = harness("agent-api", |_| {});
    let (ann, bob, root) = (h.ann(), h.login("bob", "bob-password-1"), h.root());
    let run = h.start_run(&ann, vec![call_turn("web_fetch"), final_turn("fetched")]);
    let ev = h.wait_event(&ann, &run, |e| e["type"] == "approval");
    let call = ev.iter().find(|e| e["type"] == "approval").expect("the run asks")["id"].as_str().unwrap().to_string();
    assert!(h.executed.lock().unwrap().is_empty(), "nothing ran before the answer, in spite of the desktop grant of the fake tool set");
    let (s, v, _) = h.call("GET", "/api/v1/approvals", Some(&ann), None);
    assert_eq!(s, 200);
    assert_eq!(v["approvals"][0]["tool"], "web_fetch");
    assert_eq!(v["approvals"][0]["risk"], "medium");
    assert_eq!(h.call("GET", "/api/v1/approvals", Some(&bob), None).1["approvals"].as_array().unwrap().len(), 0, "another person does not see it");
    let path = format!("/api/v1/approvals/{run}/{call}");
    // another person: 403; an unknown call: 404; a bad verdict: 400; "always" does not exist
    assert_eq!(h.call("POST", &path, Some(&bob), Some(json!({"verdict": "approve"}))).0, 403);
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/{run}/nothing"), Some(&ann), Some(json!({"verdict": "approve"}))).0, 404);
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/aaaaaaaaaaaa/{call}"), Some(&ann), Some(json!({"verdict": "approve"}))).0, 404);
    assert_eq!(h.call("POST", &path, Some(&ann), Some(json!({"verdict": "always"}))).0, 400);
    assert_eq!(h.call("POST", &path, Some(&ann), Some(json!({"verdict": "yes please"}))).0, 400);
    assert!(h.executed.lock().unwrap().is_empty());
    let (s, v, _) = h.call("POST", &path, Some(&ann), Some(json!({"verdict": "approve"})));
    assert_eq!((s, v["verdict"].as_str()), (200, Some("approve")), "{v}");
    let ev = h.wait_event(&ann, &run, |e| e["type"] == "final");
    assert!(ev.iter().any(|e| e["type"] == "final" && e["text"] == "fetched"));
    assert_eq!(*h.executed.lock().unwrap(), ["web_fetch"]);
    // a replay of the verdict: 409
    assert_eq!(h.call("POST", &path, Some(&ann), Some(json!({"verdict": "approve"}))).0, 409);
    // an administrator may answer for someone else (and others still may not)
    let run2 = h.start_run(&ann, vec![call_turn("web_fetch"), final_turn("again")]);
    let ev = h.wait_event(&ann, &run2, |e| e["type"] == "approval");
    let c2 = ev.iter().find(|e| e["type"] == "approval").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/{run2}/{c2}"), Some(&root), Some(json!({"verdict": "deny"}))).0, 200);
    let ev = h.wait_event(&ann, &run2, |e| e["type"] == "final");
    assert!(ev.iter().any(|e| e["type"] == "decision" && e["approved"] == false));
    assert_eq!(h.executed.lock().unwrap().len(), 1, "a denied call did not run");
    // the audit log has the decisions, without prompts or tokens
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("api.approvals_decide") && lines.contains("verdict=approve") && lines.contains("verdict=deny"), "{lines}");
}

#[test]
fn an_unanswered_approval_times_out_refused_and_the_tool_never_runs() {
    let h = harness("agent-api", |c| c.limits.approval_timeout_secs = 1);
    let t = h.ann();
    let run = h.start_run(&t, vec![call_turn("web_fetch"), final_turn("gave up")]);
    let ev = h.wait_event(&t, &run, |e| e["type"] == "final");
    let d = ev.iter().find(|e| e["type"] == "decision").expect("a decision was recorded");
    assert_eq!((d["approved"].clone(), d["by"].as_str()), (json!(false), Some("system")));
    assert!(h.executed.lock().unwrap().is_empty());
    // a late answer: 409, not a surprise execution
    let call = ev.iter().find(|e| e["type"] == "approval").unwrap()["id"].as_str().unwrap();
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/{run}/{call}"), Some(&t), Some(json!({"verdict": "approve"}))).0, 409);
}

#[test]
fn the_runs_a_person_has_are_theirs_alone_and_capped() {
    let h = harness("agent-api", |c| c.limits.concurrent_runs = 2);
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    let run = h.start_run(&ann, vec![final_turn("hi")]);
    h.wait_event(&ann, &run, |e| e["type"] == "final");
    for p in [format!("/api/v1/chat/runs/{run}"), format!("/api/v1/chat/runs/{run}/events")] {
        assert_eq!(h.call("GET", &p, Some(&bob), None).0, 404, "bob asks for ann's run");
        assert_eq!(h.call("GET", &p, Some(&ann), None).0, 200);
    }
    assert_eq!(h.call("POST", &format!("/api/v1/chat/runs/{run}/stop"), Some(&bob), None).0, 404);
    assert_eq!(h.call("GET", "/api/v1/chat/runs", Some(&bob), None).1["runs"].as_array().unwrap().len(), 0);
    assert_eq!(h.call("GET", "/api/v1/chat/runs", Some(&ann), None).1["runs"][0]["id"], run.as_str());
    // two runs waiting for approvals fill the cap; the third is refused; stopping one frees a slot
    let r1 = h.start_run(&ann, vec![call_turn("web_fetch")]);
    let r2 = h.start_run(&ann, vec![call_turn("web_fetch")]);
    h.wait_event(&ann, &r1, |e| e["type"] == "approval");
    h.wait_event(&ann, &r2, |e| e["type"] == "approval");
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "third"})));
    assert_eq!((s, v["error"]["code"].as_str()), (429, Some("busy")), "{v}");
    assert_eq!(h.call("POST", &format!("/api/v1/chat/runs/{r1}/stop"), Some(&ann), None).0, 200);
    h.wait_event(&ann, &r1, |e| e["type"] == "stopped");
    h.script.turns.lock().unwrap().push_back(final_turn("ok"));
    assert_eq!(h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "third"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "   "}))).0, 400);
}

// ------------------------------------------------------------------------------------------------ limits, CORS, audit

#[test]
fn requests_per_minute_are_limited_per_person_and_sign_ins_per_address() {
    let h = harness("agent-api", |c| {
        c.limits.requests_per_minute = 5;
        c.limits.logins_per_minute = 3;
    });
    let t = h.ann(); // one of the three sign-in tries
    for _ in 0..5 {
        assert_eq!(h.call("GET", "/api/v1/whoami", Some(&t), None).0, 200);
    }
    let (s, _, resp) = h.call("GET", "/api/v1/whoami", Some(&t), None);
    assert_eq!(s, 429);
    assert!(resp.headers.iter().any(|(k, v)| k == "Retry-After" && v.parse::<u64>().unwrap() >= 1));
    let other = h.login("bob", "bob-password-1"); // another person is not affected
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&other), None).0, 200);
    // the third try is the last of the minute; the fourth is refused even with the right password
    let (s, _, _) = h.call("POST", "/api/v1/login", None, Some(json!({"name": "root", "password": "root-password-1"})));
    assert_eq!(s, 200);
    let (s, _, _) = h.call("POST", "/api/v1/login", None, Some(json!({"name": "root", "password": "root-password-1"})));
    assert_eq!(s, 429);
}

#[test]
fn a_body_above_the_cap_is_refused_before_it_is_read() {
    let h = harness("agent-api", |c| c.limits.max_body_bytes = 1024);
    let big = "x".repeat(2000);
    // through the real parser: the header alone decides
    let head = format!("POST /api/v1/login HTTP/1.1\r\nHost: a\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", big.len());
    assert!(matches!(crate::httpmini::parse_limited(head.as_bytes(), 1024), crate::httpmini::Parse::Bad(413, _)));
    // and over TLS, end to end
    let (host, port) = ("127.0.0.1", h.svc.local_addr().port());
    let body = json!({"name": "root", "password": big}).to_string();
    let r = crate::devmode::client::https(host, port, Some(h.svc.fingerprint()), "POST", "/api/v1/login", &[("Content-Type".into(), "application/json".into())], body.as_bytes(), Duration::from_secs(5)).expect("answer");
    assert_eq!(r.status, 413);
    assert_eq!(r.json()["error"]["code"], "too_large");
    let small = json!({"name": "root", "password": "root-password-1"}).to_string();
    let r = crate::devmode::client::https(host, port, Some(h.svc.fingerprint()), "POST", "/api/v1/login", &[("Content-Type".into(), "application/json".into())], small.as_bytes(), Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200);
}

#[test]
fn cors_is_locked_unless_an_origin_is_listed() {
    let h = harness("agent-api", |_| {});
    let t = h.ann();
    let with_origin = |o: &str, m: &str, p: &str| {
        let mut r = req(m, p, Some(&t), None);
        r.headers.insert("origin".into(), o.into());
        r
    };
    let (s, _, resp) = h.call_req(with_origin("https://evil.example", "GET", "/api/v1/whoami"));
    assert_eq!(s, 403);
    assert!(!resp.headers.iter().any(|(k, _)| k.to_ascii_lowercase().starts_with("access-control")));
    let (s, _, _) = h.call_req(with_origin("https://evil.example", "OPTIONS", "/api/v1/whoami"));
    assert_eq!(s, 403);
    let mut pre = with_origin("https://svc.test", "OPTIONS", "/api/v1/whoami");
    pre.headers.insert("access-control-request-method".into(), "GET".into());
    assert_eq!(h.call_req(pre).0, 404, "nobody listed: no preflight answer");
    // the service's own address is fine and gets no CORS header
    let (s, _, resp) = h.call_req(with_origin("https://svc.test", "GET", "/api/v1/whoami"));
    assert_eq!(s, 200);
    assert!(!resp.headers.iter().any(|(k, _)| k.to_ascii_lowercase().starts_with("access-control")));
    // a listed origin, exactly
    let l = harness("agent-api", |c| c.cors_origins = vec!["https://app.example".into()]);
    let t2 = l.ann();
    let mut r = req("GET", "/api/v1/whoami", Some(&t2), None);
    r.headers.insert("origin".into(), "https://app.example".into());
    let (s, _, resp) = l.call_req(r);
    assert_eq!(s, 200);
    assert!(resp.headers.iter().any(|(k, v)| k == "Access-Control-Allow-Origin" && v == "https://app.example"));
    let mut r = req("GET", "/api/v1/whoami", Some(&t2), None);
    r.headers.insert("origin".into(), "https://app.example.evil".into());
    assert_eq!(l.call_req(r).0, 403);
    assert!(ServiceConfig { cors_origins: vec!["*".into()], ..ServiceConfig::default() }.check().is_err());
}

#[test]
fn a_body_must_be_json() {
    let h = harness("agent-api", |_| {});
    let mut r = req("POST", "/api/v1/login", None, None);
    r.body = b"name=root".to_vec();
    r.headers.insert("content-type".into(), "application/x-www-form-urlencoded".into());
    assert_eq!(h.call_req(r).0, 400);
}

#[test]
fn audit_lines_exist_for_every_audited_route_and_hold_no_secrets() {
    // one sign-in per audited route: more routes than the default sign-in limit
    let h = harness("cloud", |c| c.limits.logins_per_minute = 1000);
    let secret_prompt = "my-very-private-prompt-text";
    h.script.turns.lock().unwrap().push_back(final_turn("ok"));
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::Cloud) && r.audit) {
        let path = r.path.replace("{id}", "abc123abc123").replace("{run}", "abc123abc123").replace("{call}", "c");
        let body = match r.id {
            "login" => json!({"name": "root", "password": "wrong-password-xyz"}),
            "runs_start" => json!({"prompt": secret_prompt}),
            "approvals_decide" => json!({"verdict": "approve"}),
            "data_call" => json!({"tool": "data_open", "args": {"path": "secret-file-name.csv"}}),
            _ => json!({}),
        };
        let before = h.accounts.lines().len();
        let t = h.root(); // a fresh token each time: logout ends the one it is given
        h.call(r.method, &path, Some(&t), Some(body).filter(|_| r.method == "POST"));
        let added = h.accounts.lines()[before..].join("\n");
        assert!(added.contains(&format!("api.{}", r.id)) || (r.id == "login" && added.contains("api.login")), "{}: no audit line in [{added}]", r.id);
    }
    let all = h.accounts.lines().join("\n");
    for secret in [secret_prompt, "wrong-password-xyz", "root-password-1", "iyk_", "secret-file-name"] {
        assert!(!all.contains(secret), "the audit log holds a secret: {secret}");
    }
    assert!(all.contains("api.login.denied") && all.contains("name=root"));
}

#[test]
fn responses_carry_no_secret_fields_and_the_error_form_is_stable() {
    let h = harness("cloud", |_| {});
    let t = h.root();
    let mut all = String::new();
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::Cloud) && r.method == "GET" && r.id != "openapi") {
        let path = r.path.replace("{id}", "f1");
        let (_, v, _) = h.call("GET", &path, Some(&t), None);
        all.push_str(&v.to_string());
    }
    for planted in ["root-password-1", "ann-password-1", "password", "hash", "api_key", "secret"] {
        assert!(!all.to_lowercase().contains(planted), "a response mentions {planted}: {all}");
    }
    let (s, v, _) = h.call("GET", "/api/v1/nothing", Some(&t), None);
    assert_eq!((s, v["error"]["code"].as_str()), (404, Some("not_found")));
}

// ------------------------------------------------------------------------------------------------ cloud profile and hardware

#[test]
fn the_cloud_profile_offers_nothing_that_needs_local_hardware() {
    use crate::cloudmode::{app_class, Class};
    // the apps its routes need all work with no local hardware (cloudmode.rs is the one table that says so)
    for p in Profile::ALL {
        for a in p.apps() {
            assert_eq!(app_class(*a), Class::Remote, "{a:?}");
        }
    }
    // no tool of either profile needs hardware, and no route path names any
    for p in Profile::ALL {
        for t in p.tools() {
            assert!(super::tool_hardware(t).is_none(), "{t}");
            assert!(crate::runs::approval_risk(t) != "high", "{t}: a high-risk tool is never in a profile list");
        }
        for r in ROUTES.iter().filter(|r| r.in_profile(p)) {
            for bad in ["camera", "audio", "sound", "radio", "media", "usb", "firmware", "firewall", "disk", "bluetooth", "display", "shell", "install", "terminal", "dev/", "power"] {
                assert!(!r.path.contains(bad), "{} mentions {bad}", r.path);
            }
        }
    }
    // an administrator cannot enable one by name: the filter drops hardware tools in the cloud profile, the config check refuses them
    let set = tools::allowed_set(Profile::Cloud, &["radio_play".to_string(), "camera_snap".to_string()], &[]).unwrap();
    assert!(!set.contains("radio_play") && !set.contains("camera_snap"));
    assert!(ServiceConfig { profile: "cloud".into(), enable_tools: vec!["radio_play".into()], ..ServiceConfig::default() }.check().is_err());
    assert!(ServiceConfig { enable_tools: vec!["run_command".into()], ..ServiceConfig::default() }.check().is_err());
    // live: the cloud profile's tool list and its 404s
    let h = harness("cloud", |_| {});
    let t = h.root();
    let (_, v, _) = h.call("GET", "/api/v1/tools", Some(&t), None);
    let names: Vec<&str> = v["tools"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert!(names.contains(&"flow_list") && names.contains(&"data_query") && !names.contains(&"radio_play") && !names.contains(&"run_command"));
    for p in ["/api/v1/camera", "/api/v1/audio", "/api/v1/radio", "/api/v1/usb", "/api/v1/firmware", "/api/v1/firewall", "/api/v1/dev/install", "/api/v1/system/reboot"] {
        assert_eq!(h.call("GET", p, Some(&t), None).0, 404);
        assert_eq!(h.call("POST", p, Some(&t), Some(json!({}))).0, 404);
    }
    // a model that asks for a hardware tool is refused, and it never ran
    let run = h.start_run(&t, vec![call_turn("radio_play"), final_turn("ok")]);
    let ev = h.wait_event(&t, &run, |e| e["type"] == "final");
    assert!(ev.iter().any(|e| e["type"] == "tool_result" && e["ok"] == false));
    assert!(h.executed.lock().unwrap().is_empty());
}

#[test]
fn the_default_tool_set_is_exactly_this_list() {
    // the reviewed list, by name (docs/development/MINIMAL-WEBSERVICE.md, section 9); a change here is a decision
    assert_eq!(Profile::AgentApi.tools(), ["now", "web_search", "web_fetch", "web_research", "web_feed", "web_links", "weather", "stock_quote", "search_documents", "data_open", "data_describe", "data_query"]);
    let extra: Vec<&&str> = Profile::Cloud.tools().iter().filter(|t| !Profile::AgentApi.tools().contains(t)).collect();
    assert_eq!(extra, [&"flow_list", &"flow_get", &"flow_runs", &"flow_run_status"]);
    assert!(tools::allowed_set(Profile::AgentApi, &[], &["now".into(), "web_search".into()]).unwrap().len() == 2, "a request may narrow the list");
}

// ------------------------------------------------------------------------------------------------ config, loopback, firewall

#[test]
fn the_default_is_loopback_and_a_lan_needs_the_opt_in_and_a_network() {
    let d = ServiceConfig::default().check().unwrap();
    assert!(d.addr.ip().is_loopback() && d.profile == Profile::AgentApi);
    let lan = |f: fn(&mut ServiceConfig)| {
        let mut c = ServiceConfig { listen: "0.0.0.0:8444".into(), ..ServiceConfig::default() };
        f(&mut c);
        c.check()
    };
    assert!(lan(|_| {}).unwrap_err().contains("loopback"));
    assert!(lan(|c| c.lan = true).unwrap_err().contains("allow_from"));
    assert!(lan(|c| {
        c.lan = true;
        c.allow_from = vec!["0.0.0.0/0".into()]
    })
    .is_err());
    assert!(lan(|c| {
        c.lan = true;
        c.allow_from = vec!["not a cidr".into()]
    })
    .is_err());
    assert!(lan(|c| {
        c.lan = true;
        c.allow_from = vec!["192.168.1.0/24".into()]
    })
    .is_ok());
    // a host name is not an address; port 0 only on loopback; unknown profile; unknown field; half a certificate; zero limits
    assert!(ServiceConfig { listen: "example.org:8444".into(), ..ServiceConfig::default() }.check().is_err());
    assert!(ServiceConfig { listen: "0.0.0.0:0".into(), lan: true, allow_from: vec!["10.0.0.0/8".into()], ..ServiceConfig::default() }.check().is_err());
    assert!(ServiceConfig { profile: "flows-runner".into(), ..ServiceConfig::default() }.check().unwrap_err().contains("not built yet"));
    assert!(ServiceConfig::parse(r#"{"listn": "0.0.0.0:1"}"#).is_err(), "a typo is an error, not a default");
    assert!(ServiceConfig { tls: super::config::TlsFiles { cert: "a".into(), key: String::new() }, ..ServiceConfig::default() }.check().is_err());
    let mut z = ServiceConfig::default();
    z.limits.concurrent_runs = 0;
    assert!(z.check().is_err());
    assert_eq!(ServiceConfig::parse("{}").unwrap(), ServiceConfig::default());
}

#[test]
fn the_cloud_example_config_parses_and_checks() {
    let text = include_str!("../../../../dist/service/service.cloud.json");
    let c = ServiceConfig::parse(text).expect("the example parses");
    let chk = c.check().expect("the example is valid");
    assert_eq!(chk.profile, Profile::Cloud);
    assert!(c.lan && !c.allow_from.is_empty() && !c.tls.cert.is_empty() && !c.token_store.is_empty());
}

struct Recorder {
    calls: Mutex<Vec<(u16, Vec<String>)>>,
    fail: bool,
}

impl super::PortOpener for Recorder {
    fn open(&self, port: u16, allow_from: &[String]) -> Result<(), String> {
        self.calls.lock().unwrap().push((port, allow_from.to_vec()));
        if self.fail {
            Err("no firewall".into())
        } else {
            Ok(())
        }
    }
}

fn deps_for(dir: &std::path::Path) -> Arc<Deps> {
    let a = accounts();
    Arc::new(Deps {
        auth: a.clone(),
        audit: a,
        store: Arc::new(RunStore::new(&dir.join("runs"))),
        chat: Arc::new(Script { turns: Mutex::new(VecDeque::new()) }),
        tools: Arc::new(FakeSource { executed: Arc::new(Mutex::new(vec![])) }),
        extras: Arc::new(FakeExtras { ready: AtomicBool::new(true) }),
        system_prompt: "t".into(),
    })
}

fn free_port() -> u16 {
    crate::testdir::free_port()
}

fn run_in_background(chk: Checked, deps: Arc<Deps>, opener: Arc<Recorder>) -> (Arc<AtomicBool>, std::thread::JoinHandle<Result<(), String>>) {
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let h = std::thread::spawn(move || super::run(&chk, deps, opener.as_ref(), &s2));
    (stop, h)
}

#[test]
fn run_opens_the_firewall_only_for_a_lan_and_registers_with_the_startup_registry() {
    let dir = crate::testdir::unique("webservice", "run");
    std::fs::create_dir_all(&dir).unwrap();
    let base = ServiceConfig { data_dir: dir.to_string_lossy().into(), ..ServiceConfig::default() };
    // loopback: no firewall call; the service reports ready, then stopped
    let rec = Arc::new(Recorder { calls: Mutex::new(vec![]), fail: false });
    let chk = ServiceConfig { listen: "127.0.0.1:0".into(), ..base.clone() }.check().unwrap();
    let (stop, h) = run_in_background(chk, deps_for(&dir), rec.clone());
    let end = Instant::now() + Duration::from_secs(5);
    while crate::startup::global().state("service") != Some(crate::startup::StartupState::Ready) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(crate::startup::global().state("service"), Some(crate::startup::StartupState::Ready));
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap().unwrap();
    assert_eq!(crate::startup::global().state("service"), Some(crate::startup::StartupState::Stopped));
    assert!(rec.calls.lock().unwrap().is_empty(), "loopback needs no firewall rule");
    // a LAN opt-in: the rule is requested for the port and the named networks
    // a port found free can be taken before the service binds it (another test, another process): try again with a new one
    let (port, chk, rec, stop, h) = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let port = free_port();
            let chk = ServiceConfig { listen: format!("0.0.0.0:{port}"), lan: true, allow_from: vec!["127.0.0.0/8".into()], ..base.clone() }.check().unwrap();
            let rec = Arc::new(Recorder { calls: Mutex::new(vec![]), fail: false });
            let (stop, h) = run_in_background(chk.clone(), deps_for(&dir), rec.clone());
            let end = Instant::now() + Duration::from_secs(5);
            while rec.calls.lock().unwrap().is_empty() && !h.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(20));
            }
            if h.is_finished() && rec.calls.lock().unwrap().is_empty() && attempt < 5 {
                let e = h.join().unwrap().unwrap_err();
                assert!(e.contains("cannot listen"), "{e}");
                continue;
            }
            break (port, chk, rec, stop, h);
        }
    };
    assert_eq!(*rec.calls.lock().unwrap(), [(port, vec!["127.0.0.0/8".to_string()])]);
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap().unwrap();
    // the firewall refuses: the service does not stay up listening
    let rec = Arc::new(Recorder { calls: Mutex::new(vec![]), fail: true });
    let (_stop, h) = run_in_background(chk, deps_for(&dir), rec);
    let e = h.join().unwrap().unwrap_err();
    assert!(e.contains("firewall"), "{e}");
    assert!(matches!(crate::startup::global().state("service"), Some(crate::startup::StartupState::Failed(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ TLS, client, shutdown, tokens

#[test]
fn tls_is_required_and_a_wrong_pin_is_refused() {
    let h = harness("agent-api", |_| {});
    let port = h.svc.local_addr().port();
    let get = |fp: Option<&str>| crate::devmode::client::https("127.0.0.1", port, fp, "GET", "/healthz", &[], b"", Duration::from_secs(5));
    let r = get(Some(h.svc.fingerprint())).expect("the pinned certificate is accepted");
    assert_eq!((r.status, r.fingerprint.as_str()), (200, h.svc.fingerprint()));
    assert_eq!(h.svc.fingerprint().len(), 95);
    let wrong = "00:".repeat(31) + "00";
    let e = get(Some(&wrong)).unwrap_err();
    assert!(e.to_lowercase().contains("certificate") || e.contains("changed"), "{e}");
    // plain HTTP gets no HTTP answer at all
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let _ = s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(!String::from_utf8_lossy(&buf).contains("HTTP/1.1 200"), "a plain request must not be served");
    // the certificate is kept: the same fingerprint after a reload; the key is private
    let again = tls::self_signed(&h.dir.join("tls"), &[]).unwrap();
    assert_eq!(again.fingerprint, h.svc.fingerprint());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(h.dir.join("tls/key.pem")).unwrap().permissions().mode() & 0o777, 0o600);
    // the administrator's own certificate is used as it is
    let own = tls::from_files(&h.dir.join("tls/cert.pem"), &h.dir.join("tls/key.pem")).unwrap();
    assert!(!own.self_signed && own.fingerprint == again.fingerprint);
    assert!(tls::from_files(&h.dir.join("nope.pem"), &h.dir.join("tls/key.pem")).is_err());
}

#[test]
fn a_client_can_sign_in_chat_and_sign_out_over_tls_on_an_ephemeral_port() {
    let h = harness("agent-api", |_| {});
    let port = h.svc.local_addr().port();
    assert!(h.svc.local_addr().ip().is_loopback());
    let fp = h.svc.fingerprint().to_string();
    let call = |m: &str, p: &str, tok: Option<&str>, body: Option<Value>| {
        let mut hd = vec![];
        if let Some(t) = tok {
            hd.push(("Authorization".to_string(), format!("Bearer {t}")));
        }
        let b = body.map(|b| b.to_string()).unwrap_or_default();
        if !b.is_empty() {
            hd.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        let r = crate::devmode::client::https("127.0.0.1", port, Some(&fp), m, p, &hd, b.as_bytes(), Duration::from_secs(15)).expect("answer");
        (r.status, r.json())
    };
    assert_eq!(call("GET", "/api/v1/whoami", None, None).0, 401);
    let (s, v) = call("POST", "/api/v1/login", None, Some(json!({"name": "ann", "password": "ann-password-1"})));
    assert_eq!(s, 200);
    let tok = v["token"].as_str().unwrap().to_string();
    assert!(tok.starts_with("iyk_") && tok.len() == 68 && v["role"] == "user");
    assert_eq!(call("GET", "/api/v1/whoami", Some(&tok), None).1["user"], "ann");
    assert_eq!(call("GET", "/api/v1/models", Some(&tok), None).1["models"][0]["model"], "tiny");
    h.script.turns.lock().unwrap().push_back(final_turn("the answer"));
    let (s, v) = call("POST", "/api/v1/chat/runs", Some(&tok), Some(json!({"prompt": "what is up"})));
    assert_eq!(s, 200);
    let run = v["id"].as_str().unwrap().to_string();
    let (s, v) = call("GET", &format!("/api/v1/chat/runs/{run}/events?after=0&wait=10"), Some(&tok), None);
    assert_eq!(s, 200);
    assert!(v["events"].as_array().unwrap().iter().any(|e| e["type"] == "final" && e["text"] == "the answer"), "{v}");
    // once finished, the saved record answers too
    std::thread::sleep(Duration::from_millis(200));
    let (s, v) = call("GET", &format!("/api/v1/chat/runs/{run}"), Some(&tok), None);
    assert_eq!((s, v["status"].as_str()), (200, Some("completed")));
    assert_eq!(call("POST", "/api/v1/logout", Some(&tok), None).0, 200);
    assert_eq!(call("GET", "/api/v1/whoami", Some(&tok), None).0, 401);
}

#[test]
fn readiness_follows_the_services_and_shutdown_is_graceful() {
    let h = harness("agent-api", |_| {});
    // not ready: 503 with no reason in the body
    let dir = crate::testdir::unique("webservice", "ready");
    std::fs::create_dir_all(&dir).unwrap();
    let a = accounts();
    let extras = Arc::new(FakeExtras { ready: AtomicBool::new(false) });
    let deps = Arc::new(Deps { auth: a.clone(), audit: a, store: Arc::new(RunStore::new(&dir)), chat: Arc::new(Script { turns: Mutex::new(VecDeque::new()) }), tools: Arc::new(FakeSource { executed: Arc::new(Mutex::new(vec![])) }), extras: extras.clone(), system_prompt: "t".into() });
    let chk = ServiceConfig { listen: "127.0.0.1:0".into(), data_dir: dir.to_string_lossy().into(), ..ServiceConfig::default() }.check().unwrap();
    let svc = server::start(&chk, deps, tls::self_signed(&dir.join("tls"), &[]).unwrap()).unwrap();
    let get = |p: &str| {
        let r = server::handle(svc.shared(), &req("GET", p, None, None), "10.0.0.1".parse().unwrap());
        (r.status, String::from_utf8_lossy(&r.body).to_string())
    };
    let (s, body) = get("/readyz");
    assert_eq!(s, 503);
    assert!(!body.contains("not yet"), "{body}");
    assert_eq!(get("/healthz").0, 200, "alive though not ready");
    extras.ready.store(true, Ordering::SeqCst);
    assert_eq!(get("/readyz").0, 200);
    // shutdown: the port stops answering, and a second call is harmless
    let addr = svc.local_addr();
    svc.shutdown(Duration::from_secs(2));
    svc.shutdown(Duration::from_secs(2));
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(), "nothing listens after shutdown");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tokens_are_kept_hashed_expire_and_survive_a_restart() {
    use super::auth::{hash_token, Tokens};
    let dir = crate::testdir::unique("webservice", "tokens");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tokens.json");
    let clock = Arc::new(Mutex::new(1_000_000u64));
    let c2 = clock.clone();
    let mk = || {
        let c = c2.clone();
        Tokens::with_clock(Some(path.clone()), Duration::from_secs(3600), 100, Box::new(move || *c.lock().unwrap()))
    };
    let t = mk();
    let tok = t.issue("ann").unwrap();
    assert!(tok.starts_with("iyk_") && tok.len() == 68);
    assert_ne!(tok, t.issue("ann").unwrap(), "every sign-in gets its own token");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains(&tok) && saved.contains(&hash_token(&tok)), "only the hash is stored");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    // a restart keeps the person signed in
    let t2 = mk();
    assert_eq!(t2.find(&tok).as_deref(), Some("ann"));
    assert!(t2.find(&tok[..60]).is_none() && t2.find("").is_none());
    // the absolute lifetime ends it, however busy
    *clock.lock().unwrap() += 101;
    assert!(t2.find(&tok).is_none());
    assert_eq!(mk().count(), 0, "expired tokens are not loaded");
    let t3 = mk();
    let a = t3.issue("ann").unwrap();
    let b = t3.issue("bob").unwrap();
    t3.revoke_user("ann");
    assert!(t3.find(&a).is_none() && t3.find(&b).is_some());
    t3.revoke(&b);
    assert!(t3.find(&b).is_none());
    // a damaged store means everybody signs in again, never a crash
    std::fs::write(&path, "garbage").unwrap();
    assert_eq!(mk().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_route_matcher_is_strict() {
    use routes::find;
    use routes::Found;
    assert!(matches!(find(Profile::AgentApi, "GET", "/api/v1/chat/runs/abc"), Found::Route(r, p) if r.id == "runs_get" && p == ["abc"]));
    for bad in ["/api/v1/chat/runs/", "/api/v1/chat/runs/a/b/c", "/api/v1/chat/runs/%2e%2e", "/api/v1/chat/runs/a b", "/api/v1/../healthz", "//healthz", "/healthz/", "/HEALTHZ", "/api/v1/chat/runs/..%2f"] {
        assert!(matches!(find(Profile::AgentApi, "GET", bad), Found::Nothing), "{bad}");
    }
}

// ------------------------------------------------------------------------------------------------ cloud profile: roles and namespaces

/// root bootstraps as owner; ann and bob join the tenant; root makes namespace `web` and puts ann in it as editor. gus is a stranger.
fn acme_cloud() -> (H, String, String, String, String) {
    let h = harness("cloud", |_| {});
    let (root, ann, bob, gus) = (h.root(), h.ann(), h.login("bob", "bob-password-1"), h.login("gus", "gus-password-1"));
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 200);
    for who in ["ann", "bob"] {
        assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": who, "role": "member"}))).0, 200);
    }
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "create"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "ann", "role": "editor"}))).0, 200);
    (h, root, ann, bob, gus)
}

#[test]
fn cloud_a_tenant_without_lists_denies_namespaces_and_only_a_service_admin_can_bootstrap_once() {
    let h = harness("cloud", |_| {});
    let (root, ann) = (h.root(), h.ann());
    for t in [&root, &ann] {
        assert_eq!(h.call("GET", "/api/v1/tenant", Some(t), None).0, 403);
        assert_eq!(h.call("POST", "/api/v1/data/call", Some(t), Some(json!({"tool": "data_query", "args": {}, "namespace": "web"}))).0, 403);
        assert_eq!(h.call("POST", "/api/v1/namespaces", Some(t), Some(json!({"name": "web", "action": "create"}))).0, 403);
    }
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&ann), None).0, 403, "not a service administrator");
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 200);
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 403, "only once");
    let (s, v, _) = h.call("GET", "/api/v1/tenant", Some(&root), None);
    assert_eq!((s, v["tenant_role"].as_str()), (200, Some("owner")));
}

#[test]
fn cloud_a_stranger_is_denied_everywhere_and_a_member_outside_a_namespace_gets_not_found() {
    let (h, root, ann, bob, gus) = acme_cloud();
    let read = |t: &str, ns: &str| h.call("POST", "/api/v1/data/call", Some(t), Some(json!({"tool": "data_query", "args": {}, "namespace": ns}))).0;
    // gus is signed in to the service but not a member of the tenant
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&gus), None).0, 403);
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&gus), None).0, 403);
    assert_eq!(h.call("GET", "/api/v1/flows/f1", Some(&gus), None).0, 403);
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&gus), Some(json!({"tool": "data_query", "args": {}}))).0, 403);
    assert_eq!(read(&gus, "web"), 403);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&gus), Some(json!({"name": "gus", "role": "owner"}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&gus), Some(json!({"name": "gus", "role": "admin"}))).0, 403);
    // bob is in the tenant, not in web: the same answer as for a namespace that is not there
    let (s1, v1, _) = h.call("POST", "/api/v1/data/call", Some(&bob), Some(json!({"tool": "data_query", "args": {}, "namespace": "web"})));
    let (s2, v2, _) = h.call("POST", "/api/v1/data/call", Some(&bob), Some(json!({"tool": "data_query", "args": {}, "namespace": "nothing"})));
    assert_eq!((s1, &v1), (s2, &v2));
    assert_eq!(s1, 404);
    let (s1, v1, _) = h.call("POST", "/api/v1/namespaces/web/members", Some(&bob), Some(json!({"name": "bob", "role": "admin"})));
    let (s2, v2, _) = h.call("POST", "/api/v1/namespaces/nothing/members", Some(&bob), Some(json!({"name": "bob", "role": "admin"})));
    assert_eq!((s1, &v1), (s2, &v2));
    assert_eq!(s1, 404);
    // bob's own list does not hold the name; ann's does; the owner sees it without being in it
    let names = |t: &str| -> Vec<String> { h.call("GET", "/api/v1/tenant", Some(t), None).1["namespaces"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect() };
    assert!(names(&bob).is_empty());
    assert_eq!(names(&ann), ["web"]);
    // root made web, so root administers it; the tenant members list is for admins only
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&bob), None).1["members"], json!([]));
    assert_eq!(h.call("GET", "/api/v1/tenant", Some(&root), None).1["members"].as_array().unwrap().len(), 3);
    // a plain member cannot change the tenant
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&bob), Some(json!({"name": "gus", "role": "member"}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&bob), Some(json!({"name": "mine", "action": "create"}))).0, 403);
}

#[test]
fn cloud_a_tenant_admin_sees_names_but_cannot_read_and_roles_decide_who_reads() {
    let (h, root, ann, bob, _) = acme_cloud();
    // adam-like admin: bob becomes a tenant admin, makes a namespace nobody else is in, root is not in it
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "bob", "role": "admin"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&bob), Some(json!({"name": "secret", "action": "create"}))).0, 200);
    let (_, v, _) = h.call("GET", "/api/v1/tenant", Some(&root), None);
    assert!(v["namespaces"].as_array().unwrap().iter().any(|n| n["name"] == "secret" && n["joined"] == false), "{v}");
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&root), Some(json!({"tool": "data_query", "args": {}, "namespace": "secret"}))).0, 404, "seen, not readable");
    // ann (editor of web) reads
    let (s, v, _) = h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": {"path": "notes/a.csv"}, "namespace": "web"})));
    assert_eq!(s, 200, "{v}");
    let r = v["result"].as_str().unwrap();
    assert!(r.contains("ns/web/people/ann/notes/a.csv"), "the path comes from the layout: {r}");
    // viewers read and cannot change who is in the namespace; an editor cannot either
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "viewer"}))).0, 200);
    let (s, _, _) = h.call("POST", "/api/v1/data/call", Some(&bob), Some(json!({"tool": "data_query", "args": {}, "namespace": "web"})));
    assert_eq!(s, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&bob), Some(json!({"name": "bob", "role": "admin"}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&ann), Some(json!({"name": "bob", "role": "editor"}))).0, 403);
    // removing a person from the tenant takes them out of the namespace at once
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "ann", "role": "remove"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": {}, "namespace": "web"}))).0, 403);
}

#[test]
fn cloud_paths_that_leave_the_namespace_folder_are_refused() {
    let (h, _, ann, _, _) = acme_cloud();
    for bad in ["../mia/secret", "/etc/passwd", "a/../../b"] {
        let (s, v, _) = h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": {"path": bad}, "namespace": "web"})));
        assert_eq!(s, 400, "{bad}: {v}");
    }
    #[cfg(unix)]
    {
        let base = h.dir.join("cloud").join("files").join("ns").join("web").join("people").join("ann");
        std::os::unix::fs::symlink("/", base.join("rootlink")).unwrap();
        let (s, _, _) = h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": {"path": "rootlink/etc/passwd"}, "namespace": "web"})));
        assert_eq!(s, 400, "a link is not followed");
    }
    // a namespace name that is not a folder name never reaches the disk
    let (s, _, _) = h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": {}, "namespace": "../x"})));
    assert!(s == 404 || s == 400, "{s}");
}

#[test]
fn cloud_every_change_is_audited_in_the_roles_audit_file_and_refused_ones_are_not() {
    let (h, root, _, bob, _) = acme_cloud();
    let file = crate::cloudroles::audit_path(&h.dir);
    let n = std::fs::read_to_string(&file).unwrap().lines().count();
    assert_eq!(n, 5, "bootstrap, two members, a namespace, a namespace member");
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&bob), Some(json!({"name": "bob", "role": "owner"}))).0, 403);
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), n);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "bob", "role": "bogus"}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "create"}))).0, 400, "exists");
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "delete"}))).0, 200);
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), n + 1);
}

#[test]
fn the_roles_routes_do_not_exist_outside_the_cloud_profile_and_the_other_profile_is_unchanged() {
    let h = harness("agent-api", |_| {});
    let t = h.root();
    for (m, p) in [("GET", "/api/v1/tenant"), ("POST", "/api/v1/tenant/bootstrap"), ("POST", "/api/v1/tenant/members"), ("POST", "/api/v1/namespaces"), ("POST", "/api/v1/namespaces/web/members")] {
        assert_eq!(h.call(m, p, Some(&t), Some(json!({"x": 1})).filter(|_| m == "POST")).0, 404, "{p}");
    }
    assert!(!h.dir.join("cloud").exists(), "nothing was written");
    // the cloud profile with no lists keeps the personal routes as they were
    let c = harness("cloud", |_| {});
    let ct = c.ann();
    assert_eq!(c.call("GET", "/api/v1/flows", Some(&ct), None).0, 200);
    assert_eq!(c.call("POST", "/api/v1/data/call", Some(&ct), Some(json!({"tool": "data_query", "args": {}}))).1["result"], "data_query for ann");
}

// ------------------------------------------------------------------------------------------------ cloud profile: the vault link

/// The only test that sets the vault variables (the process environment is shared by the parallel tests).
#[test]
fn cloud_removal_keeps_the_role_change_and_records_a_pending_rotation_and_readyz_names_the_vault_state() {
    let (h, root, _ann, bob, _gus) = acme_cloud();
    // no vault configured: ready, the word says so, a removal has nothing to rotate
    std::env::remove_var("IYAKKI_VAULT_URL");
    let (s, v, _) = h.call("GET", "/readyz", None, None);
    assert_eq!((s, v["vault"].as_str()), (200, Some("not_configured")));
    let (s, v, _) = h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "ann", "role": "remove"})));
    assert_eq!(s, 200);
    assert_eq!(v["rotation"]["pending"], json!([]));
    // a vault that does not answer: readiness stays 200, the word is unreachable and does not leak the address
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let url = format!("https://127.0.0.1:{port}");
    std::env::set_var("IYAKKI_VAULT_URL", &url);
    std::env::set_var("IYAKKI_TENANT", "acme");
    let (s, v, resp) = h.call("GET", "/readyz", None, None);
    let body = String::from_utf8_lossy(&resp.body).to_string();
    assert_eq!((s, v["vault"].as_str()), (200, Some("unreachable")));
    assert!(!body.contains(&port.to_string()) && !body.contains("127.0.0.1"), "{body}");
    // a removal from the tenant: the role change stands, the rotation is pending, the person is told what is pending
    let (s, v, _) = h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "bob", "role": "remove"})));
    std::env::remove_var("IYAKKI_VAULT_URL");
    std::env::remove_var("IYAKKI_TENANT");
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["rotation"]["pending"], json!(["tenant"]));
    assert_ne!(h.call("GET", "/api/v1/tenant", Some(&bob), None).0, 200, "bob is out");
    let pending = crate::cloudrotate::load_pending(&h.dir).unwrap();
    assert_eq!((pending[0].scope.as_str(), pending[0].who.as_str()), ("tenant", "bob"));
    // a bad address is "invalid_address", and nothing shows the text
    std::env::set_var("IYAKKI_VAULT_URL", "http://not-https");
    let (_, v, resp) = h.call("GET", "/readyz", None, None);
    std::env::remove_var("IYAKKI_VAULT_URL");
    assert_eq!(v["vault"], "invalid_address");
    assert!(!String::from_utf8_lossy(&resp.body).contains("not-https"));
    // the agent profile has no vault detail at all
    let a = harness("agent-api", |_| {});
    assert!(a.call("GET", "/readyz", None, None).1.get("vault").is_none());
}

#[path = "files_tests.rs"]
mod files_tests;
