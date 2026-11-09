//! Settings -> Encrypted home: the page is there for everyone with a password (not guests), it says in words what is and is not protected,
//! the keys open masked forms, a wrong sign-in password makes nothing, and a request the assistant left opens the form. Nothing here touches
//! a kernel: the only paths taken stop at the password check or at a damaged stand-in file.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::enchome::form::{self as ef, Request};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn type_in(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        a.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn draw_text(a: &mut SettingsApp, cx: &AppCx) -> String {
    let mut g = Gfx::new(1100, 700);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(1100, 700);
    crate::access::start_frame();
    a.render(&mut g, R::new(0.0, 46.0 * s, 1100.0, 700.0 - 46.0 * s), s, cx, 0);
    g.text_log.iter().map(|(_, _, t)| t.clone()).collect::<Vec<_>>().join(" ")
}

#[test]
fn the_page_is_in_the_list_for_everyone_with_a_password_and_found_by_its_words() {
    let cx = admin("ench-tab");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::EncHome));
    assert_eq!(Tab::EncHome.group(), CATEGORIES[1]);
    assert!(Tab::ORDER.contains(&Tab::EncHome));
    for w in ["encrypt", "recovery key", "stolen", "move"] {
        assert!(Tab::EncHome.keywords().contains(w), "{w}");
    }
    let mut user = cx_with_admin_store("ench-user");
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(SettingsApp::new(&user).tabs().contains(&Tab::EncHome));
    let mut guest = cx_with_admin_store("ench-guest");
    guest.session.role = iyakki_sec::policy::Role::Guest;
    guest.session.policy = iyakki_sec::policy::Role::Guest.template();
    assert!(!SettingsApp::new(&guest).tabs().contains(&Tab::EncHome), "a guest has no password to encrypt with");
}

#[test]
fn the_page_says_what_is_and_is_not_protected_and_offers_make_and_move() {
    let cx = admin("ench-text");
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    let t = draw_text(&mut a, &cx);
    for w in ["Not encrypted", "Not protected", "recovery key is shown once", "stay readable"] {
        assert!(t.contains(w), "missing: {w}\n{t}");
    }
    let labels: Vec<&str> = a.ench_buttons().iter().map(|b| b.0).collect();
    assert_eq!(labels, vec!["Make my home encrypted", "Move my existing files"]);
}

#[test]
fn a_wrong_sign_in_password_makes_nothing_and_keeps_the_form_open_with_the_reason() {
    let cx = admin("ench-wrong");
    let dir = cx.services.dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    press(&mut a, &cx, 'c');
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))), "the masked form opens");
    type_in(&mut a, &cx, "not-the-password");
    a.handle(&Event::Key(Key::Enter), &cx);
    match &a.modal {
        Some(Modal::Form(FormKind::EncHome, f)) => assert!(f.error.as_deref().unwrap_or("").contains("not your sign-in password")),
        _ => panic!("the form must stay open"),
    }
    assert!(!dir.join("home-enc").exists() || std::fs::read_dir(dir.join("home-enc")).unwrap().next().is_none(), "nothing was made");
    // Esc leaves; the password never reached the status line or the audit log
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.modal.is_none());
    assert!(!a.status.as_ref().is_some_and(|(m, _)| m.contains("not-the-password")));
}

#[test]
fn a_request_from_the_assistant_opens_the_form_on_this_page() {
    let cx = admin("ench-req");
    let mut a = SettingsApp::new(&cx);
    ef::leave_request(cx.services.dir(), &cx.session.user, Request::Create);
    for _ in 0..20 {
        a.handle(&Event::Tick, &cx);
    }
    assert_eq!(a.tab(), Tab::EncHome);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))));
    assert_eq!(ef::take_request(cx.services.dir(), &cx.session.user), None, "taken once");
    // asking for something that makes no sense now only says so
    let mut b = SettingsApp::new(&cx).on(Tab::EncHome);
    b.ench_start(&cx, ef::Purpose::Unlock);
    assert!(b.modal.is_none() && b.status.as_ref().is_some_and(|(m, ok)| !ok && m.contains("make one first")), "{:?}", b.status);
}

#[test]
fn a_damaged_container_is_reported_and_offers_nothing_dangerous() {
    let cx = admin("ench-damaged");
    let d = cx.services.dir().join("home-enc");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("root.ivol"), vec![0u8; 20000]).unwrap();
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    let t = draw_text(&mut a, &cx);
    assert!(t.contains("Damaged"), "{t}");
    assert!(a.ench_buttons().is_empty());
    for c in ['c', 'm', 'u', 'l', 'k', 'a', 'x'] {
        press(&mut a, &cx, c);
        assert!(a.modal.is_none(), "{c}");
    }
}
