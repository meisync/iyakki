//! Upgrade tests of the shell's own files (v1.0.0 gate item 5): the audit log with its hash chain and the approval pins, read from
//! the alpha.1 fixtures (`tests/fixtures/upgrade/alpha1`) and the current ones. The agent crate's `upgrade_tests` does the rest.

use std::fs;
use std::path::{Path, PathBuf};

use crate::accounts::AuditLog;

fn fixtures(which: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/upgrade").join(which)
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-upg-shell-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_audit_log_of_alpha1_verifies_and_the_chain_goes_on_across_versions() {
    for which in ["alpha1", "current"] {
        let d = tmp(&format!("audit-{which}"));
        let p = d.join("audit.log");
        fs::copy(fixtures(which).join("audit.log"), &p).unwrap();
        let log = AuditLog::new(&p);
        assert_eq!(log.verify(), Ok(3), "{which}: the chain written by alpha.1 verifies");
        log.record("amy", "chat.send", "after the upgrade");
        // (lines the data stores left while other tests loaded may be flushed into this log too: they are part of the chain)
        let n = log.verify().unwrap_or_else(|at| panic!("{which}: the chain breaks at {at}"));
        assert!(n >= 4, "{which}: a line written now continues the chain");
        // a release that adds a field to the line does not break the chain of this one (the hash covers the five known fields)
        let mut text = fs::read_to_string(&p).unwrap();
        let last = text.lines().last().unwrap().to_string();
        let mut v: serde_json::Value = serde_json::from_str(&last).unwrap();
        v["from_the_future"] = "x".into();
        text = text.replace(&last, &serde_json::to_string(&v).unwrap());
        fs::write(&p, text).unwrap();
        assert_eq!(log.verify(), Ok(n), "{which}: unknown fields are ignored");
        // the old lines are never rewritten
        let after = fs::read_to_string(&p).unwrap();
        let old = fs::read_to_string(fixtures(which).join("audit.log")).unwrap();
        assert!(after.starts_with(&old), "{which}: append only");
        // tampering with an old line is still found
        fs::write(&p, after.replace("\"amy\"", "\"mallory\"")).unwrap();
        assert!(log.verify().is_err(), "{which}");
    }
}

#[test]
fn a_line_cut_short_by_a_power_loss_does_not_swallow_the_next_one_or_break_the_chain() {
    let d = tmp("audit-cut");
    let p = d.join("audit.log");
    fs::copy(fixtures("alpha1").join("audit.log"), &p).unwrap();
    let mut text = fs::read_to_string(&p).unwrap();
    text.push_str("{\"ts\":1700000100,\"user\":\"amy\",\"eve");
    fs::write(&p, &text).unwrap();
    let log = AuditLog::new(&p);
    assert_eq!(log.verify(), Ok(3), "the cut line is skipped");
    log.record("amy", "chat.send", "next");
    assert!(log.verify().unwrap() >= 4, "the new line is whole and chained to the last good one");
    assert!(log.recent(1)[0].contains("chat.send"));
}

#[test]
fn what_the_stores_say_while_loading_reaches_the_audit_log_as_lines_in_the_chain() {
    // The waiting sentences are one queue for the whole process, and every `record()` of every other test's log takes them. So the
    // damaged file is opened again (a fresh sentence) until the sentence ends up in THIS log; one pass is enough when no other test
    // writes at that moment. An audit line holds 300 characters and a long temporary folder cuts the sentence's end off, so the check is
    // the file name and the word "damaged"; the whole sentence is checked in the agent's `upgrade_tests`.
    let d = std::env::temp_dir().join(format!("u{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    let log = AuditLog::new(&d.join("audit.log"));
    let mut found = None;
    for n in 0..200 {
        let bad = d.join(format!("s{n}.json"));
        fs::write(&bad, b"{ broken").unwrap();
        // a damaged settings file, opened the way the services open it
        let _ = iyakki_agent::settings::SettingsStore::open(&bad).unwrap();
        log.flush_notes();
        let name = format!("s{n}.json");
        if let Some(l) = log.recent(500).into_iter().find(|l| l.contains(&name)) {
            found = Some(l);
            break;
        }
    }
    let mine = found.expect("no audit line for the damaged file");
    assert!(mine.contains("data.store") && mine.contains("damaged"), "{mine}");
    assert!(log.verify().is_ok());
}

#[test]
fn approval_pins_of_alpha1_are_kept_with_one_backup_and_a_newer_file_is_not_overwritten() {
    use crate::apps::chat::{read_pins, write_pin};
    let d = tmp("pins");
    let p = d.join("approval-pins-amy.json");
    fs::copy(fixtures("alpha1").join("approval-pins-amy.json"), &p).unwrap();
    let original = fs::read(&p).unwrap();
    assert_eq!(read_pins(&p), vec!["shell:ls".to_string(), "k8s_get:default".to_string()]);
    assert_eq!(fs::read(d.join("approval-pins-amy.json.pre-v1")).unwrap(), original, "the bare list of alpha.1 is kept as it was");
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    assert_eq!((v["schema"].clone(), v["pins"].as_array().unwrap().len()), (1.into(), 2));
    write_pin(&p, "shell:pwd", true).unwrap();
    write_pin(&p, "shell:ls", false).unwrap();
    assert_eq!(read_pins(&p), vec!["k8s_get:default".to_string(), "shell:pwd".to_string()]);
    // the current file reads as it is
    let c = d.join("current.json");
    fs::copy(fixtures("current").join("approval-pins-amy.json"), &c).unwrap();
    let before = fs::read(&c).unwrap();
    assert_eq!(read_pins(&c).len(), 2);
    assert_eq!(fs::read(&c).unwrap(), before);
    assert!(!d.join("current.json.pre-v1").exists());
    // newer: never written
    fs::write(&c, r#"{"schema":3,"pins":["a"],"tomorrow":1}"#).unwrap();
    let before = fs::read(&c).unwrap();
    assert!(write_pin(&c, "b", true).unwrap_err().contains("newer IYAKKI"));
    assert_eq!(fs::read(&c).unwrap(), before);
    // damaged: moved aside, no pins, and the person can pin again
    fs::write(&c, b"[\"a\"").unwrap();
    assert!(read_pins(&c).is_empty());
    assert!(fs::read_dir(&d).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("current.json.corrupt-")));
    write_pin(&c, "z", true).unwrap();
    assert_eq!(read_pins(&c), vec!["z".to_string()]);
}

// ------------------------------------------------------------------------------------------------ accounts

const FIXTURE_PASS: &str = "alpha1-fixture-pass";

#[test]
fn the_accounts_of_alpha1_sign_in_with_their_roles_and_overrides_and_the_first_administrator_is_kept() {
    use crate::accounts::{AccountError, UserStore};
    use iyakki_sec::policy::{App, Role};
    for which in ["alpha1", "current"] {
        let d = tmp(&format!("users-{which}"));
        let p = d.join("users.json");
        fs::copy(fixtures(which).join("users.json"), &p).unwrap();
        let original = fs::read(&p).unwrap();
        let mut s = UserStore::open(&p).unwrap();
        let names: Vec<(&str, Role)> = s.list().iter().map(|u| (u.name.as_str(), u.role)).collect();
        assert_eq!(names, vec![("admin", Role::Admin), ("amy", Role::User), ("ola", Role::Operator), ("visitor", Role::Guest)], "{which}");
        // the old password hashes (Argon2id, made by alpha.1's own function) still verify, and the wrong password does not
        let admin = s.authenticate("admin", FIXTURE_PASS).unwrap();
        assert!(admin.role == Role::Admin && admin.policy.manage_users, "{which}");
        assert_eq!(s.authenticate("amy", "not the password").err(), Some(AccountError::InvalidCredentials), "{which}");
        let amy = s.authenticate("amy", FIXTURE_PASS).unwrap();
        // the per-person refinements an administrator set are still applied; the apps this release added follow the role's template
        assert!(!amy.policy.apps.contains(&App::Radio) && amy.policy.apps.contains(&App::Chat) && amy.policy.max_fuel == 1000, "{which}");
        assert!(s.authenticate("ola", FIXTURE_PASS).is_ok() && s.authenticate("visitor", FIXTURE_PASS).is_ok());
        // reading and signing in wrote nothing
        assert_eq!(fs::read(&p).unwrap(), original, "{which}");
        // there is already a first administrator: setup cannot run again and take the machine over
        assert_eq!(s.create_first_admin("intruder", "another-fixture-pass"), Err(AccountError::Forbidden), "{which}");
        // the last administrator cannot be removed or demoted
        assert_eq!(s.delete_user(&admin, "admin"), Err(AccountError::LastAdmin));
        assert_eq!(s.set_role(&admin, "admin", Role::User), Err(AccountError::LastAdmin));
        // a change made by this release keeps every other account and hash as they were
        s.create_user(&admin, "newcomer", "yet-another-pass", Role::User).unwrap();
        let t = UserStore::open(&p).unwrap();
        assert_eq!(t.list().len(), 5, "{which}");
        assert_eq!(&t.list()[..4], &UserStore::open(&fixtures(which).join("users.json")).unwrap().list()[..4], "{which}");
        assert!(fs::read(&p).unwrap().len() > original.len(), "{which}");
    }
}

#[test]
fn a_damaged_accounts_file_is_never_overwritten_sign_in_is_blocked_and_a_cut_while_running_keeps_the_known_accounts() {
    use crate::accounts::{AccountError, UserStore};
    let d = tmp("users-bad");
    let p = d.join("users.json");
    fs::copy(fixtures("alpha1").join("users.json"), &p).unwrap();
    let mut running = UserStore::open(&p).unwrap();
    // the file is damaged while the shell runs: the accounts it already read stay usable, and nothing is written over the file
    let good = fs::read(&p).unwrap();
    let cut = &good[..good.len() / 2];
    fs::write(&p, cut).unwrap();
    assert!(running.authenticate("admin", FIXTURE_PASS).is_ok());
    assert_eq!(fs::read(&p).unwrap(), cut);
    // a fresh start on the damaged file does not come up with an empty store (which would offer the first-run setup to a stranger)
    match UserStore::open(&p) {
        Err(AccountError::Storage(e)) => assert!(e.contains("corrupt user file"), "{e}"),
        _ => panic!("a damaged accounts file must not open"),
    }
    assert_eq!(fs::read(&p).unwrap(), cut, "the damaged file is left for the person to repair");
    // a copy is kept for repair (the file itself stays: moving it away would offer the first-run setup to a stranger), opening again
    // does not pile up copies, and the audit queue says so
    let copies = corrupt_files(&d, "users.json");
    assert_eq!((copies.len(), fs::read(&copies[0]).unwrap()), (1, cut.to_vec()));
    let _ = UserStore::open(&p);
    assert_eq!(corrupt_files(&d, "users.json").len(), 1);
    // the same damage found while running leaves a copy too (here: a different cut)
    fs::write(&p, &good).unwrap();
    let mut running = UserStore::open(&p).unwrap();
    fs::write(&p, &good[..good.len() / 3]).unwrap();
    assert!(running.authenticate("amy", FIXTURE_PASS).is_ok());
    assert_eq!(corrupt_files(&d, "users.json").len(), 2);
}

fn corrupt_files(dir: &Path, name: &str) -> Vec<PathBuf> {
    fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}.corrupt-"))).collect()
}

#[test]
fn fields_a_newer_release_wrote_in_the_accounts_file_survive_a_save_here() {
    use crate::accounts::UserStore;
    use iyakki_sec::policy::Role;
    let d = tmp("users-extra");
    let p = d.join("users.json");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(fixtures("alpha1").join("users.json")).unwrap()).unwrap();
    v["users"][1]["tomorrow"] = serde_json::json!({"keep": [1, 2]});
    v["next_file_version"] = 7.into();
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let mut s = UserStore::open(&p).unwrap();
    let admin = s.authenticate("admin", FIXTURE_PASS).unwrap();
    assert!(s.authenticate("amy", FIXTURE_PASS).is_ok());
    s.create_user(&admin, "newcomer", "yet-another-pass", Role::User).unwrap();
    s.set_role(&admin, "ola", Role::User).unwrap();
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(after["users"][1]["tomorrow"], serde_json::json!({"keep": [1, 2]}));
    assert_eq!(after["next_file_version"], 7);
    assert_eq!(after["users"].as_array().unwrap().len(), 5);
    assert_eq!(after["users"][2]["role"], "user");
}

// ------------------------------------------------------------------------------------------------ browser files

fn browser_cx(tag: &str, which: &str) -> crate::apps::AppCx {
    let mut cx = crate::apps::tests::cx_with_admin_store(tag);
    cx.session.user = "amy".into();
    let dir = cx.services.dir().to_path_buf();
    for f in ["browser-amy.json", "browser-data-amy.json"] {
        fs::copy(fixtures(which).join(f), dir.join(f)).unwrap();
    }
    cx
}

#[test]
fn the_browser_settings_and_data_of_alpha1_load_with_nothing_lost_and_the_new_choices_at_their_defaults() {
    use crate::apps::{browser_config, browser_data};
    let json = |p: &Path| serde_json::from_slice::<serde_json::Value>(&fs::read(p).unwrap()).unwrap();
    let cx = browser_cx("upg-browser", "alpha1");
    let dir = cx.services.dir().to_path_buf();
    let (cfg_file, data_file) = (dir.join("browser-amy.json"), dir.join("browser-data-amy.json"));
    let (cfg_bytes, data_bytes) = (fs::read(&cfg_file).unwrap(), fs::read(&data_file).unwrap());
    // the choices alpha.1 saved are kept exactly (scripts off stays off: the new default is not forced on an existing choice) ...
    let c = browser_config::load(&cx);
    assert!(!c.js_all && c.images && c.styles && !c.cookies);
    assert_eq!(c.search.id(), "duckduckgo");
    assert_eq!(c.js_sites.iter().cloned().collect::<Vec<_>>(), vec!["docs.example.org".to_string(), "maps.example.org".to_string()]);
    assert!(c.scripts_for("docs.example.org") && !c.scripts_for("other.example.org"));
    // ... and what did not exist yet starts at its default
    let fresh = browser_config::BrowserConfig::default();
    assert_eq!((c.cache, c.smooth), (fresh.cache, false));
    // the bookmarks, history, zoom, night sites and downloads
    let d = browser_data::load(&cx);
    assert_eq!((d.bookmarks.len(), d.history.len(), d.downloads.len()), (2, 1, 1));
    assert_eq!((d.bookmarks[1].folder.as_str(), d.history[0].visits, d.zoom.get("docs.example.org")), ("Travel", 3, Some(&1.25)));
    assert!(d.dark.contains("docs.example.org") && d.downloads[0].status == "saved");
    // loading writes nothing and makes no backup (these files have no schema number: there is no migration to run)
    assert_eq!((fs::read(&cfg_file).unwrap(), fs::read(&data_file).unwrap()), (cfg_bytes, data_bytes));
    // a save by this release writes the new fields beside the old ones; a second load and save change nothing more
    browser_config::save(&cx, &c);
    browser_data::save(&cx, &d);
    let (cfg_json, saved_data) = (json(&cfg_file), fs::read(&data_file).unwrap());
    assert_eq!((cfg_json["js_all"].clone(), cfg_json["search"].clone(), cfg_json["smooth"].clone()), (false.into(), "duckduckgo".into(), false.into()));
    assert!(cfg_json.get("cache").is_some());
    assert_eq!(browser_config::load(&cx), c);
    assert_eq!(browser_data::load(&cx), d);
    browser_config::save(&cx, &browser_config::load(&cx));
    browser_data::save(&cx, &browser_data::load(&cx));
    assert_eq!((json(&cfg_file), fs::read(&data_file).unwrap()), (cfg_json, saved_data));
    assert!(fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains(".pre-v")));
    // the files this release writes load as they are
    let now = browser_cx("upg-browser-now", "current");
    assert_eq!(browser_config::load(&now).js_sites.len(), 2);
    assert_eq!(browser_data::load(&now).bookmarks.len(), 2);
}

#[test]
fn damaged_browser_files_start_that_person_with_defaults_and_touch_nobody_else() {
    use crate::apps::{browser_av, browser_block, browser_config, browser_data, browser_media};
    let cx = browser_cx("upg-browser-bad", "alpha1");
    let dir = cx.services.dir().to_path_buf();
    let mut other = cx.clone();
    other.session.user = "ben".into();
    fs::write(dir.join("browser-ben.json"), r#"{"js_all":true,"images":false}"#).unwrap();
    fs::write(dir.join("browser-data-amy.json"), b"{\"bookmarks\":[{\"title\":").unwrap();
    fs::write(dir.join("browser-amy.json"), b"{ not json").unwrap();
    assert_eq!(browser_config::load(&cx), browser_config::BrowserConfig::default());
    assert_eq!(browser_data::load(&cx), browser_data::Data::default());
    // the damaged bytes are set aside (not deleted), so the next save cannot overwrite them
    let kept = |name: &str| corrupt_files(&dir, name).iter().map(|p| fs::read(p).unwrap()).collect::<Vec<_>>();
    assert_eq!(kept("browser-amy.json"), vec![b"{ not json".to_vec()]);
    assert_eq!(kept("browser-data-amy.json"), vec![b"{\"bookmarks\":[{\"title\":".to_vec()]);
    assert!(!dir.join("browser-amy.json").exists() && !dir.join("browser-data-amy.json").exists());
    browser_data::save(&cx, &browser_data::Data::default());
    assert_eq!(kept("browser-data-amy.json").len(), 1, "the copy is still there after a save");
    // the other per-person browser files are treated the same way
    for (f, damaged) in [("browser-groups-amy.json", "[{\"name\":"), ("browser-open-amy.json", "[\"https://a"), ("browser-block-amy.json", "{ \"enabled\""), ("browser-media-amy.json", "{ x"), ("browser-av-amy.json", "{ y")] {
        fs::write(dir.join(f), damaged).unwrap();
    }
    assert!(crate::apps::browser_groups::load(&cx).is_empty() && crate::apps::browser_groups::load_open(&cx, 10).is_empty());
    assert_eq!(browser_block::load(&cx), browser_block::BlockConfig::default());
    let _ = browser_media::load(&cx);
    let _ = browser_av::load(&cx);
    for f in ["browser-groups-amy.json", "browser-open-amy.json", "browser-block-amy.json", "browser-media-amy.json", "browser-av-amy.json"] {
        assert_eq!(kept(f).len(), 1, "{f}");
        assert!(!dir.join(f).exists(), "{f}");
    }
    // another person's files are their own
    let b = browser_config::load(&other);
    assert!(b.js_all && !b.images);
}
