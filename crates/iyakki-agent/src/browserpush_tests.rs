use std::sync::{Arc, Mutex};

use iyakki_agent_api::PagePushHost;
use iyakki_webcrypto::{b64, webpush as wp};
use serde_json::json;

use super::*;
use crate::agent::Tools;
use crate::assign::tests_support::NoTools;
use crate::gitkeys::MemStore;

const SITE: &str = "https://news.example.org";
const SCOPE: &str = "https://news.example.org/app/";
const KEY: &str = "BKey";

#[derive(Default)]
struct Svc {
    made: Mutex<Vec<String>>,
    revoked: Mutex<Vec<String>>,
    waiting: Mutex<Vec<(String, Vec<u8>)>>,
    fail: Mutex<bool>,
}
impl PushService for Svc {
    fn create(&self, origin: &str, _key: &str) -> Result<String, String> {
        if *self.fail.lock().unwrap() {
            return Err("AbortError: the service is down".into());
        }
        let n = self.made.lock().unwrap().len() + 1;
        let e = format!("https://push.example.net/{}/{n}", origin.trim_start_matches("https://"));
        self.made.lock().unwrap().push(e.clone());
        Ok(e)
    }
    fn fetch(&self, endpoint: &str) -> Result<Vec<Vec<u8>>, String> {
        let mut w = self.waiting.lock().unwrap();
        let (mine, rest): (Vec<_>, Vec<_>) = w.drain(..).partition(|(e, _)| e == endpoint);
        *w = rest;
        Ok(mine.into_iter().map(|(_, b)| b).collect())
    }
    fn revoke(&self, endpoint: &str) {
        self.revoked.lock().unwrap().push(endpoint.to_string());
    }
}

struct Card {
    answer: bool,
    asked: Mutex<Vec<String>>,
}
impl AskOrigin for Card {
    fn allow(&self, origin: &str) -> bool {
        self.asked.lock().unwrap().push(origin.to_string());
        self.answer
    }
}

#[derive(Default)]
struct Shown(Mutex<Vec<(String, String, String)>>);
impl ShowPush for Shown {
    fn show(&self, origin: &str, title: &str, body: &str) -> Result<(), String> {
        self.0.lock().unwrap().push((origin.into(), title.into(), body.into()));
        Ok(())
    }
}

struct Rig {
    p: PagePush,
    store: Arc<MemStore>,
    svc: Arc<Svc>,
    card: Arc<Card>,
    shown: Arc<Shown>,
    log: Arc<Mutex<Vec<String>>>,
}

fn rig(answer: bool) -> Rig {
    let store = Arc::new(MemStore::default());
    let svc = Arc::new(Svc::default());
    let card = Arc::new(Card { answer, asked: Mutex::new(vec![]) });
    let shown = Arc::new(Shown::default());
    let log = Arc::new(Mutex::new(vec![]));
    let l = log.clone();
    let mut p = PagePush::new(store.clone(), svc.clone(), card.clone(), shown.clone(), Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))));
    p.now = || 5_000;
    Rig { p, store, svc, card, shown, log }
}

fn subscribe(r: &mut Rig) -> iyakki_agent_api::PagePushSub {
    r.p.register(SITE, SCOPE, "https://news.example.org/app/sw.js").unwrap();
    r.p.subscribe(SITE, SCOPE, KEY).unwrap()
}

#[test]
fn a_page_registers_is_asked_once_and_gets_a_subscription_only_it_can_open() {
    let mut r = rig(true);
    assert_eq!(r.p.permission(SITE), "prompt");
    let sub = subscribe(&mut r);
    assert_eq!(r.p.permission(SITE), "granted");
    assert!(sub.endpoint.starts_with("https://push.example.net/"));
    assert_eq!(sub.key, KEY);
    assert_eq!(b64::decode(&sub.p256dh).unwrap().len(), 65);
    assert_eq!(b64::decode(&sub.auth).unwrap().len(), 16);
    // the same key again gives the same subscription and the person is not asked again
    assert_eq!(r.p.subscribe(SITE, SCOPE, KEY).unwrap(), sub);
    assert_eq!(r.card.asked.lock().unwrap().len(), 1);
    assert_eq!(r.p.get(SITE, SCOPE), Some(sub.clone()));
    // another key needs an unsubscribe first
    assert!(r.p.subscribe(SITE, SCOPE, "BOther").unwrap_err().starts_with("InvalidStateError"));
    // the private key is in the vault entry only, never in what the page or the log sees
    let state = r.store.map.lock().unwrap().get(STATE).unwrap().clone();
    assert!(state.contains("\"private\""));
    let private = state.split("\"private\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert!(!r.log.lock().unwrap().iter().any(|l| l.contains(&private) || l.contains(&sub.auth)), "{:?}", r.log.lock().unwrap());
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("browserpush.subscribe origin=https://news.example.org service=push.example.net")));
}

#[test]
fn a_denied_site_is_blocked_without_asking_again_and_forgetting_it_asks_again() {
    let mut r = rig(false);
    r.p.register(SITE, SCOPE, "https://news.example.org/app/sw.js").unwrap();
    assert!(r.p.subscribe(SITE, SCOPE, KEY).unwrap_err().starts_with("NotAllowedError"));
    assert_eq!(r.p.permission(SITE), "denied");
    assert!(r.p.subscribe(SITE, SCOPE, KEY).unwrap_err().contains("blocked"));
    assert_eq!(r.card.asked.lock().unwrap().len(), 1, "a saved no is not asked again");
    assert_eq!(r.p.forget_site(SITE).unwrap(), 1);
    assert_eq!(r.p.permission(SITE), "prompt");
    assert!(r.p.forget_site(SITE).is_err());
}

#[test]
fn the_rules_a_browser_has_are_kept_a_secure_origin_a_registration_first_and_a_working_service() {
    let mut r = rig(true);
    assert!(r.p.register("http://plain.example.org", "http://plain.example.org/", "s.js").unwrap_err().starts_with("SecurityError"));
    assert!(r.p.subscribe(SITE, SCOPE, KEY).unwrap_err().starts_with("InvalidStateError"), "no registration yet");
    r.p.register(SITE, SCOPE, "a.js").unwrap();
    *r.svc.fail.lock().unwrap() = true;
    assert!(r.p.subscribe(SITE, SCOPE, KEY).unwrap_err().starts_with("AbortError"));
    assert!(r.p.get(SITE, SCOPE).is_none());
    // a service that gives an address that is not https is refused
    struct Bad;
    impl PushService for Bad {
        fn create(&self, _: &str, _: &str) -> Result<String, String> {
            Ok("http://insecure.example.net/x".into())
        }
        fn fetch(&self, _: &str) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![])
        }
        fn revoke(&self, _: &str) {}
    }
    let mut p = PagePush::new(r.store.clone(), Arc::new(Bad), r.card.clone(), r.shown.clone(), Arc::new(|_, _| {}));
    assert!(p.subscribe(SITE, SCOPE, KEY).unwrap_err().starts_with("AbortError"));
    // no service set up: the default says so in a sentence
    let mut none = PagePush::new(r.store.clone(), Arc::new(NoService), r.card.clone(), r.shown.clone(), Arc::new(|_, _| {}));
    assert!(none.subscribe(SITE, SCOPE, KEY).unwrap_err().contains("no push service"));
    // the number of registrations is bounded
    for i in 0..MAX_REGISTRATIONS {
        let _ = r.p.register(SITE, &format!("{SITE}/s{i}/"), "w.js");
    }
    assert!(r.p.register(SITE, &format!("{SITE}/extra/"), "w.js").unwrap_err().starts_with("QuotaExceededError"));
}

#[test]
fn unsubscribe_and_unregister_tell_the_service_and_a_locked_vault_stops_everything_in_a_sentence() {
    let mut r = rig(true);
    let sub = subscribe(&mut r);
    assert!(r.p.unsubscribe(SITE, SCOPE).unwrap());
    assert_eq!(r.svc.revoked.lock().unwrap().as_slice(), [sub.endpoint.clone()]);
    assert!(!r.p.unsubscribe(SITE, SCOPE).unwrap());
    let sub = r.p.subscribe(SITE, SCOPE, KEY).unwrap();
    assert!(r.p.unregister(SITE, SCOPE).unwrap());
    assert!(r.svc.revoked.lock().unwrap().contains(&sub.endpoint));
    assert!(!r.p.unregister(SITE, SCOPE).unwrap());
    *r.store.locked.lock().unwrap() = true;
    assert!(r.p.register(SITE, SCOPE, "a.js").unwrap_err().contains("vault is locked"));
    assert!(r.p.poll().unwrap_err().contains("vault is locked"));
}

fn push_to(r: &Rig, sub: &iyakki_agent_api::PagePushSub, payload: &[u8]) {
    let body = wp::encrypt(payload, &b64::decode(&sub.p256dh).unwrap(), &b64::decode(&sub.auth).unwrap()).unwrap();
    r.svc.waiting.lock().unwrap().push((sub.endpoint.clone(), body));
}

#[test]
fn a_message_the_service_holds_is_opened_with_the_subscriptions_own_key_and_shown_naming_the_site() {
    let mut r = rig(true);
    let sub = subscribe(&mut r);
    push_to(&r, &sub, json!({"title": "Breaking", "body": "Line one\nLine two\u{7}"}).to_string().as_bytes());
    push_to(&r, &sub, b"plain words");
    // a message for somebody else's key and a damaged one are dropped, not shown
    let other = wp::encrypt(b"x", &iyakki_webcrypto::p256::public_from_private(&[9u8; 32]).unwrap(), &[1u8; 16]).unwrap();
    r.svc.waiting.lock().unwrap().push((sub.endpoint.clone(), other));
    r.svc.waiting.lock().unwrap().push((sub.endpoint.clone(), vec![1, 2, 3]));
    assert_eq!(r.p.poll().unwrap(), 2);
    let shown = r.shown.0.lock().unwrap().clone();
    assert_eq!(shown[0], (SITE.to_string(), "Breaking".to_string(), "Line one\nLine two".to_string()));
    assert_eq!(shown[1], (SITE.to_string(), String::new(), "plain words".to_string()));
    assert!(r.log.lock().unwrap().iter().any(|l| l == "browserpush.receive shown=2 dropped=2"));
    // nothing is left at the service, so the next round shows nothing
    assert_eq!(r.p.poll().unwrap(), 0);
}

#[test]
fn a_site_that_is_not_allowed_any_more_gets_nothing_shown_and_long_text_is_cut() {
    let mut r = rig(true);
    let sub = subscribe(&mut r);
    let long = json!({"title": "t".repeat(500), "body": "b".repeat(3000)}).to_string();
    push_to(&r, &sub, long.as_bytes());
    assert_eq!(r.p.poll().unwrap(), 1);
    let (_, t, b) = r.shown.0.lock().unwrap()[0].clone();
    assert_eq!((t.len(), b.len()), (120, 1000));
    // the person takes the permission back: the message stays at the service, nothing is shown
    let mut s: serde_json::Value = serde_json::from_str(&r.store.map.lock().unwrap().get(STATE).unwrap().clone()).unwrap();
    s["perms"][SITE] = json!("denied");
    r.store.map.lock().unwrap().insert(STATE.into(), s.to_string());
    push_to(&r, &sub, b"later");
    assert_eq!(r.p.poll().unwrap(), 0);
    assert_eq!(r.svc.waiting.lock().unwrap().len(), 1);
}

#[test]
fn the_chat_lists_the_sites_without_a_key_and_forgetting_one_asks_on_a_card_every_time() {
    let mut r = rig(true);
    subscribe(&mut r);
    let p = Arc::new(r.p);
    let tools = crate::webpush::WebPushTools::new(Arc::new(NoTools), Arc::new(crate::webpush::WebPush::new(r.store.clone(), Arc::new(crate::notifyhub::transport::NoNet), Arc::new(|_, _| {})))).with_sites(p.clone());
    let list = tools.call("webpush_sites", &json!({})).unwrap();
    assert_eq!(list, "https://news.example.org: granted, subscribed through push.example.net");
    assert!(tools.pre_approved("webpush_sites"));
    assert!(!tools.pre_approved("webpush_site_forget"));
    assert!(tools.grant_blocker("webpush_site_forget", &json!({})).is_some());
    assert!(tools.refusal("webpush_site_forget", &json!({"origin": "news"})).is_some());
    assert!(tools.preview("webpush_site_forget", &json!({"origin": SITE})).unwrap().contains(SITE));
    assert_eq!(tools.call("webpush_site_forget", &json!({"origin": SITE})).unwrap(), "Forgotten (1 registrations removed).");
    assert_eq!(tools.call("webpush_sites", &json!({})).unwrap(), "No site has asked to send notifications.");
    // without page push the tools are not offered
    let plain = crate::webpush::WebPushTools::new(Arc::new(NoTools), Arc::new(crate::webpush::WebPush::new(r.store.clone(), Arc::new(crate::notifyhub::transport::NoNet), Arc::new(|_, _| {}))));
    assert!(!plain.defs().iter().any(|d| d.spec.name == "webpush_sites"));
    assert!(tools.defs().iter().any(|d| d.spec.name == "webpush_site_forget"));
}
