//! The webhook trigger's gate (F-FLOW-15): the checks that stand between a request from the network and `start_run`. Pure
//! code, no socket: the caller (the web service, once it routes `POST /api/v1/hooks/{path}`, see KNOWN-ISSUES KI-047) hands in
//! the path, the two headers and the body, and a closure that starts the run.
//!
//! The scheme: the sender signs `"<unix seconds>.<body>"` with HMAC-SHA256 over a secret that only the flow's owner and the
//! sender know (kept in the vault, never in the flow file) and sends `X-Iyakki-Timestamp` and `X-Iyakki-Signature: sha256=<hex>`.
//! A request is refused when the timestamp is more than five minutes off, when the signature does not match (compared in
//! constant time), when the same signature was already accepted (a replay), or when the path got too many calls in the last
//! minute. The accepted signature becomes the run's idempotency key, so a retry that slips through still starts one run.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use iyakki_flow::{Flow, StartOutcome, StartRequest, TriggerKind};
use serde_json::{json, Value};

use crate::devmode::auth::{hex, hmac_sha256};

/// How far the sender's clock may be from ours.
pub const WINDOW_S: i64 = 300;
/// Calls per path per minute (valid or not).
pub const PER_MINUTE: usize = 30;
/// The largest body taken.
pub const MAX_BODY: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No enabled webhook trigger has this path (or it has no secret): the same answer for both, so paths cannot be probed.
    NotFound,
    /// Missing or malformed header, body too large.
    BadRequest(&'static str),
    /// Wrong signature or a timestamp out of the window.
    Unauthorized,
    /// This exact signed request was already accepted.
    Replay,
    /// Too many calls for this path in the last minute.
    TooMany,
}

impl Refusal {
    /// The HTTP status the web service answers with.
    pub fn status(&self) -> u16 {
        match self {
            Refusal::NotFound => 404,
            Refusal::BadRequest(_) => 400,
            Refusal::Unauthorized => 401,
            Refusal::Replay => 409,
            Refusal::TooMany => 429,
        }
    }
}

#[derive(Default)]
pub struct WebhookGate {
    /// `(accepted at, signature)`, oldest first.
    seen: Mutex<VecDeque<(i64, String)>>,
    hits: Mutex<HashMap<String, VecDeque<i64>>>,
}

/// The sender's signature for `ts` and `body` (what the verifier expects in `X-Iyakki-Signature`).
pub fn sign(secret: &[u8], ts: i64, body: &[u8]) -> String {
    let mut msg = format!("{ts}.").into_bytes();
    msg.extend_from_slice(body);
    format!("sha256={}", hex(&hmac_sha256(secret, &msg)))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The enabled webhook trigger of `flow` on `path`, if any: `(trigger id, inputs given to the run)`.
pub fn find_trigger<'a>(flows: &'a [Flow], path: &str) -> Option<(&'a Flow, &'a str, &'a Value)> {
    flows.iter().find_map(|f| f.triggers.iter().find(|t| t.enabled && matches!(&t.kind, TriggerKind::Webhook { path: p } if p == path)).map(|t| (f, t.id.as_str(), &t.inputs)))
}

impl WebhookGate {
    pub fn new() -> WebhookGate {
        WebhookGate::default()
    }

    fn rate(&self, path: &str, now: i64) -> bool {
        let mut h = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        if h.len() > 1024 {
            h.retain(|_, q| q.back().is_some_and(|t| now - t < 60));
        }
        let q = h.entry(path.to_string()).or_default();
        while q.front().is_some_and(|t| now - t >= 60) {
            q.pop_front();
        }
        if q.len() >= PER_MINUTE {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Check one request. `Ok` is the accepted signature (the run's idempotency key is derived from it).
    pub fn check(&self, secret: &[u8], path: &str, now: i64, ts_header: &str, sig_header: &str, body: &[u8]) -> Result<String, Refusal> {
        if body.len() > MAX_BODY {
            return Err(Refusal::BadRequest("the body is too large"));
        }
        if !self.rate(path, now) {
            return Err(Refusal::TooMany);
        }
        let ts: i64 = ts_header.trim().parse().map_err(|_| Refusal::BadRequest("X-Iyakki-Timestamp must be unix seconds"))?;
        if !sig_header.trim().starts_with("sha256=") {
            return Err(Refusal::BadRequest("X-Iyakki-Signature must look like sha256=<hex>"));
        }
        if (now - ts).abs() > WINDOW_S || !same(&sign(secret, ts, body), sig_header.trim().to_ascii_lowercase().as_str()) {
            return Err(Refusal::Unauthorized);
        }
        let sig = sig_header.trim().to_ascii_lowercase();
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        while seen.front().is_some_and(|(t, _)| now - t > 2 * WINDOW_S) {
            seen.pop_front();
        }
        if seen.iter().any(|(_, s)| same(s, &sig)) {
            return Err(Refusal::Replay);
        }
        seen.push_back((now, sig.clone()));
        Ok(sig)
    }

    /// The whole step: find the trigger, fetch its secret, check the request, start the run. `secret_for` answers the vault
    /// lookup for a path; `start` is `Engine::start_run`. The answer is the JSON the endpoint returns.
    #[allow(clippy::too_many_arguments)]
    pub fn handle(&self, flows: &[Flow], secret_for: &dyn Fn(&str) -> Option<Vec<u8>>, path: &str, now: i64, ts_header: &str, sig_header: &str, body: &[u8], start: &mut dyn FnMut(StartRequest) -> Result<StartOutcome, String>) -> Result<Value, Refusal> {
        let (flow, trigger, base) = find_trigger(flows, path).ok_or(Refusal::NotFound)?;
        let secret = secret_for(path).filter(|s| !s.is_empty()).ok_or(Refusal::NotFound)?;
        let sig = self.check(&secret, path, now, ts_header, sig_header, body)?;
        let mut inputs = if base.is_object() { base.clone() } else { json!({}) };
        if let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) {
            for (k, v) in m {
                inputs[k] = v;
            }
        }
        let req = StartRequest::new(&flow.id).trigger("webhook", trigger).inputs(inputs).key(&format!("hook-{sig}"));
        let out = start(req).map_err(|_| Refusal::BadRequest("the flow could not be started"))?;
        Ok(json!({"run": out.run_id, "existing": out.existing}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"correct horse battery staple 0123456789";
    const NOW: i64 = 1_800_000_000;

    fn flow(path: &str, enabled: bool) -> Flow {
        let mut f: Flow = serde_json::from_value(json!({"id": "deploy", "name": "Deploy", "nodes": [], "edges": []})).unwrap();
        let mut t = iyakki_flow::Trigger::new("w", TriggerKind::Webhook { path: path.into() });
        t.enabled = enabled;
        t.inputs = json!({"env": "staging"});
        f.triggers.push(t);
        f
    }

    fn call(g: &WebhookGate, flows: &[Flow], now: i64, ts: &str, sig: &str, body: &[u8]) -> (Result<Value, Refusal>, Vec<StartRequest>) {
        let mut started = vec![];
        let r = g.handle(flows, &|p| (p == "/deploy").then(|| SECRET.to_vec()), "/deploy", now, ts, sig, body, &mut |r| {
            started.push(r);
            Ok(StartOutcome { run_id: "r1".into(), existing: false })
        });
        (r, started)
    }

    #[test]
    fn a_valid_signature_starts_the_flow_with_the_body_merged_over_the_triggers_inputs() {
        let (g, flows) = (WebhookGate::new(), vec![flow("/deploy", true)]);
        let body = br#"{"ref":"main"}"#;
        let (r, started) = call(&g, &flows, NOW, &NOW.to_string(), &sign(SECRET, NOW, body), body);
        assert_eq!(r.unwrap(), json!({"run": "r1", "existing": false}));
        assert_eq!(started.len(), 1);
        assert_eq!((started[0].flow_id.as_str(), started[0].trigger.kind.as_str(), started[0].trigger.by.as_str()), ("deploy", "webhook", "w"));
        assert_eq!(started[0].inputs, json!({"env": "staging", "ref": "main"}));
        assert!(started[0].idempotency_key.as_deref().unwrap().starts_with("hook-sha256="));
    }

    #[test]
    fn a_bad_wrong_or_stale_signature_is_refused_and_starts_nothing() {
        let (g, flows) = (WebhookGate::new(), vec![flow("/deploy", true)]);
        let body = b"{}";
        let good = sign(SECRET, NOW, body);
        let bad = sign(b"another secret entirely", NOW, body);
        for (ts, sig, body, want) in [
            (NOW.to_string(), bad.clone(), &body[..], Refusal::Unauthorized),
            (NOW.to_string(), good.clone(), &b"{\"x\":1}"[..], Refusal::Unauthorized),
            ((NOW - 301).to_string(), sign(SECRET, NOW - 301, body), &body[..], Refusal::Unauthorized),
            ((NOW + 301).to_string(), sign(SECRET, NOW + 301, body), &body[..], Refusal::Unauthorized),
            (NOW.to_string(), "".into(), &body[..], Refusal::BadRequest("X-Iyakki-Signature must look like sha256=<hex>")),
            ("soon".into(), good.clone(), &body[..], Refusal::BadRequest("X-Iyakki-Timestamp must be unix seconds")),
        ] {
            let (r, started) = call(&g, &flows, NOW, &ts, &sig, body);
            assert_eq!(r.unwrap_err(), want);
            assert!(started.is_empty());
        }
        assert_eq!(Refusal::Unauthorized.status(), 401);
    }

    #[test]
    fn the_same_signed_request_twice_is_a_replay_but_the_same_body_signed_later_is_new() {
        let (g, flows) = (WebhookGate::new(), vec![flow("/deploy", true)]);
        let body = b"{}";
        let sig = sign(SECRET, NOW, body);
        assert!(call(&g, &flows, NOW, &NOW.to_string(), &sig, body).0.is_ok());
        let (r, started) = call(&g, &flows, NOW + 5, &NOW.to_string(), &sig, body);
        assert_eq!(r.unwrap_err(), Refusal::Replay);
        assert!(started.is_empty());
        assert_eq!(Refusal::Replay.status(), 409);
        assert!(call(&g, &flows, NOW + 6, &(NOW + 6).to_string(), &sign(SECRET, NOW + 6, body), body).0.is_ok());
    }

    #[test]
    fn a_path_without_an_enabled_trigger_or_without_a_secret_looks_the_same_and_the_rate_is_limited() {
        let g = WebhookGate::new();
        let body = b"{}";
        let sig = sign(SECRET, NOW, body);
        assert_eq!(call(&g, &[flow("/deploy", false)], NOW, &NOW.to_string(), &sig, body).0.unwrap_err(), Refusal::NotFound);
        assert_eq!(call(&g, &[flow("/other", true)], NOW, &NOW.to_string(), &sig, body).0.unwrap_err(), Refusal::NotFound);
        let mut nosecret = vec![];
        let r = g.handle(&[flow("/deploy", true)], &|_| None, "/deploy", NOW, &NOW.to_string(), &sig, body, &mut |r| {
            nosecret.push(r);
            Ok(StartOutcome { run_id: "x".into(), existing: false })
        });
        assert_eq!(r.unwrap_err(), Refusal::NotFound);
        assert!(nosecret.is_empty());
        // wrong signatures count against the rate too: the 31st call within a minute is refused whatever it carries
        let flows = vec![flow("/deploy", true)];
        for i in 0..PER_MINUTE {
            assert_eq!(call(&g, &flows, NOW + 1, &NOW.to_string(), "sha256=00", body).0.unwrap_err(), Refusal::Unauthorized, "{i}");
        }
        assert_eq!(call(&g, &flows, NOW + 1, &NOW.to_string(), &sig, body).0.unwrap_err(), Refusal::TooMany);
        assert_eq!(Refusal::TooMany.status(), 429);
        // a minute later it is open again
        assert!(call(&g, &flows, NOW + 70, &(NOW + 70).to_string(), &sign(SECRET, NOW + 70, body), body).0.is_ok());
    }

    #[test]
    fn a_huge_body_is_refused_before_anything_is_computed() {
        let (g, flows) = (WebhookGate::new(), vec![flow("/deploy", true)]);
        let body = vec![b'a'; MAX_BODY + 1];
        assert_eq!(call(&g, &flows, NOW, &NOW.to_string(), &sign(SECRET, NOW, &body), &body).0.unwrap_err(), Refusal::BadRequest("the body is too large"));
    }
}
