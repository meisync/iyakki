//! Settings -> System updates on a fake port: the update-check mode is changed through the settings port by an administrator and audited,
//! a non-administrator changes nothing, busy settings are told and not audited, and the page draws at several sizes with the store facets
//! of the fake. No sleeps, no real store, no real update.

use super::super::settings_port::SettingsDataPort;
use super::system_port::{FakeSys, UpdatesPort};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use std::sync::atomic::Ordering;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("updates-port");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Updates);
    a
}

#[test]
fn an_administrator_changes_the_update_check_through_the_port_and_it_is_audited() {
    let p = FakeSys::new(true);
    let before = p.settings_get().update_check.mode;
    let mut a = app();
    a.handle_updates(&Key::Char('m'), &p);
    assert_ne!(p.settings_get().update_check.mode, before, "the mode moved");
    assert!(p.audited().iter().any(|l| l.0 == "settings.update_check"));
    assert!(a.status().is_some_and(|s| s.1));
}

#[test]
fn a_non_administrator_changes_nothing_about_the_update_check() {
    let p = FakeSys::new(false);
    let before = p.settings_get().update_check.mode;
    let mut a = app();
    a.handle_updates(&Key::Char('m'), &p);
    assert_eq!(p.settings_get().update_check.mode, before);
    assert!(!p.audited().iter().any(|l| l.0 == "settings.update_check"));
}

#[test]
fn busy_settings_are_told_and_nothing_is_audited() {
    let p = FakeSys::new(true);
    p.busy.store(true, Ordering::SeqCst);
    let mut a = app();
    a.handle_updates(&Key::Char('m'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
    assert!(!p.audited().iter().any(|l| l.0 == "settings.update_check"));
}

#[test]
fn the_port_supplies_the_user_and_the_store_facets_the_page_reads() {
    let p = FakeSys::new(true);
    assert_eq!(p.user_name(), "owner");
    assert!(p.policy().manage_settings);
    assert!(!FakeSys::new(false).policy().manage_settings);
}

#[test]
fn the_page_draws_from_the_port_at_several_sizes() {
    let p = FakeSys::new(true);
    let mut a = app();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_updates(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}
