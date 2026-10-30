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
