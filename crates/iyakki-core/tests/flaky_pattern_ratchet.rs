//! Ratchet for the patterns that make tests flaky (owner order 2026-11-18, docs/development/FLAKY-TESTS.md).
//!
//! `dist/scripts/flaky-patterns.py --counts` scans all test code and prints `crate category risky-count`. `docs/development/flaky-ratchet.txt` holds
//! the numbers allowed today. A crate may never exceed its number (a new `sleep` used for synchronisation, an ephemeral port that is freed and
//! reused, an unlocked `set_var`, ... must use `iyakki-testkit` instead, or carry `// flaky-ok: why` when it is really safe), and when the
//! number falls the table must be lowered (`python3 dist/scripts/flaky-patterns.py --write-ratchet`), so the gain cannot be lost again.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn parse(text: &str) -> BTreeMap<(String, String), u32> {
    let mut m = BTreeMap::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = l.split_whitespace().collect();
        assert_eq!(f.len(), 3, "bad ratchet line: {l}");
        m.insert((f[0].to_string(), f[1].to_string()), f[2].parse().unwrap_or_else(|_| panic!("bad number in: {l}")));
    }
    m
}

#[test]
fn risky_test_patterns_per_crate_may_only_shrink() {
    let root = root();
    let out = Command::new("python3").arg("dist/scripts/flaky-patterns.py").arg("--counts").current_dir(&root).output().expect("python3 is needed for the flaky-pattern scan");
    assert!(out.status.success(), "flaky-patterns.py failed: {}", String::from_utf8_lossy(&out.stderr));
    let now = parse(&String::from_utf8_lossy(&out.stdout));
    let allowed = parse(&std::fs::read_to_string(root.join("docs/development/flaky-ratchet.txt")).expect("docs/development/flaky-ratchet.txt"));
    let mut more = Vec::new();
    let mut fewer = Vec::new();
    for (k, n) in &now {
        let a = allowed.get(k).copied().unwrap_or(0);
        if *n > a {
            more.push(format!("{} {}: {n} risky hits, {a} allowed", k.0, k.1));
        }
    }
    for (k, a) in &allowed {
        let n = now.get(k).copied().unwrap_or(0);
        if n < *a {
            fewer.push(format!("{} {}: {n} now, table says {a}", k.0, k.1));
        }
    }
    assert!(more.is_empty(), "WHAT: new flaky-test patterns.\nWHERE: {more:?}\nFIX: use iyakki-testkit (lock, port, wait_until, GatedChild, ManualClock; see docs/development/FLAKY-TESTS.md); list the hits with `python3 dist/scripts/flaky-patterns.py` (docs/development/FLAKY-AUDIT.md). Before: `std::thread::sleep(Duration::from_millis(50)); assert!(done());`. After: `iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || done());`."); // flaky-ok: the message names the pattern
    assert!(fewer.is_empty(), "Good: fewer risky patterns than the table allows. Lower the table: python3 dist/scripts/flaky-patterns.py --write-ratchet ({fewer:?})");
}
