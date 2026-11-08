//! Settings -> This computer: the page names the computer and the others, the category switches move a kind of settings between "per
//! computer" and "shared", copying, resetting and forgetting ask first, only administrators change anything, and every control is on
//! screen at the test sizes. The computers are fake (no real DMI, no second real computer).

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::hwprofile::{Category, Computers, HardwareFacts};
use iyakki_sec::policy::Role;

struct Fake(&'static str, &'static str);
impl HardwareFacts for Fake {
    fn dmi(&self) -> iyakki_hwinfo::DmiIdentity {
        iyakki_hwinfo::DmiIdentity { sys_vendor: Some(self.0.into()), product_name: Some(self.1.into()), ..Default::default() }
    }
}

const SIZES: [(u32, u32); 6] = [(390, 844), (600, 1000), (800, 1280), (1280, 800), (1920, 1080), (844, 390)];

/// An administrator on a Macmini8,1 that has also been used on a Legion; the Legion is the one running now when `on_legion`.
fn rig(tag: &str, on_legion: bool) -> (AppCx, SettingsApp) {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    {
        let mut st = cx.services.settings.lock().unwrap();
        let mac = Computers::open(cx.services.dir(), &Fake("Apple Inc.", "Macmini8,1"), 1_700_000_000, st.get());
        st.attach_computers(mac);
        st.update(|s| {
            s.sound.output = "PCH:3".into();
            s.keyboard.fn_mode = "media".into();
        })
        .unwrap();
        if on_legion {
            let lg = Computers::open(cx.services.dir(), &Fake("LENOVO", "82WM"), 1_700_100_000, st.get());
            st.attach_computers(lg);
        }
    }
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Computer);
    (cx, a)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: char) {
    a.handle(&Event::Key(Key::Char(k)), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn computers(cx: &AppCx) -> Computers {
    cx.services.settings.lock().unwrap().computers().unwrap().clone()
}

#[test]
fn the_page_is_in_the_system_group_and_the_search_and_the_inbox_link_find_it() {
    let (cx, a) = rig("pc-tab", true);
    assert!(a.tabs.contains(&Tab::Computer) && Tab::ORDER.contains(&Tab::Computer));
    assert_eq!(Tab::Computer.group(), "System");
    assert_eq!(Tab::Computer.label(), "This computer");
    assert!(Tab::Computer.keywords().contains("usb") && Tab::Computer.keywords().contains("laptop"));
    let mut b = SettingsApp::new(&cx);
    b.open_item("tab:computer", &cx);
    assert_eq!(b.tab(), Tab::Computer);
}

#[test]
fn a_category_switch_moves_a_kind_of_settings_between_per_computer_and_shared() {
    let (cx, mut a) = rig("pc-share", true);
    assert!(computers(&cx).shared().is_empty());
    key(&mut a, &cx, 'k');
    assert!(computers(&cx).shared().contains(&Category::Keyboard));
    assert!(a.status().is_some_and(|s| s.1 && s.0.contains("shared")), "{:?}", a.status());
    key(&mut a, &cx, 'k');
    assert!(computers(&cx).shared().is_empty());
    for k in ['s', 'd', 'm', 'g', 'p'] {
        key(&mut a, &cx, k);
    }
    assert_eq!(computers(&cx).shared().len(), 5);
}

#[test]
fn only_an_administrator_changes_anything() {
    let (mut cx, mut a) = rig("pc-user", true);
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    key(&mut a, &cx, 's');
    key(&mut a, &cx, 'r');
    assert!(computers(&cx).shared().is_empty());
    assert!(a.modal.is_none());
    assert!(a.status().is_some_and(|s| !s.1));
}

#[test]
fn copying_resetting_and_forgetting_ask_first_and_then_work() {
    let (cx, mut a) = rig("pc-ops", true);
    assert_eq!(cx.services.settings.lock().unwrap().get().sound.output, "", "the Legion starts from automatic");
    // copy
    key(&mut a, &cx, 'c');
    let Some(Modal::Pick(PickKind::Computer(_), _)) = &a.modal else { panic!("no list of computers") };
    a.modal = None;
    let mac = computers(&cx).list().into_iter().find(|p| p.label == "Apple Macmini8,1").unwrap();
    a.computer_pick(&cx, computer_page::CompPick::Copy, &format!("{} ({})", mac.label, mac.id));
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Computer(computer_page::CompAct::Copy(id)), _)) if *id == mac.id));
    assert_eq!(cx.services.settings.lock().unwrap().get().sound.output, "", "nothing changed before the yes");
    key(&mut a, &cx, 'y');
    assert_eq!(cx.services.settings.lock().unwrap().get().sound.output, "PCH:3");
    // reset
    key(&mut a, &cx, 'r');
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Computer(computer_page::CompAct::Reset), _))));
    key(&mut a, &cx, 'n');
    assert_eq!(cx.services.settings.lock().unwrap().get().sound.output, "PCH:3", "no keeps it");
    key(&mut a, &cx, 'r');
    key(&mut a, &cx, 'y');
    assert_eq!(cx.services.settings.lock().unwrap().get().sound.output, "");
    // forget
    key(&mut a, &cx, 'f');
    a.modal = None;
    a.computer_pick(&cx, computer_page::CompPick::Forget, &format!("{} ({})", mac.label, mac.id));
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Computer(computer_page::CompAct::Forget(_)), _))));
    key(&mut a, &cx, 'y');
    assert_eq!(computers(&cx).list().len(), 1);
    // nobody else left to copy from or forget
    key(&mut a, &cx, 'c');
    assert!(a.modal.is_none() && a.status().is_some_and(|s| !s.1));
}

#[test]
fn a_install_without_a_profile_says_so() {
    let mut cx = cx_with_admin_store("pc-none");
    cx.services = services_in(&temp_dir("pc-none-svc"), Arc::new(Offline));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Computer);
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().all(|h| !matches!(h.ev, Event::Key(Key::Char('s')))));
    key(&mut a, &cx, 's');
    assert!(a.status().is_some_and(|s| !s.1));
}

#[test]
fn every_control_is_on_screen_at_the_test_sizes() {
    let (cx, _) = rig("pc-sizes", true);
    for (w, h) in SIZES {
        let mut a = SettingsApp::new(&cx);
        a.go_to(Tab::Computer);
        let g = draw(&mut a, &cx, w, h);
        for k in ['s', 'd', 'k', 'm', 'g', 'p'] {
            let hit = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char(k)) && x.rect.w > 0.0).unwrap_or_else(|| panic!("{w}x{h}: no switch for {k}"));
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y >= 0.0 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {k} {r:?} is not on screen");
        }
        // the Back/Copy/Reset/Forget buttons are on screen too
        for k in ['c', 'r', 'f'] {
            assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char(k)) && x.rect.w > 0.0 && x.rect.x + x.rect.w <= w as f32 + 0.5 && x.rect.y + x.rect.h <= h as f32 + 0.5), "{w}x{h}: no button {k}");
        }
        // the right edge stays clear
        let px = g.rgba();
        for y in 0..h as usize {
            for dx in [1usize, 2] {
                let o = (y * w as usize + (w as usize - dx)) * 4;
                assert_eq!(&px[o..o + 3], &px[0..3], "{w}x{h}: drawing reaches the right edge at row {y}; hits {:?}", g.hits.iter().map(|x| (x.label.clone(), x.rect)).collect::<Vec<_>>());
            }
        }
    }
}
