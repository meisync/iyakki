//! Per-test temporary folders (test builds only).
//!
//! Tests run in one process, in parallel, and `./run.sh test` points `TMPDIR` at `target/tmp` inside the build tree, which on a
//! developer machine is usually below `$HOME` and below a long path. Two things followed from that and made tests fail for reasons
//! that were not bugs: (1) tests that need a folder OUTSIDE the person's home found it inside, and (2) Unix socket paths (at most
//! 108 bytes, the code allows 100) came out too long. This module gives every call its own folder (process id plus a counter, so
//! two tests never share or delete each other's folder) and offers the two special cases. Folders left behind by a process that no
//! longer exists are removed on the next call.

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
    // no /proc (macOS): cannot tell, so a folder of another process is never swept (it may belong to a test run still going)
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
    // the real path: `/tmp` and `/var` are links on macOS, and code that checks a folder against the allowed folders compares real paths
    std::fs::canonicalize(&d).unwrap_or(d)
}

/// Does nothing answer on `127.0.0.1:port`? A port that was just freed can be taken at once by another test of this process (they all
/// bind port 0), so a refusal at any try within two seconds is the proof; only a port that answers the whole time is a failure.
pub fn port_is_closed(port: u16) -> bool {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_secs(2) {
        if std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::time::Duration::from_millis(200)).is_err() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
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

/// Whether a folder or one of its parents is a Cargo project or a git work tree (what a project lookup that walks up would find).
fn inside_a_project(p: &Path) -> bool {
    p.ancestors().any(|a| a.join("Cargo.toml").exists() || a.join(".git").exists())
}

/// A fresh folder that is NOT inside any Cargo project or git work tree (for tests of "no project here" and "not a git folder"). The
/// gate sets `TMPDIR` inside the repository's own `target/`, so the test base is below the repo's `Cargo.toml` and `.git`. Tries the
/// test base, then `/tmp`, `/var/tmp` and `/dev/shm`.
pub fn outside_project(prefix: &str, tag: &str) -> PathBuf {
    for c in [base(), PathBuf::from("/tmp"), PathBuf::from("/var/tmp"), PathBuf::from("/dev/shm")] {
        let c = std::fs::canonicalize(&c).unwrap_or(c);
        if c.is_dir() && !inside_a_project(&c) {
            return make(&c, prefix, tag);
        }
    }
    panic!("no writable folder outside every Cargo project and git work tree for the test");
}

/// A fresh folder whose path is short enough for Unix sockets inside it (`budget` bytes of the folder itself at most 40, so that
/// `<folder>/<a few levels>/agent.sock` stays below the 100 bytes the code allows). Uses the test base when it is short, else `/tmp`.
pub fn short(prefix: &str, tag: &str) -> PathBuf {
    let b = base();
    let root = if b.as_os_str().len() <= 20 { b } else { PathBuf::from("/tmp") };
    make(&root, prefix, tag)
}

#[cfg(test)]
#[test]
fn folders_are_unique_and_the_special_ones_have_their_property() {
    let (a, b) = (unique("iyakki-tdtest", "x"), unique("iyakki-tdtest", "x"));
    assert_ne!(a, b);
    let o = outside_home("iyakki-tdtest", "o");
    if let Some(h) = std::env::var("HOME").ok().and_then(|h| std::fs::canonicalize(h).ok()) { // home-ok: read only
        assert!(!o.starts_with(h));
    }
    let s = short("iyakki-tdtest", "s");
    assert!(s.as_os_str().len() <= 50, "{}", s.display());
    for d in [a, b, o, s] {
        let _ = std::fs::remove_dir_all(d);
    }
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

/// A TCP port that is free now, for a test that gives it to a server a moment later. Not from the kernel's ephemeral range
/// (`bind` to port 0 hands those out, and so does every outgoing connection of the other tests in this process): ports below it,
/// each handed out once per process (8000 of them), probed on all addresses for TCP and UDP (a test may give the number to a DNS server). Several test PROCESSES can run at the same time on one machine
/// (other builds, other people's runs), and two of them could be handed the same number between the probe and the use: every port is therefore
/// also claimed with a small file in a folder all test processes of this user share (a file of a process that no longer exists is taken over).
pub fn free_port() -> u16 {
    // 32-bit arithmetic: with 16 bits the product and the sum wrapped around for some process ids, and two different calls then got the same
    // port (a listener restarted on "its" port found it taken by another test). Consecutive calls give 8000 different ports.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let port = (20_000 + ((std::process::id() % 8_000) * 37 + n) % 8_000) as u16;
        if claim_port(port) && std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() && std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok() {
            return port;
        }
    }
}

/// Who holds `port` right now, for the message of a test that found it taken ("Address already in use"): the sockets of /proc/net
/// (tcp, tcp6, udp, udp6) on that local port with their state, and the process (id and name) that owns each, when it can be seen.
/// Tests only look at it after a failure; nothing depends on its answer.
#[allow(dead_code)]
pub fn describe_port(port: u16) -> String {
    let hex = format!(":{port:04X}");
    let mut found: Vec<(String, String, String)> = Vec::new(); // (table, state, inode)
    for table in ["tcp", "tcp6", "udp", "udp6"] {
        let Ok(text) = std::fs::read_to_string(format!("/proc/net/{table}")) else { continue };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() > 9 && f[1].ends_with(&hex) {
                found.push((table.to_string(), f[3].to_string(), f[9].to_string()));
            }
        }
    }
    if found.is_empty() {
        return format!("port {port}: no socket holds it now (it was released after the failure)");
    }
    let mut out = Vec::new();
    for (table, state, inode) in found {
        let mut owner = String::from("owner not visible");
        if let Ok(rd) = std::fs::read_dir("/proc") {
            'procs: for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else { continue };
                for fd in fds.flatten() {
                    if std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy() == format!("socket:[{inode}]")) {
                        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
                        owner = format!("process {name} ({}), this test process is {}", comm.trim(), std::process::id());
                        break 'procs;
                    }
                }
            }
        }
        out.push(format!("{table} state {state}: {owner}"));
    }
    format!("port {port}: {}", out.join("; "))
}

/// Claim `port` for this process in the shared folder `/tmp/iyakki-test-ports-<uid>`: false when another running process holds it.
/// The claim is an exclusive `flock` on the port's file, kept open until the process ends: the kernel drops it when the process dies, so
/// there is no stale file to judge and no window in which two processes could both decide to take a file over (the earlier "file with the
/// process id in it" scheme removed a fresh, still empty file of another process and then both held the same port). When the folder cannot
/// be used the claim is given (the probe in `free_port` is then all there is).
fn claim_port(port: u16) -> bool {
    use std::os::fd::AsRawFd;
    static HELD: std::sync::Mutex<Vec<std::fs::File>> = std::sync::Mutex::new(Vec::new());
    // SAFETY: getuid has no arguments and cannot fail.
    let dir = PathBuf::from(format!("/tmp/iyakki-test-ports-{}", unsafe { libc::getuid() }));
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join(port.to_string())) else { return true };
    // SAFETY: the descriptor is open and owned by `file`; flock only reads the two integers.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return false; // held by another process (or by an earlier claim of this one)
    }
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push(file);
    true
}

#[cfg(test)]
#[test]
fn a_claimed_port_is_not_handed_out_again_and_a_claim_ends_with_its_process() {
    // 29990 is outside the range free_port hands out, so no other test touches it
    let dir = PathBuf::from(format!("/tmp/iyakki-test-ports-{}", unsafe { libc::getuid() }));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("29990");
    let _ = std::fs::remove_file(&f);
    // a claim held by another open file (another process in real life) is refused
    let other = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&f).unwrap();
    assert_eq!(unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&other), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    assert!(!claim_port(29990), "held by a running process");
    drop(other); // the owner ends: the kernel lets go of the claim
    // (a test that starts a program in this process can hold the old descriptor for a moment between fork and exec: give it a moment)
    assert!((0..100).any(|_| claim_port(29990) || { std::thread::sleep(std::time::Duration::from_millis(20)); false }), "its owner is gone");
    assert!(!claim_port(29990), "now this process holds it");
    let _ = std::fs::remove_file(&f);
    let (a, b) = (free_port(), free_port());
    assert_ne!(a, b);
}

#[cfg(test)]
#[test]
fn ports_are_never_handed_to_two_test_processes_at_once() {
    // child mode: hold 400 ports for a moment and print them; parent mode: start 6 copies of this test at the same time and compare
    if std::env::var("IYAKKI_PORT_CHILD").is_ok() {
        let mine: Vec<u16> = (0..400).map(|_| free_port()).collect();
        println!("PORTS {}", mine.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" "));
        std::thread::sleep(std::time::Duration::from_millis(1500)); // all copies hold their ports at the same time
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let kids: Vec<_> = (0..6)
        .map(|_| {
            std::process::Command::new(&exe)
                .args(["--exact", "testdir::ports_are_never_handed_to_two_test_processes_at_once", "--nocapture", "--test-threads=1"])
                .env("IYAKKI_PORT_CHILD", "1")
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut seen = std::collections::HashMap::new();
    for (i, k) in kids.into_iter().enumerate() {
        let out = k.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let line = text.lines().find_map(|l| l.split_once("PORTS ").map(|x| x.1)).unwrap_or_else(|| panic!("copy {i} printed no ports: {text}"));
        for p in line.split_whitespace() {
            if let Some(other) = seen.insert(p.to_string(), i) {
                panic!("port {p} was handed to copy {other} and copy {i} at the same time");
            }
        }
    }
}
