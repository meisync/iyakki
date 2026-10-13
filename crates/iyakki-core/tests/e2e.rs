//! End-to-end tests driving the real `iyakki` binary against WASM fixtures in /tests/fixtures.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name)
}

fn iyakki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_iyakki")).args(args).output().unwrap()
}

#[test]
fn runs_hello() {
    let o = iyakki(&["run", fixture("hello.wat").to_str().unwrap()]);
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout), "hello from guest\n");
}

#[test]
fn exit_code_propagates() {
    let o = iyakki(&["run", fixture("exit7.wat").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(7));
}

#[test]
fn infinite_loop_is_stopped_by_fuel() {
    let o = iyakki(&["run", fixture("spin.wat").to_str().unwrap(), "--fuel", "50000"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("fuel"));
}

#[test]
fn filesystem_access_needs_a_grant() {
    let f = fixture("write_ws.wat");
    let denied = iyakki(&["run", f.to_str().unwrap()]);
    assert_eq!(denied.status.code(), Some(0), "denied errno is negative -> clamped to 0 exit; see stderr log");
    assert!(String::from_utf8_lossy(&denied.stderr).contains("result=Some(-2)"));
    let allowed = iyakki(&["run", f.to_str().unwrap(), "--allow-write", "/ws"]);
    assert!(String::from_utf8_lossy(&allowed.stderr).contains("result=Some(0)"));
}

#[test]
fn bad_input_fails_cleanly() {
    assert!(!iyakki(&["run", "/no/such/file.wasm"]).status.success());
    assert_eq!(iyakki(&["bogus-command"]).status.code(), Some(2));
    assert!(iyakki(&["detect"]).status.success());
}

#[test]
fn net_status_lists_loopback() {
    let o = iyakki(&["net", "status"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("Loopback"));
}

/// IYAKKI as PID 1 must survive a console that fails every write (a PC without a serial port, with the console
/// routed to it): `println!`/`eprintln!` would panic, and a panic in PID 1 is a kernel panic.
#[test]
fn a_console_that_rejects_every_write_does_not_crash_iyakki() {
    use std::process::{Command, Stdio};
    let full = || std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("detect").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "detect printed into /dev/full and must still succeed, got {out:?}");
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("hwreport").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "hwreport: {out:?}");
    // the log goes through the same path
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("storage").env("IYAKKI_LOG", "trace").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "storage: {out:?}");
}
