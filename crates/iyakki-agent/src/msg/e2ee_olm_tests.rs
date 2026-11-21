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

/// What a homeserver would answer to `/keys/query` for these uploads.
fn query_answer(devs: &[(&str, &str, &Value)]) -> Value {
    let mut m = serde_json::Map::new();
    for (user, dev, dk) in devs {
        m.entry(user.to_string()).or_insert_with(|| json!({}))[*dev] = (*dk).clone();
    }
    json!({"device_keys": Value::Object(m)})
}

fn claim_answer(user: &str, dev: &str, upload: &AddonRequest) -> Value {
    let (id, k) = upload.body["one_time_keys"].as_object().unwrap().iter().next().unwrap();
    json!({"one_time_keys": {user: {dev: {id: k}}}})
}

#[test]
fn the_room_key_goes_to_every_device_through_query_claim_and_send_to_device_and_then_the_room_can_talk() {
    let (ana, _) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    let up = bob.on_sync(&json!({})).unwrap();
    let members = vec!["@ana:hs.example".to_string(), "@bob:hs.example".to_string()];
    // step 1: look up the members' devices
    let q = ana.prepare_send(ROOM, &members).unwrap();
    assert_eq!((q.len(), q[0].path.as_str()), (1, "v3/keys/query"));
    assert!(q[0].body["device_keys"].as_object().unwrap().contains_key("@bob:hs.example"));
    // step 2: one-time keys of the devices without a session
    let c = ana.on_response(&q[0], 200, &query_answer(&[("@bob:hs.example", "B1", &up[0].body["device_keys"])])).unwrap();
    assert_eq!((c.len(), c[0].path.as_str()), (1, "v3/keys/claim"));
    assert_eq!(c[0].body["one_time_keys"]["@bob:hs.example"]["B1"], "signed_curve25519");
    // step 3: the key, Olm-encrypted, to bob's device
    let send = ana.on_response(&c[0], 200, &claim_answer("@bob:hs.example", "B1", &up[0])).unwrap();
    assert_eq!(send[0].method, "PUT");
    assert!(send[0].path.starts_with("v3/sendToDevice/m.room.encrypted/") && !send[0].body.to_string().contains("session_key"));
    assert!(ana.encrypt(ROOM, "m.room.message", &json!({"body": "x"})).is_err(), "not shared until the homeserver took it");
    assert!(ana.on_response(&send[0], 200, &json!({})).unwrap().is_empty());
    let (_, enc) = ana.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "hi bob"})).unwrap();
    let to_dev = send[0].body["messages"]["@bob:hs.example"]["B1"].clone();
    bob.on_sync(&json!({"to_device": {"events": [{"type": "m.room.encrypted", "sender": "@ana:hs.example", "content": to_dev}]}})).unwrap();
    assert_eq!(bob.decrypt(ROOM, &json!({"event_id": "$1", "content": enc})).unwrap()["body"], "hi bob");
    // nothing more to do for the same members; a new member turns the session over
    assert!(ana.prepare_send(ROOM, &members).unwrap().is_empty());
    let mut more = members.clone();
    more.push("@cy:hs.example".into());
    assert_eq!(ana.prepare_send(ROOM, &more).unwrap().len(), 1, "a new member: ask again");
    assert!(ana.encrypt(ROOM, "m.room.message", &json!({})).is_err(), "the old session is gone until the new key is delivered");
}

#[test]
fn forged_devices_changed_keys_unsigned_one_time_keys_and_refusing_servers_are_turned_away() {
    let (ana, _) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    let (mallory, _) = addon("@mal:hs.example", "M1");
    let up = bob.on_sync(&json!({})).unwrap();
    let mal_up = mallory.on_sync(&json!({})).unwrap();
    let members = vec!["@ana:hs.example".to_string(), "@bob:hs.example".to_string()];
    let q = ana.prepare_send(ROOM, &members).unwrap();
    // the homeserver hands out a device of bob's that Mallory signed, and one whose keys were changed after signing
    let mut forged = mal_up[0].body["device_keys"].clone();
    forged["user_id"] = json!("@bob:hs.example");
    forged["device_id"] = json!("B1");
    let mut tampered = up[0].body["device_keys"].clone();
    tampered["keys"]["curve25519:B1"] = mal_up[0].body["device_keys"]["keys"]["curve25519:M1"].clone();
    for dk in [forged, tampered] {
        let (ana2, _) = addon("@ana:hs.example", "A2");
        let q2 = ana2.prepare_send(ROOM, &members).unwrap();
        let next = ana2.on_response(&q2[0], 200, &query_answer(&[("@bob:hs.example", "B1", &dk)])).unwrap();
        assert!(next.is_empty(), "a device that fails its signature gets no key: {next:?}");
        assert!(ana2.take_problems().iter().any(|p| p.contains("B1")));
        assert!(ana2.encrypt(ROOM, "m.room.message", &json!({})).is_err());
    }
    // a one-time key not signed by the device is not used
    let c = ana.on_response(&q[0], 200, &query_answer(&[("@bob:hs.example", "B1", &up[0].body["device_keys"])])).unwrap();
    let mut answer = claim_answer("@bob:hs.example", "B1", &mal_up[0]);
    let e = ana.on_response(&c[0], 200, &answer).unwrap_err();
    assert!(e.contains("reached none") && e.contains("not signed"), "{e}");
    answer["one_time_keys"] = json!({});
    // a refusing homeserver ends the job with a sentence
    let q3 = ana.prepare_send(ROOM, &members).unwrap();
    assert!(ana.on_response(&q3[0], 503, &json!({})).unwrap_err().contains("503"));
    // too many members are refused up front
    let many: Vec<String> = (0..201).map(|n| format!("@u{n}:hs.example")).collect();
    assert!(ana.prepare_send("!big:hs", &many).unwrap_err().contains("more than 200"));
}

#[test]
fn a_key_export_moves_every_readable_session_to_a_new_device_under_a_passphrase_and_a_wrong_one_moves_nothing() {
    let (alice, _) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    let otk = first_otk(&bob.on_sync(&json!({})).unwrap()[0]);
    let to_dev = alice.share_room_key(ROOM, "@bob:hs.example", &bob.curve25519_key(), &bob.ed25519_key(), Some(&otk)).unwrap();
    alice.mark_shared(ROOM).unwrap();
    let (_, c) = alice.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "old news"})).unwrap();
    bob.on_sync(&json!({"to_device": {"events": [{"type": "m.room.encrypted", "sender": "@ana:hs.example", "content": to_dev}]}})).unwrap();
    let file = bob.export_keys("a long passphrase", 100).unwrap();
    assert!(file.starts_with("-----BEGIN MEGOLM SESSION DATA-----"));
    assert!(!file.contains("old news") && !file.contains("session_key"), "the file holds nothing readable");
    let (bob2, _) = addon("@bob:hs.example", "B2");
    let event = json!({"event_id": "$e1", "content": c});
    assert!(bob2.decrypt(ROOM, &event).is_err());
    assert!(bob2.import_keys("wrong", &file).is_err());
    assert!(bob2.decrypt(ROOM, &event).is_err(), "a wrong passphrase imports nothing");
    assert_eq!(bob2.import_keys("a long passphrase", &file).unwrap(), 1);
    assert_eq!(bob2.decrypt(ROOM, &event).unwrap()["body"], "old news");
    assert_eq!(bob2.import_keys("a long passphrase", &file).unwrap(), 0, "a known session is not replaced");
}

// ---------------------------------------------------------------------------------------------------------- device verification (SAS)

/// Carry the requests one addon made to the other as the homeserver would: `sendToDevice` becomes the other's to-device events, and a
/// device lookup is answered from the other's own signed device keys. Returns what the receiving side wants sent back.
fn carry(from: &OlmAddon, reqs: Vec<AddonRequest>, to: &OlmAddon) -> Vec<AddonRequest> {
    let mut events = Vec::new();
    for r in reqs {
        if let Some(rest) = r.path.strip_prefix("v3/sendToDevice/") {
            let ty = rest.split('/').next().unwrap();
            for (_, devs) in r.body["messages"].as_object().unwrap() {
                for (_, content) in devs.as_object().unwrap() {
                    events.push(json!({"type": ty, "sender": from.user_id, "content": content}));
                }
            }
        } else if r.path == "v3/keys/query" {
            // `from` asks about `to`: answer from the other's signed device keys, as the homeserver would
            let dk = to.device_keys(&to.lock());
            let resp = json!({"device_keys": {to.user_id.clone(): {to.device_id.clone(): dk}}});
            let _ = from.on_response(&r, 200, &resp).unwrap();
        }
    }
    to.on_sync(&json!({"to_device": {"events": events}, "device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap_or_default()
}

fn pair() -> (OlmAddon, OlmAddon) {
    let (a, _) = addon("@ana:hs.example", "A1");
    let (b, _) = addon("@bob:hs.example", "B1");
    for x in [&a, &b] {
        x.on_sync(&json!({})).unwrap();
    }
    (a, b)
}

/// Run messages both ways until nothing is left to send: what one side's sync asks for is carried to the other, whose answers are
/// carried back, and so on.
fn settle(a: &OlmAddon, b: &OlmAddon) {
    let sync = json!({"device_one_time_keys_count": {"signed_curve25519": 50}});
    let (mut pa, mut pb) = (a.on_sync(&sync).unwrap(), b.on_sync(&sync).unwrap());
    for _ in 0..20 {
        if pa.is_empty() && pb.is_empty() {
            return;
        }
        let (next_pb, next_pa) = (carry(a, pa, b), carry(b, pb, a));
        pa = next_pa;
        pb = next_pb;
    }
    panic!("the two sides never settled");
}

#[test]
fn two_devices_verify_each_other_by_comparing_the_same_emoji_and_both_mark_the_other_verified() {
    let (a, b) = pair();
    a.verify_start("@bob:hs.example", "B1").unwrap();
    assert!(a.verifications()[0].state.contains("asked"));
    settle(&a, &b);
    // bob's side waits for its person: nothing is answered before the accept
    let incoming = b.verifications();
    assert_eq!((incoming.len(), incoming[0].next), (1, "accept"), "{incoming:?}");
    assert!(incoming[0].emoji.is_empty());
    let txn = incoming[0].txn.clone();
    assert!(b.verify_confirm(&txn, true).is_err(), "no yes before the strings were shown");
    b.verify_accept(&txn).unwrap();
    settle(&a, &b);
    let (ca, cb) = (a.verifications(), b.verifications());
    assert_eq!((ca[0].next, cb[0].next), ("confirm", "confirm"), "{ca:?} {cb:?}");
    assert_eq!(ca[0].emoji.len(), 7);
    assert_eq!(ca[0].emoji, cb[0].emoji, "both people see the same seven emoji");
    assert_eq!(ca[0].decimals, cb[0].decimals);
    assert!(!a.is_verified("@bob:hs.example", "B1"));
    a.verify_confirm(&ca[0].txn, true).unwrap();
    settle(&a, &b);
    assert!(!a.is_verified("@bob:hs.example", "B1"), "the other person has not said yes yet");
    b.verify_confirm(&cb[0].txn, true).unwrap();
    settle(&a, &b);
    assert!(a.is_verified("@bob:hs.example", "B1") && b.is_verified("@ana:hs.example", "A1"));
    assert_eq!(a.verifications()[0].state, "verified");
    // the verified pair is in the vault: a new add-on on the same store knows it
    let (_, store) = addon("@x:hs.example", "X");
    drop(store);
}

#[test]
fn a_mismatch_a_wrong_commitment_a_forged_mac_and_stray_events_end_the_verification_with_a_reason() {
    // the person says the strings differ
    let (a, b) = pair();
    a.verify_start("@bob:hs.example", "B1").unwrap();
    settle(&a, &b);
    b.verify_accept(&b.verifications()[0].txn).unwrap();
    settle(&a, &b);
    a.verify_confirm(&a.verifications()[0].txn, false).unwrap();
    assert!(a.verifications()[0].state.contains("did not match"));
    settle(&a, &b);
    assert!(b.verifications()[0].state.contains("ended"), "the other side is told: {:?}", b.verifications());
    assert!(!a.is_verified("@bob:hs.example", "B1") && !b.is_verified("@ana:hs.example", "A1"));
    // a key that is not the one committed to ends it (a man in the middle)
    let (a, b) = pair();
    a.verify_start("@bob:hs.example", "B1").unwrap();
    settle(&a, &b);
    let txn = b.verifications()[0].txn.clone();
    b.verify_accept(&txn).unwrap();
    // walk the messages by hand up to the point where ana has sent her key and waits for bob's: ready, start, accept
    let sync = json!({"device_one_time_keys_count": {"signed_curve25519": 50}});
    let ready = b.on_sync(&sync).unwrap();
    let start = carry(&b, ready, &a);
    let accept = carry(&a, start, &b);
    carry(&b, accept, &a);
    assert_eq!(a.verifications()[0].state, "in progress; no strings yet");
    // then bob's key is replaced with another
    let evil = iyakki_webcrypto::sas::Sas::new(iyakki_webcrypto::sas::Parties { start_user: "x".into(), start_device: "x".into(), accept_user: "y".into(), accept_device: "y".into(), transaction: "t".into() }, false).unwrap().public_b64();
    let _ = a.on_sync(&json!({"to_device": {"events": [{"type": "m.key.verification.key", "sender": "@bob:hs.example", "content": {"transaction_id": txn, "key": evil}}]}, "device_one_time_keys_count": {"signed_curve25519": 50}}));
    assert!(a.take_problems().iter().any(|p| p.contains("committed") || p.contains("middle")), "the wrong key is refused");
    assert!(a.verifications()[0].state.contains("ended"));
    // events with no transaction, from nobody, for unknown verifications or with the wrong method change nothing
    let (a, _) = pair();
    let junk = json!({"to_device": {"events": [
        {"type": "m.key.verification.key", "sender": "@bob:hs.example", "content": {"key": "AAAA"}},
        {"type": "m.key.verification.mac", "sender": "@bob:hs.example", "content": {"transaction_id": "nope", "mac": {}, "keys": "x"}},
        {"type": "m.key.verification.start", "sender": "@bob:hs.example", "content": {"transaction_id": "t9", "from_device": "B1", "method": "m.reciprocate.v1"}},
        {"type": "m.key.verification.request", "sender": "@bob:hs.example", "content": {"transaction_id": "t8", "from_device": "B1", "methods": ["m.qr_code.scan.v1"]}},
    ]}});
    let _ = a.on_sync(&junk);
    assert!(a.verifications().is_empty(), "{:?}", a.verifications());
    assert!(a.verify_start("bad user", "B1").is_err());
    assert!(a.verify_accept("nope").is_err() && a.verify_cancel("nope").is_err());
}

#[test]
fn a_request_start_pair_that_comes_without_a_request_waits_for_the_person_and_the_number_of_verifications_is_bounded() {
    let (a, b) = pair();
    // bob starts at once with no request first
    let txn = "t-direct";
    let start = json!({"from_device": "B1", "method": "m.sas.v1", "transaction_id": txn, "key_agreement_protocols": ["curve25519-hkdf-sha256"], "hashes": ["sha256"], "message_authentication_codes": ["hkdf-hmac-sha256.v2"], "short_authentication_string": ["decimal", "emoji"]});
    a.on_sync(&json!({"to_device": {"events": [{"type": "m.key.verification.start", "sender": "@bob:hs.example", "content": start}]}, "device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    let c = a.verifications();
    assert_eq!((c.len(), c[0].next), (1, "accept"));
    assert!(a.lock().flows.outbox.iter().all(|r| !r.path.contains("m.key.verification.accept")), "nothing is answered before the person accepts");
    a.verify_accept(txn).unwrap();
    assert!(a.lock().flows.outbox.iter().any(|r| r.path.contains("m.key.verification.accept")));
    let _ = b;
    // bounded
    let (a, _) = pair();
    for n in 0..sas_flow::MAX_FLOWS {
        a.verify_start("@bob:hs.example", &format!("D{n}")).unwrap();
    }
    assert!(a.verify_start("@bob:hs.example", "DX").unwrap_err().contains("too many"));
}

#[test]
fn an_undecryptable_event_asks_the_persons_own_devices_for_its_key_once() {
    let (a, _) = pair();
    let ev = json!({"type": "m.room.encrypted", "event_id": "$e", "content": {"algorithm": MEGOLM_ALGORITHM, "sender_key": "SENDERKEY", "session_id": "SESSION1", "ciphertext": "x"}});
    let reqs = a.key_requests(ROOM, &ev);
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].path.starts_with("v3/sendToDevice/m.room_key_request/"));
    let msg = &reqs[0].body["messages"]["@ana:hs.example"]["*"];
    assert_eq!((msg["action"].as_str(), msg["body"]["session_id"].as_str(), msg["body"]["room_id"].as_str(), msg["requesting_device_id"].as_str()), (Some("request"), Some("SESSION1"), Some(ROOM), Some("A1")));
    assert!(a.key_requests(ROOM, &ev).is_empty(), "once per session in a run");
    assert!(a.key_requests(ROOM, &json!({"content": {}})).is_empty());
}

#[test]
fn the_verified_devices_are_kept_in_the_vault_across_a_restart() {
    let (a, b) = pair();
    a.verify_start("@bob:hs.example", "B1").unwrap();
    settle(&a, &b);
    b.verify_accept(&b.verifications()[0].txn).unwrap();
    settle(&a, &b);
    a.verify_confirm(&a.verifications()[0].txn, true).unwrap();
    b.verify_confirm(&b.verifications()[0].txn, true).unwrap();
    settle(&a, &b);
    assert!(a.is_verified("@bob:hs.example", "B1"));
    let store = a.store.clone();
    let again = OlmAddon::open(store, "acct", "@ana:hs.example", "A1").unwrap();
    assert!(again.is_verified("@bob:hs.example", "B1"));
    assert!(!again.is_verified("@bob:hs.example", "B2"));
}

// ---------------------------------------------------------------------------------------------------------- key requests of other devices

fn to_events(from_user: &str, reqs: &[AddonRequest]) -> Value {
    let mut events = Vec::new();
    for r in reqs {
        if let Some(rest) = r.path.strip_prefix("v3/sendToDevice/") {
            let ty = rest.split('/').next().unwrap();
            for (_, devs) in r.body["messages"].as_object().unwrap() {
                for (_, content) in devs.as_object().unwrap() {
                    events.push(json!({"type": ty, "sender": from_user, "content": content}));
                }
            }
        }
    }
    json!({"to_device": {"events": events}, "device_one_time_keys_count": {"signed_curve25519": 50}})
}

/// Two devices of one person: the first has a room session and a message, the second has neither and has asked for the key.
fn asked() -> (OlmAddon, OlmAddon, Value, AddonRequest, Value) {
    let (a1, _) = addon("@ana:hs.example", "A1");
    let (a2, _) = addon("@ana:hs.example", "A2");
    let up2 = a2.on_sync(&json!({})).unwrap();
    a1.on_sync(&json!({})).unwrap();
    let key = a1.room_key_content(ROOM).unwrap();
    a1.mark_shared(ROOM).unwrap();
    let (_, c) = a1.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "before you came"})).unwrap();
    let event = json!({"type": "m.room.encrypted", "event_id": "$e1", "content": c});
    assert!(a2.decrypt(ROOM, &event).is_err());
    let asks = a2.key_requests(ROOM, &event);
    assert_eq!(asks.len(), 1);
    let _ = key;
    let dk2 = a2.device_keys(&a2.lock());
    (a1, a2, to_events("@ana:hs.example", &asks), up2[0].clone(), json!({"event": event, "dk2": dk2}))
}

#[test]
fn another_device_of_the_person_gets_the_room_key_only_after_the_person_says_yes_and_then_reads_old_messages() {
    let (a1, a2, request_events, up2, extra) = asked();
    let (event, dk2) = (&extra["event"], &extra["dk2"]);
    a1.on_sync(&request_events).unwrap();
    let cards = a1.key_asks();
    assert_eq!(cards.len(), 1);
    assert_eq!((cards[0].device.as_str(), cards[0].room.as_str(), cards[0].next, cards[0].verified), ("A2", ROOM, "answer", false));
    // nothing goes out before the answer, however many syncs pass
    assert!(a1.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap().is_empty());
    assert!(a1.key_ask_answer("nothing", true).is_err());
    a1.key_ask_answer(&cards[0].id, true).unwrap();
    assert!(a1.key_ask_answer(&cards[0].id, false).unwrap_err().contains("already"));
    let step1 = a1.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    assert_eq!((step1.len(), step1[0].path.as_str()), (1, "v3/keys/query"));
    // the homeserver lists the device with its own signature
    let step2 = a1.on_response(&step1[0], 200, &query_answer(&[("@ana:hs.example", "A2", dk2)])).unwrap();
    assert_eq!((step2.len(), step2[0].path.as_str()), (1, "v3/keys/claim"));
    let step3 = a1.on_response(&step2[0], 200, &claim_answer("@ana:hs.example", "A2", &up2)).unwrap();
    assert_eq!((step3.len(), step3[0].path.starts_with("v3/sendToDevice/m.room.encrypted/")), (1, true));
    assert!(!step3[0].body.to_string().contains("session_key"), "the key travels Olm-encrypted");
    assert_eq!(a1.key_asks()[0].state, "sent");
    a2.on_sync(&to_events("@ana:hs.example", &step3)).unwrap();
    assert_eq!(a2.decrypt(ROOM, event).unwrap()["body"], "before you came");
}

#[test]
fn a_refused_request_a_stranger_an_unknown_session_and_a_key_nobody_asked_for_all_send_and_change_nothing() {
    let (a1, a2, request_events, _, extra) = asked();
    // refused: no request ever goes out
    a1.on_sync(&request_events).unwrap();
    let id = a1.key_asks()[0].id.clone();
    a1.key_ask_answer(&id, false).unwrap();
    assert_eq!(a1.key_asks()[0].state, "you refused");
    assert!(a1.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap().is_empty());
    // a request from another user, for a session this device does not hold, or from this very device
    let body = |sid: &str| json!({"action": "request", "request_id": "r9", "requesting_device_id": "A9", "body": {"algorithm": MEGOLM_ALGORITHM, "room_id": ROOM, "sender_key": "x", "session_id": sid}});
    let sid = extra["event"]["content"]["session_id"].as_str().unwrap().to_string();
    let (b1, _) = addon("@ana:hs.example", "B1");
    b1.on_sync(&json!({})).unwrap();
    b1.room_key_content(ROOM).unwrap();
    let stranger = json!({"to_device": {"events": [{"type": "m.room_key_request", "sender": "@eve:hs.example", "content": body(&sid)}, {"type": "m.room_key_request", "sender": "@ana:hs.example", "content": body("UNKNOWNSESSION")}]}});
    a1.on_sync(&stranger).unwrap();
    assert_eq!(a1.key_asks().len(), 1, "only the first, refused one");
    let mut own = body(&sid);
    own["requesting_device_id"] = json!("A1");
    a1.on_sync(&json!({"to_device": {"events": [{"type": "m.room_key_request", "sender": "@ana:hs.example", "content": own}]}})).unwrap();
    assert_eq!(a1.key_asks().len(), 1);
    // a cancellation removes a waiting card
    let mut waiting = body(&sid);
    waiting["requesting_device_id"] = json!("A5");
    a1.on_sync(&json!({"to_device": {"events": [{"type": "m.room_key_request", "sender": "@ana:hs.example", "content": waiting.clone()}]}})).unwrap();
    assert_eq!(a1.key_asks().len(), 2);
    waiting["action"] = json!("request_cancellation");
    a1.on_sync(&json!({"to_device": {"events": [{"type": "m.room_key_request", "sender": "@ana:hs.example", "content": waiting}]}})).unwrap();
    assert_eq!(a1.key_asks().len(), 1);
    // a forwarded key nobody asked for is refused: a2 never asked for this session in room "!other"
    let (a3, _) = addon("@ana:hs.example", "A3");
    let up3 = a3.on_sync(&json!({})).unwrap();
    let sk = a1.room_key_content("!other:hs.example").unwrap();
    let curve = a3.curve25519_key();
    let ed = a3.ed25519_key();
    let _ = (a2, sk);
    let mut i = a1.lock();
    let session = i.outbound["!other:hs.example"].0.session_id();
    i.devices.insert(("@ana:hs.example".into(), "A3".into()), (curve, ed));
    i.asks.insert("A3/r1".into(), keyshare_flow::Ask { device: "A3".into(), room: "!other:hs.example".into(), session, stage: keyshare_flow::AskStage::Waiting });
    drop(i);
    a1.key_ask_answer("A3/r1", true).unwrap();
    let reqs = a1.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    assert_eq!(reqs[0].path, "v3/keys/claim");
    let out = a1.on_response(&reqs[0], 200, &claim_answer("@ana:hs.example", "A3", &up3[0])).unwrap();
    let _ = a3.on_sync(&to_events("@ana:hs.example", &out));
    assert!(a3.take_problems()[0].contains("did not ask for"));
    assert!(a3.lock().inbound.is_empty());
}

#[test]
fn a_one_time_key_not_signed_by_the_asking_device_stops_the_answer() {
    let (a1, _a2, request_events, up2, extra) = asked();
    a1.on_sync(&request_events).unwrap();
    let id = a1.key_asks()[0].id.clone();
    a1.key_ask_answer(&id, true).unwrap();
    let q = a1.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    let claim = a1.on_response(&q[0], 200, &query_answer(&[("@ana:hs.example", "A2", &extra["dk2"])])).unwrap();
    let mut forged = claim_answer("@ana:hs.example", "A2", &up2);
    let (k, v) = forged["one_time_keys"]["@ana:hs.example"]["A2"].as_object().unwrap().iter().next().map(|(k, v)| (k.clone(), v.clone())).unwrap();
    let mut v = v;
    v["key"] = json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    forged["one_time_keys"]["@ana:hs.example"]["A2"] = json!({ k: v });
    assert!(a1.on_response(&claim[0], 200, &forged).unwrap().is_empty());
    assert!(a1.key_asks()[0].state.contains("not sent"));
}

#[test]
fn sessions_this_device_started_itself_are_in_the_export_and_a_new_device_reads_them() {
    let (a1, _) = addon("@ana:hs.example", "A1");
    a1.on_sync(&json!({})).unwrap();
    a1.room_key_content(ROOM).unwrap();
    a1.mark_shared(ROOM).unwrap();
    let (_, c) = a1.encrypt(ROOM, "m.room.message", &json!({"msgtype": "m.text", "body": "my own words"})).unwrap();
    let file = a1.export_keys("a long passphrase", 100).unwrap();
    let (a2, _) = addon("@ana:hs.example", "A2");
    assert_eq!(a2.import_keys("a long passphrase", &file).unwrap(), 1);
    assert_eq!(a2.decrypt(ROOM, &json!({"event_id": "$m", "content": c})).unwrap()["body"], "my own words");
    // this device reads its own message too (the echo of its own event)
    assert_eq!(a1.decrypt(ROOM, &json!({"event_id": "$m", "content": c})).unwrap()["body"], "my own words");
}

// ---------------------------------------------------------------------------------------------------------- cross-signing

fn uia() -> Value {
    json!({"session": "S1", "flows": [{"stages": ["m.login.password"]}]})
}

/// Run the setup on `a` against a homeserver that asks for the password once; returns the body of the signature upload.
fn set_up_cross_signing(a: &OlmAddon, password: &str) -> (Value, Value) {
    a.cross_signing_setup(password).unwrap();
    let reqs = a.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    let first = reqs.iter().find(|r| r.path == "v3/keys/device_signing/upload").unwrap().clone();
    assert!(first.body.get("auth").is_none(), "the first call holds no password");
    let retry = a.on_response(&first, 401, &uia()).unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].body["auth"]["type"], "m.login.password");
    assert_eq!(retry[0].body["auth"]["session"], "S1");
    assert_eq!(retry[0].body["auth"]["password"], password);
    let sigs = a.on_response(&retry[0], 200, &json!({})).unwrap();
    assert_eq!((sigs.len(), sigs[0].path.as_str()), (1, "v3/keys/signatures/upload"));
    (retry[0].body.clone(), sigs[0].body.clone())
}

#[test]
fn cross_signing_keys_are_made_uploaded_after_the_password_step_and_sign_this_device() {
    let (a, store) = addon("@ana:hs.example", "A1");
    a.on_sync(&json!({})).unwrap();
    assert!(a.cross_signing_setup("").is_err());
    assert!(a.cross_signing_state().contains("not set up"));
    let (body, sigs) = set_up_cross_signing(&a, "the account password");
    assert!(a.cross_signing_state().contains("set up (master key"));
    assert!(a.cross_signing_setup("again").unwrap_err().contains("exist already"));
    // the self-signing key and the user-signing key are signed by the master key
    let master = key_of_test(&body["master_key"]);
    for k in ["self_signing_key", "user_signing_key"] {
        let obj = &body[k];
        let sig = obj["signatures"]["@ana:hs.example"][format!("ed25519:{master}")].as_str().unwrap();
        let mut bare = obj.clone();
        bare.as_object_mut().unwrap().remove("signatures");
        iyakki_webcrypto::olm::verify_ed25519(&master, canonical_json(&bare).as_bytes(), sig).unwrap();
    }
    // this device's own object is signed by the self-signing key, and keeps its own signature
    let selfkey = key_of_test(&body["self_signing_key"]);
    let dk = &sigs["@ana:hs.example"]["A1"];
    let mut bare = dk.clone();
    bare.as_object_mut().unwrap().remove("signatures");
    iyakki_webcrypto::olm::verify_ed25519(&selfkey, canonical_json(&bare).as_bytes(), dk["signatures"]["@ana:hs.example"][format!("ed25519:{selfkey}")].as_str().unwrap()).unwrap();
    assert!(dk["signatures"]["@ana:hs.example"]["ed25519:A1"].is_string());
    assert!(a.is_verified("@ana:hs.example", "A1"));
    // the password was only in memory for the retry: the saved state has neither it nor the seeds in clear
    let saved = store.get(&vault_name("acct")).unwrap();
    assert!(!saved.contains("the account password"));
    // the keys survive a restart
    let again = OlmAddon::open(store.clone(), "acct", "@ana:hs.example", "A1").unwrap();
    assert!(again.cross_signing_state().contains(&master));
}

fn key_of_test(o: &Value) -> String {
    o["keys"].as_object().unwrap().values().next().unwrap().as_str().unwrap().to_string()
}

#[test]
fn a_refusing_or_password_less_homeserver_leaves_no_keys_behind() {
    let (a, _) = addon("@ana:hs.example", "A1");
    a.cross_signing_setup("pw").unwrap();
    let first = a.on_sync(&json!({})).unwrap().into_iter().find(|r| r.path == "v3/keys/device_signing/upload").unwrap();
    // only another way of signing in is offered
    let err = a.on_response(&first, 401, &json!({"session": "S", "flows": [{"stages": ["m.login.sso"]}]})).unwrap_err();
    assert!(err.contains("another way"));
    // the answer to the retry is a refusal
    let (b, _) = addon("@ana:hs.example", "A2");
    b.cross_signing_setup("pw").unwrap();
    let first = b.on_sync(&json!({})).unwrap().into_iter().find(|r| r.path == "v3/keys/device_signing/upload").unwrap();
    let retry = b.on_response(&first, 401, &uia()).unwrap();
    assert!(b.on_response(&retry[0], 403, &json!({"errcode": "M_FORBIDDEN"})).unwrap_err().contains("did not accept"));
    assert!(b.cross_signing_state().contains("not set up"), "the refused keys are forgotten");
    b.cross_signing_setup("pw").unwrap();
}

#[test]
fn a_second_device_trusts_the_chain_only_to_the_persons_own_master_key_and_never_to_a_changed_one() {
    let (a1, _) = addon("@ana:hs.example", "A1");
    let (a2, _) = addon("@ana:hs.example", "A2");
    a1.on_sync(&json!({})).unwrap();
    a2.on_sync(&json!({})).unwrap();
    let (body, sigs) = set_up_cross_signing(&a1, "pw");
    let answer = |master: &Value, selfk: &Value, dk: &Value| json!({"device_keys": {"@ana:hs.example": {"A1": dk}}, "master_keys": {"@ana:hs.example": master}, "self_signing_keys": {"@ana:hs.example": selfk}});
    let q = AddonRequest { method: "POST".into(), path: "v3/keys/query".into(), body: json!({"device_keys": {"@ana:hs.example": ["A1"]}, "timeout": 10_001}) };
    let dk = &sigs["@ana:hs.example"]["A1"];
    // this device has verified nobody yet: the chain is shown but the master key is not trusted
    a2.on_response(&q, 200, &answer(&body["master_key"], &body["self_signing_key"], dk)).unwrap();
    assert!(!a2.is_verified("@ana:hs.example", "A1"));
    // after a SAS verification of a device of the person the master key is met and trusted
    a2.lock().flows.verified.insert(("@ana:hs.example".into(), "A9".into()));
    a2.on_response(&q, 200, &answer(&body["master_key"], &body["self_signing_key"], dk)).unwrap();
    assert!(a2.is_verified("@ana:hs.example", "A1"));
    assert!(!a2.is_verified("@ana:hs.example", "A7"));
    // a self-signing key the master did not sign is not used
    let (a3, _) = addon("@ana:hs.example", "A3");
    a3.lock().flows.verified.insert(("@ana:hs.example".into(), "A9".into()));
    let mut forged = body["self_signing_key"].clone();
    forged["signatures"] = json!({});
    a3.on_response(&q, 200, &answer(&body["master_key"], &forged, dk)).unwrap();
    assert!(!a3.is_verified("@ana:hs.example", "A1"));
    assert!(a3.take_problems().iter().any(|p| p.contains("not signed by its master")));
    // a different master key later is refused and reported
    let (other, _) = addon("@ana:hs.example", "A5");
    other.on_sync(&json!({})).unwrap();
    let (obody, osigs) = set_up_cross_signing(&other, "pw");
    a2.on_response(&q, 200, &answer(&obody["master_key"], &obody["self_signing_key"], &osigs["@ana:hs.example"]["A5"])).unwrap();
    assert!(a2.take_problems().iter().any(|p| p.contains("master key") && p.contains("changed")));
    // a device that the self-signing key did not sign is not vouched for
    let plain = a1.device_keys(&a1.lock());
    let mut unsigned_dev = plain.clone();
    unsigned_dev["signatures"]["@ana:hs.example"].as_object_mut().unwrap().retain(|k, _| k == "ed25519:A1");
    a2.on_response(&q, 200, &answer(&body["master_key"], &body["self_signing_key"], &unsigned_dev)).unwrap();
}

#[test]
fn another_persons_master_key_is_signed_only_when_asked_and_their_devices_count_as_verified_after_the_homeserver_took_it() {
    let (ana, store) = addon("@ana:hs.example", "A1");
    let (bob, _) = addon("@bob:hs.example", "B1");
    ana.on_sync(&json!({})).unwrap();
    bob.on_sync(&json!({})).unwrap();
    let (_, _) = set_up_cross_signing(&ana, "pw");
    let (bbody, bsigs) = set_up_cross_signing(&bob, "pw");
    let bdev = &bsigs["@bob:hs.example"]["B1"];
    let q = AddonRequest { method: "POST".into(), path: "v3/keys/query".into(), body: json!({"device_keys": {"@bob:hs.example": ["B1"]}, "timeout": 10_001}) };
    let answer = |master: &Value| json!({"device_keys": {"@bob:hs.example": {"B1": bdev}}, "master_keys": {"@bob:hs.example": master}, "self_signing_keys": {"@bob:hs.example": bbody["self_signing_key"]}});
    // nothing is known of bob yet, and her own id is not for this step
    assert!(ana.cross_signing_trust_user("@bob:hs.example").unwrap_err().contains("no cross-signing key of @bob"));
    assert!(ana.cross_signing_trust_user("@ana:hs.example").is_err());
    assert!(ana.cross_signing_trust_user("bob").is_err());
    // seen, but his devices are NOT verified from the chain alone
    ana.on_response(&q, 200, &answer(&bbody["master_key"])).unwrap();
    assert!(!ana.is_verified("@bob:hs.example", "B1"));
    // the person says she checked the key: the master key object comes back signed by her user-signing key, with nothing else changed
    ana.cross_signing_trust_user("@bob:hs.example").unwrap();
    let out = ana.on_sync(&json!({"device_one_time_keys_count": {"signed_curve25519": 50}})).unwrap();
    let up = out.iter().find(|r| r.path == "v3/keys/signatures/upload" && r.body.get("@bob:hs.example").is_some()).expect("the signature upload").clone();
    let mpub = key_of_test(&bbody["master_key"]);
    let signed = &up.body["@bob:hs.example"][&mpub];
    let ana_sigs = signed["signatures"]["@ana:hs.example"].as_object().unwrap();
    assert_eq!(ana_sigs.len(), 1, "exactly one signature of hers, by her user-signing key");
    let (kid, sig) = ana_sigs.iter().next().unwrap();
    let upub = kid.trim_start_matches("ed25519:").to_string();
    let mut bare = signed.clone();
    bare.as_object_mut().unwrap().remove("signatures");
    iyakki_webcrypto::olm::verify_ed25519(&upub, canonical_json(&bare).as_bytes(), sig.as_str().unwrap()).unwrap();
    assert_eq!(bare, bbody["master_key"], "the key object itself is untouched");
    // until the homeserver took it nothing changes; a refusal says so and trusts nothing
    assert!(!ana.is_verified("@bob:hs.example", "B1"));
    assert!(ana.on_response(&up, 200, &json!({"failures": {"@bob:hs.example": {"@bob:hs.example": {"status": 400}}}})).unwrap_err().contains("did not take"));
    assert!(ana.on_response(&up, 500, &json!({})).is_err());
    ana.on_response(&q, 200, &answer(&bbody["master_key"])).unwrap();
    assert!(!ana.is_verified("@bob:hs.example", "B1"));
    assert!(ana.on_response(&up, 200, &json!({"failures": {}})).unwrap().is_empty());
    ana.on_response(&q, 200, &answer(&bbody["master_key"])).unwrap();
    assert!(ana.is_verified("@bob:hs.example", "B1"), "bob's device signed by his self-signing key now counts");
    assert!(!ana.is_verified("@bob:hs.example", "B9"));
    // it is kept across a restart
    let again = OlmAddon::open(store, "acct", "@ana:hs.example", "A1").unwrap();
    again.on_response(&q, 200, &answer(&bbody["master_key"])).unwrap();
    assert!(again.is_verified("@bob:hs.example", "B1"));
    // a different master key of bob is refused and the old signature does not carry over
    let (bob2, _) = addon("@bob:hs.example", "B2");
    bob2.on_sync(&json!({})).unwrap();
    let (b2body, b2sigs) = set_up_cross_signing(&bob2, "pw");
    let a2 = json!({"device_keys": {"@bob:hs.example": {"B2": b2sigs["@bob:hs.example"]["B2"]}}, "master_keys": {"@bob:hs.example": b2body["master_key"]}, "self_signing_keys": {"@bob:hs.example": b2body["self_signing_key"]}});
    again.on_response(&q, 200, &a2).unwrap();
    assert!(!again.is_verified("@bob:hs.example", "B2"));
    assert!(again.take_problems().iter().any(|p| p.contains("changed")));
}
