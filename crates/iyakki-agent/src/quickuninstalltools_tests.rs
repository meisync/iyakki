use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{hard_ask, Risk, Tools};
use crate::quickinstall::Item;
use crate::quickuninstalltools::*;

#[derive(Default)]
struct Fake {
    log: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn rows(&self) -> Vec<(String, String, String)> {
        vec![("k3s".into(), "k3s".into(), "installed".into()), ("lemonade".into(), "Lemonade".into(), "not installed".into())]
    }
    fn review(&self, item: Item, del: bool, _deps: bool) -> Result<Vec<String>, String> {
        if item == Item::K3s && !_deps {
            return Err("k3s cannot be removed while Lemonade is installed".into());
        }
        Ok(vec![format!("Remove {}.", item.title()), if del { "ALSO DELETED, and this cannot be undone:".into() } else { "Your data is kept.".into() }])
    }
    fn uninstall(&self, item: Item, del: bool, deps: bool) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("{} {del} {deps}", item.id()));
        Ok("removed".into())
    }
    fn firmware_card(&self, nv: bool) -> Result<(Vec<String>, String), String> {
        Ok((vec![format!("Install hardware firmware (nvidia {nv})"), "licence Linux-firmware (redistributable)".into()], format!("list-{nv}")))
    }
    fn firmware_install(&self, nv: bool, list: &str) -> Result<String, String> {
        if list != format!("list-{nv}") {
            return Err("the list changed".into());
        }
        self.log.lock().unwrap().push(format!("firmware accepted+started {list}"));
        Ok("Started".into())
    }
}

fn rig() -> (QuickUninstallTools, Arc<Fake>) {
    let f = Arc::new(Fake::default());
    (QuickUninstallTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn the_status_is_free_and_the_removal_asks_every_time_and_cannot_be_remembered() {
    let (t, f) = rig();
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(READ_TOOL), Some(Risk::Safe));
    assert_eq!(risk(WRITE_TOOL), Some(Risk::Ask));
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL));
    assert!(t.grant_blocker(WRITE_TOOL, &json!({"item": "elasticsearch"})).is_some(), "never remembered for the session");
    assert!(hard_ask(WRITE_TOOL, &json!({})).is_some(), "Auto never runs it");
    assert!(hard_ask(READ_TOOL, &json!({})).is_none());
    let s = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("k3s: k3s, installed") && s.contains("lemonade: Lemonade, not installed"));
    assert!(f.log.lock().unwrap().is_empty());
}

#[test]
fn the_card_says_what_goes_and_whether_the_data_is_deleted_and_defaults_to_keeping_it() {
    let (t, f) = rig();
    let keep = t.preview(WRITE_TOOL, &json!({"item": "elasticsearch"})).unwrap();
    assert!(keep.contains("Your data is kept"));
    let del = t.preview(WRITE_TOOL, &json!({"item": "elasticsearch", "delete_data": true})).unwrap();
    assert!(del.contains("cannot be undone"));
    t.call(WRITE_TOOL, &json!({"item": "elasticsearch"})).unwrap();
    assert_eq!(f.log.lock().unwrap().as_slice(), ["elasticsearch false false"]);
}

#[test]
fn a_dependency_or_an_unknown_item_is_refused_before_any_card() {
    let (t, f) = rig();
    assert!(t.refusal(WRITE_TOOL, &json!({"item": "k3s"})).unwrap().contains("Lemonade"));
    assert!(t.refusal(WRITE_TOOL, &json!({"item": "k3s", "with_dependants": true})).is_none());
    assert!(t.refusal(WRITE_TOOL, &json!({"item": "nonsense"})).unwrap().contains("not a Quick install item"));
    assert!(t.describe(WRITE_TOOL, &json!({"item": "nonsense"})).contains("invalid"));
    assert!(f.log.lock().unwrap().is_empty());
}

#[test]
fn hardware_firmware_asks_every_time_shows_the_licences_and_accepts_only_the_list_the_card_showed() {
    let (t, f) = rig();
    let defs = t.defs();
    let d = defs.iter().find(|d| d.spec.name == FIRMWARE_TOOL).expect("the tool is offered");
    assert_eq!(d.risk, Risk::Ask);
    // there is no argument that accepts a licence on the person's behalf
    assert!(!d.spec.schema.to_string().contains("accept"), "{}", d.spec.schema);
    assert!(hard_ask(FIRMWARE_TOOL, &json!({})).is_some(), "Auto never runs it");
    assert!(!t.pre_approved(FIRMWARE_TOOL) && !t.approved_always(FIRMWARE_TOOL));
    assert!(t.grant_blocker(FIRMWARE_TOOL, &json!({})).is_some());
    // approving without a card having been shown accepts nothing
    let e = t.call(FIRMWARE_TOOL, &json!({})).unwrap_err();
    assert!(e.contains("no licence was accepted"), "{e}");
    assert!(f.log.lock().unwrap().is_empty());
    // the card carries the licence lines; the approval then covers that list
    let card = t.preview(FIRMWARE_TOOL, &json!({})).unwrap();
    assert!(card.contains("licence Linux-firmware"), "{card}");
    t.call(FIRMWARE_TOOL, &json!({})).unwrap();
    assert_eq!(f.log.lock().unwrap().as_slice(), ["firmware accepted+started list-false"]);
    // a card for another list (the NVIDIA group) is its own approval
    t.preview(FIRMWARE_TOOL, &json!({"include_nvidia": true})).unwrap();
    assert!(t.call(FIRMWARE_TOOL, &json!({"include_nvidia": false})).is_err(), "the approval was for the other list");
}

#[test]
fn the_status_tool_prompt_explains_a_failed_last_attempt() {
    assert!(PROMPT_RULE.contains("the last attempt failed") && PROMPT_RULE.contains("never try to accept one yourself"));
}
