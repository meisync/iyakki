use std::sync::{Arc, Mutex};

use iyakki_webcrypto::{b64, p256, webpush as wp};
use serde_json::json;

use super::*;
use crate::agent::Tools;
use crate::gitkeys::MemStore;
use crate::notifyhub::transport::MailJob;

struct Push {
    posts: Mutex<Vec<HttpReq>>,
    status: Mutex<u16>,
}
impl Net for Push {
    fn post(&self, _: Option<&str>, r: &HttpReq) -> Result<u16, String> {
        self.posts.lock().unwrap().push(r.clone());
        Ok(*self.status.lock().unwrap())
    }
    fn send_mail(&self, _: Option<&str>, _: &MailJob) -> Result<(), String> {
        Err("no".into())
    }
}

struct Browser {
    private: [u8; 32],
    public: String,
    auth: [u8; 16],
}
fn browser(seed: u8) -> Browser {
    let private = [seed; 32];
    Browser { private, public: b64::url(&p256::public_from_private(&private).unwrap()), auth: [seed ^ 0x55; 16] }
}

fn setup() -> (Arc<WebPush>, Arc<MemStore>, Arc<Push>, Arc<Mutex<Vec<String>>>) {
    let store = Arc::new(MemStore::default());
    let net = Arc::new(Push { posts: Mutex::new(vec![]), status: Mutex::new(201) });
    let log = Arc::new(Mutex::new(vec![]));
    let l = log.clone();
    let mut w = WebPush::new(store.clone(), net.clone(), Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))));
    w.now = || 1_000;
    (Arc::new(w), store, net, log)
}

#[test]
fn a_saved_browser_gets_a_message_it_alone_can_open_and_the_push_service_sees_a_valid_vapid_header() {
    let (w, store, net, _) = setup();
    let b = browser(7);
    let id = w.subscribe("phone", "https://push.example.net/s/1", &b.public, &b64::url(&b.auth)).unwrap();
    assert_eq!(id, "b1");
    let rep = w.send(None, "Backup done", "12 files", 3600).unwrap();
    assert_eq!(rep, SendReport { delivered: 1, gone: vec![], failed: vec![] });
    let post = net.posts.lock().unwrap()[0].clone();
    assert_eq!(post.url, "https://push.example.net/s/1");
    let auth = &post.headers.iter().find(|(k, _)| k == "Authorization").unwrap().1;
    let claims = wp::verify_vapid(auth, &post.url, 1_000).unwrap();
    assert_eq!(b64::url(&claims.key), w.public_key().unwrap());
    let plain = wp::decrypt(&post.body, &b.private, &b.auth).unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&plain).unwrap(), json!({"title": "Backup done", "body": "12 files"}));
    assert!(!String::from_utf8_lossy(&post.body).contains("Backup"));
    // the secrets are in the vault entry only
    assert!(store.map.lock().unwrap().get(STATE).unwrap().contains("vapid"));
}

#[test]
fn a_gone_subscription_is_removed_and_a_failing_one_is_reported_not_removed() {
    let (w, _, net, _) = setup();
    for (n, seed) in [("one", 3u8), ("two", 4)] {
        let b = browser(seed);
        w.subscribe(n, &format!("https://push.example.net/s/{seed}"), &b.public, &b64::url(&b.auth)).unwrap();
    }
    *net.status.lock().unwrap() = 410;
    let rep = w.send(Some("b1"), "x", "", 60).unwrap();
    assert_eq!(rep.gone, vec!["b1".to_string()]);
    assert!(w.status().unwrap().contains("b2") && !w.status().unwrap().contains("b1 ("));
    *net.status.lock().unwrap() = 503;
    let rep = w.send(None, "x", "", 60).unwrap();
    assert_eq!((rep.delivered, rep.failed.len()), (0, 1));
    assert!(w.status().unwrap().contains("b2"));
    assert!(w.send(Some("zz"), "x", "", 1).is_err());
}

#[test]
fn bad_subscriptions_duplicates_and_the_cap_are_refused_and_nothing_secret_is_logged_or_shown() {
    let (w, _, _, log) = setup();
    let b = browser(9);
    let auth = b64::url(&b.auth);
    assert!(w.subscribe("", "https://push.example.net/s", &b.public, &auth).is_err());
    assert!(w.subscribe("p", "http://push.example.net/s", &b.public, &auth).is_err());
    assert!(w.subscribe("p", "https://push.example.net/s", "BAAA", &auth).is_err());
    w.subscribe("p", "https://push.example.net/s", &b.public, &auth).unwrap();
    assert!(w.subscribe("p", "https://push.example.net/s", &b.public, &auth).is_err(), "duplicate");
    for i in 0..MAX_SUBSCRIPTIONS {
        let _ = w.subscribe("p", &format!("https://push.example.net/n/{i}"), &b.public, &auth);
    }
    assert!(w.subscribe("p", "https://push.example.net/last", &b.public, &auth).is_err(), "cap");
    let shown = w.status().unwrap();
    let logged = log.lock().unwrap().join("\n");
    for secret in [auth.as_str(), b.public.as_str()] {
        assert!(!shown.contains(secret) && !logged.contains(secret));
    }
    assert!(!logged.contains("/s/1") && logged.contains("host=push.example.net"));
}

#[test]
fn a_locked_vault_stops_everything_with_a_plain_sentence() {
    let (w, store, _, _) = setup();
    *store.locked.lock().unwrap() = true;
    assert!(w.status().unwrap_err().contains("locked"));
    assert!(w.subscribe("p", "https://push.example.net/s", &browser(1).public, &b64::url(&[1u8; 16])).is_err());
}

#[test]
fn the_tools_ask_on_a_card_every_time_except_the_status_and_the_card_shows_the_text() {
    let (w, _, _, _) = setup();
    let t = WebPushTools::new(Arc::new(crate::assign::tests_support::NoTools), w);
    let defs = t.defs();
    for d in &defs {
        assert_eq!(d.risk == Risk::Safe, d.spec.name == STATUS, "{}", d.spec.name);
        if is_webpush_tool(&d.spec.name) {
            assert_eq!(t.pre_approved(&d.spec.name), d.spec.name == STATUS);
            assert_eq!(t.grant_blocker(&d.spec.name, &json!({})).is_some(), d.spec.name != STATUS);
            assert!(!t.approved_always(&d.spec.name));
        }
    }
    assert_eq!(defs.len(), 5);
    let card = t.preview(SEND, &json!({"title": "Deploy finished", "body": "all green"})).unwrap();
    assert!(card.contains("Deploy finished") && card.contains("all green") && card.contains("every saved browser"));
    let b = browser(5);
    let args = json!({"label": "tablet", "endpoint": "https://push.example.net/s/9", "p256dh": b.public, "auth": b64::url(&b.auth)});
    assert!(t.refusal(SUBSCRIBE, &args).is_none());
    assert!(t.preview(SUBSCRIBE, &args).unwrap().contains("push.example.net"));
    assert!(t.refusal(SUBSCRIBE, &json!({"label": "x", "endpoint": "http://x", "p256dh": "", "auth": ""})).is_some());
    assert!(t.call(SUBSCRIBE, &args).unwrap().contains("Public key for the page"));
    assert!(t.call(STATUS, &json!({})).unwrap().contains("tablet"));
    assert!(t.call(SEND, &json!({"title": "hi"})).unwrap().starts_with("Sent to 1"));
    assert_eq!(t.call(UNSUBSCRIBE, &json!({"id": "b1"})).unwrap(), "Removed.");
    assert!(PROMPT_RULE.contains("never put a password") || PROMPT_RULE.contains("Never put a password"));
}

#[test]
fn the_persons_settings_decide_whether_and_when_a_notification_is_pushed() {
    use iyakki_shared::notifyhub_sink::Severity;
    let (w, _, net, _) = setup();
    let b = browser(5);
    w.subscribe("phone", "https://push.example.net/s/5", &b.public, &b64::url(&b.auth)).unwrap();
    let sent = |n: usize| assert_eq!(net.posts.lock().unwrap().len(), n);
    // default: normal and above, no quiet hours
    assert!(matches!(w.notify(Severity::Normal, "t", "b", 1_000).unwrap(), Pushed::Sent(_)));
    sent(1);
    assert!(matches!(w.notify(Severity::Low, "t", "b", 1_000).unwrap(), Pushed::Skipped(_)));
    sent(1);
    // quiet hours 22:00-06:30 at UTC: 23:00 is inside, so a normal one is held until 06:30 and a critical one goes at once
    w.set_prefs(Prefs { quiet: "22:00-06:30".into(), ..Prefs::default() }).unwrap();
    let day = 86_400 * 100;
    let at_23 = day + 23 * 3600;
    assert_eq!(w.notify(Severity::High, "t", "b", at_23).unwrap(), Pushed::Held(day + 86_400 + 6 * 3600 + 30 * 60));
    assert!(matches!(w.notify(Severity::Critical, "t", "b", at_23).unwrap(), Pushed::Sent(_)));
    sent(2);
    // outside the quiet hours, and the zone offset moves them
    assert!(matches!(w.notify(Severity::High, "t", "b", day + 12 * 3600).unwrap(), Pushed::Sent(_)));
    w.set_prefs(Prefs { quiet: "22:00-06:30".into(), offset_min: 120, ..Prefs::default() }).unwrap();
    assert!(matches!(w.notify(Severity::High, "t", "b", day + 20 * 3600 + 60).unwrap(), Pushed::Held(_)), "22:01 local");
    // the opt-out beats everything, and bad settings are refused in words
    w.set_prefs(Prefs { on: false, ..Prefs::default() }).unwrap();
    assert!(matches!(w.notify(Severity::Critical, "t", "b", day).unwrap(), Pushed::Skipped(m) if m.contains("switched off")));
    assert!(w.set_prefs(Prefs { min_severity: "loud".into(), ..Prefs::default() }).is_err());
    assert!(w.set_prefs(Prefs { quiet: "late".into(), ..Prefs::default() }).is_err());
    assert!(w.set_prefs(Prefs { offset_min: 99_999, ..Prefs::default() }).is_err());
    assert!(w.status().unwrap().contains("Push is off"));
}

#[test]
fn the_prefs_tool_changes_only_what_is_given_and_asks_on_a_card() {
    let (w, _, _, _) = setup();
    let b = browser(6);
    w.subscribe("phone", "https://push.example.net/s/6", &b.public, &b64::url(&b.auth)).unwrap();
    let t = WebPushTools::new(Arc::new(crate::assign::tests_support::NoTools), w.clone());
    assert!(t.defs().iter().any(|d| d.spec.name == PREFS && d.risk == Risk::Ask));
    assert!(t.preview(PREFS, &json!({"quiet": "22:00-06:30"})).unwrap().contains("quiet hours 22:00-06:30"));
    let out = t.call(PREFS, &json!({"quiet": "22:00-06:30", "min_severity": "High"})).unwrap();
    assert!(out.contains("lowest severity high") && out.contains("22:00-06:30"), "{out}");
    t.call(PREFS, &json!({"on": false})).unwrap();
    assert_eq!(w.prefs().unwrap(), Prefs { on: false, min_severity: "high".into(), quiet: "22:00-06:30".into(), offset_min: 0 });
    t.call(PREFS, &json!({"quiet": "none"})).unwrap();
    assert!(w.prefs().unwrap().quiet.is_empty());
    assert!(t.call(PREFS, &json!({"min_severity": "loud"})).is_err());
}

#[test]
fn the_open_vault_registry_lets_the_hub_push_only_to_people_signed_in_with_an_open_vault_and_only_in_their_own_tenant() {
    use iyakki_shared::notifyhub_sink::Severity;
    let reg = OpenVaults::default();
    // the registry type is also the process's (`global`); a private one keeps this test away from other tests
    let reg: &'static OpenVaults = Box::leak(Box::new(reg));
    let (w, _store, net, _) = setup();
    let b = browser(9);
    w.subscribe("phone", "https://push.example.net/s/9", &b.public, &b64::url(&b.auth)).unwrap();
    let tenant = std::path::Path::new("/tenant-a");
    let sender = reg.sender(tenant);
    assert!(!sender.can_push("ada"), "nobody is signed in: nothing to push to");
    assert!(reg.register(tenant, "ada", w.clone()));
    assert!(sender.can_push("ada"));
    assert!(!sender.can_push("bob"), "bob is not signed in");
    assert!(!reg.sender(std::path::Path::new("/tenant-b")).can_push("ada"), "another tenant's hub never reaches this vault");
    sender.push("ada", Severity::High, "Disk full", "the data volume is full", 1_000).unwrap();
    assert_eq!(net.posts.lock().unwrap().len(), 1);
    assert!(sender.push("bob", Severity::High, "x", "y", 1_000).unwrap_err().contains("locked"));
    reg.forget(tenant, "ada");
    assert!(!sender.can_push("ada"), "the sign-out locks the vault: no push");
    assert!(sender.push("ada", Severity::High, "x", "y", 1_000).is_err());
}

#[test]
fn the_registry_is_bounded() {
    let reg = OpenVaults::default();
    let (w, ..) = setup();
    let dir = std::path::Path::new("/t");
    for i in 0..OpenVaults::MAX {
        assert!(reg.register(dir, &format!("p{i}"), w.clone()));
    }
    assert!(!reg.register(dir, "one-too-many", w.clone()));
    assert!(reg.register(dir, "p0", w.clone()), "a person already in it can be registered again");
}

#[test]
fn the_link_tool_asks_every_time_refuses_without_the_owners_name_and_gives_a_one_time_address_with_it() {
    let (w, ..) = setup();
    let dir = std::env::temp_dir().join(format!("iyakki-webpush-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let reg: &'static OpenVaults = Box::leak(Box::new(OpenVaults::default()));
    reg.register(&dir, "ada", w.clone());
    let pages = Arc::new(crate::webpush_page::PushPages::new(&dir, "/push", reg));
    let t = WebPushTools::new(Arc::new(crate::assign::tests_support::NoTools), w.clone());
    assert!(!t.defs().iter().any(|d| d.spec.name == LINK), "without the pages the tool is not offered");
    let t = t.with_link(pages, "ada");
    let d = t.defs().into_iter().find(|d| d.spec.name == LINK).unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(t.grant_blocker(LINK, &json!({})).is_some() && !t.pre_approved(LINK));
    assert!(t.preview(LINK, &json!({})).unwrap().contains("one-time link"));
    assert!(t.call(LINK, &json!({})).unwrap_err().contains("public HTTPS name"), "unset by default");
    std::fs::write(dir.join(crate::webpush_page::CONFIG_FILE), r#"{"public_name":"notify.example.org"}"#).unwrap();
    let said = t.call(LINK, &json!({})).unwrap();
    assert!(said.contains("https://notify.example.org/push/subscribe?t="), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}
