use std::sync::{Arc, Mutex};

use iyakki_webcrypto::webauthn as wa;
use iyakki_webcrypto::b64;
use serde_json::{json, Value};

use super::*;
use crate::gitkeys::MemStore;

const ORIGIN: &str = "https://login.example.com";

fn setup() -> (Arc<Passkeys>, Arc<MemStore>, Arc<Mutex<Vec<String>>>) {
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(vec![]));
    let l = log.clone();
    let mut p = Passkeys::new(store.clone(), Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))));
    p.now = || 50;
    (Arc::new(p), store, log)
}
fn ch(n: u8) -> String {
    b64::url(&[n; 32])
}

#[test]
fn a_site_gets_a_registration_and_later_a_sign_in_that_its_own_checks_accept() {
    let (p, _, _) = setup();
    let r = p.register(ORIGIN, "example.com", &ch(1), &b64::url(b"user-1"), "ana", &[], true).unwrap();
    let reg = wa::verify_registration(&b64::decode(&r.attestation_object).unwrap(), &b64::decode(&r.client_data_json).unwrap(), &[1; 32], ORIGIN, "example.com").unwrap();
    assert_eq!(b64::url(&reg.credential_id), r.credential_id);
    assert_eq!(credential_id_of(&b64::decode(&r.attestation_object).unwrap()).unwrap(), reg.credential_id);
    let s = p.sign_in(ORIGIN, "example.com", &ch(2), &[], true).unwrap();
    assert_eq!(b64::decode(&s.user_handle).unwrap(), b"user-1");
    let n = wa::verify_assertion(&reg.public_key, &b64::decode(&s.authenticator_data).unwrap(), &b64::decode(&s.client_data_json).unwrap(), &b64::decode(&s.signature).unwrap(), &[2; 32], ORIGIN, "example.com", 0).unwrap();
    assert_eq!(n, 1);
    let s2 = p.sign_in(ORIGIN, "example.com", &ch(3), &[], true).unwrap();
    let n2 = wa::verify_assertion(&reg.public_key, &b64::decode(&s2.authenticator_data).unwrap(), &b64::decode(&s2.client_data_json).unwrap(), &b64::decode(&s2.signature).unwrap(), &[3; 32], ORIGIN, "example.com", n).unwrap();
    assert_eq!(n2, 2, "the count is kept in the vault between uses");
}

#[test]
fn without_presence_for_the_wrong_site_or_with_a_short_challenge_nothing_happens_and_nothing_is_saved() {
    let (p, store, _) = setup();
    assert!(p.register(ORIGIN, "example.com", &ch(1), &b64::url(b"u"), "ana", &[], false).is_err());
    assert!(p.register(ORIGIN, "evil.com", &ch(1), &b64::url(b"u"), "ana", &[], true).is_err());
    assert!(p.register("http://example.com", "example.com", &ch(1), &b64::url(b"u"), "ana", &[], true).is_err());
    assert!(p.register(ORIGIN, "example.com", &b64::url(b"short"), &b64::url(b"u"), "ana", &[], true).is_err());
    assert!(store.map.lock().unwrap().is_empty());
    p.register(ORIGIN, "example.com", &ch(1), &b64::url(b"u"), "ana", &[], true).unwrap();
    assert!(p.sign_in(ORIGIN, "example.com", &ch(2), &[], false).is_err());
    assert!(p.sign_in("https://other.org", "other.org", &ch(2), &[], true).is_err(), "no passkey for that site");
}

#[test]
fn keys_ids_and_signatures_never_reach_the_audit_log_the_list_or_a_locked_vault() {
    let (p, store, log) = setup();
    let r = p.register(ORIGIN, "example.com", &ch(1), &b64::url(b"u"), "ana", &[], true).unwrap();
    let s = p.sign_in(ORIGIN, "example.com", &ch(2), &[], true).unwrap();
    let shown = format!("{}{}", p.list().unwrap(), log.lock().unwrap().join("\n"));
    for secret in [&r.credential_id, &s.signature, &store.map.lock().unwrap().get(STATE).unwrap().clone()[..40].to_string()] {
        assert!(!shown.contains(secret.as_str()));
    }
    assert_eq!(p.list().unwrap(), "example.com: ana");
    *store.locked.lock().unwrap() = true;
    assert!(p.sign_in(ORIGIN, "example.com", &ch(2), &[], true).unwrap_err().contains("locked"));
    *store.locked.lock().unwrap() = false;
    p.delete("example.com", "ana").unwrap();
    assert_eq!(p.list().unwrap(), "No passkeys are saved.");
    assert!(p.delete("example.com", "ana").is_err());
}

#[test]
fn the_tools_ask_on_a_card_that_names_the_site_every_time_and_a_grant_cannot_cover_them() {
    let (p, _, _) = setup();
    let t = PasskeyTools::new(Arc::new(crate::assign::tests_support::NoTools), p);
    let defs = t.defs();
    assert_eq!(defs.len(), 4);
    for d in &defs {
        assert_eq!(d.risk == Risk::Safe, d.spec.name == LIST);
        assert_eq!(t.pre_approved(&d.spec.name), d.spec.name == LIST);
        assert_eq!(t.grant_blocker(&d.spec.name, &json!({})).is_some(), d.spec.name != LIST);
        assert!(!t.approved_always(&d.spec.name));
    }
    let args = json!({"origin": ORIGIN, "rp_id": "example.com", "challenge": ch(4), "user_handle": b64::url(b"u"), "user_name": "ana"});
    let card = t.preview(REGISTER, &args).unwrap();
    assert!(card.contains("example.com") && card.contains("login.example.com") && card.contains("ana") && card.contains("Confirm that you are here"));
    assert!(t.preview(SIGN_IN, &args).unwrap().contains("Sign in to example.com"));
    assert!(t.refusal(REGISTER, &json!({"origin": ORIGIN, "rp_id": "evil.com", "challenge": ch(4)})).is_some());
    let made: Value = serde_json::from_str(&t.call(REGISTER, &args).unwrap()).unwrap();
    assert!(made["attestationObject"].is_string() && made["clientDataJSON"].is_string());
    let got: Value = serde_json::from_str(&t.call(SIGN_IN, &args).unwrap()).unwrap();
    assert!(got["signature"].is_string());
    assert_eq!(t.call(LIST, &json!({})).unwrap(), "example.com: ana");
    assert_eq!(t.call(DELETE, &json!({"rp_id": "example.com", "user_name": "ana"})).unwrap(), "Deleted.");
    assert!(PROMPT_RULE.contains("every time"));
}
