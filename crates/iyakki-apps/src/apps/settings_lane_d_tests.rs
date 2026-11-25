//! Settings tests of lane ST-D moved out of `settings.rs` (S0).

use super::*;
use crate::apps::tests::temp_dir;
use iyakki_sec::policy::Role;
use std::sync::Mutex;
use super::tests::*;

#[test]
fn the_hardware_pages_are_admin_only() {
    let ucx = user("hw1");
    let a = SettingsApp::new(&ucx);
    for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
        assert!(!a.tabs().contains(&t), "{t:?} is for administrators");
    }
    let (cx, _) = admin("hw2");
    let a = SettingsApp::new(&cx);
    for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
        assert!(a.tabs().contains(&t), "{t:?}");
    }
}

#[test]
fn a_non_admin_who_forces_the_page_cannot_switch_anything() {
    let ucx = user("hw4");
    let (env, ctl) = fake_machine("hw4");
    let mut a = SettingsApp::new(&ucx).with_hardware(env);
    a.tabs.push(Tab::Network); // a hand-built request, as the storage test does
    a.tab = a.tabs.len() - 1;
    key(&mut a, &ucx, Key::Enter);
    assert!(ctl.0.lock().unwrap().is_empty(), "the actuator was never called");
}

#[test]
fn the_hardware_pages_render_with_clickable_rows_and_all_tabs_fit() {
    let (cx, _) = admin("hw5");
    let (env, _) = fake_machine("hw5");
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    // tall enough for the whole list of pages (it grows with every page that is added; Developer mode made it 1080)
    let mut g = Gfx::new(1280, 3200);
    for t in [Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Sound] {
        goto(&mut a, &cx, t);
        g.clear(rgb(0, 0, 0));
        g.hits.clear();
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 3128.0), 1.0, &cx, 0);
        if t == Tab::Bluetooth {
            // the Bluetooth page has its own controls (the switch, Advanced) instead of generic rows
            assert!(g.hits.iter().any(|h| h.label == "Bluetooth") && g.hits.iter().any(|h| h.label == "Advanced"), "{t:?} controls");
            continue;
        }
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(0))), "{t:?} rows are clickable");
        assert!(g.hits.iter().any(|h| h.label == "Look again"), "{t:?} has its buttons");
    }
    // every tab can be clicked even when the bar wraps onto a second line
    for (i, _) in a.tabs().to_vec().iter().enumerate() {
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(i)), "tab {i} is on screen");
    }
}

#[test]
fn brightness_and_volume_have_sliders_whose_buttons_and_track_work() {
    let (cx, _) = admin("sld3");
    let d = temp_dir("sld3-hw");
    let put = |rel: &str, c: &str| {
        let p = d.join("sys").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    };
    put("class/backlight/bl0/max_brightness", "100\n");
    put("class/backlight/bl0/brightness", "50\n");
    put("class/graphics/fb0/virtual_size", "1280,720\n");
    let ctl = Arc::new(FakeCtl(Mutex::new(vec![])));
    let env = DeviceEnv { roots: iyakki_core_api::device_types::devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl };
    let mut a = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut a, &cx, Tab::Display);
    let g = draw(&mut a, &cx, 1280, 720);
    let row = a.dev_rows().iter().position(|r| r.slider.is_some()).expect("a brightness slider");
    assert!(g.hits.iter().any(|h| h.ev == Event::Area(SLIDER_HW + row)), "its track takes clicks");
    let bl = |d: &std::path::Path| std::fs::read_to_string(d.join("sys/class/backlight/bl0/brightness")).unwrap().trim().to_string();
    a.handle(&Event::Sub(row as u8, Key::Char('+')), &cx);
    assert_eq!(bl(&d), "60", "the + button on the card steps by ten");
    let w = a.slider_w.iter().find(|(i, _)| *i == SLIDER_HW + row).unwrap().1;
    a.handle(&Event::AreaAt(SLIDER_HW + row, w * 0.8, 5.0), &cx);
    assert_eq!(bl(&d), "80", "a click on the track sets that brightness");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("brightness bl0 80")));
    a.handle(&Event::AreaAt(SLIDER_HW + row, 0.0, 5.0), &cx);
    assert_eq!(bl(&d), "5", "never fully dark");
}

#[test]
fn the_display_tab_makes_text_and_buttons_bigger_or_smaller_for_the_person_and_keeps_it() {
    let (cx, _) = admin("uisize1");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Display);
    let normal = crate::ui::ui_size();
    key(&mut a, &cx, Key::Char('u'));
    assert_eq!(crate::uisize::load(cx.data_dir(), "root"), crate::uisize::NORMAL + 1);
    assert_eq!(crate::ui::ui_size(), 1.25);
    assert!(status(&a).contains("Large (125%)"), "{}", status(&a));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Char('u'));
    }
    assert_eq!(crate::ui::ui_size(), 2.0);
    assert!(status(&a).contains("largest"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('U'));
    assert_eq!(crate::ui::ui_size(), 1.5);
    // the page draws the choices as buttons, the current one off
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Huge")) && g.hits.iter().any(|h| h.label.starts_with("Small")));
    assert!(!g.hits.iter().any(|h| h.label.starts_with("Larger")), "the current one is not a button");
    crate::ui::set_ui_size(normal);
    // an ordinary account can change its own size too
    let mut cx2 = cx.clone();
    cx2.session.user = "bea".into();
    cx2.session.role = Role::User;
    cx2.session.policy = Role::User.template();
    let mut b = SettingsApp::new(&cx2);
    assert!(!b.tabs().contains(&Tab::Display), "the hardware pages are for administrators");
    goto(&mut b, &cx2, Tab::Account);
    key(&mut b, &cx2, Key::Char('U'));
    let mut g2 = Gfx::new(1280, 720);
    g2.clear(rgb(9, 11, 20));
    b.render(&mut g2, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx2, 0);
    assert!(g2.hits.iter().any(|h| h.label.starts_with("Normal")), "the Account page offers the sizes");
    assert_eq!(crate::uisize::load(cx2.data_dir(), "bea"), 0);
    assert_eq!(crate::uisize::load(cx2.data_dir(), "root"), 4 - 1, "root's own choice is separate");
    crate::ui::set_ui_size(1.0);
}

#[test]
fn the_theme_row_on_display_and_account_switches_at_once_marks_the_choice_and_is_kept_per_person() {
    use crate::theme::{current_id, load, ThemeId};
    let render = |a: &mut SettingsApp, cx: &AppCx| {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, cx, 0);
        g
    };
    let (cx, _) = admin("theme1");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Display);
    let dark = render(&mut a, &cx);
    assert_eq!(current_id(), ThemeId::Dark, "Dark is the starting value");
    for name in ["\u{2713} Dark", "Light", "High contrast"] {
        assert!(dark.hits.iter().any(|h| h.label == name), "{name} is an option; the chosen one carries a tick: {:?}", dark.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    }
    // a click on "Light" (it sends its key) applies at once and is kept
    let click = dark.hits.iter().find(|h| h.label == "Light").map(|h| h.ev.clone()).unwrap();
    a.handle(&click, &cx);
    assert_eq!(current_id(), ThemeId::Light);
    assert_eq!(load(cx.data_dir(), "root"), ThemeId::Light);
    assert!(status(&a).contains("Light"), "{}", status(&a));
    let light = render(&mut a, &cx);
    assert!(light.hits.iter().any(|h| h.label == "\u{2713} Light") && light.hits.iter().any(|h| h.label == "Dark"));
    // same layout (regions and their order), different pixels
    // (the three theme buttons themselves differ by the width of the tick on the chosen one)
    let shape = |g: &Gfx| g.hits.iter().filter(|h| !h.label.contains("Dark") && !h.label.contains("Light") && !h.label.contains("High contrast")).map(|h| (h.rect.x as i32, h.rect.y as i32, h.rect.w as i32, h.rect.h as i32)).collect::<Vec<_>>();
    assert_eq!(shape(&dark), shape(&light), "the layout does not move with the theme");
    assert_ne!(dark.rgba(), light.rgba(), "the colours do");
    key(&mut a, &cx, Key::Char('t'));
    assert_eq!(current_id(), ThemeId::HighContrast, "t picks the next");
    let hc = render(&mut a, &cx);
    assert_eq!(shape(&dark), shape(&hc), "nor with High contrast");
    assert_ne!(hc.rgba(), light.rgba());
    key(&mut a, &cx, Key::Char('T'));
    key(&mut a, &cx, Key::Char('T'));
    assert_eq!(current_id(), ThemeId::Dark, "Shift+T goes back");
    // the text size is untouched by all of this
    assert_eq!(crate::uisize::load(cx.data_dir(), "root"), crate::uisize::NORMAL);
    // an ordinary account has the row on its Account page, its own choice, root's stays
    let mut cx2 = cx.clone();
    cx2.session.user = "bea".into();
    cx2.session.role = Role::User;
    cx2.session.policy = Role::User.template();
    let mut b = SettingsApp::new(&cx2);
    goto(&mut b, &cx2, Tab::Account);
    let acc = render(&mut b, &cx2);
    assert!(acc.hits.iter().any(|h| h.label == "High contrast") && acc.hits.iter().any(|h| h.label.starts_with("Large")), "Theme and size sit together on Account");
    key(&mut b, &cx2, Key::Char('3'));
    assert_eq!(load(cx2.data_dir(), "bea"), ThemeId::HighContrast);
    assert_eq!(load(cx.data_dir(), "root"), ThemeId::Dark, "root's own choice is separate");
    crate::theme::set_current(ThemeId::Dark);
}
