//! The resume chat tools (F-SHELL-65) over a fake core: the status is free, the switch asks, nothing else changes.

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

#[test]
fn the_status_is_free_the_switch_asks_and_it_needs_developer_mode_to_be_active() {
    let mut cx = cx_with_admin_store("resume-tools");
    let core = Arc::new(FakeCore::new());
    cx.core = core.clone();
    let dir = temp_dir("resume-tools-d");
    let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let env = Env { dir, user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}"))) };
    let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &env));
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
    assert_eq!((risk("session_resume_status"), risk("session_resume_set")), (Risk::Safe, Risk::Ask));
    assert!(t.call("session_resume_status", &json!({})).unwrap().contains("switch off"));
    t.call("session_resume_set", &json!({"on": true})).unwrap();
    let said = t.call("session_resume_status", &json!({})).unwrap();
    assert!(said.contains("switch on") && said.contains("not active"), "the switch alone is not enough: {said}");
    core.resume_fake().dev.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(t.call("session_resume_status", &json!({})).unwrap().contains("active now"));
    t.call("session_resume_set", &json!({"on": false})).unwrap();
    assert!(t.call("session_resume_set", &json!({})).is_err());
    assert!(log.lock().unwrap().iter().filter(|l| l.contains("resume after restart")).count() == 2);
    assert!(PROMPT_RULE.contains("session_resume_set") && TOOLS.len() == 2);
}
