//! The webhook endpoint through the real web service: a real TLS listener on 127.0.0.1 answers `/hooks/<id>/<token>` with 202 and a run
//! id, with no bearer token, and the other cases go through the same pipeline `server::handle` gives every request.

use super::config::ServiceConfig;
use super::deps::{Deps, Extras, ToolSource};
use super::server::{self, Service};
use super::{tls, Profile};
use crate::agent::{ToolDef, Tools};
use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::flows::hookstore::{token_hash, HookRec, HookStore};
use crate::flows::webhook::{sign, Answer, Content, HookCfg, HookRequest, HookWorld, Hooks};
use crate::httpmini::Request;
use crate::runs::RunStore;
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
use iyakki_core_fake::FakeAccounts;
use iyakki_sec::policy::Role;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOKEN: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const SECRET: &[u8] = b"a shared secret for the loopback test";

struct NoChat;
impl ChatBackend for NoChat {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no".into())
    }
    fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        Err("no".into())
    }
}

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, _: &str, _: &Value) -> Result<String, String> {
        Err("none".into())
    }
    fn describe(&self, n: &str, _: &Value) -> String {
        n.into()
    }
}
struct Src;
impl ToolSource for Src {
    fn tools_for(&self, _: &Principal) -> Arc<dyn Tools> {
        Arc::new(NoTools)
    }
}

struct World<'a> {
    audit: &'a dyn AuditSink,
    started: &'a Mutex<Vec<Value>>,
}
impl HookWorld for World<'_> {
    fn owner_allowed(&self, _: &str) -> bool {
        true
    }
    fn config(&self, rec: &HookRec) -> Option<HookCfg> {
        Some(HookCfg { trigger: rec.trigger.clone(), methods: vec!["POST".into()], max_body: 2048, content: vec![Content::Json], rate_per_min: 100, signed: true, inputs: json!({}) })
    }
    fn secret(&self, _: &HookRec) -> Result<Vec<u8>, String> {
        Ok(SECRET.to_vec())
    }
    fn start(&self, _: &HookRec, _: &HookCfg, inputs: Value, _: Option<String>) -> Result<(String, bool), String> {
        let mut s = self.started.lock().unwrap();
        s.push(inputs);
        Ok((format!("r{}", s.len()), false))
    }
    fn audit(&self, owner: &str, event: &str, detail: &str) {
        self.audit.record(owner, event, detail);
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

struct HookExtras {
    hooks: Hooks,
    started: Mutex<Vec<Value>>,
    managed: Mutex<Vec<String>>,
}

impl Extras for HookExtras {
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
    fn models(&self, _: &Principal) -> Vec<Value> {
        vec![]
    }
    fn backend_for(&self, _: &Principal, _: &str, _: &str) -> ApiResult<Arc<dyn ChatBackend>> {
        Err(ApiError::Unsupported("no".into()))
    }
    fn flows(&self, _: &Principal) -> ApiResult<Vec<Value>> {
        Ok(vec![])
    }
    fn flow_export(&self, _: &Principal, _: &str) -> ApiResult<Value> {
        Err(ApiError::NotFound("no".into()))
    }
    fn data_call(&self, _: &Principal, _: &str, _: &Value) -> ApiResult<String> {
        Err(ApiError::Unsupported("no".into()))
    }
    fn hook_serve(&self, _: &dyn Authenticator, audit: &dyn AuditSink, rq: &HookRequest) -> Option<Answer> {
        Some(self.hooks.serve(&World { audit, started: &self.started }, rq))
    }
    fn hook_manage(&self, p: &Principal, op: super::deps::HookOp<'_>) -> ApiResult<Value> {
        use super::deps::HookOp;
        self.managed.lock().unwrap().push(format!("{} {op:?}", p.user));
        match op {
            HookOp::Create { flow, .. } => Ok(json!({"id": "hk-new", "flow": flow, "path": format!("/hooks/hk-new/{TOKEN}"), "secret": null, "shown_once": true})),
            HookOp::Regenerate(id) => Ok(json!({"id": id, "path": format!("/hooks/{id}/{TOKEN}"), "secret": null, "shown_once": true})),
            HookOp::Reveal => Ok(json!({"waiting": false})),
            HookOp::List(_) => Ok(json!({"hooks": []})),
            HookOp::Enable(id) | HookOp::Disable(id) => Ok(json!({"id": id, "enabled": true})),
            HookOp::Public(id, m) => Ok(json!({"id": id, "public_until": m, "open_to_public": m > 0})),
        }
    }
}

struct Rig {
    svc: Service,
    accounts: Arc<FakeAccounts>,
    extras: Arc<HookExtras>,
    dir: std::path::PathBuf,
}

fn rig(profile: &str, enabled: bool) -> Rig {
    let dir = crate::testdir::unique("hooksvc", &format!("{profile}-{}", std::thread::current().name().unwrap_or("t")));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = ServiceConfig { profile: profile.into(), listen: "127.0.0.1:0".into(), data_dir: dir.to_string_lossy().into(), ..ServiceConfig::default() };
    let chk = cfg.check().expect("config");
    let accounts = Arc::new(FakeAccounts::new(&[("ann", "ann-password-1", Role::User)]));
    let store = Arc::new(HookStore::open(&dir.join("hooks.json")));
    store.insert(HookRec { id: "hk-loop".into(), owner: "ann".into(), flow: "deploy".into(), trigger: "hk-loop".into(), token_hash: token_hash(TOKEN), enabled, created: 1, rotated: 0, public_until: 0, has_secret: true, deliveries: vec![] }).unwrap();
    let extras = Arc::new(HookExtras { hooks: Hooks::new(store), started: Mutex::new(vec![]), managed: Mutex::new(vec![]) });
    let deps = Arc::new(Deps { auth: accounts.clone(), audit: accounts.clone(), store: Arc::new(RunStore::new(&dir.join("runs"))), chat: Arc::new(NoChat), tools: Arc::new(Src), extras: extras.clone(), system_prompt: "t".into() });
    let id = tls::self_signed(&dir.join("tls"), &[]).expect("certificate");
    let svc = server::start(&chk, deps, id).expect("start");
    Rig { svc, accounts, extras, dir }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.svc.shutdown(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn post(r: &Rig, path: &str, body: &[u8], headers: &[(&str, String)]) -> (u16, Value) {
    let h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let rep = crate::devmode::client::https("127.0.0.1", r.svc.local_addr().port(), Some(r.svc.fingerprint()), "POST", path, &h, body, Duration::from_secs(5)).expect("the request goes through");
    (rep.status, serde_json::from_slice(&rep.body).unwrap_or(Value::Null))
}

fn signed(body: &[u8], ts: i64) -> Vec<(&'static str, String)> {
    vec![("Content-Type", "application/json".into()), ("X-Iyakki-Timestamp", ts.to_string()), ("X-Iyakki-Signature", sign(SECRET, ts, body))]
}

#[test]
fn a_signed_call_over_real_tls_starts_a_run_with_no_bearer_token_and_the_audit_has_no_secrets() {
    let r = rig("agent-api", true);
    let now = crate::clock::now_unix();
    let body = br#"{"event":"push","ref":"main"}"#;
    let path = format!("/hooks/hk-loop/{TOKEN}");
    let (s, v) = post(&r, &path, body, &signed(body, now));
    assert_eq!((s, v["run"].as_str()), (202, Some("r1")), "{v}");
    assert_eq!(r.extras.started.lock().unwrap()[0]["webhook"]["body"]["ref"], "main");
    // the same request again is a replay; a call with a wrong token, an unsigned one and a changed body start nothing
    assert_eq!(post(&r, &path, body, &signed(body, now)).0, 409);
    assert_eq!(post(&r, &format!("/hooks/hk-loop/{}", TOKEN.replace('a', "b")), body, &signed(body, now + 1)).0, 404);
    assert_eq!(post(&r, &path, body, &[("Content-Type", "application/json".into())]).0, 401);
    let mut tampered = signed(body, now + 2);
    tampered[2].1 = sign(SECRET, now + 2, b"{}");
    assert_eq!(post(&r, &path, body, &tampered).0, 401);
    // too large for this hook's cap (2048): refused as such, even when well signed
    let big = vec![b' '; 3000];
    assert_eq!(post(&r, &path, &big, &signed(&big, now + 3)).0, 413);
    assert_eq!(r.extras.started.lock().unwrap().len(), 1);
    let log = r.accounts.lines().join("\n");
    assert!(log.contains("hook.fire hook=hk-loop flow=deploy run=r1"), "{log}");
    for secret in [TOKEN, "a shared secret", "sha256=", "push", "main"] {
        assert!(!log.contains(secret), "{secret} in the audit log");
    }
}

#[test]
fn hostile_requests_to_the_endpoint_get_plain_404_405_and_never_reach_the_flow() {
    let r = rig("agent-api", true);
    let shared = r.svc.shared();
    let peer = "10.1.1.1".parse().unwrap();
    let req = |method: &str, path: &str, body: &[u8]| Request { method: method.into(), path: path.into(), query: HashMap::new(), headers: [("host".to_string(), "svc.test".to_string()), ("content-type".into(), "application/json".into())].into(), body: body.to_vec() };
    let ask = |m: &str, p: &str, b: &[u8]| server::handle(shared, &req(m, p, b), peer).status;
    let good = format!("/hooks/hk-loop/{TOKEN}");
    assert_eq!(ask("GET", &good, b""), 405, "the hook takes POST only");
    assert_eq!(ask("DELETE", &good, b""), 405);
    for bad in ["/hooks/", "/hooks/hk-loop", "/hooks/hk-loop/", &format!("{good}/extra"), "/hooks/../etc/passwd", "/hooks/hk-loop/%2e%2e", "/hooks//x", &format!("/hooks/{}/{TOKEN}", "a".repeat(80)), "/hooks/hk loop/abc"] {
        assert_eq!(ask("POST", bad, b"{}"), 404, "{bad}");
    }
    assert_eq!(r.extras.started.lock().unwrap().len(), 0);
    // an Origin from another site is refused before anything (a browser page cannot fire a hook)
    let mut cross = req("POST", &good, b"{}");
    cross.headers.insert("origin".into(), "https://evil.example".into());
    assert_eq!(server::handle(shared, &cross, peer).status, 403);
    // the answer to a refused call has no cache and carries no hint of which part was wrong
    let a = server::handle(shared, &req("POST", "/hooks/hk-loop/00", b"{}"), peer);
    let b = server::handle(shared, &req("POST", "/hooks/nope/00", b"{}"), peer);
    assert_eq!(a.body, b.body);
    assert!(a.headers.iter().any(|(k, v)| k == "Cache-Control" && v == "no-store"));
}

#[test]
fn a_hook_that_is_off_and_the_profiles_without_hooks_answer_404() {
    let off = rig("agent-api", false);
    let now = crate::clock::now_unix();
    let body = b"{}";
    assert_eq!(post(&off, &format!("/hooks/hk-loop/{TOKEN}"), body, &signed(body, now)).0, 404);
    assert_eq!(off.extras.started.lock().unwrap().len(), 0);
    for profile in ["flows-runner", "data-api", "tile-host"] {
        let r = rig(profile, true);
        assert_eq!(post(&r, &format!("/hooks/hk-loop/{TOKEN}"), body, &signed(body, now)).0, 404, "{profile} has no webhooks");
        assert_eq!(r.extras.started.lock().unwrap().len(), 0);
    }
}

#[test]
fn a_tenant_pod_takes_a_hook_from_the_gateways_forwarded_sender_and_judges_local_and_outside_by_it() {
    let cloud = rig("cloud", true);
    assert_eq!(cloud.svc.shared().profile, Profile::Cloud);
    let shared = cloud.svc.shared();
    let gateway: std::net::IpAddr = "10.42.0.7".parse().unwrap();
    let body = br#"{"event":"push"}"#;
    let now = crate::clock::now_unix();
    let good = format!("/hooks/hk-loop/{TOKEN}");
    let ask = |fwd: Option<&str>, ts: i64| {
        let mut h: HashMap<String, String> = [("host".to_string(), "svc.test".to_string()), ("content-type".into(), "application/json".into()), ("x-iyakki-timestamp".into(), ts.to_string()), ("x-iyakki-signature".into(), sign(SECRET, ts, body))].into();
        if let Some(f) = fwd {
            h.insert("x-forwarded-for".into(), f.into());
        }
        server::handle(shared, &Request { method: "POST".into(), path: good.clone(), query: HashMap::new(), headers: h, body: body.to_vec() }, gateway)
    };
    // a sender on the private network behind the gateway starts the run
    assert_eq!(ask(Some("192.168.1.20"), now).status, 202);
    // a sender outside is refused (the hook is not opened to the public), and so is a request with no forwarded address at all
    assert_eq!(ask(Some("203.0.113.9"), now + 1).status, 403);
    assert_eq!(ask(None, now + 2).status, 403);
    // a client's own earlier claim of a private address does not help: the gateway's entry is the last one
    assert_eq!(ask(Some("192.168.1.20, 203.0.113.9"), now + 3).status, 403);
    assert_eq!(cloud.extras.started.lock().unwrap().len(), 1);
    let log = cloud.accounts.lines().join("\n");
    assert!(log.contains("outcome=outside_network addr=203.0.113.9"), "{log}");
    assert!(!log.contains(TOKEN));
}

/// A real external sender: `curl` posts to the live TLS listener and `openssl` (a second implementation of HMAC-SHA256) makes the signature,
/// the way a CI system or a git host would. Skips with a sentence when either tool is missing.
#[test]
fn curl_signed_with_openssl_starts_a_run_and_a_bad_signature_starts_nothing() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let have = |t: &str| Command::new(t).arg("version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() || Command::new(t).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok();
    if !have("curl") || !have("openssl") {
        crate::qeprintln!("curl or openssl is not installed here: the real sender test is skipped");
        return;
    }
    let r = rig("agent-api", true);
    let port = r.svc.local_addr().port();
    let body = br#"{"event":"push","ref":"release"}"#;
    // openssl signs "<timestamp>.<body>" with the shared secret (read from its standard input, hex key)
    let sign_with_openssl = |ts: i64, body: &[u8]| -> String {
        let mut msg = format!("{ts}.").into_bytes();
        msg.extend_from_slice(body);
        let key: String = SECRET.iter().map(|b| format!("{b:02x}")).collect();
        let mut c = Command::new("openssl").args(["dgst", "-sha256", "-mac", "HMAC", "-macopt", &format!("hexkey:{key}"), "-r"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("openssl");
        c.stdin.take().unwrap().write_all(&msg).unwrap();
        let out = c.wait_with_output().unwrap();
        format!("sha256={}", String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or(""))
    };
    let curl = |ts: i64, sig: &str, body: &[u8]| -> (String, String) {
        let url = format!("https://127.0.0.1:{port}/hooks/hk-loop/{TOKEN}");
        let mut c = Command::new("curl").args(["-sk", "-m", "10", "-o", "-", "-w", "\n%{http_code}", "-X", "POST", "-H", "Content-Type: application/json", "-H", &format!("X-Iyakki-Timestamp: {ts}"), "-H", &format!("X-Iyakki-Signature: {sig}"), "--data-binary", "@-", &url]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("curl");
        c.stdin.take().unwrap().write_all(body).unwrap();
        let out = String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).to_string();
        let (b, code) = out.rsplit_once('\n').unwrap_or((&out, ""));
        (code.trim().to_string(), b.to_string())
    };
    let ts = crate::clock::now_unix();
    let good = sign_with_openssl(ts, body);
    assert_eq!(good, sign(SECRET, ts, body), "openssl and the in-house HMAC agree");
    let (code, answer) = curl(ts, &good, body);
    assert_eq!(code, "202", "{answer}");
    assert_eq!(r.extras.started.lock().unwrap().len(), 1);
    assert_eq!(r.extras.started.lock().unwrap()[0]["webhook"]["body"]["ref"], "release");
    // the same call again is a replay; a signature over another body is refused; nothing more started
    assert_eq!(curl(ts, &good, body).0, "409");
    let (code, _) = curl(ts + 1, &sign_with_openssl(ts + 1, b"{}"), body);
    assert_eq!(code, "401");
    assert_eq!(r.extras.started.lock().unwrap().len(), 1);
}

fn api(r: &Rig, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut h = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let Some(t) = token {
        h.push(("Authorization".into(), format!("Bearer {t}")));
    }
    let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
    let rep = crate::devmode::client::https("127.0.0.1", r.svc.local_addr().port(), Some(r.svc.fingerprint()), method, path, &h, &bytes, Duration::from_secs(5)).expect("the request goes through");
    (rep.status, serde_json::from_slice(&rep.body).unwrap_or(Value::Null))
}

#[test]
fn the_cloud_profile_has_the_webhook_routes_for_the_person_and_the_audit_never_has_the_address() {
    let r = rig("cloud", true);
    let (s, v) = api(&r, "POST", "/api/v1/login", None, Some(json!({"name": "ann", "password": "ann-password-1"})));
    assert_eq!(s, 200, "{v}");
    let t = v["token"].as_str().unwrap().to_string();
    // no token: no hooks API
    assert_eq!(api(&r, "GET", "/api/v1/hooks", None, None).0, 401);
    assert_eq!(api(&r, "GET", "/api/v1/hooks", Some(&t), None).1, json!({"hooks": []}));
    let (s, v) = api(&r, "POST", "/api/v1/hooks", Some(&t), Some(json!({"flow": "deploy", "signed": false, "methods": ["post"]})));
    assert_eq!((s, v["id"].as_str(), v["path"].as_str().map(|p| p.len())), (200, Some("hk-new"), Some("/hooks/hk-new/".len() + 64)), "{v}");
    assert_eq!(api(&r, "POST", "/api/v1/hooks/hk-new/regenerate", Some(&t), None).0, 200);
    assert_eq!(api(&r, "POST", "/api/v1/hooks/hk-new/enable", Some(&t), None).0, 200);
    assert_eq!(api(&r, "POST", "/api/v1/hooks/hk-new/disable", Some(&t), None).0, 200);
    assert_eq!(api(&r, "POST", "/api/v1/hooks/hk-new/public", Some(&t), Some(json!({"minutes": 30}))).1["open_to_public"], true);
    assert_eq!(api(&r, "GET", "/api/v1/hooks/reveal", Some(&t), None).1["waiting"], false);
    // malformed requests never reach the machine
    let before = r.extras.managed.lock().unwrap().len();
    assert_eq!(api(&r, "POST", "/api/v1/hooks", Some(&t), Some(json!({"flow": "deploy", "bogus": 1}))).0, 400);
    assert_eq!(api(&r, "POST", "/api/v1/hooks", Some(&t), Some(json!({"flow": ""}))).0, 400);
    assert_eq!(api(&r, "POST", "/api/v1/hooks/hk-new/public", Some(&t), Some(json!({"minutes": "soon"}))).0, 400);
    assert_eq!(api(&r, "POST", "/api/v1/hooks/bad!id/enable", Some(&t), None).0, 404);
    assert_eq!(r.extras.managed.lock().unwrap().len(), before);
    // the operations were made as the signed-in person
    assert!(r.extras.managed.lock().unwrap().iter().all(|l| l.starts_with("ann ")));
    // the audit log names the hook and the flow, never the address
    let log = r.accounts.lines().join("\n");
    assert!(log.contains("hooks_create") && log.contains("hook=hk-new flow=deploy"), "{log}");
    assert!(!log.contains(TOKEN) && !log.contains("/hooks/hk-new/"), "{log}");
    // the agent API has none of these routes
    let agent = rig("agent-api", true);
    let (_, v) = api(&agent, "POST", "/api/v1/login", None, Some(json!({"name": "ann", "password": "ann-password-1"})));
    assert_eq!(api(&agent, "GET", "/api/v1/hooks", Some(v["token"].as_str().unwrap()), None).0, 404);
}
