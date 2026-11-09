//! Sleep after idle, the T2 fall-back and the daily update check in the shell, with a fake clock and a made-up /sys: it sleeps once at
//! the chosen time, never while something runs, the power saver profile on a T2 Mac comes and goes with the input, and the update check
//! runs when it is due, only as an administrator, and not when it is off.

use super::tests::*;
use super::*;
use crate::apps::tests::MockHost;
use iyakki_agent::power::PowerEnv;
use iyakki_agent::screensaver::{First, SaverConfig, Style};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MIN: u64 = 60_000;

fn put(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{v}\n")).unwrap();
}

fn sys(tag: &str, t2: bool) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-shpower-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let p = "devices/system/cpu/cpufreq/policy0";
    put(&d, &format!("{p}/scaling_governor"), "powersave");
    put(&d, &format!("{p}/scaling_available_governors"), "performance powersave");
    put(&d, &format!("{p}/energy_performance_preference"), "balance_performance");
    put(&d, &format!("{p}/energy_performance_available_preferences"), "default performance balance_performance balance_power power");
    put(&d, "power/state", "freeze mem");
    if t2 {
        put(&d, "class/dmi/id/sys_vendor", "Apple Inc.");
        put(&d, "class/dmi/id/product_name", "Macmini8,1");
    }
    d
}

fn epp(d: &Path) -> String {
    std::fs::read_to_string(d.join("devices/system/cpu/cpufreq/policy0/energy_performance_preference")).unwrap().trim().to_string()
}

/// A signed-in shell with a fake clock, no screen saver of its own, and the sleep choice `minutes`.
fn shell(tag: &str, minutes: u32, t2: bool) -> (Shell, Arc<MockHost>, Arc<AtomicU64>, PathBuf) {
    let (mut sh, host) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    sh.saver_config_for_test(SaverConfig { first: First::Nothing, after_min: 5, then_off_min: 5, battery_shorter: true, style: Style::Logo, dim_pct: 0 });
    let d = sys(tag, t2);
    sh.power_env_for_test(PowerEnv::at(&d));
    *host.sleepable.lock().unwrap() = true;
    sh.env.services.settings.lock().unwrap().update(|s| s.power.sleep_after_min = minutes).unwrap();
    (sh, host, clock, d)
}

fn run(sh: &mut Shell, clock: &AtomicU64, ms: u64, every: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(every);
        clock.fetch_add(step, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        left -= step;
    }
}

fn slept(h: &MockHost) -> u32 {
    *h.slept.lock().unwrap() as u32
}

#[test]
fn it_sleeps_once_at_the_chosen_time_and_the_wake_up_counts_as_input() {
    let (mut sh, host, clock, _d) = shell("shp-sleep", 5, false);
    run(&mut sh, &clock, 4 * MIN, 250);
    assert_eq!(slept(&host), 0, "not yet");
    run(&mut sh, &clock, 2 * MIN, 250);
    assert_eq!(slept(&host), 1, "after five minutes");
    assert!(sh.session.is_some() && sh.is_locked(), "the screen was locked first (apps stay open)");
    run(&mut sh, &clock, 2 * MIN, 250);
    assert_eq!(slept(&host), 1, "the wake-up counted as input, so it does not fall asleep again at once");
}

#[test]
fn sleep_is_off_by_default_and_when_the_machine_cannot() {
    let (mut sh, host, clock, _d) = shell("shp-off", 0, false);
    run(&mut sh, &clock, 90 * MIN, 1000);
    assert_eq!(slept(&host), 0, "off by default");
    let (mut sh, host, clock, _d) = shell("shp-cannot", 5, false);
    *host.sleepable.lock().unwrap() = false;
    run(&mut sh, &clock, 20 * MIN, 1000);
    assert_eq!(slept(&host), 0, "the host cannot sleep: nothing happens");
}

#[test]
fn it_never_sleeps_while_an_update_is_being_written_unless_the_person_said_so() {
    let (mut sh, host, clock, _d) = shell("shp-busy", 5, false);
    let marker = br#"{"from":"0.1.0-alpha.1","to":"0.1.0-alpha.2","started":1,"prev_id":"x","files":["boot/initrd"]}"#;
    let dir = sh.env.services.dir().join("boot-next");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("UPDATING.json"), marker).unwrap();
    assert_eq!(sh.power_busy(), ["a system update"]);
    run(&mut sh, &clock, 30 * MIN, 1000);
    assert_eq!(slept(&host), 0, "busy");
    sh.env.services.settings.lock().unwrap().update(|s| s.power.sleep_when_busy = true).unwrap();
    run(&mut sh, &clock, MIN, 1000);
    assert_eq!(slept(&host), 1, "the person said it may");
}

#[test]
fn on_a_t2_mac_idle_means_the_power_saver_profile_and_the_first_input_brings_the_old_one_back() {
    let (mut sh, host, clock, d) = shell("shp-t2", 5, true);
    assert_eq!(epp(&d), "balance_performance");
    run(&mut sh, &clock, 6 * MIN, 250);
    assert_eq!(slept(&host), 0, "no real sleep unless the experiment is on");
    assert_eq!(epp(&d), "power");
    sh.handle(&Event::Key(Key::Esc));
    run(&mut sh, &clock, 600, 33);
    assert_eq!(epp(&d), "balance_performance", "back as soon as someone uses the computer");
    // with the experiment on it really sleeps
    sh.env.services.settings.lock().unwrap().update(|s| s.power.sleep_experimental = true).unwrap();
    run(&mut sh, &clock, 6 * MIN, 250);
    assert_eq!(slept(&host), 1);
}

#[test]
fn the_saved_profile_is_set_once_when_the_shell_runs() {
    let (mut sh, _host, clock, d) = shell("shp-start", 0, false);
    sh.env.services.settings.lock().unwrap().update(|s| s.power.profile = iyakki_agent::power::Profile::PowerSaver).unwrap();
    run(&mut sh, &clock, 2000, 33);
    assert_eq!(epp(&d), "power");
}

fn wait_for(p: &Path) -> bool {
    for _ in 0..200 {
        if p.is_file() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[test]
fn the_update_check_runs_when_due_as_an_administrator_and_is_quiet_when_off() {
    let (mut sh, _host, clock, _d) = shell("shp-check", 0, false);
    let file = sh.env.services.dir().join(iyakki_agent::store::updatecheck::FILE);
    sh.env.services.settings.lock().unwrap().update(|s| s.update_check.mode = iyakki_agent::store::updatecheck::Mode::Off).unwrap();
    run(&mut sh, &clock, 3000, 33);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!file.exists(), "off means nothing is asked");
    sh.env.services.settings.lock().unwrap().update(|s| s.update_check.mode = iyakki_agent::store::updatecheck::Mode::Daily).unwrap();
    run(&mut sh, &clock, 3000, 33);
    assert!(wait_for(&file), "the first check runs by itself");
    let rep = iyakki_agent::store::updatecheck::Report::load(sh.env.services.dir()).unwrap();
    assert!(rep.checked_at > 0 && rep.updates.is_empty(), "{rep:?}");
    // done today: the next beats do not start another
    let first = rep.checked_at;
    run(&mut sh, &clock, 3000, 33);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(iyakki_agent::store::updatecheck::Report::load(sh.env.services.dir()).unwrap().checked_at, first);
}

#[test]
fn the_update_check_does_not_run_for_an_ordinary_user() {
    let (mut sh, _host, clock, _d) = shell("shp-check-user", 0, false);
    sh.sign_out();
    login(&mut sh, "root", "rootpass12");
    if let Some(s) = sh.session.as_mut() {
        s.policy = iyakki_sec::policy::Role::User.template();
    }
    let file = sh.env.services.dir().join(iyakki_agent::store::updatecheck::FILE);
    run(&mut sh, &clock, 3000, 33);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!file.exists());
}
