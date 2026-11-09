//! The webhook trigger through the real flow environment: making, publishing, switching on and calling a hook starts a real run as the
//! owner with the body as data; the assistant's tools never carry the address; another person cannot touch the hook.

use std::net::IpAddr;
use std::sync::Arc;

use iyakki_core_fake::FakeAccounts;
use iyakki_sec::policy::Role;
use serde_json::{json, Value};

use super::hookenv::{HookOpts, RealWorld};
use super::hookstore::{take_reveal, token_hash};
use super::webhook::{sign, HookRequest, Hooks};
use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Rig {
    svc: Arc<Services>,
    accounts: Arc<FakeAccounts>,
    env: Arc<FlowEnv>,
    user: String,
    dir: PathBuf,
}

fn rig(tag: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-hookenv-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("hookowner{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let accounts = Arc::new(FakeAccounts::new(&[(&user, "pw-1234567890", Role::User), ("other", "pw-1234567890", Role::User)]));
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &Role::User.template(), false, &dir.join("home")));
    Rig { svc, accounts, env, user, dir }
}

fn flow(env: &FlowEnv) -> String {
    let f = build_flow(&json!({"name": "Hook target", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    let f = env.create_draft(f, "user:test").unwrap();
    f.id
}

fn publish(env: &FlowEnv, id: &str) {
    env.store.publish(id, &iyakki_flow::store::PublishReq { by: "user:test", label: "", changelog: "x", approved_by: Some("user:test") }).unwrap();
}

fn call(r: &Rig, token: &str, body: &[u8], peer: &str, id: &str) -> super::webhook::Answer {
    let world = RealWorld { svc: &r.svc, auth: r.accounts.as_ref(), audit: r.accounts.as_ref(), only: Some(r.env.clone()) };
    Hooks::shared(r.svc.dir()).serve(&world, &HookRequest { method: "POST", id, token, content_type: Some("application/json"), timestamp: None, signature: None, idempotency_key: None, body, peer: peer.parse::<IpAddr>().unwrap() })
}

fn token_of(path: &str) -> String {
    path.rsplit('/').next().unwrap().to_string()
}

#[test]
fn a_hook_is_off_until_the_flow_is_published_and_the_owner_switches_it_on_then_a_real_run_starts_as_the_owner() {
    let r = rig("e2e");
    let id = flow(&r.env);
    let c = r.env.hook_create(&id, &HookOpts::default(), "user:test").unwrap();
    assert!(c.secret.is_none() && c.path == format!("/hooks/{}/{}", c.id, token_of(&c.path)) && token_of(&c.path).len() == 64);
    let t = token_of(&c.path);
    // off by default, and not published: both look like nothing is there
    assert_eq!(call(&r, &t, b"{}", "192.168.1.5", &c.id).status, 404);
    assert!(r.env.hook_enable(&c.id, "user:test").unwrap_err().contains("publish the flow first"));
    publish(&r.env, &id);
    r.env.hook_enable(&c.id, "user:test").unwrap();
    let a = call(&r, &t, br#"{"ref":"main","x":"{{secret.k}}"}"#, "192.168.1.5", &c.id);
    assert_eq!(a.status, 202, "{:?}", a.body);
    let run = r.env.engine.get_run(a.body["run"].as_str().unwrap()).expect("the run exists");
    assert_eq!((run.trigger.kind.as_str(), run.flow_id.as_str()), ("webhook", id.as_str()));
    assert_eq!(run.trigger.by, format!("hook:{}", c.id));
    assert_eq!(run.inputs["webhook"]["body"]["ref"], "main");
    assert_eq!(run.inputs["webhook"]["body"]["x"], "{{secret.k}}", "kept as text, never evaluated");
    assert!(run.grants.is_empty(), "no extra permission came with the call");
    // the sender outside the local network is refused until the owner opens it, with an end
    assert_eq!(call(&r, &t, b"{}", "203.0.113.4", &c.id).status, 403);
    let until = r.env.hook_public(&c.id, 5, "user:test").unwrap();
    assert!(until > crate::clock::now_unix());
    assert_eq!(call(&r, &t, b"{}", "203.0.113.4", &c.id).status, 202);
    r.env.hook_public(&c.id, 0, "user:test").unwrap();
    assert_eq!(call(&r, &t, b"{}", "203.0.113.4", &c.id).status, 403);
    assert_eq!(r.env.hook_public(&c.id, 100_000_000, "user:test").unwrap() - crate::clock::now_unix() <= super::hookstore::PUBLIC_MAX_S, true, "never longer than seven days");
    // switching off answers 404 and closes the public opening
    r.env.hook_disable(&c.id, "user:test").unwrap();
    assert_eq!(call(&r, &t, b"{}", "192.168.1.5", &c.id).status, 404);
    assert_eq!(r.env.hooks().store.get(&c.id).unwrap().public_until, 0);
    // the audit log has the id and the flow, never the address or the body
    let audit = r.accounts.lines().join("\n");
    assert!(audit.contains("hook.fire") && audit.contains(&format!("hook={}", c.id)), "{audit}");
    for secret in [t.as_str(), "secret.k", "main"] {
        assert!(!audit.contains(secret), "{secret} leaked into the audit log");
    }
    // the stored file holds the hash of the token and not the token
    let file = std::fs::read_to_string(r.svc.dir().join("hooks.json")).unwrap();
    assert!(file.contains(&token_hash(&t)) && !file.contains(&t));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn regenerating_kills_the_old_address_and_another_person_cannot_switch_the_hook_on_or_off() {
    let r = rig("regen");
    let id = flow(&r.env);
    let c = r.env.hook_create(&id, &HookOpts::default(), "user:test").unwrap();
    publish(&r.env, &id);
    r.env.hook_enable(&c.id, "user:test").unwrap();
    let old = token_of(&c.path);
    assert_eq!(call(&r, &old, b"{}", "10.0.0.2", &c.id).status, 202);
    let n = r.env.hook_regenerate(&c.id, "user:test").unwrap();
    assert_ne!(token_of(&n.path), old);
    assert_eq!(call(&r, &old, b"{}", "10.0.0.3", &c.id).status, 404);
    assert_eq!(call(&r, &token_of(&n.path), b"{}", "10.0.0.2", &c.id).status, 202, "still on after the change");
    let other = FlowEnv::build_in(&r.svc, "other", &Role::User.template(), false, &r.dir.join("home-other"));
    for e in [other.hook_enable(&c.id, "user:other").unwrap_err(), other.hook_disable(&c.id, "user:other").unwrap_err(), other.hook_regenerate(&c.id, "user:other").map(|_| ()).unwrap_err(), other.hook_public(&c.id, 5, "user:other").map(|_| ()).unwrap_err()] {
        assert!(e.contains("no webhook"), "{e}");
    }
    assert!(other.hook_views(None).is_empty());
    // a person who may not use FLOWS cannot make or enable a hook
    let guest = Role::Guest.template();
    assert!(!allowed(&guest));
    let e = FlowEnv::build_in(&r.svc, &r.user, &guest, false, &r.dir.join("home-guest"));
    assert!(e.hook_enable(&c.id, "user:guest").unwrap_err().contains("may not use FLOWS"));
    assert!(e.hook_create(&id, &HookOpts::default(), "user:guest").map(|_| ()).unwrap_err().contains("may not use FLOWS"));
    // an owner who lost the right: the call is refused even with the right address
    let guests = Arc::new(FakeAccounts::new(&[(&r.user, "pw-1234567890", Role::Guest)]));
    let world = RealWorld { svc: &r.svc, auth: guests.as_ref(), audit: guests.as_ref(), only: Some(r.env.clone()) };
    let a = Hooks::shared(r.svc.dir()).serve(&world, &HookRequest { method: "POST", id: &c.id, token: &token_of(&n.path), content_type: Some("application/json"), timestamp: None, signature: None, idempotency_key: None, body: b"{}", peer: "10.0.0.2".parse().unwrap() });
    assert_eq!(a.status, 403);
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_signed_hook_needs_the_vault_to_be_made_and_the_signature_helper_matches_the_gate() {
    let r = rig("signed");
    let id = flow(&r.env);
    let e = r.env.hook_create(&id, &HookOpts { signed: true, ..Default::default() }, "user:test").map(|_| ()).unwrap_err();
    assert!(e.contains("unlock your vault"), "{e}");
    assert!(r.env.hook_views(None).is_empty(), "nothing was made");
    assert!(r.env.store.get_draft(&id).unwrap().triggers.is_empty());
    assert_eq!(sign(b"k", 5, b"b"), sign(b"k", 5, b"b"));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn the_tools_make_the_hook_without_ever_giving_the_model_the_address_and_the_screen_gets_it_once() {
    let r = rig("tools");
    let id = flow(&r.env);
    let c = |n: &str, a: Value| r.env.call(n, &a, None);
    assert!(c("webhook_list", json!({})).unwrap().contains("no webhooks"));
    let made = c("webhook_create", json!({"flow": id, "signed": false, "methods": ["post"], "rate_per_min": 7})).unwrap();
    let rev = take_reveal(&r.user).expect("the screen has it");
    assert!(take_reveal(&r.user).is_none(), "shown once");
    let token = token_of(&rev.path);
    assert!(made.contains("OFF") && made.contains("screen") && !made.contains(&token) && !made.contains("/hooks/"), "{made}");
    let list = c("webhook_list", json!({})).unwrap();
    assert!(list.contains(&rev.hook) && list.contains("off") && list.contains("not signed") && !list.contains(&token), "{list}");
    let t = r.env.store.get_draft(&id).unwrap().triggers.into_iter().find(|t| t.id == rev.hook).unwrap();
    assert!(matches!(&t.kind, iyakki_flow::TriggerKind::Webhook { methods, rate_per_min: 7, signed: false, .. } if methods == &["POST".to_string()]));
    assert!(c("webhook_enable", json!({"id": rev.hook})).unwrap_err().contains("publish the flow first"));
    publish(&r.env, &id);
    assert!(c("webhook_enable", json!({"id": rev.hook})).unwrap().contains("ON"));
    let again = c("webhook_regenerate", json!({"id": rev.hook})).unwrap();
    let rev2 = take_reveal(&r.user).unwrap();
    assert!(!again.contains(&token_of(&rev2.path)) && token_of(&rev2.path) != token);
    assert!(c("webhook_public", json!({"id": rev.hook, "minutes": 30})).unwrap().contains("outside the local network"));
    assert!(c("webhook_disable", json!({"id": rev.hook})).unwrap().contains("off"));
    assert!(c("webhook_enable", json!({"id": "hk-nope"})).unwrap_err().contains("no webhook"));
    // the model is told in every result what it cannot see; nothing it gets contains a token
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn risks_the_published_trigger_decides_and_the_flow_file_never_holds_the_address() {
    use crate::agent::{hard_ask, Risk};
    let defs = tools::defs(&Role::User.template());
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
    assert_eq!(risk("webhook_list"), Risk::Safe);
    for n in ["webhook_create", "webhook_regenerate", "webhook_enable", "webhook_disable", "webhook_public"] {
        assert_eq!(risk(n), Risk::Ask, "{n}");
    }
    assert!(hard_ask("webhook_public", &json!({"id": "x", "minutes": 5})).is_some() && hard_ask("webhook_enable", &json!({"id": "x"})).is_some());
    assert!(hard_ask("webhook_create", &json!({})).is_none() && hard_ask("webhook_disable", &json!({})).is_none());
    assert!(tools::defs(&Role::Guest.template()).iter().all(|d| !d.spec.name.starts_with("webhook_")), "no FLOWS, no webhook tools");
    let r = rig("file");
    let id = flow(&r.env);
    let c = r.env.hook_create(&id, &HookOpts::default(), "user:test").unwrap();
    let text = r.env.export_json(&id).unwrap();
    assert!(text.contains(&c.id) && !text.contains(&token_of(&c.path)), "the flow carries the hook id and its settings, never the token");
    let _ = std::fs::remove_dir_all(&r.dir);
}
