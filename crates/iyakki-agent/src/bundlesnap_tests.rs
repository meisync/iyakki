use super::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bsnap-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn app_data(d: &Path) -> PathBuf {
    let a = d.join("app");
    std::fs::create_dir_all(a.join("sub")).unwrap();
    std::fs::write(a.join("one.txt"), "first").unwrap();
    std::fs::write(a.join("sub/two.txt"), "second").unwrap();
    a
}

#[test]
fn a_snapshot_lists_sizes_and_sums_skips_links_and_refuses_a_second_one_with_the_same_name() {
    let d = tmp("snap");
    let a = app_data(&d);
    std::os::unix::fs::symlink("/etc/hostname", a.join("link")).unwrap();
    let root = d.join("snaps");
    let s = snapshot(&a, &root, "pre-update", 100, 1 << 20).unwrap();
    assert_eq!(s.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["one.txt", "sub/two.txt"]);
    assert_eq!((s.bytes, s.skipped.clone()), (11, vec!["link".to_string()]));
    assert_eq!(std::fs::read_to_string(root.join("pre-update/data/sub/two.txt")).unwrap(), "second");
    assert_eq!(verify(&root, "pre-update").unwrap(), s);
    assert!(snapshot(&a, &root, "pre-update", 101, 1 << 20).unwrap_err().contains("already exists"));
    assert!(snapshot(&a, &root, "../x", 1, 1 << 20).is_err());
    assert_eq!(list(&root).len(), 1);
}

#[test]
fn a_snapshot_over_the_limit_copies_nothing() {
    let d = tmp("cap");
    let a = app_data(&d);
    let root = d.join("snaps");
    assert!(snapshot(&a, &root, "big", 1, 5).unwrap_err().contains("over the 5 byte limit"));
    assert!(!root.join("big").exists() && list(&root).is_empty());
}

#[test]
fn restoring_puts_the_data_back_keeps_what_it_replaced_and_refuses_a_damaged_snapshot() {
    let d = tmp("restore");
    let a = app_data(&d);
    let root = d.join("snaps");
    snapshot(&a, &root, "s1", 1, 1 << 20).unwrap();
    std::fs::write(a.join("one.txt"), "changed").unwrap();
    std::fs::remove_file(a.join("sub/two.txt")).unwrap();
    let kept = restore(&root, "s1", &a).unwrap();
    assert_eq!(std::fs::read_to_string(a.join("one.txt")).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(a.join("sub/two.txt")).unwrap(), "second");
    assert_eq!(std::fs::read_to_string(kept.join("one.txt")).unwrap(), "changed", "what was replaced is kept");
    // a second restore finds the kept folder in the way and does nothing
    assert!(restore(&root, "s1", &a).unwrap_err().contains("already there"));
    // a flipped byte in the snapshot is found before anything is touched
    std::fs::write(root.join("s1/data/one.txt"), "tampered").unwrap();
    assert!(verify(&root, "s1").unwrap_err().contains("damaged"));
    assert!(restore(&root, "s1", &d.join("other")).unwrap_err().contains("damaged"));
    assert!(!d.join("other").exists());
}

#[test]
fn a_failing_migration_is_rolled_back_and_a_good_one_is_kept() {
    let d = tmp("run");
    let a = app_data(&d);
    let root = d.join("snaps");
    let bad = run_with_rollback(&a, &root, "m-bad", 5, 1 << 20, &|dir| {
        std::fs::write(dir.join("one.txt"), "half").unwrap();
        std::fs::remove_file(dir.join("sub/two.txt")).unwrap();
        Err("the new format could not be written".into())
    });
    let e = bad.unwrap_err();
    assert!(e.contains("could not be written") && e.contains("put back"), "{e}");
    assert_eq!(std::fs::read_to_string(a.join("one.txt")).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(a.join("sub/two.txt")).unwrap(), "second");
    let ok = run_with_rollback(&a, &root, "m-ok", 6, 1 << 20, &|dir| {
        std::fs::write(dir.join("one.txt"), "migrated").unwrap();
        Ok("converted 1 file".into())
    });
    assert_eq!(ok.unwrap(), "converted 1 file");
    assert_eq!(std::fs::read_to_string(a.join("one.txt")).unwrap(), "migrated");
    // no snapshot, no migration
    let e = run_with_rollback(&a, &root, "m-big", 7, 3, &|_| panic!("must not run")).unwrap_err();
    assert!(e.contains("migration did not run"), "{e}");
}

#[test]
fn old_snapshots_are_moved_aside_never_deleted() {
    let d = tmp("prune");
    let a = app_data(&d);
    let root = d.join("snaps");
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        snapshot(&a, &root, id, i as i64, 1 << 20).unwrap();
    }
    assert_eq!(prune(&root, 1), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(list(&root).iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["c"]);
    assert!(root.join(".old/a/manifest.json").is_file());
}

struct Inner;
impl Env for Inner {
    fn check(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn migrate(&self, _: &str, _: Option<&str>, _: Option<&str>, _: &BTreeMap<String, Value>) -> Result<String, String> {
        Err("must not be called".into())
    }
}

#[test]
fn a_wizard_migration_step_runs_from_a_snapshot_and_rolls_back_when_it_fails() {
    use crate::wizardrun::{Run, State};
    let d = tmp("wiz");
    let a = app_data(&d);
    let spec: iyakki_core_api::wizard::WizardSpec = serde_json::from_value(serde_json::json!({
        "id": "up", "trigger": {"on": "on_update", "from": "<2"},
        "steps": [{"kind": "migration", "id": "m", "title": "Move data", "migration": "to-v2", "to": "2.0.0"}]
    }))
    .unwrap();
    let go = |fail: bool| {
        let mig = move |_: &str, from: Option<&str>, _: Option<&str>, _: &BTreeMap<String, Value>, dir: &Path| {
            assert_eq!(from, Some("1.2.0"));
            std::fs::write(dir.join("one.txt"), "v2").unwrap();
            if fail { Err("disk full".to_string()) } else { Ok("converted".to_string()) }
        };
        let env = RollbackEnv { inner: &Inner, data: a.clone(), root: d.join("snaps"), now: if fail { 10 } else { 11 }, max_bytes: 1 << 20, migrate: &mig, wizard: "up".into() };
        let mut r = Run::start(spec.clone(), "on_update", Some("1.2.0")).unwrap();
        r.advance(&env);
        r.state().clone()
    };
    assert_eq!(go(true), State::Failed("Move data: disk full; the data was put back (the half-migrated copy is at ".to_string() + &format!("{}.rolled-back-up-to-v2-10)", a.display())));
    assert_eq!(std::fs::read_to_string(a.join("one.txt")).unwrap(), "first");
    assert_eq!(go(false), State::Done);
    assert_eq!(std::fs::read_to_string(a.join("one.txt")).unwrap(), "v2");
}
