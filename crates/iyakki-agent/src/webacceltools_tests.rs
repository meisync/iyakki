use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use iyakki_shared::webaccel::Machine;

struct Fake {
    pol: Mutex<Policy>,
    log: Mutex<Vec<String>>,
}
impl Backend for Fake {
    fn policy(&self) -> Policy {
        self.pol.lock().unwrap().clone()
    }
    fn save(&self, p: &Policy, what: &str) -> Result<(), String> {
        *self.pol.lock().unwrap() = p.clone();
        self.log.lock().unwrap().push(what.to_string());
        Ok(())
    }
    fn plan(&self, user: &str, host: &str) -> Plan {
        acc::plan(&Machine { node_usable: true, va_driver: true, ..Default::default() }, &self.pol.lock().unwrap().verdict(user, host))
    }
}

fn rig(admin: bool) -> (AccelTools, Arc<Fake>) {
    let f = Arc::new(Fake { pol: Mutex::new(Policy::default()), log: Mutex::new(vec![]) });
    (AccelTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone(), "ann", admin), f)
}

#[test]
fn reading_is_free_and_changing_asks_every_time_and_is_never_remembered() {
    let (t, _) = rig(false);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(READ_TOOL), Some(Risk::Safe));
    assert_eq!(risk(WRITE_TOOL), Some(Risk::Ask));
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL));
    assert!(t.grant_blocker(WRITE_TOOL, &json!({"on": false})).is_some());
}

#[test]
fn a_person_turns_it_off_for_herself_and_for_one_site_and_on_again() {
    let (t, f) = rig(false);
    let card = t.preview(WRITE_TOOL, &json!({"on": false})).unwrap();
    assert!(card.contains("for you") && card.contains("processor"), "{card}");
    t.call(WRITE_TOOL, &json!({"on": false, "scope": "site", "site": "Slow.Example.org"})).unwrap();
    assert_eq!(f.policy().people["ann"].sites_off, vec!["slow.example.org".to_string()]);
    let out = t.call(READ_TOOL, &json!({"site": "x.slow.example.org"})).unwrap();
    assert!(out.contains("compositing: off") && out.contains("slow.example.org"), "{out}");
    let out = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(out.contains("compositing: on"), "{out}");
    t.call(WRITE_TOOL, &json!({"on": false})).unwrap();
    assert!(f.policy().people["ann"].off);
    t.call(WRITE_TOOL, &json!({"on": true})).unwrap();
    t.call(WRITE_TOOL, &json!({"on": true, "scope": "site", "site": "slow.example.org"})).unwrap();
    assert!(f.policy().people.is_empty());
    assert_eq!(f.log.lock().unwrap().len(), 4, "every change is audited");
}

#[test]
fn only_an_administrator_changes_the_whole_computer_and_it_overrides_the_person() {
    let (t, f) = rig(false);
    assert!(t.refusal(WRITE_TOOL, &json!({"on": false, "scope": "computer"})).unwrap().contains("administrator"));
    assert!(t.call(WRITE_TOOL, &json!({"on": false, "scope": "computer"})).is_err());
    let (a, f2) = rig(true);
    a.call(WRITE_TOOL, &json!({"on": false, "scope": "computer"})).unwrap();
    assert!(f2.policy().admin_off);
    let out = a.call(READ_TOOL, &json!({})).unwrap();
    assert!(out.contains("An administrator turned it off"), "{out}");
    a.call(WRITE_TOOL, &json!({"on": true})).unwrap();
    assert!(f2.policy().admin_off, "a person's switch on does not lift the administrator's");
    assert!(!f.policy().admin_off);
}

#[test]
fn bad_arguments_are_refused_with_a_reason() {
    let (t, f) = rig(false);
    for bad in [json!({}), json!({"on": false, "scope": "everywhere"}), json!({"on": false, "scope": "site"}), json!({"on": false, "scope": "site", "site": "no spaces.org"})] {
        assert!(t.refusal(WRITE_TOOL, &bad).is_some(), "{bad}");
        assert!(t.call(WRITE_TOOL, &bad).is_err());
    }
    assert!(f.log.lock().unwrap().is_empty());
}
