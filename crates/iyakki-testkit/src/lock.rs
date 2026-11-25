//! Named resource locks, across threads and across processes.
//!
//! `lock("name")` returns a guard; while it is alive no other thread, test binary or parallel cargo run on this machine (same user) can
//! hold `"name"`. Keep it only for the critical section (set the thing, run the code that reads it, restore it). The lock is an `flock` on
//! `<tmp>/iyakki-testkit-<uid>/<name>.lock`, so the kernel drops it when a test process dies. The holder (process, thread, since when) is
//! written in the file; a timeout panics with that text. Locking the same name twice in one thread would wait for itself: that panics at once.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The standard names of the things tests really share.
pub mod names {
    /// The process-global input state (`crate::input`: pointer speed, key state).
    pub const INPUT: &str = "input-state";
    /// The process environment (`set_var`/`remove_var`, and code that reads variables).
    pub const ENV: &str = "process-env";
    /// The current directory (`set_current_dir`).
    pub const CWD: &str = "current-dir";
    /// The shared k3s/kube cluster of the machine (a test that creates, patches or deletes in it).
    pub const CLUSTER: &str = "kube-cluster";
    /// A fixed local port (a test that must use one particular number).
    pub const FIXED_PORT: &str = "fixed-port";
    /// The audio mixer and other global statics of the sound code.
    pub const AUDIO: &str = "audio-mixer";
    /// A system-clock override (a test that sets a process-wide time source).
    pub const CLOCK: &str = "clock-override";
    /// The process-wide clipboard (`ui::clipboard`): a test that copies, clears or pastes.
    pub const CLIPBOARD: &str = "process-clipboard";
    /// The browser's active header rules (`iyakki_agent::browserheaders::activate`): one set per process, read by the transport.
    pub const BROWSER_HEADERS: &str = "browser-header-rules";
}

/// How long a test waits for a lock before it panics with the holder's name (the longest critical section is a few seconds).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

thread_local! { static HELD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

/// A held lock; released when dropped.
#[derive(Debug)]
pub struct Lock {
    name: String,
    file: Option<File>,
    _process: Option<ProcessLock>,
}

/// Names whose state lives inside ONE process (its environment, its current folder): the lock between the threads of this process
/// is enough, a second test binary or a parallel cargo run has its own environment and must not be made to queue behind it
/// (hunts with four runs at once starved each other on the 120 s file lock).
fn per_process(name: &str) -> bool {
    name == names::ENV || name == names::CWD
}

fn dir() -> PathBuf {
    // SAFETY: getuid has no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    // a fixed base (not TMPDIR): parallel cargo runs with different TMPDIR must still meet at the same file
    let base = if std::path::Path::new("/tmp").is_dir() { PathBuf::from("/tmp") } else { std::env::temp_dir() };
    let d = base.join(format!("iyakki-testkit-{uid}"));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn who() -> String {
    let t = std::thread::current();
    format!("pid {} thread {:?} ({})", std::process::id(), t.name().unwrap_or("unnamed"), std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default())
}

/// Takes the lock `name`, waiting up to [`DEFAULT_TIMEOUT`]. Panics (naming the holder) when it cannot.
pub fn lock(name: &str) -> Lock {
    lock_timeout(name, DEFAULT_TIMEOUT)
}

/// Like [`lock`] with a chosen timeout.
pub fn lock_timeout(name: &str, timeout: Duration) -> Lock {
    let again = HELD.with(|h| h.borrow().iter().any(|n| n == name));
    assert!(!again, "testkit lock {name:?} is already held by this thread: locking it again would wait for itself forever (take it once, around the whole critical section)");
    if per_process(name) {
        let held = process_lock(name);
        HELD.with(|h| h.borrow_mut().push(name.to_string()));
        return Lock { name: name.to_string(), file: None, _process: Some(held) };
    }
    let path = dir().join(format!("{}.lock", safe(name)));
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).unwrap_or_else(|e| panic!("testkit lock {name:?}: cannot open {}: {e}", path.display()));
    let start = Instant::now();
    loop {
        // SAFETY: the descriptor is open and owned by `file`; flock only reads the two integers.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        if start.elapsed() >= timeout {
            let mut holder = String::new();
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.read_to_string(&mut holder);
            panic!("testkit lock {name:?} not free after {:?}; held by: {}", timeout, if holder.trim().is_empty() { "(unknown holder)".into() } else { holder.trim().to_string() });
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    let _ = file.set_len(0);
    let _ = file.seek(SeekFrom::Start(0));
    let _ = write!(file, "{} since {}s after epoch {}", who(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), name);
    HELD.with(|h| h.borrow_mut().push(name.to_string()));
    Lock { name: name.to_string(), file: Some(file), _process: None }
}

impl Drop for Lock {
    fn drop(&mut self) {
        HELD.with(|h| h.borrow_mut().retain(|n| n != &self.name));
        if let Some(file) = &self.file {
            let _ = file.set_len(0);
            // SAFETY: the descriptor is still open (closing it also drops the lock; the explicit unlock is for clarity).
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// Sets environment variables under the [`names::ENV`] lock and puts the old values back when dropped. Keep it only as long as the code
/// that reads the variables runs.
pub struct EnvGuard {
    old: Vec<(String, Option<std::ffi::OsString>)>,
    _lock: Lock,
}

impl EnvGuard {
    /// Sets (`Some`) or removes (`None`) each variable.
    pub fn set(vars: &[(&str, Option<&str>)]) -> EnvGuard {
        let lock = lock(names::ENV);
        let mut old = Vec::new();
        for (k, v) in vars {
            old.push((k.to_string(), std::env::var_os(k)));
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        EnvGuard { old, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.old.drain(..).rev() {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
    }
}

/// How long [`process_lock`] waits: the tests of one binary queue here one after the other, so the wait is the SUM of the sections ahead.
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(900);

static PROCESS_HELD: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
static PROCESS_FREED: std::sync::Condvar = std::sync::Condvar::new();

/// A held in-process lock; released when dropped.
#[derive(Debug)]
pub struct ProcessLock {
    name: String,
}

/// A named lock between the threads of THIS test process only (no file, no other process involved). Use it for state that lives inside one
/// process (a `static` of the code under test): a second test binary or a parallel cargo run has its own copy of that state and must not
/// be made to wait. A test that panics while holding it does not poison it. Waits up to [`PROCESS_TIMEOUT`], then panics naming the holder.
pub fn process_lock(name: &str) -> ProcessLock {
    let start = Instant::now();
    let mut held = PROCESS_HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if !held.iter().any(|(n, _)| n == name) {
            held.push((name.to_string(), who()));
            return ProcessLock { name: name.to_string() };
        }
        let left = PROCESS_TIMEOUT.saturating_sub(start.elapsed());
        if left.is_zero() {
            let by = held.iter().find(|(n, _)| n == name).map(|(_, w)| w.clone()).unwrap_or_default();
            drop(held);
            panic!("testkit process lock {name:?} not free after {PROCESS_TIMEOUT:?}; held by: {by}");
        }
        held = PROCESS_FREED.wait_timeout(held, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        let mut held = PROCESS_HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.retain(|(n, _)| *n != self.name);
        drop(held);
        PROCESS_FREED.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_threads_never_hold_the_same_name_together() {
        let inside = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let hs: Vec<_> = (0..6)
            .map(|_| {
                let inside = inside.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let _g = lock("selftest-exclusive");
                        assert_eq!(inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0, "two holders at once");
                        std::thread::yield_now();
                        inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }

    #[test]
    fn a_timeout_names_the_holder_and_a_second_take_in_one_thread_is_refused() {
        let g = lock("selftest-timeout");
        let r = std::thread::spawn(|| std::panic::catch_unwind(|| lock_timeout("selftest-timeout", Duration::from_millis(60))).map(|_| ()).map_err(|e| e.downcast_ref::<String>().cloned().unwrap_or_default())).join().unwrap();
        let msg = r.unwrap_err();
        assert!(msg.contains("held by: pid") && msg.contains("selftest-timeout"), "{msg}");
        let again = std::panic::catch_unwind(|| lock("selftest-timeout"));
        assert!(again.is_err());
        drop(g);
        let _ok = lock_timeout("selftest-timeout", Duration::from_secs(5));
    }

    #[test]
    fn the_env_guard_restores_the_old_value() {
        {
            let _g = EnvGuard::set(&[("IYAKKI_TESTKIT_SELFTEST", Some("a"))]);
            assert_eq!(std::env::var("IYAKKI_TESTKIT_SELFTEST").unwrap(), "a");
        }
        let _l = lock(names::ENV);
        assert!(std::env::var("IYAKKI_TESTKIT_SELFTEST").is_err());
    }

    #[test]
    fn a_process_lock_excludes_other_threads_and_survives_a_panic_while_held() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let inside = Arc::new(AtomicU32::new(0));
        let hs: Vec<_> = (0..6)
            .map(|_| {
                let inside = inside.clone();
                std::thread::spawn(move || {
                    let _g = process_lock("testkit-process-lock-self-test");
                    assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0, "two threads inside at once");
                    std::thread::yield_now();
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let r = std::thread::spawn(|| {
            let _g = process_lock("testkit-process-lock-self-test");
            panic!("a failing test");
        })
        .join();
        assert!(r.is_err());
        let _again = process_lock("testkit-process-lock-self-test");
    }
}
