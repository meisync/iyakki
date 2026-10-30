//! One function per route id. A handler sees the request, the signed-in person and the service's shared state, and answers with
//! JSON. Handlers use only the traits of `deps`, the run store and the run manager: no shell, no kernel, no device.

use super::openapi;
use super::server::{json_body, Fail, Shared};
use super::tools::allowed_set;
use super::API_VERSION;
use crate::agent::Verdict;
use crate::httpmini::Request;
use iyakki_core_api::{ApiError, Principal};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Ctx<'a> {
    pub sh: &'a Shared,
    pub req: &'a Request,
    pub params: &'a [String],
    pub principal: Option<&'a Principal>,
    pub token: Option<&'a str>,
    pub ip: &'a str,
}

/// (status, body, audit detail without secrets)
pub type Out = Result<(u16, Value, String), Fail>;

fn ok(v: Value) -> Out {
    Ok((200, v, String::new()))
}

impl Ctx<'_> {
    fn who(&self) -> &Principal {
        self.principal.expect("a signed-in route")
    }
    fn query_num(&self, key: &str, max: u64) -> u64 {
        self.req.query.get(key).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(max)
    }
}

pub fn dispatch(id: &str, c: &Ctx) -> Out {
    match id {
        "healthz" => ok(json!({"status": "ok", "profile": c.sh.profile.name()})),
        "readyz" => match c.sh.deps.extras.ready() {
            Ok(()) => ok(json!({"status": "ready", "profile": c.sh.profile.name()})),
            Err(e) => {
                log::warn!("service: not ready: {e}");
                Ok((503, json!({"status": "not ready", "profile": c.sh.profile.name()}), String::new()))
            }
        },
        "version" => ok(json!({"product": "iyakki", "version": env!("CARGO_PKG_VERSION"), "api": API_VERSION, "profile": c.sh.profile.name()})),
        "login" => login(c),
        "logout" => {
            if let Some(t) = c.token {
                c.sh.tokens.revoke(t);
            }
            ok(json!({"ok": true}))
        }
        "openapi" => ok(openapi::document(c.sh.profile)),
        "whoami" => {
            let p = c.who();
            let tools: Vec<&String> = c.sh.allowed_tools.iter().collect();
            ok(json!({"user": p.user, "role": p.role, "admin": p.admin, "profile": c.sh.profile.name(), "tools": tools}))
        }
        "models" => ok(json!({"models": c.sh.deps.extras.models(c.who())})),
        "tools" => ok(json!({"tools": c.sh.allowed_tools.iter().collect::<Vec<_>>()})),
        "runs_list" => ok(json!({"runs": c.sh.runs.list(c.who())})),
        "runs_start" => runs_start(c),
        "runs_get" => {
            let (status, events, next, _) = c.sh.runs.view(c.who(), &c.params[0], 0)?;
            ok(json!({"id": c.params[0], "status": status, "events": events, "next": next}))
        }
        "runs_events" => {
            let after = c.query_num("after", u32::MAX as u64) as usize;
            let wait = c.query_num("wait", 25);
            let (status, events, next, done) = c.sh.runs.events(c.who(), &c.params[0], after, Duration::from_secs(wait))?;
            ok(json!({"id": c.params[0], "status": status, "events": events, "next": next, "done": done}))
        }
        "runs_stop" => {
            c.sh.runs.stop(c.who(), &c.params[0])?;
            Ok((200, json!({"ok": true}), format!("run={}", c.params[0])))
        }
        "approvals_list" => {
            let v: Vec<Value> = c.sh.deps.store.pending_approvals(Some(&c.who().user)).into_iter().map(approval_json).collect();
            ok(json!({"approvals": v}))
        }
        "approvals_decide" => approvals_decide(c),
        "admin_status" => ok(json!({"tokens": c.sh.tokens.count(), "runs_going": c.sh.runs.going(), "approvals_pending": c.sh.deps.store.pending_approvals(None).len()})),
        "flows_list" => ok(json!({"flows": c.sh.deps.extras.flows(c.who())?})),
        "flows_get" => ok(c.sh.deps.extras.flow_export(c.who(), &c.params[0])?),
        "data_call" => data_call(c),
        _ => Err(Fail::new(501, "unsupported", "not implemented")),
    }
}

fn approval_json(a: crate::runs::PendingApproval) -> Value {
    json!({"run": a.run_id, "call": a.call_id, "tool": a.tool, "summary": a.summary, "detail": a.detail, "risk": a.risk, "since": a.since})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginReq {
    name: String,
    password: String,
}

fn login(c: &Ctx) -> Out {
    let req: LoginReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"name\":..., \"password\":...}"))?;
    let shown = if iyakki_sec::policy::valid_username(&req.name) { req.name.clone() } else { "?".to_string() };
    match c.sh.deps.auth.login(&req.name, &req.password) {
        Ok(p) => {
            let t = c.sh.tokens.issue(&p.user).map_err(|e| Fail::new(503, "unavailable", &e))?;
            c.sh.deps.audit.record(&p.user, "api.login", &format!("status=200 addr={}", c.ip));
            Ok((200, json!({"token": t, "expires_in": c.sh.cfg.limits.token_lifetime_secs, "user": p.user, "role": p.role}), String::new()))
        }
        Err(e) => {
            c.sh.deps.audit.record("-", "api.login.denied", &format!("name={shown} addr={}", c.ip));
            match e {
                ApiError::Busy(m) => Err(Fail::new(429, "rate_limited", &m)),
                _ => Err(Fail::new(401, "unauthorized", "invalid username or password")),
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunReq {
    prompt: String,
    provider: Option<String>,
    model: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
}

fn runs_start(c: &Ctx) -> Out {
    let r: RunReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"prompt\":..., optional \"provider\", \"model\", \"tools\"}"))?;
    let allowed = allowed_set(c.sh.profile, &c.sh.cfg.enable_tools, &r.tools).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    let model = match (&r.provider, &r.model) {
        (Some(p), Some(m)) => Some((p.as_str(), m.as_str())),
        (None, None) => None,
        _ => return Err(Fail::new(400, "invalid_input", "give provider and model together, or neither")),
    };
    let id = c.sh.runs.start(c.who(), &r.prompt, model, allowed, c.sh.cfg.limits.concurrent_runs)?;
    Ok((200, json!({"id": id}), format!("run={id} chars={}", r.prompt.len())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerdictReq {
    verdict: String,
}

fn approvals_decide(c: &Ctx) -> Out {
    let me = c.who();
    let v: VerdictReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"verdict\":\"approve\"} or {\"verdict\":\"deny\"}"))?;
    // "always" and "session" grants do not exist over the API: every call is answered on its own
    let verdict = match v.verdict.as_str() {
        "approve" => Verdict::Approve,
        "deny" => Verdict::Deny,
        _ => return Err(Fail::new(400, "invalid_input", "the verdict is \"approve\" or \"deny\"")),
    };
    let (run, call) = (&c.params[0], &c.params[1]);
    let store = &c.sh.deps.store;
    let pending = store.pending_approvals(None).into_iter().find(|a| a.run_id == *run && a.call_id == *call);
    let Some(a) = pending else {
        // a run that is still going but has no such waiting call: already answered (a replay) or never asked
        // a call this run already answered (a replay, or an answer that came after the timeout) is 409; one never asked is 404
        return if c.sh.runs.decided(me, run, call) {
            Err(Fail::new(409, "busy", "that request was already answered (or timed out)"))
        } else {
            Err(Fail::new(404, "not_found", "no such waiting request"))
        };
    };
    if a.user != me.user && !me.admin {
        return Err(Fail::new(403, "denied", "that approval belongs to someone else"));
    }
    store.decide_pending(&me.user, me.admin, run, call, verdict).map_err(|e| if e.contains("already") { Fail::new(409, "busy", "already answered") } else { Fail::new(404, "not_found", "the run has ended") })?;
    let word = if verdict == Verdict::Approve { "approve" } else { "deny" };
    Ok((200, json!({"run": run, "call": call, "tool": a.tool, "verdict": word}), format!("run={run} call={call} tool={} verdict={word}", a.tool)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DataReq {
    tool: String,
    args: Value,
}

fn data_call(c: &Ctx) -> Out {
    let r: DataReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"tool\":..., \"args\":{...}}"))?;
    // the data tools of the profile, by name; the same list the model gets
    if !["data_open", "data_describe", "data_query"].contains(&r.tool.as_str()) || !c.sh.allowed_tools.contains(&r.tool) {
        return Err(Fail::new(404, "not_found", "no such data tool in this profile"));
    }
    if !r.args.is_object() {
        return Err(Fail::new(400, "invalid_input", "args must be an object"));
    }
    let out = c.sh.deps.extras.data_call(c.who(), &r.tool, &r.args)?;
    Ok((200, json!({"result": out}), format!("tool={}", r.tool)))
}
