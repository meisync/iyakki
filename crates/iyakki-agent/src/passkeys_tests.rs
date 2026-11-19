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

// ---- the page-facing half: a script on a page asks, the person's card answers, the vault makes the passkey

struct Card {
    answer: bool,
    asked: Mutex<Vec<String>>,
}

impl Presence for Card {
    fn confirm(&self, action: &str, origin: &str, site: &str, account: &str) -> bool {
        self.asked.lock().unwrap().push(format!("{action} {origin} {site} {account}"));
        self.answer
    }
}

struct NoWeb;
impl iyakki_web::Fetch for NoWeb {
    fn get(&self, _: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
        Err("no network in this test".into())
    }
}

fn page(svc: Arc<Passkeys>, card: Arc<Card>) -> iyakki_js::LivePage {
    let host: Arc<Mutex<dyn iyakki_js::modules::PasskeyHost>> = Arc::new(Mutex::new(PagePasskeys::new(svc, card)));
    let data = iyakki_js::SiteData { passkeys: Some(host), ..Default::default() };
    let url = iyakki_web::Url::parse("https://login.example.com/start").unwrap();
    iyakki_js::LivePage::load("<body><p id=o>x</p></body>", &url, Arc::new(NoWeb), |_| true, &data, iyakki_js::Limits::default(), Vec::new()).expect("engine")
}

fn ask(p: &mut iyakki_js::LivePage, js: &str) -> String {
    p.eval("t", &format!("globalThis.out = 'pending'; (async () => {{ {js} }})().then((v) => {{ out = String(v); }}, (x) => {{ out = 'rejected ' + (x && x.name) + ': ' + (x && x.message); }});")).unwrap();
    p.advance(10);
    p.eval("t", "out").unwrap()
}

const MAKE: &str = "const c = await navigator.credentials.create({ publicKey: { challenge: new Uint8Array(32).fill(7), rp: { id: 'example.com', name: 'Ex' }, user: { id: new Uint8Array([1,2,3]), name: 'ana' }, pubKeyCredParams: [{ type: 'public-key', alg: -7 }] } }); const u = (b) => Array.from(new Uint8Array(b)).map((x) => x.toString(16).padStart(2, '0')).join(''); return [u(c.rawId), u(c.response.attestationObject), u(c.response.clientDataJSON)].join('/');";

#[test]
fn a_page_script_makes_and_uses_a_passkey_that_the_sites_own_checks_accept_after_the_card() {
    let (svc, _, log) = setup();
    let card = Arc::new(Card { answer: true, asked: Mutex::new(vec![]) });
    let mut p = page(svc, card.clone());
    let made = ask(&mut p, MAKE);
    let parts: Vec<Vec<u8>> = made.split('/').map(|h| (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect()).collect();
    assert_eq!(parts.len(), 3, "{made}");
    let reg = wa::verify_registration(&parts[1], &parts[2], &[7; 32], ORIGIN, "example.com").unwrap();
    assert_eq!(reg.credential_id, parts[0]);
    let got = ask(&mut p, "const c = await navigator.credentials.get({ publicKey: { challenge: new Uint8Array(32).fill(9), rpId: 'example.com' } }); const u = (b) => Array.from(new Uint8Array(b)).map((x) => x.toString(16).padStart(2, '0')).join(''); return [u(c.response.authenticatorData), u(c.response.clientDataJSON), u(c.response.signature)].join('/');");
    let g: Vec<Vec<u8>> = got.split('/').map(|h| (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect()).collect();
    assert_eq!(wa::verify_assertion(&reg.public_key, &g[0], &g[1], &g[2], &[9; 32], ORIGIN, "example.com", 0).unwrap(), 1);
    // the card named the page's origin, the site and (for a new passkey) the account, once per use
    assert_eq!(*card.asked.lock().unwrap(), vec![format!("create {ORIGIN} example.com ana"), format!("get {ORIGIN} example.com ")]);
    assert!(log.lock().unwrap().iter().all(|l| !l.contains("ana")), "no account name in the audit lines");
}

#[test]
fn without_the_persons_yes_nothing_is_made_and_a_page_may_not_name_another_site() {
    let (svc, store, _) = setup();
    let card = Arc::new(Card { answer: false, asked: Mutex::new(vec![]) });
    let mut p = page(svc.clone(), card.clone());
    let r = ask(&mut p, MAKE);
    assert!(r.starts_with("rejected NotAllowedError"), "{r}");
    assert!(store.get(STATE).is_err() || svc.list().unwrap().contains("No passkeys"), "nothing was saved");
    // another site's id is refused before the person is even asked
    let r = ask(&mut p, &MAKE.replace("example.com", "bank.org"));
    assert!(r.starts_with("rejected SecurityError"), "{r}");
    assert_eq!(card.asked.lock().unwrap().len(), 1, "only the first was asked");
    // the build without a card refuses everything
    let none: Arc<Mutex<dyn iyakki_js::modules::PasskeyHost>> = Arc::new(Mutex::new(PagePasskeys::new(svc, Arc::new(Never))));
    assert!(none.lock().unwrap().create(ORIGIN, &json!({"rpId": "example.com", "challenge": ch(1), "userHandle": "AQ", "userName": "a"})).unwrap_err().starts_with("NotAllowedError"));
}
