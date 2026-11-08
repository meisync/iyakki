//! The `components_status` chat tool with a fake backend: read-only and safe, it shows what a pinned store offers against what is
//! installed, refuses lists that are not signed by the pinned key, and never downloads or applies anything.

use std::sync::{Arc, Mutex};

use crate::agent::{Risk, Tools};
use crate::componentstools::*;
use crate::pkg::{new_signing_key, public_key_hex};
use crate::store::compmanifest::{self as cm, Delta, Entry, Record};
use crate::store::components::{Installed, PinnedStore};

struct Fake {
    stores: Vec<PinnedStore>,
    inst: Installed,
    body: Mutex<Result<Vec<u8>, String>>,
    asked: Mutex<u32>,
    serial: Option<u64>,
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
}

fn entry(c: &str, v: &str, n: u64, size: u64, requires: &[(&str, &str)]) -> Entry {
    Entry { component: c.into(), version: v.into(), sha256: format!("{n:064x}"), size, requires: requires.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), channel: "stable".into(), built_from: String::new(), delta: Vec::<Delta>::new(), install_path: String::new(), added: 0 }
}

fn rig(stores: bool) -> (ComponentsTools, Arc<Fake>) {
    let key = new_signing_key().unwrap();
    let recs: Vec<Record> = vec![entry("kernel", "6.12.5", 2, 15_000_000, &[]), entry("core", "1108", 8, 80_000_000, &[("kernel", ">=6.12")])].into_iter().map(|entry| Record { entry, owner: "acme".into(), yanked: None, purged: false }).collect();
    let body = cm::build_doc("lab", 3, 1, &recs, &key).into_bytes();
    let mut inst = Installed::default();
    inst.versions.insert("kernel".into(), "6.10.2".into());
    inst.versions.insert("core".into(), "1107".into());
    let f = Arc::new(Fake { stores: if stores { vec![PinnedStore { name: "Lab store".into(), base: "http://10.0.0.5:30300".into(), pin: public_key_hex(&key) }] } else { vec![] }, inst, body: Mutex::new(Ok(body)), asked: Mutex::new(0), serial: None });
    (ComponentsTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn it_is_a_safe_read_only_tool_that_asks_nobody_and_installs_nothing() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let d = defs.iter().find(|d| d.spec.name == "components_status").unwrap();
    assert_eq!(d.risk, Risk::Safe);
    assert!(d.spec.description.contains("never downloads or installs"));
    assert_eq!(defs.len(), 1, "no tool that applies anything");
    assert!(t.preview("components_status", &serde_json::json!({})).is_none() && t.refusal("components_status", &serde_json::json!({})).is_none());
    assert!(t.approved_always("components_status") && !t.pre_approved("components_status"));
    assert!(t.call("components_apply", &serde_json::json!({})).is_err(), "applying is not offered");
    assert!(PROMPT_RULE.contains("never downloads or installs"));
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
    let f = Arc::new(Fake { stores: vec![PinnedStore { name: "S".into(), base: "http://10.0.0.5:1".into(), pin: public_key_hex(&key) }], inst: Installed::default(), body: Mutex::new(Ok(cm::build_doc("s", 2, 1, &recs, &key).into_bytes())), asked: Mutex::new(0), serial: Some(9) });
    let t = ComponentsTools::new(Arc::new(crate::assign::tests_support::NoTools), f);
    let out = t.call("components_status", &serde_json::json!({})).unwrap();
    assert!(out.contains("sent an older component list"), "{out}");
}
