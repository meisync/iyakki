use super::*;
use serde::{Deserialize, Serialize};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-schema-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn add_level(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        o.entry("level").or_insert(Value::from(3));
    }
}
fn wrap(v: &mut Value) {
    wrap_list(v, "items");
}

const SPEC: Spec = Spec { name: "demo", current: 2, migrations: &[Migration { to: 1, what: "wrap the list", run: wrap }, Migration { to: 2, what: "add level", run: add_level }] };

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
struct Demo {
    schema: u32,
    items: Vec<String>,
    level: u32,
    #[serde(flatten)]
    extra: Extra,
}

#[test]
fn the_registry_is_complete() {
    SPEC.check().unwrap();
    let bad = Spec { name: "x", current: 2, migrations: &[Migration { to: 2, what: "", run: wrap }] };
    assert!(bad.check().is_err());
}

#[test]
fn an_old_list_is_lifted_with_one_backup_and_a_second_open_changes_nothing() {
    let d = dir("lift");
    let p = d.join("demo.json");
    fs::write(&p, r#"["a","b"]"#).unwrap();
    let (out, v, note) = load_typed::<Demo>(&SPEC, &p, 10);
    assert!(matches!(out, Outcome::Migrated { from: 0, .. }));
    assert_eq!((v.items.clone(), v.level, v.schema), (vec!["a".to_string(), "b".to_string()], 3, 2));
    assert!(note.unwrap().contains("upgraded from schema 0 to 2"));
    assert_eq!(fs::read_to_string(sibling(&p, "pre-v1")).unwrap(), r#"["a","b"]"#, "the old bytes are kept as they were");
    let after = fs::read(&p).unwrap();
    let (out2, v2, note2) = load_typed::<Demo>(&SPEC, &p, 11);
    assert!(matches!(out2, Outcome::Current(_)) && note2.is_none());
    assert_eq!(v2, v);
    assert_eq!(fs::read(&p).unwrap(), after, "idempotent");
    assert!(!sibling(&p, "pre-v2").exists());
}

#[test]
fn unknown_fields_survive_a_save() {
    let d = dir("extra");
    let p = d.join("demo.json");
    fs::write(&p, r#"{"schema":2,"items":["x"],"level":1,"from_the_future":{"a":[1,2]}}"#).unwrap();
    let (_, v, _) = load_typed::<Demo>(&SPEC, &p, 1);
    save_typed(&SPEC, &p, &v, 0o600).unwrap();
    let t: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(t["from_the_future"]["a"][1], 2);
}

#[test]
fn a_newer_file_is_never_overwritten() {
    let d = dir("newer");
    let p = d.join("demo.json");
    let text = r#"{"schema":9,"items":["x"],"shape":"new"}"#;
    fs::write(&p, text).unwrap();
    let (out, v, note) = load_typed::<Demo>(&SPEC, &p, 1);
    assert!(matches!(out, Outcome::Newer { found: 9, .. }));
    assert_eq!(v.items, vec!["x".to_string()]);
    assert!(note.unwrap().contains("read-only"));
    let e = save_typed(&SPEC, &p, &v, 0o600).unwrap_err();
    assert!(e.contains("newer IYAKKI"), "{e}");
    assert_eq!(fs::read_to_string(&p).unwrap(), text);
    assert!(!sibling(&p, "pre-v1").exists());
}

#[test]
fn a_damaged_file_is_moved_aside_not_deleted() {
    let d = dir("bad");
    let p = d.join("demo.json");
    fs::write(&p, "{\"items\": [").unwrap();
    let (out, v, note) = load_typed::<Demo>(&SPEC, &p, 77);
    assert!(matches!(out, Outcome::Corrupt { .. }));
    assert_eq!(v, Demo::default());
    assert!(note.unwrap().contains("damaged") && note_has_path(&d, "demo.json.corrupt-77"));
    assert!(!p.exists());
    assert_eq!(fs::read_to_string(d.join("demo.json.corrupt-77")).unwrap(), "{\"items\": [");
    // a second damaged file the same second gets its own name
    fs::write(&p, "x").unwrap();
    let _ = open(&SPEC, &p, 77);
    assert!(d.join("demo.json.corrupt-77-1").exists());
}

fn note_has_path(d: &Path, n: &str) -> bool {
    d.join(n).exists()
}

#[test]
fn a_file_that_does_not_fit_the_struct_is_moved_aside_too() {
    let d = dir("fit");
    let p = d.join("demo.json");
    fs::write(&p, r#"{"schema":2,"items":"not a list"}"#).unwrap();
    let (out, _, _) = load_typed::<Demo>(&SPEC, &p, 5);
    assert!(matches!(out, Outcome::Corrupt { .. }));
    assert!(d.join("demo.json.corrupt-5").exists());
}

#[test]
fn writes_are_atomic_and_keep_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("mode");
    let p = d.join("sub/demo.json");
    save(&SPEC, &p, &serde_json::json!({"items": []}), 0o600).unwrap();
    assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    save(&SPEC, &p, &serde_json::json!({"items": ["a"]}), 0o600).unwrap();
    assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
    assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1, "no temporary file is left");
    assert_eq!(version_of(&serde_json::from_slice(&fs::read(&p).unwrap()).unwrap()), 2);
}

#[test]
fn only_the_newest_three_damaged_copies_are_kept() {
    let d = dir("prune");
    let p = d.join("demo.json");
    for now in 10..16 {
        fs::write(&p, format!("{{ broken {now}")).unwrap();
        assert!(move_aside(&p, now).is_some());
    }
    let mut left: Vec<String> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    left.sort();
    assert_eq!(left, vec!["demo.json.corrupt-13", "demo.json.corrupt-14", "demo.json.corrupt-15"]);
    assert_eq!(fs::read_to_string(d.join("demo.json.corrupt-15")).unwrap(), "{ broken 15");
}

#[test]
fn a_plain_file_that_is_damaged_is_set_aside_reported_and_never_saved_over() {
    let d = dir("plain");
    let p = d.join("list.json");
    assert_eq!(read_plain::<Vec<String>>("list", &p, 1), None, "no file: nothing to report");
    fs::write(&p, r#"["a","b"]"#).unwrap();
    assert_eq!(read_plain::<Vec<String>>("list", &p, 1), Some(vec!["a".to_string(), "b".to_string()]));
    assert!(plain_writable("list", &p).is_ok());
    let _ = drain_notes();
    fs::write(&p, r#"["a","#).unwrap();
    // a save refuses while the damaged file is still there
    assert!(plain_writable("list", &p).unwrap_err().contains("damaged"));
    assert_eq!(read_plain::<Vec<String>>("list", &p, 42), None);
    assert_eq!(fs::read_to_string(d.join("list.json.corrupt-42")).unwrap(), r#"["a","#);
    assert!(!p.exists() && plain_writable("list", &p).is_ok());
    assert!(drain_notes().iter().any(|n| n.starts_with("list ") && n.contains("damaged") && n.contains("list.json.corrupt-42")));
    // valid JSON of the wrong shape counts as damaged too
    fs::write(&p, r#"{"not":"a list"}"#).unwrap();
    assert_eq!(read_plain::<Vec<String>>("list", &p, 43), None);
    assert!(d.join("list.json.corrupt-43").exists());
}

/// Make `d` read-only so a rename inside it fails. False when this user can write anyway (root), so the test has nothing to show.
fn lock_dir(d: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(d, fs::Permissions::from_mode(0o500)).unwrap();
    fs::write(d.join(".probe"), b"x").is_err()
}

fn unlock_dir(d: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(d, fs::Permissions::from_mode(0o700));
}

#[test]
fn a_damaged_plain_file_that_cannot_be_moved_aside_stays_untouched_and_blocks_a_save() {
    let d = dir("nomove-plain");
    let p = d.join("list.json");
    fs::write(&p, r#"["a","#).unwrap();
    if !lock_dir(&d) {
        return;
    }
    let _ = drain_notes();
    assert_eq!(read_plain::<Vec<String>>("list", &p, 7), None);
    let notes = drain_notes();
    unlock_dir(&d);
    assert_eq!(fs::read_to_string(&p).unwrap(), r#"["a","#, "the damaged bytes are still where they were");
    assert!(notes.iter().any(|n| n.contains("could not be moved aside, so nothing will be saved over it")), "{notes:?}");
    assert!(plain_writable("list", &p).unwrap_err().contains("could not be moved aside"));
    assert!(!d.join("list.json.corrupt-7").exists());
}

#[test]
fn a_damaged_store_file_that_cannot_be_moved_aside_reports_no_place_and_is_not_rewritten() {
    let d = dir("nomove-store");
    let p = d.join("store.json");
    fs::write(&p, b"{ not json").unwrap();
    if !lock_dir(&d) {
        return;
    }
    let out = open(&SPEC, &p, 9);
    unlock_dir(&d);
    assert!(matches!(out, Outcome::Corrupt { moved_to: None, .. }), "the file is damaged and stays");
    assert_eq!(fs::read(&p).unwrap(), b"{ not json");
}
