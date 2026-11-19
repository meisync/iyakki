//! The `notify` node's channels: the INBOX note is always made, `matrix` asks the host to also tell the person, a channel that cannot
//! be reached is said plainly and does not fail the run, an unknown channel is a validation error, and an old node behaves as before.

mod common;

use common::*;
use iyakki_flow::*;
use serde_json::{json, Value};

fn one(cfg: Value) -> Flow {
    let mut f = Flow::new("t", "T");
    f.nodes.push(Node::new("s", "start", json!({})));
    f.nodes.push(Node::new("n", "notify", cfg));
    f.edges.push(Edge::new("s", "n"));
    f
}

fn test_node(e: &Env, f: &Flow) -> iyakki_flow::engine::NodeTest {
    e.engine.test_node(f, "n", json!({}), Value::Null).unwrap()
}

#[test]
fn an_old_notify_node_makes_a_note_and_asks_no_channel() {
    let e = env("nc-old");
    let t = test_node(&e, &one(json!({"title": "Backup done", "body": "ok"})));
    assert!(t.ok);
    assert_eq!(e.host.notices.lock().unwrap().len(), 1);
    assert!(e.host.channel_calls.lock().unwrap().is_empty());
    assert!(t.output.get("channels").is_none());
}

#[test]
fn matrix_is_asked_with_the_room_the_picture_and_a_key_per_run_and_node() {
    let e = env("nc-matrix");
    *e.host.channel_result.lock().unwrap() = Some(Ok(vec!["accepted by the homeserver for the room \"main\"".into()]));
    let t = test_node(&e, &one(json!({"title": "Possible person", "body": "at the door", "channels": "inbox, Matrix", "room": "home", "attach": "/data/camera/r1/p.jpg"})));
    assert!(t.ok, "{:?}", t.error);
    assert_eq!(e.host.notices.lock().unwrap().len(), 1, "the INBOX note is always made");
    let c = e.host.channel_calls.lock().unwrap()[0].clone();
    assert_eq!((c.channels.as_slice(), c.room.as_str(), c.attach.as_str(), c.dry_run), (&["matrix".to_string()][..], "home", "/data/camera/r1/p.jpg", false));
    assert!(c.dedupe_key.starts_with("t:") && c.dedupe_key.ends_with(":n"), "{}", c.dedupe_key);
    assert_eq!(t.output["channels"]["ok"], true);
    let d = test_node(&e, &one(json!({"title": "x", "channels": "matrix", "dry_run": true})));
    assert!(d.ok);
    assert!(e.host.channel_calls.lock().unwrap().last().unwrap().dry_run);
}

#[test]
fn a_channel_that_cannot_be_reached_is_said_plainly_and_the_run_goes_on() {
    let e = env("nc-fail");
    *e.host.channel_result.lock().unwrap() = Some(Err("this room is end-to-end encrypted and IYAKKI does not send plain text into an encrypted room".into()));
    let t = test_node(&e, &one(json!({"title": "Alert", "channels": "matrix"})));
    assert!(t.ok, "a message that cannot be sent does not fail the flow");
    assert_eq!(t.output["channels"]["ok"], false);
    assert!(t.output["channels"]["error"].as_str().unwrap().contains("end-to-end encrypted"));
    assert_eq!(e.host.notices.lock().unwrap().len(), 1);
    // no account at all: the default host answer
    let e2 = env("nc-none");
    let t = test_node(&e2, &one(json!({"title": "Alert", "channels": "matrix"})));
    assert!(t.ok && t.output["channels"]["error"].as_str().unwrap().contains("no messaging account"));
}

#[test]
fn only_known_channels_validate() {
    let reg = Registry::builtin();
    assert!(!validate::has_errors(&validate::validate_flow(&one(json!({"title": "x", "channels": "inbox, matrix"})), &reg)));
    assert!(validate::has_errors(&validate::validate_flow(&one(json!({"title": "x", "channels": "sms"})), &reg)));
    let k = reg.get("notify").unwrap();
    let keys: Vec<&str> = k.schema().iter().map(|f| f.key).collect();
    assert!(keys.contains(&"channels") && keys.contains(&"room") && keys.contains(&"attach") && keys.contains(&"dry_run"));
}
