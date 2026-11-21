//! The webhook operations of the cloud profile's API over a real flow environment: the person makes, publishes, switches on and opens a hook and
//! gets the address once; another person sees and touches nothing of it.

use std::sync::Arc;

use iyakki_core_fake::FakeAccounts;
use iyakki_sec::policy::Role;
use serde_json::{json, Value};

use super::hookapi::{run, HookOp};
use super::hookenv::HookOpts;
use super::hookstore::{stash_reveal, Reveal};
use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn env_for(dir: &std::path::Path, svc: &Arc<Services>, user: &str) -> FlowEnv {
    FlowEnv::build_in(svc, user, &Role::User.template(), false, &dir.join(format!("home-{user}")))
}

fn setup(tag: &str) -> (PathBuf, Arc<Services>) {
    let dir = std::env::temp_dir().join(format!("iyakki-hookapi-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    (dir, svc)
}

fn flow(env: &FlowEnv) -> String {
    let f = build_flow(&json!({"name": "Hook target", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "hi"}}]})).unwrap();
    env.create_draft(f, "user:test").unwrap().id
}

#[test]
fn a_person_makes_publishes_enables_and_opens_a_hook_through_the_api_and_gets_the_address_once() {
    let (dir, svc) = setup("own");
    let _ = FakeAccounts::new(&[("hookapiann", "pw-1234567890", Role::User)]);
    let env = env_for(&dir, &svc, "hookapiann");
    let id = flow(&env);
    let made = run(&env, HookOp::Create { flow: &id, opts: HookOpts { signed: false, ..HookOpts::default() } }).unwrap();
    let hook = made["id"].as_str().unwrap().to_string();
    let path = made["path"].as_str().unwrap();
    assert!(path.starts_with(&format!("/hooks/{hook}/")) && path.len() == format!("/hooks/{hook}/").len() + 64, "{path}");
    assert_eq!((made["shown_once"].as_bool(), made["secret"].is_null()), (Some(true), true), "unsigned: no shared secret");
    // the list never has the address or a secret, and the hook is off and not published
    let listed = run(&env, HookOp::List(None)).unwrap();
    let text = listed.to_string();
    assert!(!text.contains("/hooks/") && !text.contains(&path[path.len() - 64..]), "{text}");
    assert_eq!((listed["hooks"][0]["enabled"].as_bool(), listed["hooks"][0]["published"].as_bool()), (Some(false), Some(false)));
    assert!(run(&env, HookOp::Enable(&hook)).unwrap_err().contains("publish the flow first"));
    env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:test", label: "", changelog: "x", approved_by: Some("user:test") }).unwrap();
    assert_eq!(run(&env, HookOp::Enable(&hook)).unwrap()["enabled"], true);
    let open = run(&env, HookOp::Public(&hook, 5)).unwrap();
    assert_eq!(open["open_to_public"], true);
    // never longer than seven days
    let long = run(&env, HookOp::Public(&hook, 100_000_000)).unwrap()["public_until"].as_i64().unwrap();
    assert!(long - crate::clock::now_unix() <= super::hookstore::PUBLIC_MAX_S);
    assert_eq!(run(&env, HookOp::Public(&hook, 0)).unwrap()["open_to_public"], false);
    // a new address: the old one is gone, the new one comes back once
    let again = run(&env, HookOp::Regenerate(&hook)).unwrap();
    assert_ne!(again["path"], made["path"]);
    assert_eq!(run(&env, HookOp::Disable(&hook)).unwrap()["enabled"], false);
    assert_eq!(run(&env, HookOp::List(Some("nope"))).unwrap()["hooks"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn another_person_cannot_list_renew_switch_or_open_a_hook_that_is_not_theirs() {
    let (dir, svc) = setup("other");
    let mine = env_for(&dir, &svc, "hookapiown");
    let theirs = env_for(&dir, &svc, "hookapibob");
    let id = flow(&mine);
    let hook = run(&mine, HookOp::Create { flow: &id, opts: HookOpts::default() }).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(run(&theirs, HookOp::List(None)).unwrap()["hooks"], json!([]));
    for op in [HookOp::Regenerate(&hook), HookOp::Enable(&hook), HookOp::Disable(&hook), HookOp::Public(&hook, 5)] {
        let e = run(&theirs, op).unwrap_err();
        assert!(e.contains("there is no webhook"), "{e}");
    }
    assert!(run(&theirs, HookOp::Create { flow: &id, opts: HookOpts::default() }).unwrap_err().contains("there is no flow"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_address_a_chat_tool_left_waits_for_the_person_and_is_taken_once() {
    let (dir, svc) = setup("reveal");
    let env = env_for(&dir, &svc, "hookapirev");
    assert_eq!(run(&env, HookOp::Reveal).unwrap()["waiting"], false);
    stash_reveal("hookapirev", Reveal { hook: "hk-1".into(), flow: "f".into(), path: "/hooks/hk-1/tok".into(), secret: Some("s".into()) });
    let v: Value = run(&env, HookOp::Reveal).unwrap();
    assert_eq!((v["waiting"].as_bool(), v["path"].as_str(), v["secret"].as_str()), (Some(true), Some("/hooks/hk-1/tok"), Some("s")));
    assert_eq!(run(&env, HookOp::Reveal).unwrap()["waiting"], false, "once");
    // somebody else's reveal is not handed over
    stash_reveal("hookapirev", Reveal { hook: "hk-2".into(), flow: "f".into(), path: "/hooks/hk-2/t".into(), secret: None });
    let other = env_for(&dir, &svc, "hookapiother");
    assert_eq!(run(&other, HookOp::Reveal).unwrap()["waiting"], false);
    let _ = std::fs::remove_dir_all(&dir);
}
