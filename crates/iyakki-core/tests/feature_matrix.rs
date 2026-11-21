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

/// The crates of the boot loader's own workspace (the `iyakki-boot` git submodule). A checkout without submodules (the public CI, a plain
/// clone) has the folder empty or missing: then a citation of these crates and of `iyakki-boot/...` scripts cannot be checked, which is
/// said loudly on stderr (`SKIPPED`) and never counted as a pass; where the submodule IS present every such citation is checked in full.
const BOOT_LOADER_CRATES: [&str; 4] = ["boot-core", "boot-uefi", "test-efi", "uefi-sys"];

fn boot_loader_checked_out(root: &Path) -> bool {
    root.join("iyakki-boot/crates").is_dir()
}

/// The public tree (dist/release/export-public.sh writes the marker) leaves out the internal process documents listed in
/// dist/release/public-exclude.txt, so a row may cite one of those and the file is missing there by design. In the private tree there is
/// no marker and the file must exist.
fn left_out_of_the_public_tree(root: &Path, path: &str) -> bool {
    if !root.join("dist/release/PUBLIC-EXPORT").is_file() {
        return false;
    }
    let Ok(list) = std::fs::read_to_string(root.join("dist/release/public-exclude.txt")) else { return false };
    list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).any(|pat| glob_matches(pat, path))
}

/// `*` matches any run of characters (enough for the exclusion list).
fn glob_matches(pat: &str, text: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return pat == text;
    }
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = text.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match text[at..end].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    true
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
    impl_pct: i64,
    test_pct: i64,
    manual: String,
    tests: Vec<String>,
    notes: String,
}

fn manual_ok(m: &str) -> bool {
    let date_ok = |d: &str| d.len() == 10 && d.char_indices().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    match m.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["none"] | ["needed"] => true,
        ["passed", d, w @ ..] => date_ok(d) && !w.is_empty(),
        ["failed", ki] => ki.strip_prefix("KI-").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())),
        _ => false,
    }
}

/// `done N · partial N (impl open N · unproven N) · planned N · blocked N · deprecated N · total N`.
fn totals_line(rows: &[Row]) -> String {
    let n = |s: &str| rows.iter().filter(|r| r.status == s).count();
    let partial = || rows.iter().filter(|r| r.status == "partial");
    let impl_open = partial().filter(|r| r.impl_pct < 100).count();
    let unproven = partial().filter(|r| r.impl_pct == 100 && (r.test_pct < 100 || r.manual == "needed" || r.manual.starts_with("failed"))).count();
    format!("done {} · partial {} (impl open {impl_open} · unproven {unproven}) · planned {} · blocked {} · deprecated {} · total {}", n("done"), n("partial"), n("planned"), n("blocked"), n("deprecated"), rows.len())
}

/// The ids listed in `partial-ceiling-raises.txt` (one `F-ID reason` per line; `#` lines are comments).
fn raise_ids(text: &str) -> Vec<String> {
    text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).filter_map(|l| l.split_whitespace().next()).map(str::to_string).collect()
}

/// The number of `partial` rows may not go above the ceiling file. The file may be lowered freely. It may be raised only by the number of
/// ids newly listed (with a reason) in `docs/development/partial-ceiling-raises.txt`, compared with git HEAD: a new feature row that starts
/// out partial, or a `done` row honestly demoted because a change needs a new manual test. Nothing else may raise it.
fn ceiling_problems(partial: usize, root: &Path) -> Vec<String> {
    let read = |t: &str| t.trim().parse::<usize>().ok();
    let Some(ceiling) = std::fs::read_to_string(root.join("docs/development/partial-ceiling.txt")).ok().and_then(|t| read(&t)) else {
        return vec!["docs/development/partial-ceiling.txt is missing or not a number".into()];
    };
    let mut p = Vec::new();
    if partial > ceiling {
        p.push(format!("{partial} partial rows is above the ceiling {ceiling} in docs/development/partial-ceiling.txt (finish rows, or list a new or demoted row with its reason in partial-ceiling-raises.txt)"));
    }
    let git_head = |path: &str| std::process::Command::new("git").args(["show", &format!("HEAD:{path}")]).current_dir(root).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string());
    if let Some(old) = git_head("docs/development/partial-ceiling.txt").and_then(|t| read(&t)) {
        if ceiling > old {
            let now = std::fs::read_to_string(root.join("docs/development/partial-ceiling-raises.txt")).map(|t| raise_ids(&t)).unwrap_or_default();
            let before = git_head("docs/development/partial-ceiling-raises.txt").map(|t| raise_ids(&t)).unwrap_or_default();
            let added = now.iter().filter(|i| !before.contains(i)).count();
            if ceiling - old > added {
                p.push(format!("the ceiling was raised from {old} to {ceiling} ({} more) but only {added} new id(s) are listed in docs/development/partial-ceiling-raises.txt", ceiling - old));
            }
        }
    }
    p
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
        if cells.len() != 9 {
            problems.push(format!("{at}: expected 9 columns, found {}", cells.len()));
            continue;
        }
        let (id, name, phase, status, pri, prog, manual, tests, notes) = (cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7], cells[8]);
        let id_ok = id.strip_prefix("F-").and_then(|r| r.rsplit_once('-')).is_some_and(|(area, num)| !area.is_empty() && area.starts_with(|c: char| c.is_ascii_uppercase()) && area.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && (2..=3).contains(&num.len()) && num.chars().all(|c| c.is_ascii_digit()));
        if !id_ok {
            problems.push(format!("{at}: bad ID `{id}` (want F-AREA-nn or F-AREA-nnn)"));
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
        let pair = prog.split_once('/').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?)));
        let Some((impl_pct, test_pct)) = pair.filter(|(a, b)| (0..=100).contains(a) && (0..=100).contains(b)) else {
            problems.push(format!("{id}: Impl/Test `{prog}` must be `<0..100>/<0..100>`"));
            continue;
        };
        if ![0, 30, 50, 70, 100].contains(&test_pct) {
            problems.push(format!("{id}: Test {test_pct} must be one of 0, 30, 50, 70, 100"));
        }
        if !manual_ok(manual) {
            problems.push(format!("{id}: Manual `{manual}` must be none, needed, `passed YYYY-MM-DD where` or `failed KI-nnn`"));
        }
        let tests: Vec<String> = if tests == "—" { vec![] } else { tests.split("<br>").map(|t| t.trim().trim_matches('`').to_string()).collect() };
        rows.push(Row { id: id.to_string(), status: status.to_string(), impl_pct, test_pct, manual: manual.to_string(), tests, notes: notes.to_string() });
    }
    if rows.is_empty() {
        problems.push("no feature rows found".into());
    }
    let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    for r in &rows {
        let open_manual = r.manual == "needed" || r.manual.starts_with("failed");
        let (i, t) = (r.impl_pct, r.test_pct);
        match r.status.as_str() {
            "done" if i != 100 || t != 100 || open_manual => problems.push(format!("{}: `done` needs Impl 100, Test 100 and Manual none or passed (is {i}/{t}, Manual {})", r.id, r.manual)),
            "planned" if i != 0 || t != 0 => problems.push(format!("{}: `planned` must be 0/0 (is {i}/{t})", r.id)),
            "partial" if i == 100 && t == 100 && !open_manual => problems.push(format!("{}: `partial` with 100/100 and no open manual test is `done`", r.id)),
            "blocked" if i == 100 && t == 100 => problems.push(format!("{}: `blocked` cannot be 100/100", r.id)),
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
                    if path.starts_with("iyakki-boot/") && !boot_loader_checked_out(root) {
                        eprintln!("SKIPPED (the iyakki-boot submodule is not checked out, this citation was NOT checked): {} cites script `{path}`", r.id);
                    } else if left_out_of_the_public_tree(root, path) {
                        eprintln!("SKIPPED (the public tree leaves this internal document out by design, this citation was NOT checked): {} cites `{path}`", r.id);
                    } else {
                        problems.push(format!("{}: script `{path}` does not exist", r.id));
                    }
                }
                continue;
            }
            let parts: Vec<&str> = t.split("::").collect();
            let (Some(krate), Some(name)) = (parts.first(), parts.last()) else { continue };
            if parts.len() >= 2 && BOOT_LOADER_CRATES.contains(krate) && !boot_loader_checked_out(root) {
                eprintln!("SKIPPED (the iyakki-boot submodule is not checked out, this citation was NOT checked): {} cites test `{t}`", r.id);
            } else if parts.len() < 2 || crate_dir(root, krate).is_none() {
                problems.push(format!("{}: `{t}` does not name a crate and a test", r.id));
            } else if !test_exists(root, krate, name) {
                problems.push(format!("{}: test `{t}` not found in the crate {krate}", r.id));
            }
        }
    }
    // the totals line
    let expected = totals_line(&rows);
    let found = text.split("<!-- summary:start -->").nth(1).and_then(|r| r.split("<!-- summary:end -->").next()).unwrap_or("").to_string();
    if found != expected {
        problems.push(format!("the totals line is stale: it says `{found}`, the rows say `{expected}`"));
    }
    problems.extend(ceiling_problems(rows.iter().filter(|r| r.status == "partial").count(), root));
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
    // status, Impl/Test and Manual must agree
    assert!(has(&broken("| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100/100 |", "| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100/70 |"), "`done` needs Impl 100"));
    assert!(has(&broken("| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100/100 | none |", "| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100/100 | needed |"), "`done` needs Impl 100"));
    assert!(has(&broken("| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | done | P0 | 100/100 | none |", "| F-CORE-01 | Supervisor: run sandboxed tasks, revoke tokens afterwards | P1 | partial | P0 | 100/100 | none |"), "is `done`"));
    assert!(has(&broken("| planned | P3 | 0/0 |", "| planned | P3 | 30/0 |"), "`planned` must be 0/0"));
    assert!(has(&broken("100/100 | none |", "100/100 | maybe |"), "Manual `maybe`"));
    assert!(has(&broken("100/100 | none |", "100/55 | none |"), "Test 55"));
    assert!(has(&broken("100/100 | none |", "100% | none |"), "Impl/Test `100%`"));
    assert!(has(&broken("| done | P0 | 100/100 |", "| shipped | P0 | 100/100 |"), "unknown status"));
    assert!(has(&broken("| P1 | done | P0 |", "| P11 | done | P0 |"), "unknown phase"));
    // the partial ceiling
    assert!(ceiling_problems(usize::MAX, &root).iter().any(|x| x.contains("above the ceiling")));
    assert!(ceiling_problems(0, &root).iter().all(|x| !x.contains("above")));
    assert_eq!(raise_ids("# comment\nF-CHAT-02 new feature row\n\nF-NET-06 demoted: new Wi-Fi message needs a manual re-check\n"), vec!["F-CHAT-02", "F-NET-06"]);
    assert!(manual_ok("passed 2026-11-08 macmini") && manual_ok("failed KI-077") && !manual_ok("passed macmini") && !manual_ok("failed 77"));
    // done without evidence
    let p = check_matrix(&good.replacen("| F-CORE-04 | Shutdown signals (own `sigaction`, no crate) | P0 | done | P1 | 100/100 | none | `iyakki-core::tests::sigterm_sets_the_shutdown_flag_and_the_flag_is_shared` |", "| F-CORE-04 | Shutdown signals (own `sigaction`, no crate) | P0 | done | P1 | 100/100 | none | — |", 1), &root);
    assert!(has(&p, "F-CORE-04: `done` needs at least one test"), "{p:?}");
    // deprecation must say what replaces it
    assert!(has(&broken("Replaced by F-WEB-03 (pixel-streamed desktop)", "Old."), "must name its replacement"));
    assert!(has(&broken("Replaced by F-WEB-03 (pixel-streamed desktop)", "Replaced by F-NOPE-99"), "must name its replacement"));
    // stale totals and malformed rows
    assert!(has(&broken("total ", "total 1"), "totals line is stale") || has(&broken("done ", "done 9"), "totals line is stale"));
    assert!(has(&check_matrix(&format!("{good}\n| F-X-01 | too | few |"), &root), "expected 9 columns"));
    assert!(check_matrix("no table here", &root).iter().any(|x| x.contains("no feature rows")));
}

#[test]
fn tests_of_the_boot_loader_workspace_can_be_cited_by_crate_name() {
    let root = repo_root();
    if !boot_loader_checked_out(&root) {
        // not a silent pass: the submodule is the only place these names can be checked; the CI job checks it out (`submodules: true`)
        eprintln!("SKIPPED: the iyakki-boot submodule is not checked out, so citing the boot loader's tests by crate name was NOT checked here");
        assert!(crate_dir(&root, "no-such-crate").is_none());
        return;
    }
    assert!(test_exists(&root, "boot-core", "crash_loop_rolls_back_after_n_failures"));
    assert!(test_exists(&root, "boot-core", "hold_key_wins_over_oneshot"));
    assert!(!test_exists(&root, "boot-core", "no_such_boot_test"));
    assert!(crate_dir(&root, "uefi-sys").is_some() && crate_dir(&root, "no-such-crate").is_none());
}

#[test]
fn glob_matching_follows_the_exclusion_list_and_the_marker_gates_it() {
    assert!(glob_matches("docs/project/BETA-RETRIAGE*.md", "docs/project/BETA-RETRIAGE.md"));
    assert!(glob_matches("docs/project/BETA-RETRIAGE*.md", "docs/project/BETA-RETRIAGE-2.md"));
    assert!(!glob_matches("docs/project/BETA-RETRIAGE*.md", "docs/project/BETA-CHECKLIST.md"));
    assert!(glob_matches("ALPHA-GATE.md", "ALPHA-GATE.md") && !glob_matches("ALPHA-GATE.md", "ALPHA-GATE.md.x"));
    assert!(glob_matches("docs/development/WAVE*-*.md", "docs/development/WAVE3-A.md") && !glob_matches("docs/development/WAVE*-*.md", "docs/development/WAVE3.md"));
    // without the marker (the private tree) nothing is ever left out
    if !repo_root().join("dist/release/PUBLIC-EXPORT").is_file() {
        assert!(!left_out_of_the_public_tree(&repo_root(), "ALPHA-GATE.md"));
    }
}

#[test]
fn the_function_index_matches_the_text_search_it_replaced() {
    let mut names = BTreeSet::new();
    fn_names_in("pub fn alpha(x: u8) {}\n    async fn beta_2() {}\nfn gamma (x: u8) {}\n// fn in_a_comment(\nlet f = |fn_x| fn_x; fn \nfn delta<T>(t: T) {}", &mut names);
    // `gamma (` has a space before the parenthesis and `delta<` a generic list: neither contains the text `fn name(`, as before
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["alpha", "beta_2", "in_a_comment"]);
}
