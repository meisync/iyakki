//! The sideways swipe between the groups of Home: it turns the page with a short slide, and does nothing over an app, a dialog or a
//! focused text field, with the setting off, or from a finger that began outside the group area.

use super::tests::*;
use super::*;

/// This thread's swipe choices, whatever other tests (a login applies the saved ones) set for the process.
fn choose(on: bool, fingers: u8) {
    crate::input::home_swipe_for_this_thread(Some((on, fingers)));
}

fn home() -> (Shell, Gfx) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (mut sh, _) = shell_with_users(&format!("swipe{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    (sh, g)
}

fn swipe(sh: &mut Shell, back: bool) {
    sh.handle(&Event::Swipe { back, at: None });
}

#[test]
fn a_swipe_moves_to_the_next_and_previous_group_and_stops_at_the_ends() {
    choose(true, 3);
    let (mut sh, _) = home();
    let p = sh.home_page();
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p + 1, "fingers moving left: the next group");
    swipe(&mut sh, true);
    assert_eq!(sh.home_page(), p, "fingers moving right: the previous group");
    for _ in 0..10 {
        swipe(&mut sh, false);
    }
    assert_eq!(sh.home_page(), sh.home_pages() - 1, "no wrapping past the last group");
}

#[test]
fn the_page_slides_in_for_a_fifth_of_a_second_and_reduced_motion_removes_the_slide() {
    choose(true, 3);
    let (mut sh, mut g) = home();
    let was = crate::fx::reduced_motion();
    crate::fx::set_reduced_motion(false);
    assert_eq!(sh.slide_offset(1280.0), 0.0);
    swipe(&mut sh, false);
    let first = sh.slide_offset(1280.0);
    assert!(first > 50.0, "the page starts off to the right and comes in from there: {first}");
    sh.render(&mut g);
    for _ in 0..3 {
        sh.handle(&Event::Tick);
    }
    let mid = sh.slide_offset(1280.0);
    assert!(mid > 0.0 && mid < first, "easing to rest: {mid}");
    for _ in 0..10 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.slide_offset(1280.0), 0.0, "at rest after about a fifth of a second");
    swipe(&mut sh, true);
    assert!(sh.slide_offset(1280.0) < 0.0, "the other way comes in from the left");
    crate::fx::set_reduced_motion(true);
    assert_eq!(sh.slide_offset(1280.0), 0.0, "reduced motion: no slide");
    sh.render(&mut g);
    crate::fx::set_reduced_motion(was);
}

#[test]
fn nothing_happens_with_the_setting_off() {
    let (mut sh, _) = home();
    let p = sh.home_page();
    choose(false, 3);
    swipe(&mut sh, false);
    choose(true, 3);
    assert_eq!(sh.home_page(), p);
}

#[test]
fn an_open_app_never_sees_or_obeys_the_swipe() {
    choose(true, 3);
    let (mut sh, mut g) = home();
    let p = sh.home_page();
    sh.open_for_test(PApp::Chat);
    sh.render(&mut g);
    swipe(&mut sh, false);
    swipe(&mut sh, true);
    assert!(matches!(sh.screen, Screen::App(..)), "the app stays open");
    key(&mut sh, Key::Esc);
    assert_eq!(sh.home_page(), p, "and Home did not turn behind it");
}

#[test]
fn a_dialog_a_menu_a_card_or_a_focused_field_stops_the_swipe() {
    choose(true, 3);
    let (mut sh, _) = home();
    let p = sh.home_page();
    sh.help = true;
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p, "the help card is up");
    sh.help = false;
    sh.fnk.power_card = true;
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p, "a power card is up");
    sh.fnk.power_card = false;
    sh.vol.open = true;
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p, "the volume slider is open");
    sh.vol.open = false;
    sh.fields.push(iyakki_uikit::ctxmenu::FieldNote { rect: R::new(0.0, 0.0, 10.0, 10.0), id: 1, focused: true, has_selection: false, empty: true, secret: false });
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p, "a text field has focus");
    sh.fields.clear();
    swipe(&mut sh, false);
    assert_eq!(sh.home_page(), p + 1, "and with all of that gone it turns");
}

#[test]
fn a_locked_screen_ignores_the_swipe() {
    choose(true, 3);
    let (mut sh, _) = home();
    let p = sh.home_page();
    sh.lockui.active = true;
    swipe(&mut sh, false);
    sh.lockui.active = false;
    assert_eq!(sh.home_page(), p);
}

#[test]
fn a_finger_swipe_counts_only_from_the_group_area() {
    choose(true, 3);
    let (mut sh, _) = home();
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let isl = launcher::island_rect(w, h, s, sh.group_items().len());
    let p = sh.home_page();
    sh.handle(&Event::Swipe { back: false, at: Some((2, h as i32 - 2)) });
    assert_eq!(sh.home_page(), p, "a finger that began in the corner, outside the island");
    sh.handle(&Event::Swipe { back: false, at: Some(((isl.x + isl.w / 2.0) as i32, (isl.y + isl.h / 2.0) as i32)) });
    assert_eq!(sh.home_page(), p + 1, "one that began on the island turns the page");
}
