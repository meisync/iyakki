//! The Browser's accessibility choices: they follow the operating system's unless told otherwise, the chat status is free, a change
//! always asks and is audited, bad values are refused with the reason, and the file is per person.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::accessibility::Access;

struct Fake {
    own: Mutex<BrowserA11y>,
    os: Mutex<(Access, bool)>,
    page: Option<String>,
    audits: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn own(&self) -> BrowserA11y {
        self.own.lock().unwrap().clone()
    }
    fn save(&self, a: &BrowserA11y) -> Result<(), String> {
        *self.own.lock().unwrap() = a.clone();
        Ok(())
    }
    fn os(&self) -> (Access, bool) {
        self.os.lock().unwrap().clone()
    }
    fn page_summary(&self) -> Option<String> {
        self.page.clone()
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audits.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(page: Option<&str>) -> (BrowserA11yTools, Arc<Fake>) {
    let f = Arc::new(Fake { own: Mutex::new(BrowserA11y::default()), os: Mutex::new((Access::default(), false)), page: page.map(str::to_string), audits: Mutex::new(Vec::new()) });
    (BrowserA11yTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bra11y-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_browser_follows_the_operating_system_unless_told_otherwise() {
    let own = BrowserA11y::default();
    let off = resolve(&own, &Access::default(), false);
    assert!(!off.browse_mode && !off.forced_colors && !off.freeze_animation && off.min_font_px == 0);
    let on = resolve(&own, &Access { screen_reader: true, reduce_motion: true, ..Access::default() }, true);
    assert!(on.browse_mode && on.forced_colors && on.freeze_animation, "{on:?}");
    let own2 = BrowserA11y { browse_mode: Some(false), colours: Colours::Off, stop_animation: Some(false), ..own };
    let still = resolve(&own2, &Access { screen_reader: true, reduce_motion: true, ..Access::default() }, true);
    assert!(!still.browse_mode && !still.forced_colors && !still.freeze_animation, "its own word wins: {still:?}");
    assert!(resolve(&BrowserA11y { colours: Colours::On, ..BrowserA11y::default() }, &Access::default(), false).forced_colors);
}

#[test]
fn the_status_is_free_and_a_change_always_asks_without_a_grant() {
    let (t, _) = rig(Some("12 links, 3 headings"));
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert!(matches!(risk(STATUS_TOOL), Some(Risk::Safe)));
    assert!(matches!(risk(SET_TOOL), Some(Risk::Ask)));
    assert!(t.grant_blocker(SET_TOOL, &json!({})).is_some() && !t.approved_always(SET_TOOL) && !t.pre_approved(SET_TOOL));
    let s = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(s.contains("12 links, 3 headings") && s.contains("full web engine") && s.contains("minimum font size off"), "{s}");
    let (t2, _) = rig(None);
    assert!(t2.call(STATUS_TOOL, &json!({})).unwrap().contains("No page is open"));
    assert!(PROMPT_RULE.contains("cannot hear"));
}

#[test]
fn a_change_is_saved_audited_and_shown_before_and_after() {
    let (t, f) = rig(None);
    let args = json!({"browse_mode": true, "forced_colors": "always", "min_font_px": 20, "stop_animation": true});
    let p = t.preview(SET_TOOL, &args).unwrap();
    assert!(p.contains("Now:") && p.contains("After:") && p.contains("20 px"), "{p}");
    let out = t.call(SET_TOOL, &args).unwrap();
    assert!(out.contains("Saved") && out.contains("forced high-contrast page colours on"), "{out}");
    let a = f.own();
    assert_eq!((a.browse_mode, a.colours, a.min_font_px, a.stop_animation), (Some(true), Colours::On, 20, Some(true)));
    assert_eq!(f.audits.lock().unwrap().len(), 1);
    t.call(SET_TOOL, &json!({"browse_mode": "follow"})).unwrap();
    assert_eq!(f.own().browse_mode, None);
    assert_eq!(f.own().min_font_px, 20, "untouched choices stay");
}

#[test]
fn bad_values_and_empty_calls_are_refused_with_the_reason() {
    let (t, f) = rig(None);
    assert!(t.refusal(SET_TOOL, &json!({})).unwrap().contains("say what to change"));
    assert!(t.refusal(SET_TOOL, &json!({"min_font_px": 17})).unwrap().contains("must be one of"));
    assert!(t.refusal(SET_TOOL, &json!({"forced_colors": "maybe"})).unwrap().contains("follow"));
    assert!(t.refusal(SET_TOOL, &json!({"browse_mode": 3})).unwrap().contains("true, false"));
    assert!(t.call(SET_TOOL, &json!({"min_font_px": 17})).is_err());
    assert_eq!(f.own(), BrowserA11y::default(), "a refused call changes nothing");
}

#[test]
fn the_file_is_per_person_survives_garbage_and_odd_sizes_are_brought_to_a_choice() {
    let d = temp("files");
    assert_eq!(load(&d, "ann"), BrowserA11y::default());
    save(&d, "ann", &BrowserA11y { min_font_px: 24, colours: Colours::On, ..BrowserA11y::default() }).unwrap();
    assert_eq!(load(&d, "ann").min_font_px, 24);
    assert_eq!(load(&d, "bob"), BrowserA11y::default());
    std::fs::write(path(&d, "ann"), "garbage").unwrap();
    assert_eq!(load(&d, "ann"), BrowserA11y::default());
    std::fs::write(path(&d, "ann"), "{\"min_font_px\": 15}").unwrap();
    assert_eq!(load(&d, "ann").min_font_px, 18);
    // the effective choices read the OS files too
    std::fs::write(d.join("ui-ann.json"), "{\"theme\":\"high-contrast\"}").unwrap();
    assert!(effective(&d, "ann").forced_colors && !effective(&d, "bob").forced_colors);
    assert!(stamp(&d, "ann")[0].is_some());
    let _ = std::fs::remove_dir_all(&d);
}
