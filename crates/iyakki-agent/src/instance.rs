//! One window per data folder, as a hint: `<dir>/window.lock` names the process that runs IYAKKI on the folder. A second start
//! is warned, never blocked (two windows share accounts and settings, which is allowed but surprising). Stale-aware: a lock
//! whose process is gone, or whose pid now belongs to some other program, does not count.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub pid: u32,
    pub started: u64,
}

pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("window.lock")
}

/// Is `pid` a running IYAKKI program (its command name starts with "iyakki")? A reused pid of another program says no.
fn alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    crate::jobs::process_name(pid).is_some_and(|c| c.starts_with("iyakki"))
}

/// The other live window on this folder, if any.
pub fn other_running(dir: &Path) -> Option<Instance> {
    let i: Instance = serde_json::from_slice(&std::fs::read(lock_path(dir)).ok()?).ok()?;
    (i.pid != std::process::id() && alive(i.pid)).then_some(i)
}

/// Record this process as the window of the folder (replacing a stale or, when the person chose to continue, a live one).
pub fn claim(dir: &Path) {
    let me = Instance { pid: std::process::id(), started: crate::clock::now_unix().max(0) as u64 };
    let _ = std::fs::create_dir_all(dir);
    let p = lock_path(dir);
    let tmp = p.with_extension("tmp");
    if let Ok(b) = serde_json::to_vec(&me) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Give the claim back (only if it is still ours).
pub fn release(dir: &Path) {
    let ours = std::fs::read(lock_path(dir)).ok().and_then(|b| serde_json::from_slice::<Instance>(&b).ok()).is_some_and(|i| i.pid == std::process::id());
    if ours {
        let _ = std::fs::remove_file(lock_path(dir));
    }
}

/// The warning shown when `other` is already running.
pub fn warning(other: &Instance) -> String {
    format!("IYAKKI is already running on this data folder (process {}). Two windows share accounts and settings; continue anyway?", other.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_window_is_noticed_and_stale_claims_are_ignored() {
        let d = std::env::temp_dir().join(format!("iyakki-inst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(other_running(&d).is_none(), "no lock, nobody else");
        claim(&d);
        assert!(other_running(&d).is_none(), "our own claim is not another window");
        // a dead process
        std::fs::write(lock_path(&d), r#"{"pid":4000000000,"started":1}"#).unwrap();
        assert!(other_running(&d).is_none(), "a dead process does not count");
        // a live process that is not IYAKKI (pid 1 is init/systemd): a reused pid does not count
        std::fs::write(lock_path(&d), r#"{"pid":1,"started":1}"#).unwrap();
        assert!(other_running(&d).is_none(), "a pid reused by another program does not count");
        // garbage
        std::fs::write(lock_path(&d), "nope").unwrap();
        assert!(other_running(&d).is_none());
        // a live IYAKKI window: a child process whose command name starts with "iyakki"
        let exe = std::env::current_exe().unwrap();
        let fake = d.join("iyakki-fake");
        std::fs::copy("/bin/sleep", &fake).unwrap();
        let _ = exe;
        // another test's fork can hold the fresh copy open for a moment ("text file busy"): try again
        let mut child = (0..50)
            .find_map(|_| {
                std::process::Command::new(&fake).arg("30").spawn().ok().or_else(|| {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    None
                })
            })
            .expect("started a program named iyakki-fake");
        std::fs::write(lock_path(&d), format!(r#"{{"pid":{},"started":1}}"#, child.id())).unwrap();
        let o = other_running(&d).expect("a live iyakki process is another window");
        assert!(warning(&o).contains("already running") && warning(&o).contains(&child.id().to_string()));
        // continuing anyway: we take the claim, and release only removes ours
        claim(&d);
        assert!(other_running(&d).is_none());
        release(&d);
        assert!(!lock_path(&d).exists());
        std::fs::write(lock_path(&d), format!(r#"{{"pid":{},"started":1}}"#, child.id())).unwrap();
        release(&d);
        assert!(lock_path(&d).exists(), "somebody else's claim stays");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&d);
    }
}
