//! The webhook trigger (F-FLOW-15): the checks that stand between a request from the network and a flow run. Pure code, no socket:
//! the web service (`webservice/hooks.rs`) hands in the request and a [`HookWorld`] that answers the questions only the machine can
//! (who is the owner, what does the published flow say, what is the shared secret, start the run).
//!
//! The address is `/hooks/<id>/<token>`: `id` names the hook, `token` is 256 random bits shown once and kept only as a SHA-256 hash.
//! The checks, in this order, and what each costs the sender:
//!
//! 1. the address has the right shape, the sender is not hammering (per sender address); wrong shape or unknown hook: 404;
//! 2. the token matches (compared in constant time; an unknown hook is compared against a dummy so the time does not tell);
//! 3. the hook is switched on, the sender is on the local network (or the owner opened it to the public until a time), the owner still
//!    may use FLOWS, and the owner's published flow still has the trigger;
//! 4. the method, the body size, the rate (calls a minute for this hook) and the content type;
//! 5. when the hook is signed: `X-Iyakki-Timestamp` (unix seconds, within five minutes) and `X-Iyakki-Signature: sha256=<hex>`, the
//!    HMAC-SHA256 of `"<timestamp>.<body>"` made with the shared secret, compared in constant time; the same signature twice is a replay;
//! 6. the body becomes **data**: JSON, a form or text, under the run's input `webhook` together with the method and content type. It is
//!    never run, never put in a command line by the engine and never evaluated as a template (inserted values are not re-evaluated).
//!
//! The answer is `202 {"run": "..."}`. Every outcome of a known hook is noted in its last 20 deliveries and, when it matters, in the
//! audit log: the hook, the flow, the outcome, the sender's address; never the token, the secret, the signature or the body.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use iyakki_flow::{Trigger, TriggerKind};
use serde_json::{json, Map, Value};

use super::hookstore::{is_local, same, token_hash, Delivery, HookRec, HookStore};
use crate::devmode::auth::{hex, hmac_sha256};

/// How far the sender's clock may be from ours.
pub const WINDOW_S: i64 = 300;
pub const DEFAULT_PER_MIN: u32 = 30;
pub const DEFAULT_MAX_BODY: usize = 16 * 1024;
/// The largest body any webhook takes.
pub const HARD_MAX_BODY: usize = 64 * 1024;
/// Failed address or token tries per sender address per minute before it is told to wait.
pub const BAD_PER_MIN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Json,
    Form,
    Text,
}

impl Content {
    fn label(self) -> &'static str {
        match self {
            Content::Json => "json",
            Content::Form => "form",
            Content::Text => "text",
        }
    }
}

/// What the published flow's webhook trigger says (its settings with the defaults filled in).
#[derive(Clone, Debug, PartialEq)]
pub struct HookCfg {
    pub trigger: String,
    pub methods: Vec<String>,
    pub max_body: usize,
    pub content: Vec<Content>,
    pub rate_per_min: u32,
    pub signed: bool,
    /// The owner's fixed inputs for the run.
    pub inputs: Value,
}

impl HookCfg {
    /// The settings of a webhook trigger (`None`: not a webhook, or switched off in the flow).
    pub fn from_trigger(t: &Trigger) -> Option<HookCfg> {
        let TriggerKind::Webhook { methods, max_body, content_types, rate_per_min, signed, .. } = &t.kind else { return None };
        if !t.enabled {
            return None;
        }
        let content: Vec<Content> = content_types.iter().filter_map(|c| match c.as_str() {
            "json" => Some(Content::Json),
            "form" => Some(Content::Form),
            "text" => Some(Content::Text),
            _ => None,
        }).collect();
        Some(HookCfg {
            trigger: t.id.clone(),
            methods: if methods.is_empty() { vec!["POST".into()] } else { methods.clone() },
            max_body: if *max_body == 0 { DEFAULT_MAX_BODY } else { (*max_body).min(HARD_MAX_BODY) },
            content: if content.is_empty() { vec![Content::Json] } else { content },
            rate_per_min: if *rate_per_min == 0 { DEFAULT_PER_MIN } else { *rate_per_min },
            signed: *signed,
            inputs: t.inputs.clone(),
        })
    }
}

/// What the machine answers for the gate.
pub trait HookWorld {
    /// May the owner still use FLOWS (the account exists and has the permission)?
    fn owner_allowed(&self, owner: &str) -> bool;
    /// The settings in the owner's published flow, `None` when the flow, its published version or the trigger is gone or off.
    fn config(&self, rec: &HookRec) -> Option<HookCfg>;
    /// The shared secret from the owner's vault; `Err` says the vault is locked or the secret missing.
    fn secret(&self, rec: &HookRec) -> Result<Vec<u8>, String>;
    /// Start the run as the owner. `Ok((run id, already existed))`.
    fn start(&self, rec: &HookRec, cfg: &HookCfg, inputs: Value, key: Option<String>) -> Result<(String, bool), String>;
    fn audit(&self, owner: &str, event: &str, detail: &str);
    fn now(&self) -> i64;
}

pub struct HookRequest<'a> {
    pub method: &'a str,
    pub id: &'a str,
    pub token: &'a str,
    pub content_type: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub signature: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
    pub body: &'a [u8],
    pub peer: IpAddr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
    pub retry_after: Option<u64>,
}

impl Answer {
    fn err(status: u16, code: &str, msg: &str) -> Answer {
        Answer { status, body: json!({"error": {"code": code, "message": msg}}), retry_after: None }
    }
}

/// The signature the sender puts in `X-Iyakki-Signature` for `ts` and `body`.
pub fn sign(secret: &[u8], ts: i64, body: &[u8]) -> String {
    let mut msg = format!("{ts}.").into_bytes();
    msg.extend_from_slice(body);
    format!("sha256={}", hex(&hmac_sha256(secret, &msg)))
}

#[derive(Default)]
struct Gate {
    /// `(time, hook, signature)` of accepted signed requests, oldest first.
    seen: VecDeque<(i64, String, String)>,
    hits: HashMap<String, VecDeque<i64>>,
    bad: HashMap<String, VecDeque<i64>>,
}

/// The registry of hooks and the memory of the gate (rates and replays).
pub struct Hooks {
    pub store: Arc<HookStore>,
    gate: Mutex<Gate>,
}

static HOOKS: std::sync::OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Hooks>>>> = std::sync::OnceLock::new();

fn window(q: &mut VecDeque<i64>, now: i64) {
    while q.front().is_some_and(|t| now - t >= 60) {
        q.pop_front();
    }
}

impl Hooks {
    pub fn new(store: Arc<HookStore>) -> Hooks {
        Hooks { store, gate: Mutex::new(Gate::default()) }
    }

    /// The one gate of this data directory.
    pub fn shared(dir: &std::path::Path) -> Arc<Hooks> {
        let m = HOOKS.get_or_init(|| Mutex::new(HashMap::new()));
        m.lock().unwrap_or_else(|e| e.into_inner()).entry(dir.join("hooks.json")).or_insert_with(|| Arc::new(Hooks::new(HookStore::shared(dir)))).clone()
    }

    fn gate(&self) -> std::sync::MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// May this sender try again (wrong tries count; `false`: tell it to wait)?
    fn sender_ok(&self, peer: &str, now: i64) -> bool {
        let mut g = self.gate();
        if g.bad.len() > 2048 {
            g.bad.retain(|_, q| q.back().is_some_and(|t| now - t < 60));
        }
        let q = g.bad.entry(peer.to_string()).or_default();
        window(q, now);
        q.len() < BAD_PER_MIN
    }

    fn note_bad(&self, peer: &str, now: i64) {
        self.gate().bad.entry(peer.to_string()).or_default().push_back(now);
    }

    fn rate_ok(&self, id: &str, limit: u32, now: i64) -> bool {
        let mut g = self.gate();
        if g.hits.len() > 1024 {
            g.hits.retain(|_, q| q.back().is_some_and(|t| now - t < 60));
        }
        let q = g.hits.entry(id.to_string()).or_default();
        window(q, now);
        if q.len() >= limit as usize {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Is this signature new for the hook? (Remembers it when it is.)
    fn first_use(&self, id: &str, sig: &str, now: i64) -> bool {
        let mut g = self.gate();
        while g.seen.front().is_some_and(|(t, ..)| now - t > 2 * WINDOW_S) {
            g.seen.pop_front();
        }
        if g.seen.iter().any(|(_, h, s)| h == id && same(s, sig)) {
            return false;
        }
        g.seen.push_back((now, id.to_string(), sig.to_string()));
        true
    }

    /// The whole step for one request. Never panics on hostile input; always answers.
    pub fn serve(&self, w: &dyn HookWorld, rq: &HookRequest) -> Answer {
        let now = w.now();
        let peer = rq.peer.to_string();
        let not_found = || Answer::err(404, "not_found", "not found");
        // 1. shape and sender
        if !crate::flows::hook_id_ok(rq.id) || rq.token.len() != 64 || !rq.token.bytes().all(|c| c.is_ascii_hexdigit()) {
            self.note_bad(&peer, now);
            return not_found();
        }
        if !self.sender_ok(&peer, now) {
            return Answer { retry_after: Some(30), ..Answer::err(429, "rate_limited", "too many tries from this address") };
        }
        // 2. the token, in constant time (an unknown hook is compared with a dummy)
        let rec = self.store.get(rq.id);
        let want = rec.as_ref().map_or_else(|| token_hash("no such hook"), |r| r.token_hash.clone());
        let ok = same(&token_hash(&rq.token.to_ascii_lowercase()), &want);
        let Some(rec) = rec.filter(|_| ok) else {
            self.note_bad(&peer, now);
            return not_found();
        };
        let done = |a: Answer, outcome: &str, run: &str| -> Answer {
            self.store.record(&rec.id, Delivery { at: now, outcome: outcome.into(), http: a.status, run: run.into(), bytes: rq.body.len(), from: peer.clone() });
            if outcome != "accepted" && matches!(outcome, "bad_signature" | "stale" | "replay" | "owner_not_allowed" | "outside_network") {
                w.audit(&rec.owner, "hook.refused", &format!("hook={} flow={} outcome={outcome} addr={peer}", rec.id, rec.flow));
            }
            a
        };
        // 3. on, near, still allowed, still published
        if !rec.enabled {
            return done(not_found(), "off", "");
        }
        if !is_local(rq.peer) && rec.public_until <= now {
            return done(Answer::err(403, "denied", "this address only takes requests from the local network"), "outside_network", "");
        }
        if !w.owner_allowed(&rec.owner) {
            return done(Answer::err(403, "denied", "the owner of this webhook may not run flows"), "owner_not_allowed", "");
        }
        let Some(cfg) = w.config(&rec) else { return done(not_found(), "not_published", "") };
        // 4. method, size, rate, content type
        if !cfg.methods.iter().any(|m| m == rq.method) {
            let mut a = Answer::err(405, "invalid_input", "method not allowed");
            a.body["allow"] = json!(cfg.methods);
            return done(a, "wrong_method", "");
        }
        if rq.body.len() > cfg.max_body {
            return done(Answer::err(413, "too_large", "the body is too large"), "too_large", "");
        }
        if !self.rate_ok(&rec.id, cfg.rate_per_min, now) {
            return done(Answer { retry_after: Some(30), ..Answer::err(429, "rate_limited", "too many calls for this webhook") }, "rate_limited", "");
        }
        let kind = match content_kind(rq.content_type) {
            _ if rq.body.is_empty() => None,
            Some(k) if cfg.content.contains(&k) => Some(k),
            _ => return done(Answer::err(415, "invalid_input", "this webhook does not take that content type"), "bad_content_type", ""),
        };
        // 5. the signature
        let mut sig_key = None;
        if cfg.signed {
            let ts = rq.timestamp.and_then(|t| t.trim().parse::<i64>().ok());
            let sig = rq.signature.map(|s| s.trim().to_ascii_lowercase());
            let (Some(ts), Some(sig)) = (ts, sig.filter(|s| s.starts_with("sha256="))) else {
                self.note_bad(&peer, now);
                return done(Answer::err(401, "unauthorized", "send X-Iyakki-Timestamp (unix seconds) and X-Iyakki-Signature: sha256=<hex>"), "bad_signature", "");
            };
            if (now - ts).abs() > WINDOW_S {
                return done(Answer::err(401, "unauthorized", "the timestamp is too far from this computer's clock"), "stale", "");
            }
            let secret = match w.secret(&rec) {
                Ok(s) if !s.is_empty() => s,
                _ => return done(Answer { retry_after: Some(60), ..Answer::err(503, "unavailable", "the shared secret cannot be read now (the owner's vault is locked)") }, "vault_locked", ""),
            };
            if !same(&sign(&secret, ts, rq.body), &sig) {
                self.note_bad(&peer, now);
                return done(Answer::err(401, "unauthorized", "the signature does not match"), "bad_signature", "");
            }
            if !self.first_use(&rec.id, &sig, now) {
                return done(Answer::err(409, "conflict", "this signed request was already taken"), "replay", "");
            }
            sig_key = Some(sig);
        }
        // 6. the payload as data
        let payload = match parse_payload(kind, rq.body) {
            Ok(p) => p,
            Err(m) => return done(Answer::err(400, "invalid_input", m), "bad_payload", ""),
        };
        let mut inputs = if cfg.inputs.is_object() { cfg.inputs.clone() } else { json!({}) };
        inputs["webhook"] = json!({
            "method": rq.method,
            "content_type": kind.map_or("none", Content::label),
            "body": payload,
            "bytes": rq.body.len(),
            "received_at": now,
            "signed": cfg.signed,
            "untrusted": true,
        });
        let key = match (rq.idempotency_key.filter(|k| safe_key(k)), sig_key) {
            (Some(k), _) => Some(format!("hook-{}-{k}", rec.id)),
            (None, Some(s)) => Some(format!("hook-{}-{s}", rec.id)),
            (None, None) => None,
        };
        match w.start(&rec, &cfg, inputs, key) {
            Ok((run, existing)) => {
                w.audit(&rec.owner, "hook.fire", &format!("hook={} flow={} run={run} existing={existing} addr={peer}", rec.id, rec.flow));
                done(Answer { status: 202, body: json!({"run": run, "existing": existing}), retry_after: None }, "accepted", &run)
            }
            Err(_) => done(Answer::err(409, "conflict", "the flow could not be started now"), "not_started", ""),
        }
    }
}

fn safe_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
}

fn content_kind(ct: Option<&str>) -> Option<Content> {
    let base = ct?.split(';').next()?.trim().to_ascii_lowercase();
    match base.as_str() {
        "application/json" => Some(Content::Json),
        "application/x-www-form-urlencoded" => Some(Content::Form),
        "text/plain" => Some(Content::Text),
        b if b.ends_with("+json") => Some(Content::Json),
        _ => None,
    }
}

fn parse_payload(kind: Option<Content>, body: &[u8]) -> Result<Value, &'static str> {
    match kind {
        None => Ok(Value::Null),
        Some(Content::Json) => {
            let v: Value = serde_json::from_slice(body).map_err(|_| "the body is not valid JSON")?;
            if depth(&v) > 24 {
                return Err("the JSON is nested too deeply");
            }
            Ok(v)
        }
        Some(Content::Text) => std::str::from_utf8(body).map(|t| Value::String(t.to_string())).map_err(|_| "the text is not UTF-8"),
        Some(Content::Form) => {
            let text = std::str::from_utf8(body).map_err(|_| "the form is not UTF-8")?;
            let mut m = Map::new();
            for pair in text.split('&').filter(|p| !p.is_empty()).take(200) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let (k, v) = (pct_decode(k).ok_or("bad %-escape in the form")?, pct_decode(v).ok_or("bad %-escape in the form")?);
                match m.get_mut(&k) {
                    Some(Value::Array(a)) => a.push(Value::String(v)),
                    Some(old) => *old = json!([old.clone(), v]),
                    None => {
                        m.insert(k, Value::String(v));
                    }
                }
            }
            Ok(Value::Object(m))
        }
    }
}

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let h = b.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests;
