//! Licence gate for the Rust dependencies: every package of both `Cargo.lock` files must be under an allowed licence, or have a row
//! in `legal/cargo-exceptions.txt` (name | version or `*` | licence exactly as the crate states it | reason | date). A row stops
//! matching when the crate's stated licence changes, so a new unknown fails the gate even for a reviewed crate.

use std::path::{Path, PathBuf};

use crate::sbom::{self, Pkg};
use crate::spdx;
use crate::table;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exception {
    pub name: String,
    pub version: String,
    pub license: String,
    pub reason: String,
    pub date: String,
    pub line: usize,
}

pub fn parse_exceptions(text: &str) -> (Vec<Exception>, Vec<String>) {
    let (mut v, mut problems) = (Vec::new(), Vec::new());
    for r in table::parse(text) {
        if r.f.len() != 5 {
            problems.push(format!("exceptions line {}: expected `name | version | licence | reason | date`", r.line));
            continue;
        }
        if !table::valid_reason(&r.f[3]) || !table::valid_date(&r.f[4]) {
            problems.push(format!("exceptions line {} ({}): needs a real reason and a date YYYY-MM-DD", r.line, r.f[0]));
            continue;
        }
        v.push(Exception { name: r.f[0].clone(), version: r.f[1].clone(), license: r.f[2].clone(), reason: r.f[3].clone(), date: r.f[4].clone(), line: r.line });
    }
    (v, problems)
}

#[derive(Debug, Default)]
pub struct Report {
    /// Blocking: unknown, not allowed and not excepted, or a source other than crates.io.
    pub failures: Vec<String>,
    /// Reviewed exceptions in use (listed so they stay visible).
    pub excepted: Vec<String>,
    /// Exception rows that match no package any more.
    pub stale: Vec<String>,
    /// Would fail, but is in a lock file only: `cargo tree` does not reach it (an optional feature that is off). Listed for review.
    pub inactive: Vec<String>,
    pub checked: usize,
}

pub fn check(pkgs: &[Pkg], exceptions: &[Exception], parse_problems: Vec<String>) -> Report {
    check_active(pkgs, exceptions, parse_problems, None)
}

/// `active`: the (name, version) pairs the build really compiles (see [`active_set`]); a failing package outside it is only listed.
pub fn check_active(pkgs: &[Pkg], exceptions: &[Exception], parse_problems: Vec<String>, active: Option<&std::collections::BTreeSet<(String, String)>>) -> Report {
    let mut r = Report { failures: parse_problems, ..Default::default() };
    let mut used = vec![false; exceptions.len()];
    let mut seen = std::collections::BTreeSet::new();
    for p in pkgs {
        if !seen.insert((p.name.clone(), p.version.clone(), p.license.clone(), p.source.clone())) {
            continue;
        }
        r.checked += 1;
        let ex = exceptions.iter().position(|e| e.name == p.name && (e.version == "*" || e.version == p.version) && e.license == p.license);
        let from_ok = p.source.is_empty() || p.source == "registry+https://github.com/rust-lang/crates.io-index" || p.source.starts_with("sparse+https://index.crates.io");
        let verdict: Result<(), String> = if p.license.is_empty() {
            Err("no licence stated".into())
        } else {
            match spdx::parse(&p.license) {
                Err(e) => Err(format!("licence `{}` does not parse: {e}", p.license)),
                Ok(e) => spdx::satisfied(&e, spdx::CARGO_ALLOW).map_err(|bad| format!("licence `{}` is not on the allow list ({})", p.license, bad.join(", "))),
            }
        };
        let id = format!("{} {} [{}]", p.name, p.version, p.origin);
        match (verdict, from_ok, ex) {
            (Ok(()), true, _) => {}
            (v, ok_src, Some(i)) => {
                used[i] = true;
                if v.is_ok() && !ok_src {
                    // an exception covers the licence, not a foreign source
                    r.failures.push(format!("{id}: source `{}` is not crates.io", p.source));
                } else {
                    r.excepted.push(format!("{id}: {} -- {} ({})", p.license, exceptions[i].reason, exceptions[i].date));
                }
            }
            (Err(why), _, None) => push(&mut r, active, p, format!("{id}: {why}")),
            (Ok(()), false, None) => push(&mut r, active, p, format!("{id}: source `{}` is not crates.io", p.source)),
        }
    }
    for (i, e) in exceptions.iter().enumerate() {
        if !used[i] {
            r.stale.push(format!("exceptions line {}: {} {} `{}` matches no package (crate removed, or its licence text changed)", e.line, e.name, e.version, e.license));
        }
    }
    r
}

fn push(r: &mut Report, active: Option<&std::collections::BTreeSet<(String, String)>>, p: &Pkg, msg: String) {
    if active.is_some_and(|a| !a.contains(&(p.name.clone(), p.version.clone()))) {
        r.inactive.push(format!("{msg} (in a lock file only: not reached by `cargo tree`, an optional feature that is off)"));
    } else {
        r.failures.push(msg);
    }
}

/// The packages `cargo tree` reaches for the workspace and for the GUI workspace (normal and build dependencies, all targets), or
/// `None` when cargo cannot say (then every locked package counts).
pub fn active_set(root: &Path) -> Option<std::collections::BTreeSet<(String, String)>> {
    let mut set = std::collections::BTreeSet::new();
    for manifest in ["Cargo.toml", "crates/iyakki-gui/Cargo.toml"] {
        if !root.join(manifest).is_file() {
            continue;
        }
        let out = std::process::Command::new("cargo").current_dir(root).args(["tree", "--offline", "--target", "all", "-e", "normal,build", "--prefix", "none", "--manifest-path", manifest]).args(if manifest == "Cargo.toml" { vec!["--workspace"] } else { vec![] }).output().ok()?;
        if !out.status.success() {
            return None;
        }
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            let mut w = l.split_whitespace();
            if let (Some(n), Some(v)) = (w.next(), w.next().and_then(|v| v.strip_prefix('v'))) {
                set.insert((n.to_string(), v.to_string()));
            }
        }
    }
    (!set.is_empty()).then_some(set)
}

/// Read both lock files with licences from the local registry. `registry` as for [`sbom::collect`].
pub fn load(root: &Path, registry: &[PathBuf]) -> Vec<Pkg> {
    sbom::collect(root, registry).into_iter().filter(|p| p.origin.starts_with("cargo:")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, lic: &str, src: &str) -> Pkg {
        Pkg { name: name.into(), version: "1.0.0".into(), license: lic.into(), source: src.into(), sha256: String::new(), origin: "cargo:Cargo.lock".into() }
    }
    const CRATES: &str = "registry+https://github.com/rust-lang/crates.io-index";

    #[test]
    fn a_deliberately_bad_lock_entry_fails_and_an_exception_with_a_reason_lets_it_pass() {
        let lock = "[[package]]\nname = \"fine\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"badcrate\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"gitdep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.invalid/x\"\n";
        let d = std::env::temp_dir().join(format!("iyakki-relcheck-cargolic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, l) in [("fine", "MIT"), ("badcrate", "GPL-3.0-only"), ("gitdep", "MIT")] {
            std::fs::create_dir_all(d.join(format!("{n}-{}", if n == "gitdep" { "0.1.0" } else { "1.0.0" }))).unwrap();
            std::fs::write(d.join(format!("{n}-{}/Cargo.toml", if n == "gitdep" { "0.1.0" } else { "1.0.0" })), format!("[package]\nlicense = \"{l}\"\n")).unwrap();
        }
        let mut p = sbom::parse_lock(lock, "cargo:Cargo.lock");
        sbom::fill_licenses(&mut p, &[d.clone()]);
        let r = check(&p, &[], vec![]);
        assert_eq!(r.failures.len(), 2, "{:?}", r.failures);
        assert!(r.failures.iter().any(|f| f.contains("badcrate") && f.contains("GPL-3.0-only")) && r.failures.iter().any(|f| f.contains("gitdep") && f.contains("not crates.io")));
        let (ex, probs) = parse_exceptions("badcrate | 1.0.0 | GPL-3.0-only | linked only in a test binary, reviewed against its LICENSE | 2026-10-26\n");
        assert!(probs.is_empty());
        let r = check(&p, &ex, vec![]);
        assert_eq!((r.failures.len(), r.excepted.len()), (1, 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_and_changed_licences_fail_and_stale_rows_show() {
        let unknown = pkg("nolic", "", CRATES);
        assert_eq!(check(&[unknown.clone()], &[], vec![]).failures.len(), 1);
        // a failing package that the build never compiles is listed, not failed
        let active: std::collections::BTreeSet<(String, String)> = [("other".to_string(), "1.0.0".to_string())].into_iter().collect();
        let r = check_active(&[unknown], &[], vec![], Some(&active));
        assert_eq!((r.failures.len(), r.inactive.len()), (0, 1));
        let (ex, _) = parse_exceptions("c | * | MPL-2.0 | file-level copyleft, we do not modify it, reviewed | 2026-10-26\ngone | * | MIT | crate was removed from the tree | 2026-10-26\n");
        let r = check(&[pkg("c", "MPL-2.0", CRATES)], &ex, vec![]);
        assert!(r.failures.is_empty() && r.excepted.len() == 1 && r.stale.len() == 1, "{r:?}");
        // the crate's licence changes in an update: the reviewed row no longer applies
        let r = check(&[pkg("c", "MPL-2.0 OR GPL-3.0-only", CRATES)], &ex, vec![]);
        assert_eq!(r.failures.len(), 1);
        // rows without a reason or date are refused
        let (ex, probs) = parse_exceptions("c | * | MPL-2.0 | ok | 2026-10-26\nd | * | MIT | a perfectly good reason | whenever\n");
        assert!(ex.is_empty() && probs.len() == 2);
    }
}
