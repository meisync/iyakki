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
}

impl ToolSource for ServicesTools {
    fn tools_for(&self, p: &Principal) -> Arc<dyn Tools> {
        // MCP servers are not passed: their tools are `mcp__*`, high risk, and not in any profile's list in this slice
        Arc::new(UserTools::new(&p.user, p.policy.clone(), self.svc.clone(), self.host.clone(), Vec::<ServerSpec>::new()))
    }
}

pub struct ServicesExtras {
    svc: Arc<Services>,
    /// Background threads started by `background_start` (the cloud scheduler); they end when the stop flag is set.
    tasks: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>>,
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io(e.to_string())
}

impl Extras for ServicesExtras {
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
        audit,
        store: svc.runs.clone(),
        chat: svc.chat_backend(),
        tools: Arc::new(ServicesTools { svc: svc.clone(), host }),
        extras: Arc::new(ServicesExtras { svc, tasks: Default::default() }),
        system_prompt: AGENT_SYSTEM_PROMPT.to_string(),
    })
}
