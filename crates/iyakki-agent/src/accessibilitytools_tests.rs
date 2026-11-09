//! The accessibility chat tools with a fake backend: the status is free, a change always asks and never gets a session grant, bad
//! values are refused with the reason, the stored files keep the choices apart per person, and High contrast shares the theme key of
//! Settings -> Display.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::accessibility::{self as acc, Access};
use crate::accessibilitytools::*;
use crate::agent::{Risk, Tools};

struct Fake {
    a: Mutex<Access>,
    hc: Mutex<bool>,
    voice: bool,
    audits: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn access(&self) -> Access {
        self.a.lock().unwrap().clone()
    }
    fn save(&self, a: &Access) -> Result<(), String> {
        *self.a.lock().unwrap() = a.clone();
        Ok(())
    }
    fn high_contrast(&self) -> bool {
        *self.hc.lock().unwrap()
    }
    fn set_high_contrast(&self, on: bool) -> Result<(), String> {
        *self.hc.lock().unwrap() = on;
        Ok(())
    }
    fn text_level(&self) -> usize {
        2
    }
    fn voice_ready(&self) -> bool {
        self.voice
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audits.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(voice: bool) -> (AccessTools, Arc<Fake>) {
    let f = Arc::new(Fake { a: Mutex::new(Access::default()), hc: Mutex::new(false), voice, audits: Mutex::new(Vec::new()) });
    (AccessTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-access-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_status_is_free_and_the_change_always_asks_without_a_grant() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert!(matches!(risk(STATUS_TOOL), Some(Risk::Safe)));
    assert!(matches!(risk(SET_TOOL), Some(Risk::Ask)));
    assert!(t.grant_blocker(SET_TOOL, &json!({})).is_some(), "no session grant for a change of how the keyboard answers");
    assert!(!t.approved_always(SET_TOOL));
    assert!(!t.pre_approved(SET_TOOL));
}

#[test]
fn a_change_is_saved_audited_and_shown_before_and_after() {
    let (t, f) = rig(true);
    let args = json!({"screen_reader": true, "sticky_keys": true, "slow_keys_ms": 500, "high_contrast": true});
    let p = t.preview(SET_TOOL, &args).unwrap();
    assert!(p.contains("Now:") && p.contains("After:") && p.contains("slow keys 500 ms"), "{p}");
    let out = t.call(SET_TOOL, &args).unwrap();
    assert!(out.contains("screen reader on") && out.contains("high contrast on"), "{out}");
    let a = f.access();
    assert!(a.screen_reader && a.sticky_keys && a.slow_keys_ms == 500 && *f.hc.lock().unwrap());
    assert_eq!(f.audits.lock().unwrap().len(), 1);
    // untouched choices stay as they were
    t.call(SET_TOOL, &json!({"reduce_motion": true})).unwrap();
    assert!(f.access().screen_reader && f.access().reduce_motion);
}

#[test]
fn bad_values_and_empty_calls_are_refused_with_the_reason() {
    let (t, f) = rig(true);
    assert!(t.refusal(SET_TOOL, &json!({})).unwrap().contains("say what to change"));
    assert!(t.refusal(SET_TOOL, &json!({"slow_keys_ms": 421})).unwrap().contains("must be one of"));
    assert!(t.refusal(SET_TOOL, &json!({"sticky_keys": "yes"})).unwrap().contains("true or false"));
    assert!(t.call(SET_TOOL, &json!({"slow_keys_ms": 421})).is_err());
    assert_eq!(f.access(), Access::default(), "a refused call changes nothing");
}

#[test]
fn the_reader_without_a_voice_is_said_plainly_never_claimed_to_work() {
    let (t, _) = rig(false);
    let out = t.call(SET_TOOL, &json!({"screen_reader": true})).unwrap();
    assert!(out.contains("no reading voice") || out.contains("NO reading voice"), "{out}");
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("NO reading voice"));
    assert!(PROMPT_RULE.contains("cannot hear"));
}

#[test]
fn files_are_per_person_and_the_reader_choice_is_also_the_computers() {
    let d = temp("files");
    assert_eq!(acc::load(&d, "ann"), Access::default());
    acc::save(&d, "ann", &Access { screen_reader: true, large_pointer: true, ..Access::default() }).unwrap();
    assert!(acc::load(&d, "ann").large_pointer && !acc::load(&d, "bob").large_pointer);
    assert!(acc::machine_reader(&d), "the sign-in screen finds the reader on");
    assert!(acc::effective(&d, "bob").screen_reader, "another person signing in hears it too until they turn it off");
    acc::save(&d, "bob", &Access::default()).unwrap();
    assert!(!acc::machine_reader(&d), "the last person to save turned it off for the computer");
    assert!(acc::effective(&d, "ann").screen_reader, "ann's own choice still says on");
    std::fs::write(d.join("access-ann.json"), "garbage").unwrap();
    assert_eq!(acc::load(&d, "ann"), Access::default(), "a broken file means the defaults");
    std::fs::write(d.join("access-ann.json"), "{\"slow_keys_ms\": 450}").unwrap();
    assert_eq!(acc::load(&d, "ann").slow_keys_ms, 500, "an odd delay is brought to the next choice");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn high_contrast_uses_the_theme_key_of_settings_and_gives_the_old_theme_back() {
    let d = temp("hc");
    std::fs::write(d.join("ui-ann.json"), "{\"theme\":\"light\",\"size\":3}").unwrap();
    assert!(!acc::high_contrast(&d, "ann") && acc::text_level(&d, "ann") == 3);
    acc::set_high_contrast(&d, "ann", true).unwrap();
    assert_eq!(acc::theme_key(&d, "ann"), "high-contrast");
    assert_eq!(acc::text_level(&d, "ann"), 3, "the text size is not touched");
    acc::set_high_contrast(&d, "ann", false).unwrap();
    assert_eq!(acc::theme_key(&d, "ann"), "light", "back to what it was");
    let _ = std::fs::remove_dir_all(&d);
}
