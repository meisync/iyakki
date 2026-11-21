//! F-SPLIT-06: the real exec, process to process, with the vault keys handed over in a memory file and read by the new program.
//!
//! No booted device is needed: the test binary plays both programs. The parent test starts it as the OLD program; the old program signs a made-up
//! person in over a made-up data folder, does what the core's before-exec hook does (packs the keys into a memory file, keeps the descriptor open
//! over the exec, names it in `IYAKKI_LIVE_KEYS_FD`) and calls `liverestart::perform` with the real exec of the same binary, now the NEW program.
//! The new program keeps the process id, finds the restart state (`begin`), reads the descriptor, opens a second `Services` over the same files
//! and imports the keys, reads the person's secret without a password, adopts the child that was running, reports healthy (which lets the
//! descriptor go) and writes what it saw. The parent checks that report, and that no key is on any disk. Linux only; no ports, no sleeps.
#![cfg(all(target_os = "linux", feature = "headless"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_core::liverestart::{self, Paths};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn env_path(k: &str) -> PathBuf {
    PathBuf::from(std::env::var(k).unwrap_or_else(|_| panic!("{k} not set")))
}

fn paths_in(base: &Path) -> Paths {
    Paths { dir: base.join("lr"), proc_root: "/proc".into(), flag: base.join("lr.flag") }
}

/// The OLD program: runs only when the parent test started this binary with the role set.
#[test]
fn old_program_entry() {
    if std::env::var("IYAKKI_LRX_ROLE").as_deref() != Ok("old") {
        return;
    }
    let base = env_path("IYAKKI_LRX_BASE");
    let svc = iyakki_agent::services::Services::open(&base.join("data"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    svc.on_login("alice", "alice-pass-1", false);
    svc.user_set("alice", "token", "secret-value").unwrap_or_else(|e| panic!("{e:?}"));
    std::fs::write(base.join("pid-before"), std::process::id().to_string()).unwrap();
    // a child of the old program that must still be its child after the exec
    let sleeper = std::process::Command::new("sleep").arg("30").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    std::fs::write(base.join("child-pid"), sleeper.id().to_string()).unwrap();
    std::env::set_var("IYAKKI_LRX_CHILDPID", sleeper.id().to_string()); // flaky-ok: runs only in the child process the parent test started; no other test shares this environment
    // the before-exec hook of the core: keys into a memory file that survives the exec, the descriptor named in the environment
    let packed = iyakki_agent::livekeys::pack(&svc);
    let fd = liverestart::hand_over("session-keys", &packed).unwrap();
    liverestart::keep_across_exec(fd).unwrap();
    std::env::set_var("IYAKKI_LIVE_KEYS_FD", fd.to_string()); // flaky-ok: runs only in the child process the parent test started; no other test shares this environment
    std::env::set_var("IYAKKI_LRX_ROLE", "new"); // flaky-ok: runs only in the child process the parent test started; no other test shares this environment
    let me = std::env::current_exe().unwrap();
    let args: Vec<String> = ["--exact", "new_program_entry", "--nocapture", "--test-threads=1"].iter().map(|s| s.to_string()).collect();
    let said = liverestart::perform(&paths_in(&base), &me, &me, "old", 1_000, vec![fd], &args, &liverestart::RealExec);
    // only reached when the exec failed
    std::fs::write(base.join("exec-failed"), said).unwrap();
}

/// The NEW program: the same process after the exec.
#[test]
fn new_program_entry() {
    if std::env::var("IYAKKI_LRX_ROLE").as_deref() != Ok("new") {
        return;
    }
    let base = env_path("IYAKKI_LRX_BASE");
    let p = paths_in(&base);
    let started = liverestart::begin(&p);
    let mut report = vec![format!("pid={}", std::process::id()), format!("restart_state_found={}", started.is_some())];
    if let Some(s) = &started {
        report.push(format!("child_adopted={}", s.adoption.kept.iter().any(|c| c.pid.to_string() == std::env::var("IYAKKI_LRX_CHILDPID").unwrap_or_default())));
        report.push(format!("from_version={}", s.state.from_version));
    }
    let fd: i32 = std::env::var("IYAKKI_LIVE_KEYS_FD").ok().and_then(|v| v.parse().ok()).unwrap_or(-1);
    let bytes = liverestart::take_over(fd).unwrap_or_default();
    let second = iyakki_agent::services::Services::open(&base.join("data"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    report.push(format!("locked_before_import={}", !second.user_vault_unlocked("alice")));
    let imported = second.import_session_keys(&iyakki_agent::livekeys::unpack(&bytes));
    report.push(format!("imported={imported}"));
    report.push(format!("unlocked_after={}", second.user_vault_unlocked("alice")));
    report.push(format!("secret={}", second.user_get("alice", "token").map(|s| s.to_string()).unwrap_or_default()));
    liverestart::mark_healthy(&p).unwrap();
    // marking healthy lets the descriptor go
    // SAFETY: fcntl(F_GETFD) on a number only reads flags and reports EBADF for a closed descriptor.
    report.push(format!("fd_closed_after_healthy={}", unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0));
    std::fs::write(base.join("report"), report.join("\n")).unwrap();
}

fn files_under(d: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { files_under(&p, out) } else { out.push(p) }
    }
}

#[test]
fn a_new_program_started_by_the_old_one_with_a_real_exec_keeps_the_pid_adopts_the_child_and_opens_the_vault_with_the_handed_over_keys() {
    if std::env::var("IYAKKI_LRX_ROLE").is_ok() {
        return; // inside one of the two programs: only the entries above run
    }
    let base = iyakki_agent::testdir::unique("live-restart-exec", "a");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("lr")).unwrap();
    std::fs::write(base.join("lr.flag"), "on").unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "old_program_entry", "--nocapture", "--test-threads=1"])
        .env("IYAKKI_LRX_ROLE", "old")
        .env("IYAKKI_LRX_BASE", &base)
        .output()
        .unwrap();
    // the adopted child outlives both programs' tests: stop it
    if let Ok(pid) = std::fs::read_to_string(base.join("child-pid")).unwrap_or_default().trim().parse::<i32>() {
        // SAFETY: kill(2) on the pid of the sleep process the old program started for this test.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    let said = |f: &str| std::fs::read_to_string(base.join(f)).unwrap_or_default();
    assert!(said("exec-failed").is_empty(), "the exec failed: {}", said("exec-failed"));
    let report = said("report");
    assert!(!report.is_empty(), "no report; the programs said: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let line = |k: &str| report.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or_else(|| panic!("no {k} in {report}")).to_string();
    assert_eq!(line("pid"), said("pid-before").trim(), "an exec keeps the process id");
    assert_eq!(line("restart_state_found"), "true");
    assert_eq!(line("from_version"), "old");
    assert_eq!(line("child_adopted"), "true", "the child that was running before the exec is adopted by pid");
    assert_eq!(line("locked_before_import"), "true", "a new program with nothing handed over starts locked");
    assert_eq!(line("imported"), "1");
    assert_eq!(line("unlocked_after"), "true");
    assert_eq!(line("secret"), "secret-value", "the new program reads the person's secret without a password");
    assert_eq!(line("fd_closed_after_healthy"), "true");
    // only the memory file carried the keys: the files alone do not open the vault
    let mut all = Vec::new();
    files_under(&base, &mut all);
    assert!(!all.is_empty());
    let svc = iyakki_agent::services::Services::open(&base.join("data"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    assert!(!svc.user_vault_unlocked("alice"), "the files alone do not open the vault");
    let _ = std::fs::remove_dir_all(&base);
}
