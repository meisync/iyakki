//! Upgrade tests of `firewall.json` (v1.0.0 gate item 5). The firewall did not exist at alpha.1, so there is no alpha.1 file; the
//! current fixture is what this release writes. A settings file of another version is never replaced, a damaged one is kept.

use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::{Saved, Store};
use crate::service::FileStore;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/upgrade/current/firewall.json")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-upg-fw-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_settings_this_release_writes_load_and_save_without_changing_meaning() {
    let d = tmp("cur");
    let p = d.join("firewall.json");
    fs::copy(fixture(), &p).unwrap();
    let st = FileStore { path: p.clone() };
    let s = st.load().unwrap().unwrap();
    assert!(s.config.enabled && s.config.rules.is_empty() && s.config.version == crate::model::CONFIG_VERSION);
    st.save(&s).unwrap();
    let again = st.load().unwrap().unwrap();
    assert_eq!(again.config, s.config);
    assert!(!fs::read_dir(&d).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".tmp")), "no temporary file is left");
}

#[test]
fn settings_of_another_version_are_not_replaced_and_the_safe_defaults_apply() {
    let d = tmp("newer");
    let p = d.join("firewall.json");
    let text = fs::read_to_string(fixture()).unwrap().replace("\"version\": 1", "\"version\": 2");
    fs::write(&p, &text).unwrap();
    let st = FileStore { path: p.clone() };
    assert!(st.load().unwrap_err().contains("version 2"));
    let e = st.save(&Saved::default()).unwrap_err();
    assert!(e.contains("newer IYAKKI") && e.contains("update IYAKKI first"), "{e}");
    assert_eq!(fs::read_to_string(&p).unwrap(), text, "the file is exactly as it was");
}

#[test]
fn a_damaged_file_is_kept_and_the_second_damaged_file_does_not_overwrite_the_first() {
    let d = tmp("bad");
    let p = d.join("firewall.json");
    let st = FileStore { path: p.clone() };
    fs::write(&p, "{ first").unwrap();
    assert!(st.load().is_err());
    assert_eq!(fs::read_to_string(d.join("firewall.json.bad")).unwrap(), "{ first");
    fs::write(&p, "{ second").unwrap();
    assert!(st.load().is_err());
    assert_eq!(fs::read_to_string(d.join("firewall.json.bad")).unwrap(), "{ first", "the first one is still there");
    let kept: Vec<_> = fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(fs::read_to_string(kept[0].path()).unwrap(), "{ second");
    assert!(st.load().unwrap().is_none(), "the next start begins with the defaults");
}
