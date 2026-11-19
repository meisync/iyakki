//! The UI as its own program (F-SPLIT-03, docs/development/UI-BOUNDARY.md section 14): when `/usr/bin/iyakki-ui` exists and the person (or the
//! image) asked for it, the core starts it as a child and keeps it running with the helper supervisor (restart with a growing pause, give up
//! after rapid crashes, SIGTERM then kill on stop) instead of running the shell as a thread of itself. OFF by default: the switch is the file
//! `/etc/iyakki/ui-process.enabled` or the kernel parameter `iyakki.ui=process`; `iyakki.ui=thread` forces the old way.
//!
//! The program is told where the core's UI socket is and, after a crash, `--restore` (the shell's own sealed restore token decides whether
//! anything is restored: developer mode only, F-SHELL-65).

use std::path::{Path, PathBuf};

use crate::helpers::{Audit, Handle, Policy, Spec};

pub const EXE: &str = "/usr/bin/iyakki-ui";
pub const FLAG: &str = "/etc/iyakki/ui-process.enabled";
pub const SOCKET: &str = "/run/iyakki/ui/ui.sock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The shell is a thread of the core (today's way).
    Thread,
    /// The shell is the program `iyakki-ui`, supervised.
    Process,
}

/// Which way to run the UI. The kernel parameter wins over the flag file; without the program file the answer is always `Thread`.
pub fn mode(cmdline: &str, flag_file: bool, exe_exists: bool) -> Mode {
    if !exe_exists {
        return Mode::Thread;
    }
    match crate::boot::cmdline_param(cmdline, "iyakki.ui") {
        Some("process") => Mode::Process,
        Some("thread") => Mode::Thread,
        _ if flag_file => Mode::Process,
        _ => Mode::Thread,
    }
}

/// The command the supervisor runs.
pub fn spec(exe: &Path, socket: &Path, fb: &str, restore: bool) -> Spec {
    let mut args = vec!["--socket".to_string(), socket.display().to_string(), "--fb".to_string(), fb.to_string()];
    if restore {
        args.push("--restore".into());
    }
    Spec { name: "ui".into(), program: exe.to_path_buf(), args }
}

/// The policy of the UI program: it is the screen, so it comes back fast, and it is given up on only after many quick crashes (the core then
/// falls back to the thread, see [`give_up_falls_back`]).
pub fn policy() -> Policy {
    Policy { first_pause: std::time::Duration::from_millis(500), max_pause: std::time::Duration::from_secs(10), healthy_after: std::time::Duration::from_secs(30), max_rapid: 6, grace: std::time::Duration::from_secs(5) }
}

/// A supervisor that gave up leaves the person with no screen: the caller runs the shell as a thread instead.
pub fn give_up_falls_back(state: &crate::helpers::State) -> bool {
    matches!(state, crate::helpers::State::GaveUp { .. })
}

/// Start the program under the supervisor. `restore` is true when this is not the first start of the boot (a restart after a crash is the
/// supervisor's own, so the first start is the only one that can say false; the program asks the shell's token anyway).
pub fn start(exe: PathBuf, fb: &str, audit: Audit) -> Handle {
    crate::helpers::supervise(spec(&exe, Path::new(SOCKET), fb, true), policy(), audit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::State;
    use std::sync::{Arc, Mutex};

    #[test]
    fn the_program_is_used_only_when_it_exists_and_was_asked_for() {
        assert_eq!(mode("quiet", false, true), Mode::Thread, "off by default");
        assert_eq!(mode("quiet", true, true), Mode::Process, "the flag file asks for it");
        assert_eq!(mode("quiet iyakki.ui=process", false, true), Mode::Process);
        assert_eq!(mode("quiet iyakki.ui=thread", true, true), Mode::Thread, "the kernel parameter wins over the file");
        assert_eq!(mode("iyakki.ui=process", true, false), Mode::Thread, "no program file, no process");
    }

    #[test]
    fn the_command_names_the_socket_the_screen_and_restore_and_runs_no_shell() {
        let s = spec(Path::new("/usr/bin/iyakki-ui"), Path::new(SOCKET), "/dev/fb0", true);
        assert_eq!(s.args, vec!["--socket", SOCKET, "--fb", "/dev/fb0", "--restore"]);
        assert_eq!(spec(Path::new("/x"), Path::new(SOCKET), "/dev/fb0", false).args.len(), 4);
    }

    /// A UI program that dies at once is restarted a bounded number of times and then given up on, and the core is told so it can fall back.
    #[cfg(unix)]
    #[test]
    fn a_program_that_keeps_crashing_is_restarted_then_given_up_on_and_the_core_falls_back() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let l2 = lines.clone();
        let audit: Audit = Arc::new(move |e, d| l2.lock().unwrap().push(format!("{e} {d}")));
        let spec = Spec { name: "ui".into(), program: "/bin/sh".into(), args: vec!["-c".into(), "exit 1".into()] };
        let p = Policy { first_pause: std::time::Duration::from_millis(1), max_pause: std::time::Duration::from_millis(2), healthy_after: std::time::Duration::from_secs(3600), max_rapid: 3, grace: std::time::Duration::from_millis(50) };
        let h = crate::helpers::supervise(spec, p, audit);
        // wait for the supervisor to give up (a state change, not a clock): poll the state until it is final
        let state = loop {
            let s = h.state();
            if matches!(s, State::GaveUp { .. }) {
                break s;
            }
            std::thread::yield_now();
        };
        assert!(give_up_falls_back(&state));
        assert!(!give_up_falls_back(&State::Running { pid: 1, starts: 1 }));
        assert!(lines.lock().unwrap().len() >= 3, "every start and exit is audited");
    }
}
