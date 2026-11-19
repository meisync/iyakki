//! The program `iyakki-ui` without the `in-process` feature must not contain the agent (F-SPLIT-03, the layered build's `ui` layer): its dependency
//! tree, as `cargo tree` prints it, holds none of the crates that make the agent and the core. This is what lets the ui layer be a few MB.
//! The same holds for the remote-object crate it is built on. `--features in-process` is the one way the agent comes in, and the test shows the
//! check can see it (so a tree that lost the agent for the wrong reason, such as a failed command, cannot pass).

use std::process::Command;

const AGENT_SIDE: &[&str] = &["iyakki-agent", "iyakki-core", "iyakki-compose", "iyakki-core-local", "iyakki-core-lib"];

fn tree(package: &str, features: &[&str]) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(env!("CARGO_MANIFEST_DIR")).args(["tree", "--locked", "--offline", "-e", "normal", "--prefix", "none", "-p", package]);
    if !features.is_empty() {
        cmd.args(["--features", &features.join(",")]);
    }
    let out = cmd.output().expect("cargo runs");
    assert!(out.status.success(), "cargo tree failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let mut names: Vec<String> = text.lines().filter_map(|l| l.split_whitespace().next()).map(String::from).collect();
    names.sort();
    names.dedup();
    names
}

fn agent_side(names: &[String]) -> Vec<&str> {
    AGENT_SIDE.iter().copied().filter(|a| names.iter().any(|n| n == a)).collect()
}

#[test]
fn the_ui_program_and_the_remote_object_crate_do_not_contain_the_agent() {
    for pkg in ["iyakki-ui-bin", "iyakki-ui-rpc", "iyakki-ui-client"] {
        let t = tree(pkg, &[]);
        assert!(t.iter().any(|n| n == "iyakki-ui-client"), "the tree of {pkg} was read ({} crates)", t.len());
        assert!(agent_side(&t).is_empty(), "{pkg} must not depend on the agent side, found {:?}", agent_side(&t));
    }
}

#[test]
fn the_in_process_feature_is_the_one_way_the_agent_comes_in() {
    let t = tree("iyakki-ui-bin", &["in-process"]);
    assert!(!agent_side(&t).is_empty(), "the check sees the agent side when the feature asks for it");
}
