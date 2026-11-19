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
    assert_eq!(c, Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: false, lid: false });
    put(&d, "class/dmi/id/sys_vendor", "Apple Inc.");
    put(&d, "class/dmi/id/product_name", "Macmini8,1");
    assert!(caps(&PowerEnv::at(&d)).t2);
    put(&d, "class/dmi/id/product_name", "Macmini7,1");
    assert!(!caps(&PowerEnv::at(&d)).t2, "an older Mac mini has no T2");
    put(&d, "power/state", "disk");
    assert!(!caps(&PowerEnv::at(&d)).sleep, "hibernate alone is not sleep");
    let lim = limits(&caps(&PowerEnv::at(&d)));
    assert!(lim.iter().any(|l| l.starts_with("Sleep: not available")));
    assert!(!caps(&PowerEnv::at(&d)).lid, "no lid switch on this made-up machine");
    assert!(lim.iter().all(|l| !l.starts_with("Lid and power button: not supported")), "the old 'not supported' line is gone");
}

#[test]
fn sleep_after_idle_is_off_by_default_waits_for_the_time_and_for_work_to_end() {
    let c = Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: true, lid: false };
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
    let t2 = Caps { profiles: true, energy_preference: true, sleep: true, t2: true, laptop: false, lid: false };
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

#[test]
fn the_lid_is_read_from_the_acpi_button_files_and_only_the_closing_moment_acts() {
    let d = pstate("lid");
    let env = PowerEnv::at(&d);
    assert_eq!(lid_state(&env.proc), None, "no lid switch");
    assert!(!caps(&env).lid);
    put(&d, "proc/acpi/button/lid/LID0/state", "state:      open");
    assert_eq!(lid_state(&env.proc), Some(Lid::Open));
    assert!(caps(&env).lid);
    put(&d, "proc/acpi/button/lid/LID0/state", "state:      closed");
    assert_eq!(lid_state(&env.proc), Some(Lid::Closed));
    put(&d, "proc/acpi/button/lid/LID0/state", "garbage");
    assert_eq!(lid_state(&env.proc), None, "an unreadable state is no state");
    let c = Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: true, lid: true };
    let sleep = PowerSettings { lid: LidAction::Sleep, ..Default::default() };
    let lock = PowerSettings { lid: LidAction::Lock, ..Default::default() };
    let (o, x) = (Some(Lid::Open), Some(Lid::Closed));
    assert_eq!(lid_step(o, x, &PowerSettings::default(), &c, &[], false), Do::Nothing, "off by default");
    assert_eq!(lid_step(o, x, &lock, &c, &[], false), Do::Lock);
    assert_eq!(lid_step(o, x, &sleep, &c, &[], false), Do::Sleep);
    // not an edge: already closed at start, still closed, opening, no switch
    assert_eq!(lid_step(None, x, &sleep, &c, &[], false), Do::Nothing, "a lid shut when the shell started does nothing");
    assert_eq!(lid_step(x, x, &sleep, &c, &[], false), Do::Nothing);
    assert_eq!(lid_step(x, o, &sleep, &c, &[], false), Do::Nothing);
    assert_eq!(lid_step(None, None, &sleep, &c, &[], false), Do::Nothing);
    // a sleep that cannot happen is a lock, never a silent nothing
    assert_eq!(lid_step(o, x, &sleep, &Caps { sleep: false, ..c.clone() }, &[], false), Do::Lock, "no sleep state");
    assert_eq!(lid_step(o, x, &sleep, &Caps { t2: true, ..c.clone() }, &[], false), Do::Lock, "a T2 Mac without the experiment");
    assert_eq!(lid_step(o, x, &PowerSettings { sleep_experimental: true, ..sleep.clone() }, &Caps { t2: true, ..c.clone() }, &[], false), Do::Sleep);
    assert_eq!(lid_step(o, x, &sleep, &c, &["an install".into()], false), Do::Lock, "work in progress");
    assert_eq!(lid_step(o, x, &PowerSettings { sleep_when_busy: true, ..sleep.clone() }, &c, &["an install".into()], false), Do::Sleep, "unless the person allowed it");
    assert_eq!(lid_step(o, x, &sleep, &c, &[], true), Do::Lock, "another screen is connected: the person is still working, so it only locks");
    assert_eq!(lid_step(o, x, &lock, &c, &[], true), Do::Lock);
}

#[test]
fn another_screen_is_found_from_the_drm_connectors_and_the_laptops_own_does_not_count() {
    let d = pstate("docked");
    assert!(!external_display(&d), "no display folder at all");
    put(&d, "class/drm/card0-eDP-1/status", "connected");
    put(&d, "class/drm/card0-DP-1/status", "disconnected");
    put(&d, "class/drm/card0/uevent", "x");
    assert!(!external_display(&d), "only the built-in screen is connected");
    put(&d, "class/drm/card0-HDMI-A-1/status", "connected");
    assert!(external_display(&d));
    put(&d, "class/drm/card0-HDMI-A-1/status", "disconnected");
    assert!(!external_display(&d));
    put(&d, "class/drm/card1-DP-2/status", "connected");
    assert!(external_display(&d), "a second graphics card's connector counts too");
}

#[test]
fn the_power_button_opens_the_card_by_default_and_a_sleep_that_cannot_happen_falls_back_to_the_card() {
    let c = Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: false, lid: false };
    let with = |b: ButtonAction| PowerSettings { button: b, ..Default::default() };
    assert_eq!(button_press(&PowerSettings::default(), &c, &[]), Do::Card, "as before");
    assert_eq!(button_press(&with(ButtonAction::Lock), &c, &[]), Do::Lock);
    assert_eq!(button_press(&with(ButtonAction::Nothing), &c, &[]), Do::Nothing);
    assert_eq!(button_press(&with(ButtonAction::Sleep), &c, &[]), Do::Sleep);
    assert_eq!(button_press(&with(ButtonAction::Sleep), &Caps { sleep: false, ..c.clone() }, &[]), Do::Card);
    assert_eq!(button_press(&with(ButtonAction::Sleep), &c, &["a job".into()]), Do::Card, "not while a job runs");
}

#[test]
fn the_lid_and_button_choices_cycle_parse_and_an_old_settings_file_keeps_the_defaults() {
    assert_eq!([LidAction::Nothing.next(), LidAction::Lock.next(), LidAction::Sleep.next()], [LidAction::Lock, LidAction::Sleep, LidAction::Nothing]);
    assert_eq!(ButtonAction::Nothing.next(), ButtonAction::Card);
    assert_eq!((LidAction::parse("sleep"), LidAction::parse("lock"), LidAction::parse("x")), (Some(LidAction::Sleep), Some(LidAction::Lock), None));
    assert_eq!((ButtonAction::parse("card"), ButtonAction::parse("nothing"), ButtonAction::parse("off")), (Some(ButtonAction::Card), Some(ButtonAction::Nothing), None));
    let old: PowerSettings = serde_json::from_str(r#"{"profile":"balanced","sleep_after_min":10}"#).unwrap();
    assert_eq!((old.lid, old.button, old.sleep_after_min), (LidAction::Nothing, ButtonAction::Card, 10));
    let new: PowerSettings = serde_json::from_str(r#"{"lid":"sleep","button":"lock"}"#).unwrap();
    assert_eq!((new.lid, new.button), (LidAction::Sleep, ButtonAction::Lock));
    // a laptop whose kernel shows no lid switch is told so
    let laptop = Caps { laptop: true, ..Default::default() };
    assert!(limits(&laptop).iter().any(|l| l.starts_with("Lid:") && l.contains("no lid switch")));
    assert!(limits(&Caps { laptop: true, lid: true, ..Default::default() }).iter().all(|l| !l.starts_with("Lid:")));
    assert!(limits(&Caps::default()).iter().all(|l| !l.starts_with("Lid:")), "a desktop has no lid to talk about");
}
