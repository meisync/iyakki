//! Version consistency: the workspace version, the separate GUI workspace, both lock files, the changelog and the release notes
//! must all name the same version, and it must be a valid semantic version.

use std::path::Path;

/// `MAJOR.MINOR.PATCH[-prerelease][+build]` with the usual rules (numbers without leading zeros, non-empty identifiers).
pub fn is_semver(v: &str) -> bool {
    let (core, rest) = match v.find(['-', '+']) {
        Some(i) => (&v[..i], &v[i..]),
        None => (v, ""),
    };
    let nums: Vec<&str> = core.split('.').collect();
    if nums.len() != 3 || nums.iter().any(|n| n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) || (n.len() > 1 && n.starts_with('0'))) {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    let (pre, build) = match rest.strip_prefix('-') {
        Some(r) => match r.split_once('+') {
            Some((p, b)) => (Some(p), Some(b)),
            None => (Some(r), None),
        },
        None => (None, rest.strip_prefix('+')),
    };
    let ids_ok = |s: &str| !s.is_empty() && s.split('.').all(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    pre.is_none_or(ids_ok) && build.is_none_or(ids_ok)
}

/// The first `version = "x"` of a Cargo.toml `[package]` or `[workspace.package]` section.
pub fn manifest_version(toml: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[package]" || t == "[workspace.package]";
            continue;
        }
        if in_pkg {
            if let Some(v) = t.strip_prefix("version") {
                let v = v.trim_start();
                if let Some(v) = v.strip_prefix('=') {
                    let v = v.trim().trim_matches('"');
                    if !v.is_empty() && !v.starts_with('{') {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// `(name, version)` of the packages of a Cargo.lock that live in this repository (no `source` line) and are called `iyakki-*`.
pub fn lock_own_packages(lock: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let (mut name, mut version, mut source) = (None, None, false);
        for l in block.lines() {
            if let Some(v) = l.strip_prefix("name = ") {
                name = Some(v.trim_matches('"').to_string());
            } else if let Some(v) = l.strip_prefix("version = ") {
                version = Some(v.trim_matches('"').to_string());
            } else if l.starts_with("source = ") {
                source = true;
            }
        }
        if let (Some(n), Some(v), false) = (name, version, source) {
            if n.starts_with("iyakki") {
                out.push((n, v));
            }
        }
    }
    out
}

/// What is wrong, as sentences; empty when everything agrees.
pub fn check(root: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok();
    let Some(ws) = read("Cargo.toml").as_deref().and_then(manifest_version) else {
        return vec!["Cargo.toml has no [workspace.package] version".into()];
    };
    if !is_semver(&ws) {
        bad.push(format!("the workspace version {ws:?} is not a semantic version"));
    }
    // every crate inherits the workspace version (or states the same one)
    for e in std::fs::read_dir(root.join("crates")).into_iter().flatten().flatten() {
        let m = e.path().join("Cargo.toml");
        let Ok(t) = std::fs::read_to_string(&m) else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let inherits = t.lines().any(|l| l.trim().replace(' ', "") == "version.workspace=true");
        if !inherits {
            match manifest_version(&t) {
                Some(v) if v == ws => {}
                Some(v) => bad.push(format!("crates/{name} says version {v}, the workspace says {ws}")),
                None => bad.push(format!("crates/{name} has no version")),
            }
        }
    }
    for lock in ["Cargo.lock", "crates/iyakki-gui/Cargo.lock"] {
        match read(lock) {
            Some(t) => {
                for (n, v) in lock_own_packages(&t) {
                    if v != ws {
                        bad.push(format!("{lock}: {n} is at {v}, expected {ws} (run cargo build to update it)"));
                    }
                }
            }
            None => bad.push(format!("{lock} is missing")),
        }
    }
    match read("CHANGELOG.md") {
        Some(t) if t.lines().any(|l| l.starts_with(&format!("## [{ws}]"))) => {}
        Some(_) => bad.push(format!("CHANGELOG.md has no `## [{ws}]` entry")),
        None => bad.push("CHANGELOG.md is missing".into()),
    }
    if !root.join(format!("RELEASE-NOTES-{ws}.md")).exists() {
        bad.push(format!("RELEASE-NOTES-{ws}.md is missing"));
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_rules() {
        for ok in ["0.1.0", "0.1.0-alpha.1", "1.2.3-rc.1+build.5", "1.0.0+x", "10.20.30"] {
            assert!(is_semver(ok), "{ok}");
        }
        for bad in ["0.1", "01.2.3", "1.2.3-", "1.2.3-a..b", "v1.2.3", "1.2.x", "", "1.2.3+"] {
            assert!(!is_semver(bad), "{bad}");
        }
    }

    #[test]
    fn manifest_and_lock_are_read() {
        assert_eq!(manifest_version("[workspace]\nmembers=[]\n[workspace.package]\nversion = \"0.1.0-alpha.1\"\nedition = \"2021\"\n").as_deref(), Some("0.1.0-alpha.1"));
        assert_eq!(manifest_version("[package]\nname = \"x\"\nversion.workspace = true\n"), None);
        assert_eq!(manifest_version("[dependencies]\nfoo = { version = \"1\" }\n"), None);
        let lock = "[[package]]\nname = \"iyakki-sec\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"iyakki-fake\"\nversion = \"9.9.9\"\nsource = \"registry+x\"\n";
        assert_eq!(lock_own_packages(lock), vec![("iyakki-sec".to_string(), "0.1.0".to_string())]);
    }

    #[test]
    fn a_made_up_repo_is_checked() {
        let d = std::env::temp_dir().join(format!("iyakki-relcheck-version-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("crates/a")).unwrap();
        std::fs::create_dir_all(d.join("crates/iyakki-gui")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[workspace.package]\nversion = \"0.2.0\"\n").unwrap();
        std::fs::write(d.join("crates/a/Cargo.toml"), "[package]\nname=\"a\"\nversion.workspace = true\n").unwrap();
        std::fs::write(d.join("Cargo.lock"), "[[package]]\nname = \"iyakki-a\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(d.join("crates/iyakki-gui/Cargo.lock"), "[[package]]\nname = \"iyakki-a\"\nversion = \"0.2.0\"\n").unwrap();
        let bad = check(&d);
        assert!(bad.iter().any(|b| b.contains("Cargo.lock: iyakki-a is at 0.1.0")), "{bad:?}");
        assert!(bad.iter().any(|b| b.contains("CHANGELOG.md is missing")) && bad.iter().any(|b| b.contains("RELEASE-NOTES-0.2.0.md")));
        std::fs::write(d.join("CHANGELOG.md"), "# Changelog\n## [0.2.0] - 2026-01-01\n").unwrap();
        std::fs::write(d.join("RELEASE-NOTES-0.2.0.md"), "x").unwrap();
        std::fs::write(d.join("Cargo.lock"), "[[package]]\nname = \"iyakki-a\"\nversion = \"0.2.0\"\n").unwrap();
        assert_eq!(check(&d), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&d);
    }
}
