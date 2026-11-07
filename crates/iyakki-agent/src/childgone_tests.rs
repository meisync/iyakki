//! The PID-1 reaper (`waitpid(-1)`) can take the exit status of a child that other code watches; `Child::try_wait()` then
//! returns `Err(ECHILD)`. These tests take the status behind the code's back and check that a dead child is always seen, that
//! its output is kept and that nothing hangs. See docs/development/PID1-REAPER.md.

use iyakki_core_api::{steal_status, steal_status_of_marked, ChildState};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sh(script: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script.into()]
}

#[test]
fn a_command_whose_status_was_taken_keeps_its_output_and_is_not_a_failure() {
    let thief = steal_status_of_marked("gone-ws-1", Duration::from_secs(5));
    let o = crate::wsexec::run_argv(&std::env::temp_dir(), &sh("echo out; sleep 0.3 # gone-ws-1"), &[], Duration::from_secs(10), 1000);
    assert_ne!(thief.join().unwrap(), 0, "the test could not take the status");
    assert_eq!(o.stdout, "out\n");
    assert!(o.status_lost && o.code.is_none() && !o.timed_out, "{o:?}");
    assert!(o.ok(), "a kubectl whose status was taken is not a failure: {o:?}");
}

#[test]
fn a_command_that_wrote_to_stderr_and_lost_its_status_is_still_a_failure() {
    let thief = steal_status_of_marked("gone-ws-2", Duration::from_secs(5));
    let o = crate::wsexec::run_argv(&std::env::temp_dir(), &sh("echo boom >&2; sleep 0.3 # gone-ws-2"), &[], Duration::from_secs(10), 1000);
    thief.join().unwrap();
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

#[test]
fn run_capture_returns_the_output_of_a_child_whose_status_was_taken() {
    let thief = steal_status_of_marked("gone-cap-1", Duration::from_secs(5));
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo captured; sleep 0.3 # gone-cap-1"]);
    let r = crate::voice::run_capture(cmd, None, Duration::from_secs(10));
    assert_ne!(thief.join().unwrap(), 0);
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

#[test]
fn the_cli_runner_keeps_the_output_of_a_program_whose_status_was_taken_and_still_fails_a_real_error() {
    let thief = steal_status_of_marked("gone-clis-1", Duration::from_secs(5));
    let r = crate::clis::run_quiet("sh", &["-c", "echo v1.2.3; sleep 0.3 # gone-clis-1"], 10);
    assert_ne!(thief.join().unwrap(), 0, "the test could not take the status");
    assert_eq!(r.unwrap(), "v1.2.3\n");
    let thief = steal_status_of_marked("gone-clis-2", Duration::from_secs(5));
    let r = crate::clis::run_quiet("sh", &["-c", "echo boom >&2; sleep 0.3 # gone-clis-2"], 10);
    thief.join().unwrap();
    assert!(r.unwrap_err().contains("boom"));
    assert_eq!(crate::clis::run_quiet("sh", &["-c", "exit 3"], 10).unwrap_err(), "sh failed: no message");
    assert!(crate::clis::run_quiet("sh", &["-c", "sleep 5"], 1).unwrap_err().contains("did not finish"));
}
