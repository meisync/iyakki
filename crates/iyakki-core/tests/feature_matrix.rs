//! Keeps FEATURES.md honest. A row may not cite a test that does not exist, claim `done` without
//! evidence, or contradict its own progress; the totals line must match the rows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const STATUSES: [&str; 5] = ["done", "partial", "planned", "blocked", "deprecated"];
const PHASES: [&str; 11] = ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10"];
const PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Does `crates/<krate>` define `fn <name>(`?
fn test_exists(root: &Path, krate: &str, name: &str, cache: &mut BTreeMap<String, String>) -> bool {
    let src = cache.entry(krate.to_string()).or_insert_with(|| {
        let mut files = Vec::new();
        for sub in ["src", "tests"] {
            rust_files(&root.join("crates").join(krate).join(sub), &mut files);
        }
        files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect::<Vec<_>>().join("\n")
    });
    let needle = format!("fn {name}(");
    src.contains(&needle)
}

struct Row {
    id: String,
    status: String,
    progress: i64,
    tests: Vec<String>,
    notes: String,
}

/// Returns the problems found (empty = consistent).
fn check_matrix(text: &str, root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut seen = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        if !line.starts_with("| F-") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let at = format!("line {}", n + 1);
        if cells.len() != 8 {
            problems.push(format!("{at}: expected 8 columns, found {}", cells.len()));
            continue;
        }
        let (id, name, phase, status, pri, prog, tests, notes) = (cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7]);
        let id_ok = id.strip_prefix("F-").and_then(|r| r.rsplit_once('-')).is_some_and(|(area, num)| !area.is_empty() && area.starts_with(|c: char| c.is_ascii_uppercase()) && area.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && num.len() == 2 && num.chars().all(|c| c.is_ascii_digit()));
        if !id_ok {
            problems.push(format!("{at}: bad ID `{id}` (want F-AREA-nn)"));
        }
        if !seen.insert(id.to_string()) {
            problems.push(format!("{at}: duplicate ID {id}"));
        }
        if name.is_empty() {
            problems.push(format!("{id}: empty feature name"));
        }
        if !PHASES.contains(&phase) {
            problems.push(format!("{id}: unknown phase `{phase}`"));
        }
        if !STATUSES.contains(&status) {
            problems.push(format!("{id}: unknown status `{status}`"));
        }
        if !PRIORITIES.contains(&pri) {
            problems.push(format!("{id}: unknown priority `{pri}`"));
        }
        let progress = prog.strip_suffix('%').and_then(|p| p.parse::<i64>().ok());
        let Some(progress) = progress.filter(|p| (0..=100).contains(p)) else {
            problems.push(format!("{id}: progress `{prog}` must be 0%..100%"));
            continue;
        };
        let tests: Vec<String> = if tests == "—" { vec![] } else { tests.split("<br>").map(|t| t.trim().trim_matches('`').to_string()).collect() };
        rows.push(Row { id: id.to_string(), status: status.to_string(), progress, tests, notes: notes.to_string() });
    }
    if rows.is_empty() {
        problems.push("no feature rows found".into());
    }
    let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut cache = BTreeMap::new();
    for r in &rows {
        match r.status.as_str() {
            "done" if r.progress != 100 => problems.push(format!("{}: `done` must be 100% (is {}%)", r.id, r.progress)),
            "planned" if r.progress != 0 => problems.push(format!("{}: `planned` must be 0% (is {}%)", r.id, r.progress)),
            "partial" if !(1..=99).contains(&r.progress) => problems.push(format!("{}: `partial` must be 1%..99% (is {}%)", r.id, r.progress)),
            "blocked" if r.progress == 100 => problems.push(format!("{}: `blocked` cannot be 100%", r.id)),
            _ => {}
        }
        if matches!(r.status.as_str(), "done" | "partial" | "deprecated") && r.tests.is_empty() {
            problems.push(format!("{}: `{}` needs at least one test or script", r.id, r.status));
        }
        if matches!(r.status.as_str(), "deprecated" | "blocked" | "partial") && r.notes.is_empty() {
            problems.push(format!("{}: `{}` needs a note saying why", r.id, r.status));
        }
        if r.status == "deprecated" {
            let replaced: Vec<&str> = r.notes.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).filter(|w| w.starts_with("F-")).collect();
            let valid = replaced.iter().any(|w| ids.contains(*w) && *w != r.id);
            if !valid && !r.notes.contains("removed") {
                problems.push(format!("{}: a deprecated row must name its replacement (an existing F-… ID) or say it is removed", r.id));
            }
        }
        for t in &r.tests {
            if let Some(path) = t.strip_prefix("script:") {
                if !root.join(path).exists() {
                    problems.push(format!("{}: script `{path}` does not exist", r.id));
                }
                continue;
            }
            let parts: Vec<&str> = t.split("::").collect();
            let (Some(krate), Some(name)) = (parts.first(), parts.last()) else { continue };
            if parts.len() < 2 || !root.join("crates").join(krate).is_dir() {
                problems.push(format!("{}: `{t}` does not name a crate and a test", r.id));
            } else if !test_exists(root, krate, name, &mut cache) {
                problems.push(format!("{}: test `{t}` not found in crates/{krate}", r.id));
            }
        }
    }
    // the totals line
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        *count.entry(r.status.as_str()).or_default() += 1;
    }
    let expected = STATUSES.iter().map(|s| format!("{s} {}", count.get(s).copied().unwrap_or(0))).collect::<Vec<_>>().join(" · ") + &format!(" · total {}", rows.len());
    let found = text.split("<!-- summary:start -->").nth(1).and_then(|r| r.split("<!-- summary:end -->").next()).unwrap_or("").to_string();
    if found != expected {
        problems.push(format!("the totals line is stale: it says `{found}`, the rows say `{expected}`"));
    }
    problems
}

#[test]
fn matrix_is_consistent() {
    let root = repo_root();
    let text = std::fs::read_to_string(root.join("FEATURES.md")).expect("FEATURES.md at the repository root");
    let problems = check_matrix(&text, &root);
    assert!(problems.is_empty(), "FEATURES.md has problems:\n  {}", problems.join("\n  "));
}

#[test]
fn the_checker_catches_the_mistakes_it_exists_for() {
    let root = repo_root();
    let good = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
    let broken = |from: &str, to: &str| {
        assert!(good.contains(from), "fixture text `{from}` not in FEATURES.md");
        check_matrix(&good.replacen(from, to, 1), &root)
    };
    let has = |p: &[String], needle: &str| p.iter().any(|x| x.contains(needle));
    // a cited test that does not exist
    let p = broken("iyakki-edit::buffer::tests::loads_and_saves_text_faithfully", "iyakki-edit::buffer::tests::no_such_test_anywhere");
    assert!(has(&p, "no_such_test_anywhere"), "{p:?}");
    // a missing script and a bad crate
    assert!(has(&broken("script:dist/buildroot/qemu-test.sh", "script:dist/nope.sh"), "does not exist"));
    assert!(has(&broken("iyakki-sec::tests::scoped_read", "iyakki-nope::tests::scoped_read"), "does not name a crate"));
    // duplicate IDs
    assert!(has(&broken("| F-CORE-02 |", "| F-CORE-01 |"), "duplicate ID"));
    assert!(has(&broken("| F-CORE-02 |", "| F-core-2 |"), "bad ID"));
    // status and progress must agree
    assert!(has(&broken("| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100% |", "| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 60% |"), "`done` must be 100%"));
    assert!(has(&broken("| planned | P3 | 0% |", "| planned | P3 | 30% |"), "`planned` must be 0%"));
    assert!(has(&broken("| partial | P2 | 50% |", "| partial | P2 | 100% |"), "`partial` must be 1%..99%"));
    assert!(has(&broken("| done | P0 | 100% |", "| shipped | P0 | 100% |"), "unknown status"));
    assert!(has(&broken("| P1 | done | P0 |", "| P11 | done | P0 |"), "unknown phase"));
    // done without evidence
    let p = check_matrix(&good.replacen("| F-CORE-04 | Shutdown signals (own `sigaction`, no crate) | P0 | done | P1 | 100% | `iyakki-core::tests::sigterm_sets_the_shutdown_flag_and_the_flag_is_shared` |", "| F-CORE-04 | Shutdown signals (own `sigaction`, no crate) | P0 | done | P1 | 100% | — |", 1), &root);
    assert!(has(&p, "F-CORE-04: `done` needs at least one test"), "{p:?}");
    // deprecation must say what replaces it
    assert!(has(&broken("Replaced by F-EDIT-05 and F-EDIT-06;", "Old."), "must name its replacement"));
    assert!(has(&broken("Replaced by F-EDIT-05 and F-EDIT-06;", "Replaced by F-NOPE-99;"), "must name its replacement"));
    // stale totals and malformed rows
    assert!(has(&broken("total ", "total 1"), "totals line is stale") || has(&broken("done ", "done 9"), "totals line is stale"));
    assert!(has(&check_matrix(&format!("{good}\n| F-X-01 | too | few |"), &root), "expected 8 columns"));
    assert!(check_matrix("no table here", &root).iter().any(|x| x.contains("no feature rows")));
}
