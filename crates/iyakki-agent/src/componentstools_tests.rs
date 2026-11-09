//! The `components_status` chat tool with a fake backend: read-only and safe, it shows what a pinned store offers against what is
//! installed, refuses lists that are not signed by the pinned key, and never downloads or applies anything.

use std::sync::{Arc, Mutex};

use crate::agent::{Risk, Tools};
use crate::componentstools::*;
use crate::pkg::{new_signing_key, public_key_hex};
use crate::store::compapply::{Applied, Approval};
use crate::store::compmanifest::{self as cm, Delta, Entry, Record, ReleaseLabel};
use crate::store::components::{Installed, PinnedStore};

struct Fake {
    stores: Vec<PinnedStore>,
    inst: Installed,
    body: Mutex<Result<Vec<u8>, String>>,
    asked: Mutex<u32>,
    serial: Option<u64>,
    applied: Mutex<Vec<Approval>>,
}

impl Backend for Fake {
    fn stores(&self) -> Vec<PinnedStore> {
        self.stores.clone()
    }
    fn installed(&self) -> Installed {
        self.inst.clone()
    }
    fn fetch(&self, s: &PinnedStore) -> Result<cm::Doc, String> {
        *self.asked.lock().unwrap() += 1;
        match &*self.body.lock().unwrap() {
            Ok(b) => cm::verify(b, &s.pin),
            Err(e) => Err(e.clone()),
        }
    }
    fn last_serial(&self, _: &PinnedStore) -> Option<u64> {
        self.serial
    }
    fn apply_approved(&self, _: &PinnedStore, _: &str, approval: &Approval) -> Result<Applied, String> {
        self.applied.lock().unwrap().push(approval.clone());
        Ok(Applied { lines: vec!["Installed: kernel 6.12.5, core 1108.".into()], restart_machine: true, restart_core: true, installed: Installed::default(), boot_message: None })
    }
}

fn entry(c: &str, v: &str, n: u64, size: u64, requires: &[(&str, &str)]) -> Entry {
    Entry { component: c.into(), version: v.into(), sha256: format!("{n:064x}"), size, requires: requires.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), channel: "stable".into(), built_from: String::new(), delta: Vec::<Delta>::new(), install_path: String::new(), added: 0 }
}

fn rig(stores: bool) -> (ComponentsTools, Arc<Fake>) {
    let key = new_signing_key().unwrap();
    let recs: Vec<Record> = vec![entry("kernel", "6.12.5", 2, 15_000_000, &[]), entry("core", "1108", 8, 80_000_000, &[("kernel", ">=6.12")])].into_iter().map(|entry| Record { entry, owner: "acme".into(), yanked: None, purged: false }).collect();
    let body = cm::build_doc_with("lab", 3, 1, &recs, Some(&ReleaseLabel { version: "0.1.0-alpha.3".into(), variant: "pc".into() }), &key).into_bytes();
    let mut inst = Installed::default();
    inst.versions.insert("kernel".into(), "6.10.2".into());
    inst.versions.insert("core".into(), "1107".into());
    let f = Arc::new(Fake { stores: if stores { vec![PinnedStore { name: "Lab store".into(), base: "http://10.0.0.5:30300".into(), pin: public_key_hex(&key) }] } else { vec![] }, inst, body: Mutex::new(Ok(body)), asked: Mutex::new(0), serial: None, applied: Mutex::new(vec![]) });
    (ComponentsTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn it_is_a_safe_read_only_tool_that_asks_nobody_and_installs_nothing() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let d = defs.iter().find(|d| d.spec.name == "components_status").unwrap();
    assert_eq!(d.risk, Risk::Safe);
    assert!(d.spec.description.contains("never downloads or installs"));
    assert_eq!(defs.len(), 3, "the status, the apply and the channel choice");
    assert!(t.preview("components_status", &serde_json::json!({})).is_none() && t.refusal("components_status", &serde_json::json!({})).is_none());
    assert!(t.approved_always("components_status") && !t.pre_approved("components_status"));
    assert!(PROMPT_RULE.contains("never downloads or installs"));
}

#[test]
fn the_apply_tool_is_a_hard_ask_that_never_gets_a_session_grant() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let d = defs.iter().find(|d| d.spec.name == "components_apply").unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(d.spec.description.contains("Always asks") && d.spec.description.contains("never restarts"));
    assert!(!t.approved_always("components_apply") && !t.pre_approved("components_apply"));
    let why = t.grant_blocker("components_apply", &serde_json::json!({})).unwrap();
    assert!(why.contains("asks every time"), "{why}");
    assert!(t.describe("components_apply", &serde_json::json!({})).contains("Lab store") && t.describe("components_apply", &serde_json::json!({})).contains("restart"));
    assert!(crate::componentstools::PROMPT_RULE.contains("components_apply") && crate::componentstools::is_components_tool("components_apply"));
    // no store, or several without a name: refused before a card
    let (none, _) = rig(false);
    assert!(none.refusal("components_apply", &serde_json::json!({})).unwrap().contains("No private store"));
    assert!(t.refusal("components_apply", &serde_json::json!({"store": "Nobody"})).unwrap().contains("no pinned store is named"));
    assert!(t.refusal("components_apply", &serde_json::json!({})).is_none());
    assert!(t.refusal("components_apply", &serde_json::json!({"channel": "Bad Channel"})).unwrap().contains("not a channel name"));
}

#[test]
fn the_card_shows_the_plan_and_only_approving_that_card_installs_it() {
    let (t, f) = rig(true);
    // a call with no card shown installs nothing
    let e = t.call("components_apply", &serde_json::json!({})).unwrap_err();
    assert!(e.contains("No plan was shown"), "{e}");
    assert!(f.applied.lock().unwrap().is_empty());
    // the card is the review: what changes, sizes, restart, what is kept
    let card = t.preview("components_apply", &serde_json::json!({})).unwrap();
    for want in ["kernel 6.10.2 -> 6.12.5", "core 1107 -> 1108", "14.3 MB", "restart", "IYAKKI (previous version)", "IYAKKI 0.1.0-alpha.3 (pc)"] {
        assert!(card.contains(want), "{want}\n{card}");
    }
    assert!(f.applied.lock().unwrap().is_empty(), "showing the card applies nothing");
    // approving it: one apply, with an approval bound to that plan
    let out = t.call("components_apply", &serde_json::json!({})).unwrap();
    assert!(out.contains("Installed: kernel 6.12.5"), "{out}");
    assert_eq!(f.applied.lock().unwrap().len(), 1);
    // the same card cannot be used twice
    assert!(t.call("components_apply", &serde_json::json!({})).unwrap_err().contains("No plan was shown"));
    assert_eq!(f.applied.lock().unwrap().len(), 1);
}

#[test]
fn a_plan_that_differs_from_the_one_shown_is_refused_by_its_digest() {
    // two backends over the same pin whose lists differ: the card of the first is not an approval of the second
    let key = new_signing_key().unwrap();
    let mk = |kernel: &str, n: u64| -> (ComponentsTools, Arc<Fake>) {
        let recs: Vec<Record> = vec![Record { entry: entry("kernel", kernel, n, 15_000_000, &[]), owner: "a".into(), yanked: None, purged: false }];
        let body = cm::build_doc_with("lab", 3, 1, &recs, Some(&ReleaseLabel { version: "0.1.0-alpha.3".into(), variant: "pc".into() }), &key).into_bytes();
        let f = Arc::new(Fake { stores: vec![PinnedStore { name: "Lab store".into(), base: "http://10.0.0.5:30300".into(), pin: public_key_hex(&key) }], inst: Installed::default(), body: Mutex::new(Ok(body)), asked: Mutex::new(0), serial: None, applied: Mutex::new(vec![]) });
        (ComponentsTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
    };
    let (t, f) = mk("6.12.5", 2);
    t.preview("components_apply", &serde_json::json!({})).unwrap();
    let newer = cm::build_doc_with("lab", 4, 2, &[Record { entry: entry("kernel", "6.12.6", 3, 15_000_000, &[]), owner: "a".into(), yanked: None, purged: false }], Some(&ReleaseLabel { version: "0.1.0-alpha.3".into(), variant: "pc".into() }), &key).into_bytes();
    *f.body.lock().unwrap() = Ok(newer);
    let e = t.call("components_apply", &serde_json::json!({})).unwrap_err();
    assert!(e.contains("changed after the card was shown"), "{e}");
    assert!(f.applied.lock().unwrap().is_empty());
}

#[test]
fn it_shows_installed_offered_and_the_plan_in_plain_words() {
    let (t, f) = rig(true);
    let out = t.call("components_status", &serde_json::json!({})).unwrap();
    assert!(out.contains("Installed components: core 1107, kernel 6.10.2."), "{out}");
    assert!(out.contains("Lab store offers in channel stable: kernel 6.12.5 (14.3 MB), core 1108 (76.3 MB)."), "{out}");
    assert!(out.contains("kernel 6.10.2 -> 6.12.5") && out.contains("core 1107 -> 1108") && out.contains("A restart of the machine is needed"), "{out}");
    assert!(out.contains("Nothing has been downloaded or installed."), "{out}");
    assert_eq!(*f.asked.lock().unwrap(), 1);
    // another channel has nothing; a bad channel name is refused
    let out = t.call("components_status", &serde_json::json!({"channel": "beta"})).unwrap();
    assert!(out.contains("no system components") && out.contains("Nothing to update"), "{out}");
    assert!(t.call("components_status", &serde_json::json!({"channel": "Not A Channel!"})).unwrap_err().contains("not a channel name"));
}

#[test]
fn a_list_not_signed_by_the_pinned_key_an_unreachable_store_and_no_store_are_said_plainly() {
    let (t, f) = rig(true);
    // another key signed it
    let other = new_signing_key().unwrap();
    *f.body.lock().unwrap() = Ok(cm::build_doc("lab", 4, 1, &[], &other).into_bytes());
    let out = t.call("components_status", &serde_json::json!({})).unwrap();
    assert!(out.contains("Lab store: the store's component list is not signed by the key you confirmed"), "{out}");
    assert!(!out.contains("offers in channel"));
    *f.body.lock().unwrap() = Err("Lab store does not offer system components (it has no /v1/components.json)".into());
    assert!(t.call("components_status", &serde_json::json!({})).unwrap().contains("does not offer system components"));
    // no pinned store at all
    let (t, f) = rig(false);
    let out = t.call("components_status", &serde_json::json!({})).unwrap();
    assert!(out.contains("No private store with a pinned key") && out.contains("STORE, Settings"), "{out}");
    assert_eq!(*f.asked.lock().unwrap(), 0, "nothing is contacted");
}

#[test]
fn an_older_list_than_the_one_seen_before_is_refused() {
    let key = new_signing_key().unwrap();
    let recs: Vec<Record> = vec![Record { entry: entry("core", "1108", 8, 1000, &[]), owner: "a".into(), yanked: None, purged: false }];
    let f = Arc::new(Fake { stores: vec![PinnedStore { name: "S".into(), base: "http://10.0.0.5:1".into(), pin: public_key_hex(&key) }], inst: Installed::default(), body: Mutex::new(Ok(cm::build_doc("s", 2, 1, &recs, &key).into_bytes())), asked: Mutex::new(0), serial: Some(9), applied: Mutex::new(vec![]) });
    let t = ComponentsTools::new(Arc::new(crate::assign::tests_support::NoTools), f);
    let out = t.call("components_status", &serde_json::json!({})).unwrap();
    assert!(out.contains("sent an older component list"), "{out}");
}

#[test]
fn the_channel_is_the_computers_not_always_stable_and_choosing_it_asks() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let d = defs.iter().find(|d| d.spec.name == "components_channel_set").unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(!t.approved_always("components_channel_set") && !t.pre_approved("components_channel_set"));
    assert!(t.refusal("components_channel_set", &serde_json::json!({"channel": "weekly"})).unwrap().contains("stable, beta or nightly"));
    assert!(t.refusal("components_channel_set", &serde_json::json!({"channel": "beta"})).is_none());
    // the fake cannot keep a choice: said plainly, nothing pretended
    assert!(t.call("components_channel_set", &serde_json::json!({"channel": "beta"})).unwrap_err().contains("not available"));
    // the status names the channel it looked at
    assert!(t.call("components_status", &serde_json::json!({})).unwrap().contains("Channel on this computer: stable."));
}

#[test]
fn the_real_backend_keeps_the_channel_and_asks_for_it_when_no_channel_is_named() {
    use crate::store::compapply::{channel, channel_sentence, next_channel, set_channel};
    let d = std::env::temp_dir().join(format!("iyakki-comp-channel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(channel(&d), "stable");
    set_channel(&d, "nightly").unwrap();
    assert_eq!(channel(&d), "nightly", "persisted");
    assert!(set_channel(&d, "weekly").unwrap_err().contains("stable, beta or nightly"));
    assert_eq!(channel(&d), "nightly", "a refused choice changes nothing");
    std::fs::write(d.join(crate::store::compapply::CHANNEL_FILE), "Bad Channel\n").unwrap();
    assert_eq!(channel(&d), "stable", "a damaged file reads as the default");
    assert_eq!((next_channel("stable"), next_channel("beta"), next_channel("nightly"), next_channel("x")), ("beta", "nightly", "stable", "stable"));
    assert!(channel_sentence("beta").contains("beta") && channel_sentence("nightly").contains("newest"));
    let _ = std::fs::remove_dir_all(&d);
}
