//! Test hygiene: tests must never write under the real home folder (they once created `a` and `ask-first` in the real `~/Flows`).
//!
//! * `test_modules_do_not_use_the_real_home_without_a_temp_override` (always runs): a test file, or the part of a source file after
//!   `#[cfg(test)]`, may not read `HOME` / call `home_dir()` unless the line says `home-ok: <why>` (a read that writes nothing) .
//! * `cargo_test_leaves_the_real_home_alone` (only with `IYAKKI_HOME_HYGIENE=1`; `dist/release/home-hygiene.sh` sets it): runs the unit
//!   tests of `iyakki-agent` and `iyakki-flow` and fails if the names in `$HOME` or `$HOME/Flows` (and the other well-known folders)
//!   changed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "node_modules") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn test_modules_do_not_use_the_real_home_without_a_temp_override() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rs_files(&crates, &mut files);
    assert!(files.len() > 100, "the scan found {} files: wrong folder?", files.len());
    let needles = ["var(\"HOME\")", "var_os(\"HOME\")", "home_dir()"];
    let mut bad = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        if f.file_name().is_some_and(|n| n == "test_hygiene.rs") {
            continue;
        }
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let whole = name.ends_with("tests.rs") || f.components().any(|c| c.as_os_str() == "tests");
        let start = if whole { Some(0) } else { text.find("#[cfg(test)]") };
        let Some(start) = start else { continue };
        let first_line = text[..start].matches('\n').count();
        for (i, l) in text[start..].lines().enumerate() {
            if needles.iter().any(|n| l.contains(n)) && !l.contains("home-ok") && !l.trim_start().starts_with("//") {
                bad.push(format!("{}:{}: {}", f.strip_prefix(&crates).unwrap_or(&f).display(), first_line + i + 1, l.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "a test reads the real HOME; give it a temp home (crate::testdir::unique) or mark a read-only use with `// home-ok: why`:\n{}", bad.join("\n"));
}

fn names(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
}

#[test]
fn cargo_test_leaves_the_real_home_alone() {
    if std::env::var_os("IYAKKI_HOME_HYGIENE").is_none() {
        eprintln!("skipped: set IYAKKI_HOME_HYGIENE=1 (dist/release/home-hygiene.sh)");
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let watched = ["", "Flows", "Pictures", "Documents", "Downloads", "Uploads", "Videos", "Music"];
    let snap = || watched.iter().map(|w| (w.to_string(), names(&home.join(w)))).collect::<Vec<_>>();
    let before = snap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for pkg in ["iyakki-agent", "iyakki-flow"] {
        let st = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .current_dir(&root)
            .args(["test", "-q", "-p", pkg, "--lib"])
            .env_remove("IYAKKI_HOME_HYGIENE")
            .env_remove("IYAKKI_NO_HOST_HOME")
            .env("CARGO_BUILD_JOBS", "3")
            .status()
            .expect("cargo");
        // a failing test is another gate's business (a machine-dependent test must not hide a home-folder leak): only report it
        if !st.success() {
            eprintln!("note: cargo test -p {pkg} --lib reported failures; only the home folder is judged here");
        }
    }
    for ((w, b), (_, a)) in before.iter().zip(snap().iter()) {
        let added: Vec<_> = a.difference(b).collect();
        let removed: Vec<_> = b.difference(a).collect();
        assert!(added.is_empty() && removed.is_empty(), "~/{w}: tests added {added:?} and removed {removed:?}");
    }
}
