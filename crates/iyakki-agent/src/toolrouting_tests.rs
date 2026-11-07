//! Tool routing per person: off by default, never turned back on against the administrator, damaged files moved aside, and the
//! assistant's tools ask before changing it.

use crate::agent::{hard_ask, Risk, Tools};
use crate::toolrouting::*;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tr-{tag}-{}", crate::cloudkeys::hex(&crate::cloudkeys::random::<6>().unwrap())));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig(tag: &str, user: &str) -> (ToolRoutingTools, std::path::PathBuf, Arc<AtomicBool>) {
    let d = dir(tag);
    let machine = Arc::new(AtomicBool::new(false));
    let m = machine.clone();
    (ToolRoutingTools::new(Arc::new(crate::assign::tests_support::NoTools), d.clone(), user, Arc::new(move || m.load(Ordering::Relaxed))), d, machine)
}

#[test]
fn routing_is_on_by_default_and_a_person_can_turn_it_off_for_themselves_only() {
    let d = dir("a");
    assert!(!person_off(&d, "amy") && lazy_groups(&d, "amy", false), "no file: no override");
    set_person_off(&d, "amy", true).unwrap();
    assert!(person_off(&d, "amy") && !lazy_groups(&d, "amy", false));
    assert!(!person_off(&d, "bob") && lazy_groups(&d, "bob", false), "another person's chat is not changed");
    set_person_off(&d, "amy", false).unwrap();
    assert!(lazy_groups(&d, "amy", false));
}

#[test]
fn a_person_cannot_turn_routing_back_on_against_the_machine_setting() {
    let d = dir("b");
    assert!(!lazy_groups(&d, "amy", true), "the administrator switched it off");
    set_person_off(&d, "amy", false).unwrap();
    assert!(!lazy_groups(&d, "amy", true), "the person's own `on` is not an override of the machine");
    assert!(summary(&d, "amy", true).contains("whole machine"));
}

#[test]
fn a_damaged_or_newer_file_is_moved_aside_and_never_overwritten() {
    let d = dir("c");
    let p = d.join("tool-routing-amy.json");
    std::fs::write(&p, "{ not json").unwrap();
    assert!(!person_off(&d, "amy"));
    assert!(!p.exists() && std::fs::read_to_string(d.join("tool-routing-amy.json.damaged")).unwrap() == "{ not json");
    // saving over a second damaged file keeps both
    std::fs::write(&p, "oops").unwrap();
    set_person_off(&d, "amy", true).unwrap();
    assert_eq!(std::fs::read_to_string(d.join("tool-routing-amy.json.damaged.1")).unwrap(), "oops");
    assert!(person_off(&d, "amy"));
    let newer = r#"{"version":99,"off":true,"future":1}"#;
    std::fs::write(&p, newer).unwrap();
    assert!(!person_off(&d, "amy"), "a newer file is not read");
    assert_eq!(std::fs::read_to_string(d.join("tool-routing-amy.json.newer")).unwrap(), newer);
}

#[test]
fn a_name_with_odd_characters_stays_inside_the_folder() {
    let d = dir("d");
    set_person_off(&d, "../../evil", true).unwrap();
    assert!(d.join("tool-routing-evil.json").is_file());
    assert!(!d.parent().unwrap().join("evil.json").exists());
}

#[test]
fn the_assistant_reads_freely_and_changes_only_the_persons_own_switch_after_asking() {
    let (t, d, machine) = rig("e", "amy");
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|x| x.spec.name == n).map(|x| x.risk);
    assert!(matches!(risk(READ_TOOL), Some(Risk::Safe)) && matches!(risk(WRITE_TOOL), Some(Risk::Ask)));
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL) && t.grant_blocker(WRITE_TOOL, &json!({})).is_some(), "never remembered for a session");
    assert!(hard_ask(WRITE_TOOL, &json!({})).is_none(), "an ordinary ask, not a hard one: it changes only the person's own offer list");
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("is on"));
    // a card says what will change; a wrong value is refused before anything runs
    assert!(t.preview(WRITE_TOOL, &json!({"routing": "off"})).unwrap().contains("OFF"));
    assert!(t.refusal(WRITE_TOOL, &json!({"routing": "maybe"})).is_some());
    assert!(t.call(WRITE_TOOL, &json!({})).is_err());
    assert!(!person_off(&d, "amy"), "nothing saved yet");
    t.call(WRITE_TOOL, &json!({"routing": "off"})).unwrap();
    assert!(person_off(&d, "amy") && t.call(READ_TOOL, &json!({})).unwrap().contains("off for you"));
    // the machine-wide switch is not touched by the tool and is read fresh each time
    machine.store(true, Ordering::Relaxed);
    assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("whole machine"));
    assert!(t.preview(WRITE_TOOL, &json!({"routing": "on"})).unwrap().contains("stays off"));
    // other tools pass through to the inner set
    assert!(t.call("something_else", &json!({})).is_err());
}
