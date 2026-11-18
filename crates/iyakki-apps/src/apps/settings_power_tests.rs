//! Settings -> Power: the profile cards write the kernel's files and the settings, sleep after idle cycles and is kept, the page says
//! plainly what the machine cannot do, only administrators change anything, and the page draws at six screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::power::{PowerEnv};
use iyakki_shared::power::{PowerSettings, Profile};
use iyakki_sec::policy::Role;
use std::path::{Path, PathBuf};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn plain_user(tag: &str) -> AppCx {
    let mut cx = admin(tag);
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    cx
}

fn put(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{v}\n")).unwrap();
}

fn get(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap().trim().to_string()
}

fn machine(tag: &str, cpu: bool, sleep: bool, t2: bool) -> PathBuf {
    let d = temp_dir(&format!("{tag}-sys"));
    if cpu {
        let p = "devices/system/cpu/cpufreq/policy0";
        put(&d, &format!("{p}/scaling_governor"), "powersave");
        put(&d, &format!("{p}/scaling_available_governors"), "performance powersave");
        put(&d, &format!("{p}/energy_performance_preference"), "balance_performance");
        put(&d, &format!("{p}/energy_performance_available_preferences"), "default performance balance_performance balance_power power");
    }
    if sleep {
        put(&d, "power/state", "freeze mem");
    }
    if t2 {
        put(&d, "class/dmi/id/sys_vendor", "Apple Inc.");
        put(&d, "class/dmi/id/product_name", "Macmini8,1");
    }
    d
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: char) {
    a.handle(&Event::Key(Key::Char(k)), cx);
}

fn cfg(cx: &AppCx) -> PowerSettings {
    cx.settings().lock().unwrap().get().power.clone()
}

fn open(cx: &AppCx, d: &Path) -> SettingsApp {
    let mut a = SettingsApp::new(cx).with_power_env(PowerEnv::at(d));
    a.go_to(Tab::Power);
    a
}

#[test]
fn the_page_is_in_the_system_group_for_everyone_and_the_search_finds_it() {
    let cx = admin("pw-list");
    let a = open(&cx, &machine("pw-list", true, true, false));
    assert!(a.tabs.contains(&Tab::Power));
    assert_eq!(Tab::Power.group(), "System");
    assert!(Tab::Power.keywords().contains("sleep") && Tab::Power.keywords().contains("governor"));
    let ucx = plain_user("pw-list-user");
    assert!(SettingsApp::new(&ucx).tabs.contains(&Tab::Power));
    let mut b = SettingsApp::new(&cx);
    b.open_item("tab:power", &cx);
    assert_eq!(b.tab(), Tab::Power);
}

#[test]
fn a_profile_card_writes_the_kernel_files_and_the_settings_and_is_audited() {
    let cx = admin("pw-prof");
    let d = machine("pw-prof", true, true, false);
    let mut a = open(&cx, &d);
    assert_eq!(cfg(&cx).profile, Profile::Balanced, "the default");
    key(&mut a, &cx, 'v');
    assert_eq!(cfg(&cx).profile, Profile::PowerSaver);
    assert_eq!(get(&d, "devices/system/cpu/cpufreq/policy0/energy_performance_preference"), "power");
    key(&mut a, &cx, 'e');
    assert_eq!((cfg(&cx).profile, get(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor").as_str()), (Profile::Performance, "performance"));
    key(&mut a, &cx, 'b');
    assert_eq!(get(&d, "devices/system/cpu/cpufreq/policy0/energy_performance_preference"), "balance_performance");
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Balanced")), "{:?}", a.status());
}

#[test]
fn a_machine_without_cpu_speed_control_refuses_in_words_and_keeps_the_old_choice() {
    let cx = admin("pw-vm");
    let d = machine("pw-vm", false, false, false);
    let mut a = open(&cx, &d);
    key(&mut a, &cx, 'e');
    assert_eq!(cfg(&cx).profile, Profile::Balanced);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("does not let IYAKKI change the processor's speed")), "{:?}", a.status());
    key(&mut a, &cx, 's');
    assert_eq!(cfg(&cx).sleep_after_min, 0);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot sleep")), "{:?}", a.status());
}

#[test]
fn sleep_after_idle_cycles_through_the_times_and_the_busy_switch_is_off_by_default() {
    let cx = admin("pw-sleep");
    let d = machine("pw-sleep", true, true, false);
    let mut a = open(&cx, &d);
    assert_eq!((cfg(&cx).sleep_after_min, cfg(&cx).sleep_when_busy), (0, false));
    let mut seen = Vec::new();
    for _ in 0..6 {
        key(&mut a, &cx, 's');
        seen.push(cfg(&cx).sleep_after_min);
    }
    assert_eq!(seen, [5, 10, 15, 30, 60, 0]);
    key(&mut a, &cx, 'w');
    assert!(cfg(&cx).sleep_when_busy);
    key(&mut a, &cx, 'w');
    assert!(!cfg(&cx).sleep_when_busy);
    // the experiment switch exists only where sleep is unreliable
    key(&mut a, &cx, 'x');
    assert!(!cfg(&cx).sleep_experimental);
    let t2 = machine("pw-t2", true, true, true);
    let mut b = open(&cx, &t2);
    key(&mut b, &cx, 'x');
    assert!(cfg(&cx).sleep_experimental);
}

#[test]
fn only_administrators_change_anything() {
    let cx = plain_user("pw-user");
    let d = machine("pw-user", true, true, false);
    let mut a = open(&cx, &d);
    for k in ['e', 'v', 's', 'w'] {
        key(&mut a, &cx, k);
    }
    assert_eq!(cfg(&cx), PowerSettings::default());
    assert_eq!(get(&d, "devices/system/cpu/cpufreq/policy0/scaling_governor"), "powersave", "no file was written");
    assert!(a.status().is_some_and(|s| !s.1), "{:?}", a.status());
}

#[test]
fn the_cards_are_clickable_and_the_page_says_what_is_not_supported() {
    let cx = admin("pw-draw");
    let mut a = open(&cx, &machine("pw-draw", true, true, true));
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    for k in ['e', 'b', 'v', 's', 'w', 'p', 'x'] {
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char(k))), "a card for {k}");
    }
    // what the machine cannot do is said plainly
    let text = iyakki_shared::power::limits(&crate::agent().host_probe().power_caps(&PowerEnv::at(machine("pw-draw-t", true, true, true)))).join(" ");
    assert!(text.contains("experimental on this Mac"), "{text}");
    // a click on a card does what the key does
    let hit = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char('v'))).unwrap().ev.clone();
    a.handle(&hit, &cx);
    assert_eq!(cfg(&cx).profile, Profile::PowerSaver);
}

#[test]
fn the_page_draws_at_six_sizes_on_every_kind_of_machine() {
    let cx = admin("pw-sizes");
    let ucx = plain_user("pw-sizes-user");
    for (name, cpu, sleep, t2, who) in [("full", true, true, false, &cx), ("vm", false, false, false, &cx), ("t2", true, true, true, &cx), ("user", true, true, false, &ucx)] {
        let mut a = open(who, &machine(&format!("pw-sizes-{name}"), cpu, sleep, t2));
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)] {
            let mut g = Gfx::new(w, h);
            let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, who, 0);
            for hit in &g.hits {
                assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{name} {w}x{h}: {} is inside the width", hit.label);
            }
        }
    }
}

#[test]
fn the_lid_and_the_power_button_are_chosen_on_the_page_kept_and_the_lid_row_exists_only_with_a_lid() {
    let cx = admin("pw-lid");
    let no_lid = machine("pw-lid-a", true, true, false);
    let mut a = SettingsApp::new(&cx).with_power_env(PowerEnv::at(&no_lid));
    a.go_to(Tab::Power);
    key(&mut a, &cx, 'l');
    assert!(a.status().unwrap().0.contains("no lid switch"), "{:?}", a.status());
    assert_eq!(cfg(&cx).lid, iyakki_shared::power::LidAction::Nothing);
    // the button works on every machine and cycles card -> lock -> sleep -> nothing -> card
    key(&mut a, &cx, 'p');
    assert_eq!(cfg(&cx).button, iyakki_shared::power::ButtonAction::Lock);
    key(&mut a, &cx, 'p');
    assert_eq!(cfg(&cx).button, iyakki_shared::power::ButtonAction::Sleep);
    assert!(a.status().unwrap().0.contains("sleep"));
    key(&mut a, &cx, 'p');
    key(&mut a, &cx, 'p');
    assert_eq!(cfg(&cx).button, iyakki_shared::power::ButtonAction::Card);
    let with_lid = machine("pw-lid-b", true, true, false);
    put(&with_lid, "proc/acpi/button/lid/LID0/state", "state:      open");
    let mut a = SettingsApp::new(&cx).with_power_env(PowerEnv::at(&with_lid));
    a.go_to(Tab::Power);
    key(&mut a, &cx, 'l');
    assert_eq!(cfg(&cx).lid, iyakki_shared::power::LidAction::Lock);
    key(&mut a, &cx, 'l');
    assert_eq!(cfg(&cx).lid, iyakki_shared::power::LidAction::Sleep);
    assert!(a.status().unwrap().0.contains("only locks"), "{:?}", a.status());
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert!(g.text_log.iter().any(|t| t.2.contains("Closing the lid: sleep")), "the lid row is drawn");
    assert!(g.text_log.iter().any(|t| t.2.contains("Power button: open the power card")), "{:?}", g.text_log.iter().map(|t| t.2.clone()).filter(|t| t.contains("Power")).collect::<Vec<_>>());
    // only an administrator changes either
    let user = plain_user("pw-lid-user");
    let mut a = SettingsApp::new(&user).with_power_env(PowerEnv::at(&with_lid));
    a.go_to(Tab::Power);
    key(&mut a, &user, 'p');
    key(&mut a, &user, 'l');
    assert_eq!(cfg(&user).lid, iyakki_shared::power::LidAction::Nothing);
    assert_eq!(cfg(&user).button, iyakki_shared::power::ButtonAction::Card);
}

fn fan_on(cx: &AppCx) -> bool {
    std::fs::read_to_string(cx.data_dir().join("fan.json")).map(|t| t.contains("\"enabled\": true")).unwrap_or(false)
}

#[test]
fn the_fan_card_exists_only_with_an_applesmc_fan_and_an_administrator_turns_it_on_and_off() {
    let cx = admin("pw-fan");
    // no fan: no card, and the key says why
    let none = machine("pw-fan-a", true, true, false);
    let mut a = open(&cx, &none);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert!(!g.text_log.iter().any(|t| t.2.contains("Fan control")));
    key(&mut a, &cx, 'f');
    assert!(a.status().unwrap().0.contains("no applesmc fan"), "{:?}", a.status());
    assert!(!fan_on(&cx));
    // a Mac with a fan: the card shows, off by default, and a key press switches it and writes the file init reads
    let mac = machine("pw-fan-b", true, true, false);
    put(&mac, "class/hwmon/hwmon2/name", "applesmc");
    for (k, v) in [("fan1_min", "1200"), ("fan1_max", "6200"), ("fan1_manual", "0"), ("fan1_output", "1200"), ("temp1_label", "TC0P CPU"), ("temp1_input", "50000")] {
        put(&mac, &format!("class/hwmon/hwmon2/{k}"), v);
    }
    let mut a = open(&cx, &mac);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert!(g.text_log.iter().any(|t| t.2.contains("Fan control: the firmware runs the fans")), "{:?}", g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>());
    key(&mut a, &cx, 'f');
    assert!(fan_on(&cx));
    assert!(a.status().unwrap().0.contains("IYAKKI runs the fans"));
    key(&mut a, &cx, 'f');
    assert!(!fan_on(&cx));
    // only an administrator
    let user = plain_user("pw-fan-user");
    let mut a = open(&user, &mac);
    key(&mut a, &user, 'f');
    assert!(!fan_on(&user));
}
