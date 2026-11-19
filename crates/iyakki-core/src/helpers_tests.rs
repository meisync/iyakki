//! Real child processes (`sh`) in temp folders: the supervisor, the pause between starts, the give-up rule, the stop, the audit lines
//! and the reconciler. Nothing here touches the network, the firewall or `/sys`.

use super::*;

fn sh(name: &str, script: &str) -> Spec {
    Spec { name: name.into(), program: "sh".into(), args: vec!["-c".into(), script.into()] }
}

fn fast() -> Policy {
    Policy { first_pause: Duration::from_millis(20), max_pause: Duration::from_millis(80), healthy_after: Duration::from_secs(30), max_rapid: 4, grace: Duration::from_millis(300) }
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

fn count(log: &Mutex<Vec<String>>, what: &str) -> usize {
    log.lock().unwrap().iter().filter(|l| l.starts_with(what)).count()
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-helpers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_helper_that_exits_is_started_again_and_every_step_is_audited() {
    let (audit, log) = recorder();
    let h = supervise(sh("flaky", "printf 'AUDIT\tdemo.event\thello\n'; exit 3"), Policy { max_rapid: 100, ..fast() }, audit);
    assert!(wait(|| count(&log, "service.start") >= 3), "restarted");
    assert!(wait(|| count(&log, "demo.event|hello") >= 3), "the helper's own AUDIT lines are forwarded");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("service.exit|flaky:") && l.contains('3')));
    let end = h.stop();
    assert!(matches!(end, State::Stopped { starts } if starts >= 3), "{end:?}");
}

#[test]
fn a_helper_that_dies_at_once_again_and_again_is_given_up_on() {
    let (audit, log) = recorder();
    let h = supervise(sh("broken", "exit 1"), fast(), audit);
    assert!(wait(|| matches!(h.state(), State::GaveUp { .. })));
    assert_eq!(count(&log, "service.start"), 4);
    assert_eq!(count(&log, "service.gave-up"), 1);
    // a program that does not exist is given up on too, without a panic
    let (audit2, log2) = recorder();
    let h2 = supervise(Spec { name: "nope".into(), program: "/nonexistent/iyakki-helper".into(), args: vec![] }, fast(), audit2);
    assert!(wait(|| matches!(h2.state(), State::GaveUp { starts: 0 })));
    assert!(count(&log2, "service.start-failed") >= 1);
}

#[test]
fn stopping_ends_a_running_helper_with_sigterm_and_kills_one_that_ignores_it() {
    let (audit, log) = recorder();
    let h = supervise(sh("sleeper", "exec sleep 600"), fast(), audit);
    assert!(wait(|| matches!(h.state(), State::Running { .. })));
    let State::Running { pid, .. } = h.state() else { unreachable!() };
    assert!(h.stop() == State::Stopped { starts: 1 });
    // the process is really gone
    // SAFETY: signal 0 only tests whether the pid exists.
    assert!(wait(|| unsafe { libc::kill(pid as i32, 0) } != 0), "the child is gone");
    assert!(log.lock().unwrap().iter().any(|l| l.starts_with("service.stop|sleeper")));
    // a helper that ignores SIGTERM is killed after the grace period
    let (audit, log) = recorder();
    let h = supervise(sh("stubborn", "trap '' TERM; while true; do sleep 0.05; done"), fast(), audit);
    assert!(wait(|| matches!(h.state(), State::Running { .. })));
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    h.stop();
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(log.lock().unwrap().iter().any(|l| l.contains("killed after the grace period")), "{:?}", log.lock().unwrap());
}

#[test]
fn a_helper_that_ran_long_enough_starts_the_failure_count_over() {
    let (audit, log) = recorder();
    // healthy after 150 ms: it lives 250 ms each time, so it is never "rapid" and is never given up on
    let p = Policy { healthy_after: Duration::from_millis(150), max_rapid: 2, ..fast() };
    let h = supervise(sh("steady", "sleep 0.25"), p, audit);
    assert!(wait(|| count(&log, "service.start") >= 4));
    assert!(matches!(h.state(), State::Running { .. } | State::Waiting { .. }));
    assert_eq!(count(&log, "service.gave-up"), 0);
}

#[test]
fn settings_decide_what_is_started_and_the_reconciler_follows_them() {
    let data = temp("wanted");
    let exe = PathBuf::from("/opt/iyakki");
    assert!(wanted(&exe, "/sys", &data, 1000).is_empty(), "nothing is started unless a setting is on");
    iyakki_drivers::fanservice::save(&data, &iyakki_drivers::fanservice::Setting { enabled: true, ..Default::default() }).unwrap();
    let v = wanted(&exe, "/sys", &data, 1000);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].args, ["fan-service", "--sys", "/sys", "--data", data.to_str().unwrap()]);
    #[cfg(feature = "headless")]
    {
        let vpn = data.join("vpn/ada");
        iyakki_agent::vpn_helper::write_setting(&vpn, &iyakki_agent::vpn_helper::HelperSetting { enabled: true, socket: "/run/x/h.sock".into(), allow_uid: None }).unwrap();
        let other = data.join("vpn/bob");
        iyakki_agent::vpn_helper::write_setting(&other, &iyakki_agent::vpn_helper::HelperSetting { enabled: true, socket: "/run/x/h.sock".into(), allow_uid: Some(7) }).unwrap();
        let v = wanted(&exe, "/sys", &data, 1000);
        assert_eq!(v.len(), 2, "one helper per socket");
        assert_eq!(v[1].args, ["vpn-helper", "--allow-uid", "1000", "--socket", "/run/x/h.sock"], "no uid in the setting means the core's own");
    }
    // the reconciler starts what is wanted, leaves it alone while it is unchanged, and stops it when it is no longer wanted
    let (audit, log) = recorder();
    let mut r = Reconciler::new(fast(), audit);
    let a = sh("a", "exec sleep 600");
    r.tick(vec![a.clone()]);
    assert!(wait(|| matches!(r.states()[0].1, State::Running { .. })));
    let State::Running { pid, .. } = r.states()[0].1.clone() else { unreachable!() };
    r.tick(vec![a.clone()]);
    assert!(matches!(r.states()[0].1, State::Running { pid: p, starts: 1 } if p == pid), "unchanged: not restarted");
    r.tick(vec![]);
    assert!(r.states().is_empty());
    assert!(count(&log, "service.stop") == 1);
    r.tick(vec![a]);
    r.stop_all();
    assert!(r.states().is_empty());
    let _ = std::fs::remove_dir_all(&data);
}
