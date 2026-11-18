//! `/hooks/<id>/<token>`: the webhook endpoint (F-FLOW-15). It is not an API route: it has no bearer token and no role, because the
//! sender is a program, not a signed-in person. The secret address, the optional signature, the local-network rule and the owner's
//! rights are all checked by the gate in `flows/webhook.rs`; this file only turns an HTTP request into a [`HookRequest`] and the
//! answer back into a response. Only the `agent-api` profile (a machine with an owner's flows) has it; a cloud tenant pod answers 404.
//!
//! The path carries the token, so nothing here logs it, audits it or echoes it.

use super::server::Shared;
use super::Profile;
use crate::flows::webhook::HookRequest;
use crate::httpmini::{Request, Response};
use serde_json::json;
use std::net::IpAddr;

pub const PREFIX: &str = "/hooks/";

pub fn is_hook_path(path: &str) -> bool {
    path.starts_with(PREFIX)
}

fn not_found() -> Response {
    Response::json(404, &json!({"error": {"code": "not_found", "message": "not found"}})).header("Cache-Control", "no-store")
}

pub fn serve(sh: &Shared, req: &Request, peer: IpAddr) -> Response {
    if sh.profile != Profile::AgentApi {
        return not_found();
    }
    let mut parts = req.path[PREFIX.len()..].split('/');
    let (Some(id), Some(token), None) = (parts.next(), parts.next(), parts.next()) else { return not_found() };
    let rq = HookRequest {
        method: &req.method,
        id,
        token,
        content_type: req.header("content-type"),
        timestamp: req.header("x-iyakki-timestamp"),
        signature: req.header("x-iyakki-signature"),
        idempotency_key: req.header("idempotency-key"),
        body: &req.body,
        peer,
    };
    match sh.deps.extras.hook_serve(sh.deps.auth.as_ref(), sh.deps.audit.as_ref(), &rq) {
        None => not_found(),
        Some(a) => {
            let r = Response::json(a.status, &a.body).header("Cache-Control", "no-store");
            match a.retry_after {
                Some(s) => r.header("Retry-After", &s.to_string()),
                None => r,
            }
        }
    }
}
