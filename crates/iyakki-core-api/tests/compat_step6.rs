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

/// A stand-in for QEMU: it reads the same arguments the harness passes, prints the lines the core prints for each selftest stage and, as the NEW
/// build, migrates `settings.json` the way a store that changed shape does (the original is kept as `settings.json.pre-v1`). With `CLOBBER=1` the
/// OLD build deletes that copy, which the harness must catch. Everything else is the harness itself, with the real mke2fs, debugfs and e2fsck.
const FAKE_QEMU: &str = r#"#!/bin/sh
kernel=""; append=""; img=""
while [ $# -gt 0 ]; do
  case "$1" in -kernel) kernel="$2"; shift;; -append) append="$2"; shift;; -drive) img="${2#file=}"; img="${img%%,*}"; shift;; esac
  shift
done
role="$(cat "$(dirname "$kernel")/../role")"
stage="${append#*selftest=}"; stage="${stage%% *}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
case "$stage" in
  persist2) echo 'IYAKKI-SELFTEST persist2 location Disk'; echo 'IYAKKI-SELFTEST persist2 marker "iyakki-persist-ok"';;
  upgrade)
    if [ "$role" = NEW ] && [ "${MIGRATE:-1}" = 1 ] && [ -z "$(debugfs -R 'cat /settings.json.pre-v1' "$img" 2>/dev/null)" ]; then
      debugfs -R 'cat /settings.json' "$img" > "$T/orig" 2>/dev/null
      debugfs -w -R "write $T/orig settings.json.pre-v1" "$img" >/dev/null 2>&1
      printf '{"schema":1}' > "$T/new"
      debugfs -w -R "rm settings.json" "$img" >/dev/null 2>&1
      debugfs -w -R "write $T/new settings.json" "$img" >/dev/null 2>&1
    fi
    if [ "$role" = OLD ] && [ "${CLOBBER:-0}" = 1 ] && [ -n "$(debugfs -R 'cat /settings.json.pre-v1' "$img" 2>/dev/null)" ]; then
      printf 'garbage' > "$T/g"
      debugfs -w -R "rm settings.json.pre-v1" "$img" >/dev/null 2>&1
      debugfs -w -R "write $T/g settings.json.pre-v1" "$img" >/dev/null 2>&1
    fi
    echo 'IYAKKI-SELFTEST upgrade settings providers=2 chat=qwen3:8b'; echo 'IYAKKI-SELFTEST upgrade sources 2'; echo 'IYAKKI-SELFTEST upgrade inbox 5'; echo 'IYAKKI-SELFTEST upgrade end';;
esac
"#;

fn harness_run(tag: &str, env: &[(&str, &str)]) -> Option<(i32, String)> {
    for t in ["mke2fs", "debugfs", "e2fsck"] {
        if std::process::Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("skipped: {t} is not installed here");
            return None;
        }
    }
    let d = std::env::temp_dir().join(format!("iyakki-step6-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (name, role) in [("old", "OLD"), ("new", "NEW")] {
        std::fs::create_dir_all(d.join(name).join("images")).unwrap();
        for f in ["bzImage", "rootfs.cpio.zst"] {
            std::fs::write(d.join(name).join("images").join(f), b"x").unwrap();
        }
        std::fs::write(d.join(name).join("role"), role).unwrap();
    }
    let q = d.join("fake-qemu");
    std::fs::write(&q, FAKE_QEMU).unwrap();
    std::fs::set_permissions(&q, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut c = std::process::Command::new("sh");
    c.arg(root().join("dist/buildroot/qemu-rollback-test.sh")).arg(d.join("old")).arg(d.join("new")).env("QEMU", &q);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()))
}

#[test]
fn the_harness_logic_passes_a_build_pair_with_a_migration_and_fails_one_without_it_or_one_whose_previous_version_clobbers_the_copy() {
    // a pair with a migration: boot 3 leaves the `.pre-v1` original, the previous version leaves it alone, the second NEW run rewrites nothing
    if let Some((code, out)) = harness_run("migrates", &[("REQUIRE_MIGRATION", "1")]) {
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("1 changed by NEW") || out.contains("changed by NEW"), "{out}");
        assert!(out.trim_end().ends_with("PASS"), "{out}");
    }
    // a pair with no migration cannot satisfy REQUIRE_MIGRATION (it would prove nothing about `.pre-v1`)
    if let Some((code, out)) = harness_run("no-migration", &[("REQUIRE_MIGRATION", "1"), ("MIGRATE", "0")]) {
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("this build pair has no migration"), "{out}");
    }
    // the same pair without the requirement still passes (the existing behaviour)
    if let Some((code, out)) = harness_run("plain", &[("MIGRATE", "0")]) {
        assert_eq!(code, 0, "{out}");
    }
    // the previous version rewriting the original the next version kept is caught
    if let Some((code, out)) = harness_run("clobber", &[("CLOBBER", "1")]) {
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("pre-v1 changed while the previous version ran"), "{out}");
    }
}
