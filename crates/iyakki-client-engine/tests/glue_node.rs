//! Runs the node tests of the glue against the committed WebAssembly module (`js/test/glue.test.mjs`). Skipped, loudly, where node (22 or
//! newer) is not installed; the module itself is checked by `dist/client-engine/build.sh --check` in the gate.

use std::process::Command;

#[test]
fn the_glue_and_the_committed_wasm_module_pass_the_node_tests() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!iyakki_client_engine::WASM.is_empty(), "the module is committed: run dist/client-engine/build.sh");
    let node = Command::new("node").arg("--version").output();
    let Ok(v) = node else {
        eprintln!("SKIPPED: node is not installed, the glue was not tested");
        return;
    };
    let ver = String::from_utf8_lossy(&v.stdout);
    let major: u32 = ver.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    if major < 20 {
        eprintln!("SKIPPED: node {ver} is too old (needs WebCrypto Ed25519, node 20+)");
        return;
    }
    let out = Command::new("node").arg("--test").arg(dir.join("js/test/glue.test.mjs")).output().expect("node ran");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("# pass 6"), "{text}");
}
