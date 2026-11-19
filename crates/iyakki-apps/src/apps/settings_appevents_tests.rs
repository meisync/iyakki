//! Settings -> App events: the registrations of the person's apps listed with what they let the app hear, a switch for each that goes through the
//! core (nothing is delivered for an event switched off), audited by names, and the page drawn at four sizes. Nothing here was seen on a real screen.

use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::apps::tile_harness::TileHarness;
use iyakki_core_api::appevents::{payload, AppEvents, Val};
use iyakki_core_fake::FakeCore;

fn setup(tag: &str) -> (AppCx, Arc<FakeCore>) {
    let mut cx = cx_with_admin_store(tag);
    let core = Arc::new(FakeCore::new());
    cx.core = core.clone();
    let u = cx.session.user.clone();
    let f = core.app_events_fake();
    f.grant("notes", "events.network");
    f.grant("notes", "events.power");
    f.register(&u, "notes", "connectivity.changed", "").unwrap();
    f.register(&u, "notes", "peers.changed", "").unwrap();
    (cx, core)
}

#[test]
fn the_page_lists_what_each_app_listens_to_and_the_switch_stops_delivery_through_the_core() {
    let (cx, core) = setup("appev1");
    let u = cx.session.user.clone();
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    assert!(t.app.tabs().contains(&Tab::AppEvents) && Tab::ORDER.contains(&Tab::AppEvents));
    assert_eq!((Tab::AppEvents.label(), Tab::AppEvents.group()), ("App events", "Apps & AI"));
    t.app.go_to(Tab::AppEvents);
    let rows = SettingsApp::appevents_rows(&cx);
    assert_eq!(rows.iter().map(|r| (r.app.as_str(), r.event.as_str(), r.enabled)).collect::<Vec<_>>(), [("notes", "connectivity.changed", true), ("notes", "peers.changed", true)]);
    assert!(rows[0].gives.contains("network"));
    let net = payload(&[("state", Val::Text("offline".into()))]);
    assert_eq!(core.app_events_fake().emit("connectivity.changed", Some(&u), net.clone()).unwrap(), 1);
    assert_eq!(SettingsApp::appevents_rows(&cx)[0].pending, 1);
    // the person turns the first one off by key
    t.app.handle_appevents(&Key::Enter, &cx);
    let rows = SettingsApp::appevents_rows(&cx);
    assert!(!rows[0].enabled && rows[0].pending == 0, "nothing waits for it any more");
    assert_eq!(core.app_events_fake().emit("connectivity.changed", Some(&u), net).unwrap(), 0);
    assert!(core.app_events_fake().audit().iter().any(|l| l == "appevents.switch notes: connectivity.changed off"));
    // and on again by click on the row's button
    t.app.appevents_sub(0, 't', &cx);
    assert!(SettingsApp::appevents_rows(&cx)[0].enabled);
}

#[test]
fn the_page_says_when_no_app_listens_and_draws_at_four_sizes() {
    let cx = cx_with_admin_store("appev2");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppEvents);
    assert_eq!(SettingsApp::appevents_len(&cx), 0);
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let _ = t.draw_at(w, h);
    }
    let (cx, _core) = setup("appev3");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppEvents);
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let _ = t.draw_at(w, h);
    }
}

#[test]
fn taking_a_permission_away_in_apps_access_tells_the_app_and_ends_the_registration_that_needed_it() {
    use iyakki_core_api::appcaps::{record_approval, ApprovalBook, CapabilityRequest};
    let mut cx = cx_with_admin_store("appev-revoke");
    let core = Arc::new(FakeCore::new());
    cx.core = core.clone();
    let user = cx.session.user.clone();
    let store = iyakki_agent::store::Store::open(cx.data_dir());
    let mut book = iyakki_agent::store::approvals::FileBook(&store);
    let reqs = vec![CapabilityRequest { name: "events.network".into(), why: "x".into(), param: None, extra: Default::default() }];
    book.put(&format!("tile/user/{user}/notes"), record_approval(&user, "notes", "1.0", "abc", 5, &reqs));
    let f = core.app_events_fake();
    f.grant("notes", "events.network");
    f.register(&user, "notes", "connectivity.changed", "").unwrap();
    f.start(&user, "notes");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppAccess);
    f.revoke("notes", "events.network"); // what the broker's own revoke does to the scopes the core checks
    t.app.handle_appaccess(&Key::Char('r'), &cx);
    let got = f.poll(&user, "notes", 10);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].name, "app.permissions");
    assert_eq!(got[0].payload["revoked"], iyakki_core_api::appevents::Val::Text("events.network".into()));
    assert_eq!(got[0].payload["dropped"], iyakki_core_api::appevents::Val::Text("connectivity.changed".into()));
    assert_eq!(got[0].payload["by"], iyakki_core_api::appevents::Val::Text("administrator".into()));
    assert!(f.list(&user, "notes").is_empty());
}

#[test]
fn the_person_takes_the_permission_of_a_registered_event_away_from_the_page_and_the_app_is_told() {
    let (cx, core) = setup("appev-perm");
    let u = cx.session.user.clone();
    let f = core.app_events_fake();
    f.start(&u, "notes");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppEvents);
    t.app.handle_appevents(&Key::Char('p'), &cx); // the first row: connectivity.changed, which needs events.network
    assert!(SettingsApp::appevents_rows(&cx).is_empty(), "both registrations needed events.network: they ended with it");
    let got = f.poll(&u, "notes", 10);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].name.as_str(), got[0].payload["revoked"].clone(), got[0].payload["by"].clone()), ("app.permissions", Val::Text("events.network".into()), Val::Text("person".into())));
    assert!(f.audit().iter().any(|l| l.starts_with("appevents.permissions notes: 0 granted, 1 revoked, 2 dropped, by person")));
}
