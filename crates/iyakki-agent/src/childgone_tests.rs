//! The PID-1 reaper (`waitpid(-1)`) can take the exit status of a child that other code watches; `Child::try_wait()` then
//! returns `Err(ECHILD)`. These tests take the status behind the code's back and check that a dead child is always seen, that
//! its output is kept and that nothing hangs. See docs/development/PID1-REAPER.md.

use iyakki_core_api::{steal_status, ChildState};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
/// The thief: finds the child of this process whose command line holds `marker`, tells `on_found` (the test then opens the gate the
/// stub waits on, so the child cannot end before the thief is ready to take its status), and waits for it as the PID-1 reaper would.
/// Gives the pid taken, 0 when the code under test got the status first.
fn thief(marker: String, on_found: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<u32> {
    std::thread::spawn(move || {
        let me = std::process::id();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            if let Ok(rd) = std::fs::read_dir("/proc") {
                for e in rd.flatten() {
                    let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
                    let ppid = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| t.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1).and_then(|p| p.parse::<u32>().ok())));
                    if ppid != Some(me) || !String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default()).contains(&marker) {
                        continue;
                    }
                    on_found();
                    let mut st = 0;
                    // SAFETY: waitpid on the pid of a marked child of this process.
                    return if unsafe { libc::waitpid(pid as i32, &mut st, 0) } > 0 { pid } else { 0 };
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        0
    })
}

#[cfg(target_os = "linux")]
/// Runs `scenario(marker, script_tail)` with a thief behind it. The stub script must end with `script_tail`: it waits until the thief has found
/// the child (no `sleep N` that a loaded machine could outlast). When the code under test happened to reap the child first (the only
/// way the thief can lose) the scenario runs again with a new marker: the test is about what the code does when the status IS taken.
fn stolen<T>(tag: &str, scenario: impl Fn(&str, &str) -> T) -> T {
    for attempt in 0..40 {
        let marker = format!("{tag}-{}-{attempt}", std::process::id());
        let gate = std::env::temp_dir().join(format!("iyakki-gone-gate-{marker}"));
        let _ = std::fs::remove_file(&gate);
        let g = gate.clone();
        let t = thief(marker.clone(), move || {
            let _ = std::fs::write(&g, b"");
        });
        let tail = format!("until [ -e '{}' ]; do sleep 0.02; done # {marker}", gate.display());
        let out = scenario(&marker, &tail);
        let taken = t.join().unwrap();
        let _ = std::fs::remove_file(&gate);
        if taken != 0 {
            return out;
        }
    }
    panic!("the test could not take the status in 40 tries");
}

fn sh(script: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script.into()]
}

#[cfg(target_os = "linux")]
#[test]
fn a_command_whose_status_was_taken_keeps_its_output_and_is_not_a_failure() {
    let o = stolen("gone-ws-1", |_, tail| crate::wsexec::run_argv(&std::env::temp_dir(), &sh(&format!("echo out; {tail}")), &[], Duration::from_secs(30), 1000));
    assert_eq!(o.stdout, "out\n");
    assert!(o.status_lost && o.code.is_none() && !o.timed_out, "{o:?}");
    assert!(o.ok(), "a kubectl whose status was taken is not a failure: {o:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_command_that_wrote_to_stderr_and_lost_its_status_is_still_a_failure() {
    let o = stolen("gone-ws-2", |_, tail| crate::wsexec::run_argv(&std::env::temp_dir(), &sh(&format!("echo boom >&2; {tail}")), &[], Duration::from_secs(30), 1000));
    assert!(o.status_lost && !o.ok(), "{o:?}");
}

#[test]
fn a_command_that_runs_too_long_still_times_out() {
    let o = crate::wsexec::run_argv(&std::env::temp_dir(), &sh("sleep 5 # gone-ws-3"), &[], Duration::from_millis(150), 1000);
    assert!(o.timed_out && !o.status_lost && !o.ok());
}

#[test]
fn wait_for_end_reports_a_normal_exit_code() {
    let mut c = Command::new("sh").args(["-c", "exit 4"]).spawn().unwrap();
    assert_eq!(crate::wsexec::wait_for_end(&mut c, Duration::from_secs(5)), (Some(4), false, false));
}

#[cfg(target_os = "linux")]
#[test]
fn run_capture_returns_the_output_of_a_child_whose_status_was_taken() {
    let r = stolen("gone-cap-1", |_, tail| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &format!("echo captured; {tail}")]);
        crate::voice::run_capture(cmd, None, Duration::from_secs(30))
    });
    let (out, _) = r.expect("a lost status is not an error");
    assert_eq!(String::from_utf8_lossy(&out).trim(), "captured");
}

#[test]
fn the_player_and_media_wrappers_see_a_taken_status_as_not_running() {
    let c = Command::new("true").spawn().unwrap();
    steal_status(c.id());
    let mut run = crate::media::ChildRun(c);
    assert!(!crate::media::Running::alive(&mut run));
    // the same for the recorder-style check
    let mut c2 = Command::new("true").stdout(Stdio::null()).spawn().unwrap();
    steal_status(c2.id());
    assert!(iyakki_core_api::child_status(&mut c2) == ChildState::Gone);
    let t = Instant::now();
    crate::voice::kill_child(&mut c2); // the reap of a child that is already gone must not hang or panic
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(crate::voice::player_failure(&mut c2), "");
}

#[cfg(target_os = "linux")]
#[test]
fn the_cli_runner_keeps_the_output_of_a_program_whose_status_was_taken_and_still_fails_a_real_error() {
    let r = stolen("gone-clis-1", |_, tail| crate::clis::run_quiet("sh", &["-c", &format!("echo v1.2.3; {tail}")], 30));
    assert_eq!(r.unwrap(), "v1.2.3\n");
    let r = stolen("gone-clis-2", |_, tail| crate::clis::run_quiet("sh", &["-c", &format!("echo boom >&2; {tail}")], 30));
    assert!(r.unwrap_err().contains("boom"));
    assert_eq!(crate::clis::run_quiet("sh", &["-c", "exit 3"], 10).unwrap_err(), "sh failed: no message");
    assert!(crate::clis::run_quiet("sh", &["-c", "sleep 5"], 1).unwrap_err().contains("did not finish"));
}

#[cfg(target_os = "linux")]
#[test]
fn kubectl_whose_status_was_taken_gives_its_answer_unless_it_wrote_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::testdir::unique("iyakki-kube-gone", "k");
    std::fs::create_dir_all(&dir).unwrap();
    let mk = |name: &str, body: &str| {
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    };
    let r = stolen("gone-kube-1", |marker, tail| {
        let ok = crate::sandbox::kube::Kubectl { program: mk(marker, &format!("echo answer; {tail}")), kubeconfig: None, timeout: Duration::from_secs(30) };
        ok.run(&["get", "pods"], None, 1000)
    });
    assert_eq!(String::from_utf8_lossy(&r.expect("a lost status with an answer is not an error")).trim(), "answer");
    let r = stolen("gone-kube-2", |marker, tail| {
        let bad = crate::sandbox::kube::Kubectl { program: mk(marker, &format!("echo boom >&2; {tail}")), kubeconfig: None, timeout: Duration::from_secs(30) };
        bad.run(&["get", "pods"], None, 1000)
    });
    assert!(r.is_err(), "an error on standard error is still a failure");
    let _ = std::fs::remove_dir_all(&dir);
}
