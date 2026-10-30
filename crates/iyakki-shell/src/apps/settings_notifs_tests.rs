//! Settings -> Notifications: a Show / Quiet / Off choice for each source, per person, kept in their inbox file, with the page
//! drawn at four sizes and every choice reachable by key and by click.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::inbox::Level;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

#[test]
fn notification_levels_are_chosen_per_source_by_key_and_click_and_kept_per_person() {
    let cx = admin("notifs1");
    let mut a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Notifications));
    assert_eq!((Tab::Notifications.label(), Tab::Notifications.group()), ("Notifications", "Account & people"));
    a.go_to(Tab::Notifications);
    let ib = &cx.services.inbox;
    // the first source is the scanner: q, o, s by key
    a.handle(&Event::Key(Key::Char('q')), &cx);
    assert_eq!(ib.level("root", "scan"), Level::Quiet);
    a.handle(&Event::Key(Key::Char('o')), &cx);
    assert_eq!(ib.level("root", "scan"), Level::Off);
    a.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(ib.level("root", "scan"), Level::Show, "Enter goes to the next level: Off then Show");
    // by click on the Quiet chip of the second row
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().filter(|x| matches!(x.ev, Event::Sub(_, Key::Char('q' | 'o' | 's')))).count() >= 3, "chips at {w}x{h}");
    }
    let g = draw(&mut a, &cx, 1280, 720);
    let chip = g.hits.iter().find(|x| matches!(x.ev, Event::Sub(1, Key::Char('q')))).expect("a Quiet chip on the second row").ev.clone();
    a.handle(&chip, &cx);
    assert_eq!(ib.level("root", "runs"), Level::Quiet);
    assert_eq!(ib.level("bob", "runs"), Level::Show, "per person");
    // Off drops new notes from the source, Quiet keeps them out of the bell
    ib.add("root", iyakki_agent::inbox::Kind::Warning, "w", "", "runs", None, 5).unwrap();
    assert_eq!(ib.counts("root", 10).attention, 0);
    a.handle(&Event::Key(Key::Down), &cx);
    a.handle(&Event::Key(Key::Char('o')), &cx);
    assert!(ib.add("root", iyakki_agent::inbox::Kind::Warning, "w2", "", "flows", None, 5).is_err());
    let log = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(log.contains("settings.notifications"), "{log}");
}

#[test]
fn everyone_has_the_notifications_page_not_only_administrators() {
    let mut cx = admin("notifs2");
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Notifications));
}
