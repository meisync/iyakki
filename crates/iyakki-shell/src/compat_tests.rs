//! Charter step 3 for the shell's own versioned store, the approval pins (`approval-pins-<user>.json`): the alpha.2 file in
//! `tests/compat/alpha2/data` is read, a field a newer release added survives a write, and a newer schema is read and never written.
//! The rest of the data fixtures are tested in `iyakki-agent` (`compat_tests`); see tests/compat/README.md.

use std::fs;
use std::path::{Path, PathBuf};

use crate::apps::chat::{read_pins, write_pin};

fn copy_of_fixture(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-compat-shell-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha2/data/approval-pins-amy.json");
    let to = d.join("approval-pins-amy.json");
    fs::copy(from, &to).unwrap();
    to
}

fn json(p: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

#[test]
fn backward_the_pins_of_alpha2_are_read_without_a_change() {
    let p = copy_of_fixture("read");
    let before = fs::read(&p).unwrap();
    assert_eq!(read_pins(&p), vec!["tool:read_file".to_string()]);
    assert_eq!(fs::read(&p).unwrap(), before);
    assert_eq!(json(&p)["schema"], 1);
}

#[test]
fn forward_an_added_field_survives_a_write_and_a_newer_schema_is_never_written() {
    let p = copy_of_fixture("forward");
    let mut v = json(&p);
    v["added_in_a_newer_release"] = serde_json::json!({"n": 1});
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    write_pin(&p, "tool:list_dir", true).unwrap();
    let after = json(&p);
    assert_eq!(after["added_in_a_newer_release"], serde_json::json!({"n": 1}));
    assert_eq!(after["pins"].as_array().unwrap().len(), 2);
    // newer
    let mut v = json(&p);
    v["schema"] = serde_json::json!(99);
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let before = fs::read(&p).unwrap();
    let e = write_pin(&p, "tool:x", true).unwrap_err();
    assert!(e.contains("newer IYAKKI") && e.contains("update IYAKKI"), "{e}");
    assert_eq!(fs::read(&p).unwrap(), before);
}
