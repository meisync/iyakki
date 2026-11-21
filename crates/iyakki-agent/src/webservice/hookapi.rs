//! The cloud profile's webhook routes (F-FLOW-15): the person's own authenticated way to make, renew, switch on and off and open a webhook, since
//! a tenant pod has no FLOWS screen. The new address and shared secret are in the answer to the call that made them, once; the audit lines name the
//! hook and the flow only. The endpoint that senders call is `hooks.rs`. Everything here is the person's own hooks: another person's are not found.

use super::deps::HookOp;
use super::handlers::{tenant_gate, Ctx, Out};
use super::server::{json_body, Fail};
use crate::flows::hookenv::HookOpts;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    flow: String,
    #[serde(default = "yes")]
    signed: bool,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    max_body: usize,
    #[serde(default)]
    content_types: Vec<String>,
    #[serde(default)]
    rate_per_min: u32,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Public {
    minutes: u64,
}

fn bad(m: &str) -> Fail {
    Fail::new(400, "invalid_input", m)
}

pub fn handle(c: &Ctx, id: &str) -> Out {
    tenant_gate(c)?;
    let who = c.who();
    let hook = c.params.first().map(String::as_str).unwrap_or("");
    if !hook.is_empty() && !crate::flows::hook_id_ok(hook) {
        return Err(Fail::new(404, "not_found", "no such webhook"));
    }
    let say = |v: Value, detail: String| -> Out { Ok((200, v, detail)) };
    let ex = &c.sh.deps.extras;
    match id {
        "hooks_list" => say(ex.hook_manage(who, HookOp::List(None))?, String::new()),
        "hooks_reveal" => say(ex.hook_manage(who, HookOp::Reveal)?, String::new()),
        "hooks_create" => {
            let r: Create = serde_json::from_value(json_body(c.req)?).map_err(|_| bad("send {\"flow\": \"id\", optional \"signed\", \"methods\", \"max_body\", \"content_types\", \"rate_per_min\"}"))?;
            if r.flow.is_empty() || r.flow.len() > 64 || r.methods.len() > 8 || r.content_types.len() > 3 || r.methods.iter().chain(r.content_types.iter()).any(|m| m.len() > 16) || r.rate_per_min > 100_000 {
                return Err(bad("the flow id, methods, content types or rate is not in range"));
            }
            let opts = HookOpts { signed: r.signed, methods: r.methods, max_body: r.max_body, content_types: r.content_types, rate_per_min: r.rate_per_min };
            // the audit line has the flow and the hook, never the address
            let v = ex.hook_manage(who, HookOp::Create { flow: &r.flow, opts })?;
            let detail = format!("hook={} flow={} signed={}", v["id"].as_str().unwrap_or(""), r.flow, r.signed);
            say(v, detail)
        }
        "hooks_regenerate" => say(ex.hook_manage(who, HookOp::Regenerate(hook))?, format!("hook={hook}")),
        "hooks_enable" => say(ex.hook_manage(who, HookOp::Enable(hook))?, format!("hook={hook}")),
        "hooks_disable" => say(ex.hook_manage(who, HookOp::Disable(hook))?, format!("hook={hook}")),
        "hooks_public" => {
            let r: Public = serde_json::from_value(json_body(c.req)?).map_err(|_| bad("send {\"minutes\": N} (0 closes it)"))?;
            say(ex.hook_manage(who, HookOp::Public(hook, r.minutes))?, format!("hook={hook} minutes={}", r.minutes.min(10_080)))
        }
        _ => Err(Fail::new(404, "not_found", "not found")),
    }
}
