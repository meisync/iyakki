//! Settings -> Storage: how the boot drive is connected (speed, port, hub, driver), the warnings for USB 2 speed and for a hub, the
//! lost-drive sentence, and the compatible USB mode switch (a fake writer: no boot partition is touched).

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_storage::loss::{self, Snapshot, State, UsbLink};
use std::cell::RefCell;
use std::rc::Rc;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        loss::set_thread_override(None);
        loss::set_thread_compat_writer(None);
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn link(speed: u32, behind_hub: bool) -> UsbLink {
    UsbLink { port: if behind_hub { "2-1.3".into() } else { "2-2".into() }, speed_mbps: speed, behind_hub, hub_siblings: if behind_hub { 2 } else { 0 }, driver: "uas".into(), id: "04e8:4001".into(), name: "Samsung PSSD T7".into() }
}

fn snap(l: Option<UsbLink>, compat: bool, st: State) -> Snapshot {
    Snapshot { state: st, link: l, compat, ..Default::default() }
}

fn screen_text(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Vec<String> {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).max(0.5), cx, 0);
    for hit in g.hits.iter().filter(|x| x.label == "Compatible USB mode") {
        let r = hit.rect;
        assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: the switch {r:?} is inside the window");
    }
    g.hits.iter().map(|h| h.label.clone()).collect()
}

#[test]
fn the_page_shows_the_link_speed_and_the_switch_at_six_sizes() {
    let _r = Reset;
    let cx = admin("stl-page");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)] {
        loss::set_thread_override(Some(snap(Some(link(10000, false)), false, State::Healthy)));
        let mut a = SettingsApp::new(&cx);
        a.go_to(Tab::Storage);
        let labels = screen_text(&mut a, &cx, w, h);
        assert!(labels.iter().any(|l| l == "Compatible USB mode"), "{w}x{h}: the switch is clickable");
    }
    assert_eq!(link(10000, false).speed_text(), "SuperSpeed+ 10 Gbit/s");
    assert_eq!(link(480, false).speed_text(), "High-speed 480 Mbit/s");
}

#[test]
fn a_usb_2_speed_or_a_hub_is_warned_about_and_a_good_link_is_not() {
    assert!(loss::link_advice(Some(&link(10000, false))).is_empty());
    let slow = loss::link_advice(Some(&link(480, false)));
    assert!(slow.contains("USB 2 speed") && slow.contains("cable"), "{slow}");
    let hub = loss::link_advice(Some(&link(5000, true)));
    assert!(hub.contains("behind a USB hub") && hub.contains("2 other"), "{hub}");
}

#[test]
fn the_switch_writes_the_quirk_of_this_drive_audits_it_and_toggles_back() {
    let _r = Reset;
    let cx = admin("stl-sw");
    let calls: Rc<RefCell<Vec<(bool, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let c2 = calls.clone();
    loss::set_thread_compat_writer(Some(Rc::new(move |on, id| {
        c2.borrow_mut().push((on, id.to_string()));
        Ok(format!("fake: {on}"))
    })));
    loss::set_thread_override(Some(snap(Some(link(10000, false)), false, State::Healthy)));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Storage);
    a.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(calls.borrow().as_slice(), [(true, "04e8:4001".to_string())]);
    assert_eq!(a.status.as_ref().map(|s| s.1), Some(true));
    // already on: the same key turns it off
    loss::set_thread_override(Some(snap(Some(link(10000, false)), true, State::Healthy)));
    a.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(calls.borrow().last(), Some(&(false, "04e8:4001".to_string())));
    assert!(cx.audit.recent(5).iter().any(|l| l.contains("storage.usbcompat")));
}

#[test]
fn the_switch_needs_a_usb_drive_an_administrator_and_a_good_id() {
    let _r = Reset;
    let cx = admin("stl-guard");
    loss::set_thread_compat_writer(Some(Rc::new(|_, _| panic!("must not be called"))));
    // not a USB drive: says so and writes nothing
    loss::set_thread_override(Some(snap(None, false, State::Healthy)));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Storage);
    a.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(a.status.as_ref().map(|s| s.1), Some(false));
    // a drive whose id is not 4+4 hex digits never reaches the boot menu
    let mut bad = link(10000, false);
    bad.id = "04e8:4001 init=/bin/sh".into();
    loss::set_thread_override(Some(snap(Some(bad), false, State::Healthy)));
    a.handle(&Event::Key(Key::Char('u')), &cx);
    assert_eq!(a.status.as_ref().map(|s| s.1), Some(false));
    assert!(a.status.as_ref().unwrap().0.contains("USB id"), "{:?}", a.status);
}

#[test]
fn a_lost_drive_says_so_on_the_page() {
    let _r = Reset;
    let cx = admin("stl-lost");
    loss::set_thread_override(Some(snap(Some(link(10000, false)), false, State::Lost)));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Storage);
    // draws without crashing at the small size and the big one
    for (w, h) in [(360u32, 740u32), (2880, 1800)] {
        let _ = screen_text(&mut a, &cx, w, h);
    }
    assert!(loss::unavailable_text(&loss::snapshot()).contains("disconnected"));
}
