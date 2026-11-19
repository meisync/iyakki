//! Sign out by keyboard asks first (the chord and the Home keys), the top bar's button does not, the card is mouse and touch
//! clickable, and the sign-in screen says "You signed out" afterwards.

use super::tests::*;
use super::*;
use crate::fnkeys::FnKey;

fn key(sh: &mut Shell, k: Key) {
    sh.handle(&Event::Key(k));
}

fn signed_in(tag: &str, user: &str, pw: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, user, pw);
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

fn chord(sh: &mut Shell) {
    sh.handle(&Event::Fn(FnKey::SignOut, true));
    sh.handle(&Event::Fn(FnKey::SignOut, false));
}

fn labels(sh: &mut Shell, g: &mut Gfx) -> Vec<String> {
    sh.render(g);
    sh.hits().iter().map(|h| h.label.clone()).collect()
}

#[test]
fn the_chord_and_the_home_keys_open_the_card_and_enter_or_y_signs_out() {
    let mut g = Gfx::new(1280, 720);
    // the chord
    let mut sh = signed_in("so-chord", "root", "rootpass12");
    chord(&mut sh);
    assert!(sh.signout_card_open() && sh.session.is_some(), "the chord only asks");
    let l = labels(&mut sh, &mut g);
    assert!(l.iter().any(|x| x == "Sign out") && l.iter().any(|x| x == "Stay"), "{l:?}");
    key(&mut sh, Key::Enter);
    assert!(sh.session.is_none() && sh.screen_name() == "login" && !sh.signout_card_open());
    // each one key of Home, then y
    for k in ['o', 'q'] {
        let mut sh = signed_in("so-home", "root", "rootpass12");
        key(&mut sh, Key::Char(k));
        assert!(sh.signout_card_open() && sh.session.is_some(), "{k} asks first");
        key(&mut sh, Key::Char('y'));
        assert!(sh.session.is_none(), "{k} then y signs out");
    }
    let mut sh = signed_in("so-esc", "root", "rootpass12");
    key(&mut sh, Key::Esc);
    assert!(sh.signout_card_open() && sh.session.is_some(), "Esc on Home asks first");
    key(&mut sh, Key::Enter);
    assert!(sh.session.is_none());
}

#[test]
fn esc_or_n_stays_and_a_second_esc_never_signs_out() {
    for stay in [Key::Esc, Key::Char('n')] {
        let mut sh = signed_in("so-stay", "root", "rootpass12");
        key(&mut sh, Key::Esc);
        assert!(sh.signout_card_open());
        key(&mut sh, stay.clone());
        assert!(!sh.signout_card_open() && sh.session.is_some() && sh.screen_name() == "launcher", "{stay:?} stays");
    }
    // Esc opens, Esc closes, a third Esc opens again; the second never signed out
    let mut sh = signed_in("so-esc2", "root", "rootpass12");
    key(&mut sh, Key::Esc);
    key(&mut sh, Key::Esc);
    assert!(sh.session.is_some() && !sh.signout_card_open());
    // other keys do not reach the launcher under the card
    chord(&mut sh);
    key(&mut sh, Key::Char('s'));
    assert!(sh.signout_card_open() && sh.screen_name() == "launcher", "the card keeps every key");
}

#[test]
fn the_card_answers_to_the_mouse_and_to_a_touch() {
    let mut g = Gfx::new(1280, 720);
    for (tag, label, signs_out) in [("so-m1", "Sign out", true), ("so-m2", "Stay", false)] {
        let mut sh = signed_in(tag, "root", "rootpass12");
        chord(&mut sh);
        // a press outside the buttons does nothing
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(2, 2));
        assert!(sh.signout_card_open(), "a press outside leaves the card");
        click(&mut sh, &mut g, label);
        assert_eq!(sh.session.is_none(), signs_out, "{label}");
        assert!(!sh.signout_card_open());
    }
    // a touch tap arrives as the same press and release; the hit rectangle is big enough for a finger
    let mut sh = signed_in("so-touch", "root", "rootpass12");
    chord(&mut sh);
    sh.render(&mut g);
    let r = sh.hits().iter().rev().find(|h| h.label == "Sign out").unwrap().rect;
    assert!(r.h >= 36.0 && r.w >= 100.0, "{r:?}");
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    assert!(sh.session.is_none(), "a tap on Sign out");
}

#[test]
fn the_top_bar_button_signs_out_at_once() {
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed_in("so-bar", "root", "rootpass12");
    sh.render(&mut g);
    let (w, h) = sh.size;
    let r = header_button(w, scale_for(w, h), 0);
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    assert!(sh.session.is_none() && !sh.signout_card_open());
}

#[test]
fn no_card_on_the_sign_in_screen_and_the_lock_screen_keeps_its_keys() {
    let (mut sh, _) = shell_with_users("so-login");
    assert_eq!(sh.screen_name(), "login");
    chord(&mut sh);
    key(&mut sh, Key::Char('o'));
    assert!(!sh.signout_card_open() && sh.screen_name() == "login");
    // locked: the chord does nothing and y is the password prompt's
    login(&mut sh, "root", "rootpass12");
    assert!(sh.lock_screen("test"));
    chord(&mut sh);
    key(&mut sh, Key::Char('y'));
    assert!(!sh.signout_card_open() && sh.is_locked() && sh.session.is_some(), "no sign-out while the lock screen is up");
    // a card that is open when the screen locks goes away
    let mut sh = signed_in("so-lockcard", "root", "rootpass12");
    chord(&mut sh);
    sh.handle(&Event::Fn(FnKey::Lock, true));
    assert!(sh.is_locked() && sh.session.is_some());
}

#[test]
fn the_one_key_o_does_not_fire_while_typing_in_an_app() {
    let mut sh = signed_in("so-type", "root", "rootpass12");
    sh.open(PApp::Chat);
    assert!(matches!(sh.screen, Screen::App(..)));
    typ(&mut sh, "foo quo");
    assert!(!sh.signout_card_open() && sh.session.is_some() && matches!(sh.screen, Screen::App(..)), "letters are typed, not commands");
}

#[test]
fn the_chord_works_from_inside_an_app_and_asks_there_too() {
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed_in("so-app", "root", "rootpass12");
    sh.open(PApp::Chat);
    chord(&mut sh);
    assert!(sh.signout_card_open() && sh.session.is_some());
    assert!(labels(&mut sh, &mut g).iter().any(|x| x == "Stay"));
    key(&mut sh, Key::Esc);
    assert!(sh.session.is_some() && matches!(sh.screen, Screen::App(..)), "Stay leaves the app as it was");
}

#[test]
fn the_sign_in_screen_says_you_signed_out_for_a_few_seconds_and_not_after_typing() {
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed_in("so-note", "root", "rootpass12");
    assert!(!sh.signed_out_note_shown());
    chord(&mut sh);
    key(&mut sh, Key::Enter);
    assert!(sh.signed_out_note_shown(), "after a sign-out");
    sh.render(&mut g);
    // away after a few seconds
    for _ in 0..200 {
        sh.handle(&Event::Tick);
    }
    assert!(!sh.signed_out_note_shown());
    // away on typing
    let mut sh = signed_in("so-note2", "root", "rootpass12");
    sh.sign_out();
    assert!(sh.signed_out_note_shown());
    typ(&mut sh, "r");
    assert!(!sh.signed_out_note_shown());
    // a lock is not a sign-out: no note on the lock screen, and the first start shows none
    let mut sh = signed_in("so-note3", "root", "rootpass12");
    sh.lock_screen("test");
    assert!(!sh.signed_out_note_shown());
    let (sh, _) = shell_with_users("so-note4");
    assert!(!sh.signed_out_note_shown());
}

/// `iyakki dev screenshot` and the user's own screenshot are the FINISHED frame: the views strip and the sign-out card are drawn late in
/// `render` and were missing from them (found on the Mac mini, 2026-11-11, while `dev state` listed both).
#[test]
fn the_developer_screenshot_and_the_users_screenshot_hold_the_late_layers() {
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed_in("so-shot", "root", "rootpass12");
    sh.render(&mut g);
    let one_view = g.rgba().to_vec();
    sh.views.model.add("Second", None).unwrap();
    sh.views_save();
    sh.render(&mut g);
    let strip = sh.hits().iter().find(|h| h.label.starts_with("view ")).map(|h| h.rect).expect("the strip is drawn with two views");
    let bare = g.rgba().to_vec();
    key(&mut sh, Key::Char('o'));
    assert!(sh.signout_card_open());
    // the device's own path: a paired computer waits in `request`, the next frame answers it
    let svc = sh.services_for_demo().clone();
    let t = std::thread::spawn(move || svc.screen().request(iyakki_shared::devmode::SCREEN_ANY, std::time::Duration::from_secs(20)));
    let mut got = None;
    for _ in 0..400 {
        sh.render(&mut g);
        if t.is_finished() {
            got = Some(t.join().unwrap().expect("the frame is served"));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let f = got.expect("answered");
    assert_eq!((f.w, f.h), (1280, 720));
    assert_eq!(f.rgba, g.rgba(), "the served picture is the finished frame, every overlay in it");
    // the card is in it (the picture differs from the bare desktop at the card's buttons) and so is the strip
    let px = |buf: &[u8], x: f32, y: f32| { let i = ((y as usize) * 1280 + x as usize) * 4; buf[i..i + 4].to_vec() };
    let card = sh.hits().iter().find(|h| h.label == "Sign out").map(|h| h.rect).expect("card button");
    assert_ne!(px(&f.rgba, card.x + 4.0, card.y + 4.0), px(&bare, card.x + 4.0, card.y + 4.0), "the card's pixels are in the picture");
    let in_strip = |buf: &[u8]| (0..strip.h as usize).flat_map(|dy| (0..strip.w as usize).map(move |dx| (dx, dy))).map(|(dx, dy)| px(buf, strip.x + dx as f32, strip.y + dy as f32)).collect::<Vec<_>>();
    assert_ne!(in_strip(&f.rgba), in_strip(&one_view), "the views strip is in the picture");
    // F12: the frame kept for framing is the finished one too
    key(&mut sh, Key::Esc);
    sh.shot = Shot::Snap;
    sh.render(&mut g);
    let Shot::Select { snap, .. } = &sh.shot else { panic!("the next frame is captured") };
    assert_eq!(snap.data(), g.rgba(), "the F12 snapshot is the finished frame");
}
