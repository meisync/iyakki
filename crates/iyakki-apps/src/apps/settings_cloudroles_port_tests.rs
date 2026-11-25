//! Settings -> Cloud access on a fake port: the first owner is made and saved and audited, a non-administrator changes nothing, a refused
//! save is shown and leaves the trail alone, and the page draws at several sizes with a trail and a member list. No sleeps, no files:
//! the roles lists and the trail are in memory in the fake.

use super::account_port::{CloudRolesPort, FakeAcct};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_shared::cloudroles::{Roles, TenantRole};

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("cloudroles-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::CloudRoles);
    a
}

#[test]
fn the_first_owner_is_made_saved_and_audited() {
    let p = FakeAcct::new(true);
    let mut a = app();
    a.handle_cloudroles(&Key::Char('o'), &p);
    let saved = p.roles_load().unwrap().expect("the lists are saved");
    assert_eq!(saved.tenant_role("owner"), Some(TenantRole::Owner));
    assert!(p.audited().iter().any(|l| l.0 == "settings.cloud_roles" && l.1.contains("first owner")));
    assert_eq!(p.trail.lock().unwrap().len(), 1, "the change is on the trail");
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("owner")));
}

#[test]
fn a_non_administrator_changes_nothing() {
    let p = FakeAcct::new(false);
    let mut a = app();
    a.handle_cloudroles(&Key::Char('o'), &p);
    assert!(p.roles_load().unwrap().is_none(), "the lists stay empty");
    assert!(p.trail.lock().unwrap().is_empty());
}

#[test]
fn a_refused_save_is_shown_and_the_trail_is_not_written() {
    let p = FakeAcct::new(true);
    *p.refuse.lock().unwrap() = Some("the disk is read-only".into());
    let mut a = app();
    a.handle_cloudroles(&Key::Char('o'), &p);
    assert!(p.roles_load().unwrap().is_none());
    assert!(p.trail.lock().unwrap().is_empty());
    assert!(a.status().is_some_and(|s| !s.1));
}

#[test]
fn the_page_draws_the_owner_and_the_trail_at_several_sizes() {
    let p = FakeAcct::new(true);
    *p.roles_in_memory.lock().unwrap() = Ok(Some(Roles::new("owner").unwrap()));
    p.trail.lock().unwrap().push("1000000 tenant: owner made themselves the first owner".into());
    let mut a = app();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_cloudroles(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}

#[test]
fn the_page_says_when_the_machine_has_no_roles_yet() {
    let p = FakeAcct::new(true);
    let mut a = app();
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    a.render_cloudroles(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &p);
    assert_eq!(a.roles_len(&p), 0);
}
