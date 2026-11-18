//! Settings -> Notifications: a Show / Quiet / Off choice for each source, per person, kept in their inbox file, with the page
//! drawn at four sizes and every choice reachable by key and by click.

use super::*;
use crate::apps::tile_harness::TileHarness;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::inbox::Level;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

#[test]
fn notification_levels_are_chosen_per_source_by_key_and_click_and_kept_per_person() {
    let cx = admin("notifs1");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    assert!(t.app.tabs().contains(&Tab::Notifications));
    assert_eq!((Tab::Notifications.label(), Tab::Notifications.group()), ("Notifications", "Account & people"));
    t.app.go_to(Tab::Notifications);
    let ib = &cx.inbox();
    // the first source is the scanner: q, o, s by key
    t.send(Event::Key(Key::Char('q')));
    assert_eq!(ib.level("root", "scan"), Level::Quiet);
    t.send(Event::Key(Key::Char('o')));
    assert_eq!(ib.level("root", "scan"), Level::Off);
    t.send(Event::Key(Key::Enter));
    assert_eq!(ib.level("root", "scan"), Level::Show, "Enter goes to the next level: Off then Show");
    // by click on the Quiet chip of the second row
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let g = t.draw_at(w, h);
        assert!(g.hits.iter().filter(|x| matches!(x.ev, Event::Sub(_, Key::Char('q' | 'o' | 's')))).count() >= 3, "chips at {w}x{h}");
    }
    let g = t.draw_at(1280, 720);
    let chip = g.hits.iter().find(|x| matches!(x.ev, Event::Sub(1, Key::Char('q')))).expect("a Quiet chip on the second row").ev.clone();
    t.send(chip);
    assert_eq!(ib.level("root", "runs"), Level::Quiet);
    assert_eq!(ib.level("bob", "runs"), Level::Show, "per person");
    // Off drops new notes from the source, Quiet keeps them out of the bell
    ib.add("root", iyakki_shared::inbox::Kind::Warning, "w", "", "runs", None, 5).unwrap();
    assert_eq!(ib.counts("root", 10).attention, 0);
    t.send(Event::Key(Key::Down));
    t.send(Event::Key(Key::Char('o')));
    assert!(ib.add("root", iyakki_shared::inbox::Kind::Warning, "w2", "", "flows", None, 5).is_err());
    let log = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(log.contains("settings.notifications"), "{log}");
}

#[test]
fn everyone_has_the_notifications_page_not_only_administrators() {
    let mut cx = admin("notifs2");
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    assert!(t.app.tabs().contains(&Tab::Notifications));
}
