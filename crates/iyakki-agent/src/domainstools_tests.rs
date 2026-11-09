//! The assistant's domain tools: the list is free, every change asks every time (also in Auto mode), the firewall opening is its own
//! call with its own card, and no certificate or key text passes through the assistant.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::agent::{hard_ask, Risk, Tools};
use crate::domains::domains_tests_support::{free_port, rig, Rig};

fn tools(r: &Rig) -> DomainTools {
    DomainTools::new(Arc::new(crate::assign::tests_support::NoTools), r.mgr.clone())
}

#[test]
fn the_list_is_free_and_every_change_asks_every_time_with_no_standing_grant() {
    let r = rig("t-ask");
    let t = tools(&r);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("domains_list"), Some(Risk::Safe));
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Some(Risk::Ask), "{n}");
        assert!(hard_ask(n, &json!({})).is_some(), "{n} is on the hard-ask list: Auto mode asks too");
        assert!(t.grant_blocker(n, &json!({})).is_some(), "{n} cannot be allowed for the session");
        assert!(!t.pre_approved(n) && !t.approved_always(n));
    }
    assert!(hard_ask("domains_list", &json!({})).is_none());
    assert_eq!(crate::toolblock::group_of("domain_route_add"), crate::toolblock::Group::System);
}

#[test]
fn a_route_is_added_by_chat_with_the_checkers_words_and_a_card_that_says_what_it_does() {
    let r = rig("t-add");
    let t = tools(&r);
    let args = json!({"host": "store.local", "port": 30580});
    let card = t.preview("domain_route_add", &args).unwrap();
    assert!(card.contains("store.local/") && card.contains("port 30580 on this device") && card.contains("does not open the firewall") && card.contains("announced on the local network"), "{card}");
    assert!(t.refusal("domain_route_add", &args).is_none());
    let out = t.call("domain_route_add", &args).unwrap();
    assert!(out.contains("The listener is off"), "{out}");
    assert_eq!(r.mgr.routes().len(), 1);
    // the engine's own messages come back
    assert!(t.refusal("domain_route_add", &json!({"host": "nohost", "port": 80})).unwrap().contains("is not a host name"));
    assert!(t.refusal("domain_route_add", &json!({"host": "a.local"})).unwrap().contains("either port"));
    assert!(t.refusal("domain_route_add", &json!({"host": "a.local", "port": 80, "scheme": "https"})).unwrap().contains("needs a certificate"));
    assert!(t.refusal("domain_route_add", &json!({"host": "a.local", "port": 80, "scheme": "https", "certificate": "ghost"})).unwrap().contains("no certificate called"));
    assert!(t.refusal("domain_route_add", &json!({"host": "a.local", "port": 80, "certificate": "x"})).unwrap().contains("belongs to an https route"));
    // certificate or key text is refused and never echoed
    let secret = json!({"host": "a.local", "port": 80, "scheme": "https", "certificate": "-----BEGIN PRIVATE KEY-----\nSECRETBYTES"});
    let e = t.refusal("domain_route_add", &secret).unwrap();
    assert!(e.contains("Never send keys") && !e.contains("SECRETBYTES"), "{e}");
    // a trial certificate is made only when the person approved the call
    let https = json!({"host": "tls.local", "port": 30580, "scheme": "https", "certificate": "trial"});
    assert!(t.preview("domain_route_add", &https).unwrap().contains("self-signed trial certificate"));
    assert!(r.store.map.lock().unwrap().is_empty(), "a card (preview) makes nothing");
    t.call("domain_route_add", &https).unwrap();
    assert!(r.store.map.lock().unwrap().contains_key("domain.tls-local-chain"));
    // a cluster service by NAMESPACE/NAME
    t.call("domain_route_add", &json!({"host": "web.local", "service": "shop/web", "service_port": 3000})).unwrap();
    assert_eq!(r.mgr.namespace_of(&r.mgr.route("web.local", "/").unwrap()).as_deref(), Some("shop"));
    let l = t.call("domains_list", &json!({})).unwrap();
    assert!(l.contains("store.local/ -> port 30580 on this device") && l.contains("tls.local/") && l.contains("certificate: valid for") && l.contains("found in the cluster: iyakki-store/iyakki-store-nodeport port 8080 (NodePort 30580"), "{l}");
    assert!(!l.contains("BEGIN") && !l.contains("kube-dns"), "{l}");
    // removing
    assert!(t.preview("domain_route_remove", &json!({"host": "store.local"})).unwrap().contains("certificates stay in the vault"));
    t.call("domain_route_remove", &json!({"host": "store.local"})).unwrap();
    assert!(t.refusal("domain_route_remove", &json!({"host": "store.local"})).unwrap().contains("there is no route"));
}

#[test]
fn switching_the_listener_on_and_opening_the_firewall_are_two_calls_with_two_cards() {
    let r = rig("t-listen");
    let t = tools(&r);
    let (http, https) = (free_port(), free_port());
    let both = json!({"enabled": true, "open_firewall": true, "http_port": http, "https_port": https});
    assert!(t.refusal("domains_listener_set", &both).unwrap().contains("two steps"));
    assert!(t.refusal("domains_listener_set", &json!({"open_firewall": true})).unwrap().contains("switch it on first"));
    assert!(t.refusal("domains_listener_set", &json!({})).unwrap().contains("nothing to change"));
    let on = json!({"enabled": true, "http_port": http, "https_port": https});
    let c1 = t.preview("domains_listener_set", &on).unwrap();
    assert!(c1.contains("ON") && c1.contains("127.0.0.1") && !c1.contains("FIREWALL"), "{c1}");
    t.call("domains_listener_set", &on).unwrap();
    assert!(r.mgr.status().enabled);
    let open = json!({"open_firewall": true, "expires_hours": 3});
    let c2 = t.preview("domains_listener_set", &open).unwrap();
    assert!(c2.contains("FIREWALL") && c2.contains("LOCAL NETWORK") && c2.contains("3 hour") && c2.contains(&http.to_string()) && c2.contains("never the internet"), "{c2}");
    t.call("domains_listener_set", &open).unwrap();
    assert!(r.mgr.settings().firewall);
    // the standard ports show on the card as such, and the approval flag is the model's statement the card carries
    let std = t.preview("domains_listener_set", &json!({"http_port": 80, "https_port": 443, "standard_ports": true})).unwrap();
    assert!(std.contains("below 1024"), "{std}");
    t.call("domains_listener_set", &json!({"enabled": false})).unwrap();
    assert!(!r.mgr.settings().firewall && !r.mgr.status().running);
}
