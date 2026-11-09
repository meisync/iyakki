//! Dependency-rule ratchet (UI/OS split plan, step 1; docs/development/UI-OS-SPLIT-PLAN.md).
//!
//! Rule: tiles in `crates/iyakki-shell/src/apps/` do not reach the kernel or the hardware crates themselves; they ask IYAKKI core
//! (`iyakki-core-api`, once the slices of the plan are moved). Today there are violations. `data/shell_kernel_access.allow` lists
//! them, one line per file (`path count`). The list may only shrink: a file that gains a violation, or a new file with one, fails;
//! a file with fewer violations than listed fails too, until its number is lowered (or the line removed), so progress is locked in.
//!
//! What counts (one per line, non-test code only: everything from the first `#[cfg(test)]` on is skipped, and so are `*_tests.rs`,
//! `tests.rs` and `testdata`; comment lines are skipped): a literal `"/proc/`, `"/sys/` or `"/dev/` path; `Command::new(` or a
//! `process::Command` path; a use of `iyakki_devctl`, `iyakki_storage`, `iyakki_usb`, `iyakki_drivers`, `iyakki_netscan`,
//! `iyakki_hwinfo` or `iyakki_zram`.
//!
//! To print the current table: `IYAKKI_RATCHET_PRINT=1 cargo test -p iyakki-core --test dependency_ratchet -- --nocapture`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ALLOW: &str = include_str!("data/shell_kernel_access.allow");
const HARDWARE_CRATES: [&str; 7] = ["iyakki_devctl", "iyakki_storage", "iyakki_usb", "iyakki_drivers", "iyakki_netscan", "iyakki_hwinfo", "iyakki_zram"];
const PATH_LITERALS: [&str; 6] = ["\"/proc/", "\"/sys/", "\"/dev/", "\"/proc\"", "\"/sys\"", "\"/dev\""];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "testdata") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if name.ends_with("_tests.rs") || name == "tests.rs" {
                continue;
            }
            out.push(p);
        }
    }
}

/// Is this line a kernel or hardware access (see the module comment)?
fn is_violation(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") {
        return false;
    }
    if PATH_LITERALS.iter().any(|p| line.contains(p)) {
        return true;
    }
    if line.contains("Command::new(") || line.contains("process::Command") {
        return true;
    }
    HARDWARE_CRATES.iter().any(|c| line.contains(c))
}

/// The file without its test module: everything from the first `#[cfg(test)]` that is followed by a `mod` item (test modules
/// close the file in this code base). A `#[cfg(test)]` on a single function does not end the scan.
fn non_test_code(text: &str) -> &str {
    let mut at = 0;
    while let Some(i) = text[at..].find("#[cfg(test)]") {
        let start = at + i;
        let after = &text[start + "#[cfg(test)]".len()..];
        let next = after.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("#[") && !l.starts_with("//"));
        if next.is_some_and(|l| l.starts_with("mod ") || l.starts_with("pub mod ") || l.starts_with("pub(crate) mod ")) {
            return &text[..start];
        }
        at = start + 1;
    }
    text
}

/// Violations in the non-test code of one file.
fn count(text: &str) -> usize {
    non_test_code(text).lines().filter(|l| is_violation(l)).count()
}

fn current() -> BTreeMap<String, usize> {
    let r = root();
    let mut files = Vec::new();
    rust_files(&r.join("crates/iyakki-shell/src/apps"), &mut files);
    let mut out = BTreeMap::new();
    for f in files {
        let n = count(&std::fs::read_to_string(&f).unwrap_or_default());
        if n > 0 {
            out.insert(f.strip_prefix(&r).unwrap().to_string_lossy().into_owned(), n);
        }
    }
    out
}

fn allowed() -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (i, l) in ALLOW.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (path, n) = l.rsplit_once(' ').unwrap_or_else(|| panic!("allow list line {}: expected `path count`", i + 1));
        let n: usize = n.trim().parse().unwrap_or_else(|_| panic!("allow list line {}: bad count", i + 1));
        assert!(m.insert(path.trim().to_string(), n).is_none(), "allow list line {}: {path} listed twice", i + 1);
    }
    m
}

#[test]
fn tiles_do_not_gain_kernel_or_hardware_access_and_the_list_only_shrinks() {
    let now = current();
    if std::env::var_os("IYAKKI_RATCHET_PRINT").is_some() {
        for (p, n) in &now {
            println!("{p} {n}");
            let text = std::fs::read_to_string(root().join(p)).unwrap_or_default();
            for l in non_test_code(&text).lines().filter(|l| is_violation(l)) {
                println!("    {}", l.trim().chars().take(140).collect::<String>());
            }
        }
    }
    let allow = allowed();
    let mut grew = Vec::new();
    let mut lower = Vec::new();
    for (path, n) in &now {
        match allow.get(path) {
            None => grew.push(format!("{path}: {n} direct kernel/hardware access(es), not in the allow list (new file or newly added)")),
            Some(a) if n > a => grew.push(format!("{path}: {n} now, {a} allowed")),
            Some(a) if n < a => lower.push(format!("{path}: {n} now, {a} listed -> lower the number to {n}")),
            _ => {}
        }
    }
    for (path, a) in &allow {
        if !now.contains_key(path) {
            lower.push(format!("{path}: no violations left (listed {a}) -> remove this line"));
        }
    }
    assert!(
        grew.is_empty(),
        "Tiles must not read /proc, /sys, /dev, spawn programs or use the hardware crates directly; ask IYAKKI core instead \
         (docs/development/UI-OS-SPLIT-PLAN.md). Offending files:\n  {}",
        grew.join("\n  ")
    );
    assert!(
        lower.is_empty(),
        "Good: fewer violations than the allow list says. Lower the list so it cannot grow back \
         (crates/iyakki-core/tests/data/shell_kernel_access.allow):\n  {}",
        lower.join("\n  ")
    );
}

#[test]
fn the_scanner_counts_what_it_says_and_skips_tests_and_comments() {
    assert!(is_violation(r#"let s = std::fs::read_to_string("/proc/meminfo");"#));
    assert!(is_violation(r#"Command::new("amixer")"#));
    assert!(is_violation("let d = iyakki_storage::list();"));
    assert!(!is_violation(r#"// reads "/proc/meminfo""#));
    assert!(!is_violation("let x = 1;"));
    assert_eq!(count("fn a() { Command::new(\"x\"); }\n#[cfg(test)]\nmod t { fn b() { Command::new(\"y\"); } }"), 1);
    assert_eq!(count("#[cfg(test)]\nfn helper() { Command::new(\"a\"); }\nfn b() { Command::new(\"b\"); }"), 2, "a test-only function does not end the scan");
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Crate-level rules for the split of the shell (docs/development/COMPILE-SPEED-PLAN.md, Phase 2; docs/development/COMPILE-SPLIT-PILOT.md).
//
// 1. `iyakki-uikit` (the toolkit: gfx, fx, ui, text fields ...) depends on no `iyakki-*` crate, so a tile crate that uses it never pulls in
//    the agent. (It is not called `iyakki-ui`: that name is the older headless editor model that `iyakki-gui` uses.)
// 2. A tile crate (`crates/iyakki-tile-*`) depends on `iyakki-uikit` and `iyakki-tile-api` and on no other `iyakki-*` crate, except the
//    lines in `data/tile_crate_deps.allow` (which may only shrink). The tile-ABI crates and the facade are not tile crates; they are
//    listed in `TILE_INFRA`.
// 3. The facade `iyakki-tile-api` depends on `iyakki-uikit` only (never the agent).

const TILE_ALLOW: &str = include_str!("data/tile_crate_deps.allow");
const UI_CRATE: &str = "iyakki-uikit";
const API_CRATE: &str = "iyakki-tile-api";
/// Crates under `iyakki-tile-*` that are the WASM tile interface and its host, not tiles. Not covered by rule 2.
const TILE_INFRA: [&str; 4] = ["iyakki-tile-abi", "iyakki-tile-host", "iyakki-tile-example", API_CRATE];

/// The `iyakki-*` crates a manifest names in `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` (also `[target.*.…]`).
fn iyakki_deps(manifest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            let name = l.trim_matches(|c| c == '[' || c == ']');
            in_deps = name.ends_with("dependencies");
            continue;
        }
        if !in_deps || l.starts_with('#') {
            continue;
        }
        let key = l.split(['=', '.', ' ']).next().unwrap_or("");
        if key.starts_with("iyakki-") {
            out.push(key.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn manifest(krate: &str) -> String {
    std::fs::read_to_string(root().join("crates").join(krate).join("Cargo.toml")).unwrap_or_else(|e| panic!("crates/{krate}/Cargo.toml: {e}"))
}

#[test]
fn the_toolkit_crate_depends_on_no_iyakki_crate() {
    let deps = iyakki_deps(&manifest(UI_CRATE));
    assert!(deps.is_empty(), "{UI_CRATE} must not depend on any iyakki-* crate (it is what tile crates build on, without the agent); found {deps:?}");
}

#[test]
fn the_tile_facade_depends_on_the_toolkit_only() {
    let deps = iyakki_deps(&manifest(API_CRATE));
    assert_eq!(deps, vec![UI_CRATE.to_string()], "{API_CRATE} may depend on {UI_CRATE} and nothing else named iyakki-* (found {deps:?})");
}

fn tile_allowed() -> Vec<(String, String)> {
    TILE_ALLOW
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut w = l.split_whitespace();
            (w.next().unwrap().to_string(), w.next().unwrap_or_else(|| panic!("tile allow list: `{l}` needs <crate> <dependency>")).to_string())
        })
        .collect()
}

#[test]
fn tile_crates_depend_only_on_the_toolkit_and_the_list_only_shrinks() {
    let allow = tile_allowed();
    let mut used = Vec::new();
    let mut bad = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(root().join("crates"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("iyakki-tile-") && !TILE_INFRA.contains(&n.as_str()))
        .collect();
    names.sort();
    for name in &names {
        for dep in iyakki_deps(&manifest(name)) {
            if dep == UI_CRATE || dep == API_CRATE {
                continue;
            }
            if allow.iter().any(|(c, d)| c == name && *d == dep) {
                used.push((name.clone(), dep));
            } else {
                bad.push(format!("{name} -> {dep}"));
            }
        }
    }
    assert!(bad.is_empty(), "A tile crate may depend only on {UI_CRATE} and {API_CRATE} (or a documented exception in tests/data/tile_crate_deps.allow): {bad:?}");
    let stale: Vec<_> = allow.iter().filter(|a| !used.contains(a)).collect();
    assert!(stale.is_empty(), "Good: these exceptions are gone; remove them from tests/data/tile_crate_deps.allow: {stale:?}");
}

#[test]
fn the_manifest_scanner_reads_dependency_tables_only() {
    let m = "[package]\nname = \"iyakki-x\"\n[dependencies]\niyakki-uikit = { path = \"../iyakki-uikit\" }\niyakki-sec.workspace = true\nserde = \"1\"\n# iyakki-no = 1\n[dev-dependencies]\niyakki-agent = { path = \"../a\" }\n[features]\niyakki-f = []\n";
    assert_eq!(iyakki_deps(m), vec!["iyakki-agent", "iyakki-sec", "iyakki-uikit"]);
}

/// The feature crates split out of `iyakki-agent` (docs/development/COMPILE-SPLIT-PILOT-2.md) may depend on small leaf crates only. If one of
/// them gained a dependency on the agent, the shell or a tile, the split would be undone (the crate would rebuild whenever the agent does).
#[test]
fn the_crates_split_out_of_the_agent_depend_on_small_leaf_crates_only() {
    let allowed: [(&str, &[&str]); 3] = [("iyakki-sshfs", &["iyakki-sec"]), ("iyakki-camdetect", &["iyakki-sec", "iyakki-volcrypt"]), ("iyakki-diskcrypt", &[])];
    for (krate, ok) in allowed {
        let deps = iyakki_deps(&manifest(krate));
        let extra: Vec<&String> = deps.iter().filter(|d| !ok.contains(&d.as_str())).collect();
        assert!(extra.is_empty(), "{krate} may depend on {ok:?} only (found {deps:?}); never on iyakki-agent, iyakki-shell or a tile crate");
    }
}
