//! Enforces BOUNDARY.md: system-level calls and process spawning happen only in the files listed there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
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

const SYSCALLS: [&str; 24] = ["swapon", "swapoff", "mount", "umount", "umount2", "reboot", "sethostname", "ioctl", "sigaction", "getrandom", "statvfs", "sync", "waitpid", "kill", "killpg", "setsid", "setpgid", "prctl", "raise", "poll", "mmap", "munmap", "mknod", "clock_settime"];

/// What a file does at the base-OS boundary: process spawns and system-level libc calls (test code excluded).
fn touchpoints(text: &str) -> Vec<String> {
    let code = text.split("#[cfg(test)]").next().unwrap_or(text);
    let mut found = BTreeSet::new();
    if code.contains("Command::new(") {
        found.insert("spawn".to_string());
    }
    let mut rest = code;
    while let Some(i) = rest.find("libc::") {
        let after = &rest[i + 6..];
        let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if SYSCALLS.contains(&name.as_str()) {
            found.insert(format!("libc::{name}"));
        }
        rest = &after[name.len().max(1)..];
    }
    found.into_iter().collect()
}

/// Files named in the tables of BOUNDARY.md (`| \`crates/...\` | ...`).
fn listed_files(doc: &str) -> BTreeSet<String> {
    doc.lines().filter_map(|l| l.strip_prefix("| `crates/")).filter_map(|r| r.split('`').next()).map(|p| format!("crates/{p}")).collect()
}

/// Source files of the main workspace, as repository-relative paths with their touchpoints.
fn scan(root: &Path) -> Vec<(String, Vec<String>)> {
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        if krate.file_name() == "iyakki-gui" {
            continue; // separate workspace: desktop development window, not part of the OS image
        }
        rust_files(&krate.path().join("src"), &mut files);
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let rel = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let t = touchpoints(&std::fs::read_to_string(&f).unwrap());
            Some((rel, t))
        })
        .collect()
}

fn problems(doc: &str, scanned: &[(String, Vec<String>)]) -> Vec<String> {
    let listed = listed_files(doc);
    let mut out = Vec::new();
    for (file, t) in scanned {
        if !t.is_empty() && !listed.contains(file) {
            out.push(format!("{file} touches the base OS ({}) but is not listed in BOUNDARY.md", t.join(", ")));
        }
    }
    for f in &listed {
        match scanned.iter().find(|(p, _)| p == f) {
            None => out.push(format!("BOUNDARY.md lists {f}, which does not exist")),
            Some((_, t)) if t.is_empty() => out.push(format!("BOUNDARY.md lists {f}, but it no longer touches the base OS: remove the row")),
            _ => {}
        }
    }
    out
}

#[test]
fn only_the_listed_files_touch_the_base_os() {
    let root = root();
    let doc = std::fs::read_to_string(root.join("BOUNDARY.md")).expect("BOUNDARY.md at the repository root");
    let p = problems(&doc, &scan(&root));
    assert!(p.is_empty(), "boundary violations:\n  {}\n(see BOUNDARY.md rule 5)", p.join("\n  "));
}

#[test]
fn upstream_sources_are_never_patched() {
    fn walk(dir: &Path, bad: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, bad);
            } else if p.extension().is_some_and(|x| x == "patch" || x == "diff") {
                bad.push(p.display().to_string());
            }
        }
    }
    let mut found = Vec::new();
    walk(&root().join("dist"), &mut found);
    // patching comes last and never silently: a patch file is allowed only when BOUNDARY.md names it (rule 3)
    let doc = std::fs::read_to_string(root().join("BOUNDARY.md")).unwrap();
    let bad: Vec<&String> = found.iter().filter(|p| !Path::new(p).file_name().is_some_and(|n| doc.contains(&*n.to_string_lossy()))).collect();
    assert!(bad.is_empty(), "patch files under dist/ that BOUNDARY.md rule 3 does not list: {bad:?}");
    // the Mac variant's four kernel patches (0001 IYAKKI's CDCLK fix, 0002-0004 pinned T2 MacBook backports) and the three
    // patches of its out-of-tree apple-bce package (now four: 0004 silences the speaker on shutdown), and the one Secure Boot lockdown patch of every
    // x86-64 image (patches-sb), and the snd-hda-macbookpro diff
    assert!(found.len() <= 10, "more than the ten listed patches: {found:?}");
}

#[test]
fn the_checker_itself_catches_violations() {
    assert_eq!(touchpoints("fn a() { Command::new(\"x\"); }\n#[cfg(test)]\nmod t { fn b() { libc::kill(1, 9); } }"), vec!["spawn"], "test code is ignored");
    assert_eq!(touchpoints("fn a() { unsafe { libc::mount(a, b, c, 0, d); libc::c_int; libc::sync(); } }"), vec!["libc::mount", "libc::sync"], "types are not system calls");
    assert!(touchpoints("fn plain() {}").is_empty());
    let doc = "| File |\n| `crates/iyakki-core/src/boot.rs` | x |\n| `crates/iyakki-gone/src/lib.rs` | y |\n";
    let scanned = vec![("crates/iyakki-core/src/boot.rs".to_string(), vec!["spawn".to_string()]), ("crates/iyakki-new/src/lib.rs".to_string(), vec!["libc::kill".to_string()]), ("crates/iyakki-other/src/lib.rs".to_string(), vec![])];
    let p = problems(doc, &scanned);
    assert!(p.iter().any(|x| x.contains("iyakki-new") && x.contains("not listed")), "{p:?}");
    assert!(p.iter().any(|x| x.contains("iyakki-gone") && x.contains("does not exist")), "{p:?}");
    assert_eq!(p.len(), 2);
    let stale = problems("| `crates/iyakki-other/src/lib.rs` | z |", &scanned);
    assert!(stale.iter().any(|x| x.contains("no longer touches")), "{stale:?}");
}
