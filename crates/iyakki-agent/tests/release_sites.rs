// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The per-release site list (F-BROW-33, `dist/webview/release-sites.{sh,tsv}`): the specs the script writes are read by the real
//! browser-test parser, every site of the list has a spec, the list names only https sites with their domains, and a bad list is refused.
//! The run itself needs the full engine on a device and is not part of this test.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn have(tool: &str) -> bool {
    std::process::Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new("sh").arg(root().join("dist/webview/release-sites.sh")).args(args).output().unwrap()
}

fn out_dir(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("iyakki-release-sites-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn every_site_of_the_list_gets_a_spec_the_real_parser_accepts() {
    if !have("python3") {
        eprintln!("SKIP: python3 is not installed");
        return;
    }
    let d = out_dir("all");
    let o = run(&["--dry-run", "--out", d.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let list = std::fs::read_to_string(root().join("dist/webview/release-sites.tsv")).unwrap();
    let rows: Vec<Vec<&str>> = list.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| l.split('\t').collect()).collect();
    assert!(rows.len() >= 6, "the owner's six apps are on the list");
    for name in ["Gmail", "Outlook", "Google Docs", "Google Sheets", "Notion", "Slack"] {
        assert!(rows.iter().any(|r| r[0] == name), "{name} is on the list");
    }
    for r in &rows {
        assert!(r.len() >= 4, "{r:?}");
        assert!(r[1].starts_with("https://"), "{}: sites are opened over https", r[0]);
        let host = r[1].trim_start_matches("https://").split('/').next().unwrap();
        assert!(r[2].split(',').any(|d| d == host), "{}: the start host {host} is among the allowed domains", r[0]);
        assert!(!r[3].is_empty());
        let slug: String = r[0].to_ascii_lowercase().replace(' ', "-").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let text = std::fs::read_to_string(d.join(format!("{slug}.json"))).unwrap_or_else(|_| panic!("a spec for {}", r[0]));
        let spec = iyakki_agent::browserrun::parse_spec(&text).unwrap_or_else(|e| panic!("{}: {e}", r[0]));
        // the runner reduces a host to its registrable domain: the start host must be covered by one of them
        assert!(spec.allow_domains.iter().any(|d| host == d || host.ends_with(&format!(".{d}"))), "{}: {:?}", r[0], spec.allow_domains);
        assert_eq!(spec.timeout_ms, 60_000);
        assert!(!text.to_ascii_lowercase().contains("password"), "no secret is in a spec");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn one_site_can_be_picked_and_a_missing_list_or_option_is_refused() {
    if !have("python3") {
        return;
    }
    let d = out_dir("one");
    let o = run(&["--dry-run", "--only", "Slack", "--out", d.to_str().unwrap()]);
    assert!(o.status.success());
    assert!(d.join("slack.json").is_file() && !d.join("gmail.json").exists());
    assert_eq!(run(&["--bogus"]).status.code(), Some(2));
    assert_eq!(run(&["--dry-run", "--list", "/nonexistent/list.tsv"]).status.code(), Some(2));
    let _ = std::fs::remove_dir_all(d);
}
