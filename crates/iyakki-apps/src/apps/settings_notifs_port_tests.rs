//! Settings -> Notifications and the Matrix account on a fake port: a level is set, audited and a refusal shown; the camera description switch;
//! the Matrix form goes through the policy check and a background sign-in whose answer is collected by the heartbeat; test message, copy switch,
//! room switch and removal act on the connector of the port; the page draws at several sizes. No sleeps (the background answer is awaited on its channel).

use super::system_port::{FakeMatrix, FakeSys, MatrixPort, NotifsPort};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_shared::inbox::Level;
use std::sync::atomic::Ordering;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("notifs-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Notifications);
    a
}

fn with_matrix() -> FakeSys {
    let mut p = FakeSys::new(true);
    p.matrix = FakeMatrix::with_account();
    p.sources.lock().unwrap().push(("flows".into(), "Flows".into()));
    p
}

/// Wait for the background job of the page (on its channel, no polling) and let the heartbeat collect the answer.
fn finish_job(a: &mut SettingsApp) -> (bool, String) {
    let rx = a.matrix.pending.take().expect("a background job was started");
    let r = rx.recv().expect("the job answers");
    let (tx, rx2) = std::sync::mpsc::channel();
    tx.send(r.clone()).unwrap();
    a.matrix.pending = Some(rx2);
    a.matrix_tick();
    assert!(a.matrix.pending.is_none());
    r
}

fn form(_a: &mut SettingsApp, server: &str, user: &str, secret: &str) -> Form {
    let mut f = SettingsApp::matrix_form(server);
    f.set_value(0, "Home");
    f.set_value(1, server);
    f.set_value(3, user);
    f.set_value(4, secret);
    f
}

#[test]
fn a_level_is_set_audited_and_the_next_press_goes_round() {
    let p = with_matrix();
    let mut a = app();
    a.handle_notifs(&Key::Char('q'), &p);
    assert_eq!(p.notif_levels().get("jobs"), Some(&Level::Quiet));
    assert_eq!(p.audited(), vec![("settings.notifications".to_string(), "jobs quiet".to_string())]);
    a.handle_notifs(&Key::Enter, &p);
    assert_eq!(p.notif_levels().get("jobs"), Some(&Level::Off), "Quiet goes to Off");
    a.handle_notifs(&Key::Down, &p);
    a.handle_notifs(&Key::Char('o'), &p);
    assert_eq!(p.notif_levels().get("devices"), Some(&Level::Off));
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Off")));
}

#[test]
fn a_refused_level_is_shown_and_not_audited_and_a_chip_click_sets_its_row() {
    let p = with_matrix();
    *p.refuse.lock().unwrap() = Some("the inbox is locked".to_string());
    let mut a = app();
    a.handle_notifs(&Key::Char('s'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("locked")));
    assert!(p.audited().is_empty());
    *p.refuse.lock().unwrap() = None;
    a.notif_sub(2, 'q', &p);
    assert_eq!(p.notif_levels().get("flows"), Some(&Level::Quiet));
    assert_eq!(a.sel(), 2);
}

#[test]
fn the_camera_description_switch_saves_and_audits_and_a_refusal_is_shown() {
    let p = with_matrix();
    let mut a = app();
    a.handle_notifs(&Key::Char('v'), &p);
    assert!(p.camvision.load(Ordering::SeqCst));
    assert_eq!(p.audited(), vec![("settings.cameras.vision".to_string(), "on".to_string())]);
    a.handle_notifs(&Key::Char('v'), &p);
    assert!(!p.camvision.load(Ordering::SeqCst));
    *p.refuse.lock().unwrap() = Some("cannot write".to_string());
    a.handle_notifs(&Key::Char('v'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot write")));
    assert_eq!(p.audited().len(), 2);
}

#[test]
fn the_reach_line_says_what_is_set_up_and_follows_the_copy_switch() {
    let p = FakeSys::new(true);
    assert!(SettingsApp::matrix_reach_line(&p).contains("nothing set up"));
    let p = with_matrix();
    assert!(SettingsApp::matrix_reach_line(&p).contains("(off now)"));
    p.matrix().set_mirror("m1", true).unwrap();
    assert!(SettingsApp::matrix_reach_line(&p).contains("m turns that off"));
    p.matrix.unread.lock().unwrap().push(("main".into(), 2));
    assert!(SettingsApp::matrix_reach_line(&p).contains("2 unread"));
}

#[test]
fn the_form_signs_in_in_the_background_and_the_heartbeat_tells_the_answer() {
    let p = with_matrix();
    p.matrix.accounts.lock().unwrap().clear();
    p.matrix.rooms.lock().unwrap().clear();
    let mut a = app();
    let mut f = form(&mut a, "https://hs.example.test", "alice", "pw-not-kept");
    assert!(a.matrix_submit(&p, &mut f).unwrap().contains("Signing in"));
    assert!(a.matrix_submit(&p, &mut f).is_err_and(|e| e.contains("still running")), "one job at a time");
    let (ok, text) = finish_job(&mut a);
    assert!(ok && text.contains("@alice:fake.test") && text.contains("No room is set yet"), "{text}");
    assert!(a.status().is_some_and(|s| s.1));
    assert_eq!(p.matrix.called(), vec!["add Home https://hs.example.test alice ".to_string()]);
}

#[test]
fn the_form_is_checked_before_any_sign_in_starts() {
    let p = with_matrix();
    let mut a = app();
    let mut f = form(&mut a, "", "alice", "pw");
    assert!(a.matrix_submit(&p, &mut f).is_err_and(|e| e.contains("address")));
    let mut f = form(&mut a, "https://hs.example.test", "alice", "  ");
    assert!(a.matrix_submit(&p, &mut f).is_err_and(|e| e.contains("password")));
    let mut f = form(&mut a, "https://hs.example.test", "", "pw");
    assert!(a.matrix_submit(&p, &mut f).is_err_and(|e| e.contains("user name")));
    p.blocked_hosts.lock().unwrap().push("hs.example.test".into());
    let mut f = form(&mut a, "https://hs.example.test", "alice", "pw");
    assert!(a.matrix_submit(&p, &mut f).is_err_and(|e| e.contains("network policy")));
    assert!(a.matrix.pending.is_none());
    assert!(p.matrix.called().is_empty());
}

#[test]
fn a_failed_sign_in_is_shown_as_a_failure() {
    let p = with_matrix();
    *p.matrix.fail.lock().unwrap() = Some("wrong password".into());
    let mut a = app();
    let mut f = form(&mut a, "https://hs.example.test", "alice", "pw");
    a.matrix_submit(&p, &mut f).unwrap();
    let (ok, text) = finish_job(&mut a);
    assert!(!ok && text == "wrong password");
    assert!(a.status().is_some_and(|s| !s.1 && s.0 == "wrong password"));
}

#[test]
fn test_message_copy_room_and_removal_act_on_the_connector_and_are_audited() {
    let p = with_matrix();
    let mut a = app();
    a.handle_notifs(&Key::Char('t'), &p);
    let (ok, text) = finish_job(&mut a);
    assert!(ok && text.contains("Test message sent (Home)"), "{text}");
    a.handle_notifs(&Key::Char('m'), &p);
    assert!(p.matrix().mirror_on("m1"));
    a.handle_notifs(&Key::Char('r'), &p);
    assert!(!p.matrix().rooms("m1")[0].mirror, "the room is left out of the copy");
    assert_eq!(p.audited().iter().map(|l| l.0.as_str()).collect::<Vec<_>>(), vec!["msg.mirror.set", "msg.mirror.room"]);
    a.handle_notifs(&Key::Char('x'), &p);
    assert!(p.matrix().accounts().is_empty());
    assert_eq!(p.matrix.called(), vec!["test_send m1".to_string(), "remove m1".to_string()]);
}

#[test]
fn without_an_account_each_action_says_to_connect_one_and_a_blocked_server_is_refused() {
    let p = FakeSys::new(true);
    let mut a = app();
    for k in ['t', 'm', 'r'] {
        a.handle_notifs(&Key::Char(k), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Connect a Matrix account")), "{k}");
    }
    a.handle_notifs(&Key::Char('x'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no Matrix account")));
    let p = with_matrix();
    p.blocked_hosts.lock().unwrap().push("fake.test".into());
    a.handle_notifs(&Key::Char('t'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("network policy")));
    assert!(p.matrix.called().is_empty());
    assert!(p.audited().is_empty());
}

#[test]
fn the_page_draws_the_sources_of_the_port_at_several_sizes() {
    let p = with_matrix();
    p.levels.lock().unwrap().insert("jobs".into(), Level::Quiet);
    let mut a = app();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_notifs(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
        assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('v'))), "{w}x{h}: the camera switch");
    }
}
