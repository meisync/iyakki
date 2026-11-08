//! `f32::clamp` / `Ord::clamp` panic when the bounds are inverted (a real device lost the AI IDE to
//! "min > max, or either was NaN. min = 1908.4784, max = 1908.4783"). Every `.clamp(` whose bounds are not both literal
//! constants must be `.clamp_safe(` (see `src/safeclamp.rs`). This test fails when a new one appears.

use std::path::{Path, PathBuf};

/// Our own `clamp` methods (not `Ord::clamp`): `Plan::clamp(x, y)` moves a pointer into the outputs and cannot panic.
const OWN_METHODS: [&str; 1] = ["backend_kms.rs"];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn is_literal(a: &str) -> bool {
    let a = a.trim();
    let a = a.strip_prefix('-').unwrap_or(a);
    let numeric = a.starts_with(|c: char| c.is_ascii_digit()) && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    let constant = a.starts_with(|c: char| c.is_ascii_uppercase()) && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == ':');
    let prim = ["f32", "f64", "u32", "i32", "usize", "u8", "u16", "u64", "i64"].iter().any(|t| a.starts_with(&format!("{t}::")) && a[t.len() + 2..].chars().all(|c| c.is_ascii_uppercase() || c == '_'));
    numeric || constant || prim
}

/// The arguments of the call whose first character after `(` is at `i`.
fn args(s: &str, i: usize) -> Option<Vec<String>> {
    let (mut depth, mut cur, mut out) = (0i32, String::new(), Vec::new());
    for c in s[i..].chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => {
                out.push(cur);
                return (out.len() == 2).then_some(out);
            }
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    None
}

/// Non-literal clamps in the code before the first `#[cfg(test)]` of each source file. Files that only hold tests, and the
/// helper itself, are skipped.
fn offenders(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rs_files(root, &mut files);
    let mut bad = Vec::new();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with("_tests.rs") || name == "safeclamp.rs" || OWN_METHODS.contains(&name.as_str()) {
            continue;
        }
        let s = std::fs::read_to_string(&p).unwrap();
        let code = s.find("\n#[cfg(test)]").map_or(&s[..], |k| &s[..k]);
        let mut from = 0;
        while let Some(k) = code[from..].find(".clamp(") {
            let at = from + k;
            from = at + 7;
            // not a two-argument call: one of our own methods (`self.sel.clamp(&doc)`, `self.clamp()`)
            let Some(a) = args(code, at + 7) else { continue };
            if a.iter().all(|x| is_literal(x)) {
                continue;
            }
            let line = code[..at].matches('\n').count() + 1;
            bad.push(format!("{}:{line}", p.strip_prefix(root).unwrap().display()));
        }
    }
    bad
}

#[test]
fn no_clamp_with_computed_bounds_outside_the_safe_helper() {
    // the shell and the toolkit crate it re-exports from (gfx, ui, textsel ... moved to crates/iyakki-uikit)
    let mut bad = offenders(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    bad.extend(offenders(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-uikit/src")));
    assert!(bad.is_empty(), "use .clamp_safe(..) (crate::safeclamp::SafeClamp) for computed bounds; found .clamp( at: {bad:?}");
}

#[test]
fn the_scanner_itself_tells_literal_from_computed_bounds() {
    for a in ["0.0", " 1.0", "-5", "255", "1.0_f32", "MIN_W", "f32::MAX", "f32::INFINITY", "0u8"] {
        assert!(is_literal(a), "{a}");
    }
    for a in ["a.y", "w - 4.0", "x", "(h).max(1.0)", "self.max", "bounds.x + 4.0 * s"] {
        assert!(!is_literal(a), "{a}");
    }
    assert_eq!(args("a, b)", 0).unwrap().len(), 2);
    assert!(args("a, f(b, c), d)", 0).is_none());
}
