//! The real `iyakki` program as the supervised helpers init starts: `fan-service` over a fake sysfs tree and `vpn-helper` on a
//! Unix socket in a temp folder, run by the real supervisor (F-DRIVERS-14, F-VPN-05). Nothing here touches this computer's `/sys`,
//! network or firewall: the fan tree is a folder of files, and the VPN helper is only asked a line of garbage (which it refuses).

use iyakki_core::helpers::{fan_spec, supervise, vpn_helper_spec, Audit, Policy, State};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_iyakki"))
}

fn policy() -> Policy {
    Policy { first_pause: Duration::from_millis(50), max_pause: Duration::from_millis(200), healthy_after: Duration::from_secs(30), max_rapid: 5, grace: Duration::from_secs(5) }
}

fn recorder() -> (Audit, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    (Arc::new(move |e, d| l.lock().unwrap().push(format!("{e}|{d}"))), log)
}

fn wait(mut f: impl FnMut() -> bool) -> bool {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn pid_of(s: State) -> Option<u32> {
    if let State::Running { pid, .. } = s { Some(pid) } else { None }
}

fn kill(pid: u32, sig: i32) {
    // SAFETY: a signal to a child of this test.
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-hreal-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fan_tree(root: &Path) -> PathBuf {
    let sys = root.join("sys");
    let smc = sys.join("class/hwmon/hwmon2");
    std::fs::create_dir_all(&smc).unwrap();
    std::fs::write(smc.join("name"), "applesmc\n").unwrap();
    for (k, v) in [("fan1_min", "1200"), ("fan1_max", "6200"), ("fan1_manual", "0"), ("fan1_output", "1200"), ("fan1_input", "1210")] {
        std::fs::write(smc.join(k), v).unwrap();
    }
    let ct = sys.join("class/hwmon/hwmon3");
    std::fs::create_dir_all(&ct).unwrap();
    std::fs::write(ct.join("name"), "coretemp\n").unwrap();
    std::fs::write(ct.join("temp1_input"), "80000").unwrap();
    sys
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

#[test]
fn the_fan_service_runs_as_a_supervised_child_audits_what_it_does_and_gives_the_fans_back() {
    let root = temp("fan");
    let sys = fan_tree(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let manual = sys.join("class/hwmon/hwmon2/fan1_manual");
    let output = sys.join("class/hwmon/hwmon2/fan1_output");
    let mut spec = fan_spec(exe(), sys.to_str().unwrap(), data.to_str().unwrap());
    spec.args.extend(["--interval-ms".into(), "50".into()]);
    let (audit, log) = recorder();
    let h = supervise(spec, policy(), audit);
    assert!(wait(|| pid_of(h.state()).is_some()));
    // the setting is off: the real process leaves the fans alone
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((read(&manual), read(&output)), ("0".into(), "1200".into()));
    // the person turns it on (the same file the chat tool writes)
    iyakki_drivers::fanservice::save(&data, &iyakki_drivers::fanservice::Setting { enabled: true, ..Default::default() }).unwrap();
    assert!(wait(|| read(&manual) == "1"), "taken over");
    // the child writes the speed a moment after the manual flag: wait for it (a single read raced under load)
    assert!(wait(|| read(&output).parse::<u32>().is_ok_and(|v| v > 1200)), "the fan speed was raised: {:?}", read(&output));
    assert!(wait(|| log.lock().unwrap().iter().any(|l| l.starts_with("fan.take-over|"))), "the child's AUDIT line reached the core's audit: {:?}", log.lock().unwrap());
    // the child writes its status a moment after taking the fans over: wait for it
    assert!(wait(|| iyakki_drivers::fanservice::status(&data).contains("controlled by IYAKKI")), "status: {}", iyakki_drivers::fanservice::status(&data));
    // the child is killed hard: the supervisor starts a new one, which takes control again
    let first = pid_of(h.state()).unwrap();
    kill(first, libc::SIGKILL);
    assert!(wait(|| pid_of(h.state()).is_some_and(|p| p != first)), "restarted");
    assert!(wait(|| log.lock().unwrap().iter().filter(|l| l.starts_with("fan.take-over|")).count() >= 2));
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("service.exit|fan:")));
    // stopping (SIGTERM) hands the fans back before the child ends
    let end = h.stop();
    assert!(matches!(end, State::Stopped { starts } if starts >= 2), "{end:?}");
    assert_eq!(read(&manual), "0", "the fans are the firmware's again");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("fan.release|") && l.contains("service stopped")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_vpn_helper_runs_as_a_supervised_child_serves_its_socket_for_one_user_and_cleans_up() {
    let dir = std::path::Path::new("/tmp").join(format!("ihx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("h.sock");
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let (audit, log) = recorder();
    let h = supervise(vpn_helper_spec(exe(), uid, sock.to_str().unwrap()), policy(), audit);
    // a line of garbage is refused by the real helper (nothing is run)
    let ask = |sock: &Path| -> Option<String> {
        let mut s = UnixStream::connect(sock).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        s.write_all(b"this is not json\n").ok()?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).ok()?;
        Some(line)
    };
    assert!(wait(|| sock.exists() && ask(&sock).is_some()), "the helper listens");
    let answer = ask(&sock).unwrap();
    assert!(answer.contains("error") || answer.contains("\"ok\":false") || answer.contains("false"), "{answer}");
    assert_eq!(std::fs::metadata(&sock).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap(), 0o600);
    // killed hard: the stale socket is left behind, the supervisor starts a new helper which replaces it
    let first = pid_of(h.state()).unwrap();
    kill(first, libc::SIGKILL);
    assert!(wait(|| pid_of(h.state()).is_some_and(|p| p != first)), "restarted");
    assert!(wait(|| ask(&sock).is_some()), "the new helper serves the same path");
    assert!(log.lock().unwrap().iter().filter(|l| l.starts_with("service.start|")).count() >= 2);
    // stopped: it ends cleanly and removes its socket
    h.stop();
    assert!(!sock.exists(), "the socket is removed");
    // a helper allowed for another user id answers nobody here: the connection is closed without an answer
    let (audit, _) = recorder();
    let h = supervise(vpn_helper_spec(exe(), uid.wrapping_add(4242), sock.to_str().unwrap()), policy(), audit);
    assert!(wait(|| sock.exists()));
    assert!(ask(&sock).is_none_or(|l| l.is_empty()), "a refused peer gets no answer");
    h.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
