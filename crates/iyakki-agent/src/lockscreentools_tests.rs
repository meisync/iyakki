//! The lock screen chat tools with a fake backend: locking asks nothing, the status is free, changing the automatic lock asks every
//! time and never gets a session grant, a guest cannot lock, and the assistant has no way to unlock.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::lockscreen::LockConfig;
use crate::lockscreentools::*;

struct Fake {
    guest: bool,
    cfg: Mutex<LockConfig>,
    locks: Mutex<u32>,
    audits: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn can_lock(&self) -> bool {
        !self.guest
    }
    fn config(&self) -> LockConfig {
        self.cfg.lock().unwrap().clone()
    }
    fn save(&self, c: &LockConfig) -> Result<(), String> {
        *self.cfg.lock().unwrap() = c.clone();
        Ok(())
    }
    fn request_lock(&self) {
        *self.locks.lock().unwrap() += 1;
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audits.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(guest: bool) -> (LockTools, Arc<Fake>) {
    let f = Arc::new(Fake { guest, cfg: Mutex::new(LockConfig::default()), locks: Mutex::new(0), audits: Mutex::new(Vec::new()) });
    (LockTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn locking_and_the_status_ask_nothing_and_changing_the_automatic_lock_asks() {
    let (t, _) = rig(false);
    let risk = |n: &str| t.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(LOCK_TOOL), Some(Risk::Safe));
    assert_eq!(risk(STATUS_TOOL), Some(Risk::Safe));
    assert_eq!(risk(SET_TOOL), Some(Risk::Ask));
    assert!(t.grant_blocker(SET_TOOL, &json!({})).is_some(), "a change never rides on a session grant");
    assert!(!t.approved_always(SET_TOOL));
    assert!(t.approved_always(LOCK_TOOL) && t.grant_blocker(LOCK_TOOL, &json!({})).is_none());
}

#[test]
fn lock_screen_asks_the_shell_to_lock_and_there_is_no_unlock_tool() {
    let (t, f) = rig(false);
    assert!(t.call(LOCK_TOOL, &json!({})).expect("locks").contains("locking"));
    assert_eq!(*f.locks.lock().unwrap(), 1);
    assert!(t.defs().iter().all(|d| !d.spec.name.contains("unlock")), "the assistant cannot open the lock");
    assert!(PROMPT_RULE.contains("never ask for the password"));
}

#[test]
fn a_guest_has_no_password_so_nothing_locks_and_nothing_changes() {
    let (t, f) = rig(true);
    assert!(t.call(LOCK_TOOL, &json!({})).is_err());
    assert!(t.call(SET_TOOL, &json!({"minutes": 10})).is_err());
    assert_eq!(*f.locks.lock().unwrap(), 0);
    assert_eq!(f.cfg.lock().unwrap().auto_min, 0);
    assert!(t.call(STATUS_TOOL, &json!({})).expect("status").contains("guest"));
}

#[test]
fn changing_the_automatic_lock_checks_the_choice_saves_it_and_audits_it() {
    let (t, f) = rig(false);
    assert!(t.call(STATUS_TOOL, &json!({})).expect("status").contains("never by itself"));
    assert!(t.refusal(SET_TOOL, &json!({"minutes": 7})).is_some(), "7 is not a choice");
    assert!(t.call(SET_TOOL, &json!({"minutes": 7})).is_err());
    assert!(t.call(SET_TOOL, &json!({})).is_err());
    assert!(t.preview(SET_TOOL, &json!({"minutes": 15})).expect("card").contains("after 15 minutes"));
    assert!(t.describe(SET_TOOL, &json!({"minutes": 15})).contains("15 minutes"));
    assert!(t.call(SET_TOOL, &json!({"minutes": 15})).expect("set").contains("15 minutes"));
    assert_eq!(f.cfg.lock().unwrap().auto_min, 15);
    assert!(f.audits.lock().unwrap().iter().any(|a| a.starts_with("settings.lockscreen") && a.contains("15")));
    assert!(t.call(SET_TOOL, &json!({"minutes": 0})).expect("off").contains("never"));
    assert_eq!(f.cfg.lock().unwrap().auto_min, 0);
}
