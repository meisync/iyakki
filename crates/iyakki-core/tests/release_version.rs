//! The release's identity is consistent: one version everywhere, the files a release needs exist, and the matrix totals add up.
//! (The same checks run as `./run.sh alpha-check`.)

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_crate_lock_changelog_and_release_notes_name_the_same_version() {
    let problems = iyakki_relcheck::version::check(&root());
    assert!(problems.is_empty(), "version problems:\n{}", problems.join("\n"));
}

#[test]
fn the_binary_is_built_as_the_workspace_version() {
    let ws = iyakki_relcheck::version::manifest_version(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(env!("CARGO_PKG_VERSION"), ws);
    let desktop = std::fs::read_to_string(root().join("crates/iyakki-gui/Cargo.toml")).unwrap();
    assert_eq!(iyakki_relcheck::version::manifest_version(&desktop).as_deref(), Some(ws.as_str()), "the separate GUI workspace follows the workspace version");
}

#[test]
fn the_files_a_release_needs_exist() {
    for f in ["LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.txt", "CHANGELOG.md", "KNOWN-ISSUES.md", "SECURITY.md", "ALPHA-GATE.md", "docs/RELEASING.md", "docs/LICENSES.md", "dist/release/alpha-check.sh", "dist/release/release.sh"] {
        assert!(root().join(f).exists(), "{f} is missing");
    }
    let sec = std::fs::read_to_string(root().join("SECURITY.md")).unwrap();
    assert!(sec.contains("Reporting a vulnerability"));
    // the mailbox is either still the marked placeholder or a real address, never silently empty
    assert!(sec.contains("OWNER: fill in") || sec.contains('@'));
}

#[test]
fn the_feature_matrix_totals_are_consistent() {
    let t = std::fs::read_to_string(root().join("FEATURES.md")).unwrap();
    assert_eq!(iyakki_relcheck::features::check(&t), Vec::<String>::new());
}

/// `./run.sh alpha-check --dry` runs the fast checks only (no build, no writes outside target/): it prints a table, and its exit code
/// is non-zero exactly when the table has a FAIL.
#[test]
fn alpha_check_in_dry_mode_prints_its_table_and_its_exit_code_follows_the_fail_rows() {
    let tmp = std::env::temp_dir().join(format!("iyakki-alpha-check-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let out = std::process::Command::new("bash")
        .arg(root().join("dist/release/alpha-check.sh"))
        .arg("--dry")
        .env("IYAKKI_TMPDIR", &tmp)
        .env_remove("IYAKKI_TMP_SET")
        .output()
        .expect("bash is needed");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("CHECK") && text.contains("RESULT"), "no table:\n{text}");
    for row in ["version consistency", "FEATURES.md totals", "docs links and anchors", "no secrets in the tree", "release files present", "workspace tests"] {
        assert!(text.contains(row), "row {row:?} missing:\n{text}");
    }
    assert!(text.lines().any(|l| l.starts_with("workspace tests") && l.contains("SKIP")), "dry mode must skip the build and the tests:\n{text}");
    let has_fail = text.lines().any(|l| l.split_whitespace().any(|w| w == "FAIL") && !l.starts_with("alpha-check:"));
    assert_eq!(out.status.success(), !has_fail, "exit code {:?} disagrees with the table:\n{text}", out.status.code());
    let _ = std::fs::remove_dir_all(&tmp);
}
