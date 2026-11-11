//! Guard against the "works with the fake runner, fails on the image" bug (the missing `curl`, 2026-10-27).
//!
//! Scans production code (no test files, no `#[cfg(test)]` items) of iyakki-agent, iyakki-shell and iyakki-core for programs that are
//! started: `Command::new("x")`, a Runner call whose argv starts with a literal, `let argv/cmd = vec!["x".into(), ..]`. Every such program
//! must be listed in `dist/buildroot/image-programs.txt` (shipped by the image, optional, host-only or a known gap). A second scan fails on
//! a literal "curl", "jq", "python3" or "perl" anywhere in production code outside the files that file exempts (command-policy lists).
//! A program named through a `const NAME: &str = "prog";` is resolved and checked like a literal (`Command::new(NAME)`, `[NAME.into(), ..]`);
//! one named through a variable or a value computed at run time is not seen (the list of those is printed by `print_the_known_gaps`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name != "tests" && !name.contains("test") {
                rs_files(&p, out);
            }
        } else if name.ends_with(".rs") && !name.contains("test") {
            out.push(p);
        }
    }
}

/// The file's production lines: comments and every `#[cfg(test)]` item (a module or a function up to its closing brace) removed.
fn production(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut skipping = false;
    for (i, line) in text.lines().enumerate() {
        if skipping {
            if line == "}" || line == "};" || (line.trim_end().ends_with(';') && !line.starts_with(' ')) {
                skipping = false;
            }
            continue;
        }
        if line.trim() == "#[cfg(test)]" && !line.starts_with(' ') {
            skipping = true;
            continue;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        out.push((i + 1, line.to_string()));
    }
    out
}

/// Plain scanning without a regex crate: the literal after `marker`, when it starts with a lowercase letter or `/`.
fn literal_after<'a>(line: &'a str, marker: &str) -> Vec<&'a str> {
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find(marker) {
        let s = from + i + marker.len();
        from = s;
        let rest = &line[s..];
        if let Some(end) = rest.find('"') {
            let lit = &rest[..end];
            if lit.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '/') && lit.chars().all(|c| c.is_ascii_alphanumeric() || "_./+-".contains(c)) {
                v.push(lit);
            }
        }
    }
    v
}

/// The programs a production line starts, found by the three patterns.
fn spawned(line: &str) -> Vec<String> {
    let mut v: Vec<String> = literal_after(line, "Command::new(\"").iter().map(|s| s.to_string()).collect();
    let owned = |l: &str| l.contains("\".into()") || l.contains("\".to_string()") || l.contains("\".to_owned()");
    // `runner.run(&prog, &["close".into(), ..], None)` passes the program as a variable and the arguments as a literal slice: the literals are
    // arguments, not a program. Only a call whose FIRST argument is the literal slice starts the program named by its first literal.
    let slice_is_argv = match line.find(".run(") {
        Some(i) => {
            let first = line[i + ".run(".len()..].trim_start();
            first.starts_with("&[\"") || first.starts_with("[\"")
        }
        None => true,
    };
    if (line.contains(".run(") || line.contains(".run_in(")) && slice_is_argv {
        for m in ["&[\"", "[\""] {
            for lit in literal_after(line, m) {
                let after = &line[line.find(&format!("{m}{lit}\"")).unwrap_or(0)..];
                if after.contains(&format!("{lit}\".into()")) || after.contains(&format!("{lit}\".to_string()")) {
                    v.push(lit.to_string());
                }
            }
        }
    }
    let t = line.trim_start();
    if (t.starts_with("let mut argv") || t.starts_with("let argv") || t.starts_with("let mut cmd") || t.starts_with("let cmd")) && owned(line) {
        v.extend(literal_after(line, "vec![\"").iter().take(1).map(|s| s.to_string()));
    }
    v
}


/// `const NAME: &str = "lit";` (also `static`, `pub`, `pub(crate)`) in production code: name -> every value it has anywhere.
fn constants(files: &[PathBuf]) -> BTreeMap<String, BTreeSet<String>> {
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        for (_, line) in production(&text) {
            let t = line.trim_start();
            let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub(super) ")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
            let Some(rest) = t.strip_prefix("const ").or_else(|| t.strip_prefix("static ")) else { continue };
            let Some((name, tail)) = rest.split_once(": &str = \"") else { continue };
            let Some((lit, _)) = tail.split_once('"') else { continue };
            if name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                m.entry(name.to_string()).or_default().insert(lit.to_string());
            }
        }
    }
    m
}

fn is_program_name(lit: &str) -> bool {
    lit.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '/') && lit.chars().all(|c| c.is_ascii_alphanumeric() || "_./+-".contains(c))
}

/// The constant names a production line starts as programs: `Command::new(NAME)`, `Command::new(&NAME)`, and `[NAME.into()` on a
/// runner line or a `let argv/cmd = vec![NAME.into()`.
fn spawned_constants(line: &str) -> Vec<String> {
    let ident = |rest: &str| -> Option<String> {
        let rest = rest.trim_start().trim_start_matches('&');
        let id: String = rest.chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
        let after = &rest[id.len()..];
        (id.len() >= 2 && id.starts_with(|c: char| c.is_ascii_uppercase())).then_some(id).filter(|_| after.starts_with(')') || after.starts_with(".into()") || after.starts_with(".to_string()") || after.starts_with(".to_owned()") || after.starts_with(','))
    };
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find("Command::new(") {
        from += i + "Command::new(".len();
        v.extend(ident(&line[from..]));
    }
    let t = line.trim_start();
    let argv_line = t.starts_with("let mut argv") || t.starts_with("let argv") || t.starts_with("let mut cmd") || t.starts_with("let cmd");
    if line.contains(".run(") || line.contains(".run_in(") || argv_line {
        for m in ["&[", "[", "vec!["] {
            let mut from = 0;
            while let Some(i) = line[from..].find(m) {
                from += i + m.len();
                if let Some(id) = ident(&line[from..]) {
                    if !line[from..].starts_with(&format!("{id},")) || argv_line || m == "&[" {
                        v.push(id);
                    }
                }
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

struct Listing {
    kinds: BTreeMap<String, (String, String)>,
    exempt: Vec<String>,
}

fn listing() -> Listing {
    let text = std::fs::read_to_string(root().join("dist/buildroot/image-programs.txt")).expect("dist/buildroot/image-programs.txt");
    let mut l = Listing { kinds: BTreeMap::new(), exempt: Vec::new() };
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let mut w = line.splitn(3, ' ');
        let (kind, name, rest) = (w.next().unwrap(), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        assert!(!rest.is_empty(), "image-programs.txt: '{line}' needs a source or a reason");
        match kind {
            "exempt" => l.exempt.push(name.to_string()),
            "image" | "optional" | "host" | "gap" => {
                l.kinds.insert(name.to_string(), (kind.to_string(), rest.to_string()));
            }
            _ => panic!("image-programs.txt: unknown kind in '{line}'"),
        }
    }
    l
}

fn sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for c in ["iyakki-agent", "iyakki-shell", "iyakki-core"] {
        rs_files(&root().join("crates").join(c).join("src"), &mut files);
    }
    files
}

#[test]
fn every_program_production_code_starts_is_listed_for_the_image() {
    let l = listing();
    let mut unknown: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let files = sources();
    let consts = constants(&files);
    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in production(&text) {
            for p in spawned(&line).into_iter().chain(spawned_constants(&line).into_iter().flat_map(|c| match consts.get(&c) {
                Some(vals) => vals.iter().filter(|v| is_program_name(v)).cloned().collect::<Vec<_>>(),
                None => {
                    unresolved.insert(c);
                    Vec::new()
                }
            })) {
                used.insert(p.clone());
                if !l.kinds.contains_key(&p) {
                    unknown.entry(p).or_default().insert(format!("{}:{n}", f.strip_prefix(root()).unwrap().display()));
                }
            }
        }
    }
    assert!(unknown.is_empty(), "production code starts programs that dist/buildroot/image-programs.txt does not list (does the image ship them? add `image`, `optional`, `host` or `gap` with a reason): {unknown:#?}");
    // the scan must keep seeing the programs it knows, or it has gone blind
    for want in ["git", "modprobe", "aplay", "kubectl"] {
        assert!(used.contains(want), "the scan no longer finds `{want}`: the patterns need updating");
    }
    if !unresolved.is_empty() {
        println!("constants used as programs that the scan cannot resolve (not a `const X: &str` in these crates): {unresolved:?}");
    }
}

#[test]
fn the_scan_resolves_programs_named_through_constants() {
    // a program named by a constant is seen, so an unlisted one fails the first test like a literal would
    assert_eq!(spawned_constants("let o = Command::new(BIN).arg(\"x\");"), ["BIN"]);
    assert_eq!(spawned_constants("Command::new(&TOOL)"), ["TOOL"]);
    assert_eq!(spawned_constants("let r = runner.run(&[PROG.into(), \"-v\".into()]);"), ["PROG"]);
    assert!(spawned("let out = self.sys.run(&prog, &[\"close\".into(), name], None)?;").is_empty(), "a slice that is not the first argument holds arguments");
    assert!(spawned("let r = runner.run(&[\"ip\".into(), \"a\".into()]);").contains(&"ip".to_string()), "a call whose first argument is the literal slice still counts");
    assert_eq!(spawned_constants("let argv = vec![HELPER.to_string(), \"a\".into()];"), ["HELPER"]);
    assert!(spawned_constants("let x = Command::new(path);").is_empty(), "a variable is not a constant");
    assert!(spawned_constants("let n = LIMITS[0];").is_empty());
    let dir = std::env::temp_dir().join(format!("iyakki-imgprog-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.rs");
    std::fs::write(&f, "const BIN: &str = \"bluetoothctl\";\npub(crate) static OTHER: &str = \"/usr/bin/foo\";\nconst N: usize = 3;\n#[cfg(test)]\nconst T: &str = \"hidden\";\n").unwrap();
    let c = constants(&[f]);
    assert_eq!(c.get("BIN").unwrap().iter().next().unwrap(), "bluetoothctl");
    assert!(c.contains_key("OTHER") && !c.contains_key("N"));
    assert!(is_program_name("bluetoothctl") && is_program_name("/usr/bin/foo") && !is_program_name("Hello world") && !is_program_name("--flag"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn image_entries_name_a_package_that_the_buildroot_configs_enable() {
    let l = listing();
    let mut configs = String::new();
    for e in std::fs::read_dir(root().join("dist/buildroot/configs")).unwrap().flatten() {
        configs.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
    }
    for (name, (kind, src)) in &l.kinds {
        if kind == "image" && src != "busybox" {
            assert!(configs.contains(&format!("{src}=y")), "{name}: {src}=y is in no dist/buildroot/configs file");
        }
    }
    assert!(!l.kinds.contains_key("curl"), "curl is not in the image; do not list it");
}

#[test]
fn no_production_code_outside_the_policy_lists_names_curl_jq_python_or_perl() {
    let l = listing();
    let mut found = Vec::new();
    for f in sources() {
        let rel = f.strip_prefix(root().join("crates")).unwrap().to_string_lossy().to_string();
        if l.exempt.iter().any(|e| rel.ends_with(e.as_str())) {
            continue;
        }
        for (n, line) in production(&std::fs::read_to_string(&f).unwrap()) {
            for bad in ["\"curl\"", "\"jq\"", "\"python3\"", "\"perl\""] {
                if line.contains(bad) {
                    found.push(format!("{rel}:{n} {bad}"));
                }
            }
        }
    }
    assert!(found.is_empty(), "the image ships none of curl, jq, python3, perl: use the in-house client or code, or list the file as `exempt` with a reason: {found:#?}");
}

#[test]
fn print_the_known_gaps() {
    for (name, (kind, why)) in &listing().kinds {
        if kind == "gap" {
            println!("image gap: {name}: {why}");
        }
    }
}
