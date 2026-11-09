//! The lock screen in the shell: locking keeps the person signed in and the apps running, closes the vault, takes every key and click;
//! the right password opens it again, a wrong one is throttled like at sign-in; Super+L, the Power menu, a request file (Settings, the
//! assistant), sleep and the idle minutes lock; media or a waiting approval hold the idle lock off; a guest is signed out instead; the
//! sign-in screen is unchanged. The password is never in the audit log.

use super::tests::*;
use super::*;
use crate::saver::Stage;
use iyakki_agent::lockscreen::{self, LockConfig};
use iyakki_agent::screensaver::{First, SaverConfig, Style};
use std::sync::atomic::{AtomicU64, Ordering};

const MIN: u64 = 60_000;

fn signed_in(tag: &str, user: &str, pw: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, user, pw);
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

fn unlock_with(sh: &mut Shell, pw: &str) {
    typ(sh, pw);
    key(sh, Key::Enter);
}

fn audit_text(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

/// A signed-in shell with a fake clock, no screen saver of its own and `lock_min` as the person's automatic lock.
fn timed(tag: &str, lock_min: u32) -> (Shell, Arc<AtomicU64>) {
    let mut sh = signed_in(tag, "root", "rootpass12");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    sh.saver_config_for_test(SaverConfig { first: First::Nothing, ..SaverConfig::default() });
    if lock_min > 0 {
        LockConfig { auto_min: lock_min }.save(sh.env.services.dir(), "root").unwrap();
    }
    (sh, clock)
}

fn run(sh: &mut Shell, clock: &AtomicU64, ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(250);
        clock.fetch_add(step, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        left -= step;
    }
}

#[test]
fn locking_keeps_the_person_signed_in_closes_the_vault_and_covers_the_desktop() {
    let mut sh = signed_in("lk-basic", "root", "rootpass12");
    assert!(sh.env.services.user_vault_unlocked("root"), "the vault opened at sign-in");
    assert!(!sh.is_locked());
    assert!(sh.lock_screen("test"));
    assert!(sh.is_locked() && sh.screen_name() == "locked");
    assert!(sh.session.is_some(), "still signed in: nothing was stopped");
    assert!(!sh.env.services.user_vault_unlocked("root"), "the vault stays closed until the password is typed");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "Unlock") && labels.iter().any(|l| l == "field"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.starts_with("group ") || l == "IYAKKI menu"), "nothing of the desktop can be clicked: {labels:?}");
    assert!(audit_text(&sh).contains("lock"), "locking is audited");
}

#[test]
fn a_locked_screen_takes_every_key_and_the_right_password_opens_it_again() {
    let mut sh = signed_in("lk-keys", "root", "rootpass12");
    sh.lock_screen("test");
    // keys that would open or close things on the desktop are the prompt's
    for k in [Key::Char('s'), Key::Char('q'), Key::Esc, Key::F(10), Key::F(12), Key::Down, Key::Ctrl('p')] {
        key(&mut sh, k.clone());
        assert!(sh.is_locked() && sh.screen_name() == "locked", "{k:?} did not get past the lock");
    }
    sh.handle(&Event::Scroll(3));
    sh.handle(&Event::ScrollPx(120));
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.is_locked() && !sh.fnk.power_card);
    unlock_with(&mut sh, "wrong password");
    assert!(sh.is_locked(), "a wrong password does not open it");
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    assert_eq!(sh.screen_name(), "launcher", "the desktop is as it was");
    assert!(sh.session.is_some() && sh.env.services.user_vault_unlocked("root"), "the vault opens with the password");
    let log = audit_text(&sh);
    assert!(log.contains("unlock") && log.contains("unlock.failed"));
    assert!(!log.contains("rootpass12") && !log.contains("wrong password"), "no password is ever logged");
}

#[test]
fn a_wrong_password_shows_an_error_wipes_the_field_and_attempts_are_throttled_like_at_sign_in() {
    let mut sh = signed_in("lk-throttle", "root", "rootpass12");
    sh.lock_screen("test");
    let mut g = Gfx::new(1280, 720);
    for _ in 0..5 {
        unlock_with(&mut sh, "nope nope nope");
        assert!(sh.lockui.form.fields[0].value.is_empty(), "the typed password is wiped after every attempt");
    }
    // even the right password is refused during the wait
    unlock_with(&mut sh, "rootpass12");
    assert!(sh.is_locked(), "throttled");
    assert!(sh.lockui.form.error.as_deref().is_some(), "the wait is explained");
    sh.render(&mut g);
    assert!(!audit_text(&sh).contains("nope nope"));
    // an empty field is asked for, not counted
    let mut sh2 = signed_in("lk-empty", "root", "rootpass12");
    sh2.lock_screen("test");
    key(&mut sh2, Key::Enter);
    assert_eq!(sh2.lockui.form.error.as_deref(), Some("Type your password"));
    unlock_with(&mut sh2, "rootpass12");
    assert!(!sh2.is_locked(), "an empty attempt did not count against the person");
}

#[test]
fn the_password_can_be_typed_pasted_and_unlocked_with_the_mouse() {
    let mut sh = signed_in("lk-mouse", "root", "rootpass12");
    sh.lock_screen("test");
    let mut g = Gfx::new(1280, 720);
    click_field(&mut sh, &mut g, 0);
    sh.handle(&Event::Paste("rootpass12".into()));
    click(&mut sh, &mut g, "Unlock");
    assert!(!sh.is_locked(), "paste, then the Unlock button");
    // another person's password does not open this lock
    sh.lock_screen("test");
    unlock_with(&mut sh, "alicepass1");
    assert!(sh.is_locked(), "only the signed-in person's own password opens their screen");
}

#[test]
fn every_way_to_lock_works_and_a_guest_is_signed_out_instead() {
    // the lock key (Super+L / KEY_SCREENLOCK)
    let mut sh = signed_in("lk-key", "root", "rootpass12");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(sh.is_locked());
    // the Power menu's Lock and the power key's card
    let mut sh = signed_in("lk-menu", "oscar", "operpass12");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.is_locked() && sh.session.is_some(), "locked, not signed out");
    unlock_with(&mut sh, "operpass12");
    assert_eq!(sh.screen_name(), "launcher", "the power menu is not left open behind the lock");
    sh.fnk.power_card = true;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.is_locked());
    // a request from Settings or the assistant, taken within a second
    let mut sh = signed_in("lk-req", "root", "rootpass12");
    lockscreen::request(sh.env.services.dir(), "alice");
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(!sh.is_locked(), "somebody else's request is not ours");
    lockscreen::request(sh.env.services.dir(), "root");
    for _ in 0..40 {
        sh.handle(&Event::Tick);
    }
    assert!(sh.is_locked(), "our own request locks");
    // a guest has no password: Lock signs out
    let mut sh = signed_in("lk-guest", "gus", "guestpass1");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(!sh.is_locked() && sh.session.is_some(), "the lock key does not sign a guest out by surprise");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('l')));
    assert!(sh.session.is_none() && sh.screen_name() == "login", "the Power menu's Lock signs a guest out, as before");
}

#[test]
fn nobody_signed_in_sees_the_normal_sign_in_screen_and_the_lock_key_does_nothing() {
    let (mut sh, _) = shell_with_users("lk-login");
    assert_eq!(sh.screen_name(), "login");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Lock, true));
    assert!(!sh.is_locked() && sh.screen_name() == "login");
    assert!(!sh.lock_screen("test"), "nothing to lock");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Sign in") && !sh.hits().iter().any(|h| h.label == "Unlock"));
    // signing out from a locked screen cannot happen: the sign-in screen has no lock state left over
    login(&mut sh, "root", "rootpass12");
    sh.lock_screen("test");
    unlock_with(&mut sh, "rootpass12");
    sh.sign_out();
    assert_eq!(sh.screen_name(), "login");
}

#[test]
fn sleeping_locks_the_screen_and_keeps_the_session() {
    let (mut sh, host) = shell_with_users("lk-sleep");
    *host.sleepable.lock().unwrap() = true;
    login(&mut sh, "root", "rootpass12");
    sh.screen = Screen::PowerMenu;
    sh.handle(&Event::Key(Key::Char('s')));
    assert_eq!(*host.slept.lock().unwrap(), 1);
    assert!(sh.is_locked() && sh.session.is_some(), "the screen asks for the password on wake; the apps were not closed");
    assert!(!sh.env.services.user_vault_unlocked("root"));
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
}

#[test]
fn the_automatic_lock_is_off_until_chosen_and_then_comes_after_the_idle_minutes() {
    let (mut sh, clock) = timed("lk-auto-off", 0);
    run(&mut sh, &clock, 90 * MIN);
    assert!(!sh.is_locked(), "never by default");
    let (mut sh, clock) = timed("lk-auto-on", 10);
    run(&mut sh, &clock, 9 * MIN);
    assert!(!sh.is_locked(), "not yet");
    sh.handle(&Event::MouseMove(10, 10));
    run(&mut sh, &clock, 9 * MIN);
    assert!(!sh.is_locked(), "input restarted the count");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked() && sh.session.is_some(), "locked after ten quiet minutes");
    assert!(!sh.env.services.user_vault_unlocked("root"));
    assert!(audit_text(&sh).contains("idle"));
    // typing on the lock screen is input: it does not lock a second time or wipe the prompt
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    run(&mut sh, &clock, 5 * MIN);
    assert!(!sh.is_locked(), "the count started again at the unlock");
}

#[test]
fn media_a_waiting_approval_or_a_dialog_hold_the_automatic_lock_off_and_the_count_starts_after_them() {
    let (mut sh, clock) = timed("lk-hold", 5);
    sh.fnk.power_card = true;
    run(&mut sh, &clock, 30 * MIN);
    assert!(!sh.is_locked(), "a card waits for the person: no lock over it");
    sh.fnk.power_card = false;
    run(&mut sh, &clock, 4 * MIN);
    assert!(!sh.is_locked(), "the count began when the hold ended, not at the last key");
    run(&mut sh, &clock, 2 * MIN);
    assert!(sh.is_locked());
}

#[test]
fn the_choice_is_each_persons_own() {
    let (mut sh, clock) = timed("lk-person", 5);
    LockConfig { auto_min: 0 }.save(sh.env.services.dir(), "root").unwrap();
    LockConfig { auto_min: 5 }.save(sh.env.services.dir(), "alice").unwrap();
    run(&mut sh, &clock, 20 * MIN);
    assert!(!sh.is_locked(), "alice's choice does not lock root");
}

#[test]
fn waking_from_the_screen_saver_shows_the_lock_and_the_waking_key_is_not_a_password_character() {
    let (mut sh, clock) = timed("lk-saver", 5);
    sh.saver_config_for_test(SaverConfig { first: First::Saver, after_min: 5, then_off_min: 5, battery_shorter: false, style: Style::Logo, dim_pct: 0 });
    run(&mut sh, &clock, 5 * MIN + 2_000);
    assert!(sh.is_locked() && sh.screen_stage() == Stage::Saver, "locked behind the saver");
    sh.handle(&Event::Key(Key::Char('r'))); // wakes only
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert!(sh.is_locked());
    assert!(sh.lockui.form.fields[0].value.is_empty(), "the waking key was not typed into the prompt");
    unlock_with(&mut sh, "rootpass12");
    assert!(!sh.is_locked());
    // the screen off: pointer movement wakes it and the prompt is there
    run(&mut sh, &clock, 20 * MIN);
    assert!(sh.is_locked());
    sh.handle(&Event::MouseMove(30, 30));
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_eq!(sh.screen_name(), "locked");
}

#[test]
fn heartbeats_go_on_behind_the_lock_so_work_in_the_background_is_not_stopped() {
    let mut sh = signed_in("lk-beat", "root", "rootpass12");
    sh.lock_screen("test");
    let t0 = sh.tick;
    for _ in 0..30 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.tick, t0 + 30, "the shell's clock, and with it every running app's, keeps going");
    assert!(sh.session.is_some());
}

#[test]
fn the_lock_draws_at_many_sizes_with_the_field_and_button_inside_the_screen_and_a_touch_keyboard_lift() {
    let mut sh = signed_in("lk-draw", "root", "rootpass12");
    sh.lock_screen("test");
    for (w, h) in [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2880, 1800)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        for want in ["field", "Unlock"] {
            let hit = sh.hits().iter().find(|x| x.label == want).unwrap_or_else(|| panic!("{want} at {w}x{h}"));
            assert!(hit.rect.x >= -0.5 && hit.rect.y >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{want} inside {w}x{h}: {:?}", hit.rect);
        }
    }
    // an error line and the clock draw without a panic
    unlock_with(&mut sh, "wrong");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
}
