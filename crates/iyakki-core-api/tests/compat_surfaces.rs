//! Charter step 1 (docs/development/CORE-COMPATIBILITY-DESIGN.md sections 8 and 10): every versioned surface in the code has a row in
//! `docs/development/compat-surfaces.txt`, and the table can only be extended on purpose.
//!
//! What it finds, in the non-test code of every crate under `crates/` (everything from the first `#[cfg(test)] mod ..` on is skipped, `tests.rs`
//! and `*_tests.rs` are skipped, comment lines are skipped):
//!
//! * `spec`     an `iyakki_schema::Spec { name: "..", current: N, .. }` (a JSON store with a schema number),
//! * `const`    a `const` with a name word `VERSION`, `VERSIONS`, `SCHEMA`, `MAGIC` or `ABI` (protocol constants, snapshot and manifest versions, file
//!              magics, the capability catalogue version).
//!
//! Numbers that sit as literals inside code (the vault's `v: 1`, for example) cannot be found by a scan: they are `manual` rows, and the
//! test checks that the file still contains the text written in the `value` column.
//!
//! Table format (tab separated, `#` starts a comment): `kind  id  file  value  class  note`. `class` is `surface` (a promise to other
//! releases; the note starts with the charter surface letter A to G) or `not-ours` (a number that looks like a version but is a third
//! party's, a mask, a limit). The test fails when
//!   * the code has a versioned thing that the table does not list (add a row: say which charter surface it is and where its fixture is),
//!   * a row names a thing that is gone (delete the row and say why in the commit: a removed surface is a compatibility event),
//!   * a `value` differs from the code (a bump: the release needs a fixture folder, see tests/compat/README.md; then update the row).
//!
//! To print the current table: `IYAKKI_COMPAT_PRINT=1 cargo test -p iyakki-core-api --test compat_surfaces -- --nocapture`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TABLE: &str = include_str!("../../../docs/development/compat-surfaces.txt");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if matches!(n.as_str(), "testdata" | "tests" | "target" | "examples" | "benches") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if !(name.ends_with("_tests.rs") || name == "tests.rs") {
                out.push(p);
            }
        }
    }
}

/// The non-test code: up to the first `#[cfg(test)]` that starts a test module (`mod tests;` or `mod tests {`).
fn code_of(text: &str) -> String {
    let mut cut = text.len();
    let mut from = 0;
    while let Some(p) = text[from..].find("#[cfg(test)]") {
        let at = from + p;
        let after = text[at + "#[cfg(test)]".len()..].trim_start();
        if after.starts_with("mod ") || after.starts_with("pub mod ") {
            cut = at;
            break;
        }
        from = at + 1;
    }
    text[..cut].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    kind: &'static str,
    id: String,
    file: String,
    value: String,
}

fn upper_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn scan() -> Vec<Found> {
    let mut files = Vec::new();
    rust_files(&root().join("crates"), &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let code = code_of(&text);
        let rel = f.strip_prefix(root()).unwrap().to_string_lossy().into_owned();
        // Spec literals
        let mut rest = code.as_str();
        while let Some(i) = rest.find("iyakki_schema::Spec { name: \"") {
            let after = &rest[i + "iyakki_schema::Spec { name: \"".len()..];
            if let Some(q) = after.find('"') {
                let name = &after[..q];
                let cur = after[q..].split("current:").nth(1).map(|s| s.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>()).unwrap_or_default();
                out.push(Found { kind: "spec", id: name.to_string(), file: rel.clone(), value: cur });
            }
            rest = &rest[i + 1..];
        }
        // named constants
        for line in code.lines() {
            let t = line.trim_start();
            let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
            let Some(t) = t.strip_prefix("const ") else { continue };
            let Some((name, tail)) = t.split_once(':') else { continue };
            let name = name.trim();
            if !upper_name(name) || !name.split('_').any(|w| ["VERSION", "VERSIONS", "SCHEMA", "MAGIC", "ABI"].contains(&w)) {
                continue;
            }
            let value = tail.split_once('=').map(|(_, v)| v.trim().trim_end_matches(';').trim().to_string()).unwrap_or_default();
            // a `Spec` constant is reported once, as a `spec` row
            if value.starts_with("iyakki_schema::Spec") {
                continue;
            }
            out.push(Found { kind: "const", id: name.to_string(), file: rel.clone(), value });
        }
    }
    out.sort_by(|a, b| (a.kind, &a.id, &a.file).cmp(&(b.kind, &b.id, &b.file)));
    out
}

#[derive(Debug, Clone)]
struct Row {
    kind: String,
    id: String,
    file: String,
    value: String,
    class: String,
    note: String,
}

fn rows() -> Vec<Row> {
    TABLE
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            assert!(c.len() >= 5, "compat-surfaces.txt: a row needs kind, id, file, value, class (and a note): {l:?}");
            Row { kind: c[0].into(), id: c[1].into(), file: c[2].into(), value: c[3].into(), class: c[4].into(), note: c.get(5).copied().unwrap_or("").into() }
        })
        .collect()
}

#[test]
fn every_versioned_surface_in_the_code_has_a_row_in_the_table_and_every_row_still_exists() {
    let found = scan();
    if std::env::var("IYAKKI_COMPAT_PRINT").is_ok() {
        for f in &found {
            println!("{}\t{}\t{}\t{}\tsurface\t", f.kind, f.id, f.file, f.value);
        }
    }
    let table = rows();
    let mut have: BTreeMap<(String, String, String), &Row> = BTreeMap::new();
    for r in &table {
        assert!(["spec", "const", "manual"].contains(&r.kind.as_str()), "unknown kind {:?} in the row for {}", r.kind, r.id);
        assert!(r.class == "surface" || r.class == "not-ours", "class of {} must be surface or not-ours, not {:?}", r.id, r.class);
        if r.kind != "manual" {
            assert!(have.insert((r.kind.clone(), r.id.clone(), r.file.clone()), r).is_none(), "duplicate row {} {} {}", r.kind, r.id, r.file);
        }
    }
    let mut problems = Vec::new();
    for f in &found {
        match have.get(&(f.kind.to_string(), f.id.clone(), f.file.clone())) {
            None => problems.push(format!("NEW versioned surface, no row in docs/development/compat-surfaces.txt: {} {} ({}) = {}", f.kind, f.id, f.file, f.value)),
            Some(r) if r.value != f.value => problems.push(format!("CHANGED: {} {} ({}) is now {} but the table says {}: a bump needs a fixture folder (tests/compat/README.md), then update the row", f.kind, f.id, f.file, f.value, r.value)),
            _ => {}
        }
    }
    for (k, id, file) in have.keys() {
        if !found.iter().any(|f| f.kind == k && &f.id == id && &f.file == file) {
            problems.push(format!("GONE: {k} {id} ({file}) is in the table but not in the code: a removed surface is a compatibility event; delete the row and say why in the commit"));
        }
    }
    assert!(problems.is_empty(), "\n  {}\n", problems.join("\n  "));
}

#[test]
fn manual_rows_still_find_their_text_and_every_surface_row_names_its_charter_letter() {
    for r in rows() {
        if r.kind == "manual" {
            let text = std::fs::read_to_string(root().join(&r.file)).unwrap_or_else(|_| panic!("manual row {}: {} does not exist", r.id, r.file));
            assert!(text.contains(&r.value), "manual row {}: {} no longer contains {:?} (the version moved: update the row, add a fixture)", r.id, r.file, r.value);
        }
        if r.class == "surface" {
            let letter = r.note.split_whitespace().next().unwrap_or("");
            assert!(matches!(letter, "A" | "B" | "C" | "D" | "E" | "F" | "G"), "row {} {}: the note starts with the charter surface letter (A to G), not {letter:?}", r.kind, r.id);
        }
    }
}

#[test]
fn the_capability_catalogue_version_is_in_the_table_with_the_value_of_the_code() {
    let r = rows().into_iter().find(|r| r.id == "CATALOGUE_VERSION").expect("CATALOGUE_VERSION row");
    assert_eq!(r.value, iyakki_core_api::appcaps::CATALOGUE_VERSION.to_string());
}
