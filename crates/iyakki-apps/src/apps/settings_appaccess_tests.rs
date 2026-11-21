//! Settings -> Apps access: the approved accesses of installed apps listed, taken away by key and by click (audited, applied at the broker's next
//! call), the access log shown with allowed and refused uses, clearing it, administrators only, and the page drawn at four sizes.
//! Nothing here was seen on a real screen.

use super::*;
use crate::apps::tile_harness::TileHarness;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_shared::accessbroker::{self as ab, FileLog};
use iyakki_shared::accessbroker::appcaps::{record_approval, ApprovalBook, CapabilityRequest};
use iyakki_core_api::broker::{AccessEvent, AccessSink, Call, Caller, CallerKind, Denial};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn approve(cx: &AppCx, inst: &str, app: &str, caps: &[&str]) {
    let store = iyakki_agent::store::Store::open(cx.data_dir());
    let mut book = iyakki_agent::store::approvals::FileBook(&store);
    let reqs: Vec<CapabilityRequest> = caps.iter().map(|c| CapabilityRequest { name: (*c).into(), why: "x".into(), param: None, extra: Default::default() }).collect();
    book.put(inst, record_approval("root", app, "1.2", "abc", 5, &reqs));
}

fn ev(at: i64, allowed: bool) -> AccessEvent {
    AccessEvent { at, app: "cameras".into(), person: "root".into(), device: "box".into(), capability: "camera.view".into(), target: "HD Webcam".into(), allowed, reason: if allowed { String::new() } else { "revoked".into() } }
}

#[test]
fn the_page_lists_accesses_and_takes_one_away_by_key_and_by_click_and_the_broker_follows() {
    let cx = admin("appacc1");
    approve(&cx, "tile/user/root/notes", "notes", &["camera.view", "notify"]);
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    assert!(t.app.tabs().contains(&Tab::AppAccess) && Tab::ORDER.contains(&Tab::AppAccess));
    assert_eq!((Tab::AppAccess.label(), Tab::AppAccess.group()), ("Apps access", "Apps & AI"));
    t.app.go_to(Tab::AppAccess);
    let rows = SettingsApp::appaccess_rows(&cx);
    assert_eq!(rows.iter().map(|r| (r.app.as_str(), r.capability.as_str(), r.revoked)).collect::<Vec<_>>(), [("notes", "camera.view", false), ("notes", "notify", false)]);
    // by key: the first row
    t.send(Event::Key(Key::Char('r')));
    let rows = SettingsApp::appaccess_rows(&cx);
    assert!(rows.iter().any(|r| r.capability == "camera.view" && r.revoked) && rows.iter().any(|r| r.capability == "notify" && !r.revoked));
    // the broker on this machine refuses at its next call, with the stable code
    let (broker, log) = ab::local(cx.data_dir(), None);
    let caller = Caller { app: "notes".into(), kind: CallerKind::Store { installation: "tile/user/root/notes".into() }, person: "root".into(), device: "box".into() };
    assert_eq!(broker.decide(&Call { caller: &caller, capability: "camera.view", unit: Some("usb:video0"), label: "HD Webcam" }), Err(Denial::Revoked));
    assert_eq!(broker.decide(&Call { caller: &caller, capability: "notify", unit: None, label: "" }), Ok(()));
    // audited
    let audit = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("access.revoke") && audit.contains("notes 1.2 camera.view"), "the revoke is in the audit chain: {audit}");
    // the revoked one moves below the ones still held; by click on the chip of the first row (notify)
    let g = t.draw_at(1280, 720);
    let chip = g.hits.iter().find(|h| matches!(h.ev, Event::Sub(0, Key::Char('r')))).expect("a chip on the first row").ev.clone();
    t.send(chip);
    assert!(SettingsApp::appaccess_rows(&cx).iter().all(|r| r.revoked));
    // revoking again changes nothing and says so
    t.send(Event::Key(Key::Char('r')));
    assert!(t.app.status.as_ref().is_some_and(|(m, _)| m.contains("already")));
    drop(log);
}

#[test]
fn the_log_is_shown_with_allowed_and_refused_lines_and_can_be_cleared_and_audited() {
    let cx = admin("appacc2");
    let log = FileLog::open(cx.data_dir());
    log.record(ev(1_700_000_000, true));
    log.record(ev(1_700_000_100, false));
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppAccess);
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let g = t.draw_at(w, h);
        assert!(g.hits.iter().any(|x| x.label == "Take it away") || g.hits.iter().any(|x| x.label == "Clear the log"), "buttons at {w}x{h}");
    }
    assert_eq!(log.recent(10).len(), 2);
    t.send(Event::Key(Key::Char('c')));
    assert!(FileLog::open(cx.data_dir()).recent(10).is_empty());
    assert!(std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default().contains("access.log.clear"));
}

#[test]
fn the_page_is_for_administrators_only() {
    let cx = admin("appacc3");
    let t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    assert!(t.app.tabs().contains(&Tab::AppAccess));
    // an ordinary account does not get the page
    let mut ucx = cx_with_admin_store("appacc3u");
    ucx.session.policy.manage_settings = false;
    let u = SettingsApp::new(&ucx);
    assert!(!u.tabs().contains(&Tab::AppAccess));
}

#[test]
fn the_walk_over_all_keys_does_not_panic_with_and_without_rows() {
    let cx = admin("appacc4");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppAccess);
    for c in "rcRCxz".chars() {
        t.send(Event::Key(Key::Char(c)));
    }
    approve(&cx, "tile/user/root/a", "a", &["camera.view"]);
    approve(&cx, "tile/user/root/b", "b", &["camera.view"]);
    for k in [Key::Down, Key::Down, Key::Up, Key::Char('r'), Key::Down, Key::Delete, Key::Enter] {
        t.send(Event::Key(k));
    }
    let _ = t.draw_at(480, 300);
}

#[test]
fn a_time_limit_is_set_from_the_page_shown_taken_off_and_the_broker_refuses_after_the_end() {
    let cx = admin("appacc-limit");
    approve(&cx, "tile/user/root/notes", "notes", &["camera.view", "notify"]);
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::AppAccess);
    assert!(t.app.appaccess_buttons().iter().any(|b| b.0 == "Time limit…"));
    t.send(Event::Key(Key::Char('e')));
    let Some(Modal::Form(FormKind::AppAccessLimit(inst, cap), _)) = &t.app.modal else { panic!("the limit form is not open") };
    assert_eq!((inst.as_str(), cap.as_str()), ("tile/user/root/notes", "camera.view"));
    let submit = |t: &mut TileHarness<SettingsApp>, text: &str| {
        let mut f = Form::new("x").text("h", "", "");
        f.set_value(0, text);
        t.app.appaccess_limit_submit(&cx, "tile/user/root/notes", "camera.view", &f)
    };
    for bad in ["0", "abc", "9000", "-1"] {
        assert!(submit(&mut t, bad).is_err(), "{bad}");
    }
    assert!(submit(&mut t, "24").unwrap().contains("24 more hour"));
    let held = ab::held_list(cx.data_dir());
    let end = held[0].ends.get("camera.view").copied().expect("the end is stored");
    assert!(held[0].ends.get("notify").is_none());
    assert!(end > iyakki_clock::now_unix() + 23 * 3600);
    let audit = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("access.limit") && audit.contains("camera.view 24 h"), "{audit}");
    // while it runs the broker allows it; the limit can be taken off again
    let (broker, _log) = ab::local(cx.data_dir(), None);
    let caller = Caller { app: "notes".into(), kind: CallerKind::Store { installation: "tile/user/root/notes".into() }, person: "root".into(), device: "box".into() };
    let call = || Call { caller: &caller, capability: "camera.view", unit: Some("usb:video0"), label: "HD Webcam" };
    assert_eq!(broker.decide(&call()), Ok(()));
    assert!(submit(&mut t, "").unwrap().contains("no time limit"));
    assert!(ab::held_list(cx.data_dir())[0].ends.is_empty());
    // a limit that has already run out (written with a clock two days back) is refused at the broker's next call
    iyakki_agent::accessbroker_revoke::limit(cx.data_dir(), "tile/user/root/notes", "camera.view", Some(1), iyakki_clock::now_unix() - 2 * 86_400).unwrap();
    assert!(broker.decide(&call()).is_err());
    assert_eq!(broker.decide(&Call { caller: &caller, capability: "notify", unit: None, label: "" }), Ok(()));
    // a capability that is not held cannot be limited
    assert_eq!(iyakki_agent::accessbroker_revoke::limit(cx.data_dir(), "tile/user/root/notes", "files.read", Some(1), 0), Ok(false));
}
