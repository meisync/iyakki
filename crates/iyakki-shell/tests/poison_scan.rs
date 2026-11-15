//! A panic in a thread an app started, while that thread holds a lock, poisons the lock. Every later `lock().unwrap()` then panics
//! too, so one bug would crash the app again and again (and everything else that shares the lock). The code in app threads and the
//! services they share therefore takes locks the poison-tolerant way: `lock().unwrap_or_else(std::sync::PoisonError::into_inner)`
//! (the data is whatever the panicking thread left; the next user sees it and goes on). This test fails when a new `lock().unwrap()`,
//! `read().unwrap()`, `write().unwrap()` or the same with `expect(..)` appears in the code of the shell, the agent, the core and the
//! toolkit (before each file's `#[cfg(test)]`; files that only hold tests are skipped). A line that must keep it says why with
//! `// poison-ok: <why>` on the same line.

use std::path::{Path, PathBuf};

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

/// Where `code` takes a lock and unwraps it (the call may be split over lines).
fn offenders_in(code: &str) -> Vec<usize> {
    let squeezed: Vec<(usize, char)> = code.char_indices().collect();
    let _ = squeezed;
    let mut found = Vec::new();
    for kind in ["lock", "read", "write"] {
        let needle = format!(".{kind}()");
        let mut from = 0;
        while let Some(k) = code[from..].find(&needle) {
            let at = from + k;
            from = at + needle.len();
            let after = code[from..].trim_start();
            if after.starts_with(".unwrap()") || after.starts_with(".expect(") {
                let line = code[..at].matches('\n').count() + 1;
                let text = code.lines().nth(line - 1).unwrap_or("");
                if !text.contains("// poison-ok:") {
                    found.push(line);
                }
            }
        }
    }
    found.sort_unstable();
    found
}

/// Names of the modules declared as `#[cfg(test)] mod name;` anywhere under `files`: their files (or folders) hold test code only.
fn test_only_modules(files: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();
    for p in files {
        let s = std::fs::read_to_string(p).unwrap_or_default();
        let lines: Vec<&str> = s.lines().collect();
        for w in lines.windows(2) {
            if w[0].trim() == "#[cfg(test)]" {
                let decl = w[1].trim().trim_start_matches("pub(crate) ").trim_start_matches("pub ");
                if let Some(n) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) {
                    names.push(n.trim().to_string());
                }
            }
        }
    }
    names
}

fn offenders(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rs_files(root, &mut files);
    let test_mods = test_only_modules(&files);
    let mut bad = Vec::new();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with("_tests.rs") || name == "tests.rs" || name.starts_with("tests_") {
            continue;
        }
        // a file or a folder that a `#[cfg(test)] mod x;` declares holds test code only
        let stem = name.trim_end_matches(".rs");
        if test_mods.iter().any(|m| m == stem) || p.components().any(|c| test_mods.iter().any(|m| c.as_os_str() == m.as_str())) {
            continue;
        }
        let s = std::fs::read_to_string(&p).unwrap();
        let code = s.find("\n#[cfg(test)]").map_or(&s[..], |k| &s[..k]);
        for line in offenders_in(code) {
            bad.push(format!("{}:{line}", p.strip_prefix(root.join("..")).unwrap_or(&p).display()));
        }
    }
    bad
}

#[test]
fn locks_in_app_code_survive_a_panic_of_another_thread() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut bad = Vec::new();
    for krate in ["iyakki-shell", "iyakki-agent", "iyakki-core", "iyakki-uikit"] {
        bad.extend(offenders(&here.join("..").join(krate).join("src")));
    }
    assert!(bad.is_empty(), "take the lock with .unwrap_or_else(std::sync::PoisonError::into_inner) (or say `// poison-ok: why`): {bad:?}");
}

#[test]
fn the_scanner_finds_split_calls_and_respects_the_marker() {
    assert_eq!(offenders_in("let a = m.lock().unwrap();\nlet b = n\n    .lock()\n    .unwrap();\n"), vec![1, 3]);
    assert_eq!(offenders_in("x.read().expect(\"r\");\ny.write().unwrap_or_else(std::sync::PoisonError::into_inner);\n"), vec![1]);
    assert!(offenders_in("let a = m.lock().unwrap(); // poison-ok: a test double\n").is_empty());
    assert!(offenders_in("f.read(&mut b).unwrap();\nlet n = s.read().unwrap_or_default();\n").is_empty());
}

#[test]
fn a_poisoned_lock_taken_the_tolerant_way_still_gives_its_data() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(vec![1, 2, 3]));
    let m2 = m.clone();
    let _ = std::thread::spawn(move || {
        let mut g = m2.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.push(4);
        panic!("a bug in an app thread while the lock is held");
    })
    .join();
    assert!(m.is_poisoned());
    assert_eq!(*m.lock().unwrap_or_else(std::sync::PoisonError::into_inner), vec![1, 2, 3, 4]);
}
