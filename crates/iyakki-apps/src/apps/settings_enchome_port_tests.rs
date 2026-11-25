//! Settings -> Encrypted home on a fake port: the sign-in check goes through the port (a wrong password is refused, the right one is
//! accepted), starting a form opens it from the port's own name, and the page draws at several sizes with the home not set up. The
//! folder is a temporary one of the test. No sleeps, no real store.

use super::account_port::{EncHomePort, FakeAcct};
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("enchome-port");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::EncHome);
    a
}

fn fake(tag: &str) -> FakeAcct {
    let mut p = FakeAcct::new(true);
    p.dir = temp_dir(tag);
    p
}

#[test]
fn the_sign_in_check_is_the_ports_and_refuses_a_wrong_password() {
    let p = fake("enc-signin");
    assert!(p.verify_sign_in("owner", "correct horse"));
    assert!(!p.verify_sign_in("owner", "wrong"));
    assert!(!p.verify_sign_in("someone-else", "correct horse"));
    let check = p.verifier();
    assert!(check("owner", "correct horse"));
    assert!(!check("owner", "wrong"));
}

#[test]
fn making_the_home_opens_its_form_for_the_port_person() {
    let p = fake("enc-create");
    let mut a = app();
    a.handle_ench(&Key::Char('c'), &p);
    assert!(a.modal.is_some(), "the create form is open");
    assert_eq!(p.user_name(), "owner");
}

#[test]
fn the_page_draws_with_the_home_not_set_up_at_several_sizes() {
    let p = fake("enc-draw");
    let mut a = app();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_ench(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
    }
}
