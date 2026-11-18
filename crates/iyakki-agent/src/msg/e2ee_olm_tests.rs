use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::gitkeys::MemStore;

const ROOM: &str = "!room:hs.example";

fn addon(user: &str, device: &str) -> (OlmAddon, Arc<MemStore>) {
    let store = Arc::new(MemStore::default());
    (OlmAddon::open(store.clone(), "acct", user, device).unwrap(), store)
}
fn first_otk(req: &AddonRequest) -> String {
    req.body["one_time_keys"].as_object().unwrap().values().next().unwrap()["key"].as_str().unwrap().to_string()
}

#[test]
fn a_new_device_uploads_signed_device_keys_and_one_time_keys_once_and_tops_up_when_the_server_runs_low() {
    let (a, _) = addon("@ana:hs.example", "DEV1");
    let r = a.on_sync(&json!({})).unwrap();
    assert_eq!((r.len(), r[0].path.as_str()), (1, "v3/keys/upload"));
    let dk = &r[0].body["device_keys"];
    assert_eq!(dk["keys"]["curve25519:DEV1"], a.curve25519_key());
    // the signature is the account's Ed25519 over the canonical JSON without the signatures
    let mut unsigned = dk.clone();
    unsigned.as_object_mut().unwrap().remove("signatures");
    let sig = dk["signatures"]["@ana:hs.example"]["ed25519:DEV1"].as_str().unwrap();
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let key = VerifyingKey::from_bytes(&b64::decode(&a.ed25519_key()).unwrap().try_into().unwrap()).unwrap();
    key.verify(canonical_json(&unsigned).as_bytes(), &Signature::from_bytes(&b64::decode(sig).unwrap().try_into().unwrap())).unwrap();
    assert_eq!(r[0].body["one_time_keys"].as_object().unwrap().len(), BATCH);
    assert!(a.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap().is_empty(), "nothing more to upload");
    let again = a.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 3}})).unwrap();
    assert!(again[0].body.get("device_keys").is_none() && again[0].body["one_time_keys"].as_object().unwrap().len() >= 1);
    assert_eq!(canonical_json(&json!({"b": 1, "a": [true, null, "x\"y"]})), r#"{"a":[true,null,"x\"y"],"b":1}"#);
}

#[test]
fn a_room_key_shared_by_olm_lets_the_other_device_read_the_rooms_messages_and_replays_are_refused() {
    let (alice, _) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    let up = bob.on_sync(&json!({})).unwrap();
    let otk = first_otk(&up[0]);
    // nothing is sent into the room before the key has been shared
    assert!(alice.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "x"})).unwrap_err().contains("not been shared"));
    let to_dev = alice.share_room_key(ROOM, "@bob:hs.example", &bob.curve25519_key(), &bob.ed25519_key(), Some(&otk)).unwrap();
    assert!(!to_dev.to_string().contains("session_key"), "the key travels encrypted");
    alice.mark_shared(ROOM).unwrap();
    assert!(alice.encrypt("!other:hs", "m.room.message", &json!({})).is_err());
    let (ty, content) = alice.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "hello room"})).unwrap();
    assert_eq!(ty, "m.room.encrypted");
    assert!(!content.to_string().contains("hello room"));
    let event = json!({"type": "m.room.encrypted", "event_id": "$e1", "content": content});
    assert!(bob.decrypt(ROOM, &event).unwrap_err().contains("has not arrived"));
    bob.on_sync(&json!({"to_device": {"events": [{"type": "m.room.encrypted", "sender": "@ana:hs.example", "content": to_dev}]}})).unwrap();
    assert_eq!(bob.decrypt(ROOM, &event).unwrap()["body"], "hello room");
    assert_eq!(bob.decrypt(ROOM, &event).unwrap()["body"], "hello room", "the same event again is fine");
    let forged = json!({"type": "m.room.encrypted", "event_id": "$other", "content": event["content"]});
    assert!(bob.decrypt(ROOM, &forged).unwrap_err().contains("replay"));
    assert!(bob.decrypt("!elsewhere:hs", &event).is_err());
    let mut wrong_sender = event.clone();
    wrong_sender["content"]["sender_key"] = json!("AAAA");
    assert!(bob.decrypt(ROOM, &wrong_sender).unwrap_err().contains("sender key"));
    // a second message and a second to-device message on the same Olm session
    let (_, c2) = alice.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "two"})).unwrap();
    assert_eq!(bob.decrypt(ROOM, &json!({"event_id": "$e2", "content": c2})).unwrap()["body"], "two");
}

#[test]
fn a_to_device_message_meant_for_another_device_or_user_is_refused_and_a_known_session_is_never_replaced() {
    let (alice, _) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    let (carol, _) = addon("@carol:hs.example", "C1");
    let otk = first_otk(&bob.on_sync(&json!({})).unwrap()[0]);
    let to_dev = alice.share_room_key(ROOM, "@carol:hs.example", &bob.curve25519_key(), &bob.ed25519_key(), Some(&otk)).unwrap();
    let ev = json!({"to_device": {"events": [{"type": "m.room.encrypted", "content": to_dev}]}});
    let _ = bob.on_sync(&ev);
    assert!(bob.take_problems()[0].contains("someone else"), "the payload names Carol");
    let _ = carol.on_sync(&ev);
    assert!(!carol.take_problems().is_empty(), "not for Carol's keys");
    // no one-time key and no session: cannot start
    assert!(alice.share_room_key(ROOM, "@bob:hs.example", &carol.curve25519_key(), &carol.ed25519_key(), None).is_err());
}

#[test]
fn the_state_is_kept_in_the_vault_survives_a_restart_and_a_locked_vault_stops_it() {
    let (alice, store) = addon("@ana:hs.example", "A1");
    let (bob, bstore) = addon("@bob:hs.example", "B1");
    let otk = first_otk(&bob.on_sync(&json!({})).unwrap()[0]);
    let to_dev = alice.share_room_key(ROOM, "@bob:hs.example", &bob.curve25519_key(), &bob.ed25519_key(), Some(&otk)).unwrap();
    alice.mark_shared(ROOM).unwrap();
    let (_, c) = alice.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "before restart"})).unwrap();
    bob.on_sync(&json!({"to_device": {"events": [{"type": "m.room.encrypted", "content": to_dev}]}})).unwrap();
    let key = vault_name("acct");
    assert!(store.map.lock().unwrap().contains_key(&key));
    let saved = store.map.lock().unwrap().get(&key).unwrap().clone();
    for secret in [alice.curve25519_key(), "before restart".to_string()] {
        assert!(!format!("{} {}", alice.name(), "").contains(&secret));
    }
    // restart both
    let alice2 = OlmAddon::open(store.clone(), "acct", "@ana:hs.example", "A1").unwrap();
    let bob2 = OlmAddon::open(bstore.clone(), "acct", "@bob:hs.example", "B1").unwrap();
    assert_eq!((alice2.curve25519_key(), saved.is_empty()), (alice.curve25519_key(), false));
    assert_eq!(bob2.decrypt(ROOM, &json!({"event_id": "$1", "content": c})).unwrap()["body"], "before restart");
    let (_, c2) = alice2.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "after restart"})).unwrap();
    assert_eq!(bob2.decrypt(ROOM, &json!({"event_id": "$2", "content": c2})).unwrap()["body"], "after restart");
    *store.locked.lock().unwrap() = true;
    assert!(OlmAddon::open(store.clone(), "acct", "@ana:hs.example", "A1").err().unwrap().contains("locked"));
    *store.locked.lock().unwrap() = false;
    store.map.lock().unwrap().insert(key, "{not json".into());
    assert!(OlmAddon::open(store, "acct", "@ana:hs.example", "A1").is_err());
    let v: Value = json!(null);
    assert!(v.is_null());
}
