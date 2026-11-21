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

fn signed_in_dir(tag: &str) -> PathBuf {
    let d = out_dir(tag).join("signed-in");
    std::fs::create_dir_all(&d).unwrap();
    // a fixture of what the owner supplies for one site: the steps after the goto, secrets by variable name, and a text only a signed-in page shows
    std::fs::write(
        d.join("slack.json"),
        r#"{"steps":[{"fill":{"label":"Email","value_env":"IYAKKI_TEST_SLACK_USER"}},{"fill":{"label":"Password","value_env":"IYAKKI_TEST_SLACK_PASSWORD"}},{"click":{"role":"button","name":"Sign in"}}],"ok_text":"All unreads"}"#,
    )
    .unwrap();
    std::fs::write(d.join("notion.json"), r#"{"steps":[],"ok_text":""}"#).unwrap();
    d
}

#[test]
fn the_signed_in_pass_builds_specs_the_real_parser_accepts_with_secrets_by_variable_name_and_skips_what_it_cannot_run() {
    if !have("python3") {
        return;
    }
    let d = out_dir("signed");
    let sd = signed_in_dir("signed-dir");
    let o = run(&["--dry-run", "--signed-in", sd.to_str().unwrap(), "--out", d.to_str().unwrap()]);
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success(), "{out} {}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(d.join("slack-signed-in.json")).unwrap();
    let spec = iyakki_agent::browserrun::parse_spec(&text).unwrap();
    // goto, two secret fills, a click, the signed-in text, the wall check, the screenshot
    assert_eq!(spec.steps.len(), 7, "{text}");
    assert!(text.contains("IYAKKI_TEST_SLACK_PASSWORD") && text.contains("All unreads") && !text.to_ascii_lowercase().contains("hunter"));
    // a site with no steps file is skipped, and so is a steps file that says nothing
    assert!(out.contains("Gmail") && out.contains("skip") && out.contains("no "), "{out}");
    assert!(out.contains("Notion") && out.contains("needs steps and ok_text"), "{out}");
    assert!(!d.join("gmail-signed-in.json").exists() && !d.join("notion-signed-in.json").exists());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_signed_in_run_without_its_secrets_is_skipped_naming_the_variables_and_with_them_runs_without_the_values_in_any_file_or_line() {
    if !have("python3") {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let d = out_dir("signedrun");
    let sd = signed_in_dir("signedrun-dir");
    // a stand-in `iyakki` that records the spec it was given and passes
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("iyakki");
    std::fs::write(&fake, "#!/bin/sh\ncp \"$2\" \"$(dirname \"$0\")/seen.json\"\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let go = |secrets: bool| {
        let mut c = std::process::Command::new("sh");
        c.arg(root().join("dist/webview/release-sites.sh")).args(["--only", "Slack", "--signed-in", sd.to_str().unwrap(), "--out", d.join("out").to_str().unwrap()]).env("PATH", &path).env_remove("IYAKKI_TEST_SLACK_USER").env_remove("IYAKKI_TEST_SLACK_PASSWORD");
        if secrets {
            c.env("IYAKKI_TEST_SLACK_USER", "tester@example.org").env("IYAKKI_TEST_SLACK_PASSWORD", "correct-horse-battery");
        }
        c.output().unwrap()
    };
    let o = go(false);
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success() && out.contains("skip") && out.contains("IYAKKI_TEST_SLACK_PASSWORD") && out.contains("IYAKKI_TEST_SLACK_USER") && !bin.join("seen.json").exists(), "{out}");
    let o = go(true);
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success() && out.contains("Slack") && out.contains("pass"), "{out} {}", String::from_utf8_lossy(&o.stderr));
    assert!(bin.join("seen.json").exists(), "the runner got the spec");
    for f in std::fs::read_dir(d.join("out")).unwrap().flatten() {
        if f.path().is_file() {
            let t = std::fs::read_to_string(f.path()).unwrap_or_default();
            assert!(!t.contains("correct-horse") && !t.contains("tester@example.org"), "a secret reached {}", f.path().display());
        }
    }
    assert!(!out.contains("correct-horse") && !out.contains("tester@example.org"));
    let _ = std::fs::remove_dir_all(d);
}
