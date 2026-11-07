// #[cfg(test)] -- test code, included by settings.rs under #[cfg(test)].
//! The AI hardware profile in Settings (docs/AI-HARDWARE.md): the "This machine" row of Models at every size, and the administrator's
//! "Show all AI tools" switch on the Cloud mode page (key A): who may press it, what it saves and what it audits.
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_sec::policy::{Policy, Role};

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> (Gfx, R) {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
    a.render(&mut g, area, s, cx, 0);
    (g, area)
}

fn audit(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default()
}

#[test]
fn the_models_page_draws_the_this_machine_row_at_every_size_inside_the_page_and_above_the_providers() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let cx = cx_with_admin_store(&format!("aihw-models-{w}"));
        let mut a = SettingsApp::new(&cx).on(Tab::Models);
        let (g, area) = draw(&mut a, &cx, w, h);
        let row = g.hits.iter().find(|x| x.label.starts_with("This machine: ")).unwrap_or_else(|| panic!("{w}x{h}: no 'This machine' row: {:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>()));
        assert!(row.label.contains("fits: "), "{}", row.label);
        let r = row.rect;
        assert!(r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.x + r.w <= area.x + area.w + 0.5 && r.y + r.h <= area.y + area.h + 0.5 && r.h > 0.0, "{w}x{h}: {r:?} inside {area:?}");
        // the provider rows and the empty-state start below it
        assert!(!g.hits.iter().any(|x| x.label != row.label && x.rect.y < r.y + r.h - 0.5 && r.y < x.rect.y + x.rect.h - 0.5 && !x.label.is_empty() && x.rect.w < r.w - 1.0 && x.rect.x >= r.x && x.rect.x + x.rect.w <= r.x + r.w), "{w}x{h}: another control sits on the row");
    }
}

#[test]
fn key_a_switches_show_all_ai_tools_for_an_administrator_saves_it_and_audits_each_change() {
    let cx = cx_with_admin_store("aihw-a");
    let mut a = SettingsApp::new(&cx).on(Tab::Cloud);
    assert!(!cx.services.show_all_ai(), "off by default");
    key(&mut a, &cx, Key::Char('a'));
    assert!(cx.services.show_all_ai());
    assert!(status(&a).starts_with("Show all AI tools: on."), "{}", status(&a));
    assert!(audit(&cx).contains("settings.show_all_ai") && audit(&cx).contains("\"detail\":\"on\""), "{}", audit(&cx));
    // the page says so
    assert!(a.cloud_rows(&cx).iter().any(|(l, v)| l == "Show all AI tools" && v.starts_with("On (press a to change)")));
    key(&mut a, &cx, Key::Char('a'));
    assert!(!cx.services.show_all_ai());
    assert!(status(&a).starts_with("Show all AI tools: off."), "{}", status(&a));
    assert_eq!(audit(&cx).matches("settings.show_all_ai").count(), 2, "one audit line per change");
    assert!(audit(&cx).contains("\"detail\":\"off\""));
    // the setting survives a restart of the settings store (it is in the machine's settings file)
    key(&mut a, &cx, Key::Char('a'));
    assert!(cx.services.settings.lock().unwrap().get().show_all_ai);
}

#[test]
fn key_a_does_nothing_for_someone_who_is_not_an_administrator() {
    let mut cx = cx_with_admin_store("aihw-a-user");
    cx.session.role = Role::User;
    cx.session.policy = Policy::resolve(Role::User, &Default::default());
    let mut a = SettingsApp::new(&cx).on(Tab::Cloud);
    key(&mut a, &cx, Key::Char('a'));
    assert!(!cx.services.show_all_ai());
    assert!(!audit(&cx).contains("settings.show_all_ai"));
}

#[test]
fn key_c_cycles_the_cloud_mode_setting_for_an_administrator_saves_it_and_audits_each_change() {
    use iyakki_agent::cloudmode::Setting;
    let cx = cx_with_admin_store("cloud-c");
    let mut a = SettingsApp::new(&cx).on(Tab::Cloud);
    assert_eq!(cx.services.cloud_setting(), Setting::Auto, "automatic by default");
    let mut seen = vec![];
    for _ in 0..3 {
        key(&mut a, &cx, Key::Char('c'));
        seen.push(cx.services.cloud_setting());
        assert!(status(&a).starts_with(&format!("Cloud mode: {}.", cx.services.cloud_setting().label())), "{}", status(&a));
    }
    assert_eq!(seen, [Setting::On, Setting::Off, Setting::Auto], "Automatic, On, Off and round again");
    assert_eq!(audit(&cx).matches("settings.cloud").count(), 3, "one audit line per change");
    assert!(a.cloud_rows(&cx).iter().any(|(l, v)| l == "Setting" && v == Setting::Auto.label()));
    // Enter and Space do the same; the choice is in the machine's settings file
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.services.cloud_setting(), Setting::On);
    key(&mut a, &cx, Key::Char(' '));
    assert_eq!(cx.services.cloud_setting(), Setting::Off);
    assert_eq!(cx.services.settings.lock().unwrap().get().cloud, Setting::Off);
    // the page draws at the four sizes with the new value
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let (g, _) = draw(&mut a, &cx, w, h);
        assert_ne!(g.rgba(), Gfx::new(w, h).rgba(), "{w}x{h}");
    }
}

#[test]
fn key_c_does_nothing_for_someone_who_is_not_an_administrator() {
    use iyakki_agent::cloudmode::Setting;
    let mut cx = cx_with_admin_store("cloud-c-user");
    cx.session.role = Role::User;
    cx.session.policy = Policy::resolve(Role::User, &Default::default());
    let mut a = SettingsApp::new(&cx).on(Tab::Cloud);
    key(&mut a, &cx, Key::Char('c'));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.services.cloud_setting(), Setting::Auto);
    assert!(!audit(&cx).contains("settings.cloud"));
}

#[test]
fn key_r_switches_tool_routing_for_the_person_only_audits_it_and_the_row_fits_at_four_sizes() {
    let cx = cx_with_admin_store("routing-r");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    let dir = cx.services.dir().to_path_buf();
    assert!(!iyakki_agent::toolrouting::person_off(&dir, "root"), "off by default: no override");
    key(&mut a, &cx, Key::Char('r'));
    assert!(iyakki_agent::toolrouting::person_off(&dir, "root"));
    assert!(status(&a).contains("off for you"), "{}", status(&a));
    assert!(audit(&cx).contains("settings.tool_routing_own") && audit(&cx).contains("\"detail\":\"off\""), "{}", audit(&cx));
    assert!(!cx.services.settings.lock().unwrap().get().no_tool_routing, "the machine-wide switch is not touched");
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let (g, _) = draw(&mut a, &cx, w, h);
        assert_ne!(g.rgba(), Gfx::new(w, h).rgba(), "{w}x{h}");
    }
    key(&mut a, &cx, Key::Char('r'));
    assert!(!iyakki_agent::toolrouting::person_off(&dir, "root"));
    assert_eq!(audit(&cx).matches("settings.tool_routing_own").count(), 2);
    // an ordinary user may do it for themselves; it never turns the machine-wide switch back on
    let mut ucx = cx_with_admin_store("routing-r-user");
    ucx.session.role = Role::User;
    ucx.session.policy = Policy::resolve(Role::User, &Default::default());
    let mut u = SettingsApp::new(&ucx).on(Tab::Models);
    key(&mut u, &ucx, Key::Char('r'));
    assert!(iyakki_agent::toolrouting::person_off(ucx.services.dir(), &ucx.session.user));
    assert!(!ucx.services.settings.lock().unwrap().get().no_tool_routing);
}
