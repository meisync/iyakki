//! The encrypted-drive chat tools with a fake backend: the status is free, unlocking and locking ask every time and never get a
//! standing allow, the assistant has no argument for a password, and the request file carries a partition name only.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::disksunlocktools::*;

struct Fake {
    rows: Vec<Row>,
    asked: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn rows(&self) -> Vec<Row> {
        self.rows.clone()
    }
    fn request_unlock(&self, part: &str) -> Result<String, String> {
        if part == "sdz9" {
            return Err("sdz9: there is no such partition".into());
        }
        self.asked.lock().unwrap().push(part.into());
        Ok("The password box is waiting in the DISKS tile.".into())
    }
    fn lock(&self, part: &str) -> Result<String, String> {
        Ok(format!("{part} is locked again."))
    }
}

struct Base;
impl Tools for Base {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

fn tools() -> (DisksCryptTools, Arc<Fake>) {
    let f = Arc::new(Fake {
        rows: vec![
            Row { part: "sdb1".into(), drive: "ST2000DM008".into(), kind: "LUKS2".into(), unlocked: false, open_at: None, read_only: false, blocked: None },
            Row { part: "nvme1n1p2".into(), drive: "WD Black".into(), kind: "BitLocker".into(), unlocked: true, open_at: Some("/home/a/media/Win".into()), read_only: true, blocked: None },
            Row { part: "sdc1".into(), drive: "Stick".into(), kind: "LUKS1".into(), unlocked: false, open_at: None, read_only: false, blocked: Some("the cryptsetup program is not installed".into()) },
        ],
        asked: Mutex::new(vec![]),
    });
    (DisksCryptTools::new(Arc::new(Base), f.clone()), f)
}

#[test]
fn status_is_free_and_says_what_is_locked_unlocked_and_blocked() {
    let (t, _) = tools();
    let risk = |n: &str| t.defs().into_iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(STATUS_TOOL), Risk::Safe);
    let s = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(s.contains("sdb1 on ST2000DM008: LUKS2 locked"), "{s}");
    assert!(s.contains("unlocked, open at /home/a/media/Win (read-only)"), "{s}");
    assert!(s.contains("locked (the cryptsetup program is not installed)"), "{s}");
}

#[test]
fn unlock_and_lock_ask_every_time_and_no_standing_allow_applies() {
    let (t, f) = tools();
    for n in [UNLOCK_TOOL, LOCK_TOOL] {
        assert_eq!(t.defs().into_iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Ask, "{n}");
        assert!(t.grant_blocker(n, &json!({"partition": "sdb1"})).is_some(), "{n}");
        assert!(!t.approved_always(n), "{n}");
        assert!(!t.pre_approved(n), "{n}");
        assert!(crate::agent::hard_ask(n, &json!({"partition": "sdb1"})).is_some(), "{n}: Auto mode asks too");
    }
    assert_eq!(t.describe(UNLOCK_TOOL, &json!({"partition": "sdb1"})), "Unlock the encrypted drive sdb1 (you type the password on the screen, read-only first)");
    assert!(t.preview(UNLOCK_TOOL, &json!({"partition": "sdb1"})).unwrap().contains("never sees what you type"));
    let r = t.call(UNLOCK_TOOL, &json!({"partition": "sdb1"})).unwrap();
    assert!(r.contains("DISKS"), "{r}");
    assert_eq!(f.asked.lock().unwrap().as_slice(), ["sdb1"]);
    assert_eq!(t.call(LOCK_TOOL, &json!({"partition": "sdb1"})).unwrap(), "sdb1 is locked again.");
    assert!(t.call(UNLOCK_TOOL, &json!({"partition": "sdz9"})).is_err());
}

#[test]
fn there_is_no_way_to_hand_the_assistant_a_password_and_odd_names_are_refused() {
    let (t, f) = tools();
    let d = t.defs().into_iter().find(|d| d.spec.name == UNLOCK_TOOL).unwrap();
    let props = d.spec.schema["properties"].as_object().unwrap();
    assert_eq!(props.keys().collect::<Vec<_>>(), ["partition"], "the schema has no password argument");
    // a password slipped in as an extra argument goes nowhere
    t.call(UNLOCK_TOOL, &json!({"partition": "sdb1", "password": "hunter2", "key": "x"})).unwrap();
    assert_eq!(f.asked.lock().unwrap().len(), 1);
    for bad in ["", "../etc", "sdb1; rm -rf /", "a b", &"x".repeat(40)] {
        assert!(t.refusal(UNLOCK_TOOL, &json!({"partition": bad})).is_some(), "{bad:?}");
        assert!(t.refusal(LOCK_TOOL, &json!({"partition": bad})).is_some(), "{bad:?}");
    }
    assert!(t.refusal(UNLOCK_TOOL, &json!({"partition": "sdb1"})).is_none());
}

#[test]
fn the_request_file_holds_a_partition_name_and_is_taken_once() {
    let d = std::env::temp_dir().join(format!("iyakki-disks-req-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(take_request(&d, "amy"), None);
    request(&d, "amy", "sdb1");
    assert_eq!(take_request(&d, "bob"), None, "another person's shell does not take it");
    assert_eq!(take_request(&d, "amy").as_deref(), Some("sdb1"));
    assert_eq!(take_request(&d, "amy"), None);
    // a hostile file content is dropped
    std::fs::write(d.join("disks-unlock-request-amy"), b"sdb1; reboot").unwrap();
    assert_eq!(take_request(&d, "amy"), None);
    let _ = std::fs::remove_dir_all(&d);
}
