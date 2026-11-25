//! Settings -> Power on a fake port: a profile is written through the port and saved and audited, a refused write is shown and not saved, the
//! sleep, lid, button and busy-sleep cards save and say what they did (greyed out without the hardware), the fan card goes through the port, a
//! non-administrator is refused and audited, busy settings are told, and the page draws at several sizes. No sleeps, no real processor.

use super::super::settings_port::SettingsDataPort;
use super::system_port::{FakeSys, PowerPort};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_shared::power::{Caps, PowerEnv, Profile};
use std::sync::atomic::Ordering;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("power-fake");
    let mut a = SettingsApp::new(&cx).with_power_env(PowerEnv::at("/nonexistent-iyakki-sys"));
    a.go_to(Tab::Power);
    a
}

fn laptop(p: &FakeSys) {
    *p.caps.lock().unwrap() = Caps { profiles: true, energy_preference: true, sleep: true, t2: false, laptop: true, lid: true };
}

#[test]
fn a_profile_is_written_to_the_processor_saved_and_audited() {
    let p = FakeSys::new(true);
    laptop(&p);
    let mut a = app();
    a.handle_power(&Key::Char('e'), &p);
    assert_eq!(p.power_current(&PowerEnv::at("x")), Some(Profile::Performance));
    assert_eq!(p.settings_get().power.profile, Profile::Performance);
    assert_eq!(p.audited(), vec![("settings.power".to_string(), "profile=Performance".to_string())]);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Power profile")));
}

#[test]
fn a_profile_the_machine_refuses_is_shown_and_not_saved() {
    let p = FakeSys::new(true);
    laptop(&p);
    *p.refuse.lock().unwrap() = Some("cpufreq is not writable".into());
    let mut a = app();
    a.handle_power(&Key::Char('v'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not writable")));
    assert_eq!(p.settings_get().power.profile, iyakki_shared::power::PowerSettings::default().profile);
    assert!(p.audited().is_empty());
}

#[test]
fn others_change_nothing_and_the_refusal_is_audited() {
    let p = FakeSys::new(false);
    laptop(&p);
    let mut a = app();
    for k in ['e', 's', 'l', 'p', 'w', 'f'] {
        a.handle_power(&Key::Char(k), &p);
    }
    assert_eq!(p.settings_get().power, iyakki_shared::power::PowerSettings::default());
    assert_eq!(p.audited().len(), 6);
    assert!(p.audited().iter().all(|l| l.0 == "settings.denied"));
}

#[test]
fn the_sleep_lid_button_and_busy_cards_save_through_the_port_and_say_what_they_do() {
    let p = FakeSys::new(true);
    laptop(&p);
    let mut a = app();
    a.handle_power(&Key::Char('s'), &p);
    let m = p.settings_get().power.sleep_after_min;
    assert_ne!(m, iyakki_shared::power::PowerSettings::default().sleep_after_min);
    a.handle_power(&Key::Char('l'), &p);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("lid")));
    let before = p.settings_get().power.button;
    a.handle_power(&Key::Char('p'), &p);
    assert_ne!(p.settings_get().power.button, before);
    let busy = p.settings_get().power.sleep_when_busy;
    a.handle_power(&Key::Char('w'), &p);
    assert_eq!(p.settings_get().power.sleep_when_busy, !busy);
    let kinds: Vec<String> = p.audited().into_iter().map(|l| l.1.split('=').next().unwrap_or("").to_string()).collect();
    assert_eq!(kinds, vec!["sleep_after_min", "lid", "button", "sleep_when_busy"]);
}

#[test]
fn a_card_without_the_hardware_says_why_and_changes_nothing() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_power(&Key::Char('s'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot sleep")));
    a.handle_power(&Key::Char('l'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no lid switch")));
    a.handle_power(&Key::Char('x'), &p);
    assert!(p.audited().is_empty());
}

#[test]
fn the_fan_card_goes_through_the_port_and_is_audited() {
    let p = FakeSys::new(true);
    *p.fan.lock().unwrap() = Some(false);
    let mut a = app();
    a.handle_power(&Key::Char('f'), &p);
    assert_eq!(*p.fan.lock().unwrap(), Some(true));
    assert_eq!(p.audited(), vec![("settings.fan".to_string(), "on".to_string())]);
    *p.refuse.lock().unwrap() = Some("no fan service".into());
    a.handle_power(&Key::Char('f'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no fan service")));
    assert_eq!(p.audited().len(), 1);
}

#[test]
fn busy_settings_are_told_and_nothing_is_audited() {
    let p = FakeSys::new(true);
    laptop(&p);
    p.busy.store(true, Ordering::SeqCst);
    let mut a = app();
    a.handle_power(&Key::Char('b'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
    assert!(p.audited().is_empty());
}

#[test]
fn the_page_draws_the_state_of_the_port_at_several_sizes_with_and_without_the_hardware() {
    for (hw, fan) in [(true, Some(true)), (false, None)] {
        let p = FakeSys::new(true);
        if hw {
            laptop(&p);
            p.caps.lock().unwrap().t2 = true;
        }
        *p.fan.lock().unwrap() = fan;
        let mut a = app();
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            a.render_power(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
            assert_eq!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('b'))), hw, "{w}x{h}: the balanced card is clickable only on a machine that can change its speed");
            assert!(g.hits.iter().all(|x| x.rect.x >= -0.5 && x.rect.x + x.rect.w <= w as f32 + 0.5), "{w}x{h}: inside the width");
        }
    }
}
