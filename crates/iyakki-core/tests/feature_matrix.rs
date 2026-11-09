//! Keeps FEATURES.md honest. A row may not cite a test that does not exist, claim `done` without
//! evidence, or contradict its own progress; the totals line must match the rows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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

/// Where the source of a named crate is: `crates/<krate>`, else the boot loader's own workspace `iyakki-boot/crates/<krate>` (a standalone
/// project with its own `cargo test --manifest-path iyakki-boot/Cargo.toml`, so its tests are cited as `boot-core::module::tests::name`).
fn crate_dir(root: &Path, krate: &str) -> Option<PathBuf> {
    [root.join("crates").join(krate), root.join("iyakki-boot").join("crates").join(krate)].into_iter().find(|d| d.is_dir())
}

/// The names `n` for which a source text contains `fn n(`: every `fn ` followed by identifier characters and `(` (the same text match as
/// searching for the string `fn <name>(`, done once per file instead of once per cited test).
fn fn_names_in(text: &str, out: &mut BTreeSet<String>) {
    for (at, _) in text.match_indices("fn ") {
        let rest = &text[at + 3..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        if end > 0 && rest[end..].starts_with('(') {
            out.insert(rest[..end].to_string());
        }
    }
}

/// The function names of one crate (`src` and `tests`), read once per process and shared by every check in it. This used to concatenate
/// the crate's whole source into a string for each `check_matrix` call and search it once per cited test: about ten minutes for the agent.
fn crate_fn_names(root: &Path, krate: &str) -> Arc<BTreeSet<String>> {
    static INDEX: OnceLock<Mutex<BTreeMap<String, Arc<BTreeSet<String>>>>> = OnceLock::new();
    let mut index = INDEX.get_or_init(Default::default).lock().unwrap();
    index
        .entry(format!("{}::{krate}", root.display()))
        .or_insert_with(|| {
            let mut files = Vec::new();
            for sub in ["src", "tests"] {
                if let Some(d) = crate_dir(root, krate) {
                    rust_files(&d.join(sub), &mut files);
                }
            }
            let mut names = BTreeSet::new();
            for f in &files {
                if let Ok(text) = std::fs::read_to_string(f) {
                    fn_names_in(&text, &mut names);
                }
            }
            Arc::new(names)
        })
        .clone()
}

/// Does the crate `<krate>` define `fn <name>(`?
fn test_exists(root: &Path, krate: &str, name: &str) -> bool {
    crate_fn_names(root, krate).contains(name)
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
            if parts.len() < 2 || crate_dir(root, krate).is_none() {
                problems.push(format!("{}: `{t}` does not name a crate and a test", r.id));
            } else if !test_exists(root, krate, name) {
                problems.push(format!("{}: test `{t}` not found in the crate {krate}", r.id));
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

#[test]
fn tests_of_the_boot_loader_workspace_can_be_cited_by_crate_name() {
    let root = repo_root();
    assert!(test_exists(&root, "boot-core", "crash_loop_rolls_back_after_n_failures"));
    assert!(test_exists(&root, "boot-core", "hold_key_wins_over_oneshot"));
    assert!(!test_exists(&root, "boot-core", "no_such_boot_test"));
    assert!(crate_dir(&root, "uefi-sys").is_some() && crate_dir(&root, "no-such-crate").is_none());
}

#[test]
fn the_function_index_matches_the_text_search_it_replaced() {
    let mut names = BTreeSet::new();
    fn_names_in("pub fn alpha(x: u8) {}\n    async fn beta_2() {}\nfn gamma (x: u8) {}\n// fn in_a_comment(\nlet f = |fn_x| fn_x; fn \nfn delta<T>(t: T) {}", &mut names);
    // `gamma (` has a space before the parenthesis and `delta<` a generic list: neither contains the text `fn name(`, as before
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["alpha", "beta_2", "in_a_comment"]);
}
