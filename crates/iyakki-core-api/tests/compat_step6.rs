//! Charter step 6 (F-CORE-21): the QEMU upgrade and rollback harness (`dist/buildroot/qemu-rollback-test.sh`). QEMU is not available to
//! the unit tests, so these checks keep the script honest: it parses, it takes two builds, it boots the OLD build again after the NEW one
//! (the rollback), it names every guard, and it says clearly when it cannot run instead of passing.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script() -> String {
    std::fs::read_to_string(root().join("dist/buildroot/qemu-rollback-test.sh")).expect("the harness script exists")
}

#[test]
fn the_script_is_valid_shell_and_runs_nothing_without_two_builds_and_qemu() {
    let p = root().join("dist/buildroot/qemu-rollback-test.sh");
    let ok = std::process::Command::new("sh").arg("-n").arg(&p).status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "sh -n finds a syntax error");
    // no arguments: not runnable (exit 3), never a pass
    let o = std::process::Command::new("sh").arg(&p).output().unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", String::from_utf8_lossy(&o.stdout));
    assert!(String::from_utf8_lossy(&o.stdout).contains("usage:"));
    // two paths that are not builds: not runnable either
    let o = std::process::Command::new("sh").arg(&p).arg("/nonexistent-old").arg("/nonexistent-new").output().unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&o.stdout).contains("NOT RUNNABLE"));
}

#[test]
fn the_script_boots_old_then_new_then_old_again_and_checks_what_the_charter_asks() {
    let s = script();
    // the order of the boots is the rollback: OLD, NEW, OLD again
    let at = |needle: &str| s.find(needle).unwrap_or_else(|| panic!("the script does not contain {needle:?}"));
    assert!(at("--- boot 1: OLD") < at("--- boot 3: NEW migrates") && at("--- boot 3: NEW migrates") < at("--- boot 4: OLD again") && at("--- boot 4: OLD again") < at("--- boot 5: NEW again"));
    for must in [
        "iyakki.selftest=",
        "e2fsck -fn",
        ".corrupt-",
        "iyakki-persist-ok",
        "upgrade settings providers=2 chat=qwen3:8b",
        "upgrade sources 2",
        "upgrade inbox 5",
        "a newer file must be read, not rewritten",
        "exit 2",
        "exit 3",
        "never writes to an image",
    ] {
        assert!(s.to_lowercase().contains(&must.to_lowercase()), "the harness does not check {must:?}");
    }
    // it never writes to the builds: the only disk is a temporary file
    assert!(s.contains("mktemp -d") && s.contains("mke2fs -q -t ext4 -L IYAKKI-DATA -d"));
}

#[test]
fn the_fixture_the_harness_boots_exists_and_the_stage_names_are_the_ones_the_core_prints() {
    let fix = root().join("tests/fixtures/upgrade/alpha1");
    for f in ["settings.json", "inbox/amy.json", "dash/sources-amy.json"] {
        assert!(fix.join(f).is_file(), "{f} is missing from the fixture");
    }
    let main = std::fs::read_to_string(root().join("crates/iyakki-core/src/main.rs")).unwrap();
    for line in ["upgrade settings providers=", "upgrade sources", "upgrade inbox"] {
        assert!(main.contains(line), "the core no longer prints {line:?}, which the harness greps for");
    }
}
