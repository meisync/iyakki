//! Settings -> Backup on a fake port: the machine's own backup is refused to a non-administrator and offered to an administrator (the
//! form opens), stopping with no backup running says so, and the page draws at several sizes from the port's home folder. The folders
//! are temporary ones of the test. No sleeps, no real backup is written.

use super::account_port::{BackupPort, FakeAcct};
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("backup-port");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Backup);
    a
}

fn fake(tag: &str, admin: bool) -> FakeAcct {
    let mut p = FakeAcct::new(admin);
    p.dir = temp_dir(tag);
    p
}

#[test]
fn the_machine_backup_is_refused_to_a_non_administrator() {
    let p = fake("backup-refuse", false);
    assert!(!p.may_manage_settings());
    let mut a = app();
    a.handle_backup(&Key::Char('y'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrator")));
    assert!(a.modal.is_none());
}

#[test]
fn the_machine_backup_form_opens_for_an_administrator() {
    let p = fake("backup-admin", true);
    let mut a = app();
    a.handle_backup(&Key::Char('y'), &p);
    assert!(a.modal.is_some(), "the form to choose a folder is open");
}

#[test]
fn stopping_with_no_backup_running_says_so() {
    let p = fake("backup-stop", true);
    let mut a = app();
    a.handle_backup(&Key::Char('s'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No backup is running")));
}

#[test]
fn the_page_draws_from_the_ports_home_folder_at_several_sizes() {
    let p = fake("backup-draw", true);
    assert_eq!(p.backup_home(), p.dir.join("home"));
    let mut a = app();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_backup(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}
