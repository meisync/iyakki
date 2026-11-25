//! Dependency-rule ratchet (UI/OS split plan, step 1; docs/development/UI-OS-SPLIT-PLAN.md).
//!
//! Rule: tiles in `crates/iyakki-apps/src/apps/` do not reach the kernel or the hardware crates themselves; they ask IYAKKI core
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
    rust_files(&r.join("crates/iyakki-apps/src/apps"), &mut files);
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
//    the agent. (The older headless editor crate `iyakki-ui` was removed on 2026-11-13; `iyakki-gui` uses `iyakki-edit` directly.)
// 2. A tile crate (`crates/iyakki-tile-*`) depends on `iyakki-uikit` and `iyakki-tile-api` and on no other `iyakki-*` crate, except the
//    lines in `data/tile_crate_deps.allow` (which may only shrink). The tile-ABI crates and the facade are not tile crates; they are
//    listed in `TILE_INFRA`.
// 3. The facade `iyakki-tile-api` depends on `iyakki-uikit` only (never the agent).

const TILE_ALLOW: &str = include_str!("data/tile_crate_deps.allow");
const UI_CRATE: &str = "iyakki-uikit";
const API_CRATE: &str = "iyakki-tile-api";
/// Crates under `iyakki-tile-*` that are the WASM tile interface and its host, or WASM tiles built on the ABI (the example and Runs), not native toolkit tiles. Not covered by rule 2.
const TILE_INFRA: [&str; 9] = ["iyakki-tile-abi", "iyakki-tile-host", "iyakki-tile-example", "iyakki-tile-runs", "iyakki-tile-kit", "iyakki-tile-settings", "iyakki-tile-edit", "iyakki-tile-chat", API_CRATE];

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
    assert!(bad.is_empty(), "WHAT: a tile crate depends on an IYAKKI crate it may not.\nWHERE: the pairs (crate, dependency) are in {bad:?}.\nFIX: remove the line from that crate's Cargo.toml [dependencies] and reach the machine through the core API / a port instead; an SDK app depends on `iyakki-ui-sdk` only. Before: `iyakki-agent = {{ path = \"../iyakki-agent\" }}`. After: no such line, and the call goes through a port. A tile may depend only on {UI_CRATE} and {API_CRATE} (or a documented exception in tests/data/tile_crate_deps.allow, which only the owner adds).");
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
    // PILOT 4 adds four (below). PILOT 3: iyakki-camdetect also takes iyakki-clock (events.rs); the others are std plus crates the agent already had
    let allowed: [(&str, &[&str]); 16] = [
        ("iyakki-sshfs", &["iyakki-sec"]),
        ("iyakki-camdetect", &["iyakki-sec", "iyakki-volcrypt", "iyakki-clock"]),
        ("iyakki-diskcrypt", &[]),
        ("iyakki-clock", &[]),
        ("iyakki-wordtime", &[]),
        ("iyakki-qr", &[]),
        ("iyakki-archive", &[]),
        ("iyakki-bindelta", &[]),
        ("iyakki-lint", &[]),
        ("iyakki-battery", &[]),
        // PILOT 4: clockfloor takes the clock; macrecovery the download client and the board-id check; regex and openpgp are std/sha2 only
        ("iyakki-clockfloor", &["iyakki-clock"]),
        ("iyakki-macrecovery", &["iyakki-proxy", "iyakki-drivers"]),
        ("iyakki-regex", &[]),
        ("iyakki-openpgp", &[]),
        // F-BROW-79: the filter matcher split out of iyakki-vpn (serde only)
        ("iyakki-filter", &[]),
        // F-INBOX-09, F-BROX-03, F-MSG-08: protocol cryptography (ring, sha2, aes, dalek only)
        ("iyakki-webcrypto", &[]),
    ];
    for (krate, ok) in allowed {
        let deps = iyakki_deps(&manifest(krate));
        let extra: Vec<&String> = deps.iter().filter(|d| !ok.contains(&d.as_str())).collect();
        assert!(extra.is_empty(), "{krate} may depend on {ok:?} only (found {deps:?}); never on iyakki-agent, iyakki-shell or a tile crate");
    }
}

/// The owner's rule (docs/development/CORE-ACCESS-MODEL.md 6f): a tile gets the machine only as the guarded handle in `cx.os`; it does not
/// make a raw one. Files under `apps/` may name `iyakki_core_local` (the raw implementation) only as listed here, and the numbers may only
/// shrink. The shell (`shell.rs`, `shell_osguard.rs`) builds the raw handle once and hands out `GuardedOs`.
const RAW_OS_ALLOWED: [(&str, usize); 2] = [("crates/iyakki-apps/src/apps/browser_speed.rs", 1), ("crates/iyakki-apps/src/apps/devices.rs", 2)];

#[test]
fn apps_do_not_name_the_raw_os_crate_and_the_shell_hands_out_only_the_guarded_one() {
    let r = root();
    let mut files = Vec::new();
    rust_files(&r.join("crates/iyakki-apps/src/apps"), &mut files);
    let mut found = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let n = non_test_code(&text).lines().filter(|l| !l.trim_start().starts_with("//") && l.contains("iyakki_core_local")).count();
        if n > 0 {
            found.insert(f.strip_prefix(&r).unwrap().to_string_lossy().into_owned(), n);
        }
    }
    let allow: BTreeMap<String, usize> = RAW_OS_ALLOWED.iter().map(|(p, n)| (p.to_string(), *n)).collect();
    assert_eq!(found, allow, "an app names the raw Os crate (new, or the list can shrink): ask `cx.os` instead");
    // the shell builds the raw handle in one place only, and `cx()` hands the app the guarded one
    let shell = std::fs::read_to_string(r.join("crates/iyakki-shell/src/shell.rs")).unwrap();
    assert!(!non_test_code(&shell).contains("LocalOs"), "shell.rs must not build a raw LocalOs: only shell_osguard.rs does");
    assert!(non_test_code(&shell).contains("self.osguard.os_for("), "AppCx.os must be made by the guard");
    let guard = std::fs::read_to_string(r.join("crates/iyakki-shell/src/shell_osguard.rs")).unwrap();
    assert_eq!(non_test_code(&guard).matches("LocalOs::system()").count(), 1);
}

// ---- migrated tiles stay clean (procedure section 10) ----------------------------------------------------------------------------

const MIGRATED: &str = include_str!("data/migrated_tiles.list");
/// Handles of the shell context that a migrated tile may not name. `Services` is a type name, so it is matched as a whole word that is not a
/// string literal and not a variant of a tile's own enum (see [`names_the_services_type`]); the others are specific enough as text.
const FORBIDDEN: [&str; 9] = ["Services", ".services", "cx.host", "cx.web", "cx.chat", "cx.store", "cx.session", "cx.audit", "iyakki_agent::services"];

/// The line with the text of its string and character literals blanked out, so a tab label such as "Services" is not read as the type.
fn without_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let (mut in_str, mut esc) = (false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push('"');
            }
            continue;
        }
        if c == '\'' && chars.peek() == Some(&'"') {
            // the character literal '"' is not the start of a string
            chars.next();
            chars.next();
            out.push_str("' '");
        } else if c == '"' {
            in_str = true;
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out
}

/// `Services` the shell's handle type: the whole word, outside literals, standing where a type or a path to it stands (after `:`, `<`, `&`,
/// `->`, `dyn`, `impl`, `as`, `use`, or followed by `::`, or at the end of a path whose head is a module such as `iyakki_agent::Services`). A
/// variant of a tile's own enum (`enum Tab { Services }`, `Tab::Services`) is not the handle.
fn names_the_services_type(line: &str) -> bool {
    let code = without_literals(line);
    let b = code.as_bytes();
    let mut from = 0;
    while let Some(i) = code[from..].find("Services") {
        let at = from + i;
        let end = at + "Services".len();
        from = end;
        let word_before = at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_');
        let word_after = end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_');
        if word_before || word_after {
            continue;
        }
        let before = code[..at].trim_end();
        if let Some(path) = before.strip_suffix("::") {
            // `Head::Services`: the handle only when the head is a module (lower case), not a type of the tile's own
            let head: String = path.chars().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
            if head.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
                return true;
            }
            continue;
        }
        let word = before.rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().unwrap_or("");
        let type_position = before.ends_with(':') || before.ends_with('<') || before.ends_with('&') || before.ends_with("->") || matches!(word, "dyn" | "impl" | "as" | "use" | "mut");
        if type_position || code[end..].starts_with("::") {
            return true;
        }
    }
    false
}

fn migrated() -> Vec<(String, usize)> {
    MIGRATED
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (p, n) = l.rsplit_once(' ').unwrap_or_else(|| panic!("migrated_tiles.list: expected `path count`: {l}"));
            (p.trim().to_string(), n.trim().parse().unwrap_or_else(|_| panic!("migrated_tiles.list: bad count: {l}")))
        })
        .collect()
}

/// What a migrated tile's non-test code names that it may not, and how often it names the shell context.
fn tile_context_use(text: &str) -> (Vec<String>, usize) {
    let code = non_test_code(text);
    let mut bad = Vec::new();
    let mut appcx = 0;
    for (i, l) in code.lines().enumerate() {
        if l.trim_start().starts_with("//") {
            continue;
        }
        for f in FORBIDDEN {
            let hit = if f == "Services" { names_the_services_type(l) } else { l.contains(f) };
            if hit {
                bad.push(format!("line {}: `{f}` in `{}`", i + 1, l.trim().chars().take(80).collect::<String>()));
            }
        }
        appcx += l.matches("AppCx").count();
    }
    (bad, appcx)
}

#[test]
fn migrated_tiles_never_name_the_services_or_the_shell_context_and_the_list_is_real() {
    let list = migrated();
    assert!(!list.is_empty());
    for (path, most) in &list {
        let text = std::fs::read_to_string(root().join(path)).unwrap_or_else(|_| panic!("migrated_tiles.list names {path}, which does not exist"));
        let (bad, appcx) = tile_context_use(&text);
        assert!(bad.is_empty(), "{path} is a migrated tile and may not reach the services or the shell context directly (ask its port):\n  {}", bad.join("\n  "));
        assert!(appcx <= *most, "{path} names AppCx {appcx} times, {most} allowed: a migrated tile takes the shell context only at the App trait's entry points");
        assert!(appcx >= *most, "{path} names AppCx {appcx} times, {most} listed: lower the number so it cannot grow back");
    }
}

#[test]
fn the_migrated_tile_checker_sees_what_it_is_for() {
    let (bad, n) = tile_context_use("use super::{App, AppCx};\nfn a(cx: &AppCx) { cx.services.radio.stop(); }\n// cx.services in a comment\nfn b(p: &dyn Port) { p.stop(); }\n#[cfg(test)]\nmod t { fn c(cx: &AppCx) { cx.services.x(); } }");
    assert_eq!((bad.len(), n), (1, 2), "{bad:?}");
    assert!(bad[0].contains("cx.services") || bad[0].contains(".services"));
    assert_eq!(tile_context_use("fn a(s: Arc<Services>) {}").0.len(), 1);
    assert_eq!(tile_context_use("fn a(p: &dyn RadioPort) { p.audit(\"x\", \"y\"); }").0.len(), 0);
}

#[test]
fn the_word_services_is_the_type_only_as_a_whole_word_outside_literals_and_own_enum_paths() {
    // the shell's handle: forbidden in every spelling a tile could name it
    for bad in ["fn a(s: Arc<Services>) {}", "let s: &Services = x;", "use iyakki_agent::Services;", "fn a(s: Services) {}", "x.push(Services::new());"] {
        assert!(names_the_services_type(bad), "{bad}");
    }
    // a tab, a label, a variant of the tile's own enum and longer names are not the handle (MESH, NET SCAN)
    for fine in ["enum Tab { Services, Nodes }", "(Tab::Services, \"Services\"),", "let t = Tab::Services;", "let l = \"the Services tab\";", "struct MyServices;", "fn a(s: ServicesList) {}", "let n = Section::Services;"] {
        assert!(!names_the_services_type(fine), "{fine}");
    }
    // a quote character literal does not hide the type that follows it
    assert!(names_the_services_type("let c = '\"'; let s: Services = x;"));
    // an escaped quote does not end the literal early
    assert!(!names_the_services_type("let l = \"say \\\"Services\\\" now\";"));
    // the checker as a whole: a label and a variant pass, the handle is still found
    assert_eq!(tile_context_use("const TABS: [(Tab, &str); 1] = [(Tab::Services, \"Services\")];").0.len(), 0);
    assert_eq!(tile_context_use("fn a(s: Arc<Services>) { let _ = Tab::Services; }").0.len(), 1);
}
