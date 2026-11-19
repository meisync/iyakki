//! One function per route id. A handler sees the request, the signed-in person and the service's shared state, and answers with
//! JSON. Handlers use only the traits of `deps`, the run store and the run manager: no shell, no kernel, no device.

use super::openapi;
use super::server::{json_body, Fail, Shared};
use super::tools::allowed_set;
use super::API_VERSION;
use crate::agent::Verdict;
use crate::cloudaccess::{Access, Denial, Revoke};
use crate::cloudrotate::Report;
use crate::cloudvaultlink::{self, Link};
use crate::cloudroles::{Action, NsRole, TenantRole};
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
    pub(super) fn who(&self) -> &Principal {
        self.principal.expect("a signed-in route")
    }
    /// The volume this request works on: the service's own, or that of the tenant the person's session was switched to.
    pub(super) fn data_dir(&self) -> std::path::PathBuf {
        super::tenants::data_dir_for(self.sh, self.token, self.principal.map(|p| p.user.as_str()).unwrap_or(""))
    }
    /// A function that puts one event on this person's stream (their tenant's bus, only they receive it).
    pub(super) fn notifier(&self) -> super::runs::Notify {
        let bus = crate::notifyhub::events::bus_for(&self.data_dir());
        let user = self.who().user.clone();
        std::sync::Arc::new(move |ev: String| {
            bus.publish(&user, &ev);
        })
    }

    fn query_num(&self, key: &str, max: u64) -> u64 {
        self.req.query.get(key).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(max)
    }
}

pub fn dispatch(id: &str, c: &Ctx) -> Out {
    match id {
        "healthz" => ok(json!({"status": "ok", "profile": c.sh.profile.name()})),
        "readyz" => match c.sh.deps.extras.ready() {
            Ok(()) => ok(with_vault(c, json!({"status": "ready", "profile": c.sh.profile.name()}))),
            Err(e) => {
                log::warn!("service: not ready: {e}");
                Ok((503, with_vault(c, json!({"status": "not ready", "profile": c.sh.profile.name()})), String::new()))
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
            // reachable by any signed-in person; what they get depends on their rights: tool-call approvals only with the Chat right,
            // tenant approvals only as an eligible approver. A person with neither gets an empty list (200), by design.
            let p = c.who();
            let chat = p.policy.agent && p.policy.apps.contains(&iyakki_sec::policy::App::Chat);
            let mut v: Vec<Value> = if chat { c.sh.deps.store.pending_approvals(Some(&p.user)).into_iter().map(approval_json).collect() } else { vec![] };
            if chat {
                // a tile in the browser that wants a tool which asks first (the run is "tile", the call is the question's id)
                for a in c.sh.tiles.asks.waiting(&p.user) {
                    v.push(json!({"run": "tile", "call": a.id, "tool": a.tool, "summary": if a.summary.is_empty() { format!("tile \"{}\" wants to use {}", a.tile, a.tool) } else { format!("tile \"{}\": {}", a.tile, a.summary) }, "detail": a.detail, "risk": crate::runs::approval_risk(&a.tool), "since": crate::tenantconfig::now_secs() as i64 - a.age_secs as i64}));
                }
            }
            if chat {
                for i in c.sh.imports.waiting(&p.user) {
                    v.push(json!({"run": "data", "call": i.id, "tool": i.tool, "summary": format!("a data import asks to {}", i.summary), "detail": "", "risk": "ask", "since": crate::tenantconfig::now_secs() as i64 - i.age_secs as i64}));
                }
            }
            ok(super::notifyapi::unify(c, v))
        }
        "approvals_decide" => approvals_decide(c),
        "admin_status" => ok(json!({"tokens": c.sh.tokens.count(), "runs_going": c.sh.runs.going(), "approvals_pending": c.sh.deps.store.pending_approvals(None).len()})),
        "flows_list" => {
            tenant_gate(c)?;
            ok(json!({"flows": c.sh.deps.extras.flows(c.who())?}))
        }
        "flows_get" => {
            tenant_gate(c)?;
            ok(c.sh.deps.extras.flow_export(c.who(), &c.params[0])?)
        }
        "data_call" => data_call(c),
        "data_import_get" => match c.sh.imports.poll(&c.who().user, &c.params[0]) {
            Some(v) => ok(v),
            None => Err(Fail::new(404, "not_found", "no such import (it was read already, refused, or expired)")),
        },
        "files_list" => super::files::list(c),
        "files_read" => super::files::read(c),
        "files_write" => super::files::write(c),
        "files_upload" => super::files::upload(c),
        "files_download" => super::files::download(c),
        "files_delete" => super::files::delete(c),
        "files_move" => super::files::move_(c),
        "files_mkdir" => super::files::mkdir(c),
        "git_status" => super::gitread::status(c),
        "git_branches" => super::gitread::branches(c),
        "git_log" => super::gitread::log(c),
        "git_diff" => super::gitread::diff(c),
        id if id.starts_with("sched_") => super::schedules::handle(c, id),
        "flows_run" => super::flowruns::start(c),
        "flows_runs" => super::flowruns::list(c),
        "flows_run_get" => super::flowruns::view(c),
        "flows_import" => super::flowruns::import(c),
        "flows_validate" => super::flowruns::validate(c),
        "flows_run_stop" => super::flowruns::stop(c),
        "flows_run_decide" => super::flowruns::decide(c),
        "git_clone" => super::files::clone(c),
        "git_clones" => super::files::clones(c),
        "webaccess_get" => {
            no_desktop_on_a_pod(c)?;
            ok(c.sh.deps.extras.web_access(c.who())?)
        }
        "webaccess_set" => webaccess_set(c),
        "tenants_list" => super::tenants::list(c),
        "tenant_switch" => super::tenants::switch(c),
        "tenant_view" => tenant_view(c),
        "tenant_audit" => tenant_audit(c),
        "tenant_bootstrap" => tenant_bootstrap(c),
        "tenant_member" => tenant_member(c),
        "ns_manage" => ns_manage(c),
        "ns_members_list" => ns_members_list(c),
        "ns_member" => ns_member(c),
        id if id.starts_with("tile_") => super::tiles::handle(c, id),
        id if id.starts_with("notif_") => super::notifyapi::handle(c, id),
        "events" => Err(Fail::new(426, "invalid_input", "this is a WebSocket: connect with an Upgrade request")),
        id if id.starts_with("tcfg_") || id.starts_with("tcfgme_") || id.starts_with("tapprovals_") || id.starts_with("tteam_") => super::tenantcfg::handle(c, id),
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

/// A program signs in with the developer key of a paired computer instead of a password: the request is signed (`devmode::auth`,
/// bound to this service's certificate fingerprint, a fresh nonce, the clock) and the token it gets can call only the route
/// families granted to that computer as `api:*` scopes.
fn paired_login(c: &Ctx) -> Out {
    let headers = |n: &str| c.req.header(n).map(str::to_string);
    let query: Vec<(String, String)> = c.req.query.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let now = crate::clock::now_unix();
    let denied = |why: &str| {
        c.sh.deps.audit.record("-", "api.login.denied", &format!("paired program: {why} addr={}", c.ip));
        Fail::new(401, "unauthorized", "this request is not signed by a paired developer key")
    };
    if c.sh.fingerprint.is_empty() {
        return Err(Fail::new(501, "unsupported", "this service has no certificate of its own, so programs cannot sign in with a key here"));
    }
    let key = crate::devmode::auth::check(&headers, &c.req.method, &c.req.path, &query, &c.req.body, &c.sh.fingerprint, now, &c.sh.nonces, &|k| c.sh.deps.extras.paired_program(k).is_some()).map_err(|e| denied(&e.to_string()))?;
    let (computer, account) = c.sh.deps.extras.paired_program(&key).ok_or_else(|| denied("unknown key"))?;
    let scopes = c.sh.deps.extras.program_scopes(&computer);
    if scopes.is_empty() {
        c.sh.deps.audit.record("-", "api.login.denied", &format!("paired program {computer}: no api scope granted addr={}", c.ip));
        return Err(Fail::new(403, "denied", "no api scope has been granted to this computer; an administrator grants them on the device (Settings, Developer mode)"));
    }
    let Some(p) = c.sh.deps.auth.lookup(&account) else { return Err(denied("the account is gone")) };
    let t = c.sh.tokens.issue_program(&p.user, &computer).map_err(|e| Fail::new(503, "unavailable", &e))?;
    c.sh.deps.audit.record(&p.user, "api.login.paired", &format!("computer={computer} scopes={} addr={}", scopes.join(","), c.ip));
    Ok((200, json!({"token": t, "expires_in": c.sh.cfg.limits.token_lifetime_secs, "user": p.user, "role": p.role, "computer": computer, "scopes": scopes}), String::new()))
}

fn login(c: &Ctx) -> Out {
    if c.req.header(crate::devmode::auth::H_KEY).is_some() {
        return paired_login(c);
    }
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
    /// The team whose integration to use (the caller must be in it).
    #[serde(default)]
    namespace: Option<String>,
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
    // a team's own model host or provider: the run's backend is the team's (gate, counters and audit on every call)
    let team_backend = match model {
        Some((pr, m)) => super::tenantcfg::team_backend(c, pr, m, r.namespace.as_deref())?,
        None => None,
    };
    let id = match team_backend {
        Some(b) => c.sh.runs.start_with_backend(c.who(), &r.prompt, b, allowed, c.sh.cfg.limits.concurrent_runs, Some(c.notifier()))?,
        None => {
            agent_gate(c, model, r.namespace.as_deref())?;
            c.sh.runs.start(c.who(), &r.prompt, model, allowed, c.sh.cfg.limits.concurrent_runs, Some(c.notifier()))?
        }
    };
    Ok((200, json!({"id": id}), format!("run={id} chars={}", r.prompt.len())))
}

/// A tenant member starting a coding agent (provider `claude` or `copilot`) needs the tenant to have switched that agent on
/// (`tenantconfig::enforce::agent_gate`: off by default, secret and vault state, allowed models, shared-use limits, audit line).
fn agent_gate(c: &Ctx, model: Option<(&str, &str)>, team: Option<&str>) -> Result<(), Fail> {
    let Some((provider, m)) = model else { return Ok(()) };
    let kind = match provider {
        "claude" => "agent.claude",
        "copilot" => "agent.copilot",
        // a team's own provider, API-key integration or local host: model list, role, per-person and team counters
        other => return super::tenantcfg::provider_entry_gate(c, other, m, team),
    };
    if !c.sh.profile.is_cloud() {
        return Ok(());
    }
    let dir = c.data_dir();
    if let Some(t) = team {
        // a team's integration is for the team's people
        let me = &c.who().user;
        let roles = Access::new(&dir).roles().ok().flatten();
        if !roles.is_some_and(|r| r.ns_role(t, me).is_some()) {
            return Err(Fail::new(404, "not_found", "no such team"));
        }
    }
    struct Sink<'a>(&'a dyn iyakki_core_api::AuditSink);
    impl crate::tenantconfig::api::ConfigAudit for Sink<'_> {
        fn record(&self, who: &str, event: &str, detail: &str) {
            self.0.record(who, event, detail);
        }
    }
    let sink = Sink(c.sh.deps.audit.as_ref());
    let env = crate::tenantconfig::api::Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, &c.who().user), audit: &sink };
    let now = crate::tenantconfig::now_secs();
    match crate::tenantconfig::enforce::agent_gate(&env, kind, "default", team, &c.who().user, Some(m), now) {
        crate::tenantconfig::enforce::AgentUse::Allowed { .. } => Ok(()),
        crate::tenantconfig::enforce::AgentUse::Refused(why) => Err(Fail::new(403, "denied", &why)),
    }
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
    if run == "tile" {
        let approve = verdict == Verdict::Approve;
        let (tile, tool) = c.sh.tiles.asks.decide(&me.user, call, approve).map_err(|e| if e.contains("already") { Fail::new(409, "busy", "already answered") } else { Fail::new(404, "not_found", "no such waiting request") })?;
        let word = if approve { "approve" } else { "deny" };
        // the person's other streams learn the answer (no arguments, only names)
        (c.notifier())(crate::notifyhub::events::event("tile", json!({"call": call, "tile": tile, "tool": tool, "verdict": word})));
        return Ok((200, json!({"run": run, "call": call, "tool": tool, "verdict": word}), format!("tile={tile} tool={tool} verdict={word}")));
    }
    if run == "data" {
        let approve = verdict == Verdict::Approve;
        let word = if approve { "approve" } else { "deny" };
        let decider = super::dataimport::fingerprint(c.token.unwrap_or(""));
        let job = c.sh.imports.decide(&me.user, decider, call, approve).map_err(|e| match e {
            "already answered" => Fail::new(409, "busy", "already answered"),
            "no such waiting import" => Fail::new(404, "not_found", "no such waiting request"),
            _ => Fail::new(403, "denied", e),
        })?;
        let Some((tool, args)) = job else {
            (c.notifier())(crate::notifyhub::events::event("data_import", json!({"id": call, "status": "denied"})));
            return Ok((200, json!({"run": run, "call": call, "tool": "data", "verdict": word}), format!("import={call} verdict={word}")));
        };
        let res = c.sh.deps.extras.data_call(me, &tool, &args).map_err(|e| e.to_string());
        let outcome = if res.is_ok() { "done" } else { "failed" };
        c.sh.imports.finish(&me.user, call, res);
        (c.notifier())(crate::notifyhub::events::event("data_import", json!({"id": call, "tool": tool, "status": outcome})));
        return Ok((200, json!({"run": run, "call": call, "tool": tool, "verdict": word, "outcome": outcome}), format!("import={call} tool={tool} verdict={word} outcome={outcome}")));
    }
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
    /// Work inside this namespace of the tenant (needs the right to read it); without it, the caller's own data as before.
    #[serde(default)]
    namespace: Option<String>,
}

fn data_call(c: &Ctx) -> Out {
    let r: DataReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"tool\":..., \"args\":{...}}"))?;
    // a write tool is a pending import on a cloud machine (F-WEBSVC-08): nothing is written until the person approves elsewhere
    if super::dataimport::is_write_tool(&r.tool) {
        if !c.sh.profile.is_cloud() {
            return Err(Fail::new(404, "not_found", "no such data tool in this profile"));
        }
        if r.namespace.is_some() {
            return Err(Fail::new(400, "invalid_input", "imports work in the caller's own data; a namespace is not accepted"));
        }
        if !r.args.is_object() {
            return Err(Fail::new(400, "invalid_input", "args must be an object"));
        }
        tenant_gate(c)?;
        let asker = super::dataimport::fingerprint(c.token.unwrap_or(""));
        let (id, summary) = c.sh.imports.submit(&c.who().user, asker, &r.tool, r.args).map_err(|e| Fail::new(if e.starts_with("too many") { 429 } else { 400 }, if e.starts_with("too many") { "rate_limited" } else { "invalid_input" }, &e))?;
        (c.notifier())(crate::notifyhub::events::event("approval", json!({"run": "data", "call": id, "tool": r.tool})));
        return Ok((202, json!({"id": id, "status": "pending", "summary": summary, "expires_in": super::dataimport::LIFE.as_secs(), "approve": format!("/api/v1/approvals/data/{id}"), "poll": format!("/api/v1/data/imports/{id}")}), format!("tool={} import={id} {summary}", r.tool)));
    }
    // the data tools of the profile, by name; the same list the model gets
    if !["data_open", "data_describe", "data_query"].contains(&r.tool.as_str()) || !c.sh.allowed_tools.contains(&r.tool) {
        return Err(Fail::new(404, "not_found", "no such data tool in this profile"));
    }
    if !r.args.is_object() {
        return Err(Fail::new(400, "invalid_input", "args must be an object"));
    }
    let mut args = r.args.clone();
    let detail = match &r.namespace {
        Some(ns) => {
            // membership first (a member outside the namespace gets the answer for a namespace that is not there), then the path
            // from the layout and the jail, never built by hand
            let acc = access(c);
            let v = acc.check(&c.who().user, &Action::ReadNamespace(ns.clone()));
            c.sh.deps.audit.record(&c.who().user, "access", &audit_clean_line(&v.audit));
            denied(&v)?;
            let base = acc.place(&c.who().user, ns).map_err(|e| Fail::new(400, "invalid_input", &e))?;
            if let Some(rel) = args.get("path").and_then(|p| p.as_str()) {
                let real = crate::cloudstorage::jail(&base, rel).map_err(|e| Fail::new(400, "invalid_input", &e))?;
                args["path"] = json!(real.to_string_lossy());
            }
            format!("tool={} namespace={ns}", r.tool)
        }
        None => {
            tenant_gate(c)?;
            format!("tool={}", r.tool)
        }
    };
    let out = c.sh.deps.extras.data_call(c.who(), &r.tool, &args)?;
    Ok((200, json!({"result": out}), detail))
}

// ---------------------------------------------------------------------------------------------- cloud profile: roles and namespaces

fn access(c: &Ctx) -> Access {
    Access::new(&c.data_dir())
}

/// The cloud profile's health detail names the state of its vault link in one word (`cloudvaultlink::status`: not_configured,
/// invalid_address, unreachable or ok), from `IYAKKI_VAULT_URL`. Never the address, never an error text. It does not change readiness.
fn with_vault(c: &Ctx, mut v: Value) -> Value {
    if c.sh.profile.name() == "cloud" {
        v["vault"] = json!(cloudvaultlink::status(&Link::from_env()));
    }
    v
}

/// A removal and the rotation of the vault scopes the person held (`cloudrotate.rs`): the acting person's own vault secret is used when
/// this volume holds one; otherwise the rotation is recorded as pending for `iyakki cloud reconcile --retry-rotations`.
fn revoke(c: &Ctx, by: &str, what: &Revoke) -> Result<(crate::cloudroles::Change, Report), String> {
    let keys = crate::cloudrotate::actor_keys(&c.data_dir(), by);
    access(c).revoke(by, what, &Link::from_env(), keys.as_ref())
}

fn rotation_answer(rep: Option<Report>) -> Value {
    match rep {
        None => json!({"ok": true}),
        Some(r) => json!({"ok": true, "rotation": {"rotated": r.rotated, "nothing_to_do": r.nothing, "pending": r.pending}}),
    }
}

fn audit_clean_line(s: &str) -> String {
    s.chars().filter(|ch| !ch.is_control()).collect()
}

fn denied(v: &crate::cloudaccess::Verdict) -> Result<(), Fail> {
    match &v.denial {
        None => Ok(()),
        Some(Denial::Forbidden(m)) => Err(Fail::new(403, "denied", m)),
        Some(Denial::NotFound(m)) => Err(Fail::new(404, "not_found", m)),
    }
}

/// Once the tenant has its lists, the routes that are the person's own (flows, data outside a namespace) are for members of the tenant.
/// A tenant that has no lists yet behaves as before for these; everything about namespaces is denied until the first owner exists.
pub(super) fn tenant_gate(c: &Ctx) -> Result<(), Fail> {
    if !c.sh.profile.is_cloud() {
        return Ok(());
    }
    let acc = access(c);
    match acc.roles() {
        Ok(None) => Ok(()),
        _ => denied(&acc.check(&c.who().user, &Action::EnterTenant)),
    }
}

fn tenant_view(c: &Ctx) -> Out {
    let acc = access(c);
    denied(&acc.check(&c.who().user, &Action::EnterTenant))?;
    let r = acc.roles().ok().flatten().ok_or_else(|| Fail::new(403, "denied", "no access"))?;
    let me = &c.who().user;
    let role = r.tenant_role(me);
    let admin = role.is_some_and(|t| t >= TenantRole::Admin);
    let members: Vec<Value> = if admin { r.tenant.iter().map(|(n, t)| json!({"name": n, "role": format!("{t:?}").to_lowercase()})).collect() } else { vec![] };
    let namespaces: Vec<Value> = r.visible_namespaces(me).into_iter().map(|(n, m)| json!({"name": n, "role": m.map(|x| format!("{x:?}").to_lowercase()), "joined": m.is_some()})).collect();
    ok(json!({"user": me, "tenant_role": role.map(|t| format!("{t:?}").to_lowercase()), "members": members, "namespaces": namespaces}))
}

/// `GET /api/v1/tenant/audit?limit=&offset=`: a page of the role-change trail, newest first (`cloudroles::audit_page`; 100 lines by
/// default, at most 200), with the number of lines in all and whether older ones follow; for tenant owners and administrators.
fn tenant_audit(c: &Ctx) -> Out {
    let acc = access(c);
    denied(&acc.check(&c.who().user, &Action::EnterTenant))?;
    let r = acc.roles().ok().flatten().ok_or_else(|| Fail::new(403, "denied", "no access"))?;
    if !r.tenant_role(&c.who().user).is_some_and(|t| t >= TenantRole::Admin) {
        return Err(Fail::new(403, "denied", "only tenant owners and administrators may read the trail"));
    }
    let limit = match c.query_num("limit", 200) {
        0 => 100,
        n => n as usize,
    };
    let offset = c.query_num("offset", u32::MAX as u64) as usize;
    let (page, total) = crate::cloudroles::audit_page(&crate::cloudroles::audit_path(&c.data_dir()), offset, limit);
    let lines: Vec<Value> = page.into_iter().map(|(t, ch)| json!({"time": t, "change": ch})).collect();
    let more = offset + lines.len() < total;
    ok(json!({"lines": lines, "total": total, "offset": offset, "limit": limit, "more": more}))
}

fn tenant_bootstrap(c: &Ctx) -> Out {
    // the service route is for the service's administrators; `Access::bootstrap` itself makes it work once
    let ch = access(c).bootstrap(&c.who().user).map_err(|e| Fail::new(403, "denied", &e))?;
    Ok((200, json!({"ok": true}), audit_clean_line(&ch.0)))
}

fn change_err(e: String) -> Fail {
    if e.contains("no access") || e.contains("not allowed") || e.contains("higher role") || e.contains("only an owner") {
        Fail::new(403, "denied", &e)
    } else {
        Fail::new(400, "invalid_input", &e)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantMemberReq {
    name: String,
    role: String,
}

fn tenant_member(c: &Ctx) -> Out {
    let r: TenantMemberReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"name\":..., \"role\":\"member|admin|owner|remove\"}"))?;
    let by = c.who().user.clone();
    let role = match r.role.as_str() {
        "member" => Some(TenantRole::Member),
        "admin" => Some(TenantRole::Admin),
        "owner" => Some(TenantRole::Owner),
        "remove" => None,
        _ => return Err(Fail::new(400, "invalid_input", "the role is member, admin, owner or remove")),
    };
    let (ch, rotation) = match role {
        Some(t) => (access(c).change(|ro| ro.add_tenant_member(&by, &r.name, t)).map_err(change_err)?, None),
        None => {
            let (ch, rep) = revoke(c, &by, &Revoke::Tenant { who: r.name.clone() }).map_err(change_err)?;
            (ch, Some(rep))
        }
    };
    Ok((200, rotation_answer(rotation), audit_clean_line(&ch.0)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceReq {
    name: String,
    action: String,
}

fn ns_manage(c: &Ctx) -> Out {
    let r: NamespaceReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"name\":..., \"action\":\"create|delete\"}"))?;
    let by = c.who().user.clone();
    let ch = match r.action.as_str() {
        "create" => access(c).change(|ro| ro.create_namespace(&by, &r.name)),
        "delete" => access(c).change(|ro| ro.delete_namespace(&by, &r.name)),
        _ => return Err(Fail::new(400, "invalid_input", "the action is create or delete")),
    };
    let ch = ch.map_err(change_err)?;
    Ok((200, json!({"ok": true}), audit_clean_line(&ch.0)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NsMemberReq {
    name: String,
    role: String,
}

fn ns_member(c: &Ctx) -> Out {
    let r: NsMemberReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"name\":..., \"role\":\"viewer|editor|admin|remove\"}"))?;
    let ns = c.params[0].clone();
    let by = c.who().user.clone();
    let acc = access(c);
    // a member who cannot even see this namespace is told it does not exist, whatever it is called
    denied(&acc.check(&by, &Action::SeeNamespace(ns.clone())))?;
    let role = match r.role.as_str() {
        "viewer" => Some(NsRole::Viewer),
        "editor" => Some(NsRole::Editor),
        "admin" => Some(NsRole::Admin),
        "remove" => None,
        _ => return Err(Fail::new(400, "invalid_input", "the role is viewer, editor, admin or remove")),
    };
    let (ch, rotation) = match role {
        Some(n) => (acc.change(|ro| ro.add_namespace_member(&by, &ns, &r.name, n)).map_err(change_err)?, None),
        None => {
            let (ch, rep) = revoke(c, &by, &Revoke::Namespace { ns: ns.clone(), who: r.name.clone() }).map_err(change_err)?;
            (ch, Some(rep))
        }
    };
    Ok((200, rotation_answer(rotation), audit_clean_line(&ch.0)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebAccessReq {
    on: Option<bool>,
    port: Option<u16>,
}

/// Settings -> Remote access as an API call: the same switch, the same port rule and the same audit lines as the native page.
fn webaccess_set(c: &Ctx) -> Out {
    no_desktop_on_a_pod(c)?;
    let r: WebAccessReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"on\": true|false} and/or {\"port\": 1024..65535 or 0}"))?;
    if r.on.is_none() && r.port.is_none() {
        return Err(Fail::new(400, "invalid_input", "send \"on\" and/or \"port\""));
    }
    let done = c.sh.deps.extras.set_web_access(c.who(), r.on, r.port)?;
    // the native page writes these lines; the route's own `api.webaccess_set` line (who, address, status) comes on top
    for (event, detail) in &done {
        c.sh.deps.audit.record(&c.who().user, event, detail);
    }
    let detail = done.iter().map(|(e, d)| format!("{e}: {d}")).collect::<Vec<_>>().join("; ");
    Ok((200, json!({"ok": true}), audit_clean_line(&detail)))
}

/// What the native page lists under a namespace: its members, only for a namespace the caller is in.
fn ns_members_list(c: &Ctx) -> Out {
    let ns = c.params[0].clone();
    let me = &c.who().user;
    let acc = access(c);
    denied(&acc.check(me, &Action::SeeNamespace(ns.clone())))?;
    let r = acc.roles().ok().flatten().ok_or_else(|| Fail::new(403, "denied", "no access"))?;
    let Some(mine) = r.ns_role(&ns, me) else {
        // seen by name (a tenant administrator), not joined: the same refusal as for a namespace that is not there
        return Err(Fail::new(403, "denied", "no access"));
    };
    let mut people: Vec<_> = r.namespaces.get(&ns).into_iter().flatten().collect();
    people.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let members: Vec<Value> = people.into_iter().map(|(n, role)| json!({"name": n, "role": format!("{role:?}").to_lowercase()})).collect();
    ok(json!({"name": ns, "role": format!("{mine:?}").to_lowercase(), "members": members}))
}

/// A cloud pod has no desktop of its own to serve to browsers, whatever the machine under it has.
fn no_desktop_on_a_pod(c: &Ctx) -> Result<(), Fail> {
    if c.sh.profile.is_cloud() {
        return Err(Fail::new(501, "unsupported", "a cloud pod has no desktop to serve to browsers"));
    }
    Ok(())
}
