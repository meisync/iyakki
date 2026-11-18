//! The last-session chat tools with a fake backend: the status is free, the copy asks every time, administrators only, and the log
//! text is never given to the model.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::sessionlogtools::*;

struct Fake {
    admin: bool,
    note: Mutex<Note>,
    copies: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn note(&self) -> Note {
        self.note.lock().unwrap().clone()
    }
    fn may_copy(&self) -> bool {
        self.admin
    }
    fn copy(&self, name: &str) -> Result<PathBuf, String> {
        self.copies.lock().unwrap().push(name.to_string());
        Ok(PathBuf::from("/home/amy/iyakki-session-log-20261112-0900.log"))
    }
}

struct Base;
impl Tools for Base {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("base: unknown {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

fn rig(admin: bool, note: Note) -> (SessionLogTools, Arc<Fake>) {
    let f = Arc::new(Fake { admin, note: Mutex::new(note), copies: Mutex::new(vec![]) });
    (SessionLogTools::new(Arc::new(Base), f.clone()), f)
}

fn crashed() -> Note {
    Note { unclean: Some(("2026-11-12 08:00:00".into(), 420, "BUG: unable to handle page fault".into(), false)), last_how: Some("unclean".into()), log: Some("kmsg-crash-aaaa1111.log".into()) }
}

#[test]
fn the_status_is_free_and_says_how_the_last_session_ended() {
    let (t, _) = rig(false, crashed());
    let d = t.defs();
    assert_eq!(d.iter().find(|d| d.spec.name == STATUS_TOOL).unwrap().risk, Risk::Safe);
    assert_eq!(d.iter().find(|d| d.spec.name == COPY_TOOL).unwrap().risk, Risk::Ask);
    assert!(t.preview(STATUS_TOOL, &json!({})).is_none() && t.refusal(STATUS_TOOL, &json!({})).is_none());
    let s = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(s.contains("ended unexpectedly") && s.contains("2026-11-12 08:00:00") && s.contains("420 s") && s.contains("BUG: unable to handle page fault") && s.contains("is kept"), "{s}");
    // the person cleared the note: it is a history line now
    let (t, _) = rig(false, Note { unclean: None, last_how: Some("unclean".into()), log: Some("kmsg-crash-aaaa1111.log".into()) });
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("note about it was cleared"));
    let (t, _) = rig(false, Note { unclean: None, last_how: Some("clean".into()), log: None });
    let s = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(s.contains("ended cleanly") && s.contains("No kernel log is kept"), "{s}");
    let (t, _) = rig(false, Note::default());
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("No session history"));
}

#[test]
fn the_copy_asks_every_time_for_administrators_only_and_never_returns_the_log_text() {
    let (t, f) = rig(true, crashed());
    let card = t.preview(COPY_TOOL, &json!({})).expect("a card is shown");
    assert!(card.contains("private file in your home folder") && card.contains("Nothing is sent anywhere"), "{card}");
    assert!(!t.pre_approved(COPY_TOOL) && !t.approved_always(COPY_TOOL));
    assert!(t.grant_blocker(COPY_TOOL, &json!({})).unwrap().contains("asks every time"), "no standing grant");
    let out = t.call(COPY_TOOL, &json!({})).unwrap();
    assert!(out.contains("/home/amy/iyakki-session-log-20261112-0900.log") && !out.contains("BUG:"), "{out}");
    assert_eq!(f.copies.lock().unwrap().as_slice(), ["kmsg-crash-aaaa1111.log"], "the backend gets the kept file's name, not a path from the model");
    // the model cannot name another file: arguments are ignored
    t.call(COPY_TOOL, &json!({"name": "../../etc/passwd"})).unwrap();
    assert_eq!(f.copies.lock().unwrap().last().unwrap(), "kmsg-crash-aaaa1111.log");
}

#[test]
fn a_person_who_is_not_an_administrator_or_a_session_without_a_log_gets_a_plain_refusal_and_nothing_is_copied() {
    let (t, f) = rig(false, crashed());
    assert!(t.refusal(COPY_TOOL, &json!({})).unwrap().contains("only an administrator"));
    assert!(t.call(COPY_TOOL, &json!({})).is_err());
    let (t2, f2) = rig(true, Note { unclean: None, last_how: Some("clean".into()), log: None });
    assert!(t2.refusal(COPY_TOOL, &json!({})).unwrap().contains("no kept kernel log"));
    assert!(t2.call(COPY_TOOL, &json!({})).is_err());
    assert!(f.copies.lock().unwrap().is_empty() && f2.copies.lock().unwrap().is_empty());
    // other tools go to the wrapped set
    assert!(t.call("nope", &json!({})).unwrap_err().contains("base: unknown nope"));
}
