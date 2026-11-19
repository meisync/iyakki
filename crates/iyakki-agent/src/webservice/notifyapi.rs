//! The notification routes (`/api/v1/notifications...`) and the unified approvals list. A person only ever sees their own notes: the
//! person is the signed-in account, the volume is the tenant the session works in. Thin: the work is in `notifyhub::api`.

use super::handlers::{Ctx, Out};
use super::server::{json_body, Fail};
use crate::cloudaccess::Access;
use crate::notifyhub::{api, service::hub_for};
use crate::tenantconfig::api::{Caller, ConfigAudit, Env};
use serde_json::{json, Value};

struct Sink<'a>(&'a dyn iyakki_core_api::AuditSink);
impl ConfigAudit for Sink<'_> {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.record(who, event, detail);
    }
}

fn fail(e: api::ApiErr) -> Fail {
    Fail::new(e.status, e.code, &e.message)
}

fn now() -> i64 {
    iyakki_schema::now_secs() as i64
}

/// The tenant requests this person may decide right now (empty outside a cloud tenant).
fn tenant_pending(c: &Ctx) -> Vec<Value> {
    let dir = c.data_dir();
    let Some(roles) = Access::new(&dir).roles().ok().flatten() else { return vec![] };
    let sink = Sink(c.sh.deps.audit.as_ref());
    let who = &c.who().user;
    let env = Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, who), audit: &sink };
    let caller = Caller { user: who, roles: Some(&roles) };
    crate::tenantconfig::approvals::list(&env, &caller, crate::tenantconfig::now_secs()).ok().and_then(|v| v.get("approvals").and_then(|a| a.as_array().cloned())).unwrap_or_default()
}

/// One list of everything waiting for this person, each with a `kind`: `tool_call` (a run of theirs asks to use a tool; only the run's
/// owner answers) or `tenant` (a configuration change needs an approver; only eligible approvers see it). Decisions go through the
/// existing endpoints named in `decide`; nothing here decides.
pub fn unify(c: &Ctx, tool_calls: Vec<Value>) -> Value {
    let mut all: Vec<Value> = tool_calls
        .into_iter()
        .map(|mut v| {
            let (run, call) = (v["run"].as_str().unwrap_or("").to_string(), v["call"].as_str().unwrap_or("").to_string());
            v["kind"] = json!("tool_call");
            v["decide"] = json!({"method": "POST", "path": format!("/api/v1/approvals/{run}/{call}"), "body": {"verdict": "approve|deny"}});
            v
        })
        .collect();
    for mut v in tenant_pending(c) {
        let id = v["id"].as_str().unwrap_or("").to_string();
        v["kind_of_change"] = v["kind"].clone();
        v["kind"] = json!("tenant");
        v["decide"] = json!({"approve": format!("/api/v1/tenant/approvals/{id}/approve"), "deny": format!("/api/v1/tenant/approvals/{id}/deny"), "method": "POST"});
        all.push(v);
    }
    json!({"approvals": all})
}

fn approvals_waiting(c: &Ctx) -> usize {
    c.sh.deps.store.pending_approvals(Some(&c.who().user)).len() + tenant_pending(c).len()
}

pub fn handle(c: &Ctx, id: &str) -> Out {
    let dir = c.data_dir();
    let hub = hub_for(&dir);
    let me = c.who().user.clone();
    let t = now();
    let nid = c.params.first().map(String::as_str).unwrap_or("");
    let ok = |v: Value, d: String| -> Out { Ok((200, v, d)) };
    match id {
        "notif_list" => {
            let q = &c.req.query;
            let unread = matches!(q.get("unread").map(String::as_str), Some("1" | "true"));
            let snoozed = matches!(q.get("snoozed").map(String::as_str), Some("1" | "true"));
            let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            let before = match q.get("before") {
                Some(b) => Some(b.parse::<i64>().map_err(|_| Fail::new(400, "invalid_input", "before is a time in seconds"))?),
                None => None,
            };
            ok(api::list(&hub, &me, unread, snoozed, limit, before, t), String::new())
        }
        "notif_count" => ok(api::count(&hub, &me, t, approvals_waiting(c)), String::new()),
        "notif_read_all" => ok(api::read_all(&hub, &me, t).map_err(fail)?, String::new()),
        "notif_read" => ok(api::read(&hub, &me, nid, true, t).map_err(fail)?, String::new()),
        "notif_unread" => ok(api::read(&hub, &me, nid, false, t).map_err(fail)?, String::new()),
        "notif_ack" => ok(api::ack(&hub, &me, nid, t).map_err(fail)?, String::new()),
        "notif_snooze" => ok(api::snooze(&hub, &me, nid, &json_body(c.req)?, t).map_err(fail)?, String::new()),
        "notif_channels" => ok(api::channels(&hub, &me), String::new()),
        "notif_optout" => {
            let body = json_body(c.req)?;
            let r = api::optout(&hub, &me, &body).map_err(fail)?;
            let d = format!("{} {}", body["channel"].as_str().unwrap_or(""), if body["out"].as_bool() == Some(true) { "off" } else { "on" });
            ok(r, d)
        }
        "notif_delete" => ok(api::delete(&hub, &me, nid, t).map_err(fail)?, String::new()),
        _ => Err(Fail::new(501, "unsupported", "not implemented")),
    }
}
