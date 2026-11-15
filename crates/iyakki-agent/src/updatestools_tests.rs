//! The update chat tools with a fake backend: status is free, the check asks every time, administrators only, and nothing is installed.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::store::updatecheck::{CheckSettings, Kind, Mode, Report, Update};
use crate::updatestools::*;

struct Fake {
    admin: bool,
    cfg: Mutex<CheckSettings>,
    last: Mutex<Option<Report>>,
    fresh: Report,
    metered: Option<bool>,
    checks: Mutex<u32>,
    opened: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn running(&self) -> (String, String) {
        ("0.9.0".into(), "desktop".into())
    }
    fn settings(&self) -> CheckSettings {
        self.cfg.lock().unwrap().clone()
    }
    fn report(&self) -> Option<Report> {
        self.last.lock().unwrap().clone()
    }
    fn metered(&self) -> Option<bool> {
        self.metered
    }
    fn may_check(&self) -> bool {
        self.admin
    }
    fn check(&self) -> Report {
        *self.checks.lock().unwrap() += 1;
        *self.last.lock().unwrap() = Some(self.fresh.clone());
        self.fresh.clone()
    }
    fn now(&self) -> i64 {
        10_000
    }
    fn save_mode(&self, mode: Mode) -> Result<(), String> {
        self.cfg.lock().unwrap().mode = mode;
        Ok(())
    }
    fn open_bundle(&self, path: &str) -> Result<String, String> {
        self.opened.lock().unwrap().push(path.to_string());
        if path.contains("bad") { Err("the signature does not match".into()) } else { Ok("The signature is good. IYAKKI 0.10.0 (desktop) is unpacked and listed.".into()) }
    }
    fn compat_lines(&self) -> Vec<String> {
        vec!["core 0.9.0 · contract 0.1 (serves from 0.1) · client protocol 1 to 1".into(), "deprecated names: none".into()]
    }
}

fn upd(id: &str, perms: &[&str]) -> Update {
    Update { kind: Kind::App, id: id.into(), title: id.into(), from: "1.0.0".into(), to: "1.1.0".into(), source: "Stick".into(), new_permissions: perms.iter().map(|s| s.to_string()).collect(), note: String::new(), driver: false }
}

fn rig(admin: bool) -> (UpdatesTools, Arc<Fake>) {
    let f = Arc::new(Fake {
        admin,
        cfg: Mutex::new(CheckSettings::default()),
        last: Mutex::new(None),
        fresh: Report { checked_at: 9_900, updates: vec![upd("skill:notes", &["read your files"]), upd("tool:calc", &[])], errors: vec![("Mirror".into(), "no answer".into())], asked: vec![] },
        metered: Some(true),
        checks: Mutex::new(0),
        opened: Mutex::new(vec![]),
    });
    (UpdatesTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn status_is_free_and_the_check_asks_every_time_and_is_never_remembered() {
    let (t, _) = rig(true);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(READ_TOOL), Some(Risk::Safe));
    assert_eq!(risk(WRITE_TOOL), Some(Risk::Ask));
    assert!(!t.pre_approved(WRITE_TOOL) && !t.approved_always(WRITE_TOOL));
    assert!(t.grant_blocker(WRITE_TOOL, &json!({})).is_some());
    assert!(crate::agent::hard_ask(WRITE_TOOL, &json!({})).is_some());
    assert!(crate::agent::hard_ask(READ_TOOL, &json!({})).is_none());
}

#[test]
fn status_tells_the_version_the_mode_the_metered_state_and_what_is_pending_without_contacting_anything() {
    let (t, f) = rig(false);
    let s = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("IYAKKI 0.9.0 (desktop)") && s.contains("Not checked yet") && s.contains("metered") && s.contains("daily check waits"), "{s}");
    *f.last.lock().unwrap() = Some(f.fresh.clone());
    let s = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("Last checked 1 minutes ago") && s.contains("2 update(s) waiting") && s.contains("ASKS FOR NEW PERMISSIONS: read your files") && s.contains("Could not reach Mirror"), "{s}");
    assert!(s.contains("nothing was downloaded"));
    assert_eq!(*f.checks.lock().unwrap(), 0, "status contacts nothing");
}

#[test]
fn the_check_card_names_what_is_asked_and_only_an_administrator_with_checking_on_may_ask() {
    let (t, f) = rig(true);
    f.cfg.lock().unwrap().system_url = "https://updates.example.org/index.json".into();
    let c = t.preview(WRITE_TOOL, &json!({})).unwrap();
    assert!(c.contains("every store source") && c.contains("updates.example.org") && c.contains("nothing is downloaded or installed"), "{c}");
    f.cfg.lock().unwrap().mode = Mode::Off;
    assert!(t.refusal(WRITE_TOOL, &json!({})).unwrap().contains("off"));
    assert!(t.call(WRITE_TOOL, &json!({})).is_err());
    let (t2, f2) = rig(false);
    assert!(t2.refusal(WRITE_TOOL, &json!({})).unwrap().contains("administrator"));
    assert!(t2.call(WRITE_TOOL, &json!({})).is_err());
    assert_eq!(*f.checks.lock().unwrap() + *f2.checks.lock().unwrap(), 0, "a refused check contacts nobody");
}

#[test]
fn the_check_reports_what_it_found_and_keeps_the_answer_for_the_page() {
    let (t, f) = rig(true);
    let r = t.call(WRITE_TOOL, &json!({})).unwrap();
    assert!(r.contains("2 update(s) available; 1 source(s) could not be reached") && r.contains("App skill:notes") && r.contains("tool:calc"), "{r}");
    assert!(r.contains("Nothing was downloaded or installed"));
    assert_eq!(*f.checks.lock().unwrap(), 1);
    assert!(f.report().is_some());
}

#[test]
fn the_schedule_can_be_read_free_and_changed_only_by_an_administrator_after_a_card() {
    let (t, f) = rig(true);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!((risk(SETTINGS_READ_TOOL), risk(SETTINGS_WRITE_TOOL)), (Some(Risk::Safe), Some(Risk::Ask)));
    assert!(!t.pre_approved(SETTINGS_WRITE_TOOL) && !t.approved_always(SETTINGS_WRITE_TOOL) && t.grant_blocker(SETTINGS_WRITE_TOOL, &json!({})).is_some());
    let s = t.call(SETTINGS_READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("every day") && s.contains("built-in catalog is never contacted") && s.contains("No system update list address"), "{s}");
    let card = t.preview(SETTINGS_WRITE_TOOL, &json!({"how_often": "off"})).unwrap();
    assert!(card.contains("every day -> off"), "{card}");
    assert!(t.refusal(SETTINGS_WRITE_TOOL, &json!({"how_often": "daily"})).unwrap().contains("already"));
    assert!(t.refusal(SETTINGS_WRITE_TOOL, &json!({"how_often": "weekly"})).unwrap().contains("not a choice"));
    assert!(t.call(SETTINGS_WRITE_TOOL, &json!({"how_often": "on_request"})).unwrap().contains("only when I press Check now"));
    assert_eq!(f.cfg.lock().unwrap().mode, Mode::Manual);
    let (t2, f2) = rig(false);
    assert!(t2.call(SETTINGS_WRITE_TOOL, &json!({"how_often": "off"})).is_err());
    assert_eq!(f2.cfg.lock().unwrap().mode, Mode::Daily, "an ordinary user changed nothing");
    assert_eq!(*f.checks.lock().unwrap(), 0, "no contact");
}

#[test]
fn the_status_tool_also_shows_the_compat_report_and_reads_nothing_else_for_it() {
    let (t, _f) = rig(true);
    let s = t.call("updates_status", &json!({})).unwrap();
    assert!(s.contains("Compatibility:\n  core 0.9.0 · contract 0.1 (serves from 0.1) · client protocol 1 to 1\n  deprecated names: none"), "{s}");
    // the real backend's report comes from the committed tables, shows the contract and the client protocol, and says who may see peers
    let r = crate::compatreport::report(vec![]).lines(true);
    assert!(r[0].starts_with(&format!("core {}", env!("CARGO_PKG_VERSION"))) && r[0].contains("client protocol"), "{r:?}");
}

#[test]
fn the_status_tool_marks_an_update_that_needs_a_newer_iyakki() {
    let (t, f) = rig(true);
    let mut r = Report { checked_at: 9_000, ..Report::default() };
    r.updates.push(upd("skill:notes", &[]));
    r.updates[0].note = "Needs IYAKKI 0.3.0 or newer first: update IYAKKI, then this app.".into();
    *f.last.lock().unwrap() = Some(r);
    let s = t.call(READ_TOOL, &json!({})).unwrap();
    assert!(s.contains("NEEDS A NEWER IYAKKI (0.3.0 or newer): update IYAKKI first"), "{s}");
}

#[test]
fn opening_a_system_update_file_asks_every_time_shows_what_is_checked_and_never_installs() {
    let (t, f) = rig(true);
    let d = t.defs().into_iter().find(|d| d.spec.name == BUNDLE_TOOL).expect("the tool is offered");
    assert_eq!(d.risk, Risk::Ask);
    assert!(!t.pre_approved(BUNDLE_TOOL) && !t.approved_always(BUNDLE_TOOL));
    assert!(t.grant_blocker(BUNDLE_TOOL, &json!({})).is_some() && crate::agent::hard_ask(BUNDLE_TOOL, &json!({})).is_some(), "no standing allow");
    let a = json!({"path": "/media/stick/iyakki-0.10.0.tgz"});
    let card = t.preview(BUNDLE_TOOL, &a).unwrap();
    assert!(card.contains("iyakki-0.10.0.tgz.sig") && card.contains("release key") && card.contains("Nothing is installed"), "{card}");
    assert!(t.describe(BUNDLE_TOOL, &a).contains("iyakki-0.10.0.tgz"));
    let said = t.call(BUNDLE_TOOL, &a).unwrap();
    assert!(said.contains("signature is good") && said.contains("listed"), "{said}");
    assert_eq!(*f.opened.lock().unwrap(), vec!["/media/stick/iyakki-0.10.0.tgz".to_string()]);
    // the backend's refusal comes back plain; odd paths never reach the backend
    assert!(t.call(BUNDLE_TOOL, &json!({"path": "/media/bad.tgz"})).unwrap_err().contains("signature"));
    for bad in [json!({}), json!({"path": ""}), json!({"path": "/etc/passwd"}), json!({"path": "a\u{0}b.tgz"})] {
        assert!(t.refusal(BUNDLE_TOOL, &bad).is_some(), "{bad}");
        assert!(t.call(BUNDLE_TOOL, &bad).is_err());
    }
    assert_eq!(f.opened.lock().unwrap().len(), 2, "refused calls did not reach the backend");
    // only an administrator; the same name is also what the prompt explains
    let (t2, f2) = rig(false);
    assert!(t2.refusal(BUNDLE_TOOL, &a).unwrap().contains("administrator"));
    assert!(t2.call(BUNDLE_TOOL, &a).is_err() && f2.opened.lock().unwrap().is_empty());
    assert!(PROMPT_RULE.contains(BUNDLE_TOOL));
}
