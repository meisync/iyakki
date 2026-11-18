use std::cell::RefCell;
use std::net::IpAddr;

use super::*;
use crate::flows::hookstore::{HookRec, HookStore};

const SECRET: &[u8] = b"correct horse battery staple 0123456789";
const NOW: i64 = 1_800_000_000;
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const LAN: &str = "192.168.1.20";
const WAN: &str = "203.0.113.9";

struct World {
    now: RefCell<i64>,
    cfg: RefCell<Option<HookCfg>>,
    allowed: RefCell<bool>,
    vault: RefCell<bool>,
    started: RefCell<Vec<(Value, Option<String>)>>,
    audit: RefCell<Vec<String>>,
}

impl World {
    fn new(cfg: HookCfg) -> World {
        World { now: RefCell::new(NOW), cfg: RefCell::new(Some(cfg)), allowed: RefCell::new(true), vault: RefCell::new(true), started: RefCell::new(vec![]), audit: RefCell::new(vec![]) }
    }
}

impl HookWorld for World {
    fn owner_allowed(&self, _: &str) -> bool {
        *self.allowed.borrow()
    }
    fn config(&self, _: &HookRec) -> Option<HookCfg> {
        self.cfg.borrow().clone()
    }
    fn secret(&self, _: &HookRec) -> Result<Vec<u8>, String> {
        if *self.vault.borrow() { Ok(SECRET.to_vec()) } else { Err("locked".into()) }
    }
    fn start(&self, _: &HookRec, _: &HookCfg, inputs: Value, key: Option<String>) -> Result<(String, bool), String> {
        self.started.borrow_mut().push((inputs, key));
        Ok((format!("r{}", self.started.borrow().len()), false))
    }
    fn audit(&self, owner: &str, event: &str, detail: &str) {
        self.audit.borrow_mut().push(format!("{owner} {event} {detail}"));
    }
    fn now(&self) -> i64 {
        *self.now.borrow()
    }
}

fn cfg(signed: bool) -> HookCfg {
    HookCfg { trigger: "hk-1".into(), methods: vec!["POST".into()], max_body: 1024, content: vec![Content::Json, Content::Form, Content::Text], rate_per_min: 600, signed, inputs: json!({"env": "staging"}) }
}

fn rig(signed: bool, enabled: bool) -> (Hooks, World, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-hooks-{}-{}", std::process::id(), crate::sharedfolder::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(HookStore::open(&dir.join("hooks.json")));
    store.insert(HookRec { id: "hk-1".into(), owner: "admin".into(), flow: "deploy".into(), trigger: "hk-1".into(), token_hash: token_hash(TOKEN), enabled, created: 1, rotated: 0, public_until: 0, has_secret: signed, deliveries: vec![] }).unwrap();
    (Hooks::new(store), World::new(cfg(signed)), dir)
}

struct Rq {
    method: &'static str,
    id: &'static str,
    token: String,
    ct: Option<&'static str>,
    ts: Option<String>,
    sig: Option<String>,
    key: Option<&'static str>,
    body: Vec<u8>,
    peer: &'static str,
}

impl Rq {
    fn new(body: &[u8]) -> Rq {
        Rq { method: "POST", id: "hk-1", token: TOKEN.into(), ct: Some("application/json"), ts: None, sig: None, key: None, body: body.to_vec(), peer: LAN }
    }
    fn signed(mut self, ts: i64) -> Rq {
        self.ts = Some(ts.to_string());
        self.sig = Some(sign(SECRET, ts, &self.body));
        self
    }
}

fn go(h: &Hooks, w: &World, r: &Rq) -> Answer {
    h.serve(w, &HookRequest { method: r.method, id: r.id, token: &r.token, content_type: r.ct, timestamp: r.ts.as_deref(), signature: r.sig.as_deref(), idempotency_key: r.key, body: &r.body, peer: r.peer.parse::<IpAddr>().unwrap() })
}

fn outcomes(h: &Hooks) -> Vec<String> {
    h.store.get("hk-1").unwrap().deliveries.iter().map(|d| d.outcome.clone()).collect()
}

#[test]
fn a_good_request_starts_the_flow_with_the_body_as_untrusted_data_and_answers_202() {
    let (h, w, _d) = rig(true, true);
    let body = br#"{"ref":"main","cmd":"rm -rf / {{secret.token}} $(id)"}"#;
    let a = go(&h, &w, &Rq::new(body).signed(NOW));
    assert_eq!((a.status, a.body["run"].as_str()), (202, Some("r1")));
    let (inputs, key) = w.started.borrow()[0].clone();
    assert_eq!(inputs["env"], "staging", "the owner's own inputs stay");
    let hook = &inputs["webhook"];
    assert_eq!((hook["method"].as_str(), hook["content_type"].as_str(), hook["untrusted"].as_bool(), hook["signed"].as_bool()), (Some("POST"), Some("json"), Some(true), Some(true)));
    assert_eq!(hook["body"]["cmd"], "rm -rf / {{secret.token}} $(id)", "kept as plain text, byte for byte");
    assert!(key.unwrap().starts_with("hook-hk-1-sha256="));
    assert_eq!(outcomes(&h), vec!["accepted"]);
    // the body is not under any other key: the payload cannot overwrite the owner's inputs
    let a = go(&h, &w, &Rq::new(br#"{"env":"prod"}"#).signed(NOW + 1));
    assert_eq!(a.status, 202);
    assert_eq!(w.started.borrow()[1].0["env"], "staging");
}

#[test]
fn form_and_text_bodies_become_typed_data_too() {
    let (h, w, _d) = rig(false, true);
    let mut r = Rq::new(b"a=1&b=x%20y+z&a=2");
    r.ct = Some("application/x-www-form-urlencoded; charset=utf-8");
    assert_eq!(go(&h, &w, &r).status, 202);
    assert_eq!(w.started.borrow()[0].0["webhook"]["body"], json!({"a": ["1", "2"], "b": "x y z"}));
    let mut t = Rq::new("héllo".as_bytes());
    t.ct = Some("text/plain");
    assert_eq!(go(&h, &w, &t).status, 202);
    assert_eq!(w.started.borrow()[1].0["webhook"]["body"], "héllo");
    let mut bad = Rq::new(b"a=%zz");
    bad.ct = Some("application/x-www-form-urlencoded");
    assert_eq!(go(&h, &w, &bad).status, 400);
    assert_eq!(go(&h, &w, &Rq::new(b"{not json")).status, 400);
    let mut xml = Rq::new(b"<a/>");
    xml.ct = Some("application/xml");
    assert_eq!(go(&h, &w, &xml).status, 415);
    assert_eq!(w.started.borrow().len(), 2);
}

#[test]
fn a_wrong_or_missing_signature_a_stale_clock_and_a_replay_start_nothing() {
    let (h, w, _d) = rig(true, true);
    let body = b"{}".to_vec();
    let mut bad = Rq::new(&body).signed(NOW);
    bad.sig = Some(sign(b"another secret entirely", NOW, &body));
    assert_eq!(go(&h, &w, &bad).status, 401);
    let mut other_body = Rq::new(b"{\"x\":1}").signed(NOW);
    other_body.body = b"{\"x\":2}".to_vec();
    assert_eq!(go(&h, &w, &other_body).status, 401, "a body changed after signing");
    assert_eq!(go(&h, &w, &Rq::new(&body)).status, 401, "unsigned");
    let mut mal = Rq::new(&body).signed(NOW);
    mal.sig = Some("md5=00".into());
    assert_eq!(go(&h, &w, &mal).status, 401);
    for skew in [301, -301] {
        assert_eq!(go(&h, &w, &Rq::new(&body).signed(NOW + skew)).status, 401, "skew {skew}");
    }
    assert_eq!(go(&h, &w, &Rq::new(&body).signed(NOW + 299)).status, 202);
    // the same signed request again is a replay, even from another address
    let mut again = Rq::new(&body).signed(NOW + 299);
    assert_eq!(go(&h, &w, &again).status, 409);
    again.peer = "10.0.0.7";
    assert_eq!(go(&h, &w, &again).status, 409);
    // the same body signed at another second is a new request
    assert_eq!(go(&h, &w, &Rq::new(&body).signed(NOW + 1)).status, 202);
    assert_eq!(w.started.borrow().len(), 2);
    assert_eq!(outcomes(&h), vec!["bad_signature", "bad_signature", "bad_signature", "bad_signature", "stale", "stale", "accepted", "replay", "replay", "accepted"]);
    // the audit line names the hook and the outcome and nothing else of the request
    let a = w.audit.borrow().join("\n");
    assert!(a.contains("hook.fire hook=hk-1 flow=deploy run=r1") && a.contains("hook.refused hook=hk-1 flow=deploy outcome=bad_signature"), "{a}");
    for secret in [TOKEN, "correct horse", "sha256=", "another secret"] {
        assert!(!a.contains(secret), "{secret} in the audit");
    }
}

#[test]
fn a_locked_vault_is_503_and_a_signed_request_is_not_taken_without_the_secret() {
    let (h, w, _d) = rig(true, true);
    *w.vault.borrow_mut() = false;
    let a = go(&h, &w, &Rq::new(b"{}").signed(NOW));
    assert_eq!((a.status, a.retry_after), (503, Some(60)));
    assert!(w.started.borrow().is_empty());
    assert_eq!(outcomes(&h), vec!["vault_locked"]);
}

#[test]
fn the_rate_limit_counts_calls_of_the_hook_and_opens_again_after_a_minute() {
    let (h, w, _d) = rig(false, true);
    *w.cfg.borrow_mut() = Some(HookCfg { rate_per_min: 5, ..cfg(false) });
    for i in 0..5 {
        assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 202, "{i}");
    }
    let a = go(&h, &w, &Rq::new(b"{}"));
    assert_eq!((a.status, a.retry_after), (429, Some(30)));
    assert_eq!(w.started.borrow().len(), 5);
    *w.now.borrow_mut() = NOW + 61;
    assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 202);
    assert_eq!(outcomes(&h).iter().filter(|o| *o == "rate_limited").count(), 1);
}

#[test]
fn body_caps_methods_and_the_local_network_rule_are_enforced() {
    let (h, w, _d) = rig(false, true);
    let big = vec![b' '; 1025];
    assert_eq!(go(&h, &w, &Rq::new(&big)).status, 413);
    assert_eq!(go(&h, &w, &Rq::new(&big[..1024])).status, 400, "1024 spaces fit but are not JSON");
    for m in ["GET", "PUT", "DELETE", "TRACE", "post"] {
        let mut r = Rq::new(b"{}");
        r.method = m;
        let a = go(&h, &w, &r);
        assert_eq!(a.status, 405, "{m}");
        assert_eq!(a.body["allow"], json!(["POST"]));
    }
    let mut wan = Rq::new(b"{}");
    wan.peer = WAN;
    assert_eq!(go(&h, &w, &wan).status, 403, "outside the local network");
    for local in ["127.0.0.1", "10.1.2.3", "172.16.0.9", "169.254.1.1", "::1", "fd00::5", "fe80::1", "::ffff:192.168.1.5"] {
        assert!(super::super::hookstore::is_local(local.parse().unwrap()), "{local}");
    }
    for far in [WAN, "8.8.8.8", "2001:db8::1", "::ffff:8.8.8.8", "172.32.0.1"] {
        assert!(!super::super::hookstore::is_local(far.parse().unwrap()), "{far}");
    }
    // the owner opens it to the public until a time: then it is taken, and after the time it is not
    h.store.update("hk-1", |r| r.public_until = NOW + 600).unwrap();
    assert_eq!(go(&h, &w, &wan).status, 202);
    *w.now.borrow_mut() = NOW + 601;
    assert_eq!(go(&h, &w, &wan).status, 403);
    assert!(w.started.borrow().len() == 1);
}

#[test]
fn a_hook_that_is_off_unpublished_or_whose_owner_lost_the_right_looks_unavailable() {
    let (h, w, _d) = rig(false, false);
    assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 404, "off by default");
    h.store.update("hk-1", |r| r.enabled = true).unwrap();
    *w.allowed.borrow_mut() = false;
    assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 403);
    *w.allowed.borrow_mut() = true;
    *w.cfg.borrow_mut() = None;
    assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 404);
    assert!(w.started.borrow().is_empty());
    assert_eq!(outcomes(&h), vec!["off", "owner_not_allowed", "not_published"]);
}

#[test]
fn bad_tokens_and_unknown_hooks_get_the_same_404_and_hammering_is_slowed() {
    let (h, w, _d) = rig(false, true);
    let mut wrong = Rq::new(b"{}");
    wrong.token = TOKEN.replace('0', "1");
    let a = go(&h, &w, &wrong);
    let mut unknown = Rq::new(b"{}");
    unknown.id = "hk-nope";
    let b = go(&h, &w, &unknown);
    let mut short = Rq::new(b"{}");
    short.token = "abc".into();
    let c = go(&h, &w, &short);
    assert_eq!((a.status, a.body.clone()), (404, b.body.clone()));
    assert_eq!((b.status, c.status), (404, 404));
    assert_eq!(a.body, c.body, "the same answer whichever part is wrong");
    // a token differing only in its last character is as wrong as any other
    let mut near = Rq::new(b"{}");
    near.token = format!("{}0", &TOKEN[..63]);
    assert_eq!(go(&h, &w, &near).status, 404);
    for _ in 0..BAD_PER_MIN {
        go(&h, &w, &wrong);
    }
    let slow = go(&h, &w, &wrong);
    assert_eq!((slow.status, slow.retry_after), (429, Some(30)));
    // the same sender is slowed for the real token too; another sender is not
    assert_eq!(go(&h, &w, &Rq::new(b"{}")).status, 429);
    let mut other = Rq::new(b"{}");
    other.peer = "192.168.1.99";
    assert_eq!(go(&h, &w, &other).status, 202);
    assert!(h.store.get("hk-1").unwrap().deliveries.iter().all(|d| d.outcome == "accepted"), "wrong tries are not written to the hook's deliveries");
    // the constant-time comparison itself
    assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
}

#[test]
fn only_the_last_twenty_deliveries_are_kept_and_the_file_holds_no_token_or_body() {
    let (h, w, dir) = rig(false, true);
        for i in 0..30 {
        *w.now.borrow_mut() = NOW + i;
        assert_eq!(go(&h, &w, &Rq::new(format!("{{\"secretbody\":{i}}}").as_bytes())).status, 202);
    }
    let rec = h.store.get("hk-1").unwrap();
    assert_eq!(rec.deliveries.len(), 20);
    assert_eq!(rec.deliveries[0].at, NOW + 10);
    let file = std::fs::read_to_string(dir.join("hooks.json")).unwrap();
    assert!(!file.contains(TOKEN) && !file.contains("secretbody") && file.contains(&token_hash(TOKEN)));
    // a restart reads the same
    let again = HookStore::open(&dir.join("hooks.json"));
    assert_eq!(again.get("hk-1").unwrap().deliveries.len(), 20);
}

#[test]
fn trigger_settings_get_their_defaults_and_caps() {
    let mut t = Trigger::new("hk-1", TriggerKind::webhook("hk-1"));
    let c = HookCfg::from_trigger(&t).unwrap();
    assert_eq!((c.methods, c.max_body, c.content, c.rate_per_min, c.signed), (vec!["POST".to_string()], DEFAULT_MAX_BODY, vec![Content::Json], DEFAULT_PER_MIN, false));
    t.kind = TriggerKind::Webhook { path: "hk-1".into(), methods: vec!["GET".into()], max_body: 10_000_000, content_types: vec!["form".into(), "yaml".into()], rate_per_min: 3, signed: true };
    let c = HookCfg::from_trigger(&t).unwrap();
    assert_eq!((c.methods, c.max_body, c.content, c.rate_per_min, c.signed), (vec!["GET".to_string()], HARD_MAX_BODY, vec![Content::Form], 3, true));
    t.enabled = false;
    assert!(HookCfg::from_trigger(&t).is_none());
    assert!(HookCfg::from_trigger(&Trigger::new("m", TriggerKind::Manual)).is_none());
}

#[test]
fn a_very_deep_json_body_is_refused_without_overflowing() {
    let (h, w, _d) = rig(false, true);
    let body = format!("{}{}", "[".repeat(900), "]".repeat(900));
    let mut r = Rq::new(body.as_bytes());
    *w.cfg.borrow_mut() = Some(HookCfg { max_body: 4096, ..cfg(false) });
    r.ct = Some("application/json");
    assert_eq!(go(&h, &w, &r).status, 400);
    let ok = format!("{}{}", "[".repeat(10), "]".repeat(10));
    assert_eq!(go(&h, &w, &Rq::new(ok.as_bytes())).status, 202);
}
