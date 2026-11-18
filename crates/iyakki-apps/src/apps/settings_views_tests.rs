//! Settings -> Desktop views: views made, renamed, put in order, locked and deleted, a view from a template, the step keys and the
//! "after the lock" choice, the limit, who sees it (everyone, for their own views) and the page at four sizes.

use super::*;
use super::views_page::VwRow;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::views::{KeySet, Views, MAX_VIEWS};

fn user(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn views(cx: &AppCx) -> Views {
    Views::load(cx.data_dir(), &cx.session.user)
}
fn names(cx: &AppCx) -> Vec<String> {
    views(cx).views.iter().map(|v| v.name.clone()).collect()
}
fn select(a: &mut SettingsApp, cx: &AppCx, name: &str) {
    let i = SettingsApp::vw_rows(cx).iter().position(|r| matches!(r, VwRow::View { name: n, .. } if n == name)).unwrap_or_else(|| panic!("no row {name}"));
    a.sel[a.tab] = i;
}

#[test]
fn everyone_has_the_page_and_it_makes_renames_orders_locks_and_deletes_views() {
    let cx = user("vw1");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Views) && Tab::ORDER.contains(&Tab::Views));
    a.go_to(Tab::Views);
    assert_eq!(names(&cx), ["Main"]);
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "main");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a name in use is refused and the form stays");
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "Docs");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal() && status(&a).contains("Docs"), "{}", status(&a));
    assert_eq!(names(&cx), ["Main", "Docs"]);
    // rename
    select(&mut a, &cx, "Docs");
    key(&mut a, &cx, Key::Char('r'));
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "Notes");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(names(&cx), ["Main", "Notes"]);
    // order: earlier
    select(&mut a, &cx, "Notes");
    key(&mut a, &cx, Key::Char('['));
    assert_eq!(names(&cx), ["Notes", "Main"]);
    // lock on and off
    select(&mut a, &cx, "Notes");
    key(&mut a, &cx, Key::Char('l'));
    assert!(views(&cx).views[0].locked && status(&a).contains("password"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('l'));
    assert!(!views(&cx).views[0].locked);
    // delete asks first
    select(&mut a, &cx, "Notes");
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(names(&cx), ["Main"]);
    // the last view stays
    select(&mut a, &cx, "Main");
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(names(&cx), ["Main"]);
    assert!(status(&a).contains("last view"), "{}", status(&a));
}

#[test]
fn a_view_from_a_template_gets_its_apps_and_a_free_name() {
    let cx = user("vw2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Views);
    key(&mut a, &cx, Key::Char('t'));
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Enter); // the first template: Work
    assert_eq!(names(&cx), ["Main", "Work"]);
    assert_eq!(views(&cx).views[1].tiles, ["edit", "cli"]);
    key(&mut a, &cx, Key::Char('t'));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(names(&cx), ["Main", "Work", "Work 2"], "{}", status(&a));
}

#[test]
fn the_persons_own_templates_are_in_the_list_after_the_built_in_ones_and_make_views() {
    let cx = user("vw-own");
    let mut ut = iyakki_shared::views::UserTemplates::default();
    ut.save_template("Work 2", &["radio".to_string(), "edit".to_string()]).unwrap(); // a name that starts like a built-in one
    assert!(ut.save(cx.data_dir(), &cx.session.user));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Views);
    key(&mut a, &cx, Key::Char('t'));
    assert!(a.has_modal());
    // the built-in templates come first; walk down to the last row, which is the person's own
    for _ in 0..iyakki_shared::views::TEMPLATES.len() {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Enter);
    assert_eq!(names(&cx), ["Main", "Work 2"], "{}", status(&a));
    assert_eq!(views(&cx).views[1].tiles, ["radio", "edit"], "the own template's apps, not the built-in Work");
    // the built-in first row still makes the built-in view
    key(&mut a, &cx, Key::Char('t'));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(views(&cx).views[2].tiles, ["edit", "cli"]);
}

#[test]
fn the_step_keys_and_the_after_the_lock_choice_are_saved() {
    let cx = user("vw3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Views);
    assert_eq!(views(&cx).key_set, KeySet::Both);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(views(&cx).key_set, KeySet::Browser);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(views(&cx).key_set, KeySet::Mac);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(views(&cx).key_set, KeySet::Both);
    assert!(!views(&cx).reset_on_lock);
    key(&mut a, &cx, Key::Char('o'));
    assert!(views(&cx).reset_on_lock && status(&a).contains("first view"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('o'));
    assert!(!views(&cx).reset_on_lock);
}

#[test]
fn the_page_stops_at_the_limit_and_draws_at_four_sizes() {
    let cx = user("vw4");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Views);
    for i in 1..MAX_VIEWS {
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, &format!("V{i}"));
        key(&mut a, &cx, Key::Enter);
    }
    assert_eq!(names(&cx).len(), MAX_VIEWS);
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "One more");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal() && names(&cx).len() == MAX_VIEWS, "refused at the limit");
    key(&mut a, &cx, Key::Esc);
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let mut g = Gfx::new(w, h);
        let s = crate::ui::scale_for(w, h);
        a.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), s, &cx, 0);
        assert!(g.hits.iter().any(|x| x.label == "New view"), "buttons at {w}x{h}");
    }
}

#[test]
fn the_page_switches_the_kept_arrangement_and_the_one_window_rule_and_audits_both() {
    let cx = user("vw-layout-keys");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Views);
    let rows = SettingsApp::vw_rows(&cx);
    assert!(rows.contains(&VwRow::RememberLayout) && rows.contains(&VwRow::OneWindow));
    assert!(views(&cx).remember_layout && views(&cx).single_apps.is_empty(), "arrangement kept, one window off, by default");
    key(&mut a, &cx, Key::Char('y'));
    assert!(!views(&cx).remember_layout, "{}", status(&a));
    key(&mut a, &cx, Key::Char('y'));
    assert!(views(&cx).remember_layout);
    key(&mut a, &cx, Key::Char('i'));
    assert!(views(&cx).is_single("calendar") && views(&cx).is_single("anything"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('i'));
    assert!(views(&cx).single_apps.is_empty());
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(audit.contains("view.layout") && audit.contains("view.single"), "{audit}");
    let g = {
        let mut g = Gfx::new(1280, 720);
        a.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &cx, 0);
        g
    };
    assert!(g.text_log.iter().any(|(_, _, t)| t == "Window arrangement") && g.text_log.iter().any(|(_, _, t)| t == "One window per app"));
}
