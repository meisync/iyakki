//! The monitor chat tools (F-KBD-08) over a fake core: listing reads only, choosing and setting ask first, only the chosen screen changes.

use std::sync::{Arc, Mutex};

use iyakki_shared::agent::{Risk};
use iyakki_shared::agent::{Tools};
use iyakki_shared::settingstools::{SettingsTools};
use iyakki_agent::settingstools::{Env};
use iyakki_core_fake::FakeCore;
use serde_json::json;

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
        name.to_string()
    }
}

fn rig(tag: &str) -> (Arc<FakeCore>, SettingsTools, Arc<Mutex<Vec<String>>>) {
    let core = Arc::new(FakeCore::new());
    core.monitors_fake().set_screens(&[("DEL-A0C5-1", "DELL U2720Q", 40), ("LGD-0001-2", "LG 27", 70)]);
    let mut cx = cx_with_admin_store(tag);
    cx.core = core.clone();
    let dir = temp_dir(&format!("{tag}-mon"));
    let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let env = Env { dir, user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}"))) };
    let t = SettingsTools::new(Arc::new(NoTools), true, entries(&crate::apps::chat_port::CxChat(&cx), &env));
    (core, t, log)
}

#[test]
fn listing_is_free_and_reads_only_and_changes_ask_every_time() {
    let (core, t, _) = rig("mon1");
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
    assert_eq!(risk("monitor_brightness"), Risk::Safe);
    assert_eq!(risk("monitor_choose"), Risk::Ask);
    assert_eq!(risk("monitor_brightness_set"), Risk::Ask);
    let said = t.call("monitor_brightness", &json!({})).unwrap();
    assert!(said.contains("DELL U2720Q") && said.contains("40%") && said.contains("LG 27") && !said.contains("CHOSEN"), "{said}");
    assert_eq!(core.monitors_fake().percent_of("DEL-A0C5-1"), Some(40));
}

#[test]
fn nothing_is_written_until_a_screen_is_chosen_and_then_only_that_screen() {
    let (core, t, log) = rig("mon2");
    let e = t.call("monitor_brightness_set", &json!({"percent": 20})).unwrap_err();
    assert!(e.contains("choose one first"), "{e}");
    assert!(t.call("monitor_choose", &json!({"id": "nope"})).is_err());
    t.call("monitor_choose", &json!({"id": "LGD-0001-2"})).unwrap();
    assert!(t.call("monitor_brightness", &json!({})).unwrap().contains("CHOSEN"));
    let r = t.call("monitor_brightness_set", &json!({"percent": 20})).unwrap();
    assert!(r.contains("20%"), "{r}");
    assert_eq!(core.monitors_fake().percent_of("LGD-0001-2"), Some(20));
    assert_eq!(core.monitors_fake().percent_of("DEL-A0C5-1"), Some(40), "the other screen is untouched");
    assert!(log.lock().unwrap().iter().any(|l| l.contains("monitor_choose LGD-0001-2")) && log.lock().unwrap().iter().any(|l| l.contains("monitor_brightness 20%")));
    t.call("monitor_choose", &json!({"id": "none"})).unwrap();
    assert!(t.call("monitor_brightness_set", &json!({"percent": 90})).is_err());
}

#[test]
fn a_percent_outside_one_to_a_hundred_is_refused_and_never_fully_dark() {
    let (core, t, _) = rig("mon3");
    t.call("monitor_choose", &json!({"id": "DEL-A0C5-1"})).unwrap();
    assert!(t.call("monitor_brightness_set", &json!({"percent": 0})).is_err());
    assert!(t.call("monitor_brightness_set", &json!({"percent": 101})).is_err());
    assert!(t.call("monitor_brightness_set", &json!({"percent": "x"})).is_err());
    assert_eq!(core.monitors_fake().percent_of("DEL-A0C5-1"), Some(40));
}

#[test]
fn a_machine_without_screens_says_so() {
    let mut cx = cx_with_admin_store("mon4");
    cx.core = Arc::new(FakeCore::new());
    let dir = temp_dir("mon4-mon");
    let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    let env = Env { dir, user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(|_, _| {}) };
    let t = SettingsTools::new(Arc::new(NoTools), true, entries(&crate::apps::chat_port::CxChat(&cx), &env));
    assert!(t.call("monitor_brightness", &json!({})).unwrap().contains("No screen"));
    assert!(t.call("monitor_choose", &json!({"id": "x"})).is_err());
}
