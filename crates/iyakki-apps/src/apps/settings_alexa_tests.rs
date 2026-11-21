//! Settings -> Alexa (F-VOICE-43): fails closed with no owner client id, off by default, the card that is "No" until a person presses Yes
//! (and that no tool can answer), taking the yes back, and the page for administrators and for others.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::alexa;
use iyakki_sec::policy::Role;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn set_up(cx: &AppCx) {
    std::fs::write(cx.data_dir().join(alexa::CONFIG_FILE), r#"{"client_id":"id1","product_id":"p1"}"#).unwrap();
}

#[test]
fn with_no_owner_product_the_switch_and_the_card_refuse_in_words_and_nothing_is_kept() {
    let cx = admin("alexa-closed");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Alexa);
    key(&mut a, &cx, Key::Char('o'));
    assert!(status(&a).contains("no client id"), "{}", status(&a));
    assert!(!alexa::load(cx.data_dir()).enabled);
    key(&mut a, &cx, Key::Char('C'));
    assert!(!a.has_modal(), "no card is shown for a product that does not exist");
    assert!(status(&a).contains("no client id"));
    assert!(alexa::consent_date(cx.data_dir(), "root").is_none());
}

#[test]
fn the_card_names_amazon_starts_on_no_and_only_a_press_says_yes_and_the_yes_can_be_taken_back() {
    let cx = admin("alexa-consent");
    set_up(&cx);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Alexa);
    key(&mut a, &cx, Key::Char('o'));
    assert!(alexa::load(cx.data_dir()).enabled);
    assert!(alexa::allowed(cx.data_dir(), "root").is_err(), "switching on gives no yes");
    key(&mut a, &cx, Key::Char('C'));
    let note = match &a.modal { Some(Modal::Form(_, f)) => f.note.clone(), _ => String::new() };
    assert!(note.contains("Amazon") && note.contains("not IYAKKI's") && note.contains("microphone"), "{note}");
    key(&mut a, &cx, Key::Enter); // Enter alone answers No
    assert!(!a.has_modal() && status(&a).contains("stays on this computer"));
    assert!(alexa::consent_date(cx.data_dir(), "root").is_none());
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Esc);
    assert!(alexa::consent_date(cx.data_dir(), "root").is_none());
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Enter);
    assert!(alexa::allowed(cx.data_dir(), "root").is_ok(), "{}", status(&a));
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("consent given for the voice")));
    key(&mut a, &cx, Key::Char('C'));
    assert!(alexa::consent_date(cx.data_dir(), "root").is_none(), "the yes is taken back at once");
}

#[test]
fn the_page_is_for_administrators_and_for_others_only_once_the_owner_set_the_product_up() {
    let cx = admin("alexa-tab");
    assert!(SettingsApp::new(&cx).tabs.contains(&Tab::Alexa));
    assert_eq!(Tab::Alexa.group(), "Apps & AI");
    let mut ucx = admin("alexa-tab-user");
    ucx.session.user = "amy".into();
    ucx.session.role = Role::User;
    ucx.session.policy = Role::User.template();
    assert!(!SettingsApp::new(&ucx).tabs.contains(&Tab::Alexa));
    if ucx.session.policy.voice {
        set_up(&ucx);
        assert!(SettingsApp::new(&ucx).tabs.contains(&Tab::Alexa));
        let mut a = SettingsApp::new(&ucx);
        a.go_to(Tab::Alexa);
        key(&mut a, &ucx, Key::Char('o'));
        assert!(status(&a).contains("administrator"), "{}", status(&a));
    }
}
