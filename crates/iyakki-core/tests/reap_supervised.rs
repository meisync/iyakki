//! The reaper of PID 1 (`iyakki_core::reap_children`) and the supervisors (`helpers::supervise`): a child a supervisor waits for keeps its exit status
//! (the booted run of `iyakki.ui=process` on 2026-11-24 logged "gone (No child process)" for the UI program because the reaper took the status), an orphan
//! nobody waits for is still reaped. One test in this file: it reaps every ended child of the test process, so no other test may run beside it.

use std::process::{Command, Stdio};

/// The pid shows as an ended child (a zombie): `waitid` with WNOWAIT looks without reaping.
fn ended(pid: u32) -> bool {
    // SAFETY: waitid with WNOHANG | WNOWAIT on one pid of ours writes only into the zeroed siginfo.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0 && info.si_pid() as u32 == pid
    }
}

#[test]
fn a_supervised_child_keeps_its_exit_status_and_an_orphan_is_reaped() {
    let mut watched = Command::new("sh").args(["-c", "exit 7"]).stdin(Stdio::null()).spawn().expect("sh");
    let mut orphan = Command::new("sh").args(["-c", "exit 0"]).stdin(Stdio::null()).spawn().expect("sh");
    iyakki_core::keep_supervised(watched.id());
    let started = std::time::Instant::now();
    while !(ended(watched.id()) && ended(orphan.id())) {
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "the children never ended");
        std::thread::yield_now();
    }
    iyakki_core::reap_children();
    // the supervised one is still there to be waited for, with its real status
    let status = watched.try_wait().expect("the status of the watched child is still there").expect("it has ended");
    assert_eq!(status.code(), Some(7));
    // the orphan was reaped: nothing is left to wait for
    assert!(orphan.try_wait().is_err(), "the reaper took the orphan's status");
    iyakki_core::release_supervised(watched.id());
}
