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
