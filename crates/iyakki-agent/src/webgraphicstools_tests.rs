//! The browser graphics chat tools with a fake backend: reading is free, changing asks every time and cannot be remembered.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::webgraphics::{Mode, Status};
use crate::webgraphicstools::*;

struct Fake {
    st: Mutex<Status>,
    log: Mutex<Vec<String>>,
}
impl Backend for Fake {
    fn status(&self) -> Status {
        self.st.lock().unwrap().clone()
    }
    fn set_mode(&self, m: Mode) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("set {}", m.id()));
        self.st.lock().unwrap().mode = m;
        Ok(())
    }
    fn try_hardware_again(&self) -> Result<(), String> {
        self.log.lock().unwrap().push("retry".into());
        Ok(())
    }
}

fn status(fallback: bool, read_only: Option<&str>) -> Status {
    Status { mode: Mode::Automatic, software: fallback, gpu: "1002:1681".into(), line: if fallback { "Automatic: software because the last session stopped right after the browser started on this graphics card".into() } else { "Automatic: hardware".into() }, detail: "d".into(), fallback, offer: fallback, read_only: read_only.map(String::from) }
}

fn rig(fallback: bool, ro: Option<&str>) -> (GraphicsTools, Arc<Fake>) {
    let f = Arc::new(Fake { st: Mutex::new(status(fallback, ro)), log: Mutex::new(vec![]) });
    (GraphicsTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn get_is_free_and_set_asks_every_time_and_is_never_remembered() {
    let (t, _) = rig(false, None);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("browser_graphics_get"), Some(Risk::Safe));
    assert_eq!(risk("browser_graphics_set"), Some(Risk::Ask));
    for n in WRITE_TOOLS {
        assert!(!t.pre_approved(n) && !t.approved_always(n));
        assert!(t.grant_blocker(n, &json!({"mode": "software"})).is_some(), "asks every time, never remembered");
        assert!(crate::agent::hard_ask(n, &json!({})).is_some(), "on the hard-ask list");
    }
    assert!(crate::agent::hard_ask("browser_graphics_get", &json!({})).is_none());
}

#[test]
fn the_card_says_what_changes_and_the_call_changes_it() {
    let (t, f) = rig(true, None);
    let card = t.preview("browser_graphics_set", &json!({"mode": "hardware"})).unwrap();
    assert!(card.contains("never falls back") && card.contains("freeze") && card.contains("next time the browser engine starts"), "{card}");
    let card = t.preview("browser_graphics_set", &json!({"mode": "software"})).unwrap();
    assert!(card.contains("slower") && card.contains("processor"), "{card}");
    assert!(t.describe("browser_graphics_set", &json!({"mode": "software"})).starts_with("Set Browser graphics to Software (safe)"));
    let out = t.call("browser_graphics_set", &json!({"mode": "software"})).unwrap();
    assert!(out.starts_with("Done.") && out.contains("Browser graphics:"), "{out}");
    t.call("browser_graphics_set", &json!({"mode": "try_hardware_again"})).unwrap();
    assert_eq!(*f.log.lock().unwrap(), vec!["set software".to_string(), "retry".to_string()]);
    let card = t.preview("browser_graphics_set", &json!({"mode": "try_hardware_again"})).unwrap();
    assert!(card.contains("falls back once more"));
}

#[test]
fn refusals_leave_everything_alone() {
    let (t, f) = rig(false, None);
    assert!(t.refusal("browser_graphics_set", &json!({"mode": "turbo"})).unwrap().contains("automatic"));
    assert!(t.call("browser_graphics_set", &json!({})).is_err());
    assert!(t.refusal("browser_graphics_set", &json!({"mode": "try_hardware_again"})).unwrap().contains("no fall back"), "nothing to retry");
    assert!(f.log.lock().unwrap().is_empty());
    let (t, f) = rig(false, Some("The browser graphics setting file was written by a newer IYAKKI"));
    assert!(t.refusal("browser_graphics_set", &json!({"mode": "software"})).unwrap().contains("newer IYAKKI"));
    assert!(t.call("browser_graphics_set", &json!({"mode": "software"})).is_err());
    assert!(f.log.lock().unwrap().is_empty());
}

#[test]
fn get_reports_the_state_in_plain_words() {
    let (t, _) = rig(true, None);
    let s = t.call("browser_graphics_get", &json!({})).unwrap();
    assert!(s.contains("Automatic: software because the last session stopped") && s.contains("1002:1681") && s.contains("try_hardware_again"), "{s}");
    let (t, _) = rig(false, None);
    let s = t.call("browser_graphics_get", &json!({})).unwrap();
    assert!(s.contains("Automatic: hardware") && !s.contains("try_hardware_again"), "{s}");
}
