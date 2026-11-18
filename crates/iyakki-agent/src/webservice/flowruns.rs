//! The cloud profile's flow-run routes: start a run of one of the caller's own flows, list a flow's runs (paged) and read one run.
//! The flows are the caller's own (one environment per person), the Flows permission and the tenant gate decide who may call, and a
//! question a run asks a person is answered only by that person's own approval; there is no route here that answers it.

use super::handlers::{tenant_gate, Ctx, Out};
use super::server::{json_body, Fail};
use serde::Deserialize;
use serde_json::{json, Value};

const PER_DEFAULT: usize = 25;
const PER_MAX: usize = 100;
const PAGE_MAX: usize = 10_000;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunReq {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    inputs: Value,
}

pub fn start(c: &Ctx) -> Out {
    tenant_gate(c)?;
    let r: RunReq = if c.req.body.iter().all(u8::is_ascii_whitespace) {
        RunReq::default()
    } else {
        serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {optional \"draft\": true, optional \"inputs\": {...}}"))?
    };
    if !(r.inputs.is_null() || r.inputs.is_object()) || r.inputs.to_string().len() > 64 * 1024 {
        return Err(Fail::new(400, "invalid_input", "inputs is an object of at most 64 KiB"));
    }
    let id = &c.params[0];
    let run = c.sh.deps.extras.flow_run(c.who(), id, r.draft, &r.inputs)?;
    watch(c.sh.deps.clone(), c.who().clone(), id.clone(), run.clone(), c.notifier());
    Ok((200, json!({"run": run, "flow": id}), format!("flow={id} run={run} draft={}", r.draft)))
}

/// Follow a run the person just started and tell their event stream when its status changes (`{type: flowrun, data: {flow, run,
/// status}}`), so the Runs and Flows pages refresh at once and poll only as a safety net. Asks the run's own status once a second,
/// stops at its end or after an hour, and sends the status word only (what the run did is read through the API, which checks who may).
fn watch(deps: std::sync::Arc<super::deps::Deps>, who: iyakki_core_api::Principal, flow: String, run: String, notify: super::runs::Notify) {
    let _ = std::thread::Builder::new().name("iyakki-svc-flowwatch".into()).spawn(move || {
        let mut told = String::new();
        for _ in 0..3600 {
            let status = deps.extras.flow_run_get(&who, &flow, &run).ok().and_then(|v| v["status"].as_str().map(str::to_string));
            if let Some(s) = status {
                if s != told {
                    notify(crate::notifyhub::events::event("flowrun", json!({"flow": flow, "run": run, "status": s})));
                    told = s.clone();
                }
                if matches!(s.as_str(), "failed" | "succeeded" | "cancelled") {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

fn num(c: &Ctx, k: &str, default: usize, max: usize) -> Result<usize, Fail> {
    match c.req.query.get(k).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(v) => v.parse::<usize>().map(|n| n.min(max)).map_err(|_| Fail::new(400, "invalid_input", &format!("{k} is a number"))),
    }
}

pub fn list(c: &Ctx) -> Out {
    tenant_gate(c)?;
    let per = num(c, "per", PER_DEFAULT, PER_MAX)?.max(1);
    let page = num(c, "page", 0, PAGE_MAX)?;
    let all = c.sh.deps.extras.flow_runs(c.who(), &c.params[0])?;
    let runs: Vec<Value> = all.iter().skip(page * per).take(per).cloned().collect();
    Ok((200, json!({"runs": runs, "page": page, "per": per, "more": all.len() > (page + 1) * per}), String::new()))
}

pub fn view(c: &Ctx) -> Out {
    tenant_gate(c)?;
    Ok((200, c.sh.deps.extras.flow_run_get(c.who(), &c.params[0], &c.params[1])?, String::new()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportReq {
    flow: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ValidateReq {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    flow: Option<Value>,
}

/// Bring a flow in as a safe draft of the caller's own.
pub fn import(c: &Ctx) -> Out {
    tenant_gate(c)?;
    let r: ImportReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"flow\": {...}} (the JSON that GET /api/v1/flows/{id} gives)"))?;
    if !r.flow.is_object() {
        return Err(Fail::new(400, "invalid_input", "flow is an object"));
    }
    let out = c.sh.deps.extras.flow_import(c.who(), &r.flow)?;
    let id = out["id"].as_str().unwrap_or("").to_string();
    Ok((200, out, format!("flow={id}")))
}

/// What is wrong with a flow of the caller's, or with one given as JSON.
pub fn validate(c: &Ctx) -> Out {
    tenant_gate(c)?;
    let r: ValidateReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"id\": ...} or {\"flow\": {...}}"))?;
    if r.id.is_some() == r.flow.is_some() {
        return Err(Fail::new(400, "invalid_input", "give either id or flow, not both and not neither"));
    }
    if let Some(id) = &r.id {
        if id.is_empty() || id.len() > 64 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')) {
            return Err(Fail::new(400, "invalid_input", "that is not a flow id"));
        }
    }
    let issues = c.sh.deps.extras.flow_validate(c.who(), r.id.as_deref(), r.flow.as_ref())?;
    let ok = !issues.iter().any(|i| i["level"] == "error");
    Ok((200, json!({"ok": ok, "issues": issues}), format!("issues={}", issues.len())))
}

/// Stop one run of one of the caller's own flows.
pub fn stop(c: &Ctx) -> Out {
    tenant_gate(c)?;
    c.sh.deps.extras.flow_run_stop(c.who(), &c.params[0], &c.params[1])?;
    Ok((200, json!({"ok": true}), format!("flow={} run={}", c.params[0], c.params[1])))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideReq {
    verdict: String,
}

/// Answer one waiting step of one of the caller's own runs (the flows runner only).
pub fn decide(c: &Ctx) -> Out {
    tenant_gate(c)?;
    let r: DecideReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"verdict\": \"approve\" or \"deny\"}"))?;
    let approved = match r.verdict.as_str() {
        "approve" => true,
        "deny" => false,
        _ => return Err(Fail::new(400, "invalid_input", "the verdict is \"approve\" or \"deny\"")),
    };
    let (id, run, step) = (&c.params[0], &c.params[1], &c.params[2]);
    c.sh.deps.extras.flow_run_decide(c.who(), id, run, step, approved)?;
    Ok((200, json!({"ok": true}), format!("flow={id} run={run} step={step} verdict={}", r.verdict)))
}
