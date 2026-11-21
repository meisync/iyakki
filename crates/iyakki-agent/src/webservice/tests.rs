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

/// (the start of a run's prompt, the tools the model was offered), for the tests that look at which tools a run may use
static SEEN_TOOLS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

struct Script {
    turns: Mutex<VecDeque<ModelTurn>>,
}

impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("plain chat is not scripted".into())
    }
    fn complete_tools(&self, m: &[Message], specs: &[ToolSpec]) -> Result<ModelTurn, String> {
        // what each run was offered: its first user message (whole, the tests look inside prompts) and the names of the tools
        if let Some(first) = m.iter().find(|x| matches!(x.role, crate::chat::Role::User)) {
            SEEN_TOOLS.lock().unwrap().push((first.content.chars().take(200_000).collect(), specs.iter().map(|s| s.name.clone()).collect()));
        }
        let t = self.turns.lock().unwrap().pop_front().unwrap_or(ModelTurn { text: "done".into(), calls: vec![] });
        // two turns for the real-browser fixture: a failing model and a slow one
        if t.text == "__ERR__" {
            return Err("the model is not reachable".into());
        }
        if t.text == "__HANG__" {
            std::thread::sleep(Duration::from_secs(8));
            return Ok(ModelTurn { text: "done after a wait".into(), calls: vec![] });
        }
        Ok(t)
    }
}

fn call_turn(name: &str) -> ModelTurn {
    ModelTurn { text: String::new(), calls: vec![ToolCall { id: format!("c-{name}"), name: name.into(), args: json!({"url": "https://example.org"}) }] }
}

fn final_turn(t: &str) -> ModelTurn {
    ModelTurn { text: t.into(), calls: vec![] }
}

/// The files of the fake tool set (paths are unique per test).
static FAKE_FILES: Mutex<std::collections::BTreeMap<String, String>> = Mutex::new(std::collections::BTreeMap::new());

/// (flow notices shown, saver shorter on battery) of the fake tool set.
/// reduce motion, large pointer, big focus ring, typing echo
static FAKE_ACCESS: Mutex<(bool, bool, bool, bool)> = Mutex::new((false, false, true, true));
static FAKE_SWITCHES: Mutex<(bool, bool, bool, bool)> = Mutex::new((true, false, true, true));

/// The calls the settings forms made: (tool, arguments).
static FAKE_FORMS: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new()); // flaky-ok: an append-only log behind a lock; each test looks only for its own unique entry

struct FakeTools {
    executed: Arc<Mutex<Vec<String>>>,
}

impl Tools for FakeTools {
    fn defs(&self) -> Vec<ToolDef> {
        let t = |n: &str, risk| ToolDef { spec: ToolSpec { name: n.into(), description: n.into(), schema: json!({"type": "object"}) }, risk, untrusted: true };
        // `web_fetch` asks here so approvals can be tested with a tool that is in the profile; `run_command` is not in any profile
        // `read_file` and `write_file` are what a person's own tool set has and no service profile lists: only the tiles' `files` domain reaches them
        vec![t("now", Risk::Safe), t("web_fetch", Risk::Ask), t("run_command", Risk::Ask), t("radio_play", Risk::Safe), t("read_file", Risk::Safe), t("write_file", Risk::Ask), t("notification_filters", Risk::Safe), t("notification_filter_set", Risk::Ask), t("screensaver_status", Risk::Safe), t("screensaver_set", Risk::Ask), t("system_sounds_status", Risk::Safe), t("system_sounds_set", Risk::Ask), t("battery_settings_status", Risk::Safe), t("battery_settings_set", Risk::Ask), t("accessibility_status", Risk::Safe), t("accessibility_set", Risk::Ask), t("display_dock", Risk::Safe), t("display_dock_set", Risk::Ask), t("display_wallpaper", Risk::Safe), t("display_wallpaper_set", Risk::Ask), t("displays_status", Risk::Safe), t("wifi_status", Risk::Safe), t("wifi_scan", Risk::Safe), t("wifi_join", Risk::Ask), t("model_options_set", Risk::Ask)]
    }
    fn call(&self, name: &str, a: &Value) -> Result<String, String> {
        self.executed.lock().unwrap().push(name.into());
        match name {
            "read_file" => FAKE_FILES.lock().unwrap().get(a["path"].as_str().unwrap_or("")).cloned().ok_or_else(|| "no such file".to_string()),
            "write_file" => {
                let p = a["path"].as_str().unwrap_or("");
                if p.starts_with("/etc/") {
                    return Err("you may not write there".into());
                }
                FAKE_FILES.lock().unwrap().insert(p.into(), a["content"].as_str().unwrap_or("").into());
                Ok(format!("wrote {p}"))
            }
            "notification_filters" => Ok(format!("scan: show (New devices)\nflows: {} (Flows)\n", if FAKE_SWITCHES.lock().unwrap().0 { "show" } else { "quiet" })),
            "screensaver_status" => Ok(format!("After 10 minutes idle: saver. Style: logo. Shorter on battery: {}.", if FAKE_SWITCHES.lock().unwrap().1 { "yes" } else { "no" })),
            "system_sounds_status" => Ok(format!("System sounds are {}. Level: -10 dB below IYAKKI's volume. Start: on", if FAKE_SWITCHES.lock().unwrap().2 { "on" } else { "off" })),
            "battery_settings_status" => Ok(format!("Percentage in the top bar: {}. Low-battery warnings: on. Shutdown at the critical level: on.", if FAKE_SWITCHES.lock().unwrap().3 { "shown" } else { "hidden" })),
            "accessibility_status" => {
                let f = *FAKE_ACCESS.lock().unwrap();
                let on = |b: bool| if b { "on" } else { "off" };
                Ok(format!("screen reader off; speak notices on; large pointer {}; big focus ring {}; reduce motion {}; sticky keys off; slow keys off; typing echo {}; high contrast off", on(f.1), on(f.2), on(f.0), on(f.3)))
            }
            "accessibility_set" => {
                let mut f = FAKE_ACCESS.lock().unwrap();
                if let Some(b) = a["reduce_motion"].as_bool() {
                    f.0 = b;
                }
                if let Some(b) = a["large_pointer"].as_bool() {
                    f.1 = b;
                }
                if let Some(b) = a["big_focus_ring"].as_bool() {
                    f.2 = b;
                }
                if let Some(b) = a["typing_echo"].as_bool() {
                    f.3 = b;
                }
                Ok("done".into())
            }
            "system_sounds_set" => {
                FAKE_SWITCHES.lock().unwrap().2 = a["enabled"].as_bool().unwrap_or(false);
                Ok("done".into())
            }
            "battery_settings_set" => {
                FAKE_SWITCHES.lock().unwrap().3 = a["show_percent"].as_bool().unwrap_or(false);
                Ok("done".into())
            }
            "notification_filter_set" => {
                FAKE_SWITCHES.lock().unwrap().0 = a["level"] == "show";
                Ok("done".into())
            }
            "screensaver_set" => {
                if a.get("battery_shorter").is_none() {
                    return Err("nothing to change".into());
                }
                FAKE_SWITCHES.lock().unwrap().1 = a["battery_shorter"].as_bool().unwrap_or(false);
                Ok("done".into())
            }
            "display_dock" => Ok("{\"pinned\":[\"files\"]}".into()),
            "display_wallpaper" => Ok("Chosen: dusk".into()),
            "displays_status" => Ok("Screen 1: 2560x1440 at 100%\n\nScreen 2: off\x07\n".into()),
            "wifi_status" => Ok("Connected to home-net (strong)".into()),
            "wifi_scan" => Ok("home-net  WPA2  strong\ncafe-guest  open  weak\n".into()),
            "wifi_join" | "model_options_set" => {
                FAKE_FORMS.lock().unwrap().push((name.to_string(), a.clone()));
                Ok("done".into())
            }
            _ => Ok(format!("{name} ok")),
        }
    }
    fn describe(&self, name: &str, a: &Value) -> String {
        if name == "wifi_join" || name == "model_options_set" {
            return format!("{name} {a}");
        }
        if name == "screensaver_set" || name == "notification_filter_set" || name == "system_sounds_set" || name == "battery_settings_set" || name == "accessibility_set" {
            return format!("{name} {a}");
        }
        if name == "write_file" {
            return format!("write {} characters to {}", a["content"].as_str().map_or(0, |c| c.chars().count()), a["path"].as_str().unwrap_or(""));
        }
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
        // the same wrapper the real source adds (`deploy_team_host`), over a backend that records what it was told to apply
        Arc::new(crate::hostservicetools::TeamHostTools::new(Arc::new(FakeTools { executed: self.executed.clone() }), Arc::new(FakeHostBackend { executed: self.executed.clone() })))
    }
}

struct FakeHostBackend {
    executed: Arc<Mutex<Vec<String>>>,
}

impl crate::hostservicetools::Backend for FakeHostBackend {
    fn preview(&self, team: &str, name: &str) -> Result<String, String> {
        Ok(format!("kind: Service\nmetadata: {{name: {name}, namespace: {team}}}"))
    }
    fn publish(&self, team: &str, name: &str, environment: &str) -> Result<String, String> {
        self.executed.lock().unwrap().push(format!("apply:{team}/{name}@{environment}"));
        Ok("service created".into())
    }
}

struct FakeExtras {
    ready: AtomicBool,
    /// (switch, port) of "Serve this desktop to browsers"
    web: Mutex<(bool, u16)>,
    voice_ret: Mutex<Vec<(String, String, String)>>,
    /// (person, from, to) of every tenant switch the service reported
    switched: Arc<Mutex<Vec<(String, String, String)>>>,
    /// developer key (hex) -> (computer, account); computer -> its api scopes
    progs: Mutex<HashMap<String, (String, String)>>,
    pscopes: Mutex<HashMap<String, Vec<String>>>,
    /// the one schedule's pause, kept so that a page that pauses it sees it paused
    paused: AtomicBool,
    /// the files of the person's home that `pictures.load` may read (path to bytes)
    home: Mutex<HashMap<String, Vec<u8>>>,
    /// (person, tile, badge, line) of every `dock.status` a tile set
    tile_status: Mutex<Vec<(String, String, Option<u32>, String)>>,
    /// the terminal a tile's `terminal.*` calls reach (F-SDK-34)
    terminal: Arc<iyakki_core_fake::FakeTerminal>,
}

impl Extras for FakeExtras {
    fn terminal(&self) -> Arc<dyn iyakki_core_api::Terminal> {
        self.terminal.clone()
    }
    fn set_tile_status(&self, p: &Principal, tile: &str, badge: Option<u32>, line: &str) {
        self.tile_status.lock().unwrap().push((p.user.clone(), tile.into(), badge, line.into()));
    }
    fn home_bytes(&self, _user: &str, path: &str, max: usize) -> Result<Vec<u8>, String> {
        self.home.lock().unwrap().get(path).filter(|b| b.len() <= max).cloned().ok_or_else(|| "that file cannot be read".to_string())
    }
    fn paired_program(&self, key_hex: &str) -> Option<(String, String)> {
        self.progs.lock().unwrap().get(key_hex).cloned()
    }
    fn program_scopes(&self, computer: &str) -> Vec<String> {
        self.pscopes.lock().unwrap().get(computer).cloned().unwrap_or_default()
    }
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
            Ok(json!({"id": "f1", "name": "Nightly", "nodes": []}))
        } else if id == "f2" {
            // needs a person's own Claude sign-in: cannot be scheduled
            Ok(json!({"id": "f2", "name": "Needs me", "nodes": [{"id": "a", "kind": "agent", "config": {"agent": "claude"}}]}))
        } else if id == "f3" {
            // uses a team secret nobody marked for scheduled runs
            Ok(json!({"id": "f3", "name": "Secret user", "nodes": [{"id": "h", "kind": "http", "config": {"url": "https://x.example.com", "headers": {"Authorization": "Bearer {{secret.api-token}}"}}}]}))
        } else {
            Err(ApiError::NotFound("no such flow".into()))
        }
    }
    fn flow_run(&self, _: &Principal, id: &str, draft: bool, inputs: &Value) -> ApiResult<String> {
        if id != "f1" {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        Ok(format!("r{}{}", if draft { "d" } else { "" }, inputs.as_object().map(|o| o.len()).unwrap_or(0) + 1))
    }
    fn flow_runs(&self, _: &Principal, id: &str) -> ApiResult<Vec<Value>> {
        if id != "f1" {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        Ok((1..=7).rev().map(|n| json!({"id": format!("r{n}"), "flow": "f1", "status": if n == 7 { "blocked" } else { "succeeded" }, "blocked": if n == 7 { "asks: delete the old files?" } else { "" }})).collect())
    }
    fn flow_import(&self, _: &Principal, flow: &Value) -> ApiResult<Value> {
        if flow["id"].as_str().is_none() {
            return Err(ApiError::InvalidInput("the flow has no id".into()));
        }
        Ok(json!({"id": format!("{}-2", flow["id"].as_str().unwrap_or("")), "name": flow["name"]}))
    }
    fn flow_validate(&self, _: &Principal, id: Option<&str>, flow: Option<&Value>) -> ApiResult<Vec<Value>> {
        match (id, flow) {
            (Some("f1"), _) => Ok(vec![json!({"node": "n1", "level": "warning", "message": "this step has no timeout"})]),
            (Some(_), _) => Err(ApiError::NotFound("no such flow".into())),
            (None, Some(f)) if f["broken"] == true => Ok(vec![json!({"node": null, "level": "error", "message": "no start step"})]),
            (None, Some(_)) => Ok(vec![]),
            _ => Err(ApiError::InvalidInput("give a flow id or a flow".into())),
        }
    }
    fn flow_run_decide(&self, _: &Principal, id: &str, run: &str, node: &str, approved: bool) -> ApiResult<()> {
        match (id, run, node) {
            ("f1", "r1", "ask") if approved => Ok(()),
            ("f1", "r1", "ask") => Ok(()),
                        _ => Err(ApiError::NotFound("no such waiting step".into())),
        }
    }
    fn flow_run_stop(&self, _: &Principal, id: &str, run: &str) -> ApiResult<()> {
        if id == "f1" && run == "r1" {
            Ok(())
        } else {
            Err(ApiError::NotFound("no such run".into()))
        }
    }
    fn schedules(&self, _: &Principal) -> ApiResult<Vec<Value>> {
        Ok(vec![json!({"flow": "f1", "name": "Nightly", "published": 1, "active": true, "trigger": {"id": "t1", "kind": "cron", "expr": "0 9 * * *", "tz": "local", "enabled": true}, "next": [1_800_000_000_000i64], "paused": self.paused.load(Ordering::SeqCst)})])
    }
    fn schedule_save(&self, _: &Principal, flow: &str, trigger: &Value) -> ApiResult<Value> {
        if !["f1", "f2", "f3"].contains(&flow) {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        Ok(json!({"active": true, "trigger": trigger, "next": []}))
    }
    fn schedule_delete(&self, _: &Principal, flow: &str, trigger: &str) -> ApiResult<Value> {
        if flow != "f1" || trigger != "t1" {
            return Err(ApiError::NotFound("no such trigger".into()));
        }
        Ok(json!({"removed": trigger, "timed_left": 0, "active": true}))
    }
    fn schedule_pause(&self, _: &Principal, flow: &str, paused: bool) -> ApiResult<Value> {
        if flow != "f1" {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        self.paused.store(paused, Ordering::SeqCst);
        Ok(json!({"flow": flow, "paused": paused}))
    }
    fn flow_run_get(&self, _: &Principal, id: &str, run: &str) -> ApiResult<Value> {
        if id != "f1" || run != "r1" {
            return Err(ApiError::NotFound("no such run".into()));
        }
        Ok(json!({"id": "r1", "flow": "f1", "status": "succeeded", "nodes": []}))
    }
    fn voice_offer(&self, p: &Principal) -> ApiResult<(String, String)> {
        Ok(("cloud.example:7443".into(), format!("{{\"for\":\"{}\"}}", p.user)))
    }
    fn voice_return(&self, p: &Principal, addr: &str, offer: &str) -> ApiResult<()> {
        if addr.parse::<std::net::SocketAddr>().is_err() {
            return Err(ApiError::InvalidInput("the device's audio address is not host:port".into()));
        }
        self.voice_ret.lock().unwrap().push((p.user.clone(), addr.to_string(), offer.to_string()));
        Ok(())
    }
    fn web_access(&self, _: &Principal) -> ApiResult<Value> {
        let (on, port) = *self.web.lock().unwrap();
        Ok(json!({"on": on, "port": port, "listening": on, "boot": false, "error": "", "status": if on { format!("on, port {port}") } else { "off: nothing listens".to_string() }, "urls": if on { vec![format!("https://x.local:{port}")] } else { vec![] }, "fingerprint": "AA:BB", "exposes": "Anyone on this network who knows a sign-in can use this desktop.", "has_admin_password": true}))
    }
    fn set_web_access(&self, _: &Principal, on: Option<bool>, port: Option<u16>) -> ApiResult<Vec<(String, String)>> {
        let mut w = self.web.lock().unwrap();
        let mut done = vec![];
        if let Some(p) = port {
            if p != 0 && p < 1024 {
                return Err(ApiError::InvalidInput("Choose a port from 1024 to 65535".into()));
            }
            w.1 = if p == 0 { 8443 } else { p };
            done.push(("settings.web_access.port".to_string(), w.1.to_string()));
        }
        if let Some(o) = on {
            w.0 = o;
            done.push(("settings.web_access".to_string(), format!("{} (port {}, HTTPS)", if o { "on" } else { "off" }, w.1)));
        }
        Ok(done)
    }
    fn tenant_switched(&self, p: &Principal, from: &str, to: &str) {
        self.switched.lock().unwrap().push((p.user.clone(), from.into(), to.into()));
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
    switched: Arc<Mutex<Vec<(String, String, String)>>>,
    extras: Arc<FakeExtras>,
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
    let switched = Arc::new(Mutex::new(Vec::new()));
    let extras = Arc::new(FakeExtras { ready: AtomicBool::new(true), web: Mutex::new((false, 8443)), switched: switched.clone(), progs: Default::default(), pscopes: Default::default(), paused: Default::default(), home: Default::default(), tile_status: Default::default(), voice_ret: Default::default(), terminal: Default::default() });
    let deps = Arc::new(Deps {
        auth: accounts.clone(),
        audit: accounts.clone(),
        store: Arc::new(RunStore::new(&dir.join("runs"))),
        chat: script.clone(),
        tools: Arc::new(FakeSource { executed: executed.clone() }),
        extras: extras.clone(),
        system_prompt: "test".into(),
    });
    let id = tls::self_signed(&dir.join("tls"), &[]).expect("certificate");
    let svc = server::start(&chk, deps, id).expect("start");
    H { svc, accounts, executed, script, dir, switched, extras }
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
    // only these are reachable without signing in (the last three are a tile's static page, its script and its style sheet: the same bytes
    // for everyone, no data in them, served only by the profiles that have tiles; F-WEBSVC-12)
    let public: Vec<&str> = ROUTES.iter().filter(|r| r.access == Access::Public).map(|r| r.id).collect();
    assert_eq!(public, ["healthz", "readyz", "version", "login", "tile_page", "tile_page_js", "tile_page_css", "tile_bundle_file", "tile_wrapper", "tile_wrapper_js", "tile_bridge_js", "tile_bridge"]);
    // the cloud profile is the agent-api profile plus more
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::AgentApi)) {
        assert!(r.in_profile(Profile::Cloud), "{}", r.id);
    }
    assert!(ROUTES.iter().any(|r| r.in_profile(Profile::Cloud) && !r.in_profile(Profile::AgentApi)));
}

#[test]
fn a_program_signs_in_with_a_paired_key_and_can_call_only_the_families_granted_to_its_computer() {
    use crate::devmode::auth::{hex, sign_headers};
    let h = harness("cloud", |_| {});
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let key_hex = hex(key.verifying_key().as_bytes());
    let fp = h.svc.fingerprint().to_string();
    let signed = |key: &ed25519_dalek::SigningKey, fp: &str| -> Request {
        let mut r = req("POST", "/api/v1/login", None, None);
        for (k, v) in sign_headers(key, "POST", "/api/v1/login", &[], &[], fp, crate::clock::now_unix()).unwrap() {
            r.headers.insert(k, v);
        }
        r
    };
    // a key nobody paired, a wrong certificate and an unsigned try are 401; a paired key without a grant is 403
    assert_eq!(h.call_req(signed(&key, &fp)).0, 401, "not paired");
    h.extras.progs.lock().unwrap().insert(key_hex.clone(), ("laptop".into(), "ann".into()));
    assert_eq!(h.call_req(signed(&key, "AA:BB")).0, 401, "signed for another certificate");
    let (s, v, _) = h.call_req(signed(&key, &fp));
    assert_eq!(s, 403, "{v}");
    assert!(v["error"]["message"].as_str().unwrap_or("").contains("no api scope"), "{v}");
    // granted: read flows and see who it is
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:flows:read".into(), "api:status".into()]);
    let r = signed(&key, &fp);
    let (s, v, _) = h.call_req(r.clone());
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["user"].as_str(), v["computer"].as_str()), (Some("ann"), Some("laptop")));
    assert_eq!(h.call_req(r).0, 401, "the same signed request is not accepted twice");
    let tok = v["token"].as_str().unwrap().to_string();
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&tok), None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&tok), None).0, 200);
    let (s, v, _) = h.call("POST", "/api/v1/flows/f1/run", Some(&tok), Some(json!({})));
    assert_eq!(s, 403, "reading only: no run");
    assert!(v["error"]["message"].as_str().unwrap_or("").contains("laptop"), "{v}");
    assert_eq!(h.call("GET", "/api/v1/files", Some(&tok), None).0, 403, "another family");
    assert_eq!(h.call("GET", "/api/v1/chat/runs", Some(&tok), None).0, 403);
    // the account's own rights still apply, and the administrator routes stay out of reach whatever is granted
    let admin = harness("cloud", |_| {});
    let akey = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
    admin.extras.progs.lock().unwrap().insert(hex(akey.verifying_key().as_bytes()), ("ops".into(), "root".into()));
    admin.extras.pscopes.lock().unwrap().insert("ops".into(), crate::devmode::apiscopes::FAMILIES.iter().map(|(f, _)| f.to_string()).collect());
    let mut r = req("POST", "/api/v1/login", None, None);
    for (k, v) in sign_headers(&akey, "POST", "/api/v1/login", &[], &[], admin.svc.fingerprint(), crate::clock::now_unix()).unwrap() {
        r.headers.insert(k, v);
    }
    let (s, v, _) = admin.call_req(r);
    assert_eq!(s, 200, "{v}");
    let atok = v["token"].as_str().unwrap().to_string();
    assert_eq!(admin.call("GET", "/api/v1/admin/status", Some(&atok), None).0, 403, "the administrator route is not reached by any scope");
    assert_eq!(admin.call("GET", "/api/v1/flows", Some(&atok), None).0, 200);
    // taking the grant back applies on the very next request, and logout still works
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:status".into()]);
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&tok), None).0, 403, "revoked at once");
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&tok), None).0, 200);
    h.extras.progs.lock().unwrap().clear();
    h.extras.pscopes.lock().unwrap().clear();
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&tok), None).0, 403, "an unpaired computer holds nothing");
    assert_eq!(h.call("POST", "/api/v1/logout", Some(&tok), None).0, 200, "ending its own token is always allowed");
    // a person's ordinary token is not limited by any of this
    let ann = h.ann();
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/chat/runs", Some(&ann), None).0, 200);
}

#[test]
fn the_client_helper_signs_in_over_real_tls_with_the_paired_key_and_calls_only_what_its_scopes_allow() {
    use crate::devmode::auth::hex;
    use crate::devmode::webclient::ApiClient;
    let h = harness("cloud", |_| {});
    let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
    h.extras.progs.lock().unwrap().insert(hex(key.verifying_key().as_bytes()), ("ci".into(), "ann".into()));
    let (port, fp) = (h.svc.local_addr().port(), h.svc.fingerprint().to_string());
    let api = ApiClient::with_key("127.0.0.1", port, &fp, key.clone());
    assert!(api.call("GET", "/api/v1/whoami", &[], None).unwrap_err().contains("sign in first"));
    assert!(api.sign_in().unwrap_err().contains("no api scope"), "a computer without a grant is refused");
    h.extras.pscopes.lock().unwrap().insert("ci".into(), vec!["api:status".into(), "api:flows:read".into()]);
    let who = api.sign_in().unwrap();
    assert_eq!((who["user"].as_str(), who["computer"].as_str()), (Some("ann"), Some("ci")));
    assert_eq!(api.call("GET", "/api/v1/whoami", &[], None).unwrap().0, 200);
    assert_eq!(api.call("GET", "/api/v1/flows", &[], None).unwrap().0, 200);
    let (s, v) = api.call("POST", "/api/v1/flows/f1/run", &[], Some(&json!({}))).unwrap();
    assert_eq!(s, 403, "{v}");
    // the wrong certificate is never trusted
    let wrong = ApiClient::with_key("127.0.0.1", port, "AA:BB:CC", key);
    assert!(wrong.sign_in().is_err());
    api.sign_out().unwrap();
    assert!(api.call("GET", "/api/v1/whoami", &[], None).is_err());
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
    for (m, p) in [("GET", "/api/v1/models"), ("GET", "/api/v1/chat/runs"), ("POST", "/api/v1/chat/runs"), ("GET", "/api/v1/flows"), ("POST", "/api/v1/data/call")] {
        assert_eq!(h.call(m, p, Some(&gus), Some(json!({"prompt": "x"})).filter(|_| m == "POST")).0, 403, "{m} {p}");
    }
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&gus), None).0, 200, "whoami needs no app");
    // the one deliberate exception: the unified approvals list is open to any signed-in person (tenant approvers have no Chat right) and
    // its handler returns only what that person may see, so a person with no rights gets 200 and an empty list, not 403
    let (s, v, _) = h.call("GET", "/api/v1/approvals", Some(&gus), None);
    assert_eq!((s, v["approvals"].clone()), (200, json!([])));
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
    for p in ["/", "/api/v1", "/api/v1/camera", "/api/v1/audio", "/api/v1/usb", "/api/v1/firmware", "/api/v1/firewall", "/api/v1/shell", "/v1/dev/install", "/api/v1/dev/install", "/api/v1/system/power", "/ws", "/api/v1/chat/runs/abc/../../x"] {
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
    // a write tool is a pending import in the cloud profile (it needs complete arguments), and does not exist in the data API
    assert_eq!(c.call("POST", "/api/v1/data/call", Some(&ct), Some(json!({"tool": "data_export", "args": {}}))).0, 400);
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
fn a_program_token_opens_the_event_stream_and_a_members_address_only_as_far_as_its_scopes_reach() {
    use crate::cloudmembers::SessionLookup;
    use crate::devmode::auth::{hex, sign_headers};
    let h = harness("cloud", |_| {});
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let fp = h.svc.fingerprint().to_string();
    h.extras.progs.lock().unwrap().insert(hex(key.verifying_key().as_bytes()), ("laptop".into(), "ann".into()));
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:status".into()]);
    let mut r = req("POST", "/api/v1/login", None, None);
    for (k, v) in sign_headers(&key, "POST", "/api/v1/login", &[], &[], &fp, crate::clock::now_unix()).unwrap() {
        r.headers.insert(k, v);
    }
    let (s, v, _) = h.call_req(r);
    assert_eq!(s, 200, "{v}");
    let prog = v["token"].as_str().unwrap().to_string();
    let sh = h.svc.shared();
    assert!(!super::events::token_opens_stream(sh, &prog), "api:status does not reach the event stream (api:tiles)");
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:tiles:read".into()]);
    assert!(super::events::token_opens_stream(sh, &prog), "granted at once");
    assert!(super::events::token_opens_stream(sh, &h.ann()), "a person's own token is not limited");
    // the domain listener: a program's token is not a person's session
    let sessions = server::sessions_for_test(sh);
    assert_eq!(sessions.user_of(&prog), None, "a program token does not open a members address");
    assert_eq!(sessions.user_of(&h.ann()).as_deref(), Some("ann"));
}

#[test]
fn audit_lines_exist_for_every_audited_route_and_hold_no_secrets() {
    // one sign-in per audited route: more routes than the default sign-in limit
    let h = harness("cloud", |c| c.limits.logins_per_minute = 1000);
    let secret_prompt = "my-very-private-prompt-text";
    h.script.turns.lock().unwrap().push_back(final_turn("ok"));
    for r in ROUTES.iter().filter(|r| r.in_profile(Profile::Cloud) && r.audit) {
        let path = r.path.replace("{id}", "abc123abc123").replace("{run}", "abc123abc123").replace("{call}", "c").replace("{scope}", "prod").replace("{group}", "g").replace("{kind}", "egress-policy").replace("{team}", "web").replace("{name}", "default");
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
                // installing a tile package (a WASM file kept in the person's own tile folder) needs no hardware and no operating-system install
                if bad == "install" && r.path == "/api/v1/tiles/install" {
                    continue;
                }
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
    // tiles_list is free; deploy_team_host and k8s_apply_yaml ask on a card every time (F-WEB-04, F-TCFG-14, F-TCFG-10: D-TCFG-10; a cluster
    // bound to a team environment also goes through that team's deploy rules)
    assert_eq!(extra, [&"deploy_team_host", &"k8s_apply_yaml", &"tiles_list", &"flow_list", &"flow_get", &"flow_runs", &"flow_run_status", &"webhook_list", &"webhook_create", &"webhook_regenerate", &"webhook_enable", &"webhook_disable", &"webhook_public"]);
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
    assert!(ServiceConfig { profile: "nope".into(), ..ServiceConfig::default() }.check().unwrap_err().contains("unknown profile"));
    assert!(ServiceConfig { profile: "flows-runner".into(), ..ServiceConfig::default() }.check().is_ok(), "the flows runner is a profile now");
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
        extras: Arc::new(FakeExtras { ready: AtomicBool::new(true), web: Mutex::new((false, 8443)), switched: Default::default(), progs: Default::default(), pscopes: Default::default(), paused: Default::default(), home: Default::default(), tile_status: Default::default(), voice_ret: Default::default(), terminal: Default::default() }),
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
    let extras = Arc::new(FakeExtras { ready: AtomicBool::new(false), web: Mutex::new((false, 8443)), switched: Default::default(), progs: Default::default(), pscopes: Default::default(), paused: Default::default(), home: Default::default(), tile_status: Default::default(), voice_ret: Default::default(), terminal: Default::default() });
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
    // shutdown joined the accept thread, so the listener is gone. A freed port can be taken at once by another test of this process
    // (they all bind port 0): so a refusal at any of the tries over two seconds is the proof; only a port that answers the whole time fails
    let t0 = Instant::now();
    let mut refused = false;
    while t0.elapsed() < Duration::from_secs(2) {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
            refused = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(refused, "nothing listens after shutdown");
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
    let _shared = iyakki_testkit::lock(iyakki_testkit::names::ENV); // flaky: the process-wide state below is shared with the other tests of this run
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

#[path = "ui_tests.rs"]
mod ui_tests;
#[path = "pages_api_tests.rs"]
mod pages_api_tests;

#[path = "embed_tests.rs"]
mod embed_tests;

#[path = "tenants_tests.rs"]
mod tenants_tests;

#[path = "schedules_tests.rs"]
mod schedules_tests;

#[path = "notifyapi_tests.rs"]
mod notifyapi_tests;
#[cfg(test)]
#[path = "tiles_tests.rs"]
mod tiles_tests;
#[cfg(test)]
#[path = "remote_runner_tests.rs"]
mod remote_runner_tests;

// ------------------------------------------------------------------------------------------------ tenant configuration

/// Cloud harness with root (owner), ann (admin), bob (member).
fn cfg_cloud() -> (H, String, String, String) {
    let h = harness("cloud", |_| {});
    let (root, ann, bob) = (h.root(), h.ann(), h.login("bob", "bob-password-1"));
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 200);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "ann", "role": "admin"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "bob", "role": "member"}))).0, 200);
    (h, root, ann, bob)
}

#[test]
fn tenantcfg_person_layer_routes_store_only_the_callers_own_narrowing() {
    let (h, _root, ann, bob) = cfg_cloud();
    let me = "/api/v1/tenant/config-me/media.policy";
    // nothing configured by the tenant: nothing to narrow
    assert_eq!(h.call("PUT", me, Some(&bob), Some(json!({"enabled": true, "values": {"deny_devices": ["mic"]}}))).0, 409);
    assert_eq!(h.call("PUT", "/api/v1/tenant/config/media.policy", Some(&ann), Some(json!({"enabled": true, "values": {"media": "allow"}, "ack_warnings": crate::tenantconfig::safety::rules_table()}))).0, 200);
    let (s, v, _) = h.call("PUT", me, Some(&bob), Some(json!({"enabled": true, "values": {"deny_devices": ["mic"]}})));
    assert_eq!((s, v["effective"]["deny_devices"].clone()), (200, json!(["mic"])), "{v}");
    let (s, g, _) = h.call("GET", me, Some(&bob), None);
    assert_eq!((s, g["items"][0]["values"]["deny_devices"].clone()), (200, json!(["mic"])));
    assert_eq!(h.call("GET", me, Some(&ann), None).1["items"], json!([]), "another person sees none of it");
    assert_eq!(h.call("PUT", "/api/v1/tenant/config-me/egress.policy", Some(&bob), Some(json!({"enabled": true}))).0, 400);
    assert_eq!(h.call("DELETE", me, Some(&bob), None).0, 200);
    assert_eq!(h.call("DELETE", me, Some(&bob), None).0, 404);
    assert!(h.accounts.lines().iter().any(|l| l.contains("tenant.config.person.put")));
}

#[test]
fn tenantcfg_api_put_get_validate_secret_and_roles() {
    let (h, _root, ann, bob) = cfg_cloud();
    let url = "/api/v1/tenant/config/agent.claude";
    let body = json!({"enabled": true, "values": {"mode": "shared", "terms_ack": true, "daily_requests_per_person": 3, "monthly_requests_team": 30, "allowed_models": ["*"]}, "secrets": {"token": "sk-ant-API-TEST-SECRET"}, "ack_warnings": crate::tenantconfig::safety::rules_table()});
    // a dry run stores nothing; a member cannot write; an admin can
    assert_eq!(h.call("POST", &format!("{url}/validate"), Some(&ann), Some(body.clone())).1["valid"], true);
    assert_eq!(h.call("GET", url, Some(&ann), None).0, 404);
    assert_eq!(h.call("PUT", url, Some(&bob), Some(body.clone())).0, 403);
    // no vault is bound in a test service: the secret cannot be stored, and nothing is saved
    let (s, v, _) = h.call("PUT", url, Some(&ann), Some(body.clone()));
    assert_eq!(s, 409, "{v}");
    assert_eq!(h.call("GET", url, Some(&ann), None).0, 404);
    // with the vault bound the same call works, and nothing ever shows the secret
    let vault = crate::tenantconfig::vault::MemVault::new();
    crate::tenantconfig::vault::bind(&h.dir, vault.clone());
    let (s, v, _) = h.call("PUT", url, Some(&ann), Some(body));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "ready");
    let (s, g, _) = h.call("GET", url, Some(&ann), None);
    assert_eq!((s, g["values"]["token"].as_str()), (200, Some("[set]")));
    let (s, g, _) = h.call("GET", url, Some(&bob), None);
    assert_eq!((s, g["state"].as_str(), g.get("values").is_none()), (200, Some("ready"), true));
    let (_, l, _) = h.call("GET", "/api/v1/tenant/config", Some(&bob), None);
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    assert!(l.get("registry").is_none());
    assert!(h.call("GET", "/api/v1/tenant/config", Some(&ann), None).1["registry"]["kinds"].is_array());
    for out in [g.to_string(), l.to_string(), h.accounts.lines().join("\n"), std::fs::read_to_string(h.dir.join("cloud/config/tenant-config.local.json")).unwrap()] {
        assert!(!out.contains("API-TEST-SECRET"), "{out}");
    }
    assert!(h.accounts.lines().iter().any(|l| l.contains("tenant.config.put") && l.contains("after=")));
    // versions, rollback, delete
    assert_eq!(h.call("GET", "/api/v1/tenant/config/versions", Some(&ann), None).1["current"], 1);
    assert_eq!(h.call("GET", "/api/v1/tenant/config/versions", Some(&bob), None).0, 403);
    assert_eq!(h.call("POST", "/api/v1/tenant/config/rollback", Some(&ann), Some(json!({"version": 1}))).0, 200);
    assert_eq!(h.call("DELETE", url, Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", url, Some(&ann), None).0, 404);
    assert_eq!(h.call("GET", "/api/v1/tenant/config/mcp.server", Some(&ann), None).0, 404);
    crate::tenantconfig::vault::unbind(&h.dir);
}

#[test]
fn tenantcfg_api_end_to_end_over_the_real_role_files_and_a_live_loopback_vault() {
    use crate::cloudvaultd::{self, Client, Principal as VPrincipal};
    use crate::tenantconfig::vault::{self, LinkVault};
    // the pod's roles are the real files the cloud profile writes (cfg_cloud bootstraps root, then adds ann as admin and bob as member)
    let (h, _root, ann, bob) = cfg_cloud();
    assert!(crate::cloudroles::path(&h.dir).exists(), "the roles are a file on the volume, not a fake");
    // the vault is the real service on the loopback, with ann enrolled as an administrator of tenant "t"
    let vdir = h.dir.join("vault-service");
    let store = cloudvaultd::Store::open(&vdir).unwrap();
    let mut roles = crate::cloudroles::Roles::new("ann").unwrap();
    roles.add_tenant_member("ann", "bob", crate::cloudroles::TenantRole::Member).unwrap();
    let tdir = vdir.join("tenants").join("t");
    std::fs::create_dir_all(tdir.join("cloud")).unwrap();
    crate::cloudroles::save(&crate::cloudroles::path(&tdir), &roles).unwrap();
    let ann_keys = crate::cloudkeys::MemberKeys::from_seed([71; 32]);
    store.enroll(&VPrincipal::Member { tenant: "t".into(), user: "ann".into() }, &ann_keys.public()).unwrap();
    drop(store);
    let run = cloudvaultd::start("127.0.0.1:0".parse().unwrap(), &vdir, None).unwrap();
    let client = Client { host: "127.0.0.1".into(), port: run.local_addr().port(), fingerprint: Some(run.fingerprint.clone()), who: VPrincipal::Member { tenant: "t".into(), user: "ann".into() }, keys: ann_keys };
    vault::bind(&h.dir, Arc::new(LinkVault { ops: client }));
    let url = "/api/v1/tenant/config/agent.claude";
    let body = json!({"enabled": true, "values": {"mode": "shared", "terms_ack": true, "daily_requests_per_person": 3, "monthly_requests_team": 30, "allowed_models": ["*"]}, "secrets": {"token": "sk-ant-E2E-SECRET"}, "ack_warnings": crate::tenantconfig::safety::rules_table()});
    // roles from the real file decide; the secret goes to the live vault and nowhere else
    assert_eq!(h.call("PUT", url, Some(&bob), Some(body.clone())).0, 403);
    let (s, v, _) = h.call("PUT", url, Some(&ann), Some(body));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "ready");
    let (_, g, _) = h.call("GET", url, Some(&ann), None);
    assert_eq!(g["values"]["token"].as_str(), Some("[set]"));
    let (_, g2, _) = h.call("GET", url, Some(&bob), None);
    assert!(g2.get("values").is_none(), "a member sees state only");
    fn walk(d: &std::path::Path, out: &mut String) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(t) = std::fs::read(&p) {
                out.push_str(&String::from_utf8_lossy(&t));
            }
        }
    }
    let mut all = String::new();
    walk(&h.dir, &mut all);
    assert!(!all.contains("E2E-SECRET"), "neither the pod's volume nor the vault service's files hold the secret in the clear");
    // the vault goes away: the item reads as locked and nothing is lost on the pod's side
    run.stop();
    let (_, g, _) = h.call("GET", url, Some(&ann), None);
    assert_eq!(g["values"]["token"].as_str(), Some("[vault locked]"), "{g}");
    vault::unbind(&h.dir);
}

#[test]
fn tenantcfg_api_k3s_binding_per_team_and_environment_and_the_managed_file() {
    let (h, root, ann, bob) = cfg_cloud();
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "create"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "admin"}))).0, 200);
    let vault = crate::tenantconfig::vault::MemVault::new();
    crate::tenantconfig::vault::bind(&h.dir, vault);
    let url = "/api/v1/tenant/config/k3s.cluster/web/dev";
    let b = json!({"enabled": true, "values": {"api_server": "https://web-dev.example.com:6443"}, "secrets": {"credential": "kubeconfig-dev"}, "ack_warnings": crate::tenantconfig::safety::rules_table()});
    assert_eq!(h.call("PUT", url, Some(&bob), Some(b.clone())).0, 200, "a team admin writes the team's binding");
    assert_eq!(h.call("GET", url, Some(&bob), None).1["state"], "ready");
    assert_eq!(h.call("GET", "/api/v1/tenant/config/k3s.cluster/data/dev", Some(&bob), None).0, 404);
    assert_eq!(h.call("PUT", "/api/v1/tenant/config/k3s.cluster/data/dev", Some(&bob), Some(b.clone())).0, 404);
    let (_, l, _) = h.call("GET", "/api/v1/tenant/config", Some(&bob), None);
    assert_eq!(l["items"][0]["reachability"], "unknown");
    // the operator marks an item as managed in the file
    let d = h.dir.join("cloud/config");
    std::fs::write(d.join("tenant-config.json"), json!({"items": [{"kind": "k3s.cluster", "namespace": "web", "name": "dev", "enabled": true, "managed": true, "values": {"api_server": "https://fixed.example.com", "credential": "vault:k3s.cluster/web-dev-credential"}}]}).to_string()).unwrap();
    let (s, v, _) = h.call("PUT", url, Some(&ann), Some(b));
    assert_eq!(s, 409, "{v}");
    let g = h.call("GET", url, Some(&ann), None).1;
    assert_eq!((g["source"].as_str(), g["managed"].clone()), (Some("file"), json!(true)));
    assert_eq!(h.call("DELETE", url, Some(&ann), None).0, 409);
    crate::tenantconfig::vault::unbind(&h.dir);
}

#[test]
fn tenantcfg_api_refuses_a_disabled_shared_agent_at_run_start() {
    let (h, _root, ann, bob) = cfg_cloud();
    let run = |tok: &str, provider: &str| h.call("POST", "/api/v1/chat/runs", Some(tok), Some(json!({"prompt": "hi", "provider": provider, "model": "sonnet"})));
    // off by default
    let (s, v, _) = run(&bob, "claude");
    assert_eq!(s, 403, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("not switched on"));
    // other providers are not this gate's business
    assert_ne!(run(&bob, "local").1["error"]["message"], "this agent is not switched on for the tenant");
    // switched on per person: the gate lets it through to the model lookup (the test service has no other model)
    assert_eq!(h.call("PUT", "/api/v1/tenant/config/agent.claude", Some(&ann), Some(json!({"enabled": true, "values": {"allowed_models": ["sonnet"]}, "ack_warnings": crate::tenantconfig::safety::rules_table()}))).0, 200);
    assert_ne!(run(&bob, "claude").0, 403);
    // switched off again: refused
    assert_eq!(h.call("PUT", "/api/v1/tenant/config/agent.claude", Some(&ann), Some(json!({"enabled": false, "values": {}}))).0, 200);
    assert_eq!(run(&bob, "claude").0, 403);
    assert!(h.accounts.lines().iter().any(|l| l.contains("tenant.agent.refused")));
}

#[test]
fn a_tiles_page_is_public_static_framed_by_nobody_and_only_in_the_profiles_that_have_tiles() {
    let h = harness("tile-host", |_| {});
    let hdr = |r: &Response, k: &str| r.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()).unwrap_or_default();
    // no token: the page, its script and its style sheet are served, with the strict policy
    for (path, ctype, needle) in [("/tiles/counter/app", "text/html", "/tiles/app.js"), ("/tiles/app.js", "text/javascript", "window.iyakki"), ("/tiles/app.css", "text/css", "canvas")] {
        let (s, _, r) = h.call("GET", path, None, None);
        assert_eq!(s, 200, "{path}");
        assert!(hdr(&r, "content-type").starts_with(ctype), "{path}: {:?}", r.headers);
        assert!(String::from_utf8_lossy(&r.body).contains(needle), "{path}");
        let csp = hdr(&r, "content-security-policy");
        assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'") && !csp.contains("unsafe-inline"), "{path}: {csp}");
        assert_eq!(hdr(&r, "x-content-type-options"), "nosniff");
    }
    // a name that is not a plain tile id finds nothing, and another method is refused
    assert_eq!(h.call("GET", "/tiles/a..b/app", None, None).0, 404);
    assert_eq!(h.call("GET", "/tiles/.hidden/app", None, None).0, 404);
    assert_eq!(h.call("POST", "/tiles/counter/app", None, None).0, 405);
    // the flows runner has no tiles: nothing is served there
    let f = harness("flows-runner", |_| {});
    assert_eq!(f.call("GET", "/tiles/counter/app", None, None).0, 404);
    assert_eq!(f.call("GET", "/tiles/app.js", None, None).0, 404);
}

#[test]
fn tenantcfg_api_gates_a_named_other_agent_at_run_start_by_its_own_model_list_and_switch() {
    let (h, _root, ann, bob) = cfg_cloud();
    let run = |tok: &str, provider: &str, model: &str| h.call("POST", "/api/v1/chat/runs", Some(tok), Some(json!({"prompt": "hi", "provider": provider, "model": model})));
    // no such entry: this gate has nothing to say about the name
    assert_ne!(run(&bob, "aider", "m1").1["error"]["message"], "this model is not allowed for the agent");
    let put = |on: bool| h.call("PUT", "/api/v1/tenant/config/agent.other/aider", Some(&ann), Some(json!({"enabled": on, "values": {"allowed_models": ["m1"]}, "ack_warnings": crate::tenantconfig::safety::rules_table()}))).0;
    assert_eq!(put(true), 200);
    // the entry names the agent: a model that is not on its list is refused, the one that is passes the gate
    let (s, v, _) = run(&bob, "aider", "m2");
    assert_eq!(s, 403, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("not allowed"), "{v}");
    assert_ne!(run(&bob, "aider", "m1").0, 403);
    // switched off: refused
    assert_eq!(put(false), 200);
    assert_eq!(run(&bob, "aider", "m1").0, 403);
    assert!(h.accounts.lines().iter().any(|l| l.contains("tenant.agent.refused") && l.contains("agent.other")));
}

#[test]
fn tenantcfg_api_groups_approver_roles_and_the_approvals_list() {
    let (h, root, ann, bob) = cfg_cloud();
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "create"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "admin"}))).0, 200);
    // the team's admin defines a group and gives the group the prod approver role; a person outside the tenant is refused
    assert_eq!(h.call("PUT", "/api/v1/namespaces/web/groups/leads", Some(&bob), Some(json!({"members": ["ann"]}))).0, 200);
    assert_eq!(h.call("PUT", "/api/v1/namespaces/web/groups/bad", Some(&bob), Some(json!({"members": ["nobody"]}))).0, 400);
    assert_eq!(h.call("PUT", "/api/v1/namespaces/web/approvers/prod", Some(&bob), Some(json!({"groups": ["leads"]}))).0, 200);
    let g = h.call("GET", "/api/v1/namespaces/web/approvers", Some(&bob), None).1;
    assert_eq!(g["approvers"]["prod"]["expanded"], json!(["ann"]));
    // tenant-wide groups are for tenant admins
    assert_eq!(h.call("PUT", "/api/v1/namespaces/_tenant/groups/ops", Some(&bob), Some(json!({"members": ["bob"]}))).0, 403);
    assert_eq!(h.call("PUT", "/api/v1/namespaces/_tenant/groups/ops", Some(&ann), Some(json!({"members": ["bob"]}))).0, 200);
    // a stranger to the team sees nothing
    let carl = h.login("gus", "gus-password-1");
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/approvers", Some(&carl), None).0, 403);
    // nobody has anything pending
    assert_eq!(h.call("GET", "/api/v1/tenant/approvals", Some(&bob), None).1["approvals"], json!([]));
    assert_eq!(h.call("POST", "/api/v1/tenant/approvals/abc123/approve", Some(&bob), None).0, 404);
    // revoke
    assert_eq!(h.call("DELETE", "/api/v1/namespaces/web/approvers/prod", Some(&bob), Some(json!({"groups": ["leads"]}))).0, 200);
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/approvers", Some(&bob), None).1["approvers"], json!({}));
    assert!(h.accounts.lines().iter().any(|l| l.contains("api.tteam_approver_grant")));
}

// ------------------------------------------------------------------------------------------------ real-browser fixture

/// NOT a test of its own: the server that the real-browser harness (`dist/browsertest/`) drives. Ignored, so a plain test run never
/// starts it. It starts the same service the tests above use (real TLS socket on 127.0.0.1, fake accounts with throw-away
/// passwords, a scripted model, a temporary data folder), writes where it listens to `$IYAKKI_BROWSERTEST_INFO`, and serves until
/// the file `$IYAKKI_BROWSERTEST_INFO.stop` appears (or 20 minutes pass), then removes its data. Profile: `$IYAKKI_BROWSERTEST_PROFILE`
/// (default `cloud`).
#[test]
#[ignore = "serves for dist/browsertest; run by its driver"]
fn browser_fixture_server() {
    let Some(info) = std::env::var_os("IYAKKI_BROWSERTEST_INFO") else {
        eprintln!("SKIP: IYAKKI_BROWSERTEST_INFO is not set (this is started by dist/browsertest/run_pages.py)");
        return;
    };
    let profile = std::env::var("IYAKKI_BROWSERTEST_PROFILE").unwrap_or_else(|_| "cloud".into());
    let h = harness(&profile, |c| {
        c.limits.logins_per_minute = 1000;
        c.limits.requests_per_minute = 100_000; // the run measures how many requests the pages make; the fixture must not refuse them
    });
    // IYAKKI_BROWSERTEST_TENANTS=2: the home tenant (root and ann are members) and a second one `gamma` that root owns, so the page has a switch
    if std::env::var("IYAKKI_BROWSERTEST_TENANTS").as_deref() == Ok("2") && profile == "cloud" {
        let gamma = h.dir.join("gamma-volume");
        let mut r = crate::cloudroles::Roles::new("root").unwrap();
        r.add_tenant_member("root", "bob", crate::cloudroles::TenantRole::Member).unwrap();
        crate::cloudroles::save(&crate::cloudroles::path(&gamma), &r).unwrap();
        let file = super::tenants::file(&h.dir);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = json!({"home": {"id": "acme", "name": "Acme Ltd"}, "tenants": [{"id": "gamma", "name": "Gamma GmbH", "data_dir": gamma.to_string_lossy()}]});
        std::fs::write(file, text.to_string()).unwrap();
    }
    // a small git repository in root's own space, with one changed and one untracked file, for the Git page
    if profile == "cloud" {
        let d = h.dir.join("cloud").join("files").join("personal").join("root").join("repo");
        std::fs::create_dir_all(&d).unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin").env("HOME", "/nonexistent").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.org", "-c", "commit.gpgsign=false", "-C"]).arg(&d).args(args).output();
            o.map(|o| o.status.success()).unwrap_or(false)
        };
        if git(&["init", "-q", "-b", "main"]) {
            for (i, t) in ["one", "two", "three"].iter().enumerate() {
                std::fs::write(d.join("f.txt"), format!("{t}\n")).unwrap();
                git(&["add", "."]);
                git(&["commit", "-q", "-m", &format!("commit {t} {i}")]);
            }
            std::fs::write(d.join("f.txt"), "changed\n").unwrap();
            std::fs::write(d.join("new.txt"), "n\n").unwrap();
        }
    }
    let turns_file = std::path::PathBuf::from(format!("{}.turns", std::path::Path::new(&info).display()));
    let mut turns_seen = 0usize;
    let stop = std::path::PathBuf::from(format!("{}.stop", std::path::Path::new(&info).display()));
    let body = json!({"port": h.svc.local_addr().port(), "fingerprint": h.svc.fingerprint(), "profile": profile, "admin": "root", "admin_password": "root-password-1", "user": "ann", "user_password": "ann-password-1"});
    let tmp = std::path::PathBuf::from(format!("{}.tmp", std::path::Path::new(&info).display()));
    std::fs::write(&tmp, body.to_string()).unwrap();
    std::fs::rename(&tmp, &info).unwrap(); // appears whole
    let t0 = Instant::now();
    while !stop.exists() && t0.elapsed() < Duration::from_secs(1200) {
        // the driver appends one line per model turn to `<info>.turns`: `call` (asks to fetch a page: an approval), `err`, `hang`, or `final:TEXT`
        if let Ok(text) = std::fs::read_to_string(&turns_file) {
            let lines: Vec<&str> = text.lines().collect();
            for l in lines.iter().skip(turns_seen) {
                let turn = match *l {
                    "call" => call_turn("web_fetch"),
                    "err" => final_turn("__ERR__"),
                    "hang" => final_turn("__HANG__"),
                    other => final_turn(other.strip_prefix("final:").unwrap_or(other)),
                };
                h.script.turns.lock().unwrap().push_back(turn);
            }
            turns_seen = lines.len();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let dir = h.dir.clone();
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cloud_profile_offers_tiles_list_and_the_agent_api_profile_does_not() {
    let cloud = tools::allowed_set(Profile::Cloud, &[], &[]).unwrap();
    assert!(cloud.contains("tiles_list"), "{cloud:?}");
    assert!(!tools::allowed_set(Profile::AgentApi, &[], &[]).unwrap().contains("tiles_list"));
    // the wrapper that adds the tool names the built-in tile catalog
    let t = crate::tilestools::TilesTools::new(std::sync::Arc::new(NoTools), crate::tilestools::TilesTools::builtin_catalog());
    let text = Tools::call(&t, "tiles_list", &serde_json::json!({})).unwrap();
    assert!(text.contains("Tiles that can be opened"), "{text}");
}

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("unknown tool {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.to_string()
    }
}

#[test]
fn a_cloud_run_publishes_a_team_host_only_after_the_person_approves_the_card() {
    let h = harness("cloud", |_| {});
    let ann = h.ann();
    let turn = ModelTurn { text: String::new(), calls: vec![ToolCall { id: "c-host".into(), name: "deploy_team_host".into(), args: json!({"team": "red", "name": "gpu"}) }] };
    let run = h.start_run(&ann, vec![turn.clone(), final_turn("published")]);
    let ev = h.wait_event(&ann, &run, |e| e["type"] == "approval");
    let ask = ev.iter().find(|e| e["type"] == "approval").expect("the run asks first");
    assert!(ask.to_string().contains("deploy_team_host"), "{ask}");
    assert!(h.executed.lock().unwrap().is_empty(), "nothing is applied before the answer");
    let call = ask["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/approvals/{run}/{call}");
    assert_eq!(h.call("POST", &path, Some(&ann), Some(json!({"verdict": "approve"}))).0, 200);
    let ev = h.wait_event(&ann, &run, |e| e["type"] == "final");
    assert!(ev.iter().any(|e| e["type"] == "final" && e["text"] == "published"));
    assert_eq!(*h.executed.lock().unwrap(), ["apply:red/gpu@dev"]);
    // a denied card applies nothing
    let run2 = h.start_run(&ann, vec![turn, final_turn("not published")]);
    let ev = h.wait_event(&ann, &run2, |e| e["type"] == "approval");
    let c2 = ev.iter().find(|e| e["type"] == "approval").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/{run2}/{c2}"), Some(&ann), Some(json!({"verdict": "deny"}))).0, 200);
    let ev = h.wait_event(&ann, &run2, |e| e["type"] == "final");
    assert!(ev.iter().any(|e| e["type"] == "decision" && e["approved"] == false));
    assert_eq!(h.executed.lock().unwrap().len(), 1);
    // the agent-API profile never offers it
    let h2 = harness("agent-api", |_| {});
    let a2 = h2.ann();
    let run3 = h2.start_run(&a2, vec![ModelTurn { text: String::new(), calls: vec![ToolCall { id: "c-x".into(), name: "deploy_team_host".into(), args: json!({"team": "red", "name": "gpu"}) }] }, final_turn("no")]);
    h2.wait_event(&a2, &run3, |e| e["type"] == "final");
    assert!(h2.executed.lock().unwrap().is_empty());
}

#[test]
fn metrics_are_prometheus_text_for_administrators_only_and_count_by_route_and_status_class_without_names() {
    let h = harness("agent-api", |_| {});
    let (ann, root) = (h.ann(), h.root());
    // no sign-in: refused; a person who is not an administrator: refused
    assert_eq!(h.call("GET", "/metrics", None, None).0, 401);
    assert_eq!(h.call("GET", "/metrics", Some(&ann), None).0, 403);
    // some traffic: a good call, a refused one, a missing page, a failed sign-in
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/nonesuch", Some(&ann), None).0, 404);
    assert_eq!(h.call("POST", "/api/v1/login", None, Some(json!({"name": "ann", "password": "wrong-password-1"}))).0, 401);
    let (s, _, resp) = h.call("GET", "/metrics", Some(&root), None);
    assert_eq!(s, 200);
    let ct = resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone()).unwrap_or_default();
    assert!(ct.starts_with("text/plain; version=0.0.4"), "{ct}");
    let text = String::from_utf8(resp.body.clone()).unwrap();
    assert!(text.contains("iyakki_requests_total{route=\"whoami\",status=\"2xx\"} 2\n"), "{text}");
    assert!(text.contains("iyakki_requests_total{route=\"unknown\",status=\"4xx\"} 1\n"), "a path that is no route is counted under one fixed label: {text}");
    assert!(text.contains("iyakki_logins_total{outcome=\"failed\"} 1\n") && text.contains("iyakki_logins_total{outcome=\"ok\"} "), "{text}");
    assert!(text.contains("iyakki_build_info{profile=\"agent-api\",version=\"") && text.contains("iyakki_sessions "), "{text}");
    // nothing that identifies a person, a token or an address is in it
    for secret in ["ann", "root", "10.1.1.1", &ann, &root, "password"] {
        assert!(!text.contains(&format!("\"{secret}")) && !text.contains(&format!("={secret}")) && !text.contains(&format!("{secret}\"")), "{secret} in {text}");
    }
    assert!(!text.contains(&ann) && !text.contains(&root));
    // a second scrape counts the first one
    let (_, _, again) = h.call("GET", "/metrics", Some(&root), None);
    assert!(String::from_utf8(again.body).unwrap().contains("iyakki_requests_total{route=\"metrics\",status=\"2xx\"} 1\n"));
    // POST to it does not exist
    assert_eq!(h.call("POST", "/metrics", Some(&root), None).0, 405);
    // it is part of the described API
    assert!(openapi::document(Profile::AgentApi)["paths"].get("/metrics").is_some());
}

#[test]
fn metrics_count_runs_approvals_and_flow_runs_and_say_how_many_runs_are_held_and_how_much_room_is_left() {
    let h = harness("agent-api", |_| {});
    let (ann, root) = (h.ann(), h.root());
    h.script.turns.lock().unwrap().push_back(final_turn("done"));
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "hello"})));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    // a refused start is not counted as a start
    assert_eq!(h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({}))).0, 400);
    let end = std::time::Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    while std::time::Instant::now() < end {
        let (_, _, resp) = h.call("GET", "/metrics", Some(&root), None);
        text = String::from_utf8(resp.body).unwrap();
        if text.contains("iyakki_runs_held{status=\"completed\"} 1\n") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(text.contains("iyakki_events_total{event=\"runs_started\"} 1\n"), "{text}");
    assert!(text.contains("iyakki_runs_held{status=\"completed\"} 1\n") && text.contains("iyakki_runs_held{status=\"running\"} 0\n"), "{text}");
    assert!(text.contains("iyakki_approvals_pending 0\n") && text.contains("iyakki_tool_results_held{outcome=\"ok\"} 0\n"), "{text}");
    assert!(text.contains("iyakki_data_free_bytes "), "{text}");
    #[cfg(target_os = "linux")]
    assert!(text.contains("iyakki_process_resident_memory_bytes "), "{text}");
    // a stop of that run is counted when the call succeeded
    let _ = h.call("POST", &format!("/api/v1/chat/runs/{id}/stop"), Some(&ann), None);
    assert!(!text.contains(&id), "no run id is ever in the numbers");
    // every line is still well formed
    for l in text.lines().filter(|l| !l.starts_with('#')) {
        assert!(l.rsplit_once(' ').is_some_and(|(_, v)| v.parse::<f64>().is_ok()), "{l}");
    }
}

#[test]
fn metrics_keep_running_totals_of_tool_calls_and_model_tokens_that_agree_with_the_runs_held() {
    let h = harness("agent-api", |_| {});
    let (ann, root) = (h.ann(), h.root());
    let metrics = |h: &H| String::from_utf8(h.call("GET", "/metrics", Some(&root), None).2.body).unwrap();
    let t0 = metrics(&h);
    assert!(t0.contains("iyakki_tool_calls_total{outcome=\"ok\"} 0\n") && t0.contains("iyakki_tool_calls_total{outcome=\"failed\"} 0\n") && t0.contains("iyakki_model_tokens_total 0\n"), "{t0}");
    let run = h.start_run(&ann, vec![call_turn("web_fetch"), final_turn("fetched")]);
    let ev = h.wait_event(&ann, &run, |e| e["type"] == "approval");
    let call = ev.iter().find(|e| e["type"] == "approval").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(h.call("POST", &format!("/api/v1/approvals/{run}/{call}"), Some(&ann), Some(json!({"verdict": "approve"}))).0, 200);
    h.wait_event(&ann, &run, |e| e["type"] == "final");
    let t1 = metrics(&h);
    assert!(t1.contains("iyakki_tool_calls_total{outcome=\"ok\"} 1\n"), "{t1}");
    assert!(t1.contains("iyakki_tool_results_held{outcome=\"ok\"} 1\n"), "the total agrees with what the held run shows: {t1}");
    assert!(!t1.contains(&run) && !t1.contains("web_fetch"), "no run id and no tool name in the numbers");
    for l in t1.lines().filter(|l| !l.starts_with('#')) {
        assert!(l.rsplit_once(' ').is_some_and(|(_, v)| v.parse::<f64>().is_ok()), "{l}");
    }
}

#[test]
fn the_token_total_adds_only_what_a_run_used_since_its_last_report_and_never_goes_down() {
    use super::runs::{count_event, Totals};
    use std::sync::atomic::Ordering::Relaxed;
    let t = Totals::default();
    let mut last = 0u64;
    let usage = |tokens| crate::agent::RunEvent::Usage { tokens, cost_cents: 0.0, files: 0, seconds: 1 };
    for tokens in [100, 250, 250, 240, 400] {
        count_event(&t, &mut last, &usage(tokens));
    }
    assert_eq!(t.tokens.load(Relaxed), 400, "the growth of one run: 100, +150, +0, +0 (a lower report adds nothing), +150");
    let mut other = 0u64;
    count_event(&t, &mut other, &usage(30));
    assert_eq!(t.tokens.load(Relaxed), 430, "a second run adds its own");
    count_event(&t, &mut other, &crate::agent::RunEvent::ToolResult { id: "1".into(), name: "x".into(), ok: false, text: String::new() });
    count_event(&t, &mut other, &crate::agent::RunEvent::ToolResult { id: "2".into(), name: "x".into(), ok: true, text: String::new() });
    assert_eq!((t.tool_ok.load(Relaxed), t.tool_failed.load(Relaxed)), (1, 1));
}

#[test]
fn every_route_that_changes_something_is_audited_except_the_ones_listed_here_with_their_reasons() {
    // A table test: a new route that changes state must say `audit: true` or be added here on purpose, with the reason.
    let exempt: &[(&str, &str)] = &[
        ("login", "its outcome is counted by the sign-in rate limiter and the audit line is written by the login handler itself"),
        ("logout", "ends the caller's own session; nothing of anyone else's changes"),
        ("metrics", "read only (a text, not JSON)"),
    ];
    let mut missing = Vec::new();
    for r in routes::ROUTES {
        if r.method != "GET" && r.method != "HEAD" && !r.audit && !exempt.iter().any(|(id, _)| *id == r.id) {
            missing.push(format!("{} {} ({})", r.method, r.path, r.id));
        }
    }
    assert!(missing.is_empty(), "these routes change state and are not audited (set `audit: true` or list them with a reason): {missing:?}");
    for (id, why) in exempt {
        assert!(!why.is_empty() && routes::ROUTES.iter().any(|r| r.id == *id), "{id} is a route of the table");
    }
}

#[test]
fn a_run_made_outside_the_service_is_announced_to_the_persons_event_stream_when_it_appears_and_when_its_status_changes() {
    let h = harness("agent-api", |_| {});
    let (_ann, _bob) = (h.ann(), h.login("bob", "bob-password-1"));
    let bus = crate::notifyhub::events::bus_for(&h.dir);
    let (mine, theirs) = (bus.subscribe("ann").unwrap(), bus.subscribe("bob").unwrap());
    // the first look at a person only notes what is there
    let store = h.svc.shared().deps.store.clone();
    let log = |id: &str, user: &str, status: &str| crate::runs::RunLog { id: id.into(), user: user.into(), started: 1_700_000_000, prompt: "a secret prompt from the desktop".into(), status: status.into(), events: vec![], session: None, task_id: None, row: None, agent: None };
    std::thread::sleep(Duration::from_millis(2600));
    let (id, other) = (crate::runs::new_run_id(), crate::runs::new_run_id());
    let t0 = std::time::Instant::now();
    store.save(&log(&id, "ann", "running")).unwrap();
    store.save(&log(&other, "bob", "running")).unwrap();
    let mut got: Vec<Value> = Vec::new();
    let end = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < end && !got.iter().any(|k| k["data"]["id"] == id.as_str()) {
        for e in bus.drain(mine, Duration::from_millis(200)).unwrap_or_default() {
            got.push(serde_json::from_str::<Value>(&e).unwrap());
        }
    }
    let first = got.iter().find(|k| k["data"]["id"] == id.as_str()).expect("the run made outside the service was announced");
    // a save wakes the watcher: it does not wait for its two-second turn (a short margin keeps this steady on a busy machine)
    assert!(t0.elapsed() < Duration::from_millis(1700), "announced after {:?}", t0.elapsed());
    assert_eq!((first["type"].as_str(), first["data"]["status"].as_str()), (Some("run"), Some("running")), "{first}");
    // the end is announced too
    store.save(&log(&id, "ann", "completed")).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < end && !got.iter().any(|k| k["data"]["id"] == id.as_str() && k["data"]["status"] == "completed") {
        for e in bus.drain(mine, Duration::from_millis(200)).unwrap_or_default() {
            got.push(serde_json::from_str::<Value>(&e).unwrap());
        }
    }
    assert!(got.iter().any(|k| k["data"]["id"] == id.as_str() && k["data"]["status"] == "completed"), "{got:?}");
    // only words and counts travel, and only to the person whose run it is
    let all = got.iter().map(|k| k.to_string()).collect::<String>();
    assert!(!all.contains("secret prompt") && !all.contains(&other), "{all}");
    let bobs = bus.drain(theirs, Duration::from_millis(50)).unwrap_or_default();
    assert!(bobs.iter().all(|e| !e.contains(&id)), "{bobs:?}");
}

#[test]
fn a_run_tells_the_persons_event_stream_when_it_starts_and_ends_and_nobody_elses() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    let bus = crate::notifyhub::events::bus_for(&h.dir);
    let (mine, theirs) = (bus.subscribe("ann").unwrap(), bus.subscribe("bob").unwrap());
    h.script.turns.lock().unwrap().push_back(final_turn("the answer"));
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "hello"})));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let mut kinds = Vec::new();
    let end = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < end && !kinds.iter().any(|k: &Value| k["type"] == "run" && k["data"]["status"] == "completed") {
        for e in bus.drain(mine, Duration::from_millis(200)).unwrap_or_default() {
            kinds.push(serde_json::from_str::<Value>(&e).unwrap());
        }
    }
    let runs: Vec<&Value> = kinds.iter().filter(|k| k["type"] == "run").collect();
    assert!(runs.len() >= 2, "{kinds:?}");
    assert_eq!(runs[0]["data"]["id"], id.as_str());
    assert_eq!(runs[0]["data"]["status"], "running", "the start is told first");
    assert_eq!(runs.last().unwrap()["data"]["status"], "completed", "and the end last");
    // only counts and words travel: no prompt, no answer
    let all = kinds.iter().map(|k| k.to_string()).collect::<String>();
    assert!(!all.contains("hello") && !all.contains("the answer"), "{all}");
    // the other person's stream got no run event of ours
    let others = bus.drain(theirs, Duration::from_millis(50)).unwrap_or_default();
    assert!(others.iter().all(|e| !e.contains("\"type\":\"run\"")), "{others:?}");
    let _ = bob;
}

#[test]
fn a_flow_run_the_person_starts_is_followed_and_its_status_is_told_to_their_stream() {
    let h = harness("cloud", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    let bus = crate::notifyhub::events::bus_for(&h.dir);
    let (mine, theirs) = (bus.subscribe("ann").unwrap(), bus.subscribe("bob").unwrap());
    let (s, v, _) = h.call("POST", "/api/v1/flows/f1/run", Some(&ann), Some(json!({})));
    assert_eq!(s, 200, "{v}");
    let mut seen = Vec::new();
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < end && !seen.iter().any(|k: &Value| k["type"] == "flowrun") {
        for e in bus.drain(mine, Duration::from_millis(200)).unwrap_or_default() {
            seen.push(serde_json::from_str::<Value>(&e).unwrap());
        }
    }
    let ev = seen.iter().find(|k| k["type"] == "flowrun").unwrap_or_else(|| panic!("{seen:?}"));
    assert_eq!((ev["data"]["flow"].as_str(), ev["data"]["run"].as_str(), ev["data"]["status"].as_str()), (Some("f1"), Some("r1"), Some("succeeded")));
    assert_eq!(ev["data"].as_object().unwrap().len(), 3, "the words only: {ev}");
    // another person's stream hears nothing of it
    let others = bus.drain(theirs, Duration::from_millis(50)).unwrap_or_default();
    assert!(others.iter().all(|e| !e.contains("flowrun")), "{others:?}");
    let _ = bob;
}

#[test]
fn the_flows_runner_profile_offers_sign_in_and_the_flows_and_nothing_else() {
    let h = harness("flows-runner", |_| {});
    let (ann, gus) = (h.ann(), h.login("gus", "gus-password-1"));
    // the flows of the person: list, read, start, follow
    let (s, v, _) = h.call("GET", "/api/v1/flows", Some(&ann), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["flows"][0]["id"], "f1");
    assert_eq!(h.call("GET", "/api/v1/flows/f1", Some(&ann), None).0, 200);
    let (s, v, _) = h.call("POST", "/api/v1/flows/f1/run", Some(&ann), Some(json!({"inputs": {}})));
    assert_eq!((s, v["run"].as_str()), (200, Some("r1")), "{v}");
    let (s, v, _) = h.call("GET", "/api/v1/flows/f1/runs/r1", Some(&ann), None);
    assert_eq!((s, v["status"].as_str()), (200, Some("succeeded")), "{v}");
    assert_eq!(h.call("GET", "/api/v1/flows/nope/runs/r1", Some(&ann), None).0, 404);
    // health, version, who am I and the API description of this profile
    for p in ["/healthz", "/readyz", "/api/v1/version"] {
        assert_eq!(h.call("GET", p, None, None).0, 200, "{p}");
    }
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&ann), None).0, 200);
    let (s, doc, _) = h.call("GET", "/api/v1/openapi.json", Some(&ann), None);
    assert_eq!(s, 200);
    let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
    assert!(paths.iter().any(|p| p.contains("/flows")) && !paths.iter().any(|p| p.contains("/chat/") || p.contains("/models") || p.contains("/tiles") || p.contains("/data/")), "{paths:?}");
    // everything else is not there: no chat, models, tools, tiles, data, approvals, files
    for (m, p) in [("GET", "/api/v1/models"), ("GET", "/api/v1/tools"), ("GET", "/api/v1/chat/runs"), ("POST", "/api/v1/chat/runs"), ("GET", "/api/v1/tiles"), ("POST", "/api/v1/data/call"), ("GET", "/api/v1/approvals"), ("GET", "/api/v1/files")] {
        assert_eq!(h.call(m, p, Some(&ann), if m == "POST" { Some(json!({})) } else { None }).0, 404, "{m} {p}");
    }
    // a person without the flows right is refused where the cloud profile would refuse
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&gus), None).0, 403);
    // no chat tool at all
    assert!(Profile::FlowsRunner.tools().is_empty());
    // import: a safe draft with a free id; a body that is not a flow is refused
    let (s, v, _) = h.call("POST", "/api/v1/flows/import", Some(&ann), Some(json!({"flow": {"id": "f9", "name": "Nine"}})));
    assert_eq!((s, v["id"].as_str()), (200, Some("f9-2")), "{v}");
    assert_eq!(h.call("POST", "/api/v1/flows/import", Some(&ann), Some(json!({"flow": 5}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/flows/import", Some(&ann), Some(json!({"flow": {"name": "no id"}}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/flows/import", Some(&ann), Some(json!({"flow": {}, "extra": 1}))).0, 400, "an unknown field is refused");
    // validate: by id or by a whole flow, never both
    let (s, v, _) = h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({"id": "f1"})));
    assert_eq!((s, v["ok"].as_bool(), v["issues"][0]["level"].as_str()), (200, Some(true), Some("warning")), "{v}");
    let (_, v, _) = h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({"flow": {"broken": true}})));
    assert_eq!((v["ok"].as_bool(), v["issues"][0]["message"].as_str()), (Some(false), Some("no start step")), "{v}");
    assert_eq!(h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({"id": "f1", "flow": {}}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({"id": "../x"}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/flows/validate", Some(&ann), Some(json!({"id": "nope"}))).0, 404);
    // stop: the run of the person's own flow; another run is not found
    assert_eq!(h.call("POST", "/api/v1/flows/f1/runs/r1/stop", Some(&ann), None).0, 200);
    assert_eq!(h.call("POST", "/api/v1/flows/f1/runs/r9/stop", Some(&ann), None).0, 404);
    // a step that waits for a person is answered by the person's own call: approve or deny, once, only their own run
    let step = |s: &str, tok: &str, body: Value| h.call("POST", &format!("/api/v1/flows/f1/runs/r1/steps/{s}/decide"), Some(tok), Some(body)).0;
    assert_eq!(step("ask", &ann, json!({"verdict": "approve"})), 200);
    assert_eq!(step("ask", &ann, json!({"verdict": "deny"})), 200);
    assert_eq!(step("done", &ann, json!({"verdict": "approve"})), 404, "a step that was answered is not waiting any more");
    assert_eq!(step("nope", &ann, json!({"verdict": "approve"})), 404);
    assert_eq!(step("ask", &ann, json!({"verdict": "maybe"})), 400);
    assert_eq!(step("ask", &ann, json!({})), 400);
    assert_eq!(step("ask", &gus, json!({"verdict": "approve"})), 403);
    assert_eq!(h.call("POST", "/api/v1/flows/f1/runs/r9/steps/ask/decide", Some(&ann), Some(json!({"verdict": "approve"}))).0, 404);
    // the cloud profile has no such route: its approvals go through the tenant approval router
    let c = harness("cloud", |_| {});
    let ct = c.ann();
    assert_eq!(c.call("POST", "/api/v1/flows/f1/runs/r1/steps/ask/decide", Some(&ct), Some(json!({"verdict": "approve"}))).0, 404);
    // schedules are on the flows runner too
    assert_eq!(h.call("GET", "/api/v1/schedules", Some(&ann), None).0, 200);
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("flows_import") && lines.contains("flows_run_stop"), "{lines}");
    // nobody else: the guest may do none of it
    for (m, p) in [("POST", "/api/v1/flows/import"), ("POST", "/api/v1/flows/validate"), ("POST", "/api/v1/flows/f1/runs/r1/stop")] {
        assert_eq!(h.call(m, p, Some(&gus), Some(json!({}))).0, 403, "{p}");
    }
}

#[test]
fn the_data_api_profile_offers_sign_in_and_the_read_only_data_tools_and_nothing_else() {
    let h = harness("data-api", |_| {});
    let (ann, gus) = (h.ann(), h.login("gus", "gus-password-1"));
    assert!(Profile::DataApi.tools().iter().all(|t| t.starts_with("data_")) && Profile::DataApi.tools().len() == 3);
    for p in ["/healthz", "/readyz", "/api/v1/version"] {
        assert_eq!(h.call("GET", p, None, None).0, 200, "{p}");
    }
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&ann), None).0, 200);
    // the three data tools work; others are not found; a call needs the data right
    for tool in ["data_open", "data_describe", "data_query"] {
        let (s, v, _) = h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": tool, "args": {}})));
        assert_eq!(s, 200, "{tool}: {v}");
    }
    for tool in ["data_export", "run_command", "web_fetch", "flow_run"] {
        assert_eq!(h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": tool, "args": {}}))).0, 404, "{tool}");
    }
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&gus), Some(json!({"tool": "data_query", "args": {}}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/data/call", Some(&ann), Some(json!({"tool": "data_query", "args": 5}))).0, 400);
    // nothing else is there
    for (m, p) in [("GET", "/api/v1/models"), ("GET", "/api/v1/tools"), ("GET", "/api/v1/chat/runs"), ("GET", "/api/v1/flows"), ("GET", "/api/v1/tiles"), ("GET", "/api/v1/approvals"), ("GET", "/api/v1/schedules")] {
        assert_eq!(h.call(m, p, Some(&ann), None).0, 404, "{m} {p}");
    }
    let (_, doc, _) = h.call("GET", "/api/v1/openapi.json", Some(&ann), None);
    let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
    assert!(paths.iter().any(|p| p.contains("/data/call")) && !paths.iter().any(|p| p.contains("/flows") || p.contains("/chat/") || p.contains("/tiles")), "{paths:?}");
    // the call is audited with the tool's name and no arguments
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tool=data_query") && !lines.contains("secret"), "{lines}");
}

#[test]
fn the_tile_host_profile_offers_sign_in_and_the_tiles_and_nothing_else() {
    let h = harness("tile-host", |_| {});
    let ann = h.ann();
    assert!(Profile::TileHost.tools() == ["tile_bundle_list", "tile_bundle_install", "tile_bundle_remove"], "the only tools are the three that manage web-bundle tiles, given to administrators");
    assert!(Profile::parse("tile-host") == Ok(Profile::TileHost));
    for p in ["/healthz", "/readyz", "/api/v1/version"] {
        assert_eq!(h.call("GET", p, None, None).0, 200, "{p}");
    }
    assert_eq!(h.call("GET", "/api/v1/whoami", Some(&ann), None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/tiles", None, None).0, 401, "the tile list needs a sign-in");
    // the tiles are listed and one is drawn and answers events
    let (s, list, _) = h.call("GET", "/api/v1/tiles", Some(&ann), None);
    assert_eq!(s, 200, "{list}");
    assert!(list.to_string().contains("settings"), "{list}");
    let (s, v, _) = h.call("POST", "/api/v1/tiles/settings/events", Some(&ann), Some(json!({"width": 360, "height": 480})));
    assert_eq!(s, 200, "{v}");
    assert!(v.to_string().contains("SETTINGS"), "{v}");
    // nothing else is there
    for (m, p) in [("GET", "/api/v1/models"), ("GET", "/api/v1/tools"), ("GET", "/api/v1/chat/runs"), ("GET", "/api/v1/flows"), ("GET", "/api/v1/approvals"), ("GET", "/api/v1/schedules"), ("POST", "/api/v1/data/call"), ("GET", "/api/v1/files")] {
        assert_eq!(h.call(m, p, Some(&ann), None).0, 404, "{m} {p}");
    }
    let (_, doc, _) = h.call("GET", "/api/v1/openapi.json", Some(&ann), None);
    let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
    assert!(paths.iter().any(|p| p.contains("/tiles")) && !paths.iter().any(|p| p.contains("/flows") || p.contains("/chat/") || p.contains("/data/")), "{paths:?}");
}

#[test]
fn a_paired_computer_reads_metrics_only_with_the_metrics_scope_and_only_as_an_administrators_program() {
    use crate::devmode::auth::{hex, sign_headers};
    let h = harness("cloud", |_| {});
    let fp = h.svc.fingerprint().to_string();
    let login = |seed: u8, computer: &str, user: &str| -> String {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        h.extras.progs.lock().unwrap().insert(hex(key.verifying_key().as_bytes()), (computer.into(), user.into()));
        let mut r = req("POST", "/api/v1/login", None, None);
        for (k, v) in sign_headers(&key, "POST", "/api/v1/login", &[], &[], &fp, crate::clock::now_unix()).unwrap() {
            r.headers.insert(k, v);
        }
        let (s, v, _) = h.call_req(r);
        assert_eq!(s, 200, "{v}");
        v["token"].as_str().unwrap().to_string()
    };
    // a computer with no scope cannot even sign in; one with another scope signs in and reads no numbers
    h.extras.pscopes.lock().unwrap().insert("ops".into(), vec!["api:status".into()]);
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:status".into()]);
    let ops = login(11, "ops", "root");
    let plain = login(12, "laptop", "ann");
    assert_eq!(h.call("GET", "/metrics", Some(&ops), None).0, 403, "another scope reads no numbers");
    h.extras.pscopes.lock().unwrap().insert("ops".into(), vec!["api:metrics:read".into()]);
    h.extras.pscopes.lock().unwrap().insert("laptop".into(), vec!["api:metrics".into()]);
    let (s, _, body) = h.call("GET", "/metrics", Some(&ops), None);
    assert_eq!(s, 200);
    let text = String::from_utf8(body.body).unwrap();
    assert!(text.contains("# TYPE") && !text.contains("root") && !text.contains(&ops), "numbers only, no names or tokens");
    assert_eq!(h.call("GET", "/metrics", Some(&plain), None).0, 403, "the scope does not make an ordinary account an administrator");
    assert_eq!(h.call("GET", "/api/v1/admin/status", Some(&ops), None).0, 403, "the scope opens /metrics and nothing else of the administrator's");
    assert_eq!(h.call("GET", "/api/v1/flows", Some(&ops), None).0, 403, "and no other family");
    h.extras.pscopes.lock().unwrap().insert("ops".into(), vec!["api:status".into()]);
    assert_eq!(h.call("GET", "/metrics", Some(&ops), None).0, 403, "taken back at once");
}

#[test]
fn the_cli_client_follows_the_event_socket_with_the_paired_key_and_prints_each_event() {
    use crate::devmode::auth::hex;
    use crate::devmode::webclient::{event_line, ApiClient};
    let (h, _, _, _, _) = acme_cloud();
    let key = ed25519_dalek::SigningKey::from_bytes(&[21u8; 32]);
    h.extras.progs.lock().unwrap().insert(hex(key.verifying_key().as_bytes()), ("ci".into(), "ann".into()));
    h.extras.pscopes.lock().unwrap().insert("ci".into(), vec!["api:tiles:read".into(), "api:files".into()]);
    let (port, fp) = (h.svc.local_addr().port(), h.svc.fingerprint().to_string());
    let api = ApiClient::with_key("127.0.0.1", port, &fp, key.clone());
    let doer = ApiClient::with_key("127.0.0.1", port, &fp, key);
    doer.sign_in().unwrap();
    // a change made through the API while the follower waits arrives as an event, with no polling
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        doer.call("POST", "/api/v1/files/mkdir", &[], Some(&json!({"namespace": "web", "path": "followed"})))
    });
    let mut lines = Vec::new();
    let n = api.follow(std::time::Duration::from_secs(10), 3, &mut |v| lines.push(event_line(v))).unwrap();
    let made = t.join().unwrap().unwrap();
    assert_eq!(made.0, 200, "the change itself worked: {made:?}");
    assert_eq!(n, 3, "{lines:?}");
    assert!(lines[0].starts_with("ready"), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("changed")), "the change arrived as an event: {lines:?}");
    assert!(!lines.iter().any(|l| l.contains("token")), "a token is never printed");
    // without the tiles scope the stream is refused in the service's own words
    let h2 = harness("cloud", |_| {});
    let k2 = ed25519_dalek::SigningKey::from_bytes(&[22u8; 32]);
    h2.extras.progs.lock().unwrap().insert(hex(k2.verifying_key().as_bytes()), ("ci".into(), "ann".into()));
    h2.extras.pscopes.lock().unwrap().insert("ci".into(), vec!["api:status".into()]);
    let api2 = ApiClient::with_key("127.0.0.1", h2.svc.local_addr().port(), h2.svc.fingerprint(), k2);
    let err = api2.follow(std::time::Duration::from_secs(5), 1, &mut |_| {}).unwrap_err();
    assert!(!err.is_empty(), "refused: {err}");
}

#[test]
fn a_data_import_waits_for_an_approval_from_another_sign_in_and_runs_once() {
    let c = harness("cloud", |_| {});
    let bus = crate::notifyhub::events::bus_for(&c.dir);
    let sub = bus.subscribe("ann").unwrap();
    let (script, browser) = (c.ann(), c.ann());
    let body = json!({"tool": "data_create", "args": {"name": "sales.csv", "columns": ["a", "b"], "rows": [["secret-cell", 1], ["x", 2]]}});
    let (s, v, _) = c.call("POST", "/api/v1/data/call", Some(&script), Some(body));
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["status"], "pending");
    assert_eq!(v["summary"], "create the table file sales.csv with 2 columns and 2 rows");
    assert!(!v.to_string().contains("secret-cell"));
    let id = v["id"].as_str().unwrap().to_string();
    // nothing ran yet
    assert_eq!(c.call("GET", &format!("/api/v1/data/imports/{id}"), Some(&script), None).1["status"], "pending");
    // it shows in the person's approvals list, another person sees nothing
    let (_, list, _) = c.call("GET", "/api/v1/approvals", Some(&browser), None);
    assert!(list["approvals"].as_array().unwrap().iter().any(|a| a["run"] == "data" && a["call"] == id.as_str()), "{list}");
    let bob = c.login("bob", "bob-password-1");
    assert!(c.call("GET", "/api/v1/approvals", Some(&bob), None).1["approvals"].as_array().unwrap().iter().all(|a| a["run"] != "data"));
    assert_eq!(c.call("POST", &format!("/api/v1/approvals/data/{id}"), Some(&bob), Some(json!({"verdict": "approve"}))).0, 404);
    // the asking token cannot approve its own import
    assert_eq!(c.call("POST", &format!("/api/v1/approvals/data/{id}"), Some(&script), Some(json!({"verdict": "approve"}))).0, 403);
    let (s, v, _) = c.call("POST", &format!("/api/v1/approvals/data/{id}"), Some(&browser), Some(json!({"verdict": "approve"})));
    assert_eq!((s, v["outcome"].as_str()), (200, Some("done")), "{v}");
    assert_eq!(c.call("POST", &format!("/api/v1/approvals/data/{id}"), Some(&browser), Some(json!({"verdict": "approve"}))).0, 409);
    let (s, v, _) = c.call("GET", &format!("/api/v1/data/imports/{id}"), Some(&script), None);
    assert_eq!((s, v["status"].as_str(), v["result"].as_str()), (200, Some("done"), Some("data_create for ann")));
    assert_eq!(c.call("GET", &format!("/api/v1/data/imports/{id}"), Some(&script), None).0, 404, "read once");
    // a refusal leaves nothing
    let (_, v, _) = c.call("POST", "/api/v1/data/call", Some(&script), Some(json!({"tool": "data_append", "args": {"path": "sales.csv", "rows": [[1, 2]]}})));
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(c.call("POST", &format!("/api/v1/approvals/data/{id}"), Some(&browser), Some(json!({"verdict": "deny"}))).0, 200);
    assert_eq!(c.call("GET", &format!("/api/v1/data/imports/{id}"), Some(&script), None).1["status"], "denied");
    // the stream carried the answers: names and states only, never the rows
    let seen: Vec<String> = bus.drain(sub, Duration::ZERO).unwrap_or_default();
    assert!(seen.iter().any(|e| e.contains("\"data_import\"") && e.contains("\"done\"")), "{seen:?}");
    assert!(seen.iter().any(|e| e.contains("\"data_import\"") && e.contains("\"denied\"")), "{seen:?}");
    assert!(seen.iter().all(|e| !e.contains("secret-cell")), "{seen:?}");
    // the agent API profile has no such tool
    let a = harness("data-api", |_| {});
    let t = a.ann();
    assert_eq!(a.call("POST", "/api/v1/data/call", Some(&t), Some(json!({"tool": "data_create", "args": {"name": "a.csv", "columns": ["a"], "rows": []}}))).0, 404);
}

#[test]
fn the_real_connector_pins_the_service_signs_in_lists_the_tenants_and_switches_over_real_tls_against_the_real_cloud_routes() {
    use iyakki_connect::{pins::Store, run, Ctx, Fail};
    let (h, _root, _ann, _bob, _gus) = acme_cloud();
    let addr = format!("127.0.0.1:{}", h.svc.local_addr().port());
    let dir = std::env::temp_dir().join(format!("iyakki-connect-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let c = Ctx { store: Store::new(&dir), timeout: Duration::from_secs(8), interactive: false };
    let go = |input: &str, args: &[&str]| -> (Result<(), Fail>, String) {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let r = run(&a, &c, &mut std::io::Cursor::new(input.to_string()), &mut out);
        (r, String::from_utf8(out).unwrap())
    };
    // not pinned: refused before anything is asked; then pinned by the certificate the service really shows
    assert!(matches!(go("ann-password-1\n", &["tenants", &addr, "ann"]).0, Err(Fail::Error(m)) if m.contains("not pinned")));
    go("", &["pin", &addr, "--yes"]).0.expect("the pin of the real certificate");
    let (res, out) = go("ann-password-1\n", &["tenants", &addr, "ann"]);
    res.unwrap_or_else(|e| panic!("{e:?}"));
    assert!(out.lines().count() >= 1 && (out.contains("member") || out.contains("owner") || out.contains("admin")), "{out}");
    assert!(out.lines().any(|l| l.starts_with("*")), "the current tenant is marked: {out}");
    // a wrong password and an empty one are refused by the service and by the connector
    assert!(matches!(go("wrong-password\n", &["tenants", &addr, "ann"]).0, Err(Fail::Error(m)) if m.contains("refused")));
    assert!(matches!(go("\n", &["tenants", &addr, "ann"]).0, Err(Fail::Error(m)) if m.contains("no password")));
    // switching to a tenant that does not exist is the service's refusal in the person's words
    let (res, _) = go("ann-password-1\n", &["switch", &addr, "ann", "no-such-tenant"]);
    assert!(matches!(res, Err(Fail::Error(ref m)) if !m.is_empty()), "{res:?}");
    // a certificate that changed is refused by the pin: a second service has another certificate
    let other = acme_cloud();
    let other_addr = format!("127.0.0.1:{}", other.0.svc.local_addr().port());
    let (res, _) = go("ann-password-1\n", &["tenants", &other_addr, "ann"]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("not pinned")), "another service is not trusted because the first was");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_web_command_starts_a_flow_and_follows_its_run_to_the_end_over_the_event_stream() {
    use crate::devmode::auth::hex;
    let h = harness("cloud", |_| {});
    let key = ed25519_dalek::SigningKey::from_bytes(&[31u8; 32]);
    h.extras.progs.lock().unwrap().insert(hex(key.verifying_key().as_bytes()), ("ci".into(), "ann".into()));
    h.extras.pscopes.lock().unwrap().insert("ci".into(), vec!["api:flows".into(), "api:tiles:read".into()]);
    let home = crate::testdir::outside_project("web-cli", "follow");
    std::fs::write(home.join("id.key"), crate::pkg::signing_key_hex(&key)).unwrap();
    let addr = format!("127.0.0.1:{}", h.svc.local_addr().port());
    let cmd = |rest: &[&str]| -> Result<Vec<String>, String> {
        let mut a: Vec<String> = vec!["web".into(), addr.clone(), "--fingerprint".into(), h.svc.fingerprint().into(), "--home".into(), home.display().to_string()];
        a.extend(rest.iter().map(|s| s.to_string()));
        let mut lines = Vec::new();
        crate::devmode::cli::run(&a, &mut |l| lines.push(l.to_string())).map(|_| lines)
    };
    // without --follow it prints the start and returns
    let plain = cmd(&["flow-run", "f1"]).unwrap();
    assert_eq!(plain.len(), 1, "{plain:?}");
    // with --follow it ends by itself when the run it started has ended, and says how it ended
    let lines = cmd(&["flow-run", "f1", "--follow", "--seconds", "20"]).unwrap();
    assert!(lines.iter().any(|l| l.starts_with("flowrun") && l.contains("status=succeeded")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.to_lowercase().contains("token")), "{lines:?}");
    // the lists work, and a flow id with a slash is refused before anything is sent
    assert!(cmd(&["flows"]).is_ok());
    assert!(cmd(&["flow-runs", "a/b"]).is_err());
    assert!(cmd(&["nonsense"]).is_err());
}

#[test]
fn a_dropped_or_unreachable_stream_is_opened_again_a_few_times_then_said_plainly() {
    use crate::devmode::webclient::ApiClient;
    // a port nobody listens on: every attempt fails, each retry is announced, and the end says how many were tried
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let api = ApiClient::with_key("127.0.0.1", port, "AA:BB", ed25519_dalek::SigningKey::from_bytes(&[32u8; 32]));
    let mut told = Vec::new();
    let t = std::time::Instant::now();
    let err = api.follow_reconnecting(Duration::from_secs(30), 10, 2, Duration::from_millis(10), &mut |_| true, &mut |n, why| told.push((n, why.to_string()))).unwrap_err();
    assert_eq!(told.iter().map(|t| t.0).collect::<Vec<_>>(), vec![1, 2], "{told:?}");
    assert!(err.contains("2 new attempts"), "{err}");
    assert!(t.elapsed() < Duration::from_secs(10), "the pause between attempts is bounded");
    // a refusal in the service's own words is not retried
    let h = harness("cloud", |_| {});
    let k = ed25519_dalek::SigningKey::from_bytes(&[33u8; 32]);
    let api = ApiClient::with_key("127.0.0.1", h.svc.local_addr().port(), h.svc.fingerprint(), k);
    let mut n = 0;
    let _ = api.follow_reconnecting(Duration::from_secs(5), 1, 3, Duration::from_millis(10), &mut |_| true, &mut |_, _| n += 1);
    assert_eq!(n, 0, "an unknown computer is refused, not retried");
}

#[test]
fn the_voice_offer_route_needs_a_sign_in_exists_only_in_the_cloud_profile_and_is_audited() {
    let h = harness("cloud", |_| {});
    assert_eq!(h.call("GET", "/api/v1/voice/offer", None, None).0, 401);
    let root = h.root();
    let (s, v, _) = h.call("GET", "/api/v1/voice/offer", Some(&root), None);
    assert_eq!((s, v["addr"].as_str()), (200, Some("cloud.example:7443")), "{v}");
    assert!(v["offer"].as_str().unwrap().contains("root"), "the call carries the signed-in person to the dependency");
    let plain = harness("agent-api", |_| {});
    assert_eq!(plain.call("GET", "/api/v1/voice/offer", Some(&plain.root()), None).0, 404, "a machine that is not a pod has no such route");
}

#[test]
fn the_voice_return_route_hands_the_devices_offer_to_the_dependency_and_keeps_it_out_of_the_audit_line() {
    let h = harness("cloud", |_| {});
    let body = json!({"addr": "203.0.113.9:7444", "offer": "{\"machine\":\"laptop\",\"cert\":\"SECRET-CERT-TEXT\"}"});
    assert_eq!(h.call("POST", "/api/v1/voice/return", None, Some(body.clone())).0, 401, "a sign-in is needed");
    let root = h.root();
    let (s, v, _) = h.call("POST", "/api/v1/voice/return", Some(&root), Some(body));
    assert_eq!((s, v["ok"].as_bool()), (200, Some(true)), "{v}");
    let kept = h.extras.voice_ret.lock().unwrap().clone();
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].0.as_str(), kept[0].1.as_str()), ("root", "203.0.113.9:7444"), "the call carries the signed-in person");
    assert!(kept[0].2.contains("SECRET-CERT-TEXT"));
    let log = h.accounts.lines().join("\n");
    assert!(log.contains("api.voice_return") && !log.contains("SECRET-CERT-TEXT") && !log.contains("203.0.113.9"), "the audit line names the call, never the address or the certificate: {log}");
    // the dependency's refusal and a malformed or oversized body are answered in words, nothing is kept
    let (s, _, _) = h.call("POST", "/api/v1/voice/return", Some(&root), Some(json!({"addr": "no-port", "offer": "x"})));
    assert_eq!(s, 400);
    assert_eq!(h.call("POST", "/api/v1/voice/return", Some(&root), Some(json!({"addr": "1.2.3.4:5"}))).0, 400);
    assert_eq!(h.call("POST", "/api/v1/voice/return", Some(&root), Some(json!({"addr": "1.2.3.4:5", "offer": "x".repeat(20 << 10)}))).0, 400);
    assert_eq!(h.extras.voice_ret.lock().unwrap().len(), 1);
    let plain = harness("agent-api", |_| {});
    assert_eq!(plain.call("POST", "/api/v1/voice/return", Some(&plain.root()), Some(json!({"addr": "1.2.3.4:5", "offer": "x"}))).0, 404, "a machine that is not a pod has no such route");
}
