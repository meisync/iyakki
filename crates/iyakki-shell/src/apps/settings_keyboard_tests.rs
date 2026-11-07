//! Settings -> Keyboard and function keys: the Fn-key mode written to a fake `hid-apple`, saved in the settings store and
//! audited, the detected controls with their tests, who may change what, and the page at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use std::path::{Path, PathBuf};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto(a: &mut SettingsApp, _cx: &AppCx, t: Tab) {
    a.go_to(t);
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}
fn fake_sys(tag: &str) -> PathBuf {
    let d = temp_dir(&format!("{tag}-sys"));
    put(&d, "module/hid_apple/parameters/fnmode", "3\n");
    put(&d, "module/hid_apple/parameters/iso_layout", "N\n");
    put(&d, "module/hid_apple/parameters/swap_opt_cmd", "N\n");
    put(&d, "bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0001/mode", "2\n");
    put(&d, "class/backlight/intel_backlight/brightness", "40000\n");
    put(&d, "class/backlight/intel_backlight/max_brightness", "96000\n");
    put(&d, "class/leds/smc::kbd_backlight/brightness", "60\n");
    put(&d, "class/leds/smc::kbd_backlight/max_brightness", "255\n");
    d
}
fn read(d: &Path, rel: &str) -> String {
    std::fs::read_to_string(d.join(rel)).unwrap().trim().to_string()
}

#[test]
fn the_function_key_mode_cycles_is_written_to_the_kernel_saved_and_audited() {
    let cx = admin("kb1");
    let sys = fake_sys("kb1");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(sys.clone());
    goto(&mut a, &cx, Tab::Keyboard);
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("media keys first"), "{}", status(&a));
    assert_eq!(read(&sys, "module/hid_apple/parameters/fnmode"), "1");
    assert_eq!(read(&sys, "bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0001/mode"), "2", "the Touch Bar follows: media keys");
    assert_eq!(cx.services.settings.lock().unwrap().get().keyboard.fn_mode, "media", "saved in the settings store");
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("F-keys first"));
    assert_eq!(read(&sys, "module/hid_apple/parameters/fnmode"), "2");
    assert_eq!(read(&sys, "bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0001/mode"), "1");
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(read(&sys, "module/hid_apple/parameters/fnmode"), "3", "automatic");
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(cx.services.settings.lock().unwrap().get().keyboard.fn_mode, "", "back to not changed");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.keyboard") && l.contains("fn_mode=fkeys")), "{:?}", cx.audit.recent(20));
    // ISO layout and the Option/Command swap
    key(&mut a, &cx, Key::Char('i'));
    key(&mut a, &cx, Key::Char('o'));
    assert_eq!(read(&sys, "module/hid_apple/parameters/iso_layout"), "1");
    assert_eq!(read(&sys, "module/hid_apple/parameters/swap_opt_cmd"), "1");
    key(&mut a, &cx, Key::Char('i'));
    assert_eq!(read(&sys, "module/hid_apple/parameters/iso_layout"), "0");
    let k = cx.services.settings.lock().unwrap().get().keyboard.clone();
    assert_eq!((k.iso_layout, k.swap_opt_cmd), (Some(false), Some(true)));
}

#[test]
fn the_choice_is_kept_when_the_machine_has_no_apple_keyboard() {
    let cx = admin("kb2");
    let sys = temp_dir("kb2-sys");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(sys.clone());
    goto(&mut a, &cx, Tab::Keyboard);
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("no Apple keyboard driver"), "{}", status(&a));
    assert_eq!(cx.services.settings.lock().unwrap().get().keyboard.fn_mode, "media", "saved for when a keyboard or driver appears");
    assert!(!sys.join("module").exists(), "nothing is created in the kernel's tree");
}

#[test]
fn key_repeat_is_chosen_from_steps_and_validated() {
    let cx = admin("kb3");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb3"));
    goto(&mut a, &cx, Tab::Keyboard);
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('r'));
    let k = cx.services.settings.lock().unwrap().get().keyboard.clone();
    assert_eq!((k.repeat_delay_ms, k.repeat_rate), (200, 10));
    assert_eq!(crate::input::current_repeat().map(|r| r.1 > 0), Some(true), "the input layer was asked");
    for _ in 0..6 {
        key(&mut a, &cx, Key::Char('d'));
    }
    assert_eq!(cx.services.settings.lock().unwrap().get().keyboard.repeat_delay_ms, 0, "after the last step: the kernel's own again");
}

#[test]
fn the_page_lists_the_detected_controls_and_the_test_buttons_work() {
    let cx = admin("kb4");
    let sys = fake_sys("kb4");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(sys.clone());
    goto(&mut a, &cx, Tab::Keyboard);
    let rows = a.kbd_rows(&cx);
    let text = rows.iter().map(|(l, v, _)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Screen brightness: intel_backlight 42%"), "{text}");
    assert!(text.contains("Keyboard light: smc::kbd_backlight 24%"), "{text}");
    assert!(text.contains("Apple keyboard driver: hid_apple loaded, fnmode 3"), "{text}");
    assert!(text.contains("Touch Bar keys: 1 device(s), mode 2"), "{text}");
    // the screen test dims for a moment and puts it back
    key(&mut a, &cx, Key::Char('b'));
    settle(&mut a, &cx);
    assert_eq!(read(&sys, "class/backlight/intel_backlight/brightness"), "40000", "restored");
    assert!(status(&a).contains("restored"), "{}", status(&a));
    // the keyboard light goes off and on
    key(&mut a, &cx, Key::Char('k'));
    assert!(status(&a).contains("off and on"), "{}", status(&a));
    assert_eq!(read(&sys, "class/leds/smc::kbd_backlight/brightness"), "60");
    // a machine without them says so
    let mut b = SettingsApp::new(&cx).with_keyboard_sys(temp_dir("kb4-empty"));
    goto(&mut b, &cx, Tab::Keyboard);
    let t = b.kbd_rows(&cx).iter().map(|(l, v, _)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n");
    assert!(t.contains("none: no brightness control on this display") && t.contains("Keyboard light: none found") && t.contains("not loaded"), "{t}");
    key(&mut b, &cx, Key::Char('b'));
    settle(&mut b, &cx);
    assert!(status(&b).contains("No brightness control"), "{}", status(&b));
}

#[test]
fn the_table_of_keys_scrolls_and_the_page_draws_at_four_sizes() {
    let cx = admin("kb5");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb5"));
    goto(&mut a, &cx, Tab::Keyboard);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, &cx, 0);
        assert!(g.hits.iter().any(|x| x.label == "Function keys"), "{w}x{h}: the button is there");
        assert!(g.hits.iter().any(|x| x.label.contains("Test screen")), "{w}x{h}");
    }
    let first = a.sel[a.tab];
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    assert_eq!(a.sel[a.tab], first + 2);
    a.handle(&Event::Scroll(-1), &cx);
    assert_eq!(a.sel[a.tab], first + 3, "the wheel scrolls the table too");
}

#[test]
fn only_administrators_have_the_page_and_a_forced_request_changes_nothing() {
    let (mut ucx, _) = (admin("kb6"), 0);
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let sys = fake_sys("kb6");
    let mut a = SettingsApp::new(&ucx).with_keyboard_sys(sys.clone());
    assert!(!a.tabs().contains(&Tab::Keyboard));
    a.tabs.push(Tab::Keyboard);
    a.tab = a.tabs.len() - 1;
    key(&mut a, &ucx, Key::Char('f'));
    assert_eq!(read(&sys, "module/hid_apple/parameters/fnmode"), "3");
    assert_eq!(ucx.services.settings.lock().unwrap().get().keyboard.fn_mode, "");
    assert!(ucx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
    let cx = admin("kb6b");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Keyboard));
}

#[test]
fn the_touch_bar_choices_cycle_save_and_are_audited() {
    let cx = admin("kb7");
    let sys = fake_sys("kb7");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(sys.clone());
    goto(&mut a, &cx, Tab::Keyboard);
    let get = || cx.services.settings.lock().unwrap().get().keyboard.clone();
    for want in ["strip", "keys", "off", ""] {
        key(&mut a, &cx, Key::Char('u'));
        assert_eq!(get().touchbar, want);
    }
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.keyboard") && l.contains("touchbar=off")));
    key(&mut a, &cx, Key::Char('e'));
    assert_eq!(get().touchbar_strips, ["system", "recent"]);
    for _ in 0..7 {
        key(&mut a, &cx, Key::Char('e'));
    }
    assert!(get().touchbar_strips.is_empty(), "eight steps and it is back to all four");
    for want in [1, 2, 3, 0] {
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(get().touchbar_flip, want);
    }
    assert!(get().validate().is_ok());
    // "off" darkens the firmware's keys at once, "keys" returns them in the chosen mode
    let m = sys.join("bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(m.join("mode"), "2\n").unwrap();
    key(&mut a, &cx, Key::Char('u'));
    key(&mut a, &cx, Key::Char('u'));
    key(&mut a, &cx, Key::Char('u'));
    assert_eq!(get().touchbar, "off");
    assert_eq!(read(&sys, "bus/hid/drivers/hid-appletb-kbd/0003:05AC:8302.0005/mode"), "3");
    // the page says what the strip does and that it is display only
    let t = (0..1).map(|_| a.kbd_rows(&cx).iter().map(|(l, v, _)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n")).next().unwrap();
    assert!(t.contains("Touch Bar: off (dark)") && t.contains("Touch Bar strips: system, recent, ai, app") && t.contains("device(s), mode 3"), "{t}");
    assert!(t.contains("Touch Bar strip: not started") || t.contains("display only"), "{t}");
}

#[test]
fn the_touch_bar_buttons_are_inside_the_page_at_every_size_and_in_panes() {
    let cx = admin("kb8");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb8"));
    goto(&mut a, &cx, Tab::Keyboard);
    for (w, h) in [(360u32, 740u32), (800, 480), (1024, 600), (1280, 720), (1920, 1080), (2560, 1600), (640, 480), (420, 300)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
        a.render(&mut g, area, s, &cx, 0);
        for label in ["Touch Bar", "Touch Bar strips", "Touch Bar flip"] {
            let hit = g.hits.iter().find(|x| x.label == label);
            if let Some(hit) = hit {
                let r = hit.rect;
                assert!(r.w > 0.0 && r.h > 0.0 && r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h} {label}: {r:?} outside the screen");
            }
        }
        assert!(g.hits.iter().any(|x| x.label == "Touch Bar"), "{w}x{h}: the Touch Bar button is reachable");
    }
}

// ---- scrolling: the choices, the Scroll test and the path from raw events to a list

fn render_page(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
}

#[test]
fn the_scroll_choices_are_saved_audited_applied_and_have_defaults_and_limits() {
    let cx = admin("kb-scroll");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-scroll"));
    goto(&mut a, &cx, Tab::Keyboard);
    let sc = |cx: &AppCx| cx.services.settings.lock().unwrap().get().keyboard.scroll();
    assert_eq!((sc(&cx).speed_pct, sc(&cx).wheel_lines, sc(&cx).smooth, sc(&cx).momentum, sc(&cx).wheel_natural, sc(&cx).pad_natural), (100, 3, true, true, false, true), "defaults");
    let rows = a.kbd_rows(&cx);
    assert!(rows.iter().any(|(l, v, _)| l == "Scrolling" && v.contains("speed 100%") && v.contains("3 lines")), "{rows:?}");
    // the speed steps through its list and wraps; every value is inside 50..=300
    let mut seen = vec![];
    for _ in 0..8 {
        key(&mut a, &cx, Key::Char('s'));
        seen.push(sc(&cx).speed_pct);
    }
    assert_eq!(seen, vec![150, 200, 250, 300, 50, 75, 100, 150]);
    assert!(seen.iter().all(|v| (50..=300).contains(v)));
    assert_eq!(crate::scrolleng::config().speed_pct, 150, "applied to the engine at once");
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(sc(&cx).wheel_lines, 5);
    assert_eq!(crate::scrolleng::config().wheel_lines, 5);
    for k in ['w', 'g', 'm', 'n'] {
        key(&mut a, &cx, Key::Char(k));
    }
    let c = crate::scrolleng::config();
    assert_eq!((c.wheel_natural, c.smooth, c.momentum, c.pad_natural), (true, false, false, false), "each switch flipped");
    let saved = cx.services.settings.lock().unwrap().get().keyboard.clone();
    assert!(saved.validate().is_ok());
    crate::scrolleng::request_config(Default::default());
}

#[test]
fn only_administrators_change_the_scroll_choices() {
    let mut cx = admin("kb-scroll-user");
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-scroll-user"));
    a.tabs.push(Tab::Keyboard);
    goto(&mut a, &cx, Tab::Keyboard);
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(cx.services.settings.lock().unwrap().get().keyboard.scroll_speed_pct, 0, "not changed");
}

#[test]
fn the_scroll_test_list_scrolls_by_pixels_in_small_steps_and_by_notches_and_stays_in_bounds() {
    let cx = admin("kb-test");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-test"));
    goto(&mut a, &cx, Tab::Keyboard);
    render_page(&mut a, &cx, 1920, 1080);
    let v = a.kbd.test.view;
    assert!(v.h > 60.0 && a.kbd.test.scrolls(), "the test list is on the page and taller than its view");
    // the pointer outside it: the steps are not taken
    a.handle(&Event::MouseMove(2, 2), &cx);
    assert!(!a.scroll_px(-10, &cx) || a.kbd.test.offset == 0.0);
    a.handle(&Event::MouseMove((v.x + 20.0) as i32, (v.y + 20.0) as i32), &cx);
    assert!(a.scroll_px(-10, &cx), "pixels over the list are taken");
    let one = a.kbd.test.offset;
    assert!(one > 0.0 && one < a.kbd.test.row, "a small step moves less than a row: {one}");
    for _ in 0..8 {
        a.scroll_px(-10, &cx);
    }
    assert!(a.kbd.test.offset > 3.0 * one, "the steps add up");
    // whole notches work over it too, and it never leaves its bounds
    a.handle(&Event::Scroll(-1), &cx);
    for _ in 0..500 {
        a.handle(&Event::Scroll(-3), &cx);
    }
    assert!((a.kbd.test.offset - a.kbd.test.max).abs() < 0.01, "stops at the end");
    for _ in 0..500 {
        a.scroll_px(400, &cx);
    }
    assert!(a.kbd.test.offset.abs() < 0.01, "and at the start, never negative");
    // a wheel over the page's other parts still scrolls the key table
    a.handle(&Event::MouseMove(2, 2), &cx);
    let first = a.sel[a.tab];
    a.handle(&Event::Scroll(-1), &cx);
    assert!(a.sel[a.tab] > first);
}

#[test]
fn raw_wheel_events_reach_the_settings_list_as_smooth_pixels_end_to_end() {
    crate::scrolleng::request_config(crate::scrolleng::ScrollConfig::default());
    crate::scrolleng::set_raw(false);
    let cx = admin("kb-e2e");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-e2e"));
    goto(&mut a, &cx, Tab::Keyboard);
    render_page(&mut a, &cx, 1920, 1080);
    let v = a.kbd.test.view;
    a.handle(&Event::MouseMove((v.x + 20.0) as i32, (v.y + 20.0) as i32), &cx);
    let mut st = crate::input::InputState::new(1920, 1080);
    st.fake_clock(0);
    // one click of a hi-res wheel, rolled towards the person (down the list)
    st.feed(crate::input::Raw { typ: crate::input::EV_REL, code: 11, value: -120 }, None);
    st.feed(crate::input::Raw { typ: crate::input::EV_REL, code: 8, value: -1 }, None);
    st.feed(crate::input::Raw { typ: crate::input::EV_SYN, code: 0, value: 0 }, None);
    let mut offsets = vec![];
    for t in (0..400).step_by(4) {
        st.fake_clock(t);
        for e in st.scroll_tick() {
            if let Event::ScrollPx(px) = e {
                assert!(a.scroll_px(px, &cx));
                offsets.push(a.kbd.test.offset);
            }
        }
    }
    assert!(offsets.len() >= 5, "several frames, not one jump: {offsets:?}");
    assert!(offsets.windows(2).all(|w| w[1] >= w[0]), "monotonic: {offsets:?}");
    let expect = 3.0 * a.kbd.test.row; // a click is three rows
    assert!((a.kbd.test.offset - expect).abs() <= 2.0, "one click = three rows ({} of {expect})", a.kbd.test.offset);
}

#[test]
fn the_probe_touch_screen_button_switches_the_panel_off_and_on_and_a_second_press_resets_the_hub() {
    let cx = admin("kb-touch");
    let sys = temp_dir("kb-touch-sys");
    let hub = sys.join("devices/usb1/1-5");
    let usb = hub.join("1-5.3");
    put(&hub, "idVendor", "1a40\n");
    put(&usb, "idVendor", "27c0\n");
    put(&hub, "authorized", "1");
    put(&usb, "authorized", "1");
    put(&usb, "1-5.3:1.0/input/input4/name", "wch.cn TouchScreen\n");
    std::fs::create_dir_all(sys.join("class/input")).unwrap();
    std::os::unix::fs::symlink(usb.join("1-5.3:1.0/input/input4"), sys.join("class/input/input4")).unwrap();
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(sys);
    goto(&mut a, &cx, Tab::Keyboard);
    let text = a.kbd_rows(&cx).iter().map(|(l, v, _)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Touch screen: 1 found") && text.contains("Probe touch screen"), "{text}");
    assert!(a.keyboard_buttons().iter().any(|b| b.0 == "Probe touch screen"));
    key(&mut a, &cx, Key::Char('h'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("switched off and on") && status(&a).contains("press the button again"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('h'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("USB hub"), "{}", status(&a));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.touch.probe")));
}

#[test]
fn the_input_profile_cycles_applies_at_once_is_saved_and_old_files_load() {
    let cx = admin("kb-profile");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-profile"));
    goto(&mut a, &cx, Tab::Keyboard);
    let prof = |cx: &AppCx| cx.services.settings.lock().unwrap().get().keyboard.profile.clone();
    assert_eq!(prof(&cx), "", "Automatic is the default");
    let rows = a.kbd_rows(&cx);
    assert!(rows[0].0 == "Input profile" && rows[0].1.starts_with("Automatic"), "{rows:?}");
    assert!(rows.iter().any(|r| r.0 == "What you will see: last key") && rows.iter().any(|r| r.0 == "What you will see: last scroll"));
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!((prof(&cx).as_str(), crate::inputprofile::resolved()), ("mac", iyakki_agent::inputprofile::Resolved::Mac), "live without a restart");
    assert!(a.kbd_rows(&cx)[0].1.starts_with("Mac: Mac (chosen by you)"), "{:?}", a.kbd_rows(&cx)[0]);
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!((prof(&cx).as_str(), crate::inputprofile::resolved()), ("pc", iyakki_agent::inputprofile::Resolved::Pc));
    key(&mut a, &cx, Key::Char('v'));
    assert_eq!(prof(&cx), "");
    // the swap row says in one line that the two never swap twice
    assert!(SettingsApp::kbd_help("ISO layout / Option-Command swap").unwrap().contains("never swaps twice"));
    // an old settings file without `profile` loads as Automatic
    let old: iyakki_agent::keyboard::KeyboardSettings = serde_json::from_str("{\"swap_opt_cmd\":true}").unwrap();
    assert_eq!(old.profile, "");
}

#[test]
fn reset_to_profile_defaults_puts_the_pointer_choices_back_and_leaves_the_keyboard_alone() {
    let cx = admin("kb-reset");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-reset"));
    goto(&mut a, &cx, Tab::Keyboard);
    key(&mut a, &cx, Key::Char('v')); // Mac
    key(&mut a, &cx, Key::Char('n')); // trackpad natural off
    key(&mut a, &cx, Key::Char('w')); // wheel natural on
    key(&mut a, &cx, Key::Char('s')); // speed 150
    key(&mut a, &cx, Key::Char('r')); // repeat rate
    let k = |cx: &AppCx| cx.services.settings.lock().unwrap().get().keyboard.clone();
    assert_eq!((k(&cx).trackpad().1, k(&cx).scroll().wheel_natural, k(&cx).scroll().speed_pct), (false, true, 150));
    key(&mut a, &cx, Key::Char('x'));
    let after = k(&cx);
    assert_eq!((after.trackpad(), after.scroll().wheel_natural, after.scroll().speed_pct), ((true, true), false, 100));
    assert_eq!((after.profile.as_str(), after.repeat_rate), ("mac", 10), "profile and key repeat are kept");
    assert!(status(&a).contains("Mac profile's defaults"), "{}", status(&a));
}

#[test]
fn every_row_with_a_control_has_a_help_line_and_the_buttons_exist() {
    let cx = admin("kb-help");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(fake_sys("kb-help"));
    goto(&mut a, &cx, Tab::Keyboard);
    for l in ["Input profile", "Function keys", "Trackpad", "Scrolling", "Touch Bar", "Touch Bar strips", "ISO layout / Option-Command swap", "Key repeat"] {
        let h = SettingsApp::kbd_help(l).unwrap_or_else(|| panic!("no help for {l}"));
        assert!(h.len() > 20 && h.len() < 220, "{l}: {h}");
    }
    let names: Vec<_> = a.keyboard_buttons().iter().map(|b| b.0).collect();
    assert!(names.contains(&"Input profile") && names.contains(&"Reset to profile defaults"), "{names:?}");
    // the existing buttons are all still there
    for n in ["Function keys", "Touch Bar", "ISO layout", "Swap Option/Cmd", "Repeat delay", "Repeat rate", "Trackpad tap", "Trackpad scroll", "Wheel direction", "Smooth scrolling", "Momentum", "Test keyboard light", "Test sound"] {
        assert!(names.contains(&n), "{n}");
    }
}

#[test]
fn the_reset_touch_screen_button_is_next_to_the_probe_button_and_a_plain_status_line_is_shown() {
    let cx = admin("kb-touch-reset");
    let mut a = SettingsApp::new(&cx).with_keyboard_sys(temp_dir("kb-touch-reset-sys"));
    goto(&mut a, &cx, Tab::Keyboard);
    // mouse, touch and keyboard all reach the same button: it has a label and a key
    assert!(a.keyboard_buttons().iter().any(|b| b.0 == "Reset touch screen" && b.2 == 'z'));
    assert!(a.keyboard_buttons().iter().any(|b| b.0 == "Probe touch screen"));
    let text = a.kbd_rows(&cx).iter().map(|(l, v, _)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Touch recovery: ") && text.contains("Reset touch screen"), "{text}");
}
