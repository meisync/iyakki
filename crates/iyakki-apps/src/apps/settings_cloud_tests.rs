//! Settings -> Cloud mode on a fake port: the mode setting goes round (administrators only, audited, a refusal is shown), "show all AI tools"
//! switches and is audited, and the page draws the rows of the port at several sizes. No sleeps, no real services.

use super::system_port::{CloudPort, FakeSys};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_cloudmode::Setting;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("cloud-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Cloud);
    a
}

#[test]
fn the_mode_goes_to_the_next_setting_and_is_audited() {
    let p = FakeSys::new(true);
    let mut a = app();
    let first = p.cloud_setting();
    a.handle_cloud(&Key::Char('c'), &p);
    assert_eq!(p.cloud_setting(), first.next());
    assert_eq!(p.audited(), vec![("settings.cloud".to_string(), first.next().label().to_string())]);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Cloud mode")));
    // Enter and space do the same
    a.handle_cloud(&Key::Enter, &p);
    assert_eq!(p.cloud_setting(), first.next().next());
}

#[test]
fn a_person_who_is_not_an_administrator_changes_nothing_and_the_refusal_is_audited() {
    let p = FakeSys::new(false);
    let mut a = app();
    a.handle_cloud(&Key::Char('c'), &p);
    a.handle_cloud(&Key::Char('a'), &p);
    assert_eq!(p.cloud_setting(), Setting::Auto);
    assert!(!p.show_all_ai());
    assert!(p.audited().iter().all(|l| l.0 == "settings.denied"), "{:?}", p.audited());
    assert!(!p.audited().is_empty());
}

#[test]
fn show_all_ai_tools_switches_on_and_off_and_says_what_it_does() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_cloud(&Key::Char('a'), &p);
    assert!(p.show_all_ai());
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("not recommended")));
    a.handle_cloud(&Key::Char('a'), &p);
    assert!(!p.show_all_ai());
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("hidden again")));
    assert_eq!(p.audited(), vec![("settings.show_all_ai".to_string(), "on".to_string()), ("settings.show_all_ai".to_string(), "off".to_string())]);
}

#[test]
fn a_refusal_of_the_service_is_shown_and_nothing_is_audited() {
    let p = FakeSys::new(true);
    *p.refuse.lock().unwrap() = Some("the settings file is read-only".to_string());
    let mut a = app();
    a.handle_cloud(&Key::Char('c'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("read-only")));
    a.handle_cloud(&Key::Char('a'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("read-only")));
    assert!(p.audited().is_empty());
}

#[test]
fn the_rows_come_from_the_port_and_the_page_draws_at_several_sizes() {
    let p = FakeSys::new(true);
    p.show_ai.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut a = app();
    let rows = a.cloud_rows(&p);
    assert!(rows.iter().any(|(l, v)| l == "This machine (AI)" && v == "fake machine"));
    assert!(rows.iter().any(|(l, v)| l == "Show all AI tools" && v.starts_with("On ")));
    assert!(rows.iter().any(|(l, v)| l == "Setting" && v == Setting::Auto.label()));
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_cloud(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}
