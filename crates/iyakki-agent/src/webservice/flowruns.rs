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
    Ok((200, json!({"run": run, "flow": id}), format!("flow={id} run={run} draft={}", r.draft)))
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
