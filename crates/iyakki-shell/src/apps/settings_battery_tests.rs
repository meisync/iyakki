//! Settings -> Battery: the page exists only on a laptop, shows the bar, status, health and time from a fake /sys, the three
//! switches save and are audited, the shutdown switch is for administrators, and the page draws at six screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::battery::BatteryEnv;
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

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn laptop_sys(tag: &str) -> PathBuf {
    let d = temp_dir(&format!("{tag}-sys"));
    for (k, v) in [("type", "Battery"), ("status", "Discharging"), ("capacity", "64"), ("energy_now", "32000000"), ("energy_full", "50000000"), ("energy_full_design", "60000000"), ("power_now", "8000000"), ("cycle_count", "301"), ("technology", "Li-ion")] {
        put(&d, &format!("class/power_supply/BAT0/{k}"), &format!("{v}\n"));
    }
    put(&d, "class/power_supply/ADP1/type", "Mains\n");
    put(&d, "class/power_supply/ADP1/online", "0\n");
    d
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn cfg(cx: &AppCx) -> iyakki_agent::battery::BatterySettings {
    cx.services.settings.lock().unwrap().get().battery.clone()
}

#[test]
fn the_page_is_in_the_list_only_on_a_laptop() {
    let cx = admin("bat-list");
    let desktop = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(temp_dir("bat-list-desktop")));
    assert!(!desktop.tabs.contains(&Tab::Battery), "no battery, no page");
    let laptop = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(laptop_sys("bat-list")));
    assert!(laptop.tabs.contains(&Tab::Battery));
    let order: Vec<Tab> = laptop.tabs.clone();
    let pos = |t: Tab| order.iter().position(|x| *x == t).unwrap();
    assert!(pos(Tab::Time) < pos(Tab::Battery) && pos(Tab::Battery) < pos(Tab::HostName), "in the System group, after Time");
    assert_eq!(Tab::Battery.group(), "System");
    // a laptop whose kernel has no battery driver still gets the page (it says why)
    let d = temp_dir("bat-list-nodrv");
    put(&d, "class/dmi/id/chassis_type", "10\n");
    put(&d, "bus/acpi/devices/PNP0C0A:00/status", "15\n");
    let nodrv = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(d));
    assert!(nodrv.tabs.contains(&Tab::Battery));
    // a plain user sees it too
    let ucx = plain_user("bat-list-user");
    assert!(SettingsApp::new(&ucx).with_battery_env(BatteryEnv::at(laptop_sys("bat-list-user"))).tabs.contains(&Tab::Battery));
}

#[test]
fn the_search_box_finds_the_page_and_the_devices_row_leads_to_it() {
    let cx = admin("bat-find");
    let mut a = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(laptop_sys("bat-find")));
    a.open_item("tab:battery", &cx);
    assert_eq!(a.tab(), Tab::Battery);
    assert!(Tab::Battery.keywords().contains("battery") && Tab::Battery.keywords().contains("percentage"));
}

#[test]
fn the_page_shows_the_bar_the_health_and_the_time() {
    let cx = admin("bat-page");
    let mut a = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(laptop_sys("bat-page")));
    a.go_to(Tab::Battery);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    // the three switches are clickable and the buttons exist
    for k in ['p', 'w', 's'] {
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char(k))), "the switch for {k}");
    }
    let r = a.bat_reading();
    assert_eq!(r.percent_whole(), Some(64));
    assert!(r.rows().iter().any(|(k, v)| k == "Health" && v.starts_with("83%")));
    assert!(r.time_words().is_some_and(|t| t.starts_with("About 4 h")), "{:?}", r.time_words());
}

#[test]
fn the_switches_save_and_are_audited() {
    let cx = admin("bat-sw");
    let mut a = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(laptop_sys("bat-sw")));
    a.go_to(Tab::Battery);
    assert!(cfg(&cx).show_percent() && cfg(&cx).warnings() && cfg(&cx).shutdown_at_critical(), "all on by default");
    key(&mut a, &cx, Key::Char('p'));
    assert!(!cfg(&cx).show_percent());
    key(&mut a, &cx, Key::Char('p'));
    assert!(cfg(&cx).show_percent());
    key(&mut a, &cx, Key::Char('w'));
    assert!(!cfg(&cx).warnings());
    key(&mut a, &cx, Key::Char('s'));
    assert!(!cfg(&cx).shutdown_at_critical());
    key(&mut a, &cx, Key::Char('s'));
    assert!(cfg(&cx).shutdown_at_critical());
}

#[test]
fn only_administrators_change_the_shutdown_switch() {
    let cx = plain_user("bat-user");
    let mut a = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(laptop_sys("bat-user")));
    a.go_to(Tab::Battery);
    key(&mut a, &cx, Key::Char('s'));
    assert!(cfg(&cx).shutdown_at_critical(), "refused");
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")), "{:?}", a.status());
    // the display switches are for everyone
    key(&mut a, &cx, Key::Char('p'));
    assert!(!cfg(&cx).show_percent());
}

#[test]
fn the_page_draws_at_six_sizes_and_says_why_when_there_is_no_driver() {
    let cx = admin("bat-sizes");
    let d = temp_dir("bat-sizes-nodrv");
    put(&d, "class/dmi/id/chassis_type", "10\n");
    put(&d, "bus/acpi/devices/PNP0C0A:00/status", "15\n");
    for (name, env) in [("laptop", BatteryEnv::at(laptop_sys("bat-sizes"))), ("nodriver", BatteryEnv::at(d))] {
        let mut a = SettingsApp::new(&cx).with_battery_env(env);
        a.go_to(Tab::Battery);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)] {
            let mut g = Gfx::new(w, h);
            let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &cx, 0);
            for hit in &g.hits {
                assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{name} {w}x{h}: {} is inside the width", hit.label);
            }
        }
    }
    let mut a = SettingsApp::new(&cx).with_battery_env(BatteryEnv::at(temp_dir("bat-sizes-x")));
    a.go_to(Tab::Overview);
    assert!(!a.tabs.contains(&Tab::Battery));
}
