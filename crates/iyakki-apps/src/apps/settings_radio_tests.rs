//! Settings -> Radio: reconnect tries and moving on to the next station (F-RADIO-17): the page saves and audits both choices, only
//! administrators change them, the same settings are what the chat tool changes, and the page draws at six screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_sec::policy::Role;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn cfg(cx: &AppCx) -> iyakki_shared::radiorecover::RadioRecover {
    cx.settings().lock().unwrap().get().radio.clone()
}

fn key(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

#[test]
fn the_page_is_under_apps_and_ai_and_the_search_finds_it() {
    let cx = admin("rr-find");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs.contains(&Tab::Radio));
    assert_eq!(Tab::Radio.group(), "Apps & AI");
    a.open_item("tab:radio", &cx);
    assert_eq!(a.tab(), Tab::Radio);
    assert!(Tab::Radio.keywords().contains("reconnect") && Tab::Radio.keywords().contains("next station"));
}

#[test]
fn the_choices_start_at_two_tries_and_switching_on_and_save_with_an_audit_line() {
    let cx = admin("rr-save");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Radio);
    assert_eq!((cfg(&cx).retries(), cfg(&cx).auto_switch()), (2, true));
    key(&mut a, &cx, '+');
    assert_eq!(cfg(&cx).retries(), 3);
    key(&mut a, &cx, '-');
    key(&mut a, &cx, '-');
    key(&mut a, &cx, '-');
    key(&mut a, &cx, '-');
    assert_eq!(cfg(&cx).retries(), 0, "never below none");
    for _ in 0..7 {
        key(&mut a, &cx, 'r');
    }
    assert_eq!(cfg(&cx).retries(), 1, "r goes round 0 to 5 and back");
    key(&mut a, &cx, 'a');
    assert!(!cfg(&cx).auto_switch());
    key(&mut a, &cx, 'a');
    assert!(cfg(&cx).auto_switch());
    assert!(cx.audit.recent(50).iter().any(|l| l.contains("settings.radio") && l.contains("auto_switch=false")), "{:?}", cx.audit.recent(50));
}

#[test]
fn only_administrators_change_them() {
    let mut cx = admin("rr-user");
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Radio);
    key(&mut a, &cx, '+');
    key(&mut a, &cx, 'a');
    assert_eq!((cfg(&cx).retries(), cfg(&cx).auto_switch()), (2, true));
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")), "{:?}", a.status());
}

#[test]
fn the_page_draws_at_six_sizes_with_both_switches_clickable() {
    let cx = admin("rr-sizes");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Radio);
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &cx, 0);
        for k in ['r', 'a'] {
            assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char(k))), "{w}x{h}: the switch for {k}");
        }
    }
}

// ---- the page on a fake port (no AppCx for the page's own logic: the settings, the audit log and the administrator switch are in memory) ----

mod on_a_fake_port {
    use super::*;
    use crate::apps::settings_port::{FakeSettingsPort, SettingsDataPort};
    use std::sync::atomic::Ordering;

    fn app() -> SettingsApp {
        let cx = admin("rr-fake");
        let mut a = SettingsApp::new(&cx);
        a.go_to(Tab::Radio);
        a
    }

    #[test]
    fn the_choices_save_into_the_fake_settings_and_are_audited() {
        let p = FakeSettingsPort::new(true);
        let mut a = app();
        a.handle_radio(&Key::Char('+'), &p);
        assert_eq!(p.settings_get().radio.retries(), 3);
        a.handle_radio(&Key::Char('a'), &p);
        assert!(!p.settings_get().radio.auto_switch());
        assert_eq!(p.audited(), vec![("settings.radio".to_string(), "retries=3".to_string()), ("settings.radio".to_string(), "auto_switch=false".to_string())]);
        assert!(a.status().is_some_and(|s| s.1 && s.0 == "Saved."));
    }

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing_and_the_refusal_is_audited() {
        let p = FakeSettingsPort::new(false);
        let mut a = app();
        a.handle_radio(&Key::Char('+'), &p);
        assert_eq!(p.settings_get().radio.retries(), 2);
        assert_eq!(p.audited(), vec![("settings.denied".to_string(), a.tab().label().to_string())]);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")));
    }

    #[test]
    fn busy_settings_are_told_and_nothing_is_audited() {
        let p = FakeSettingsPort::new(true);
        p.busy.store(true, Ordering::SeqCst);
        let mut a = app();
        a.handle_radio(&Key::Char('-'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
        assert!(p.audited().is_empty());
        assert_eq!(p.settings_get().radio.retries(), 2);
    }

    #[test]
    fn the_page_draws_from_the_fake_settings() {
        let p = FakeSettingsPort::new(true);
        let mut a = app();
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(0, 0, 0));
        a.render_radio(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &p);
        for k in ['r', 'a'] {
            assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char(k))), "the switch for {k}");
        }
    }
}
