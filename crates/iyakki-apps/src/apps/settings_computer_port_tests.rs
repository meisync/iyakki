//! Settings -> This computer on a fake port: sharing a kind flips through the port and is audited, copy, reset and forget ask first and then
//! act through the port and are audited, the startup-disk keys ask first and go to the port, a non-administrator is refused and audited,
//! an install without per-computer settings says so, and the page draws at several sizes. No sleeps, no firmware.

use super::computer_page::{CompAct, CompPick};
use super::system_port::{ComputerPort, FakeSys};
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_agent_api::ComputerPage;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("computer-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Computer);
    a
}

fn two_computers(p: &FakeSys) {
    *p.computer.lock().unwrap() = Some(ComputerPage {
        me_id: "me1".into(),
        me_label: "This Mac".into(),
        shared: Default::default(),
        me_seen: Some((3, 1_700_000_000, 1_790_000_000)),
        all: vec![("me1".into(), "This Mac".into(), 1_790_000_000), ("pc2".into(), "Lenovo".into(), 1_780_000_000)],
    });
}

#[test]
fn a_kind_is_switched_between_per_computer_and_shared_through_the_port_and_audited() {
    let p = FakeSys::new(true);
    two_computers(&p);
    let mut a = app();
    a.handle_computer(&Key::Char('s'), &p);
    let calls = p.comp_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("sharing ") && calls[0].ends_with("=true"), "{calls:?}");
    assert_eq!(p.audited().len(), 1);
    assert_eq!(p.audited()[0].0, "settings.computer.sharing");
    assert!(a.status().is_some_and(|s| s.1 && s.0 == "Changed."));
}

#[test]
fn copy_reset_and_forget_ask_first_then_act_and_are_audited() {
    let p = FakeSys::new(true);
    two_computers(&p);
    let mut a = app();
    a.handle_computer(&Key::Char('r'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Computer(CompAct::Reset), _))));
    assert!(p.comp_calls.lock().unwrap().is_empty(), "nothing before the yes");
    a.modal = None;
    a.handle_computer(&Key::Char('c'), &p);
    assert!(matches!(a.modal, Some(Modal::Pick(PickKind::Computer(CompPick::Copy), _))));
    a.modal = None;
    a.computer_pick(&p, CompPick::Forget, "Lenovo (pc2)");
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Computer(CompAct::Forget(id)), t)) if id == "pc2" && t.contains("Lenovo")));
    a.computer_confirm(&p, CompAct::Forget("pc2".into()));
    a.computer_confirm(&p, CompAct::Copy("pc2".into()));
    a.computer_confirm(&p, CompAct::Reset);
    assert_eq!(p.comp_calls.lock().unwrap().clone(), vec!["forget pc2", "copy pc2", "reset"]);
    assert_eq!(p.audited().iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), vec!["forget pc2", "copy pc2", "reset"]);
}

#[test]
fn with_no_other_computer_copy_and_forget_say_so() {
    let p = FakeSys::new(true);
    two_computers(&p);
    p.computer.lock().unwrap().as_mut().unwrap().all.truncate(1);
    let mut a = app();
    a.handle_computer(&Key::Char('f'), &p);
    assert!(a.modal.is_none());
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No other computer")));
}

#[test]
fn the_startup_disk_keys_ask_first_and_the_yes_goes_to_the_port_with_a_refusal_shown() {
    let p = FakeSys::new(true);
    let mut a = app();
    for (k, act) in [('e', CompAct::StartRegister), ('b', CompAct::StartFirst), ('n', CompAct::StartNext), ('v', CompAct::StartRevert)] {
        a.modal = None;
        a.handle_computer(&Key::Char(k), &p);
        assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Computer(x), _)) if *x == act), "{k}");
        a.computer_confirm(&p, act);
    }
    assert_eq!(p.comp_calls.lock().unwrap().len(), 4);
    a.handle_computer(&Key::Char('w'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Computer(CompAct::RebootInto(true)), _))));
    a.computer_confirm(&p, CompAct::RebootInto(true));
    assert!(p.reboot_into_iyakki());
    *p.refuse.lock().unwrap() = Some("the firmware variables are read-only".into());
    a.computer_confirm(&p, CompAct::StartFirst);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("read-only")));
}

#[test]
fn others_change_nothing_and_without_per_computer_settings_the_page_says_so() {
    let p = FakeSys::new(false);
    two_computers(&p);
    let mut a = app();
    a.handle_computer(&Key::Char('s'), &p);
    a.handle_computer(&Key::Char('e'), &p);
    a.computer_confirm(&p, CompAct::Reset);
    assert!(p.comp_calls.lock().unwrap().is_empty());
    assert!(a.modal.is_none());
    assert!(p.audited().iter().all(|l| l.0 == "settings.denied"));
    let q = FakeSys::new(true);
    a.handle_computer(&Key::Char('s'), &q);
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no per-computer settings")));
}

#[test]
fn the_page_draws_with_and_without_a_profile_at_several_sizes() {
    for (profile, admin) in [(true, true), (true, false), (false, true)] {
        let p = FakeSys::new(admin);
        if profile {
            two_computers(&p);
        }
        let mut a = app();
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            a.render_computer(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, &p);
            assert!(g.hits.iter().all(|x| x.rect.x >= -0.5 && x.rect.x + x.rect.w <= w as f32 + 0.5), "{w}x{h}");
        }
    }
}
