//! One reader per kernel fact (owner rule 2026-11-06: tiles and services reach `/proc`, `/sys` and `/dev` through IYAKKI's shared
//! modules, they do not keep copies of the parsing). The same kernel file read in many places drifts apart and breaks differently on
//! different hardware.
//!
//! What it checks: for each kernel path in [`PATHS`], which source files (non-test code of every crate under `crates/`, everything
//! from the first `#[cfg(test)]` on skipped, `*_tests.rs` and `tests.rs` skipped, comment lines skipped) hold that literal. The
//! pairs (path, file) that exist today are in `data/kernel_readers.allow`. The list may only shrink: a new pair fails (read the fact
//! through the owner module instead, or add the owner module's file with a one-line reason in the allow file's header), and a pair
//! that disappeared fails until it is removed from the list, so progress is locked in.
//!
//! To print the current table: `IYAKKI_KERNEL_READERS_PRINT=1 cargo test -p iyakki-core --test kernel_single_reader -- --nocapture`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const ALLOW: &str = include_str!("data/kernel_readers.allow");

/// Kernel files and directories that carry a fact somebody parses. Harmless sources of bytes (`/dev/null`, `/dev/urandom`, `/dev/zero`)
/// and the process's own `/proc/self` are not listed.
const PATHS: [&str; 34] = [
    "/proc/meminfo", "/proc/mounts", "/proc/uptime", "/proc/cpuinfo", "/proc/loadavg", "/proc/swaps", "/proc/cmdline", "/proc/asound",
    "/proc/net", "/proc/partitions", "/proc/sys", "/sys/class/net", "/sys/class/drm", "/sys/class/power_supply", "/sys/class/backlight",
    "/sys/class/thermal", "/sys/class/input", "/sys/class/block", "/sys/block", "/sys/bus/usb", "/sys/bus/hid", "/sys/devices",
    "/sys/firmware", "/sys/module", "/sys/kernel", "/sys/class/bluetooth", "/sys/class/ieee80211", "/sys/class/sound", "/sys/class/hwmon",
    "/dev/input", "/dev/dri", "/dev/snd", "/dev/fb0", "/dev/tty",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if n == "testdata" || n == "tests" || n == "target" || n == "examples" || n == "benches" {
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

/// The non-test code of a file: up to the first `#[cfg(test)]` line, comment lines dropped.
fn code_of(text: &str) -> String {
    let cut = text.find("\n#[cfg(test)]").or_else(|| text.find("\n    #[cfg(test)]")).unwrap_or(text.len());
    text[..cut].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn current() -> BTreeSet<(String, String)> {
    let mut files = Vec::new();
    rust_files(&root().join("crates"), &mut files);
    let mut out = BTreeSet::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let code = code_of(&text);
        let rel = f.strip_prefix(root()).unwrap().to_string_lossy().into_owned();
        for p in PATHS {
            if code.contains(&format!("\"{p}")) {
                out.insert((p.to_string(), rel.clone()));
            }
        }
    }
    out
}

fn allowed() -> BTreeSet<(String, String)> {
    ALLOW
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.trim().to_string())))
        .collect()
}

#[test]
fn each_kernel_path_is_read_in_no_more_places_than_before() {
    let now = current();
    if std::env::var("IYAKKI_KERNEL_READERS_PRINT").is_ok() {
        for (p, f) in &now {
            println!("{p}\t{f}");
        }
    }
    let was = allowed();
    let new: Vec<String> = now.difference(&was).map(|(p, f)| format!("{f} now reads {p}")).collect();
    let gone: Vec<String> = was.difference(&now).map(|(p, f)| format!("{f} no longer reads {p}")).collect();
    assert!(
        new.is_empty(),
        "new copies of kernel access (read the fact through its owner module instead, see feedback in docs/development/UI-OS-SPLIT-PLAN.md):\n  {}",
        new.join("\n  ")
    );
    assert!(gone.is_empty(), "progress to lock in: remove these lines from crates/iyakki-core/tests/data/kernel_readers.allow:\n  {}", gone.join("\n  "));
}

#[test]
fn the_list_of_paths_has_no_duplicates_and_every_allowed_pair_names_a_listed_path() {
    let set: BTreeSet<&str> = PATHS.iter().copied().collect();
    assert_eq!(set.len(), PATHS.len());
    for (p, f) in allowed() {
        assert!(set.contains(p.as_str()), "{p} ({f}) is not a listed kernel path");
    }
}
