//! The core contract (docs/development/CORE-CONTRACT.md) over developer mode's real TLS channel on loopback: the real listener, the
//! real signing client, the real adapters over a temporary device. Off by default; hello and version negotiation; scopes granted per
//! computer by an administrator on the device; audit lines without values; event subscribe and resume from a sequence number; a
//! remote client that cannot answer an unlock or a consent question (`needs_consent`); chunked streaming of a large payload.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::contract_local::core_for;
use iyakki_agent::core_contract::{CallContext, ConsentAnswer, ConsentKind, Consents, LocalSeat, RunId, Scope};
use iyakki_agent::devmode::client::{self, DevClient, Home};
use iyakki_agent::devmode::DevMode;
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::devserver::DevServer;
use iyakki_sec::policy::{Policy, Role};
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;
use serde_json::{json, Value};

struct Offline;
impl Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

struct Fx {
    env: ShellEnv,
    dir: PathBuf,
    server: DevServer,
    dm: DevMode,
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.server.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    let d = std::env::temp_dir().join(format!("iyakki-core-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    store.add_local("bob", "bobpass1234", Role::User).unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    let env = ShellEnv {
        store: Arc::new(Mutex::new(store)),
        audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
        host: Arc::new(Host),
        cfg: iyakki_agent::config::AiConfig::parse("").unwrap(),
        chat: Arc::new(iyakki_agent::chat::Unconfigured),
        mcp_servers: vec![],
        services: iyakki_agent::services::Services::open(&d.join("svc"), up.clone(), up).unwrap(),
        web: Arc::new(iyakki_web::fetch::NoNetwork),
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    };
    let id = iyakki_remote::tls::identity(&iyakki_agent::devmode::tls_dir(env.services.dir()), &[]).unwrap();
    let server = DevServer::start(env.share(), "127.0.0.1:0", &id).unwrap();
    let dm = DevMode::open(env.services.dir());
    Fx { env, dir: d, server, dm }
}

fn admin() -> Policy {
    Role::Admin.template()
}

/// Pair a computer called `name` acting as `account`.
fn paired(f: &Fx, name: &str, account: &str) -> DevClient {
    let h = Home::at(&f.dir.join(format!("home-{name}")));
    let p = f.dm.start_pairing_as(&admin(), "root", account).unwrap();
    let d = client::pair(&h, "127.0.0.1", f.server.local_addr().port(), name, &p.code, None, Some(name)).unwrap();
    DevClient::new(&h, d).unwrap()
}

fn grant(f: &Fx, computer: &str, scopes: &[&str]) {
    for s in scopes {
        f.dm.set_core_scope(&admin(), "root", computer, s, true).unwrap();
    }
}

/// POST a JSON body to a contract route: (HTTP status, JSON answer).
fn post(c: &DevClient, path: &str, body: Value) -> (u16, Value) {
    c.call_status("POST", path, &[], body.to_string().as_bytes()).unwrap()
}

/// A contract call: the whole envelope.
fn call(c: &DevClient, domain: &str, name: &str, args: Value) -> Value {
    let (st, v) = post(c, "/v1/dev/core/call", json!({ "domain": domain, "call": name, "args": args }));
    assert_eq!(st, 200, "{v}");
    v
}

/// A call that must succeed: its result.
fn ok(c: &DevClient, domain: &str, name: &str, args: Value) -> Value {
    let v = call(c, domain, name, args);
    assert_eq!(v["ok"], true, "{domain}.{name}: {v}");
    v["result"].clone()
}

/// A call that must fail: its error code.
fn code(c: &DevClient, domain: &str, name: &str, args: Value) -> String {
    let v = call(c, domain, name, args);
    assert_eq!(v["ok"], false, "{domain}.{name} should have been refused: {v}");
    v["error"]["code"].as_str().unwrap().to_string()
}

#[test]
fn it_is_off_by_default_per_device_and_a_new_computer_holds_no_contract_scope() {
    let f = fx("off");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let c = paired(&f, "laptop", "root");
    assert!(!f.dm.core_enabled(), "off by default");
    let (st, v) = post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 1}, "wants": ["settings"]}));
    assert_eq!(st, 403, "{v}");
    assert!(v["error"].as_str().unwrap().contains("switched off"));
    assert_eq!(post(&c, "/v1/dev/core/call", json!({"domain": "settings", "call": "schema"})).0, 403);
    // only an administrator switches it on
    assert!(f.dm.set_core_enabled(&Role::User.template(), "bob", true).is_err());
    assert!(f.dm.set_core_scope(&Role::User.template(), "bob", "laptop", Scope::SETTINGS_READ, true).is_err());
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    assert_eq!(post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 1}, "wants": []})).0, 200);
    // a new computer holds nothing: every call is denied until an administrator grants a scope on the device
    assert!(f.dm.core_scopes("laptop").is_empty());
    assert_eq!(code(&c, "settings", "schema", json!({})), "denied");
    assert_eq!(code(&c, "flows", "list", json!({})), "denied");
    assert_eq!(code(&c, "vault", "status", json!({})), "denied");
    // a scope that is not grantable (local) and a computer that is not paired are refused
    assert!(f.dm.set_core_scope(&admin(), "root", "laptop", Scope::LOCAL, true).is_err());
    assert!(f.dm.set_core_scope(&admin(), "root", "ghost", Scope::SETTINGS_READ, true).is_err());
    // the request cannot grant itself anything
    let (_, v) = post(&c, "/v1/dev/core/call", json!({"domain": "settings", "call": "schema", "scopes": ["settings:read"], "args": {"scopes": ["settings:read"]}}));
    assert_eq!(v["error"]["code"], "denied");
    // switching it off again closes every route
    f.dm.set_core_enabled(&admin(), "root", false).unwrap();
    assert_eq!(post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 1}, "wants": []})).0, 403);
}

#[test]
fn hello_negotiates_the_version_and_tells_a_client_which_domains_it_can_use() {
    let f = fx("hello");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let c = paired(&f, "laptop", "root");
    grant(&f, "laptop", &[Scope::SETTINGS_READ]);
    // a client of the same version, wanting one domain the machine has and one it does not
    let (st, v) = post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 1}, "wants": ["settings", "flows", "browser"]}));
    assert_eq!(st, 200);
    let r = &v["result"];
    assert_eq!(r["hello"]["contract"], json!({"major": 0, "minor": 1}));
    assert_eq!(r["hello"]["machine"], "iyakki-test");
    assert_eq!(r["negotiated"]["version"], json!({"major": 0, "minor": 1}));
    assert_eq!(r["negotiated"]["usable"], json!(["settings", "flows"]));
    assert_eq!(r["negotiated"]["missing"], json!(["browser"]), "the client hides what is missing instead of failing");
    assert_eq!(r["scopes"], json!(["settings:read"]), "and sees what this computer may do");
    let domains: Vec<&str> = r["hello"]["domains"].as_array().unwrap().iter().map(|d| d["domain"].as_str().unwrap()).collect();
    assert_eq!(domains, ["settings", "vault", "flows", "consent"]);
    assert!(r["hello"]["rate_limit"]["calls"].as_u64().unwrap() > 0);
    // a newer client minor still works at the machine's version; another major gets no calls at all; one too old gets none either
    let (_, newer) = post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 9}, "wants": ["settings"]}));
    assert_eq!(newer["result"]["negotiated"]["version"], json!({"major": 0, "minor": 1}));
    for old in [json!({"major": 1, "minor": 0}), json!({"major": 0, "minor": 0})] {
        let (_, v) = post(&c, "/v1/dev/core/hello", json!({"contract": old, "wants": ["settings"]}));
        assert_eq!(v["result"]["negotiated"]["version"], Value::Null);
        assert_eq!(v["result"]["negotiated"]["usable"], json!([]));
    }
    // a call that names its contract is refused across a major, and calls that do not exist say so
    let (_, v) = post(&c, "/v1/dev/core/call", json!({"contract": {"major": 1, "minor": 0}, "domain": "settings", "call": "schema"}));
    assert_eq!(v["error"]["code"], "version_mismatch");
    assert_eq!(code(&c, "settings", "explode", json!({})), "unsupported");
    assert_eq!(code(&c, "browser", "tabs", json!({})), "unsupported");
    assert_eq!(ok(&c, "settings", "schema", json!({})).as_array().unwrap().len() >= 7, true);
    // a bad body is a bad request, not a crash
    assert_eq!(post(&c, "/v1/dev/core/call", json!({"call": "schema"})).1["error"]["code"], "invalid_input");
    assert_eq!(c.call_status("POST", "/v1/dev/core/call", &[], b"not json").unwrap().0, 400);
    // unsigned requests never get here
    let r = client::https("127.0.0.1", f.server.local_addr().port(), Some(f.server.fingerprint()), "POST", "/v1/dev/core/call", &[], b"{}", Duration::from_secs(10)).unwrap();
    assert_eq!(r.status, 401);
}

#[test]
fn scopes_are_enforced_per_computer_local_only_keys_stay_local_and_the_audit_names_keys_not_values() {
    let f = fx("scopes");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let laptop = paired(&f, "laptop", "root");
    let tablet = paired(&f, "tablet", "root");
    grant(&f, "laptop", &[Scope::SETTINGS_READ]);
    grant(&f, "tablet", &[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE]);
    assert_eq!(ok(&laptop, "settings", "get", json!({"key": "tool_log_days"})), json!({"type": "int", "value": 0}));
    // reading is not writing
    assert_eq!(code(&laptop, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 321}, "idempotency_key": "set-days-0001"})), "denied");
    // the other computer, with the write scope, may; the value reaches the device's own settings store
    let ch = ok(&tablet, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 321}, "idempotency_key": "set-days-0001"}));
    assert_eq!((ch["repeated"].clone(), ch["seq"].as_u64().is_some()), (json!(false), true));
    assert_eq!(f.env.services.settings.lock().unwrap().get().tool_log_days, 321);
    let again = ok(&tablet, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 321}, "idempotency_key": "set-days-0001"}));
    assert_eq!(again["repeated"], true, "idempotent");
    assert_eq!(code(&tablet, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 5}, "idempotency_key": "set-days-0001"})), "conflict");
    assert_eq!(code(&tablet, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 9999}})), "invalid_input");
    assert_eq!(code(&tablet, "settings", "get", json!({"key": "nope"})), "not_found");
    // the vault's own setting is the machine's: not for any remote computer, whatever it holds
    assert_eq!(code(&tablet, "settings", "set", json!({"key": "vault.lock_on_admin_logout", "value": {"type": "bool", "value": true}})), "denied");
    assert!(!f.env.services.settings.lock().unwrap().get().vault.lock_on_admin_logout);
    // taking a scope away takes effect on the next request
    f.dm.set_core_scope(&admin(), "root", "tablet", Scope::SETTINGS_WRITE, false).unwrap();
    assert_eq!(code(&tablet, "settings", "set", json!({"key": "tool_log_days", "value": {"type": "int", "value": 7}})), "denied");
    assert_eq!(f.dm.core_scopes("tablet"), [Scope::SETTINGS_READ]);
    // audit: who, over what, which call, outcome; the key, never the value
    let core = core_for(&f.env.services);
    let adapter = core.settings.audit();
    assert!(adapter.iter().any(|l| l == "tablet via devmode: settings.set tool_log_days [change] ok"), "{adapter:?}");
    assert!(adapter.iter().any(|l| l.starts_with("laptop via devmode: settings.set [change] refused: denied")), "{adapter:?}");
    let system = f.env.audit.recent(200);
    assert!(system.iter().any(|l| l.contains("tablet via devmode: settings.set tool_log_days [change] ok")), "the system audit log has it too");
    assert!(system.iter().any(|l| l.contains("devmode.core") && l.contains("settings.set [change]")));
    for l in adapter.iter().chain(system.iter()).chain(f.dm.activity(200).iter().map(|a| &a.detail)) {
        assert!(!l.contains("321"), "a value leaked: {l}");
    }
    assert!(f.dm.activity(200).iter().any(|a| a.who == "laptop" && a.action == "dev.core" && !a.ok), "refusals are in the developer-mode activity log");
}

#[test]
fn a_watcher_subscribes_waits_for_news_and_resumes_from_the_last_sequence_number() {
    let f = fx("events");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let c = paired(&f, "laptop", "root");
    grant(&f, "laptop", &[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE]);
    let set = |k: &str, v: Value| ok(&c, "settings", "set", json!({"key": k, "value": v}));
    // subscribe "from now": nothing old
    let first = ok(&c, "settings", "watch", json!({"resume": "latest"}));
    assert_eq!(first["events"], json!([]));
    let mut cursor = first["next"].as_u64().unwrap();
    set("show_all_ai", json!({"type": "bool", "value": true}));
    set("tool_log_days", json!({"type": "int", "value": 30}));
    let b = ok(&c, "settings", "watch", json!({"resume": {"after": cursor}}));
    let keys: Vec<&str> = b["events"].as_array().unwrap().iter().map(|e| e["body"]["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["show_all_ai", "tool_log_days"], "exactly what was missed, in order");
    let seqs: Vec<u64> = b["events"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs[0] < seqs[1] && seqs[0] > cursor);
    // resume from the first event's number: only the second
    let tail = ok(&c, "settings", "watch", json!({"resume": {"after": seqs[0]}}));
    assert_eq!(tail["events"].as_array().unwrap().len(), 1);
    // a page of one, then the next page: nothing skipped or repeated
    let p1 = ok(&c, "settings", "watch", json!({"resume": {"after": cursor}, "limit": 1}));
    let p2 = ok(&c, "settings", "watch", json!({"resume": {"after": p1["next"]}, "limit": 1}));
    assert_eq!((p1["events"].as_array().unwrap().len(), p2["events"][0]["seq"].as_u64()), (1, Some(seqs[1])));
    cursor = b["next"].as_u64().unwrap();
    // wait for news: the call returns when a change happens on the machine itself (the person changes a setting on the screen)
    let core = core_for(&f.env.services);
    let svc = f.env.services.clone();
    let waiter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        svc.settings.lock().unwrap().update(|s| s.media_download_max_mb = 77).unwrap();
        drop(core);
    });
    let started = Instant::now();
    let w = ok(&c, "settings", "watch", json!({"resume": {"after": cursor}, "wait_ms": 8000}));
    assert!(started.elapsed() < Duration::from_secs(7), "returned as soon as there was news");
    assert_eq!(w["events"][0]["body"]["key"], "media_download_max_mb");
    assert_eq!(w["events"][0]["body"]["by"], "machine");
    waiter.join().unwrap();
    // nothing new within the wait: an empty batch, the cursor unchanged
    let quiet = ok(&c, "settings", "watch", json!({"resume": {"after": w["next"]}, "wait_ms": 200}));
    assert_eq!((quiet["events"].clone(), quiet["next"].clone()), (json!([]), w["next"].clone()));
}

#[test]
fn a_remote_client_cannot_unlock_the_vault_or_answer_a_question_and_the_machine_shows_the_question() {
    let f = fx("consent");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let c = paired(&f, "laptop", "root");
    grant(&f, "laptop", &[Scope::VAULT_STATUS, Scope::VAULT_LOCK, Scope::VAULT_USE, Scope::CONSENT_READ, Scope::FLOWS_APPROVE]);
    // the person's vault is open at the machine; a remote computer may ask whether it is
    f.env.services.on_login("root", "rootpass12", true);
    f.env.services.user_set("root", "deploy-token", "S3CRET-VALUE-xyz-0042").unwrap();
    let st = ok(&c, "vault", "status", json!({}));
    assert_eq!(st, json!({"locked": false, "entries": 1}));
    // no action is registered: the caller can only name one, and nothing comes back
    assert_eq!(code(&c, "vault", "use_secret", json!({"action": "sign_in:git"})), "not_found");
    // locking is allowed with the scope; unlocking is not, with any scope, and asks the person at the machine
    assert_eq!(ok(&c, "vault", "lock", json!({}))["locked"], true);
    assert_eq!(code(&c, "vault", "use_secret", json!({"action": "sign_in:git"})), "locked");
    let (_, v) = post(&c, "/v1/dev/core/call", json!({"domain": "vault", "call": "unlock", "args": {"login_password": "rootpass12"}}));
    assert_eq!(v["error"]["code"], "needs_consent");
    assert_eq!(v["error"]["message_key"], "core.needs_local_person");
    assert_eq!(ok(&c, "vault", "status", json!({}))["locked"], true, "still locked: the password sent was ignored");
    // the machine's screen has the question; the remote side can see it, not answer it
    let asked = ok(&c, "consent", "pending", json!({}));
    assert_eq!(asked.as_array().unwrap().len(), 1);
    assert_eq!((asked[0]["kind"].as_str(), asked[0]["subject"].as_str()), (Some("vault_unlock"), Some("laptop")));
    let id = asked[0]["id"].as_str().unwrap().to_string();
    assert_eq!(code(&c, "vault", "unlock", json!({})), "needs_consent");
    assert_eq!(ok(&c, "consent", "pending", json!({})).as_array().unwrap().len(), 1, "asking again does not pile up questions");
    assert_eq!(code(&c, "consent", "answer", json!({"id": id, "answer": "allow"})), "needs_consent");
    let core = core_for(&f.env.services);
    assert!(core.consents.answered().is_empty(), "nothing was answered remotely");
    // a person at the machine can answer; nothing else could
    let seat = LocalSeat::claim(&CallContext::local("root", true)).unwrap();
    core.consents.answer(&seat, &id, ConsentAnswer::Deny).unwrap();
    assert_eq!(core.consents.answered(), [(id, ConsentAnswer::Deny, "root".to_string())]);
    assert_eq!(core.consents.ask(ConsentKind::Camera, "tile.scan", "consent.camera", "Use the camera?").len() > 1, true);
    // the audit trail never holds the password or the secret
    let all: Vec<String> = f.env.audit.recent(500).into_iter().chain(core.vault.audit()).chain(f.dm.activity(200).into_iter().map(|a| a.detail)).collect();
    assert!(all.iter().all(|l| !l.contains("rootpass12") && !l.contains("S3CRET-VALUE")), "{all:?}");
}

#[test]
fn flows_run_over_the_channel_and_a_large_frame_is_read_in_chunks_that_resume() {
    let f = fx("flows");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let c = paired(&f, "laptop", "root");
    let reader = paired(&f, "tablet", "root");
    grant(&f, "laptop", &[Scope::FLOWS_READ, Scope::FLOWS_RUN, Scope::FLOWS_APPROVE]);
    grant(&f, "tablet", &[Scope::SETTINGS_READ]);
    // a flow with an approval, made on the device by its owner
    let env = iyakki_agent::flows::FlowEnv::open(&f.env.services, "root", &admin()).unwrap();
    let spec = json!({"name": "Ask first", "description": "asks", "nodes": [{"id": "gate", "kind": "approval", "config": {"message": "Send the summary?"}}, {"id": "done", "kind": "end"}], "edges": [{"from": "gate", "to": "done", "port": "approved"}]});
    let flow = env.create_draft(iyakki_agent::flows::build_flow(&spec).unwrap(), "user:root").unwrap();
    env.store.publish(&flow.id, &iyakki_flow::store::PublishReq { by: "user:root", label: "", changelog: "x", approved_by: None }).unwrap();

    let list = ok(&c, "flows", "list", json!({}));
    assert!(list.as_array().unwrap().iter().any(|i| i["id"] == flow.id.as_str() && i["name"] == "Ask first"));
    assert_eq!(code(&c, "flows", "start", json!({"flow": flow.id})), "invalid_input", "a start needs an idempotency key");
    let cursor = ok(&c, "flows", "events", json!({"resume": "latest"}))["next"].as_u64().unwrap();
    let run = ok(&c, "flows", "start", json!({"flow": flow.id, "idempotency_key": "start-ask-0001"}));
    assert_eq!(ok(&c, "flows", "start", json!({"flow": flow.id, "idempotency_key": "start-ask-0001"})), run, "a repeat starts nothing new");
    // the run waits for its approval; the watcher is told
    let ev = ok(&c, "flows", "events", json!({"run": run, "resume": {"after": cursor}, "wait_ms": 5000}));
    assert_eq!(ev["events"][0]["body"]["type"], "started");
    let deadline = Instant::now() + Duration::from_secs(10);
    let st = loop {
        let st = ok(&c, "flows", "status", json!({"run": run}));
        if st["state"] == "waiting_approval" || Instant::now() > deadline {
            break st;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(st["state"], "waiting_approval", "{st}");
    let aid = st["pending_approval"]["id"].as_str().unwrap().to_string();
    // the same scoped computer may answer an ordinary approval remotely; a computer without the scope may not even read
    assert_eq!(code(&reader, "flows", "status", json!({"run": run})), "denied");
    let after = ok(&c, "flows", "approve", json!({"run": run, "approval_id": aid, "allow": true}));
    assert_ne!(after["state"], "waiting_approval");

    // a camera frame handed to the run: the event holds a handle, never the bytes
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 253) as u8).collect();
    let flows = core_for(&f.env.services).flows_for("root", &admin()).unwrap();
    let handle = flows.hand_frame(&RunId(run.as_str().unwrap().to_string()), "image/png", payload.clone()).unwrap();
    let ev = ok(&c, "flows", "events", json!({"run": run, "resume": {"after": 0}, "limit": 100}));
    let frame = ev["events"].as_array().unwrap().iter().find(|e| e["body"]["type"] == "frame").expect("a frame event");
    assert_eq!(frame["body"]["stream"]["id"], handle.id.0.as_str());
    assert_eq!((frame["body"]["stream"]["total"].clone(), frame["body"]["stream"]["max_chunk"].clone()), (json!(200_000), json!(16384)));
    assert!(ev.to_string().len() < 5_000, "the event is small: {} bytes", ev.to_string().len());
    // read it in chunks by offset; the "link" drops after three chunks and resumes at the offset reached
    let read = |cl: &DevClient, off: u64| post(cl, "/v1/dev/core/stream/read", json!({"id": handle.id.0, "offset": off, "max_len": 1 << 20}));
    let mut got: Vec<u8> = Vec::new();
    let mut off = 0u64;
    let mut n = 0;
    loop {
        let (st, v) = read(&c, off);
        assert_eq!((st, v["ok"].clone()), (200, json!(true)), "{v}");
        let ch = &v["result"];
        assert_eq!(ch["offset"], off);
        let data: Vec<u8> = ch["data"].as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as u8).collect();
        assert!(data.len() <= 16 * 1024);
        got.extend_from_slice(&data);
        off += data.len() as u64;
        n += 1;
        if n == 3 {
            // dropped link: a fresh read at the same offset repeats the same bytes (resume is harmless)
            let (_, again) = read(&c, off);
            assert_eq!(again["result"]["offset"], off);
        }
        if ch["last"] == true {
            break;
        }
    }
    assert_eq!((got.len(), n), (200_000, 13));
    assert_eq!(got, payload, "every byte, in order");
    // another computer without flows:read cannot read the handle, and a past-the-end offset is invalid
    assert_eq!(read(&reader, 0).1["error"]["code"], "denied");
    assert_eq!(read(&c, 200_001).1["error"]["code"], "invalid_input");
    assert_eq!(post(&c, "/v1/dev/core/stream/close", json!({"id": handle.id.0})).1["ok"], true);
    assert_eq!(read(&c, 0).1["error"]["code"], "not_found", "closed");
    // stop is harmless once finished
    let fin = ok(&c, "flows", "stop", json!({"run": run}));
    assert!(matches!(fin["state"].as_str(), Some("succeeded") | Some("stopped") | Some("failed")), "{fin}");
}

#[test]
fn a_computer_is_rate_limited_by_the_limit_the_machine_announces() {
    let f = fx("rate");
    f.dm.enable(&admin(), "root", 0).unwrap();
    f.dm.set_core_enabled(&admin(), "root", true).unwrap();
    let c = paired(&f, "laptop", "root");
    grant(&f, "laptop", &[Scope::SETTINGS_READ]);
    let hello = ok(&c, "settings", "schema", json!({}));
    assert!(hello.is_array());
    let limit = {
        let (_, v) = post(&c, "/v1/dev/core/hello", json!({"contract": {"major": 0, "minor": 1}, "wants": []}));
        v["result"]["hello"]["rate_limit"]["calls"].as_u64().unwrap()
    };
    let mut refused = None;
    for _ in 0..limit + 5 {
        let v = call(&c, "settings", "get", json!({"key": "show_all_ai"}));
        if v["ok"] == false {
            refused = Some(v);
            break;
        }
    }
    let v = refused.expect("rate limited");
    assert_eq!(v["error"]["code"], "rate_limited");
    assert!(v["error"]["retry_after_ms"].as_u64().unwrap() > 0, "{v}");
}
