//! The on-screen keyboard in the shell: it opens for a finger on a text field and not for a mouse, types into the sign-in form, hides
//! for a physical key, and its keys stay on the screen at the usual sizes.

use super::tests::*;
use super::*;

const SIZES: [(u32, u32); 5] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480), (360, 740)];

fn value(sh: &Shell, i: usize) -> String {
    match &sh.screen {
        Screen::Login(f) | Screen::Setup(f) => f.fields[i].value.clone(),
        _ => panic!("not a form"),
    }
}

fn lift(sh: &mut Shell) {
    let (x, y) = sh.mouse;
    sh.handle(&Event::MouseUp(x, y));
}

fn type_on_screen(sh: &mut Shell, g: &mut Gfx, text: &str) {
    for c in text.chars() {
        let label = if c == ' ' { "key space".to_string() } else { format!("key {c}") };
        click(sh, g, &label);
        lift(sh);
    }
}

#[test]
fn a_mouse_click_on_a_field_opens_no_keyboard() {
    let (mut sh, _) = shell_with_users("osk-mouse");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    assert!(!has(&mut sh, &mut g, "key q"), "a click does not open it by itself");
    assert!(has(&mut sh, &mut g, "keyboard"), "but the button is there for a mouse user");
    click(&mut sh, &mut g, "keyboard");
    assert!(has(&mut sh, &mut g, "key q"), "and it opens the keyboard, which a mouse can press too");
    click(&mut sh, &mut g, "key x");
    assert_eq!(value(&sh, 0), "x");
}

#[test]
fn a_finger_on_the_field_opens_it_and_the_sign_in_form_can_be_filled_with_it() {
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("osk-finger");
    let mut g = Gfx::new(1280, 720);
    sh.osk.touched(sh.tick);
    click_field(&mut sh, &mut g, 0);
    assert!(has(&mut sh, &mut g, "key q"), "the keyboard is up");
    type_on_screen(&mut sh, &mut g, "alice");
    assert_eq!(value(&sh, 0), "alice");
    // the next field: tap it, the keyboard stays
    click_field(&mut sh, &mut g, 1);
    assert!(has(&mut sh, &mut g, "key q"));
    click(&mut sh, &mut g, "key shift");
    lift(&mut sh);
    click(&mut sh, &mut g, "key A");
    lift(&mut sh);
    assert_eq!(value(&sh, 1), "A", "one capital");
    click(&mut sh, &mut g, "key backspace");
    lift(&mut sh);
    assert_eq!(value(&sh, 1), "", "a capital A, then backspace");
    click(&mut sh, &mut g, "key ?123");
    lift(&mut sh);
    type_on_screen(&mut sh, &mut g, "1");
    assert_eq!(value(&sh, 1), "1");
    click(&mut sh, &mut g, "key backspace");
    lift(&mut sh);
    click(&mut sh, &mut g, "key ABC");
    lift(&mut sh);
    type_on_screen(&mut sh, &mut g, "alicepass");
    click(&mut sh, &mut g, "key ?123");
    lift(&mut sh);
    type_on_screen(&mut sh, &mut g, "1");
    assert_eq!(value(&sh, 1), "alicepass1");
    click(&mut sh, &mut g, "key enter");
    lift(&mut sh);
    assert_ne!(sh.screen_name(), "login", "Enter on the screen signs in");
}

#[test]
fn a_physical_key_puts_the_keyboard_away_and_the_button_brings_it_back() {
    let (mut sh, _) = shell_with_users("osk-physical");
    let mut g = Gfx::new(1280, 720);
    sh.osk.touched(sh.tick);
    click_field(&mut sh, &mut g, 0);
    assert!(has(&mut sh, &mut g, "key q"));
    for _ in 0..60 {
        sh.handle(&Event::Tick);
    }
    sh.handle(&Event::Key(Key::Char('a')));
    assert_eq!(value(&sh, 0), "a", "the physical key typed");
    assert!(!has(&mut sh, &mut g, "key q"), "the person is on the keyboard now");
    // a finger again
    sh.osk.touched(sh.tick);
    sh.handle(&Event::Tick);
    assert!(has(&mut sh, &mut g, "key q"));
    // hide it with its own key, then the button opens it
    click(&mut sh, &mut g, "key hide");
    lift(&mut sh);
    assert!(!has(&mut sh, &mut g, "key q"));
    assert!(has(&mut sh, &mut g, "keyboard"));
    click(&mut sh, &mut g, "keyboard");
    assert!(has(&mut sh, &mut g, "key q"));
}

#[test]
fn a_press_on_the_keyboard_never_reaches_what_is_behind_it() {
    let (mut sh, _) = shell_with_users("osk-behind");
    let mut g = Gfx::new(1280, 720);
    sh.osk.touched(sh.tick);
    click_field(&mut sh, &mut g, 0);
    sh.render(&mut g);
    let area = crate::osk::Osk::area(1280.0, 720.0, scale_for(1280, 720));
    // the gap at the very top edge of the keyboard is still the keyboard's: the field keeps the focus
    sh.handle(&Event::MouseDown((area.x + 2.0) as i32, (area.y + 1.0) as i32));
    lift(&mut sh);
    sh.render(&mut g);
    assert!(has(&mut sh, &mut g, "key q"));
    assert_eq!(value(&sh, 0), "");
}

#[test]
fn the_keys_stay_on_the_screen_at_the_usual_sizes() {
    for (w, h) in SIZES {
        let (mut sh, _) = shell_with_users("osk-sizes");
        let mut g = Gfx::new(w, h);
        sh.osk.touched(sh.tick);
        click_field(&mut sh, &mut g, 0);
        sh.render(&mut g);
        let keys: Vec<R> = sh.hits().iter().filter(|x| x.label.starts_with("key ")).map(|x| x.rect).collect();
        assert!(keys.len() > 30, "{w}x{h}: {} keys", keys.len());
        for r in keys {
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: a key is off the screen");
        }
    }
}


#[test]
#[ignore = "writes pictures for a person to look at: IYAKKI_OSK_PICS=dir cargo test -p iyakki-shell osk_pictures -- --ignored"]
fn osk_pictures() {
    let Ok(dir) = std::env::var("IYAKKI_OSK_PICS") else { return };
    for (name, w, h) in [("login-1280", 1280u32, 720u32), ("login-800", 800, 480), ("login-1920", 1920, 1080)] {
        let (mut sh, _) = shell_with_users("osk-pic");
        let mut g = Gfx::new(w, h);
        sh.osk.touched(sh.tick);
        click_field(&mut sh, &mut g, 1);
        sh.render(&mut g);
        g.pm.save_png(format!("{dir}/{name}.png")).expect("png");
    }
}

#[test]
fn a_key_of_the_keyboard_asks_for_a_new_picture_at_once_and_the_field_shows_it() {
    let (mut sh, _) = shell_with_users("osk-stale");
    let mut g = Gfx::new(1280, 720);
    sh.osk.touched(sh.tick);
    click_field(&mut sh, &mut g, 0);
    sh.render(&mut g);
    sh.take_stale();
    let r = sh.hits().iter().rev().find(|h| h.label == "key a").unwrap().rect;
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    assert_eq!(value(&sh, 0), "a");
    assert!(sh.take_stale(), "the press changed the text: the picture is stale");
    sh.handle(&Event::MouseUp(x, y));
    // the very next picture has the letter: draw it and compare with a picture of the same screen without it
    let mut with = Gfx::new(1280, 720);
    sh.render(&mut with);
    if let Screen::Login(f) = &mut sh.screen {
        f.fields[0].value.clear();
    }
    let mut without = Gfx::new(1280, 720);
    sh.render(&mut without);
    assert_ne!(with.rgba(), without.rgba(), "the field shows the typed letter in the first picture after the press");
}
