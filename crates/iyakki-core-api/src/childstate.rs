//! One honest look at a child process, for programs that run as PID 1 (or next to a reaper).
//!
//! The PID-1 reaper calls `waitpid(-1, WNOHANG)`, which can take the exit status of any child, including one that other code
//! watches with `Child::try_wait()`. When the reaper wins, `try_wait()` returns `Err(ECHILD)` instead of the status. Code that
//! reads that error as "still running" never notices the death. [`child_status`] turns it into [`ChildState::Gone`]: the child
//! has ended, its exit code is not known. See docs/development/PID1-REAPER.md.

use std::process::Child;

/// What a child process is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    /// Still running.
    Running,
    /// Ended, status read: `Some(code)`, or `None` when a signal ended it.
    Exited(Option<i32>),
    /// Ended, but nobody can read its status any more (another reaper took it, or the process no longer exists).
    Gone,
}

impl ChildState {
    pub fn is_running(&self) -> bool {
        matches!(self, ChildState::Running)
    }
    /// Ended in either way.
    pub fn is_over(&self) -> bool {
        !self.is_running()
    }
    /// Ended with exit code 0. A signal, a failure code and an unknown status are all not success.
    pub fn success(&self) -> bool {
        matches!(self, ChildState::Exited(Some(0)))
    }
    /// The exit code, when it is known.
    pub fn code(&self) -> Option<i32> {
        match self {
            ChildState::Exited(c) => *c,
            _ => None,
        }
    }
    /// A short phrase for messages.
    pub fn describe(&self) -> String {
        match self {
            ChildState::Running => "still running".into(),
            ChildState::Exited(Some(c)) => format!("exit status: {c}"),
            ChildState::Exited(None) => "ended by a signal".into(),
            ChildState::Gone => "exited, its exit status is not available".into(),
        }
    }
}

fn from_status(st: std::process::ExitStatus) -> ChildState {
    ChildState::Exited(st.code())
}

fn pid_vanished(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid as i32, 0) != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) }
}

/// Look at a child without waiting. `Err(ECHILD)` from the OS, or a pid that no longer exists, is `Gone`; any other error with
/// a live pid is `Running` (nothing proves it ended).
pub fn child_status(child: &mut Child) -> ChildState {
    match child.try_wait() {
        Ok(None) => ChildState::Running,
        Ok(Some(st)) => from_status(st),
        Err(e) => {
            if e.raw_os_error() == Some(libc::ECHILD) || pid_vanished(child.id()) {
                ChildState::Gone
            } else {
                ChildState::Running
            }
        }
    }
}

/// Wait for the end of a child. `Err(ECHILD)` (the status was taken by another reaper) is `Gone`, not a failure; the caller has
/// still to read the pipes it holds. Any other error also ends the wait as `Gone` (there is nothing left to wait for).
pub fn child_wait(child: &mut Child) -> ChildState {
    match child.wait() {
        Ok(st) => from_status(st),
        Err(_) => ChildState::Gone,
    }
}

/// For tests: wait for `pid` to end and take its exit status behind the owner's back, as the PID-1 reaper does. Afterwards
/// `Child::try_wait()` on it returns `Err(ECHILD)`. Only call it with the pid of a child the test itself spawned.
#[doc(hidden)]
pub fn steal_status(pid: u32) {
    let mut st = 0;
    // SAFETY: waitpid on one pid; the caller owns that child.
    let r = unsafe { libc::waitpid(pid as i32, &mut st, 0) };
    assert_eq!(r as u32, pid, "the child was not there to steal");
}

/// For tests of code that starts its own children: from a background thread, find a child of this process whose command line
/// contains `marker` (within `within`), wait for it and take its exit status, as the PID-1 reaper would. Returns the pid
/// taken (0 when none was found). Other children of the test process are left alone.
#[doc(hidden)]
pub fn steal_status_of_marked(marker: &str, within: std::time::Duration) -> std::thread::JoinHandle<u32> {
    let marker = marker.to_string();
    std::thread::spawn(move || {
        let me = std::process::id();
        let end = std::time::Instant::now() + within;
        while std::time::Instant::now() < end {
            if let Ok(rd) = std::fs::read_dir("/proc") {
                for e in rd.flatten() {
                    let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
                    let ppid = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| t.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1).and_then(|p| p.parse::<u32>().ok())));
                    if ppid != Some(me) {
                        continue;
                    }
                    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                    if String::from_utf8_lossy(&cmd).contains(&marker) {
                        let mut st = 0;
                        // SAFETY: waitpid on the pid of a marked child of this process.
                        let r = unsafe { libc::waitpid(pid as i32, &mut st, 0) };
                        return if r > 0 { pid } else { 0 };
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    use super::steal_status;

    #[test]
    fn running_exited_and_signalled() {
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        assert_eq!(child_status(&mut c), ChildState::Running);
        c.kill().unwrap();
        let st = child_wait(&mut c);
        assert_eq!(st, ChildState::Exited(None));
        assert!(st.is_over() && !st.success());
        let mut f = Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap();
        let st = child_wait(&mut f);
        assert_eq!((st.code(), st.success()), (Some(3), false));
        let mut t = Command::new("true").spawn().unwrap();
        assert!(child_wait(&mut t).success());
    }

    #[test]
    fn a_status_taken_by_another_reaper_is_gone_not_running() {
        let mut c = Command::new("true").spawn().unwrap();
        steal_status(c.id());
        // std alone says Err(ECHILD) here: this is the bug being guarded against
        assert!(c.try_wait().is_err());
        assert_eq!(child_status(&mut c), ChildState::Gone);
        assert_eq!(child_wait(&mut c), ChildState::Gone);
        assert!(ChildState::Gone.is_over() && !ChildState::Gone.success() && ChildState::Gone.code().is_none());
    }

    #[test]
    fn pipes_can_still_be_read_after_the_status_was_taken() {
        use std::io::Read;
        let mut c = Command::new("echo").arg("hello").stdout(Stdio::piped()).spawn().unwrap();
        steal_status(c.id());
        assert_eq!(child_wait(&mut c), ChildState::Gone);
        let mut s = String::new();
        c.stdout.take().unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s.trim(), "hello");
    }
}
