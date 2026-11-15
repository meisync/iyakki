//! Runs the node tests of the media half of the page and of the audio worklet's queue (`js/test/media.test.mjs`): the 100 ms bound, prefill,
//! flush, sequence gaps, the microphone chunker, and the page half with a fake DOM. Skipped, loudly, where node is not installed.

use std::process::Command;

#[test]
fn the_media_page_code_and_the_audio_worklet_pass_the_node_tests() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(iyakki_client_engine::MEDIA_JS.contains("createMedia") && iyakki_client_engine::WORKLET_JS.contains("iyakki-out"));
    let Ok(v) = Command::new("node").arg("--version").output() else {
        eprintln!("SKIPPED: node is not installed, the media code was not tested");
        return;
    };
    let ver = String::from_utf8_lossy(&v.stdout);
    let major: u32 = ver.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    if major < 20 {
        eprintln!("SKIPPED: node {ver} is too old");
        return;
    }
    let out = Command::new("node").arg("--test").arg(dir.join("js/test/media.test.mjs")).output().expect("node ran");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    let passed: u32 = text.lines().find_map(|l| l.strip_prefix("# pass ")).and_then(|n| n.trim().parse().ok()).unwrap_or(0);
    assert!(passed >= 11 && text.contains("# fail 0"), "at least 11 node tests must pass and none fail: {text}");
}
