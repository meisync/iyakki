//! The real implementations of the service's dependencies, over the machine's `Services`. The supervisor adds the accounts and the
//! audit log (they live in the shell crate today) through the `iyakki-core-api` traits, then calls [`assemble`].

use super::deps::{Deps, Extras, ToolSource};
use crate::agent::{Tools, UserTools, AGENT_SYSTEM_PROMPT};
use crate::chat::ChatBackend;
use crate::mcp::ServerSpec;
use crate::services::Services;
use crate::settings::ModelChoice;
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, HostApi, Principal};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ServicesTools {
    svc: Arc<Services>,
    host: Arc<dyn HostApi>,
    audit: Arc<dyn AuditSink>,
}

impl ToolSource for ServicesTools {
    fn tools_for(&self, p: &Principal) -> Arc<dyn Tools> {
        // MCP servers are not passed: their tools are `mcp__*`, high risk, and not in any profile's list in this slice
        let user: Arc<dyn Tools> = Arc::new(UserTools::new(&p.user, p.policy.clone(), self.svc.clone(), self.host.clone(), Vec::<ServerSpec>::new()));
        // tiles_list (free): the cloud chat names the browser tiles too, the same as the desktop chat
        let user: Arc<dyn Tools> = Arc::new(crate::tilestools::TilesTools::new(user, crate::tilestools::TilesTools::builtin_catalog()));
        // deploy_team_host (asks): the team's local model host as a cluster Service, through the team's deploy rules
        Arc::new(crate::hostservicetools::TeamHostTools::new(
            user,
            Arc::new(crate::hostservicetools::RealBackend { dir: self.svc.dir().to_path_buf(), user: p.user.clone(), audit: self.audit.clone(), applier: self.svc.cluster.clone() }),
        ))
    }
}

pub struct ServicesExtras {
    svc: Arc<Services>,
    /// Background threads started by `background_start` (the cloud scheduler); they end when the stop flag is set.
    tasks: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// The core's files facet over the same services (one book of waiting changes for the machine).
    core: Arc<crate::corehandle::AgentCore>,
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io(e.to_string())
}

impl Extras for ServicesExtras {
    fn paired_program(&self, key_hex: &str) -> Option<(String, String)> {
        let dm = crate::devmode::DevMode::open(self.svc.dir());
        dm.find_key(key_hex).map(|k| (k.name, k.account))
    }

    fn program_scopes(&self, computer: &str) -> Vec<String> {
        crate::devmode::DevMode::open(self.svc.dir()).api_scopes(computer)
    }

    fn files(&self) -> Arc<dyn iyakki_core_api::Files> {
        self.core.clone()
    }

    fn browser(&self) -> Arc<dyn iyakki_core_api::BrowserSessions> {
        self.core.shared_browser()
    }

    fn set_tile_status(&self, p: &Principal, tile: &str, badge: Option<u32>, line: &str) {
        self.svc.set_tile_status(&p.user, tile, badge, line);
    }

    fn ready(&self) -> Result<(), String> {
        let probe = self.svc.dir().join(".service-ready");
        std::fs::write(&probe, b"ok").map_err(|e| format!("the data directory is not writable: {e}"))?;
        let _ = std::fs::remove_file(&probe);
        Ok(())
    }

    fn models(&self, _p: &Principal) -> Vec<Value> {
        self.svc.all_models().into_iter().map(|(provider, model)| json!({"provider": provider, "model": model})).collect()
    }

    fn backend_for(&self, _p: &Principal, provider: &str, model: &str) -> ApiResult<Arc<dyn ChatBackend>> {
        if !self.svc.all_models().iter().any(|(p, m)| p == provider && m == model) {
            return Err(ApiError::InvalidInput("that provider and model are not offered (see GET /api/v1/models)".into()));
        }
        Ok(self.svc.chat_backend_for(ModelChoice { provider: provider.into(), model: model.into() }))
    }

    fn background_start(&self, auth: Arc<dyn Authenticator>, audit: Arc<dyn AuditSink>, stop: Arc<std::sync::atomic::AtomicBool>) {
        if let Some(h) = crate::cloudflows::real::start(self.svc.clone(), auth, audit, stop) {
            self.tasks.lock().unwrap_or_else(|e| e.into_inner()).push(h);
        }
    }

    fn background_stop(&self) {
        let hs: Vec<_> = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for h in hs {
            let _ = h.join();
        }
    }

    fn hook_serve(&self, auth: &dyn Authenticator, audit: &dyn AuditSink, rq: &crate::flows::webhook::HookRequest) -> Option<crate::flows::webhook::Answer> {
        let world = crate::flows::hookenv::RealWorld { svc: &self.svc, auth, audit, only: None };
        Some(crate::flows::webhook::Hooks::shared(self.svc.dir()).serve(&world, rq))
    }

    fn flows(&self, p: &Principal) -> ApiResult<Vec<Value>> {
        let env = crate::flows::FlowEnv::build(&self.svc, &p.user, &p.policy, false);
        Ok(env.summaries().into_iter().map(|f| json!({"id": f.id, "name": f.name, "description": f.description, "published": f.published, "nodes": f.nodes, "triggers": f.triggers, "production": f.production})).collect())
    }

    fn flow_export(&self, p: &Principal, id: &str) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::build(&self.svc, &p.user, &p.policy, false);
        let text = env.export_json(id).map_err(|_| ApiError::NotFound("no such flow".into()))?;
        serde_json::from_str(&text).map_err(io)
    }

    fn flow_run(&self, p: &Principal, id: &str, draft: bool, inputs: &Value) -> ApiResult<String> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        if !env.summaries().iter().any(|f| f.id == id) {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        let mut req = iyakki_flow::engine::StartRequest::new(id).trigger("manual", &env.who());
        req.draft = draft || env.store.published(id).is_none();
        if !inputs.is_null() {
            req.inputs = inputs.clone();
        }
        env.engine.start_run(req).map(|o| o.run_id).map_err(ApiError::InvalidInput)
    }

    fn flow_import(&self, p: &Principal, flow: &Value) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let f = env.import_json(&flow.to_string(), &env.who()).map_err(ApiError::InvalidInput)?;
        Ok(json!({"id": f.id, "name": f.name}))
    }

    fn flow_validate(&self, p: &Principal, id: Option<&str>, flow: Option<&Value>) -> ApiResult<Vec<Value>> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let f = match (id, flow) {
            (Some(id), _) => env.store.get_draft(id).ok_or_else(|| ApiError::NotFound("no such flow".into()))?,
            (None, Some(v)) => iyakki_flow::Flow::from_json(&v.to_string()).map_err(ApiError::InvalidInput)?,
            _ => return Err(ApiError::InvalidInput("give a flow id or a flow".into())),
        };
        Ok(iyakki_flow::validate::validate_flow(&f, &env.reg).into_iter().map(|i| json!({"node": i.node, "level": if i.level == iyakki_flow::kinds::Level::Error { "error" } else { "warning" }, "message": crate::bugs::redact(&i.msg)})).collect())
    }

    fn flow_run_decide(&self, p: &Principal, id: &str, run: &str, node: &str, approved: bool) -> ApiResult<()> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let r = env.engine.get_run(run).filter(|r| r.flow_id == id).ok_or_else(|| ApiError::NotFound("no such run".into()))?;
        if !r.nodes.get(node).is_some_and(|s| s.state == iyakki_flow::NodeState::Waiting) {
            return Err(ApiError::NotFound("no such waiting step".into()));
        }
        env.engine.decide(run, node, approved, &env.who(), "answered through the web service").map_err(ApiError::InvalidInput)
    }

    fn flow_run_stop(&self, p: &Principal, id: &str, run: &str) -> ApiResult<()> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        env.engine.get_run(run).filter(|r| r.flow_id == id).ok_or_else(|| ApiError::NotFound("no such run".into()))?;
        env.engine.cancel(run).map_err(ApiError::InvalidInput)
    }

    fn flow_runs(&self, p: &Principal, id: &str) -> ApiResult<Vec<Value>> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        if !env.summaries().iter().any(|f| f.id == id) {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        Ok(env.engine.runs_of(id).into_iter().map(|r| run_summary_json(&r)).collect())
    }

    fn flow_run_get(&self, p: &Principal, id: &str, run: &str) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let r = env.engine.get_run(run).filter(|r| r.flow_id == id).ok_or_else(|| ApiError::NotFound("no such run".into()))?;
        let nodes: Vec<Value> = r.nodes.iter().map(|(n, s)| json!({"node": n, "state": serde_json::to_value(s.state).unwrap_or(Value::Null), "error": crate::bugs::redact(&s.error)})).collect();
        Ok(json!({"id": r.id, "flow": r.flow_id, "status": r.status.label(), "trigger": r.trigger.kind, "created": r.created_ms, "started": r.started_ms, "ended": r.ended_ms,
            "error": crate::bugs::redact(&r.error), "blocked": crate::bugs::redact(&r.blocked), "nodes": nodes}))
    }

    fn schedules(&self, p: &Principal) -> ApiResult<Vec<Value>> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let mut out = Vec::new();
        for s in env.summaries() {
            let Some(f) = env.store.get_draft(&s.id) else { continue };
            let live = env.store.published(&s.id);
            for t in f.triggers.iter().filter(|t| t.kind.is_timed()) {
                let mut one = f.clone();
                one.triggers = vec![t.clone()];
                let next: Vec<i64> = env.engine.next_runs(&one, 5).into_iter().map(|x| x.1).collect();
                out.push(json!({"flow": s.id, "name": s.name, "published": s.published, "active": live.as_ref().is_some_and(|l| l.triggers.iter().any(|x| x.id == t.id)),
                    "trigger": serde_json::to_value(t).unwrap_or(Value::Null), "next": next, "paused": env.engine.schedule_paused(&s.id)}));
            }
        }
        Ok(out)
    }

    fn schedule_save(&self, p: &Principal, flow: &str, trigger: &Value) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let mut f = env.store.get_draft(flow).ok_or_else(|| ApiError::NotFound("no such flow".into()))?;
        let clean = !env.store.draft_changed(flow) && env.store.published(flow).is_some();
        let mut t = crate::flows::trigger_from_value(trigger).map_err(ApiError::InvalidInput)?;
        if t.id.is_empty() {
            t.id = format!("{}-{}", trigger.get("kind").and_then(Value::as_str).unwrap_or("t"), f.triggers.len() + 1);
        }
        match f.triggers.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t.clone(),
            None => f.triggers.push(t.clone()),
        }
        env.save_edit(&f, &env.who(), &format!("schedule {}", t.id), "").map_err(ApiError::InvalidInput)?;
        let active = clean && env.store.publish(flow, &iyakki_flow::store::PublishReq { by: &env.who(), label: "", changelog: &format!("schedule {}", t.id), approved_by: None }).is_ok();
        let mut one = f.clone();
        one.triggers = vec![t.clone()];
        let next: Vec<i64> = env.engine.next_runs(&one, 5).into_iter().map(|x| x.1).collect();
        Ok(json!({"active": active, "trigger": serde_json::to_value(&t).unwrap_or(Value::Null), "next": next,
            "note": if active { "" } else { "saved in the draft; the flow has other unpublished changes, publish it in FLOWS to start this schedule" }}))
    }

    fn schedule_delete(&self, p: &Principal, flow: &str, trigger: &str) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        let mut f = env.store.get_draft(flow).ok_or_else(|| ApiError::NotFound("no such flow".into()))?;
        let clean = !env.store.draft_changed(flow) && env.store.published(flow).is_some();
        let n = f.triggers.len();
        f.triggers.retain(|t| t.id != trigger);
        if f.triggers.len() == n {
            return Err(ApiError::NotFound("no such trigger".into()));
        }
        env.save_edit(&f, &env.who(), &format!("remove schedule {trigger}"), "").map_err(ApiError::InvalidInput)?;
        let active = clean && env.store.publish(flow, &iyakki_flow::store::PublishReq { by: &env.who(), label: "", changelog: &format!("remove schedule {trigger}"), approved_by: None }).is_ok();
        Ok(json!({"removed": trigger, "timed_left": f.triggers.iter().filter(|t| t.kind.is_timed()).count(), "active": active}))
    }

    fn schedule_pause(&self, p: &Principal, flow: &str, paused: bool) -> ApiResult<Value> {
        let env = crate::flows::FlowEnv::open(&self.svc, &p.user, &p.policy).map_err(ApiError::InvalidInput)?;
        if !env.summaries().iter().any(|f| f.id == flow) {
            return Err(ApiError::NotFound("no such flow".into()));
        }
        env.engine.pause_schedule(flow, paused);
        Ok(json!({"flow": flow, "paused": env.engine.schedule_paused(flow)}))
    }

    fn data_call(&self, p: &Principal, tool: &str, args: &Value) -> ApiResult<String> {
        let cwd = crate::scope::user_home(&p.user);
        let cwd = if cwd.is_dir() { cwd } else { std::path::PathBuf::from("/") };
        crate::datatools::call(tool, args, &p.policy, &p.user, &cwd, self.svc.dir()).map_err(ApiError::InvalidInput)
    }

    fn web_access(&self, _p: &Principal) -> ApiResult<Value> {
        use crate::webaccess as wa;
        let (on, port) = (self.svc.web_access_on(), self.svc.web_access_port());
        let st = wa::read_status(self.svc.dir());
        let ips: Vec<String> = crate::webaccess::local_ips();
        let shown = if st.port != 0 { st.port } else { port };
        let urls = if on || st.listening { wa::urls(&crate::devname::label(self.svc.dir()), &ips, shown) } else { vec![] };
        Ok(json!({"on": on, "port": port, "listening": st.listening, "boot": st.boot, "error": st.error, "status": st.line(on),
            "urls": urls, "fingerprint": crate::devmode::cert_fingerprint(self.svc.dir()), "exposes": wa::EXPOSES,
            // the caller is an administrator who signed in with a password, so the switch's precondition holds
            "has_admin_password": true}))
    }

    fn set_web_access(&self, _p: &Principal, on: Option<bool>, port: Option<u16>) -> ApiResult<Vec<(String, String)>> {
        use crate::webaccess as wa;
        let mut done = Vec::new();
        if let Some(p) = port {
            let p = wa::check_port(p).map_err(ApiError::InvalidInput)?;
            self.svc.set_web_access_port(p).map_err(ApiError::InvalidInput)?;
            done.push(("settings.web_access.port".to_string(), wa::effective_port(p).to_string()));
        }
        if let Some(on) = on {
            self.svc.set_web_access(on).map_err(ApiError::InvalidInput)?;
            done.push(("settings.web_access".to_string(), format!("{} (port {}, HTTPS)", if on { "on" } else { "off" }, self.svc.web_access_port())));
        }
        Ok(done)
    }
}

fn run_summary_json(r: &iyakki_flow::model::RunSummary) -> Value {
    json!({"id": r.id, "flow": r.flow_id, "version": r.version, "status": r.status.label(), "trigger": r.trigger, "created": r.created_ms, "started": r.started_ms, "ended": r.ended_ms,
        "done": r.nodes_done, "total": r.nodes_total, "error": crate::bugs::redact(&r.error), "blocked": crate::bugs::redact(&r.blocked)})
}

/// Everything the service needs, over the machine's services.
pub fn assemble(auth: Arc<dyn Authenticator>, audit: Arc<dyn AuditSink>, svc: Arc<Services>, host: Arc<dyn HostApi>) -> Arc<Deps> {
    Arc::new(Deps {
        auth,
        audit: audit.clone(),
        store: svc.runs.clone(),
        chat: svc.chat_backend(),
        tools: Arc::new(ServicesTools { svc: svc.clone(), host, audit: audit.clone() }),
        extras: Arc::new(ServicesExtras { core: Arc::new(crate::corehandle::AgentCore::new(svc.clone())), svc, tasks: Default::default() }),
        system_prompt: AGENT_SYSTEM_PROMPT.to_string(),
    })
}

#[cfg(test)]
mod real_extras_tests {
    //! The cloud profile's flow-run and schedule routes call these methods; the route tests use a fake `Extras`. Here the REAL
    //! implementation runs over a real `Services` and a real flow engine in a temporary folder (no network, no cluster).
    use super::*;
    use iyakki_sec::policy::Role;

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn rig(tag: &str) -> (ServicesExtras, Principal, std::path::PathBuf) {
        let dir = std::fs::canonicalize({
            let d = std::env::temp_dir().join(format!("iyakki-realextras-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        })
        .unwrap();
        let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        let p = Principal { user: "ann".into(), role: "admin".into(), admin: true, policy: Role::Admin.template() };
        (ServicesExtras { core: Arc::new(crate::corehandle::AgentCore::new(svc.clone())), svc, tasks: Default::default() }, p, dir)
    }

    fn make_flow(x: &ServicesExtras, p: &Principal, id: &str) {
        let env = crate::flows::FlowEnv::open(&x.svc, &p.user, &p.policy).unwrap();
        let mut f = iyakki_flow::model::Flow::new(id, "Daily");
        f.nodes.push(iyakki_flow::model::Node::new("start", "start", json!({})));
        f.nodes.push(iyakki_flow::model::Node::new("end", "end", json!({})));
        f.edges.push(iyakki_flow::model::Edge::new("start", "end"));
        env.create_draft(f, &env.who()).unwrap();
    }

    #[test]
    fn the_real_flow_run_routes_start_list_and_read_a_run_of_the_callers_own_flow_only() {
        let (x, p, dir) = rig("runs");
        make_flow(&x, &p, "f1");
        assert!(x.flows(&p).unwrap().iter().any(|f| f["id"] == "f1"));
        // a flow that is not there, in all three calls
        assert!(matches!(x.flow_run(&p, "nope", false, &Value::Null), Err(ApiError::NotFound(_))));
        assert!(matches!(x.flow_runs(&p, "nope"), Err(ApiError::NotFound(_))));
        assert!(matches!(x.flow_run_get(&p, "f1", "r-nope"), Err(ApiError::NotFound(_))));
        // a run of the draft (the flow is not published)
        let run = match x.flow_run(&p, "f1", false, &json!({"who": "ann"})) {
            Ok(r) => r,
            Err(e) => panic!("the real engine refused to start a run: {e}"),
        };
        assert!(!run.is_empty());
        let t0 = std::time::Instant::now();
        let listed = loop {
            let l = x.flow_runs(&p, "f1").unwrap();
            if !l.is_empty() || t0.elapsed() > std::time::Duration::from_secs(5) {
                break l;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!((listed[0]["id"].as_str(), listed[0]["flow"].as_str()), (Some(run.as_str()), Some("f1")));
        let one = x.flow_run_get(&p, "f1", &run).unwrap();
        assert_eq!((one["id"].as_str(), one["flow"].as_str(), one["trigger"].as_str()), (Some(run.as_str()), Some("f1"), Some("manual")));
        assert!(one["status"].is_string() && one["nodes"].is_array());
        // another person has their own environment: they neither see the flow nor the run
        let other = Principal { user: "bob".into(), role: "admin".into(), admin: true, policy: Role::Admin.template() };
        assert!(matches!(x.flow_run(&other, "f1", false, &Value::Null), Err(ApiError::NotFound(_))));
        assert!(matches!(x.flow_run_get(&other, "f1", &run), Err(ApiError::NotFound(_))));
        // the run belongs to f1: asking for it under another flow id is a not-found
        make_flow(&x, &p, "f2");
        assert!(matches!(x.flow_run_get(&p, "f2", &run), Err(ApiError::NotFound(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_schedule_methods_save_list_pause_and_delete_a_timed_trigger_on_the_flows_draft() {
        let (x, p, dir) = rig("sched");
        make_flow(&x, &p, "f1");
        assert!(matches!(x.schedule_save(&p, "nope", &json!({"kind": "cron", "expr": "0 9 * * *"})), Err(ApiError::NotFound(_))));
        // a flow that was never published: the trigger is saved in the draft and the note says to publish
        let saved = x.schedule_save(&p, "f1", &json!({"id": "t1", "kind": "cron", "expr": "0 9 * * *"})).unwrap();
        assert_eq!(saved["active"], json!(false), "{saved}");
        assert!(saved["note"].as_str().unwrap().contains("publish"), "{saved}");
        assert_eq!(saved["next"].as_array().unwrap().len(), 5, "five coming times are shown");
        let list = x.schedules(&p).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0]["flow"].as_str(), list[0]["trigger"]["id"].as_str(), list[0]["active"].clone(), list[0]["paused"].clone()), (Some("f1"), Some("t1"), json!(false), json!(false)));
        // (a cron expression is checked by the route before this method is called, see schedules.rs; here a trigger of an unknown kind is refused)
        assert!(matches!(x.schedule_save(&p, "f1", &json!({"id": "t3", "kind": "sometimes"})), Err(ApiError::InvalidInput(_))));
        // a published flow with no other unpublished change: the schedule goes live at once, and deleting it publishes the removal
        make_flow(&x, &p, "f3");
        {
            let env = crate::flows::FlowEnv::open(&x.svc, &p.user, &p.policy).unwrap();
            env.store.publish("f3", &iyakki_flow::store::PublishReq { by: &env.who(), label: "", changelog: "first", approved_by: None }).unwrap();
        }
        let live = x.schedule_save(&p, "f3", &json!({"id": "t1", "kind": "interval", "every_s": 3600})).unwrap();
        assert_eq!(live["active"], json!(true), "{live}");
        assert!(x.schedules(&p).unwrap().iter().any(|s| s["flow"] == "f3" && s["active"] == json!(true)));
        let off = x.schedule_delete(&p, "f3", "t1").unwrap();
        assert_eq!((off["timed_left"].clone(), off["active"].clone()), (json!(0), json!(true)), "{off}");
        // pause and resume
        assert_eq!(x.schedule_pause(&p, "f1", true).unwrap()["paused"], json!(true));
        assert_eq!(x.schedules(&p).unwrap()[0]["paused"], json!(true));
        assert_eq!(x.schedule_pause(&p, "f1", false).unwrap()["paused"], json!(false));
        assert!(matches!(x.schedule_pause(&p, "nope", true), Err(ApiError::NotFound(_))));
        // delete: an unknown trigger is a not-found, the real one goes
        assert!(matches!(x.schedule_delete(&p, "f1", "t-missing"), Err(ApiError::NotFound(_))));
        let gone = x.schedule_delete(&p, "f1", "t1").unwrap();
        assert_eq!(gone["removed"], json!("t1"));
        assert!(x.schedules(&p).unwrap().iter().all(|s| s["trigger"]["id"] != "t1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
