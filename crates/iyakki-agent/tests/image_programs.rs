//! Guard against the "works with the fake runner, fails on the image" bug (the missing `curl`, 2026-10-27).
//!
//! Scans production code (no test files, no `#[cfg(test)]` items) of iyakki-agent, iyakki-shell and iyakki-core for programs that are
//! started: `Command::new("x")`, a Runner call whose argv starts with a literal, `let argv/cmd = vec!["x".into(), ..]`. Every such program
//! must be listed in `dist/buildroot/image-programs.txt` (shipped by the image, optional, host-only or a known gap). A second scan fails on
//! a literal "curl", "jq", "python3" or "perl" anywhere in production code outside the files that file exempts (command-policy lists).
//! Limits: a program named through a constant or a variable is not seen.

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
    if line.contains(".run(") || line.contains(".run_in(") {
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
    for f in sources() {
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in production(&text) {
            for p in spawned(&line) {
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
