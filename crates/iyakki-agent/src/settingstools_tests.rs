//! The Settings chat tools over a real data folder (no fakes: they call the same service code as the pages).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::settingstools::{Env, SettingsTools};

pub(crate) struct Rig {
    pub dir: PathBuf,
    pub audits: Arc<Mutex<Vec<String>>>,
    pub env: Env,
}

pub(crate) fn rig(tag: &str, admin: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-settingstools-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    let settings = Arc::new(Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).expect("settings")));
    let env = Env { dir: dir.clone(), user: "ada".into(), admin, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
    Rig { dir, audits, env }
}

pub(crate) fn tools(r: &Rig, entries: Vec<crate::settingstools::Entry>) -> SettingsTools {
    SettingsTools::new(Arc::new(crate::assign::tests_support::NoTools), r.env.admin, entries)
}

/// Every change asks every time with no session grant; every read is free.
pub(crate) fn assert_policy(t: &SettingsTools, reads: &[&str], changes: &[&str]) {
    let defs = t.defs();
    for n in reads {
        let d = defs.iter().find(|d| d.spec.name == *n).unwrap_or_else(|| panic!("{n} missing"));
        assert_eq!(d.risk, Risk::Safe, "{n}");
        assert!(t.grant_blocker(n, &json!({})).is_none());
    }
    for n in changes {
        let d = defs.iter().find(|d| d.spec.name == *n).unwrap_or_else(|| panic!("{n} missing"));
        assert_eq!(d.risk, Risk::Ask, "{n}");
        assert!(t.grant_blocker(n, &json!({})).is_some(), "{n} must never ride on a grant");
        assert!(!t.approved_always(n), "{n}");
        assert!(!t.describe(n, &json!({})).is_empty());
    }
}

#[test]
fn views_tools_follow_the_policy_and_change_the_pages_file() {
    let r = rig("views", false);
    let t = tools(&r, crate::settingstools_views::entries(&r.env));
    assert_policy(&t, &["views_list"], &["views_make", "views_rename", "views_delete", "views_lock", "views_keys", "views_switch", "views_share"]);
    assert!(t.call("views_list", &json!({})).unwrap().contains("1. Main (showing)"));
    assert!(t.call("views_make", &json!({"name": "Docs"})).unwrap().contains("Docs"));
    assert!(t.call("views_make", &json!({"template": "media"})).unwrap().contains("Media"));
    assert!(t.call("views_make", &json!({"template": "media"})).unwrap().contains("Media 2"));
    assert!(t.call("views_make", &json!({})).is_err());
    assert!(t.call("views_rename", &json!({"view": "docs", "name": "Notes"})).is_ok());
    assert!(t.call("views_rename", &json!({"view": "2", "name": "main"})).is_err(), "name taken");
    assert!(t.call("views_lock", &json!({"view": "Notes", "locked": true})).unwrap().contains("password"));
    // the page reads the very same file
    let v = crate::views::Views::load(&r.dir, "ada");
    assert!(v.views.iter().any(|w| w.name == "Notes" && w.locked));
    assert!(t.call("views_keys", &json!({"key_set": "mac", "reset_on_lock": true})).unwrap().contains("Mac-style"));
    assert!(t.call("views_keys", &json!({})).is_err());
    let out = t.call("views_keys", &json!({"remember_layout": false, "single_apps": ["browser", "edit"]})).unwrap();
    assert!(out.contains("Window layouts: not remembered") && out.contains("One window per app: browser, edit"), "{out}");
    assert!(t.call("views_keys", &json!({"single_apps": ["../x"]})).is_err() && t.call("views_keys", &json!({"single_apps": "edit"})).is_err());
    assert!(t.call("views_list", &json!({})).unwrap().contains("One window per app: browser, edit"));
    t.call("views_keys", &json!({"remember_layout": true, "single_apps": []})).unwrap();
    // the person's own step keys: both or none, letters the shell does not use, two different keys; the page reads the same file
    let out = t.call("views_keys", &json!({"custom_prev": "j", "custom_next": "k"})).unwrap();
    assert!(out.contains("Ctrl+Alt+J (previous) / Ctrl+Alt+K (next)"), "{out}");
    assert_eq!(crate::views::Views::load(&r.dir, "ada").custom_keys, Some(crate::views::CustomKeys { prev: "KeyJ".into(), next: "KeyK".into() }));
    for bad in [json!({"custom_prev": "q", "custom_next": "k"}), json!({"custom_prev": "j", "custom_next": "j"}), json!({"custom_prev": "j"}), json!({"custom_prev": "1", "custom_next": "k"})] {
        assert!(t.call("views_keys", &bad).is_err(), "{bad}");
    }
    assert_eq!(crate::views::Views::load(&r.dir, "ada").custom_keys.map(|c| c.prev), Some("KeyJ".into()), "a refused call changes nothing");
    t.call("views_keys", &json!({"custom_prev": "", "custom_next": ""})).unwrap();
    assert_eq!(crate::views::Views::load(&r.dir, "ada").custom_keys, None);
    assert!(t.call("views_delete", &json!({"view": "Media 2"})).is_ok());
    assert!(t.call("views_delete", &json!({"view": "9"})).is_err());
    let msg = t.call("views_switch", &json!({"view": "Notes"})).unwrap();
    assert!(msg.contains("lock screen asks"), "{msg}");
    let id = crate::views::Views::load(&r.dir, "ada").find("Notes").unwrap();
    assert_eq!(crate::views::take_switch(&r.dir, "ada"), Some(id));
    assert_eq!(crate::views::take_switch(&r.dir, "ada"), None);
    let shared = t.call("views_share", &json!({"view": "Notes"})).unwrap();
    assert!(shared.contains("Notes"), "{shared}");
    assert_eq!(crate::views::take_share(&r.dir, "ada"), Some(id));
    assert_eq!(crate::views::take_share(&r.dir, "ada"), None);
    assert!(t.call("views_share", &json!({"view": "9"})).is_err());
    assert!(r.audits.lock().unwrap().iter().any(|a| a.starts_with("view.share.request")));
    assert!(r.audits.lock().unwrap().iter().any(|a| a.starts_with("view.lock")));
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn own_view_templates_are_kept_by_chat_and_make_views() {
    let r = rig("views-own", false);
    let t = tools(&r, crate::settingstools_views::entries(&r.env));
    assert_policy(&t, &[], &["views_template_save", "views_template_delete"]);
    assert!(t.call("views_template_save", &json!({"name": "Study", "tiles": ["learn", "notes"]})).unwrap().contains("learn, notes"));
    assert!(t.call("views_template_save", &json!({"name": "Work", "tiles": ["edit"]})).is_err(), "a built-in name");
    assert!(t.call("views_template_save", &json!({"name": "Bad", "tiles": ["../x"]})).is_err());
    assert!(t.call("views_list", &json!({})).unwrap().contains("Study (learn, notes)"));
    assert!(t.call("views_make", &json!({"template": "study"})).unwrap().contains("Made the view Study"));
    assert!(t.call("views_template_delete", &json!({"name": "Study"})).is_ok());
    assert!(t.call("views_template_delete", &json!({"name": "Study"})).is_err());
}

#[test]
fn backup_tools_follow_the_policy_keep_secrets_out_of_the_chat_and_stay_inside_the_home() {
    let r = rig("backup-tools", false);
    // the person's home is this test's own folder (the tools read $HOME): not the real home, which parallel runs share
    let rig_dir = r.env.dir.to_string_lossy().to_string();
    let _home = iyakki_testkit::EnvGuard::set(&[("HOME", Some(rig_dir.as_str()))]);
    let t = tools(&r, crate::settingstools_backup::entries(&r.env));
    assert_policy(&t, &["backup_status", "backup_inspect", "checkup_result_verify"], &["backup_create", "backup_restore", "backup_schedule", "edit_run_stop", "edit_run_input"]);
    let home = crate::scope::home_root(&r.env.user, &r.env.dir);
    std::fs::create_dir_all(home.join("backups")).unwrap();
    assert!(t.call("backup_status", &json!({})).unwrap().contains("A backup would hold"));
    // no passphrase through the chat: a plain file only when the call says so
    assert!(t.call("backup_create", &json!({"folder": "backups", "unencrypted": false})).unwrap_err().contains("passphrase"));
    assert!(t.call("backup_create", &json!({"folder": "../x", "unencrypted": true})).is_err());
    assert!(t.call("backup_create", &json!({"folder": "/", "unencrypted": true})).unwrap_err().contains("outside your home"));
    assert!(t.call("backup_create", &json!({"folder": "backups", "unencrypted": true, "areas": ["nonsense"]})).unwrap_err().contains("not a backup area"));
    let said = t.call("backup_create", &json!({"folder": "backups", "unencrypted": true})).unwrap();
    assert!(said.contains("not encrypted"), "{said}");
    let file = std::fs::read_dir(home.join("backups")).unwrap().next().unwrap().unwrap().file_name().to_string_lossy().into_owned();
    assert!(t.call("backup_inspect", &json!({"file": format!("backups/{file}")})).unwrap().contains("this account"));
    assert!(t.call("backup_restore", &json!({"file": format!("backups/{file}"), "areas": []})).unwrap_err().contains("name the areas"));
    assert!(t.call("backup_inspect", &json!({"file": "/etc/passwd"})).unwrap_err().contains("outside your home"));
    assert!(t.describe("backup_create", &json!({"folder": "backups", "unencrypted": true})).contains("NOT encrypted"));
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("backup.create")));
    // the scheduled backup: a passphrase never goes through the chat; an encrypted schedule names a file the person made
    assert!(t.call("backup_status", &json!({})).unwrap().contains("Scheduled backup: No scheduled backup."));
    assert!(t.call("backup_schedule", &json!({})).unwrap_err().contains("say what to set"));
    assert!(t.call("backup_schedule", &json!({"enabled": true, "folder": "backups"})).unwrap_err().contains("passphrase"), "plain files only when the call says so");
    assert!(t.call("backup_schedule", &json!({"enabled": true, "folder": "../x", "unencrypted": true})).is_err());
    assert!(t.call("backup_schedule", &json!({"enabled": true, "folder": "backups", "every_days": 40, "unencrypted": true})).unwrap_err().contains("every_days"));
    let said = t.call("backup_schedule", &json!({"enabled": true, "folder": "backups", "every_days": 3, "hour": 2, "keep": 4, "unencrypted": true})).unwrap();
    assert!(said.contains("Every 3 day(s) from 02:00") && said.contains("NOT encrypted") && said.contains("newest 4"), "{said}");
    std::fs::write(home.join("pass.txt"), "correct horse battery\n").unwrap();
    let said = t.call("backup_schedule", &json!({"passphrase_file": "pass.txt"})).unwrap();
    assert!(said.contains("encrypted with the passphrase in the file") && !said.contains("correct horse"), "{said}");
    // a share and a chain length (F-BACKUP-04, F-BACKUP-06): a bad address and a chain of one are refused, the password file is read at run time only
    assert!(t.call("backup_schedule", &json!({"share": "http://x/y"})).is_err());
    assert!(t.call("backup_schedule", &json!({"full_every": 1})).unwrap_err().contains("full_every"), "a chain of one is refused while the schedule is on");
    let said = t.call("backup_schedule", &json!({"enabled": true, "share": "nfs://nas/export/b", "full_every": 7, "passphrase_file": "pass.txt"})).unwrap();
    assert!(said.contains("the share nfs://nas/export/b") && said.contains("incremental links") && said.contains("after 7 files"), "{said}");
    assert!(t.describe("backup_schedule", &json!({"share": "nfs://nas/export/b"})).contains("into nfs://nas/export/b"));
    assert!(t.describe("backup_schedule", &json!({"enabled": false})).starts_with("Turn off"));
    assert!(t.call("backup_schedule", &json!({"enabled": false})).unwrap().contains("No scheduled backup"));
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("backup.schedule")));
    // the run stop is a request file the editor takes once
    assert!(t.call("edit_run_stop", &json!({})).is_ok());
    assert!(crate::settingstools_backup::take_run_stop(&r.env.dir, &r.env.user));
    assert!(!crate::settingstools_backup::take_run_stop(&r.env.dir, &r.env.user));
    // the run input is a request the editor takes once; too much or a NUL byte is refused before it is written
    assert!(t.call("edit_run_input", &json!({"text": "3\n4\n"})).unwrap().contains("4 bytes"));
    assert_eq!(crate::settingstools_backup::take_run_input(&r.env.dir, &r.env.user).as_deref(), Some("3\n4\n"));
    assert_eq!(crate::settingstools_backup::take_run_input(&r.env.dir, &r.env.user), None);
    assert!(t.call("edit_run_input", &json!({"text": "a\0b"})).is_err() && t.call("edit_run_input", &json!({"text": "x".repeat(20_000)})).is_err());
    assert!(t.call("edit_run_input", &json!({})).is_err());
    assert!(t.describe("edit_run_input", &json!({"text": "1\n2"})).contains("1 ⏎ 2"));
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("edit.run.input.request")));
    // a check-up file made on this computer verifies; an edited one does not
    let results: Vec<_> = crate::checkup::StepId::ALL.iter().map(|i| { let mut x = crate::checkup::StepResult::pending(*i); x.outcome = crate::checkup::Outcome::Pass; x }).collect();
    crate::checkup_result::save_if_complete(&r.env.dir, &results, &Default::default(), "Box", "1", 1).unwrap();
    assert!(t.call("checkup_result_verify", &json!({"file": "latest"})).unwrap().contains("signature is good"));
    let p = r.env.dir.join(crate::checkup_result::FILE_NAME);
    let bad = std::fs::read_to_string(&p).unwrap().replace("Box", "Bog");
    std::fs::write(&p, bad).unwrap();
    assert!(t.call("checkup_result_verify", &json!({"file": "latest"})).is_err());
}
