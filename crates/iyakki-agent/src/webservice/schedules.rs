//! The cloud profile's schedule routes (`/api/v1/schedules...`): list, create or change, delete, pause, resume, the next-run preview and
//! the status counters. A schedule is a timed trigger of one of the caller's own flows; its owner and team are recorded with the role the
//! owner had (`cloudflows::identity`), and the tenant service runs it with nobody signed in. "Run now" and a flow's run history are the
//! existing `/api/v1/flows/{id}/run` and `/api/v1/flows/{id}/runs`; they are not repeated here.
//!
//! Changing a schedule edits the flow's draft; it takes effect at once only when the draft holds no other unpublished change (then it
//! is published as a new version), otherwise the answer says `active: false` and the person publishes the flow in FLOWS.

use super::handlers::{tenant_gate, Ctx, Out};
use super::server::{json_body, Fail};
use crate::cloudaccess::Access;
use crate::cloudflows::{identity, quota, tz};
use iyakki_flow::sched::Cron;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn handle(c: &Ctx, id: &str) -> Out {
    tenant_gate(c)?;
    match id {
        "sched_status" => status(c),
        "sched_preview" => preview(c),
        "sched_list" => list(c),
        "sched_save" => save(c),
        "sched_delete" => delete(c),
        "sched_pause" => pause(c, true),
        "sched_resume" => pause(c, false),
        _ => Err(Fail::new(404, "not_found", "no such route")),
    }
}

fn flows_dir(c: &Ctx) -> std::path::PathBuf {
    crate::cloudflows::dir(&c.data_dir())
}

fn roles(c: &Ctx) -> Option<crate::cloudroles::Roles> {
    Access::new(&c.data_dir()).roles().ok().flatten()
}

fn bad(m: &str) -> Fail {
    Fail::new(400, "invalid_input", m)
}

fn status(c: &Ctx) -> Out {
    let mut v = crate::cloudflows::real::last_status().unwrap_or_else(|| json!({"leader": false, "note": "the tenant scheduler has not reported yet"}));
    let waits = crate::cloudflows::approval::Waits::new(&flows_dir(c)).waiting().len();
    v["approvals_waiting"] = json!(waits);
    Ok((200, v, String::new()))
}

struct PreviewReq {
    expr: String,
    tz: String,
    count: usize,
    from_ms: i64,
}

/// The next fire times of a cron expression in the tenant's zone, for the form that shows them as the person types.
fn preview(c: &Ctx) -> Out {
    let q = |k: &str| c.req.query.get(k).cloned().unwrap_or_default();
    let num = |k: &str| -> Result<i64, Fail> { if q(k).is_empty() { Ok(0) } else { q(k).parse::<i64>().map_err(|_| bad(&format!("{k} is a number"))) } };
    let r = PreviewReq { expr: q("expr"), tz: q("tz"), count: num("count")?.max(0) as usize, from_ms: num("from_ms")? };
    if r.expr.is_empty() {
        return Err(bad("send ?expr=<cron expression> (optional tz, count of at most 10, from_ms)"));
    }
    let cron = Cron::parse(&r.expr).map_err(|e| bad(&e))?;
    let zone = crate::cloudflows::real::tenant_zone(&c.data_dir());
    let resolve = tz::tenant_resolver(&zone, Some(std::path::PathBuf::from(tz::ZONEINFO)));
    let z = resolve(&r.tz).ok_or_else(|| bad("unknown time zone"))?;
    let mut at = if r.from_ms > 0 { r.from_ms } else { crate::clock::now_unix() * 1000 };
    let mut out = Vec::new();
    for _ in 0..r.count.clamp(1, 10) {
        match cron.next_after(at.div_euclid(1000), z.as_ref()) {
            Some(s) => {
                out.push(s * 1000);
                at = s * 1000;
            }
            None => break,
        }
    }
    Ok((200, json!({"expr": r.expr, "zone": if r.tz.trim().is_empty() || r.tz == "local" { zone } else { r.tz }, "next": out}), String::new()))
}

fn list(c: &Ctx) -> Out {
    let reg = crate::cloudflows::identity::Registry::new(&flows_dir(c));
    let rs = roles(c);
    let mut v = c.sh.deps.extras.schedules(c.who())?;
    for s in &mut v {
        let flow = s["flow"].as_str().unwrap_or("").to_string();
        if let Some(rec) = reg.get(&c.who().user, &flow) {
            let held = rs.as_ref().map_or(identity::OwnerCheck::Ok, |r| identity::check(r, &rec));
            s["team"] = json!(rec.team);
            s["environment"] = json!(rec.environment);
            s["run_as"] = json!(rec.run_as);
            s["owner"] = json!(rec.owner);
            s["role_snapshot"] = json!(rec.role_snapshot);
            s["paused_by_service"] = json!(rec.paused_reason);
            s["owner_check"] = json!(match held {
                identity::OwnerCheck::Ok => Value::Null,
                identity::OwnerCheck::Pause(w) => json!(w),
            });
        } else {
            s["team"] = Value::Null;
        }
    }
    Ok((200, json!({"schedules": v, "limit": quota::Quota::load(&c.data_dir(), None).max_schedules}), String::new()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveReq {
    flow: String,
    trigger: Value,
    team: String,
    #[serde(default)]
    environment: String,
    #[serde(default)]
    run_as: Option<identity::RunAs>,
}

fn save(c: &Ctx) -> Out {
    let r: SaveReq = serde_json::from_value(json_body(c.req)?).map_err(|_| bad("send {\"flow\", \"team\", \"trigger\": {\"id\", \"kind\": \"cron|interval|once\", ...}, optional \"environment\", \"run_as\": \"owner|team-service\"}"))?;
    let kind = r.trigger.get("kind").and_then(Value::as_str).unwrap_or("");
    if !matches!(kind, "cron" | "interval" | "once") {
        return Err(bad("a schedule is a cron, interval or once trigger"));
    }
    if let Some(e) = r.trigger.get("expr").and_then(Value::as_str) {
        Cron::parse(e).map_err(|m| bad(&m))?;
    }
    if kind == "interval" && r.trigger.get("every_s").and_then(Value::as_u64).unwrap_or(0) < 60 {
        return Err(bad("an interval is at least 60 seconds"));
    }
    if kind == "once" && r.trigger.get("at_ms").and_then(Value::as_i64).unwrap_or(0) <= crate::clock::now_unix() * 1000 {
        return Err(bad("a one-time schedule needs a time in the future"));
    }
    let dir = c.data_dir();
    let rs = roles(c).ok_or_else(|| Fail::new(409, "invalid_input", "this tenant has no members list yet, so a schedule cannot have an owner"))?;
    let me = &c.who().user;
    let now = crate::clock::now_unix() as u64;
    let reg = identity::Registry::new(&flows_dir(c));
    let existing = reg.get(me, &r.flow);
    if existing.is_none() {
        let q = quota::Quota::load(&dir, Some(&r.team));
        quota::admit_schedule(&q, reg.list().len()).map_err(|f| Fail::new(429, "quota", &f.reason))?;
    }
    let rec = match existing {
        Some(e) if e.team == r.team && Some(e.run_as) == r.run_as.or(Some(e.run_as)) => e,
        _ => identity::make_schedule(&rs, me, &r.flow, &r.team, if r.environment.is_empty() { "dev" } else { &r.environment }, r.run_as.unwrap_or(identity::RunAs::Owner), now).map_err(|m| Fail::new(403, "denied", &m))?,
    };
    // the flow must be able to run with nobody there
    let flow: iyakki_flow::model::Flow = serde_json::from_value(c.sh.deps.extras.flow_export(c.who(), &r.flow).map_err(Fail::from)?).map_err(|_| Fail::new(500, "io", "the flow could not be read"))?;
    let problems = identity::unattended_problems(&flow, &dir, crate::tenantconfig::vault::for_user(&dir, "_flows").as_ref(), &r.team);
    if !problems.is_empty() {
        return Ok((422, json!({"error": {"code": "not_unattended", "message": "this flow cannot run while nobody is signed in", "problems": problems}}), format!("flow={} refused: {}", r.flow, problems.len())));
    }
    let res = c.sh.deps.extras.schedule_save(c.who(), &r.flow, &r.trigger)?;
    reg.upsert(rec.clone()).map_err(|m| Fail::new(500, "io", &m))?;
    crate::cloudflows::real::audit_line(&dir, me, "flow.schedule.save", &format!("flow={} trigger={} kind={kind} team={} run_as={} role={}", r.flow, r.trigger.get("id").and_then(Value::as_str).unwrap_or(kind), rec.team, serde_json::to_string(&rec.run_as).unwrap_or_default().trim_matches('"'), rec.role_snapshot));
    Ok((200, res, format!("flow={} kind={kind} team={}", r.flow, rec.team)))
}

fn delete(c: &Ctx) -> Out {
    let (flow, trig) = (&c.params[0], &c.params[1]);
    let res = c.sh.deps.extras.schedule_delete(c.who(), flow, trig)?;
    if res["timed_left"].as_u64() == Some(0) {
        let _ = identity::Registry::new(&flows_dir(c)).remove(&c.who().user, flow);
    }
    crate::cloudflows::real::audit_line(&c.data_dir(), &c.who().user, "flow.schedule.delete", &format!("flow={flow} trigger={trig}"));
    Ok((200, res, format!("flow={flow} trigger={trig}")))
}

fn pause(c: &Ctx, paused: bool) -> Out {
    let flow = &c.params[0];
    let reg = identity::Registry::new(&flows_dir(c));
    if !paused {
        // a schedule the service paused (its owner lost access) resumes only when the check passes again
        if let (Some(rec), Some(rs)) = (reg.get(&c.who().user, flow), roles(c)) {
            if let identity::OwnerCheck::Pause(why) = identity::check(&rs, &rec) {
                return Err(Fail::new(403, "denied", &format!("it stays paused: {why}")));
            }
            let _ = reg.set_paused(&c.who().user, flow, None);
        }
    }
    let res = c.sh.deps.extras.schedule_pause(c.who(), flow, paused)?;
    crate::cloudflows::real::audit_line(&c.data_dir(), &c.who().user, if paused { "flow.schedule.pause" } else { "flow.schedule.resume" }, &format!("flow={flow}"));
    Ok((200, res, format!("flow={flow}")))
}
