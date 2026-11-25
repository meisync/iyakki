//! Settings -> Startup on a fake port: the page reads the registry from the port, Retry audits and starts the service again (administrators only),
//! Open its page goes to the page the row names, Look again asks the port to poll, and the page draws at several sizes. No sleeps, no real services.

use super::system_port::{FakeSys, StartupPort};
use super::*;
use crate::apps::tests::cx_with_admin_store;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("startup-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Startup);
    a
}

/// The app with the row of `id` chosen.
fn choose(a: &mut SettingsApp, p: &FakeSys, id: &str) {
    let at = p.startup_rows().iter().position(|r| r.id == id).expect("the service is in the catalogue");
    a.handle_startup(&Key::Home, p);
    for _ in 0..at {
        a.handle_startup(&Key::Down, p);
    }
}

#[test]
fn retry_audits_and_starts_a_failed_service_again() {
    let p = FakeSys::new(true);
    p.startup.failed("k3s", "no disk");
    let mut a = app();
    choose(&mut a, &p, "k3s");
    a.handle_startup(&Key::Char('e'), &p);
    assert_eq!(*p.retried.lock().unwrap(), vec!["k3s"]);
    assert_eq!(p.audited(), vec![("startup.retry".to_string(), "k3s".to_string())]);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("again")), "{:?}", a.status());
}

#[test]
fn a_service_that_is_not_failed_or_cannot_be_retried_is_not_started() {
    let p = FakeSys::new(true);
    p.startup.ready("k3s");
    p.startup.failed("clock", "no time");
    let mut a = app();
    choose(&mut a, &p, "k3s");
    a.handle_startup(&Key::Char('e'), &p);
    choose(&mut a, &p, "clock");
    a.handle_startup(&Key::Char('e'), &p);
    assert!(p.retried.lock().unwrap().is_empty());
    assert!(p.audited().is_empty());
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not waiting")));
}

#[test]
fn a_person_who_is_not_an_administrator_cannot_retry_or_open() {
    let p = FakeSys::new(false);
    p.startup.failed("k3s", "no disk");
    let mut a = app();
    choose(&mut a, &p, "k3s");
    a.handle_startup(&Key::Char('e'), &p);
    a.handle_startup(&Key::Char('o'), &p);
    assert!(p.retried.lock().unwrap().is_empty());
    assert_eq!(a.tab(), Tab::Startup);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrator")));
}

#[test]
fn open_its_page_goes_to_the_page_the_row_names_and_says_so_when_there_is_none() {
    let p = FakeSys::new(true);
    let mut a = app();
    choose(&mut a, &p, "clock");
    a.handle_startup(&Key::Char('o'), &p);
    assert_eq!(a.tab(), Tab::Time);
    let mut a = app();
    choose(&mut a, &p, "touch");
    a.handle_startup(&Key::Char('o'), &p);
    assert_eq!(a.tab(), Tab::Startup);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no page of its own")));
}

#[test]
fn look_again_asks_the_port_to_poll() {
    let p = FakeSys::new(false);
    let mut a = app();
    a.handle_startup(&Key::Char('r'), &p);
    assert_eq!(p.polled.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(a.status().is_some_and(|s| s.1 && s.0 == "Looked again."));
}

#[test]
fn the_page_draws_the_registry_of_the_port_at_several_sizes_and_hides_the_reason_from_others() {
    for admin in [true, false] {
        let p = FakeSys::new(admin);
        p.startup.failed("k3s", "secret reason");
        let mut a = app();
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            a.render_startup(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
            assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Row(_))), "{w}x{h}: rows are clickable");
        }
    }
}
