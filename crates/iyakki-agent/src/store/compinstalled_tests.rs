//! The installed-components record: seeded and corrected from facts, written after a verified apply, never left stale.

use std::path::PathBuf;

use super::compinstalled::*;
use super::components::{Installed, INSTALLED_FILE};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-cinst-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn facts() -> Facts {
    Facts { kernel_release: Some("6.12.112-iyakki".into()), release_version: Some("0.1.0-alpha.3".into()), core_version: Some("0.1.0-alpha.3".into()), boot_id: Some("boot-A".into()), core_file: None, previous_root: false }
}

#[test]
fn versions_follow_the_conventions() {
    assert_eq!(numeric_start("6.12.112-iyakki").as_deref(), Some("6.12.112"));
    assert_eq!(numeric_start("6.12.").as_deref(), Some("6.12"));
    assert_eq!(numeric_start("rc1"), None);
    assert!(consistent("0.1.0-alpha.3", "0.1.0-alpha.3"));
    assert!(consistent("0.1.0-alpha.3.1107", "0.1.0-alpha.3"), "a build of the running version");
    assert!(consistent("6.12", "6.12.112") && consistent("6.12.112", "6.12"));
    assert!(!consistent("0.1.0-alpha.4", "0.1.0-alpha.3") && !consistent("6.13.1", "6.12.112") && !consistent("1107", "0.1.0-alpha.3"));
    // "6.1" is not a start of "6.12"
    assert!(!consistent("6.1", "6.12.112"));
}

#[test]
fn an_empty_record_takes_the_facts_and_a_matching_one_is_left_alone() {
    let mut i = Installed::default();
    let notes = correct(&mut i, &facts(), false);
    assert!(notes.is_empty());
    for (c, v) in [("kernel", "6.12.112"), ("core", "0.1.0-alpha.3"), ("kernel-layer", "0.1.0-alpha.3"), ("base", "0.1.0-alpha.3"), ("boot", "0.1.0-alpha.3")] {
        assert_eq!(i.versions.get(c).map(String::as_str), Some(v), "{c}");
        assert_eq!(i.how.get(c).map(String::as_str), Some("observed"));
    }
    // a record that agrees keeps its hash
    i.versions.insert("base".into(), "0.1.0-alpha.3.1107".into());
    i.shas.insert("base".into(), "a".repeat(64));
    assert!(correct(&mut i, &facts(), false).is_empty());
    assert_eq!(i.shas.get("base").map(String::len), Some(64));
    // unreadable facts leave the record alone
    let before = i.clone();
    correct(&mut i, &Facts::default(), false);
    assert_eq!(i, before);
}

#[test]
fn a_stale_record_follows_the_running_system_and_loses_its_hash_and_file() {
    let mut i = Installed::default();
    i.versions.insert("kernel".into(), "6.13.2".into());
    i.shas.insert("kernel".into(), "b".repeat(64));
    i.files.insert("kernel".into(), "/old/bzImage".into());
    i.how.insert("kernel".into(), "applied".into());
    let notes = correct(&mut i, &facts(), false);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("record said 6.13.2") && notes[0].contains("runs 6.12.112"), "{notes:?}");
    assert_eq!(i.versions["kernel"], "6.12.112");
    assert!(!i.shas.contains_key("kernel") && !i.files.contains_key("kernel"));
    assert_eq!(i.how["kernel"], "observed");
}

#[test]
fn a_part_written_but_not_restarted_into_is_not_corrected_until_the_start_changes() {
    let d = dir("await");
    let a = Applied1 { component: "kernel".into(), version: "6.13.2".into(), sha256: "c".repeat(64), file: None, needs_restart: true };
    let rec = record_applied(&d, &[a], "boot-A").unwrap();
    assert_eq!(rec.awaiting["kernel"], "boot-A");
    // same start: the system still runs the old kernel, the record keeps the new one
    let mut i = Installed::load(&d);
    correct(&mut i, &facts(), false);
    assert_eq!(i.versions["kernel"], "6.13.2");
    assert_eq!(i.how["kernel"], "applied");
    // a new start that runs the new kernel: waiting is over, the record stands (and keeps its hash)
    let mut up = Facts { boot_id: Some("boot-B".into()), kernel_release: Some("6.13.2-iyakki".into()), ..facts() };
    let mut j = i.clone();
    assert!(correct(&mut j, &up, false).is_empty());
    assert!(j.awaiting.is_empty() && j.shas.contains_key("kernel") && j.versions["kernel"] == "6.13.2");
    // a new start that runs the OLD kernel (the boot menu fell back): the record follows the system
    up.kernel_release = Some("6.12.112-iyakki".into());
    let mut k = i.clone();
    let notes = correct(&mut k, &up, false);
    assert_eq!(k.versions["kernel"], "6.12.112");
    assert!(k.awaiting.is_empty() && !k.shas.contains_key("kernel"));
    assert_eq!(notes.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn applied_parts_are_recorded_with_their_hash_and_a_damaged_record_reads_as_empty() {
    let d = dir("rec");
    assert_eq!(Installed::load(&d), Installed::default());
    let items = [
        Applied1 { component: "core".into(), version: "0.1.0-alpha.3.1108".into(), sha256: "d".repeat(64), file: Some("/usr/bin/iyakki".into()), needs_restart: true },
        Applied1 { component: "base".into(), version: "0.1.0-alpha.3".into(), sha256: "e".repeat(64), file: None, needs_restart: false },
    ];
    let r = record_applied(&d, &items, "boot-A").unwrap();
    assert_eq!(Installed::load(&d), r);
    assert_eq!((r.versions["core"].as_str(), r.how["core"].as_str(), r.files["core"].to_str()), ("0.1.0-alpha.3.1108", "applied", Some("/usr/bin/iyakki")));
    assert!(r.awaiting.contains_key("core") && !r.awaiting.contains_key("base") && !r.files.contains_key("base"));
    std::fs::write(d.join(INSTALLED_FILE), b"{broken").unwrap();
    assert_eq!(Installed::load(&d), Installed::default());
    // an older record file (without the new fields) still reads
    std::fs::write(d.join(INSTALLED_FILE), br#"{"versions":{"core":"1107"},"shas":{}}"#).unwrap();
    assert_eq!(Installed::load(&d).versions["core"], "1107");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn at_boot_the_record_is_corrected_saved_and_the_running_programs_hash_is_read() {
    let d = dir("boot");
    let prog = d.join("iyakki-bin");
    std::fs::write(&prog, b"the running program").unwrap();
    let f = Facts { core_file: Some(prog.clone()), ..facts() };
    assert!(!d.join(INSTALLED_FILE).exists());
    let notes = correct_at_boot(&d, &f).unwrap();
    assert!(notes.is_empty());
    let i = Installed::load(&d);
    assert_eq!(i.versions["core"], "0.1.0-alpha.3");
    assert_eq!(i.files["core"], prog);
    assert_eq!(i.shas["core"], crate::pkg::hex(&{
        use sha2::{Digest, Sha256};
        Sha256::digest(b"the running program")
    }));
    // looking (not at boot) does not hash and does not save
    std::fs::remove_file(d.join(INSTALLED_FILE)).unwrap();
    let c = current(&d, &f);
    assert!(!c.shas.contains_key("core") && !d.join(INSTALLED_FILE).exists());
    assert_eq!(c.versions["kernel"], "6.12.112");
    // a second boot with nothing changed rewrites nothing
    correct_at_boot(&d, &f).unwrap();
    let t = std::fs::metadata(d.join(INSTALLED_FILE)).unwrap().modified().unwrap();
    correct_at_boot(&d, &f).unwrap();
    assert_eq!(std::fs::metadata(d.join(INSTALLED_FILE)).unwrap().modified().unwrap(), t);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_plan_and_the_status_show_real_installed_values_not_not_on_record() {
    let d = dir("plan");
    let inst = current(&d, &facts());
    let doc_text = {
        use super::compmanifest as cm;
        let key = crate::pkg::new_signing_key().unwrap();
        let e = cm::Entry { component: "kernel".into(), version: "6.13.0".into(), sha256: "1".repeat(64), size: 1000, requires: Default::default(), channel: "stable".into(), built_from: String::new(), delta: vec![], install_path: "boot/bzImage".into(), added: 0 };
        let recs = vec![cm::Record { entry: e, owner: "acme".into(), yanked: None, purged: false }];
        (cm::build_doc("s", 2, 1, &recs, &key), crate::pkg::public_key_hex(&key))
    };
    let doc = super::compmanifest::verify(doc_text.0.as_bytes(), &doc_text.1).unwrap();
    let p = super::components::plan(&doc, "S", "stable", &inst, None).unwrap();
    let t = super::components::plan_text(&p);
    assert!(t.contains("kernel 6.12.112 -> 6.13.0") && !t.contains("not on record"), "{t}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_applied_boot_or_base_version_survives_the_restart_while_the_release_label_is_the_same() {
    // the system can only say its release label for boot and base, not which RAM disk or image file is in place (seen on the Mac mini)
    let d = dir("boot-keeps");
    let mut i = Installed::load(&d);
    correct(&mut i, &facts(), false);
    assert_eq!(i.release, "0.1.0-alpha.3");
    i.save(&d).unwrap();
    let a = Applied1 { component: "boot".into(), version: "1107.15".into(), sha256: "b".repeat(64), file: None, needs_restart: true };
    record_applied(&d, &[a], "boot-A").unwrap();
    // after the restart: same label, normal start: the applied version stands (with its hash)
    let up = Facts { boot_id: Some("boot-B".into()), ..facts() };
    let mut j = Installed::load(&d);
    assert!(correct(&mut j, &up, false).is_empty(), "{j:?}");
    assert_eq!(j.versions["boot"], "1107.15");
    assert!(j.shas.contains_key("boot") && j.how["boot"] == "applied" && j.awaiting.is_empty());
    // the start fell back to the previous root: the record follows the system
    let mut k = Installed::load(&d);
    let notes = correct(&mut k, &Facts { previous_root: true, ..up.clone() }, false);
    assert_eq!(k.versions["boot"], "0.1.0-alpha.3");
    assert!(!k.shas.contains_key("boot") && notes.len() == 1, "{notes:?}");
    // a new release label (a whole system update): the record follows the system as well
    let mut m = Installed::load(&d);
    correct(&mut m, &Facts { release_version: Some("0.1.0-alpha.4".into()), ..up.clone() }, false);
    assert_eq!(m.versions["boot"], "0.1.0-alpha.4");
    let _ = std::fs::remove_dir_all(&d);
}
