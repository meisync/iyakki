//! Settings -> Time and Host name on a fake port: the clock face is the person's own, the machine's time is for administrators (zone, automatic
//! zone, network time, server, sync), every change is audited, a refused save is shown, the zone lookup goes through the port, the host name is
//! typed, saved and applied through the port, and both pages draw the port's values at several sizes. No sleeps (the sync job is awaited on its
//! channel), no real clock changes.

use super::system_port::{FakeSys, HostNamePort, TimePort};
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("time-fake");
    let mut a = SettingsApp::new(&cx).with_time(true, Arc::new(|_| Ok(())), None);
    a.go_to(Tab::Time);
    a
}

#[test]
fn the_clock_face_is_the_persons_own_and_needs_no_administrator() {
    let p = FakeSys::new(false);
    let mut a = app();
    a.handle_time(&Key::Char('h'), &p);
    assert_eq!(p.clock_face(), (true, false));
    a.handle_time(&Key::Char('s'), &p);
    assert_eq!(p.clock_face(), (true, true));
    a.handle_time(&Key::Char('h'), &p);
    assert_eq!(p.clock_face(), (false, true));
    assert!(p.audited().is_empty(), "a person's own face is not an audited machine change");
}

#[test]
fn the_machines_time_is_refused_to_others_and_the_refusal_is_audited() {
    let p = FakeSys::new(false);
    let mut a = app();
    for k in ['n', 'u', 'a', 'z'] {
        a.handle_time(&Key::Char(k), &p);
    }
    assert_eq!(p.time_settings(), Default::default());
    assert!(p.audited().iter().all(|l| l.0 == "settings.denied") && p.audited().len() == 4);
}

#[test]
fn network_time_and_the_default_zone_save_and_are_audited() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_time(&Key::Char('n'), &p);
    assert!(p.time_settings().no_ntp);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("no longer set")));
    a.handle_time(&Key::Char('n'), &p);
    assert!(!p.time_settings().no_ntp);
    p.time.lock().unwrap().zone = "Europe/Paris".into();
    a.handle_time(&Key::Char('u'), &p);
    assert_eq!(p.time_settings().zone, "");
    assert_eq!(p.audited().iter().map(|l| (l.0.as_str(), l.1.as_str())).collect::<Vec<_>>(), vec![("settings.time", "ntp=off"), ("settings.time", "ntp=on"), ("settings.time", "zone=default")]);
}

#[test]
fn a_refused_save_is_shown_and_not_audited() {
    let p = FakeSys::new(true);
    *p.refuse.lock().unwrap() = Some("the settings file is read-only".into());
    let mut a = app();
    a.handle_time(&Key::Char('n'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("read-only")));
    assert!(p.audited().is_empty());
    assert!(!p.time_settings().no_ntp);
}

#[test]
fn a_zone_picked_by_hand_is_fixed_and_automatic_detection_stops() {
    let p = FakeSys::new(true);
    p.time.lock().unwrap().zone_auto = true;
    let mut a = app();
    a.time_pick(&p, "Asia/Tokyo");
    let t = p.time_settings();
    assert_eq!((t.zone.as_str(), t.zone_auto), ("Asia/Tokyo", false));
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Asia/Tokyo")));
}

#[test]
fn automatic_zone_looks_up_through_the_detector_and_saves_the_answer() {
    let p = FakeSys::new(true);
    let cx = cx_with_admin_store("time-fake-detect");
    let mut a = SettingsApp::new(&cx).with_time(true, Arc::new(|_| Ok(())), None).with_zone_detector(Arc::new(|| Ok(("Europe/Berlin".to_string(), "fake lookup"))));
    a.go_to(Tab::Time);
    a.handle_time(&Key::Char('a'), &p);
    let t = p.time_settings();
    assert!(t.zone_auto);
    assert_eq!(t.zone, "Europe/Berlin");
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Europe/Berlin")));
    assert!(p.audited().iter().any(|l| l.0 == "settings.time.detect"));
    // "Detect now" needs the automatic zone on
    p.time.lock().unwrap().zone_auto = false;
    a.handle_time(&Key::Char('d'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Switch on Detect automatically")));
}

#[test]
fn without_a_detector_the_lookup_runs_in_the_background_through_the_port_once() {
    let p = FakeSys::new(true);
    p.time.lock().unwrap().zone_auto = true;
    let mut a = app();
    a.handle_time(&Key::Char('d'), &p);
    assert_eq!(p.lookups.load(Ordering::SeqCst), 1);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Looking up")));
    p.lookup_busy.store(true, Ordering::SeqCst);
    a.handle_time(&Key::Char('d'), &p);
    assert!(a.status().is_some_and(|s| s.0.contains("already running")));
}

#[test]
fn the_server_form_checks_the_name_and_saves_it() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_time(&Key::Char('c'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::TimeServer, _))));
    let form = |s: &str| Form::new("Time server").text("Host name", s, "");
    assert!(a.time_submit(&p, &form("bad name!")).is_err_and(|e| e.contains("host name")));
    assert_eq!(a.time_submit(&p, &form("ntp.example.org")).unwrap(), "Time server: ntp.example.org.");
    assert_eq!(p.time_settings().ntp_server, "ntp.example.org");
    assert_eq!(a.time_submit(&p, &form("")).unwrap(), "Using the usual time servers.");
    assert_eq!(p.time_settings().ntp_server, "");
    let q = FakeSys::new(false);
    assert_eq!(a.time_submit(&q, &form("ntp.example.org")).unwrap(), "");
    assert_eq!(q.time_settings().ntp_server, "");
}

#[test]
fn ask_server_now_runs_the_sync_job_of_the_port_with_its_servers_and_tells_the_answer() {
    let p = FakeSys::new(true);
    p.time.lock().unwrap().ntp_server = "ntp.example.org".into();
    let set = Arc::new(Mutex::new(Vec::new()));
    let seen = set.clone();
    let cx = cx_with_admin_store("time-fake-sync");
    let mut a = SettingsApp::new(&cx).with_time(true, Arc::new(move |u| { seen.lock().unwrap().push(u); Ok(()) }), None);
    a.go_to(Tab::Time);
    a.handle_time(&Key::Char('b'), &p);
    let job = a.job.take().expect("a job runs");
    match job.rx.recv().unwrap() {
        JobOut::Msg(Ok(m)) => assert_eq!(m, "Set the clock."),
        _ => panic!("expected the answer"),
    }
    assert_eq!(p.sync_log.lock().unwrap().clone(), vec![(vec!["ntp.example.org".to_string()], true)]);
    assert_eq!(set.lock().unwrap().len(), 1, "the clock setter of the page was used");
    assert!(p.audited().iter().any(|l| l.0 == "settings.time.sync" && l.1 == "allow back"));
    *p.ntp_answer.lock().unwrap() = "Could not reach the time server.".into();
    a.handle_time(&Key::Char('y'), &p);
    match a.job.take().unwrap().rx.recv().unwrap() {
        JobOut::Msg(Err(m)) => assert!(m.starts_with("Could not")),
        _ => panic!("expected a failure"),
    }
}

#[test]
fn a_desktop_window_only_looks() {
    let p = FakeSys::new(true);
    let cx = cx_with_admin_store("time-fake-window");
    let mut a = SettingsApp::new(&cx).with_time(false, Arc::new(|_| Ok(())), None);
    a.go_to(Tab::Time);
    a.handle_time(&Key::Char('n'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("follows the clock")));
    assert!(!p.time_settings().no_ntp);
}

#[test]
fn the_rows_show_what_the_port_knows_and_the_page_draws_at_several_sizes() {
    let p = FakeSys::new(true);
    p.time.lock().unwrap().zone = "Europe/Paris".into();
    *p.face.lock().unwrap() = (true, true);
    *p.ntp.lock().unwrap() = iyakki_shared::ntp::Status { message: "Clock is right.".into(), last_sync: iyakki_clock::now_unix(), server: "time.fake.test".into(), ..Default::default() };
    let mut a = app();
    let rows = a.time_rows(&p);
    let get = |l: &str| rows.iter().find(|r| r.0 == l).map(|r| r.1.clone());
    assert_eq!(get("Time zone").as_deref(), Some("Europe/Paris"));
    assert_eq!(get("Your clock").as_deref(), Some("12-hour, with seconds"));
    assert!(get("Last answer").is_some_and(|v| v.contains("time.fake.test")));
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_time(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}

// ---- host name ----

fn name_app(tag: &str, p: &FakeSys) -> SettingsApp {
    let cx = cx_with_admin_store(tag);
    let mut a = SettingsApp::new(&cx).with_host_name(p.hostname_conf_path(), temp_dir(&format!("{tag}-etc")).join("hostname"), "", true, Arc::new(|_, _| Ok(())));
    a.hn.apply = None;
    a.go_to(Tab::HostName);
    a
}

fn with_conf(tag: &str, admin: bool) -> FakeSys {
    let mut p = FakeSys::new(admin);
    p.hostname_conf = temp_dir(tag).join("system.conf");
    p
}

#[test]
fn the_typed_name_is_checked_then_applied_through_the_port_and_kept_for_the_next_boots() {
    let p = with_conf("hn-fake-apply", true);
    let mut a = name_app("hn-fake-apply", &p);
    a.handle_host_name(&Key::Char('e'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::HostName, _))));
    a.handle_host_name(&Key::Char('y'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("type a new name")));
    let f = Form::new("Host name").text("Name", "kitchen-box", "");
    assert!(a.host_name_submit(&p, &f).unwrap().contains("kitchen-box"));
    a.handle_host_name(&Key::Char('y'), &p);
    assert_eq!(p.hostnames.lock().unwrap().clone(), vec!["kitchen-box".to_string()]);
    assert_eq!(p.audited(), vec![("hostname.apply".to_string(), "kitchen-box".to_string())]);
    assert_eq!(crate::hostname::saved(&p.hostname_conf_path()).as_deref(), Some("kitchen-box"));
    assert!(a.hn.draft.is_none());
}

#[test]
fn save_for_next_boots_applies_nothing_and_a_failed_apply_still_keeps_the_name() {
    let p = with_conf("hn-fake-save", true);
    let mut a = name_app("hn-fake-save", &p);
    a.hn.draft = Some("lab-box".into());
    a.handle_host_name(&Key::Char('v'), &p);
    assert!(p.hostnames.lock().unwrap().is_empty());
    assert_eq!(p.audited(), vec![("hostname.save".to_string(), "lab-box".to_string())]);
    *p.refuse.lock().unwrap() = Some("not allowed".into());
    a.handle_host_name(&Key::Char('y'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Saved for the next boots, but not applied now: not allowed")));
}

#[test]
fn others_a_window_and_a_boot_option_cannot_change_the_name() {
    let p = with_conf("hn-fake-deny", false);
    let mut a = name_app("hn-fake-deny", &p);
    a.handle_host_name(&Key::Char('e'), &p);
    assert!(a.modal.is_none());
    assert_eq!(p.audited(), vec![("settings.denied".to_string(), a.tab().label().to_string())]);
    let q = with_conf("hn-fake-locked", true);
    let cx = cx_with_admin_store("hn-fake-locked");
    let mut b = SettingsApp::new(&cx).with_host_name(q.hostname_conf_path(), "/nonexistent/hostname".into(), "quiet iyakki.hostname=fixed-name", true, Arc::new(|_, _| Ok(())));
    b.go_to(Tab::HostName);
    b.handle_host_name(&Key::Char('e'), &q);
    assert!(b.modal.is_none());
    assert!(b.status().is_some_and(|s| !s.1 && s.0.contains("fixed-name")));
    let w = with_conf("hn-fake-window", true);
    let mut c = SettingsApp::new(&cx).with_host_name(w.hostname_conf_path(), "/nonexistent/hostname".into(), "", false, Arc::new(|_, _| Ok(())));
    c.go_to(Tab::HostName);
    c.handle_host_name(&Key::Char('e'), &w);
    assert!(c.status().is_some_and(|s| !s.1 && s.0.contains("changes nothing")));
}

#[test]
fn the_host_name_page_draws_at_several_sizes() {
    let p = with_conf("hn-fake-draw", true);
    let mut a = name_app("hn-fake-draw", &p);
    a.hn.draft = Some("typed-name".into());
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_host_name(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}
