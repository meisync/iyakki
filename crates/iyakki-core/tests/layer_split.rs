//! The layered test gate (alpha.5 item 0b): every workspace crate has a layer in dist/scripts/crate-layers.txt (there is no catch-all, so a new crate
//! without a line fails here), and the stamp logic and the layer runner pass their tests on fake trees with a fake cargo (no real cargo runs).

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python(args: &[&str]) -> (i32, String) {
    let o = Command::new("python3").arg("-I").args(args).env("IYAKKI_ROOT", root()).output().expect("python3 is needed");
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr))
}

#[test]
fn every_workspace_crate_has_a_layer() {
    let (rc, out) = python(&[root().join("dist/scripts/testlayers.py").to_str().unwrap(), "check"]);
    assert_eq!(rc, 0, "a workspace crate has no layer in dist/scripts/crate-layers.txt:\n{out}");
}

#[test]
fn stamp_logic_and_layer_runner_pass_on_fake_trees() {
    let (rc, out) = python(&[root().join("dist/scripts/test_testlayers.py").to_str().unwrap()]);
    assert_eq!(rc, 0, "{out}");
}
