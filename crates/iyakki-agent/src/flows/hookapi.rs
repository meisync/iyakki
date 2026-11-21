//! A person's webhooks as the web service's cloud profile offers them (F-FLOW-15, hooks in tenant pods). The FLOWS panel and the assistant's
//! `webhook_*` tools are for a machine with a screen; a pod has neither, so the same operations are an authenticated API of the person's own: the
//! secret address and the shared secret come back in the answer to the person's own call (once, over TLS), exactly as the panel shows them, and
//! never to a model. The rules are those of [`FlowEnv`]'s `hook_*` functions: only the owner, the FLOWS right, a published trigger, off until
//! switched on, public for at most seven days.

use serde_json::{json, Value};

use super::hookenv::{Created, HookOpts, HookView};
use super::hookstore::take_reveal;
use super::FlowEnv;

const BY: &str = "web";

/// What a person may do to their webhooks through the service.
#[derive(Debug)]
pub enum HookOp<'a> {
    /// All of the person's webhooks, or those of one flow.
    List(Option<&'a str>),
    Create { flow: &'a str, opts: HookOpts },
    Regenerate(&'a str),
    Enable(&'a str),
    Disable(&'a str),
    /// Open to senders outside the local network for this many minutes (0 closes it).
    Public(&'a str, u64),
    /// The address and secret a chat tool left for the person's screen, once.
    Reveal,
}

fn view_json(v: &HookView, now: i64) -> Value {
    let last = v.deliveries.last();
    json!({
        "id": v.id, "flow": v.flow, "flow_name": v.flow_name, "enabled": v.enabled, "signed": v.signed, "published": v.published,
        "public_until": v.public_until, "open_to_public": v.public_until > now, "created": v.created, "rotated": v.rotated,
        "deliveries": v.deliveries.iter().rev().take(20).map(|d| json!({"at": d.at, "outcome": d.outcome, "http": d.http, "run": d.run, "bytes": d.bytes, "from": d.from})).collect::<Vec<_>>(),
        "last": last.map(|d| d.outcome.clone()),
    })
}

fn secret_json(c: &Created) -> Value {
    json!({"id": c.id, "flow": c.flow, "path": c.path, "secret": c.secret, "shown_once": true})
}

/// Do one operation as the environment's person. A new address or secret is in the answer and nowhere else (no audit line, no log).
pub fn run(env: &FlowEnv, op: HookOp<'_>) -> Result<Value, String> {
    let now = crate::clock::now_unix();
    match op {
        HookOp::List(flow) => Ok(json!({"hooks": env.hook_views(flow).iter().map(|v| view_json(v, now)).collect::<Vec<_>>()})),
        HookOp::Create { flow, opts } => Ok(secret_json(&env.hook_create(flow, &opts, BY)?)),
        HookOp::Regenerate(id) => Ok(secret_json(&env.hook_regenerate(id, BY)?)),
        HookOp::Enable(id) => env.hook_enable(id, BY).map(|()| json!({"id": id, "enabled": true})),
        HookOp::Disable(id) => env.hook_disable(id, BY).map(|()| json!({"id": id, "enabled": false})),
        HookOp::Public(id, minutes) => env.hook_public(id, minutes, BY).map(|until| json!({"id": id, "public_until": until, "open_to_public": until > now})),
        HookOp::Reveal => Ok(match take_reveal(&env.user) {
            Some(r) => json!({"waiting": true, "id": r.hook, "flow": r.flow, "path": r.path, "secret": r.secret, "shown_once": true}),
            None => json!({"waiting": false}),
        }),
    }
}
