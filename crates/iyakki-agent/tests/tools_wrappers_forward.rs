//! Every set of tools that wraps another (a struct with an `Arc<dyn Tools>` field and an `impl Tools`) must implement `inner_tools`, so that
//! what the tools underneath decide (an approval card, a refusal, a grant rule, open work, notes, running jobs) is never hidden by a wrapper
//! that forgot one of the trait's methods (the screen-capture approval was lost that way once). The behaviour of the forwarding is tested
//! in `tools_forward_tests.rs`; this reads the sources, so a new wrapper that forgets is caught the day it is written.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "testdata")) {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The text of `impl ... Tools for <name> { ... }` (up to the closing brace at the start of a line), for each such impl in the file.
fn tools_impls(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find(" Tools for ") {
        let start = at + i;
        let after = &text[start + " Tools for ".len()..];
        let name: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
        let header = &text[line_start..start];
        let body_start = text[start..].find('{').map_or(text.len(), |b| start + b);
        let end = text[body_start..].find("\n}\n").map_or(text.len(), |e| body_start + e);
        if header.trim_start().starts_with("impl") && !name.is_empty() {
            out.push((name, text[body_start..end].to_string()));
        }
        at = start + 1;
    }
    out
}

fn wraps_tools(text: &str, name: &str) -> bool {
    let Some(i) = text.find(&format!("struct {name}")) else { return false };
    let rest = &text[i..];
    let Some(open) = rest.find('{') else { return false };
    let semi = rest.find(';').unwrap_or(usize::MAX);
    if semi < open {
        return false; // a unit or tuple struct
    }
    let close = rest[open..].find("\n}").map_or(rest.len(), |c| open + c);
    let body = &rest[open..close];
    body.contains("Arc<dyn Tools") || body.contains("Arc<dyn crate::agent::Tools") || body.contains("Arc<dyn agent::Tools") || body.contains("Arc<dyn iyakki_agent::agent::Tools") || body.contains("Box<dyn Tools")
}

#[test]
fn every_set_of_tools_that_wraps_another_forwards_what_it_does_not_decide() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut wrappers = 0;
    let mut missing = Vec::new();
    for f in files {
        if !f.to_string_lossy().contains("/src/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        if !text.contains(" Tools for ") {
            continue;
        }
        for (name, body) in tools_impls(&text) {
            if wraps_tools(&text, &name) {
                wrappers += 1;
                if !body.contains("fn inner_tools") {
                    missing.push(format!("{}: {name}", f.strip_prefix(&root).unwrap_or(&f).display()));
                }
            }
        }
    }
    assert!(wrappers > 40, "the scan should find the wrappers of the chat chain, found {wrappers}");
    assert!(missing.is_empty(), "these wrap other tools and do not implement `inner_tools` (add `fn inner_tools(&self) -> Option<&dyn Tools> {{ Some(&*self.inner) }}`):\n  {}", missing.join("\n  "));
}

#[test]
fn the_scan_itself_sees_a_wrapper_that_forgets_and_one_that_does_not() {
    let forgetful = "pub struct W {\n    inner: Arc<dyn Tools>,\n}\n\nimpl Tools for W {\n    fn defs(&self) -> Vec<ToolDef> {\n        vec![]\n    }\n}\n";
    let good = "pub struct G {\n    inner: Arc<dyn Tools>,\n}\n\nimpl Tools for G {\n    fn inner_tools(&self) -> Option<&dyn Tools> {\n        Some(&*self.inner)\n    }\n}\n";
    let plain = "pub struct P;\n\nimpl Tools for P {\n    fn defs(&self) -> Vec<ToolDef> {\n        vec![]\n    }\n}\n";
    let found = |t: &str| tools_impls(t).into_iter().filter(|(n, _)| wraps_tools(t, n)).map(|(_, b)| b.contains("fn inner_tools")).collect::<Vec<_>>();
    assert_eq!(found(forgetful), [false]);
    assert_eq!(found(good), [true]);
    assert!(found(plain).is_empty(), "a tool set that wraps nothing needs no inner_tools");
}
