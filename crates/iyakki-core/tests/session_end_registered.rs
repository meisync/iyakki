//! Ratchet (D-SESSION-END): a new per-person player, reader, recorder, listener, capture or speaker must say what a sign-out does to it.
//!
//! Every non-test `pub struct` whose name ends in Player, Reader, Recorder, Listener, Capture or Speaker (or Players, Recorders) is
//! scanned and must be a line of `docs/development/session-end-resources.txt` with a status: a registered end-of-session hook, stopped
//! by drop, continuing on purpose, not per person, or `open`. A new struct fails here until it is registered or classified; the number
//! of `open` lines may only fall. A `registered:<hook>` must name a hook the source really registers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "tests" || n == "target") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            if !(n.ends_with("_tests.rs") || n.ends_with("_test.rs") || n == "tests.rs") {
                out.push(p);
            }
        }
    }
}

const SUFFIXES: &[&str] = &["Player", "Reader", "Recorder", "Listener", "Capture", "Speaker", "Players", "Recorders"];

fn found(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut v = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        // only the part before the test module counts
        let lines: Vec<&str> = text.lines().collect();
        let cut = (0..lines.len()).find(|&i| lines[i].trim() == "#[cfg(test)]" && lines[i + 1..(i + 3).min(lines.len())].iter().any(|n| n.contains("mod ") && n.trim_end().ends_with('{'))).unwrap_or(lines.len());
        for l in &lines[..cut] {
            let rest = l.strip_prefix("pub struct ").or_else(|| l.strip_prefix("pub(crate) struct "));
            let Some(rest) = rest else { continue };
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if SUFFIXES.iter().any(|s| name.ends_with(s)) {
                v.push((rel.clone(), name));
            }
        }
    }
    v.sort();
    v
}

#[test]
fn every_player_reader_recorder_listener_capture_and_speaker_says_what_a_sign_out_does_to_it() {
    let root = root();
    let text = std::fs::read_to_string(root.join("docs/development/session-end-resources.txt")).unwrap();
    let mut open_max = usize::MAX;
    let mut listed: BTreeMap<(String, String), String> = BTreeMap::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(n) = l.strip_prefix("# open-max ") {
            open_max = n.trim().parse().unwrap();
            continue;
        }
        if l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        assert!(f.len() >= 3, "bad line: {l}");
        listed.insert((f[0].to_string(), f[1].to_string()), f[2].to_string());
    }
    let have = found(&root);
    for k in &have {
        assert!(listed.contains_key(k), "{} in {} is new: register an end-of-session hook for it (crates/iyakki-agent/src/sessionend_hooks.rs) and list it in docs/development/session-end-resources.txt, or classify it there", k.1, k.0);
    }
    for k in listed.keys() {
        assert!(have.contains(k), "{} in {} is listed but not found: remove the line", k.1, k.0);
    }
    let hook_sources: String = ["crates/iyakki-agent/src/sessionend_hooks.rs", "crates/iyakki-shell/src/soundowner.rs", "crates/iyakki-shell/src/shell_wake.rs"].iter().map(|p| std::fs::read_to_string(root.join(p)).unwrap()).collect();
    let mut open = 0;
    for ((p, n), st) in &listed {
        match st.as_str() {
            "drop-stops" | "continue" | "not-per-person" => {}
            "open" => open += 1,
            s if s.starts_with("registered:") => {
                let hook = &s["registered:".len()..];
                assert!(hook_sources.contains(&format!("\"{hook}\"")), "{n} in {p}: no hook named {hook} is registered");
            }
            other => panic!("{n} in {p}: unknown status {other}"),
        }
    }
    assert!(open <= open_max, "{open} open lines, the ratchet allows {open_max}");
    assert!(open >= open_max, "{open} open lines: lower `# open-max` to {open} so the gain cannot be lost");
}
