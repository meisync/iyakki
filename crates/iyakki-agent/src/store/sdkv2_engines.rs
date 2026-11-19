//! Engine backends for `iyakki.dash` and `iyakki.flows` of the SDK 1.2 bridge (F-SDK-15): the work behind `dash.sources|query|panel.add` and
//! `flows.list|run|status|cancel` once `sdkv2.rs` has checked arguments, the app's grants, the person's answer, limits and the audit line
//! (docs/development/SDK-BRIDGE-V2.md rule 8). The k3s namespace is `sdkv2_k3s.rs` (F-SDK-32). The Markdown viewer is [`PendingViewer`].
//!
//! * Dash goes through the dash engine, so its read-only guard applies: a source's own connector refuses a statement that could change data.
//!   A [`DashPort`] is the small seam; [`RealDash`] is the dash `Env` of the person; tests use a fake port.
//! * Flows run the person's PUBLISHED flows only (a draft is never started by an app), with the trigger named after the app, the idempotency
//!   key passed to the engine, and the run read back by its id. [`RealFlows`] is the person's `FlowEnv`.
//! * No secret value is returned: a dash answer is the frame (columns and rows) and nothing about the source's address or credentials.

use std::sync::Arc;

use serde_json::{json, Value};

use super::sdkv2_backends::{Backend, Who};
use super::tilesdk::{code, SdkError};

fn fail(msg: impl Into<String>) -> SdkError {
    SdkError::new(code::FAILED, msg.into())
}

fn bad(msg: &str) -> SdkError {
    SdkError::new(code::BAD_ARGUMENT, msg.to_string())
}

fn s<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

// ------------------------------------------------------------------------------------------------------------ dash

/// A data source as the app may see it (never the address or the credentials).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub name: String,
    pub kind: String,
    pub language: String,
}

/// What the dash backend needs from the dash engine.
pub trait DashPort: Send + Sync {
    fn sources(&self, user: &str) -> Vec<SourceInfo>;
    /// Run a read-only query. `range` is `null` or `{from, to}`. Returns `(columns, rows)`; rows are cut to `limit` by the caller.
    fn query(&self, user: &str, source: &str, text: &str, range: &Value, limit: usize) -> Result<(Value, Vec<Value>), String>;
    /// Add a panel to the person's dashboard called `dashboard` (made when there is none). Returns the panel id.
    fn add_panel(&self, user: &str, dashboard: &str, title: &str, source: &str, query: &str, visual: &str) -> Result<String, String>;
}

pub struct DashEngine {
    pub port: Arc<dyn DashPort>,
}

impl Backend for DashEngine {
    fn call(&self, who: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        match op {
            "dash.sources" => {
                let names: Vec<&str> = a.get("names").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let items: Vec<Value> = self.port.sources(&who.user).into_iter().filter(|x| names.is_empty() || names.contains(&x.name.as_str())).map(|x| json!({"name": x.name, "kind": x.kind, "language": x.language})).collect();
                Ok(json!(items))
            }
            "dash.query" => {
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(1000).clamp(1, 10_000) as usize;
                let (cols, mut rows) = self.port.query(&who.user, s(a, "source"), s(a, "query"), a.get("range").unwrap_or(&Value::Null), limit + 1).map_err(fail)?;
                let truncated = rows.len() > limit;
                rows.truncate(limit);
                Ok(json!({"columns": cols, "rows": rows, "truncated": truncated}))
            }
            "dash.panel.add" => {
                let p = a.get("panel").unwrap_or(&Value::Null);
                let id = self.port.add_panel(&who.user, s(a, "dashboard"), s(p, "title"), s(p, "source"), s(p, "query"), s(p, "visual")).map_err(fail)?;
                Ok(json!({"id": id}))
            }
            other => Err(bad(&format!("the dashboard backend has no operation {other}"))),
        }
    }
}

/// The real port: the dash engine of the person.
pub struct RealDash {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
}

impl RealDash {
    fn env(&self, user: &str) -> crate::dash::tools::Env {
        crate::dash::tools::Env::from_host(&self.svc, user, &self.policy)
    }
}

impl DashPort for RealDash {
    fn sources(&self, user: &str) -> Vec<SourceInfo> {
        if !crate::dash::tools::offered(&self.policy) {
            return vec![];
        }
        self.env(user).briefs().into_iter().map(|b| SourceInfo { name: b.name, kind: b.kind, language: b.language }).collect()
    }

    fn query(&self, user: &str, source: &str, text: &str, range: &Value, limit: usize) -> Result<(Value, Vec<Value>), String> {
        use crate::dash::connector::{Cancel, Lang};
        use crate::dash::engine::{self, RunCtx};
        use crate::dash::{QueryRequest, TimeRange};
        if !crate::dash::tools::offered(&self.policy) {
            return Err("your account may not use the dashboards".into());
        }
        let env = self.env(user);
        // an app names a source by its name; the engine knows it by id
        let id = env.briefs().into_iter().find(|b| b.name == source || b.id == source).map(|b| b.id).ok_or_else(|| format!("there is no dashboard source called {source}"))?;
        let (_cfg, src) = env.source(&id).map_err(|e| e.to_string())?;
        let now = env.now();
        let range = crate::dash::tools::parse_range(Some(range).filter(|r| !r.is_null()), TimeRange::last(3_600_000, now), now)?;
        let mut ctx = RunCtx::new(range);
        ctx.timeout_s = Some(20);
        let step = ctx.step_ms(None);
        let mut req = QueryRequest::new(&engine::interpolate(text, &ctx, step), range);
        req.step_ms = (src.language() == Lang::PromQl).then_some(step);
        req.limit = limit;
        let f = src.query(&req, &Cancel::new()).map_err(|e| e.to_string())?;
        if f.rows.is_empty() && !f.series.is_empty() {
            let cols = json!([{"name": "time", "type": "time"}, {"name": "series", "type": "str"}, {"name": "value", "type": "float"}]);
            let rows = f.series.iter().flat_map(|se| se.points.iter().map(move |(t, v)| json!([t, se.name, v]))).take(limit).collect();
            return Ok((cols, rows));
        }
        Ok((serde_json::to_value(&f.columns).unwrap_or(json!([])), f.rows.iter().take(limit).map(|r| Value::Array(r.clone())).collect()))
    }

    fn add_panel(&self, user: &str, dashboard: &str, title: &str, source: &str, query: &str, visual: &str) -> Result<String, String> {
        use crate::dash::model::{Dashboard, Grid, Panel, Visual};
        if !crate::dash::tools::offered(&self.policy) {
            return Err("your account may not use the dashboards".into());
        }
        let env = self.env(user);
        let cfg = env.sources().get(source).ok_or_else(|| format!("there is no dashboard source called {source}"))?;
        let visual = Visual::parse(if visual.is_empty() { "line" } else { visual }).ok_or_else(|| format!("unknown visual {visual}"))?;
        let p = Panel::new("", title, &cfg.id, visual, query, Grid::new(0, 0, 12, 8));
        let dashes = env.dashboards();
        let mut d = dashes.find(dashboard).unwrap_or_else(|| Dashboard::new(&dashes.new_id(dashboard), dashboard));
        let id = d.add_panel(p)?;
        dashes.save(&d)?;
        Ok(id)
    }
}

// ----------------------------------------------------------------------------------------------------------- flows

/// A published flow as the app may see it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowInfo {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub inputs: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunInfo {
    pub state: String,
    pub steps: Vec<(String, String)>,
    pub error: String,
}

pub trait FlowsPort: Send + Sync {
    fn published(&self, user: &str) -> Vec<FlowInfo>;
    fn start(&self, user: &str, app: &str, flow_id: &str, version: Option<u32>, inputs: &Value, key: Option<&str>) -> Result<String, String>;
    fn status(&self, user: &str, run: &str) -> Result<RunInfo, String>;
    fn cancel(&self, user: &str, run: &str) -> Result<(), String>;
}

pub struct FlowsEngine {
    pub port: Arc<dyn FlowsPort>,
}

impl Backend for FlowsEngine {
    fn call(&self, who: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        match op {
            "flows.list" => {
                let names: Vec<&str> = a.get("names").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let items: Vec<Value> = self.port.published(&who.user).into_iter().filter(|f| names.is_empty() || names.contains(&f.name.as_str())).map(|f| json!({"id": f.id, "name": f.name, "version": f.version, "inputs": f.inputs})).collect();
                Ok(json!(items))
            }
            "flows.run" => {
                let name = s(a, "flow");
                let flow = self.port.published(&who.user).into_iter().find(|f| f.name == name).ok_or_else(|| bad("not a published flow"))?;
                let version = a.get("version").and_then(Value::as_u64).map(|v| v as u32);
                if version.is_some_and(|v| v != flow.version) {
                    return Err(fail("the flow was published again after the card was shown; ask again"));
                }
                let key = a.get("key").and_then(Value::as_str);
                let run = self.port.start(&who.user, &who.app, &flow.id, version, a.get("inputs").unwrap_or(&Value::Null), key).map_err(fail)?;
                Ok(json!({"run": run}))
            }
            "flows.status" => {
                let r = self.port.status(&who.user, s(a, "run")).map_err(fail)?;
                Ok(json!({"state": r.state, "steps": r.steps.iter().map(|(n, st)| json!({"node": n, "state": st})).collect::<Vec<_>>(), "error": if r.error.is_empty() { Value::Null } else { json!(r.error) }}))
            }
            "flows.cancel" => {
                self.port.cancel(&who.user, s(a, "run")).map_err(fail)?;
                Ok(json!({"state": "cancelled"}))
            }
            other => Err(bad(&format!("the flows backend has no operation {other}"))),
        }
    }
}

/// The real port: the person's flow environment and engine.
pub struct RealFlows {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    /// Opens the environment (the shared one on a device; a private one in tests).
    pub open: Box<dyn Fn(&str) -> Result<Arc<crate::flows::FlowEnv>, String> + Send + Sync>,
}

impl RealFlows {
    pub fn on_device(svc: Arc<crate::services::Services>, policy: iyakki_sec::policy::Policy) -> RealFlows {
        let (s2, p2) = (svc.clone(), policy.clone());
        RealFlows { svc, policy, open: Box::new(move |u| crate::flows::FlowEnv::open(&s2, u, &p2)) }
    }
}

impl FlowsPort for RealFlows {
    fn published(&self, user: &str) -> Vec<FlowInfo> {
        let Ok(env) = (self.open)(user) else { return vec![] };
        env.summaries()
            .into_iter()
            .filter(|f| f.published > 0)
            .map(|f| {
                let inputs = env.store.published(&f.id).map(|fl| fl.inputs.iter().map(|p| json!({"name": p.name, "type": p.ty, "required": p.required})).collect()).unwrap_or_default();
                FlowInfo { id: f.id, name: f.name, version: f.published, inputs }
            })
            .collect()
    }

    fn start(&self, user: &str, app: &str, flow_id: &str, version: Option<u32>, inputs: &Value, key: Option<&str>) -> Result<String, String> {
        let env = (self.open)(user)?;
        let mut req = iyakki_flow::engine::StartRequest::new(flow_id).trigger("manual", &format!("app:{app}"));
        req.version = version;
        req.draft = false;
        if !inputs.is_null() {
            req.inputs = inputs.clone();
        }
        req.idempotency_key = key.map(|k| format!("app:{app}:{k}"));
        env.engine.start_run(req).map(|o| o.run_id)
    }

    fn status(&self, user: &str, run: &str) -> Result<RunInfo, String> {
        let env = (self.open)(user)?;
        let r = env.engine.get_run(run).ok_or("no such run")?;
        Ok(RunInfo { state: r.status.label().to_string(), steps: r.nodes.iter().map(|(n, st)| (n.clone(), serde_json::to_value(st.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default())).collect(), error: crate::bugs::redact(&r.error) })
    }

    fn cancel(&self, user: &str, run: &str) -> Result<(), String> {
        (self.open)(user)?.engine.cancel(run)
    }
}

// -------------------------------------------------------------------------------------------------------- markdown viewer

/// Most documents waiting for a person in the viewer, and the most text kept for them together (the core's app data takes 64 KB a value).
pub const VIEWER_WAITING: usize = 5;
pub const VIEWER_BYTES: usize = 56 * 1024;

/// The backend of `markdown.view` (F-SDK-14): the document is kept for the person in the app data of the MARKDOWN app (the core's, per
/// person), and the viewer tile shows it when the person opens it (the app list shows "1 document waiting"). The bridge has already checked
/// the arguments, the app's grant and its hourly quota. `markdown.openFile` leaves a pick request that the viewer answers with a list of the person's own Markdown files.
pub struct PendingViewer {
    state: iyakki_core_local::uistate::LocalUiState,
}

impl PendingViewer {
    pub fn new(dir: &std::path::Path) -> PendingViewer {
        PendingViewer { state: iyakki_core_local::uistate::LocalUiState::new(dir) }
    }
}

/// The documents of `user` that wait (also read by the app list).
pub fn viewer_waiting(state: &iyakki_core_local::uistate::LocalUiState, user: &str) -> Vec<Value> {
    use iyakki_core_api::AppData;
    state.get(user, "markdown", "pending").and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default()
}

impl Backend for PendingViewer {
    fn call(&self, who: &Who, op: &str, args: &Value) -> Result<Value, SdkError> {
        use iyakki_core_api::AppData;
        if op != "markdown.view" && op != "markdown.openFile" {
            return Err(SdkError::new(code::UNAVAILABLE, "this viewer call is not available on this computer".to_string()));
        }
        let mut list = viewer_waiting(&self.state, &who.user);
        if list.len() >= VIEWER_WAITING {
            return Err(fail("the viewer already has documents waiting for this person: they open MARKDOWN first"));
        }
        // openFile leaves a request: the viewer shows the person their own Markdown files and they choose one (the person was asked on a
        // card by the bridge first; the app never learns the path or the choice, only that the viewer was opened)
        if op == "markdown.openFile" {
            list.push(json!({"pick": true}));
            let text = serde_json::to_string(&list).map_err(|e| fail(e.to_string()))?;
            self.state.set(&who.user, "markdown", "pending", &text).map_err(fail)?;
            return Ok(json!({"id": list.len()}));
        }
        list.push(json!({"title": s(args, "title"), "text": s(args, "text")}));
        let text = serde_json::to_string(&list).map_err(|e| fail(e.to_string()))?;
        if text.len() > VIEWER_BYTES {
            return Err(bad("the document is too long for the viewer yet (about 50 KB)"));
        }
        self.state.set(&who.user, "markdown", "pending", &text).map_err(fail)?;
        Ok(json!({"id": list.len()}))
    }
}

impl super::sdkv2_backends::Backends {
    /// The backends of the running system that are built: dash and flows go through the person's engines; the rest stay as they were.
    pub fn with_engines(mut self, svc: &Arc<crate::services::Services>, policy: &iyakki_sec::policy::Policy) -> Self {
        self.dash = Arc::new(DashEngine { port: Arc::new(RealDash { svc: svc.clone(), policy: policy.clone() }) });
        self.flows = Arc::new(FlowsEngine { port: Arc::new(RealFlows::on_device(svc.clone(), policy.clone())) });
        self.viewer = Arc::new(PendingViewer::new(svc.dir()));
        // an app's own namespace on the managed cluster (F-SDK-32)
        self.k3s = Arc::new(super::sdkv2_k3s::K3sEngine { kube: Arc::new(super::sdkv2_k3s::RealKube { dev: svc.k3sdev.clone(), dir: svc.dir().to_path_buf() }) });
        self
    }
}

#[cfg(test)]
#[path = "sdkv2_engines_tests.rs"]
mod tests;
