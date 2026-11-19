//! Per-test temporary folders (test builds only).
//!
//! Tests run in one process, in parallel, and `./run.sh test` points `TMPDIR` at `target/tmp` inside the build tree, which on a
//! developer machine is usually below `$HOME` and below a long path. Two things followed from that and made tests fail for reasons
//! that were not bugs: (1) tests that need a folder OUTSIDE the person's home found it inside, and (2) Unix socket paths (at most
//! 108 bytes, the code allows 100) came out too long. This module gives every call its own folder (process id plus a counter, so
//! two tests never share or delete each other's folder) and offers the two special cases. Folders left behind by a process that no
//! longer exists are removed on the next call.

#![allow(dead_code)] // a copy of the agent's helper: only `unique` is used here

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The base for ordinary test folders: `TMPDIR` (the build tree under `./run.sh test`), else the system default.
pub fn base() -> PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir())
}

fn alive(pid: u32) -> bool {
    if Path::new("/proc/self").exists() {
        return Path::new(&format!("/proc/{pid}")).exists();
    }
    // no /proc (macOS): cannot tell, so a folder of another process is never swept (it may belong to a test run still going; without
    // this guard a second test process on the same machine deleted the folders of the first)
    true
}

/// Removes `<prefix>-<pid>-...` folders in `dir` whose process is gone.
fn sweep(dir: &Path, prefix: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(prefix).and_then(|r| r.strip_prefix('-')) else { continue };
        if let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) {
            if pid != std::process::id() && !alive(pid) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

fn make(dir: &Path, prefix: &str, tag: &str) -> PathBuf {
    sweep(dir, prefix);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let d = dir.join(format!("{prefix}-{}-{n}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fresh folder of its own under [`base`].
pub fn unique(prefix: &str, tag: &str) -> PathBuf {
    make(&base(), prefix, tag)
}

/// A fresh folder that is NOT below the real `$HOME` (for tests of "outside your home folder"). Tries the test base, then `/tmp`,
/// `/var/tmp` and `/dev/shm`.
pub fn outside_home(prefix: &str, tag: &str) -> PathBuf {
    let home = std::env::var("HOME").ok().and_then(|h| std::fs::canonicalize(h).ok());
    let under_home = |p: &Path| home.as_ref().is_some_and(|h| p.starts_with(h));
    for c in [base(), PathBuf::from("/tmp"), PathBuf::from("/var/tmp"), PathBuf::from("/dev/shm")] {
        if c.is_dir() && !under_home(&c) {
            return make(&c, prefix, tag);
        }
    }
    panic!("no writable folder outside $HOME for the test");
}

/// A fresh folder whose path is short enough for Unix sockets inside it (`budget` bytes of the folder itself at most 40, so that
/// `<folder>/<a few levels>/agent.sock` stays below the 100 bytes the code allows). Uses the test base when it is short, else `/tmp`.
pub fn short(prefix: &str, tag: &str) -> PathBuf {
    let b = base();
    let root = if b.as_os_str().len() <= 20 { b } else { PathBuf::from("/tmp") };
    make(&root, prefix, tag)
}

/// The newest folder this process made with [`unique`], [`short`] or [`outside_home`] for `prefix` and `tag` (tests that look at a
/// file a service wrote into a folder another helper created). Panics when there is none.
pub fn find(prefix: &str, tag: &str) -> PathBuf {
    let want = format!("{prefix}-{}-", std::process::id());
    let suffix = format!("-{tag}");
    let mut best: Option<(u32, PathBuf)> = None;
    for root in [base(), PathBuf::from("/tmp"), PathBuf::from("/var/tmp"), PathBuf::from("/dev/shm")] {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(mid) = name.strip_prefix(&want).and_then(|r| r.strip_suffix(&suffix)) {
                if let Ok(n) = mid.parse::<u32>() {
                    if best.as_ref().map_or(true, |(b, _)| n > *b) {
                        best = Some((n, e.path()));
                    }
                }
            }
        }
    }
    best.unwrap_or_else(|| panic!("no test folder {prefix} {tag} of this process")).1
}

/// A TCP port that is free now, for a test that gives it to a server a moment later. Same scheme as the agent's helper (the two crates'
/// tests can run at the same time on one machine and share the claim folder): ports below the kernel's ephemeral range, each handed out
/// once per process, probed on all addresses and claimed with an exclusive `flock` that lasts as long as this process.
pub fn free_port() -> u16 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let port = (20_000 + ((std::process::id() % 8_000) * 37 + n) % 8_000) as u16;
        if claim_port(port) && std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return port;
        }
    }
}

fn claim_port(port: u16) -> bool {
    use std::os::fd::AsRawFd;
    static HELD: std::sync::Mutex<Vec<std::fs::File>> = std::sync::Mutex::new(Vec::new());
    // SAFETY: getuid has no arguments and cannot fail.
    let dir = PathBuf::from(format!("/tmp/iyakki-test-ports-{}", unsafe { libc::getuid() }));
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join(port.to_string())) else { return true };
    // SAFETY: the descriptor is open and owned by `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return false;
    }
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push(file);
    true
}
