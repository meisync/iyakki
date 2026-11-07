//! Power settings with a made-up /sys: the profile writes, what each kind of processor offers, the limits in words, and the idle rules.

use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-power-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{v}\n")).unwrap();
}

fn get(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap().trim().to_string()
}

/// An Intel-style machine: two policies, governors performance/powersave, an energy preference.
fn pstate(tag: &str) -> PathBuf {
    let d = dir(tag);
    for n in 0..2 {
        let p = format!("devices/system/cpu/cpufreq/policy{n}");
        put(&d, &format!("{p}/scaling_governor"), "powersave");
        put(&d, &format!("{p}/scaling_available_governors"), "performance powersave");
        put(&d, &format!("{p}/energy_performance_preference"), "balance_performance");
        put(&d, &format!("{p}/energy_performance_available_preferences"), "default performance balance_performance balance_power power");
    }
    put(&d, "power/state", "freeze mem disk");
    d
}

#[test]
fn each_profile_sets_the_governor_and_the_energy_preference_on_every_policy_and_reads_back() {
    let d = pstate("profiles");
    let env = PowerEnv::at(&d);
    assert_eq!(current(&env), Some(Profile::Balanced));
    for (prof, gov, epp) in [(Profile::Performance, "performance", "performance"), (Profile::PowerSaver, "powersave", "power"), (Profile::Balanced, "powersave", "balance_performance")] {
        assert_eq!(set_profile(&env, prof).unwrap(), 4, "{prof:?}");
        for n in 0..2 {
            assert_eq!(get(&d, &format!("devices/system/cpu/cpufreq/policy{n}/scaling_governor")), gov);
            assert_eq!(get(&d, &format!("devices/system/cpu/cpufreq/policy{n}/energy_performance_preference")), epp);
        }
        assert_eq!(current(&env), Some(prof));
    }
}

#[test]
fn a_processor_without_an_energy_preference_uses_the_governors_it_has() {
    let d = dir("acpi");
    put(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor", "ondemand");
    put(&d, "devices/system/cpu/cpufreq/policy0/scaling_available_governors", "conservative ondemand powersave performance");
    let env = PowerEnv::at(&d);
    let g = |p| {
        set_profile(&env, p).unwrap();
        get(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor")
    };
    assert_eq!((g(Profile::Performance).as_str(), g(Profile::PowerSaver).as_str(), g(Profile::Balanced).as_str()), ("performance", "powersave", "ondemand"));
    assert!(!d.join("devices/system/cpu/cpufreq/policy0/energy_performance_preference").exists(), "nothing is invented");
    assert_eq!(current(&env), Some(Profile::Balanced));
}

#[test]
fn a_machine_without_cpu_frequency_control_says_so_and_writes_nothing() {
    let d = dir("vm");
    let env = PowerEnv::at(&d);
    let e = set_profile(&env, Profile::PowerSaver).unwrap_err();
    assert!(e.contains("does not let IYAKKI change the processor's speed"), "{e}");
    assert_eq!(current(&env), None);
    assert!(!caps(&env).profiles);
    assert!(limits(&caps(&env)).iter().any(|l| l.starts_with("Power profile: not available")));
    // a governor the processor does not list is never written
    put(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor", "userspace");
    put(&d, "devices/system/cpu/cpufreq/policy0/scaling_available_governors", "userspace");
    assert!(plan(&env, Profile::Performance).unwrap_err().contains("offers no setting"));
    assert_eq!(get(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor"), "userspace");
}

#[test]
fn a_file_the_kernel_will_not_take_is_reported_in_words() {
    let d = pstate("readonly");
    let env = PowerEnv::at(&d);
    // a folder where the file should be cannot be written
    let f = d.join("devices/system/cpu/cpufreq/policy1/energy_performance_preference");
    std::fs::remove_file(&f).unwrap();
    std::fs::create_dir(&f).unwrap();
    let e = set_profile(&env, Profile::Performance).unwrap_err();
    assert!(e.contains("could not be changed") && e.contains("energy_performance_preference"), "{e}");
}

#[test]
fn what_the_machine_can_do_is_read_from_sys() {
    let d = pstate("caps");
    let c = caps(&PowerEnv::at(&d));
    assert_eq!(c, Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: false });
    put(&d, "class/dmi/id/sys_vendor", "Apple Inc.");
    put(&d, "class/dmi/id/product_name", "Macmini8,1");
    assert!(caps(&PowerEnv::at(&d)).t2);
    put(&d, "class/dmi/id/product_name", "Macmini7,1");
    assert!(!caps(&PowerEnv::at(&d)).t2, "an older Mac mini has no T2");
    put(&d, "power/state", "disk");
    assert!(!caps(&PowerEnv::at(&d)).sleep, "hibernate alone is not sleep");
    let lim = limits(&caps(&PowerEnv::at(&d)));
    assert!(lim.iter().any(|l| l.starts_with("Sleep: not available")));
    assert!(lim.iter().any(|l| l.starts_with("Lid and power button: not supported")), "said plainly, always");
    assert!(!caps(&PowerEnv::at(&d)).lid_and_button());
}

#[test]
fn sleep_after_idle_is_off_by_default_waits_for_the_time_and_for_work_to_end() {
    let c = Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: true };
    let s = PowerSettings::default();
    assert_eq!((s.sleep_after_min, s.profile), (0, Profile::Balanced));
    assert_eq!(idle_action(&s, &c, 99_999, &[]), Idle::Nothing, "off by default");
    let s = PowerSettings { sleep_after_min: 10, ..Default::default() };
    assert_eq!(idle_action(&s, &c, 599, &[]), Idle::Nothing);
    assert_eq!(idle_action(&s, &c, 600, &[]), Idle::Sleep);
    let busy = vec!["an install".to_string()];
    assert_eq!(idle_action(&s, &c, 6000, &busy), Idle::Nothing, "never while something runs");
    assert_eq!(idle_action(&PowerSettings { sleep_when_busy: true, ..s.clone() }, &c, 6000, &busy), Idle::Sleep, "unless the person said so");
    assert_eq!(idle_action(&s, &Caps { sleep: false, ..c.clone() }, 6000, &[]), Idle::Nothing, "no sleep state, no sleep");
}

#[test]
fn on_a_t2_mac_the_safe_fallback_is_the_power_saver_profile_unless_the_experiment_is_on() {
    let t2 = Caps { profiles: true, energy_preference: true, sleep: true, t2: true, laptop: false };
    let s = PowerSettings { sleep_after_min: 5, ..Default::default() };
    assert_eq!(idle_action(&s, &t2, 300, &[]), Idle::SaverProfile);
    assert_eq!(idle_action(&PowerSettings { sleep_experimental: true, ..s.clone() }, &t2, 300, &[]), Idle::Sleep);
    assert_eq!(idle_action(&s, &t2, 300, &["a job".into()]), Idle::Nothing, "the slow profile does not start while work runs");
    assert_eq!(idle_action(&s, &Caps { profiles: false, ..t2 }, 300, &[]), Idle::Nothing);
}

#[test]
fn the_choices_cycle_and_an_old_settings_file_means_the_defaults() {
    let mut s = PowerSettings::default();
    let mut seen = vec![s.sleep_after_min];
    for _ in 0..SLEEP_CHOICES.len() {
        s.sleep_after_min = s.next_sleep();
        seen.push(s.sleep_after_min);
    }
    assert_eq!(seen, [0, 5, 10, 15, 30, 60, 0]);
    s.sleep_after_min = 7;
    assert_eq!(s.next_sleep(), 0, "an odd value from a hand-edited file starts the cycle again");
    let back: PowerSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(back, PowerSettings::default());
    let j = serde_json::to_string(&PowerSettings { profile: Profile::PowerSaver, ..Default::default() }).unwrap();
    assert!(j.contains("\"power_saver\""), "{j}");
}

#[test]
fn the_saved_profile_is_applied_once_and_left_alone_when_already_set() {
    let d = pstate("saved");
    let env = PowerEnv::at(&d);
    let s = PowerSettings { profile: Profile::PowerSaver, ..Default::default() };
    assert!(apply_saved(&env, &s).unwrap().contains("set (4 values)"));
    assert!(apply_saved(&env, &s).unwrap().contains("already set"));
    assert!(apply_saved(&PowerEnv::at(dir("saved-vm")), &s).is_err());
}
