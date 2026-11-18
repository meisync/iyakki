use super::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-twophase-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("store")).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn upper(_: &str, old: &[u8]) -> Result<Vec<u8>, String> {
    Ok(old.to_ascii_uppercase())
}

fn staged(d: &Path) -> State {
    std::fs::write(d.join("store/a.txt"), "alpha").unwrap();
    std::fs::write(d.join("b.txt"), "beta").unwrap();
    stage(d, "m1", "1.0", "2.0", &["store/a.txt", "b.txt"], 100, &upper).unwrap()
}

#[test]
fn staging_writes_the_new_layout_next_to_the_old_and_leaves_the_old_alone() {
    let d = tmp("stage");
    let s = staged(&d);
    assert_eq!(s.phase, Phase::Staged);
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt")).unwrap(), "alpha");
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt.next")).unwrap(), "ALPHA");
    // the new version reads the staged file, the old version the original
    assert_eq!(path_to_read(&d, "m1", "store/a.txt", "2.0"), d.join("store/a.txt.next"));
    assert_eq!(path_to_read(&d, "m1", "store/a.txt", "1.0"), d.join("store/a.txt"));
    assert_eq!(path_to_read(&d, "other", "b.txt", "2.0"), d.join("b.txt"));
    // a restart stages nothing again
    std::fs::write(d.join("b.txt"), "changed").unwrap();
    assert_eq!(stage(&d, "m1", "1.0", "2.0", &["b.txt"], 999, &upper).unwrap(), s);
    assert_eq!(std::fs::read_to_string(d.join("b.txt.next")).unwrap(), "BETA");
}

#[test]
fn a_failing_conversion_changes_nothing_and_bad_names_are_refused() {
    let d = tmp("fail");
    std::fs::write(d.join("a"), "1").unwrap();
    std::fs::write(d.join("b"), "2").unwrap();
    let e = stage(&d, "m", "1", "2", &["a", "b"], 1, &|f, o| if f == "b" { Err("cannot read".into()) } else { Ok(o.to_vec()) }).unwrap_err();
    assert!(e.contains("b:") && e.contains("nothing was changed"), "{e}");
    assert!(!d.join("a.next").exists() && load(&d, "m").is_none() && pending(&d).is_empty());
    assert!(stage(&d, "../x", "1", "2", &["a"], 1, &upper).is_err());
    assert!(stage(&d, "m", "1", "2", &["../a"], 1, &upper).is_err());
    assert!(stage(&d, "m", "1", "2", &["/etc/passwd"], 1, &upper).is_err());
}

#[test]
fn the_new_version_confirming_starts_the_grace_period_and_commit_waits_for_it() {
    let d = tmp("commit");
    staged(&d);
    // before the new version has booted and confirmed, nothing may be dropped
    assert!(commit(&d, "m1", 10_000, 0).unwrap_err().contains("not confirmed"));
    assert_eq!(reconcile(&d, "2.0", 200), vec![("m1".to_string(), Phase::Confirmed)]);
    assert_eq!(load(&d, "m1").unwrap().confirmed_at, Some(200));
    assert!(status_lines(&d)[0].contains("grace period"));
    assert!(commit(&d, "m1", 300, 1000).unwrap_err().contains("900 more seconds"));
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt")).unwrap(), "alpha", "still the old data");
    let s = commit(&d, "m1", 1200, 1000).unwrap();
    assert_eq!(s.phase, Phase::Committed);
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt")).unwrap(), "ALPHA");
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt.pre-m1")).unwrap(), "alpha", "the old file is kept, never deleted");
    assert!(!d.join("store/a.txt.next").exists() && pending(&d).is_empty() && status_lines(&d).is_empty());
    assert_eq!(commit(&d, "m1", 5000, 0).unwrap().phase, Phase::Committed, "committing twice is harmless");
}

#[test]
fn the_old_version_starting_again_rolls_the_staged_data_back_and_loses_nothing() {
    let d = tmp("rollback");
    staged(&d);
    // the new version never confirmed: the old one boots (the loader fell back)
    assert_eq!(reconcile(&d, "1.0", 300), vec![("m1".to_string(), Phase::RolledBack)]);
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt")).unwrap(), "alpha");
    assert!(!d.join("store/a.txt.next").exists());
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt.next.rolled-back")).unwrap(), "ALPHA");
    assert!(commit(&d, "m1", 9999, 0).unwrap_err().contains("rolled back"));
    // after confirming too, a rollback to the old version still puts the old data first
    let d = tmp("rollback2");
    staged(&d);
    reconcile(&d, "2.0", 200);
    assert_eq!(reconcile(&d, "1.0", 250), vec![("m1".to_string(), Phase::RolledBack)]);
    assert_eq!(path_to_read(&d, "m1", "b.txt", "1.0"), d.join("b.txt"));
    assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "beta");
}

#[test]
fn a_commit_that_stopped_half_way_is_finished_by_the_next_call() {
    let d = tmp("resume");
    staged(&d);
    reconcile(&d, "2.0", 200);
    // simulate a crash after the first file moved: state says committing, a.txt already swapped
    let mut s = load(&d, "m1").unwrap();
    s.phase = Phase::Committing;
    save(&d, &s).unwrap();
    std::fs::rename(d.join("store/a.txt"), d.join("store/a.txt.pre-m1")).unwrap();
    std::fs::rename(d.join("store/a.txt.next"), d.join("store/a.txt")).unwrap();
    let s = commit(&d, "m1", 0, 1_000_000).unwrap();
    assert_eq!(s.phase, Phase::Committed);
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt")).unwrap(), "ALPHA");
    assert_eq!(std::fs::read_to_string(d.join("store/a.txt.pre-m1")).unwrap(), "alpha");
    assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "BETA");
    assert_eq!(std::fs::read_to_string(d.join("b.txt.pre-m1")).unwrap(), "beta");
}

#[test]
fn fields_a_newer_release_adds_to_the_state_survive_a_save() {
    let d = tmp("extra");
    staged(&d);
    let p = state_path(&d, "m1");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    v["note"] = serde_json::json!("from a newer release");
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    reconcile(&d, "2.0", 5);
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(after["note"], "from a newer release");
    assert_eq!(after["phase"], "confirmed");
}
