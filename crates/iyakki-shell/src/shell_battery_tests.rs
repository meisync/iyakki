//! The battery indicator in the shell: hidden on a desktop, drawn in the launcher bar, the app header and the panes bar without
//! crossing any other control at six screen sizes, its icon states (pixel checks), the popup (click, F4, Esc), the warnings, the
//! countdown card and the clean shutdown, and the cases that must never power the machine off.

use super::tests::*;
use super::*;
use crate::apps::tests::MockHost;
use crate::batterybar as bb;
use iyakki_agent::battery::BatteryEnv;
use std::path::{Path, PathBuf};

const SIZES: [(u32, u32); 6] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)];

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{content}\n")).unwrap();
}

fn fresh(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-shbat-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A made-up /sys with one battery (`cap` percent, `status`) and an adapter (`ac`).
fn laptop(tag: &str, cap: u32, status: &str, ac: bool) -> PathBuf {
    let d = fresh(tag);
    set(&d, cap, status, ac);
    d
}

fn set(d: &Path, cap: u32, status: &str, ac: bool) {
    for (k, v) in [("type", "Battery"), ("status", status), ("capacity", &cap.to_string()), ("energy_now", &(cap as u64 * 500_000).to_string()), ("energy_full", "50000000"), ("energy_full_design", "60000000"), ("power_now", "10000000")] {
        put(d, &format!("class/power_supply/BAT0/{k}"), v);
    }
    put(d, "class/power_supply/ADP1/type", "Mains");
    put(d, "class/power_supply/ADP1/online", if ac { "1" } else { "0" });
}

fn signed(tag: &str, sys: Option<&Path>) -> (Shell, Arc<MockHost>) {
    let (mut sh, host) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    sh.devprobe = Some(crate::devpop::Probe::real(crate::apps::devices::DeviceEnv::real(), None, sh.env.services.dir().to_path_buf(), "t".into(), false));
    if let Some(d) = sys {
        sh.battery.set_env(BatteryEnv::at(d));
    }
    sh.battery_step(0);
    (sh, host)
}

fn chip(sh: &Shell) -> Option<R> {
    sh.hits().iter().find(|h| h.label.starts_with("Battery")).map(|h| h.rect)
}

fn disjoint(a: R, b: R) -> bool {
    a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5
}

fn near(p: [u8; 4], c: [u8; 4]) -> bool {
    (0..3).all(|i| (p[i] as i32 - c[i] as i32).abs() <= 12)
}

fn px(g: &Gfx, x: f32, y: f32) -> [u8; 4] {
    let p = g.pm.pixel(x as u32, y as u32).unwrap();
    [p.red(), p.green(), p.blue(), p.alpha()]
}

fn reading(tag: &str, cap: u32, status: &str, ac: bool) -> iyakki_agent::battery::Reading {
    iyakki_agent::battery::read(&BatteryEnv::at(laptop(tag, cap, status, ac)))
}

// ---- the icon

fn icon_on_black(r: &iyakki_agent::battery::Reading) -> (Gfx, R) {
    let mut g = Gfx::new(200, 80);
    g.clear(rgb(0, 0, 0));
    let body = bb::draw_icon(&mut g, 20.0, 40.0, 24.0, 1.0, r);
    (g, body)
}

#[test]
fn the_fill_is_green_amber_under_20_red_under_10_and_as_long_as_the_charge() {
    for (cap, want) in [(80, crate::ui::OK), (50, crate::ui::OK), (19, crate::fx::AMBER), (15, crate::fx::AMBER), (9, crate::ui::ERR), (2, crate::ui::ERR)] {
        let r = reading(&format!("ic-{cap}"), cap, "Discharging", false);
        let (g, body) = icon_on_black(&r);
        // inside the fill, near its left end (a bolt or plug would sit in the middle)
        let p = px(&g, body.x + 3.5, body.y + body.h / 2.0);
        assert!(near(p, want), "{cap}%: {p:?} should be {want:?}");
    }
    // the fill follows the charge: 25 % leaves the right half empty
    let (g, body) = icon_on_black(&reading("ic-len", 25, "Discharging", false));
    assert!(near(px(&g, body.x + 6.0, body.y + body.h / 2.0), crate::fx::AMBER) || near(px(&g, body.x + 6.0, body.y + body.h / 2.0), crate::ui::OK));
    assert_eq!(px(&g, body.x + body.w * 0.8, body.y + body.h / 2.0)[..3], [0, 0, 0], "empty right part");
    let (g, body) = icon_on_black(&reading("ic-full", 100, "Discharging", false));
    assert!(near(px(&g, body.x + body.w * 0.85, body.y + body.h / 2.0), crate::ui::OK), "100 % fills it");
    // charging is not amber or red even when low
    let (g, body) = icon_on_black(&reading("ic-chg", 8, "Charging", true));
    assert!(near(px(&g, body.x + 4.0, body.y + body.h / 2.0), crate::ui::OK));
}

#[test]
fn charging_draws_a_white_bolt_and_a_full_battery_on_the_adapter_a_plug() {
    let white = |p: [u8; 4]| p[0] > 235 && p[1] > 235 && p[2] > 235;
    let count_white = |g: &Gfx, body: R| {
        let mut n = 0;
        for x in body.x as u32..(body.x + body.w) as u32 {
            for y in body.y as u32..(body.y + body.h) as u32 {
                n += white(px(g, x as f32, y as f32)) as u32;
            }
        }
        n
    };
    let (g, body) = icon_on_black(&reading("sym-chg", 60, "Charging", true));
    assert!(count_white(&g, body) >= 8, "a bolt in the body: {}", count_white(&g, body));
    let (g, body) = icon_on_black(&reading("sym-full", 100, "Full", true));
    assert!(count_white(&g, body) >= 8, "a plug in the body");
    let (g, body) = icon_on_black(&reading("sym-nc", 80, "Not charging", true));
    assert!(count_white(&g, body) >= 8, "not charging on the adapter: the plug too");
    let (g, body) = icon_on_black(&reading("sym-dis", 60, "Discharging", false));
    assert_eq!(count_white(&g, body), 0, "no symbol while discharging");
    // unknown: a question mark (text) and no fill colour
    let d = fresh("sym-unk");
    put(&d, "class/power_supply/BAT0/type", "Battery");
    let (g, body) = icon_on_black(&iyakki_agent::battery::read(&BatteryEnv::at(d)));
    let mut bright = 0;
    let mut green = 0;
    for x in body.x as u32..(body.x + body.w) as u32 {
        for y in body.y as u32..(body.y + body.h) as u32 {
            let p = px(&g, x as f32, y as f32);
            bright += (p[0] > 150 && p[1] > 150 && p[2] > 150) as u32;
            green += near(p, crate::ui::OK) as u32;
        }
    }
    assert!(bright >= 6, "a question mark is drawn: {bright}");
    assert_eq!(green, 0);
}

// ---- hidden on a desktop

#[test]
fn a_desktop_a_vm_and_a_mac_mini_show_nothing_and_f4_does_nothing() {
    for (name, d) in [("none", fresh("desk-none")), ("mains", {
        let d = fresh("desk-mains");
        put(&d, "class/power_supply/ADP1/type", "Mains");
        put(&d, "class/power_supply/ADP1/online", "1");
        put(&d, "class/dmi/id/chassis_type", "3");
        d
    }), ("mouse", {
        let d = fresh("desk-mouse");
        put(&d, "class/power_supply/hid-aa-battery/type", "Battery");
        put(&d, "class/power_supply/hid-aa-battery/scope", "Device");
        put(&d, "class/power_supply/hid-aa-battery/capacity", "15");
        d
    })] {
        let (mut sh, _) = signed(&format!("desk-{name}"), Some(&d));
        let sizes: Vec<(u32, u32)> = if name == "none" { SIZES.to_vec() } else { vec![SIZES[0], SIZES[3]] };
        for (w, h) in sizes {
            for launcher in [true, false] {
                let mut g = Gfx::new(w, h);
                sh.screen = Screen::Launcher;
                if !launcher {
                    assert!(sh.open_for_demo("settings"));
                }
                sh.render(&mut g);
                assert!(chip(&sh).is_none(), "{name} {w}x{h}: no battery chip");
            }
        }
        key(&mut sh, Key::F(4));
        assert!(!sh.battery.open, "{name}: F4 opens nothing");
    }
}

// ---- the bars

fn other_rects(sh: &Shell, w: u32, s: f32, app: bool) -> Vec<(&'static str, R)> {
    let mut v = Vec::new();
    if app {
        v.extend([("devices", dev_rect(w, s)), ("screenshot", shot_rect(w, s)), ("clipboard", clip_rect(w, s)), ("inbox", bell_rect(w, s)), ("help", help_rect(w, s)), ("back", back_rect(w, s))]);
        if let Some(c) = sh.hits().iter().find(|h| h.label == "clock") {
            v.push(("clock", c.rect));
        }
    } else {
        let power = sh.may_power();
        let n = 3 + power as usize + sh.inbox_allowed() as usize + 1;
        for i in 0..n {
            v.push(("button", header_button(w, s, i)));
        }
        if let Some(c) = sh.hits().iter().find(|h| h.label == "clock") {
            v.push(("clock", c.rect));
        }
        if let Some(c) = sh.hits().iter().find(|h| h.label == "IYAKKI menu") {
            v.push(("name", c.rect));
        }
    }
    v
}

#[test]
fn the_chip_fits_the_launcher_bar_and_the_app_bar_at_six_sizes_without_crossing_anything() {
    let d = laptop("bars", 64, "Discharging", false);
    let (mut sh, _) = signed("bars", Some(&d));
    sh.clock.fixed = Some(1_790_000_000);
    for (w, h) in SIZES {
        let s = scale_for(w, h);
        for app in [false, true] {
            let mut g = Gfx::new(w, h);
            sh.screen = Screen::Launcher;
            if app {
                assert!(sh.open_for_demo("settings"));
            }
            sh.render(&mut g);
            let big = w >= 1280;
            let Some(c) = chip(&sh) else {
                assert!(!big, "{w}x{h} app={app}: there is room for the chip");
                continue;
            };
            assert!(c.x >= 0.0 && c.x + c.w <= w as f32 && c.y >= 0.0 && c.y + c.h <= 90.0 * s, "{w}x{h} app={app}: inside the bar {c:?}");
            for (n, o) in other_rects(&sh, w, s, app) {
                assert!(disjoint(c, o), "{w}x{h} app={app}: the chip {c:?} overlaps {n} {o:?}");
            }
            assert!(c.w >= 20.0 * s && c.h >= 24.0 * s, "{w}x{h}: big enough to hit");
            // the chip's click is F4
            assert_eq!(sh.hits().iter().find(|h| h.rect == c).unwrap().ev, Event::Key(Key::F(4)));
            assert!(c.x > 150.0 * s, "{w}x{h}: leaves room for the name or the app's title");
        }
    }
}

#[test]
fn the_percentage_can_be_hidden_and_the_chip_shrinks_to_the_icon() {
    let d = laptop("pct", 64, "Discharging", false);
    let (mut sh, _) = signed("pct", Some(&d));
    let mut g = Gfx::new(1920, 1080);
    sh.render(&mut g);
    let wide = chip(&sh).unwrap();
    sh.env.services.settings.lock().unwrap().update(|s| s.battery.hide_percent = true).unwrap();
    sh.battery_step(1);
    sh.render(&mut g);
    let narrow = chip(&sh).unwrap();
    assert!(narrow.w < wide.w - 10.0, "icon only: {narrow:?} vs {wide:?}");
    assert_eq!(narrow.x + narrow.w, wide.x + wide.w, "the right edge stays");
}

#[test]
fn the_chip_is_in_a_panes_bar_beside_the_clock_without_overlap() {
    let d = laptop("panes", 64, "Discharging", false);
    let (mut sh, _) = signed("panes", Some(&d));
    let n = {
        let mut g = Gfx::new(1920, 1080);
        sh.render(&mut g);
        chip(&sh).is_some()
    };
    assert!(n);
    // the panes bar is drawn by shell_panes.rs; the plan function is the same one, so check it directly at its sizes
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        let s = scale_for(w, h);
        let bh = 36.0 * s;
        let right = w as f32 * 0.6;
        let left_limit = 20.0 * s + 120.0 * s;
        if let Some(p) = sh.battery.plan(&mut g, s, right, left_limit, 4.0 * s, bh - 8.0 * s, 13.0 * s) {
            assert!(p.rect.x >= left_limit && p.rect.x + p.rect.w <= right + 0.01, "{w}x{h}: {p:?}");
        }
    }
}

// ---- the popup

#[test]
fn a_click_or_f4_opens_the_popup_and_esc_or_a_click_outside_closes_it() {
    let d = laptop("pop", 64, "Charging", true);
    let (mut sh, _) = signed("pop", Some(&d));
    for (w, h) in SIZES {
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        sh.screen = Screen::Launcher;
        sh.render(&mut g);
        let c = chip(&sh);
        key(&mut sh, Key::F(4));
        assert!(sh.battery.open, "{w}x{h}: F4 opens it");
        sh.render(&mut g);
        let s = scale_for(w, h);
        let p = sh.battery.panel(w, h, s, 8);
        assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= w as f32 + 0.5 && p.y + p.h <= h as f32 + 0.5, "{w}x{h}: panel on screen {p:?}");
        let labels: Vec<String> = sh.hits().iter().map(|x| x.label.clone()).collect();
        assert!(labels.contains(&"close".to_string()) && labels.iter().any(|l| l == "Battery settings"), "{labels:?}");
        for hit in sh.hits() {
            assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5, "{w}x{h}: {} inside", hit.label);
        }
        key(&mut sh, Key::Esc);
        assert!(!sh.battery.open, "Esc closes");
        sh.render(&mut g);
        // the chip's own click (when drawn) opens it too; a click outside closes it
        if let Some(c) = c {
            sh.handle(&Event::MouseDown((c.x + c.w / 2.0) as i32, (c.y + c.h / 2.0) as i32));
            assert!(sh.battery.open, "{w}x{h}: clicking the chip opens it");
            sh.render(&mut g);
            sh.handle(&Event::MouseDown(2, h as i32 - 2));
            assert!(!sh.battery.open, "a click outside closes it");
        }
    }
}

#[test]
fn the_popup_tells_the_status_in_plain_words_and_leads_to_the_settings_page() {
    let d = laptop("words", 100, "Not charging", true);
    let (mut sh, _) = signed("words", Some(&d));
    let rows = sh.battery.reading.rows();
    assert!(rows.iter().any(|(k, v)| k == "Status" && v == "Plugged in but not charging (battery protection)"), "{rows:?}");
    assert!(rows.iter().any(|(k, v)| k == "Power adapter" && v == "Connected"));
    key(&mut sh, Key::F(4));
    key(&mut sh, Key::Enter);
    assert!(!sh.battery.open);
    assert!(sh.screen_name().starts_with("app:"), "Enter opens Settings: {}", sh.screen_name());
}

#[test]
fn a_kernel_without_a_battery_driver_shows_a_question_mark_and_says_why() {
    let d = fresh("nodrv");
    put(&d, "class/dmi/id/chassis_type", "10");
    put(&d, "bus/acpi/devices/PNP0C0A:00/status", "15");
    put(&d, "class/power_supply/ADP1/type", "Mains");
    put(&d, "class/power_supply/ADP1/online", "0");
    let (mut sh, _) = signed("nodrv", Some(&d));
    let mut g = Gfx::new(1920, 1080);
    sh.render(&mut g);
    let c = chip(&sh).expect("the chip is there");
    assert!(sh.hits().iter().any(|h| h.label.contains("No battery driver: this image needs a newer kernel")));
    assert!(c.w < 60.0, "icon only: {c:?}");
    key(&mut sh, Key::F(4));
    assert!(sh.battery.open);
    assert_eq!(sh.battery.reading.rows()[0].1, "No battery driver: this image needs a newer kernel");
}

// ---- warnings, the card, the countdown and the shutdown

#[test]
fn toasts_at_20_and_10_percent_once_and_the_plug_in_card_at_5() {
    let d = laptop("warn", 50, "Discharging", false);
    let (mut sh, _) = signed("warn", Some(&d));
    sh.toast = None;
    set(&d, 20, "Discharging", false);
    sh.battery_step(10);
    assert!(sh.toast_text().is_some_and(|t| t.contains("20%")), "{:?}", sh.toast_text());
    sh.toast = None;
    set(&d, 19, "Discharging", false);
    sh.battery_step(20);
    assert!(sh.toast_text().is_none(), "no repeat");
    set(&d, 10, "Discharging", false);
    sh.battery_step(30);
    assert!(sh.toast_text().is_some_and(|t| t.contains("10%")), "{:?}", sh.toast_text());
    set(&d, 5, "Discharging", false);
    sh.battery_step(40);
    assert!(sh.battery.plug_card, "the Plug in now card");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(bb::CARD_OK)));
    sh.dispatch(&Event::Tab(bb::CARD_OK));
    assert!(!sh.battery.plug_card);
    // plugging in closes it too
    set(&d, 5, "Discharging", false);
    sh.battery_step(50);
    set(&d, 5, "Charging", true);
    sh.battery_step(60);
    assert!(!sh.battery.plug_card);
}

#[test]
fn warnings_off_means_no_toast_and_no_card() {
    let d = laptop("warn-off", 50, "Discharging", false);
    let (mut sh, _) = signed("warn-off", Some(&d));
    sh.env.services.settings.lock().unwrap().update(|s| s.battery.no_warnings = true).unwrap();
    sh.toast = None;
    for (i, c) in [20, 10, 5].iter().enumerate() {
        set(&d, *c, "Discharging", false);
        sh.battery_step(10 * (i as u64 + 1));
    }
    assert!(sh.toast_text().is_none() && !sh.battery.plug_card);
}

#[test]
fn at_the_critical_level_a_visible_countdown_runs_and_then_the_machine_powers_off_cleanly() {
    let d = laptop("crit", 50, "Discharging", false);
    let (mut sh, host) = signed("crit", Some(&d));
    set(&d, 3, "Discharging", false);
    sh.battery_step(10);
    assert!(!sh.battery.counting_down(), "one critical reading is not enough");
    sh.battery_step(20);
    assert!(sh.battery.counting_down());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(bb::CANCEL)), "the Cancel button is on the card");
    assert!(host.powered_off.lock().unwrap().is_none());
    sh.battery_step(35);
    assert!(host.powered_off.lock().unwrap().is_none(), "not before 30 seconds");
    sh.battery_step(50);
    assert_eq!(host.powered_off.lock().unwrap().as_deref(), Some("off"), "after the countdown");
}

#[test]
fn plugging_in_or_pressing_cancel_or_turning_it_off_stops_the_countdown() {
    // a cable
    let d = laptop("crit-ac", 3, "Discharging", false);
    let (mut sh, host) = signed("crit-ac", Some(&d));
    sh.battery_step(10);
    sh.battery_step(20);
    assert!(sh.battery.counting_down());
    set(&d, 3, "Charging", true);
    sh.battery_step(21);
    assert!(!sh.battery.counting_down());
    sh.battery_step(60);
    assert!(host.powered_off.lock().unwrap().is_none());
    // the Cancel button
    let d = laptop("crit-btn", 3, "Discharging", false);
    let (mut sh, host) = signed("crit-btn", Some(&d));
    sh.battery_step(10);
    sh.battery_step(20);
    sh.dispatch(&Event::Tab(bb::CANCEL));
    sh.battery_step(60);
    sh.battery_step(100);
    assert!(host.powered_off.lock().unwrap().is_none(), "cancelled by the person");
    // Esc
    let d = laptop("crit-esc", 3, "Discharging", false);
    let (mut sh, host) = signed("crit-esc", Some(&d));
    sh.battery_step(10);
    sh.battery_step(20);
    key(&mut sh, Key::Esc);
    sh.battery_step(70);
    assert!(host.powered_off.lock().unwrap().is_none());
    // the switch
    let d = laptop("crit-off", 3, "Discharging", false);
    let (mut sh, host) = signed("crit-off", Some(&d));
    sh.env.services.settings.lock().unwrap().update(|s| s.battery.no_shutdown = true).unwrap();
    for t in (10..200).step_by(10) {
        sh.battery_step(t);
    }
    assert!(host.powered_off.lock().unwrap().is_none(), "switched off: never");
}

#[test]
fn a_machine_that_merely_lacks_battery_data_is_never_shut_down() {
    let mut cases: Vec<(&str, PathBuf)> = Vec::new();
    // no driver
    let d = fresh("never-nodrv");
    put(&d, "class/dmi/id/chassis_type", "10");
    put(&d, "bus/acpi/devices/PNP0C0A:00/status", "31");
    cases.push(("no driver", d));
    // a desktop
    cases.push(("desktop", fresh("never-desk")));
    // a battery that reports 0 and unknown status
    let d = fresh("never-zero");
    put(&d, "class/power_supply/BAT0/type", "Battery");
    put(&d, "class/power_supply/BAT0/capacity", "0");
    cases.push(("zero and unknown", d));
    // 0 percent while discharging with nothing else (a driver that has not read the pack)
    let d = fresh("never-zero2");
    put(&d, "class/power_supply/BAT0/type", "Battery");
    put(&d, "class/power_supply/BAT0/capacity", "0");
    put(&d, "class/power_supply/BAT0/status", "Discharging");
    cases.push(("zero while discharging", d));
    // garbage
    let d = fresh("never-garbage");
    put(&d, "class/power_supply/BAT0/type", "Battery");
    put(&d, "class/power_supply/BAT0/capacity", "-7");
    put(&d, "class/power_supply/BAT0/status", "Discharging");
    cases.push(("garbage", d));
    // charging at 1 %
    cases.push(("charging", laptop("never-chg", 1, "Charging", true)));
    // a low mouse
    let d = fresh("never-mouse");
    put(&d, "class/power_supply/hidpp_battery_0/type", "Battery");
    put(&d, "class/power_supply/hidpp_battery_0/capacity", "1");
    put(&d, "class/power_supply/hidpp_battery_0/status", "Discharging");
    cases.push(("mouse", d));
    for (name, d) in cases {
        let (mut sh, host) = signed(&format!("never-{}", name.replace(' ', "-")), Some(&d));
        for t in (10..400).step_by(10) {
            sh.battery_step(t);
            assert!(!sh.battery.counting_down(), "{name}: no countdown");
        }
        assert!(host.powered_off.lock().unwrap().is_none(), "{name}: never powers off");
    }
}

#[test]
fn the_countdown_card_is_on_screen_at_every_size_and_keeps_the_rest_clickable() {
    let d = laptop("card", 3, "Discharging", false);
    let (mut sh, _) = signed("card", Some(&d));
    sh.battery_step(10);
    sh.battery_step(20);
    for (w, h) in SIZES {
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        sh.screen = Screen::Launcher;
        sh.render(&mut g);
        let cancel = sh.hits().iter().find(|x| x.ev == Event::Tab(bb::CANCEL)).unwrap_or_else(|| panic!("{w}x{h}: Cancel")).rect;
        assert!(cancel.x >= 0.0 && cancel.y >= 0.0 && cancel.x + cancel.w <= w as f32 && cancel.y + cancel.h <= h as f32, "{w}x{h}: {cancel:?}");
        assert!(sh.hits().iter().any(|x| x.label == "clock" || x.label == "IYAKKI menu"), "{w}x{h}: the bar still answers");
    }
}
