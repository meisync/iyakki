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

// ---- account choice, user verification and security keys

struct Choosy {
    pick: Option<usize>,
    prove: bool,
    asked: Mutex<Vec<String>>,
}

impl Presence for Choosy {
    fn confirm(&self, action: &str, _origin: &str, site: &str, account: &str) -> bool {
        self.asked.lock().unwrap().push(format!("confirm {action} {site} {account}"));
        true
    }
    fn choose(&self, _origin: &str, site: &str, accounts: &[String]) -> Option<usize> {
        self.asked.lock().unwrap().push(format!("choose {site} {}", accounts.join(",")));
        self.pick
    }
    fn verify(&self, action: &str, _origin: &str, site: &str) -> bool {
        self.asked.lock().unwrap().push(format!("verify {action} {site}"));
        self.prove
    }
}

fn host_with(svc: Arc<Passkeys>, card: Arc<dyn Presence>, keys: Option<Arc<dyn SecurityKeys>>) -> PagePasskeys {
    let h = PagePasskeys::new(svc, card);
    match keys {
        Some(k) => h.with_keys(k),
        None => h,
    }
}

fn create_req(name: &str, handle: &str, extra: Value) -> Value {
    let mut v = json!({"rpId": "example.com", "challenge": ch(7), "userHandle": b64::url(handle.as_bytes()), "userName": name});
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}

fn flags_of(auth_data_b64: &str) -> u8 {
    b64::decode(auth_data_b64).unwrap()[32]
}

#[test]
fn a_site_that_names_no_account_gets_the_one_the_person_chooses_among_several_and_a_no_signs_nothing() {
    use iyakki_js::modules::PasskeyHost;
    let (svc, _, _) = setup();
    for (n, who) in ["ana", "bo"].iter().enumerate() {
        svc.register(ORIGIN, "example.com", &ch(1), &b64::url(who.as_bytes()), who, &[], true).unwrap();
        // distinct creation times: the second is the newer
        let mut st: serde_json::Value = serde_json::from_str(&svc.store.get(STATE).unwrap()).unwrap();
        st["creds"][n]["created"] = json!(10 + n as u64);
        svc.store.set(STATE, &st.to_string()).unwrap();
    }
    let card = Arc::new(Choosy { pick: Some(1), prove: false, asked: Mutex::new(vec![]) });
    let mut h = host_with(svc.clone(), card.clone(), None);
    let req = json!({"rpId": "example.com", "challenge": ch(9), "allow": []});
    let got = h.get(ORIGIN, &req).unwrap();
    assert_eq!(b64::decode(got["userHandle"].as_str().unwrap()).unwrap(), b"ana", "index 1 of the newest-first list is the older account");
    assert_eq!(*card.asked.lock().unwrap(), vec!["choose example.com bo,ana".to_string()], "no other card: the choice is the presence");
    // a no, and an index that does not exist, sign nothing
    for pick in [None, Some(5)] {
        let mut h = host_with(svc.clone(), Arc::new(Choosy { pick, prove: false, asked: Mutex::new(vec![]) }), None);
        assert!(h.get(ORIGIN, &req).unwrap_err().starts_with("NotAllowedError"));
    }
    // one account, or an allow list: the plain card as before
    let one = svc.candidates("example.com", &[]).unwrap();
    assert_eq!(one.len(), 2);
    let allow_id = b64::decode(&one[0].0).unwrap();
    let card = Arc::new(Choosy { pick: None, prove: false, asked: Mutex::new(vec![]) });
    let mut h = host_with(svc, card.clone(), None);
    h.get(ORIGIN, &json!({"rpId": "example.com", "challenge": ch(9), "allow": [b64::url(&allow_id)]})).unwrap();
    assert_eq!(*card.asked.lock().unwrap(), vec!["confirm get example.com ".to_string()]);
}

#[test]
fn a_site_that_requires_user_verification_gets_a_passkey_only_when_the_person_proved_who_they_are() {
    use iyakki_js::modules::PasskeyHost;
    let (svc, _, _) = setup();
    let ask = |prove: bool, uv: &str| {
        let card = Arc::new(Choosy { pick: None, prove, asked: Mutex::new(vec![]) });
        let mut h = host_with(svc.clone(), card.clone(), None);
        (h.create(ORIGIN, &create_req(&format!("u{prove}{uv}"), &format!("h{prove}{uv}"), json!({"uv": uv}))), card)
    };
    // required: refused without proof, and nothing was made
    let (r, card) = ask(false, "required");
    assert!(r.unwrap_err().starts_with("NotAllowedError"), "refused");
    assert!(svc.list().unwrap().contains("No passkeys"), "nothing was made");
    assert!(card.asked.lock().unwrap().iter().any(|l| l.starts_with("verify create")));
    // required with proof: made, flag set
    let (r, _) = ask(true, "required");
    let made = r.unwrap();
    let ad = |v: &Value| flags_of(&b64::url(&cbor::decode_all(&b64::decode(v["attestationObject"].as_str().unwrap()).unwrap()).unwrap().get_text("authData").unwrap().as_bytes().unwrap().to_vec()));
    assert_ne!(ad(&made) & wa::FLAG_UV, 0);
    // preferred without proof still works, with no flag; discouraged never asks
    let (r, _) = ask(false, "preferred");
    assert_eq!(ad(&r.unwrap()) & wa::FLAG_UV, 0);
    let (r, card) = ask(true, "discouraged");
    assert_eq!(ad(&r.unwrap()) & wa::FLAG_UV, 0);
    assert!(!card.asked.lock().unwrap().iter().any(|l| l.starts_with("verify")), "discouraged: no proof is asked");
    // sign-in: required without proof is refused before the passkey is used; with proof the flag is in the signed data
    let (_, card) = ask(false, "");
    drop(card);
    let mut no = host_with(svc.clone(), Arc::new(Choosy { pick: None, prove: false, asked: Mutex::new(vec![]) }), None);
    let req = json!({"rpId": "example.com", "challenge": ch(9), "allow": [], "uv": "required"});
    assert!(no.get(ORIGIN, &req).is_err(), "several accounts and nobody chooses");
    let one_id = svc.candidates("example.com", &[]).unwrap()[0].0.clone();
    let req = json!({"rpId": "example.com", "challenge": ch(9), "allow": [one_id], "uv": "required"});
    assert!(no.get(ORIGIN, &req).unwrap_err().starts_with("NotAllowedError"));
    let mut yes = host_with(svc, Arc::new(Choosy { pick: None, prove: true, asked: Mutex::new(vec![]) }), None);
    let got = yes.get(ORIGIN, &req).unwrap();
    assert_ne!(flags_of(got["authenticatorData"].as_str().unwrap()) & wa::FLAG_UV, 0);
}

/// A security key made of the software authenticator.
struct Stick {
    creds: Mutex<Vec<wa::Credential>>,
    connected: bool,
}

impl SecurityKeys for Stick {
    fn present(&self) -> bool {
        self.connected
    }
    fn make(&self, rp_id: &str, _rp_name: &str, user_id: &[u8], user_name: &str, cd_hash: &[u8; 32], _resident: bool, uv: bool, _exclude: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let made = wa::make_credential_uv(rp_id, cd_hash, user_id, user_name, &[wa::ES256], &[], &[], wa::Check { presence: true, verified: uv, required: false }, 1)?;
        let id = made.credential.id.clone();
        self.creds.lock().unwrap().push(made.credential);
        Ok((made.attestation_object, id))
    }
    fn get(&self, rp_id: &str, cd_hash: &[u8; 32], allow: &[Vec<u8>], uv: bool) -> Result<Vec<iyakki_webcrypto::ctap::KeyAssertion>, String> {
        let mut creds = self.creds.lock().unwrap();
        let ids: Vec<Vec<u8>> = wa::candidates(rp_id, allow, &creds).into_iter().map(|c| c.0).collect();
        if ids.is_empty() {
            return Err("this key has no passkey for this site".into());
        }
        ids.iter().map(|id| {
            let a = wa::get_assertion_uv(rp_id, cd_hash, allow, Some(id), &mut creds, wa::Check { presence: true, verified: uv, required: false })?;
            let name = creds.iter().find(|c| &c.id == id).map(|c| c.user_name.clone()).unwrap_or_default();
            Ok(iyakki_webcrypto::ctap::KeyAssertion { credential_id: a.credential_id, auth_data: a.auth_data, signature: a.signature, user_handle: a.user_handle, user_name: name })
        }).collect()
    }
}

#[test]
fn a_page_asking_for_a_security_key_makes_and_uses_a_credential_on_it_after_the_card_and_the_site_accepts_it() {
    use iyakki_js::modules::PasskeyHost;
    let (svc, store, _) = setup();
    let key: Arc<dyn SecurityKeys> = Arc::new(Stick { creds: Mutex::new(vec![]), connected: true });
    let card = Arc::new(Choosy { pick: Some(0), prove: false, asked: Mutex::new(vec![]) });
    let mut h = host_with(svc.clone(), card.clone(), Some(key));
    let made = h.create(ORIGIN, &create_req("ana", "user-1", json!({"attachment": "cross-platform", "uv": "required"}))).unwrap();
    assert_eq!(made["attachment"], "cross-platform");
    assert_eq!(made["transports"], json!(["usb"]));
    let att = b64::decode(made["attestationObject"].as_str().unwrap()).unwrap();
    let cd = b64::decode(made["clientDataJSON"].as_str().unwrap()).unwrap();
    let reg = wa::verify_registration(&att, &cd, &[7; 32], ORIGIN, "example.com").unwrap();
    assert_eq!(b64::url(&reg.credential_id), made["credentialId"].as_str().unwrap());
    assert!(store.map.lock().unwrap().is_empty(), "the vault holds nothing: the passkey is on the key");
    // the vault has no passkey for the site, so the sign-in goes to the key
    let got = h.get(ORIGIN, &json!({"rpId": "example.com", "challenge": ch(9), "allow": []})).unwrap();
    assert_eq!(got["attachment"], "cross-platform");
    let n = wa::verify_assertion(&reg.public_key, &b64::decode(got["authenticatorData"].as_str().unwrap()).unwrap(), &b64::decode(got["clientDataJSON"].as_str().unwrap()).unwrap(), &b64::decode(got["signature"].as_str().unwrap()).unwrap(), &[9; 32], ORIGIN, "example.com", 0).unwrap();
    assert_eq!(n, 1);
    assert_eq!(*card.asked.lock().unwrap(), vec!["confirm create example.com ana".to_string(), "confirm get example.com ".to_string()]);
    // no key connected: a plain sentence and no card; a build with no keys at all says the same
    for keys in [Some(Arc::new(Stick { creds: Mutex::new(vec![]), connected: false }) as Arc<dyn SecurityKeys>), None] {
        let card = Arc::new(Choosy { pick: None, prove: false, asked: Mutex::new(vec![]) });
        let mut h = host_with(svc.clone(), card.clone(), keys);
        let e = h.create(ORIGIN, &create_req("ana", "user-1", json!({"attachment": "cross-platform"}))).unwrap_err();
        assert!(e.starts_with("NotSupportedError"), "{e}");
        assert!(card.asked.lock().unwrap().is_empty(), "the person is not asked about something that cannot be done");
    }
    // a person who says no to the card makes nothing on the key either
    let key = Arc::new(Stick { creds: Mutex::new(vec![]), connected: true });
    let mut h = host_with(svc, Arc::new(Card { answer: false, asked: Mutex::new(vec![]) }), Some(key.clone()));
    assert!(h.create(ORIGIN, &create_req("ana", "user-1", json!({"attachment": "cross-platform"}))).unwrap_err().starts_with("NotAllowedError"));
    assert!(key.creds.lock().unwrap().is_empty());
    // the real opener with no device
    let none = HidKeys::new(Box::new(|| None));
    assert!(!none.present());
    assert!(none.get("example.com", &[0; 32], &[], false).unwrap_err().contains("no security key"));
}

mod shell_card {
    use super::*;
    use crate::browserconsent::{Answer, DEVICE_PAYLOAD};
    use std::time::Duration;

    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn services(tag: &str) -> (Arc<crate::services::Services>, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-pkcard-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (crate::services::Services::open(&d, Arc::new(Dead), Arc::new(Dead)).unwrap(), d)
    }

    #[test]
    fn the_card_names_the_page_the_site_and_the_account() {
        let (title, text) = card_text("create", "https://login.example.com", "example.com", "ana");
        assert_eq!(title, "Allow example.com to make a passkey?");
        assert!(text.contains("https://login.example.com") && text.contains("for ana") && text.contains("every time"), "{text}");
        let (title, text) = card_text("get", "https://login.example.com", "example.com", "");
        assert_eq!(title, "Allow example.com to sign in with a passkey?");
        assert!(!text.contains(" for "), "{text}");
    }

    #[test]
    fn only_allow_this_time_allows_it_is_asked_on_this_machine_and_silence_is_a_refusal() {
        let (svc, dir) = services("answer");
        let card = Arc::new(ShellCard::new(svc.clone(), "ana").with_wait(Duration::from_secs(30)));
        for (answer, expect) in [(Answer::Allow, true), (Answer::Deny, false)] {
            let c = card.clone();
            let t = std::thread::spawn(move || c.confirm("get", "https://login.example.com", "example.com", ""));
            let req = loop {
                if let Some(r) = svc.browser.consent.pending("ana").into_iter().next() {
                    break r;
                }
                std::thread::yield_now();
            };
            assert_eq!(req.payload, DEVICE_PAYLOAD, "shown over the screen, not answerable by developer-mode input");
            assert!(req.title.contains("example.com") && req.text.contains("login.example.com"));
            assert!(svc.browser.consent.pending("sam").is_empty(), "only the signed-in person sees it");
            svc.browser.consent.resolve("ana", &req.id, answer).unwrap();
            assert_eq!(t.join().unwrap(), expect);
        }
        let quick = ShellCard::new(svc.clone(), "ana").with_wait(Duration::from_millis(100));
        assert!(!quick.confirm("create", "https://login.example.com", "example.com", "ana"), "nobody answered: refused");
        assert!(svc.browser.consent.pending("ana").is_empty(), "the question went with the timeout");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_card_asks_account_by_account_and_the_first_allow_chooses_it_and_denying_all_or_silence_is_a_no() {
        let (svc, dir) = services("choose");
        let card = Arc::new(ShellCard::new(svc.clone(), "ana").with_wait(Duration::from_secs(30)));
        let accounts = vec!["bo".to_string(), "cy".to_string(), "di".to_string()];
        // deny the first, allow the second
        let (c, a) = (card.clone(), accounts.clone());
        let t = std::thread::spawn(move || c.choose("https://login.example.com", "example.com", &a));
        for (answer, who) in [(Answer::Deny, "bo"), (Answer::Allow, "cy")] {
            let req = loop {
                if let Some(r) = svc.browser.consent.pending("ana").into_iter().next() {
                    break r;
                }
                std::thread::yield_now();
            };
            assert_eq!(req.payload, DEVICE_PAYLOAD, "answerable only on this machine");
            assert!(req.title.contains(&format!("as {who}")), "{}", req.title);
            svc.browser.consent.resolve("ana", &req.id, answer).unwrap();
        }
        assert_eq!(t.join().unwrap(), Some(1));
        // denying every account is a no; so is silence
        let (c, a) = (card.clone(), accounts.clone());
        let t = std::thread::spawn(move || c.choose("https://login.example.com", "example.com", &a));
        for _ in 0..3 {
            let req = loop {
                if let Some(r) = svc.browser.consent.pending("ana").into_iter().next() {
                    break r;
                }
                std::thread::yield_now();
            };
            svc.browser.consent.resolve("ana", &req.id, Answer::Deny).unwrap();
        }
        assert_eq!(t.join().unwrap(), None);
        let quick = ShellCard::new(svc.clone(), "ana").with_wait(Duration::from_millis(100));
        assert_eq!(quick.choose("https://login.example.com", "example.com", &accounts), None);
        assert!(svc.browser.consent.pending("ana").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_page_with_the_shell_card_makes_nothing_until_the_person_allows_it() {
        let (svc, dir) = services("page");
        let (log, audit) = {
            let l = Arc::new(Mutex::new(Vec::<String>::new()));
            let l2 = l.clone();
            (l, Arc::new(move |a: &str, d: &str| l2.lock().unwrap().push(format!("{a} {d}"))) as crate::gitkeys::AuditFn)
        };
        let host = page_host(&svc, "ana", audit);
        // the vault is locked in this test: a refused card never reaches it, so the error is the refusal, not "locked"
        let (h2, s2) = (host.clone(), svc.clone());
        let t = std::thread::spawn(move || {
            let _ = &s2;
            h2.lock().unwrap().get("https://login.example.com", &json!({"rpId": "example.com", "challenge": ch(2)}))
        });
        let req = loop {
            if let Some(r) = svc.browser.consent.pending("ana").into_iter().next() {
                break r;
            }
            std::thread::yield_now();
        };
        svc.browser.consent.resolve("ana", &req.id, Answer::Deny).unwrap();
        let e = t.join().unwrap().unwrap_err();
        assert!(e.starts_with("NotAllowedError"), "{e}");
        // a site the page does not own is refused before anything is asked
        let e = host.lock().unwrap().create("https://login.example.com", &json!({"rpId": "other.org", "challenge": ch(3), "userHandle": b64::url(b"u"), "userName": "ana"})).unwrap_err();
        assert!(e.starts_with("SecurityError"), "{e}");
        assert!(svc.browser.consent.pending("ana").is_empty());
        assert!(log.lock().unwrap().is_empty(), "nothing was used, nothing is logged");
        let _ = std::fs::remove_dir_all(dir);
    }
}
