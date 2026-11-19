//! A look at a child process that survives a PID-1 reaper taking its exit status.
//!
//! The same semantics as `iyakki_core_api::child_status` (this crate has no dependencies, so it keeps its own small copy):
//! `Err(ECHILD)` from `Child::try_wait()` or a process that no longer exists means the child has ended and its status is not
//! known (`Gone`), never "still running".

use std::process::Child;

const ECHILD: i32 = 10;
const ESRCH: i32 = 3;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Running,
    /// Ended; `Some(code)` or `None` when a signal ended it.
    Exited(Option<i32>),
    /// Ended, status unavailable (another reaper took it).
    Gone,
}

impl ChildState {
    pub fn is_over(&self) -> bool {
        !matches!(self, ChildState::Running)
    }
    pub fn describe(&self) -> String {
        match self {
            ChildState::Running => "still running".into(),
            ChildState::Exited(Some(c)) => format!("exit status: {c}"),
            ChildState::Exited(None) => "ended by a signal".into(),
            ChildState::Gone => "exit status not available".into(),
        }
    }
}

pub fn child_status(child: &mut Child) -> ChildState {
    match child.try_wait() {
        Ok(None) => ChildState::Running,
        Ok(Some(st)) => ChildState::Exited(st.code()),
        Err(e) => {
            // SAFETY: signal 0 only asks whether the process exists.
            let vanished = unsafe { kill(child.id() as i32, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(ESRCH);
            if e.raw_os_error() == Some(ECHILD) || vanished {
                ChildState::Gone
            } else {
                ChildState::Running
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" {
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }

    #[test]
    fn a_status_taken_behind_its_back_is_gone_not_running() {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let mut st = 0;
        // SAFETY: waitpid on the pid of the child spawned above.
        assert_eq!(unsafe { waitpid(c.id() as i32, &mut st, 0) } as u32, c.id());
        assert!(c.try_wait().is_err());
        assert_eq!(child_status(&mut c), ChildState::Gone);
        let mut s = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert_eq!(child_status(&mut s), ChildState::Running);
        s.kill().unwrap();
        s.wait().unwrap();
        let mut t = std::process::Command::new("sh").args(["-c", "exit 2"]).spawn().unwrap();
        t.wait().unwrap();
        assert_eq!(child_status(&mut t), ChildState::Exited(Some(2)));
    }
}
