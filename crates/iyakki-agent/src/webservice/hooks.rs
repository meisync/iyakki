//! `/hooks/<id>/<token>`: the webhook endpoint (F-FLOW-15). It is not an API route: it has no bearer token and no role, because the
//! sender is a program, not a signed-in person. The secret address, the optional signature, the local-network rule and the owner's
//! rights are all checked by the gate in `flows/webhook.rs`; this file only turns an HTTP request into a [`HookRequest`] and the
//! answer back into a response. The `agent-api` profile (a machine with an owner's flows) and the cloud profile (a tenant pod, F-FLOW-15)
//! have it; the other profiles answer 404. In a pod the peer is the cloud gateway, so the sender's own address is read from the
//! `X-Forwarded-For` the gateway sets (it replaces any value a client sent); a request that did not come from a private address, or has no
//! usable forwarded address, is treated as coming from OUTSIDE the local network (never as local).
//!
//! The path carries the token, so nothing here logs it, audits it or echoes it.

use super::server::Shared;
use super::Profile;
use crate::flows::webhook::HookRequest;
use crate::httpmini::{Request, Response};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr};

pub const PREFIX: &str = "/hooks/";

pub fn is_hook_path(path: &str) -> bool {
    path.starts_with(PREFIX)
}

fn not_found() -> Response {
    Response::json(404, &json!({"error": {"code": "not_found", "message": "not found"}})).header("Cache-Control", "no-store")
}

/// The profiles that take webhooks.
pub fn has_hooks(p: Profile) -> bool {
    matches!(p, Profile::AgentApi | Profile::Cloud)
}

/// The sender's address as the gate should judge it. A tenant pod sits behind the cloud gateway: when the peer is a private address (the
/// gateway) the last address of `X-Forwarded-For` is the sender; a peer that is not private is the sender itself (nobody in front of it, so a
/// header it sends means nothing); a missing or unreadable header gives the unspecified address, which is never local. Other profiles: the peer.
pub fn sender_ip(profile: Profile, peer: IpAddr, forwarded_for: Option<&str>) -> IpAddr {
    if profile != Profile::Cloud || !crate::flows::hookstore::is_local(peer) {
        return peer;
    }
    forwarded_for.and_then(|h| h.rsplit(',').next()).and_then(|a| a.trim().parse::<IpAddr>().ok()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

pub fn serve(sh: &Shared, req: &Request, peer: IpAddr) -> Response {
    if !has_hooks(sh.profile) {
        return not_found();
    }
    let peer = sender_ip(sh.profile, peer, req.header("x-forwarded-for"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn only_the_agent_api_and_the_cloud_profile_take_webhooks() {
        assert!(has_hooks(Profile::AgentApi) && has_hooks(Profile::Cloud));
        assert!(!has_hooks(Profile::FlowsRunner) && !has_hooks(Profile::DataApi) && !has_hooks(Profile::TileHost));
    }

    #[test]
    fn a_pod_reads_the_sender_from_the_gateways_header_and_never_trusts_a_header_from_outside() {
        let gw = ip("10.42.0.7");
        assert_eq!(sender_ip(Profile::Cloud, gw, Some("203.0.113.9")), ip("203.0.113.9"));
        // the last entry is the one the gateway set; earlier ones are the client's own claims
        assert_eq!(sender_ip(Profile::Cloud, gw, Some("10.0.0.1, 203.0.113.9")), ip("203.0.113.9"));
        assert_eq!(sender_ip(Profile::Cloud, gw, Some("203.0.113.9, 192.168.1.5")), ip("192.168.1.5"));
        assert_eq!(sender_ip(Profile::Cloud, gw, Some("2001:db8::1")), ip("2001:db8::1"));
        // no header or junk: outside, never local
        for h in [None, Some(""), Some("not an address"), Some("10.0.0.1,")] {
            let s = sender_ip(Profile::Cloud, gw, h);
            assert!(!crate::flows::hookstore::is_local(s), "{h:?} gave {s}");
        }
        // a peer that is not private is the sender; its header is only a claim
        assert_eq!(sender_ip(Profile::Cloud, ip("198.51.100.4"), Some("10.0.0.1")), ip("198.51.100.4"));
        // the agent API has no gateway in front of it
        assert_eq!(sender_ip(Profile::AgentApi, gw, Some("203.0.113.9")), gw);
    }
}
