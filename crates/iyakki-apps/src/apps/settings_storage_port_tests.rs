//! Settings -> Storage on a fake port: the compatible USB mode switch (administrators, only for a USB boot drive, audited, a refusal shown),
//! the restart question (administrators with the power right), the sign-out choice for the person's drives, Open DISKS, and the page at several
//! sizes with and without the DISKS line. No sleeps, no real drive.

use super::system_port::{FakeSys, StoragePort};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_core_api::device_types::storage::loss::{Snapshot, State, UsbLink};
use std::sync::atomic::Ordering;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("storage-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Storage);
    a
}

fn usb(p: &FakeSys) {
    *p.usb.lock().unwrap() = Snapshot { state: State::Healthy, link: Some(UsbLink { port: "2-2".into(), speed_mbps: 480, behind_hub: true, hub_siblings: 2, driver: "uas".into(), id: "04e8:4001".into(), name: "Fake T7".into() }), compat: false, ..Default::default() };
}

#[test]
fn the_usb_mode_switches_for_an_administrator_with_a_usb_drive_and_is_audited() {
    let p = FakeSys::new(true);
    usb(&p);
    let mut a = app();
    a.handle_storage(&Key::Char('u'), &p);
    assert!(p.usb_loss_snapshot().compat);
    assert_eq!(p.audited(), vec![("storage.usbcompat".to_string(), "on".to_string())]);
    assert!(a.status().is_some_and(|s| s.1 && s.0 == "compat true for 04e8:4001"));
    a.handle_storage(&Key::Char('u'), &p);
    assert!(!p.usb_loss_snapshot().compat);
    assert_eq!(p.audited()[1], ("storage.usbcompat".to_string(), "off".to_string()));
}

#[test]
fn the_usb_mode_is_refused_for_everyone_else_and_without_a_usb_drive_and_a_failure_is_shown() {
    let p = FakeSys::new(false);
    usb(&p);
    let mut a = app();
    a.handle_storage(&Key::Char('u'), &p);
    assert!(!p.usb_loss_snapshot().compat);
    assert_eq!(p.audited(), vec![("settings.denied".to_string(), a.tab().label().to_string())]);
    let q = FakeSys::new(true);
    a.handle_storage(&Key::Char('u'), &q);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not a USB drive")));
    usb(&q);
    *q.refuse.lock().unwrap() = Some("boot partition is read-only".into());
    a.handle_storage(&Key::Char('u'), &q);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("read-only")));
    assert!(q.audited().is_empty());
}

#[test]
fn restart_asks_first_and_only_for_an_administrator_who_may_power_off() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_storage(&Key::Char('b'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Reboot, _))));
    a.modal = None;
    p.power_allowed.store(false, Ordering::SeqCst);
    a.handle_storage(&Key::Char('b'), &p);
    assert!(a.modal.is_none());
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("may not restart")));
}

#[test]
fn the_sign_out_choice_flips_and_a_failure_is_told() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_storage(&Key::Char('o'), &p);
    assert!(p.signout_close.load(Ordering::SeqCst));
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("closed when you sign out")));
    a.handle_storage(&Key::Char('o'), &p);
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("leave your drives open")));
    *p.refuse.lock().unwrap() = Some("no space".into());
    a.handle_storage(&Key::Char('o'), &p);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no space")));
}

#[test]
fn the_disks_keys_need_the_disks_right_and_ask_the_shell_to_open_disks() {
    let p = FakeSys::new(true);
    let mut a = app();
    a.handle_storage(&Key::Char('d'), &p);
    assert_eq!(a.take_request(), Some(("disks".to_string(), String::new())));
    p.disks_allowed.store(false, Ordering::SeqCst);
    a.handle_storage(&Key::Char('d'), &p);
    a.handle_storage(&Key::Char('o'), &p);
    assert_eq!(a.take_request(), None);
    assert!(!p.signout_close.load(Ordering::SeqCst));
}

#[test]
fn the_page_draws_with_and_without_the_disks_line_and_the_usb_link_at_several_sizes() {
    for (disks, link) in [(true, true), (false, true), (true, false)] {
        let p = FakeSys::new(true);
        p.disks_allowed.store(disks, Ordering::SeqCst);
        if link {
            usb(&p);
        }
        let mut a = app();
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            a.render_storage(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
            assert_eq!(g.hits.iter().any(|x| x.label == "Compatible USB mode"), link, "{w}x{h}: the USB switch shows with a USB link");
        }
    }
}
