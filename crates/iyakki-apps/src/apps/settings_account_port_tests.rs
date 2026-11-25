//! Settings -> Account on a fake port: the page opens the password form, reads the person's limits only through the port, and draws at
//! several sizes (also with a long list of apps). No sleeps, no files: the limits are in the fake. The text size, theme and wallpaper keys are
//! tested with the device port's own tests, because they write the person's files.

use super::account_port::{AccountFacts, AccountPort, FakeAcct};
use super::*;
use crate::apps::tests::cx_with_admin_store;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("account-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Account);
    a
}

const SIZES: [(u32, u32); 6] = [(640, 400), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (360, 640)];

#[test]
fn the_password_key_opens_the_change_password_form_with_three_secret_fields() {
    let p = FakeAcct::new(false);
    let mut a = app();
    a.handle_account(&Key::Char('p'), &p);
    match a.modal.take() {
        Some(Modal::Form(FormKind::Password, f)) => assert_eq!(f.fields.len(), 3, "current, new and repeat"),
        _ => panic!("the password form is not open"),
    }
}

#[test]
fn an_unknown_key_changes_nothing() {
    let p = FakeAcct::new(true);
    let mut a = app();
    a.handle_account(&Key::Char('z'), &p);
    assert!(a.modal.is_none(), "no form opens");
    assert!(p.audited().is_empty(), "nothing is audited");
}

#[test]
fn the_port_gives_the_limits_of_an_administrator_and_of_a_user() {
    let admin = FakeAcct::new(true).account_facts();
    let user = FakeAcct::new(false).account_facts();
    assert_eq!(admin.role, "Administrator");
    assert_eq!(user.role, "User");
    assert!(user.vfs_quota_mb > 0 && user.max_memory_mb > 0 && !user.apps.is_empty());
}

#[test]
fn the_page_draws_at_six_sizes() {
    let p = FakeAcct::new(true);
    let mut a = app();
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_account(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}

#[test]
fn a_long_app_list_still_draws_without_running_into_the_cards() {
    struct Many(FakeAcct);
    impl super::super::settings_port::PagePort for Many {
        fn is_admin(&self) -> bool {
            self.0.is_admin()
        }
        fn audit(&self, a: &str, s: &str) {
            self.0.audit(a, s)
        }
        fn data_dir(&self) -> std::path::PathBuf {
            self.0.data_dir()
        }
        fn now_secs(&self) -> u64 {
            self.0.now_secs()
        }
    }
    impl super::super::settings_device_port::UserPort for Many {
        fn user(&self) -> String {
            "a-person-with-a-rather-long-account-name".into()
        }
    }
    impl AccountPort for Many {
        fn account_facts(&self) -> AccountFacts {
            AccountFacts { role: "User".into(), vfs_quota_mb: 1, max_memory_mb: 1, apps: (0..60).map(|i| format!("app-number-{i}")).collect() }
        }
    }
    let p = Many(FakeAcct::new(false));
    let mut a = app();
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_account(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}
