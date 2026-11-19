//! The generated reference (docs/sdk/reference) and the code blocks of the guide (docs/sdk/recipes) must match the code: this runs
//! `python3 dist/sdk/gen-widget-docs.py --check` and fails with the list of stale files. Fix: run the script without `--check`.
//! Skipped (and says so) where python3 is not installed.

use std::process::Command;

#[test]
fn the_sdk_docs_are_not_stale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = match Command::new("python3").arg("-I").arg(root.join("dist/sdk/gen-widget-docs.py")).arg("--check").output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("SKIP: python3 is not installed");
            return;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the SDK docs are stale.\n{text}\nRun: python3 dist/sdk/gen-widget-docs.py   (then commit docs/sdk)");
}

/// The author check (`dist/sdk/app-check.py`) is a Python port of the style-literal lint plus parsers of test output; its own selftest runs
/// it over samples and over every recipe, so it cannot drift from `iyakki_ui_sdk::lint` unnoticed.
#[test]
fn the_app_check_selftest_passes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = match Command::new("python3").arg("-I").arg(root.join("dist/sdk/app-check.py")).arg("--selftest").output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("SKIP: python3 is not installed");
            return;
        }
    };
    assert!(out.status.success(), "app-check selftest failed:\n{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
