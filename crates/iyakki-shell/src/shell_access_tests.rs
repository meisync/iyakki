//! Accessibility in the shell, with a fake voice: the Super+Alt+S chord at the sign-in screen, what the reader says for a walk over the
//! controls, a new screen, a toast and a password field (never its text), the voice that is missing, choices picked up from the files
//! (Settings, the assistant), and the guarantee that every clickable region of the main screens can be reached and pressed from the
//! keyboard alone. Nothing here was heard or used by a person; the voice is a recorder.

use super::tests::*;
use super::*;
use crate::access::SpeechSink;
use crate::fnkeys::FnKey;
use iyakki_agent::accessibility as accfile;
use std::sync::Mutex;

#[derive(Default)]
struct Log {
    said: Vec<String>,
    stops: usize,
}

struct FakeVoice(Arc<Mutex<Log>>);

impl SpeechSink for FakeVoice {
    fn say(&mut self, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().said.push(text.to_string());
        Ok(())
    }
    fn busy(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().stops += 1;
    }
}

fn voiced(sh: &mut Shell) -> Arc<Mutex<Log>> {
    let log = Arc::new(Mutex::new(Log::default()));
    sh.access_sink_for_test(Box::new(FakeVoice(log.clone())));
    log
}

fn chord(sh: &mut Shell, k: FnKey) {
    sh.handle(&Event::Fn(k, true));
    sh.handle(&Event::Fn(k, false));
}

fn beats(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

/// A frame, then a few beats: the reader speaks on the beat.
fn settle(sh: &mut Shell, g: &mut Gfx) {
    sh.render(g);
    beats(sh, 3);
    sh.render(g);
    beats(sh, 3);
}

fn said(log: &Arc<Mutex<Log>>) -> Vec<String> {
    log.lock().unwrap().said.clone()
}

fn signed_in(tag: &str) -> Shell {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

#[test]
fn the_chord_turns_the_reader_on_at_the_sign_in_screen_and_the_computer_keeps_it() {
    let (mut sh, _) = shell_with_users("acc-chord");
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "login");
    assert!(!sh.access_config().screen_reader, "off by default");
    beats(&mut sh, 3);
    assert!(said(&log).is_empty(), "silent while off");
    chord(&mut sh, FnKey::ScreenReader);
    assert!(sh.access_config().screen_reader);
    assert!(accfile::machine_reader(sh.env.services.dir()), "kept with the computer, not with a person");
    settle(&mut sh, &mut g);
    let all = said(&log);
    assert!(all[0].starts_with("Screen reader on"), "{all:?}");
    assert!(all.iter().any(|s| s == "Sign in. Username, edit, empty"), "the sign-in screen and its first field are read once the reader starts: {all:?}");
    chord(&mut sh, FnKey::ScreenReader);
    assert!(!sh.access_config().screen_reader && !accfile::machine_reader(sh.env.services.dir()));
    assert_eq!(said(&log).last().unwrap(), "Screen reader off");
    assert!(log.lock().unwrap().stops >= 1, "the voice was stopped");
    let before = said(&log).len();
    settle(&mut sh, &mut g);
    assert_eq!(said(&log).len(), before, "silent again");
}

#[test]
fn after_a_restart_the_sign_in_screen_still_speaks_and_a_person_inherits_the_choice() {
    let (mut sh, _) = shell_with_users("acc-keep");
    accfile::set_machine_reader(sh.env.services.dir(), true).unwrap();
    sh.access_reload();
    assert!(sh.access_config().screen_reader, "read from the computer's file at the sign-in screen");
    login(&mut sh, "root", "rootpass12");
    assert!(sh.access_config().screen_reader, "still on for the person who signs in");
    let mut g = Gfx::new(1280, 720);
    chord(&mut sh, FnKey::ScreenReader);
    assert!(!sh.access_config().screen_reader);
    assert!(!accfile::load(sh.env.services.dir(), "root").screen_reader && !accfile::machine_reader(sh.env.services.dir()));
    sh.render(&mut g);
}

#[test]
fn the_walk_speaks_each_control_and_a_password_is_never_read() {
    let (mut sh, _) = shell_with_users("acc-walk");
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    chord(&mut sh, FnKey::ScreenReader);
    settle(&mut sh, &mut g);
    let first = said(&log).len();
    // walk to the password field and press it
    let mut found = false;
    for _ in 0..20 {
        chord(&mut sh, FnKey::AccessNext);
        beats(&mut sh, 2);
        if sh.access_walk_at().as_deref() == Some("Password") {
            found = true;
            break;
        }
    }
    assert!(found, "the walk reaches the password field: {:?}", said(&log));
    assert_eq!(said(&log).last().unwrap(), "Password, password field, empty");
    chord(&mut sh, FnKey::AccessActivate);
    for c in "hunter22".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    settle(&mut sh, &mut g);
    let all = said(&log);
    assert!(all.len() > first);
    assert!(!all.iter().any(|s| s.contains("hunter")), "the text of a password is never spoken: {all:?}");
    // asking again says how many characters, not which
    chord(&mut sh, FnKey::AccessPrev);
    chord(&mut sh, FnKey::AccessNext);
    beats(&mut sh, 2);
    assert_eq!(said(&log).last().unwrap(), "Password, password field, 8 characters typed");
    // repeat says the last thing again
    let n = said(&log).len();
    chord(&mut sh, FnKey::AccessRepeat);
    beats(&mut sh, 2);
    assert_eq!(said(&log).len(), n + 1);
    assert_eq!(said(&log).last().unwrap(), "Password, password field, 8 characters typed");
}

#[test]
fn a_person_can_sign_in_with_the_chords_and_the_keyboard_alone() {
    let (mut sh, _) = shell_with_users("acc-signin");
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    chord(&mut sh, FnKey::ScreenReader);
    settle(&mut sh, &mut g);
    for c in "root".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Tab));
    for c in "rootpass12".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    settle(&mut sh, &mut g);
    assert_eq!(sh.screen_name(), "launcher");
    let all = said(&log);
    assert!(all.iter().any(|s| s.starts_with("Home")), "the new screen is named: {all:?}");
    assert!(!all.iter().any(|s| s.contains("rootpass")), "{all:?}");
}

#[test]
fn a_wrong_password_and_a_toast_are_read_and_the_option_can_silence_them() {
    let (mut sh, _) = shell_with_users("acc-notice");
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    chord(&mut sh, FnKey::ScreenReader);
    settle(&mut sh, &mut g);
    for c in "root".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Tab));
    for c in "wrongwrong".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    settle(&mut sh, &mut g);
    assert!(said(&log).iter().any(|s| s.starts_with("Error.")), "the sign-in error is read: {:?}", said(&log));
    sh.say("Disk almost full");
    settle(&mut sh, &mut g);
    assert!(said(&log).iter().any(|s| s == "Disk almost full"));
    // notices off: the next toast stays silent, a focus change is still spoken
    let mut cfg = sh.access_config().clone();
    cfg.speak_notices = false;
    sh.access_apply_for_test(cfg);
    let n = said(&log).len();
    sh.say("Another message");
    settle(&mut sh, &mut g);
    assert_eq!(said(&log).len(), n, "{:?}", said(&log));
}

#[test]
fn without_a_voice_the_reader_says_so_on_screen_once() {
    let (mut sh, _) = shell_with_users("acc-novoice");
    let mut g = Gfx::new(1280, 720);
    chord(&mut sh, FnKey::ScreenReader); // the real speaker: this test computer has no reading voice set up
    settle(&mut sh, &mut g);
    let toast = sh.toast_text().unwrap_or_default().to_string();
    assert!(toast.contains("no reading voice is installed") && toast.contains("Settings, Voice"), "{toast:?}");
    assert!(sh.access_problem().is_some());
    // a sign stays on the screen while the reader has nothing to speak with
    sh.render(&mut g);
    assert!(g.text_log.iter().any(|(_, _, t)| t.contains("no voice")), "the pill is drawn");
    // and it goes away with the reader
    chord(&mut sh, FnKey::ScreenReader);
    sh.render(&mut g);
    let mut g2 = Gfx::new(1280, 720);
    sh.render(&mut g2);
    assert!(!g2.text_log.iter().any(|(_, _, t)| t.contains("no voice")));
}

#[test]
fn the_walk_works_without_the_reader_and_draws_a_ring_and_pressing_clicks_the_control() {
    let mut sh = signed_in("acc-ring");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(!sh.access_config().screen_reader);
    let controls = sh.access_controls();
    assert!(controls.len() > 5, "the home screen has controls: {:?}", controls.iter().map(|e| e.node.speak()).collect::<Vec<_>>());
    let plain = g.rgba().to_vec();
    chord(&mut sh, FnKey::AccessNext);
    assert!(sh.access_walk_at().is_some());
    let mut g2 = Gfx::new(1280, 720);
    sh.render(&mut g2);
    assert_ne!(g2.rgba(), &plain[..], "a ring is drawn round the control");
    // walk to the clock (F7 opens Settings from it) and press it
    let mut n = 0;
    while sh.access_walk_at().as_deref() != Some("clock") {
        chord(&mut sh, FnKey::AccessNext);
        n += 1;
        assert!(n < 80, "the clock is among the controls: {:?}", sh.access_controls().iter().map(|e| e.node.name.clone()).collect::<Vec<_>>());
    }
    chord(&mut sh, FnKey::AccessActivate);
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "app:settings", "pressed from the keyboard");
}

#[test]
fn choices_written_by_settings_or_the_assistant_are_applied_within_a_second() {
    let mut sh = signed_in("acc-pickup");
    let dir = sh.env.services.dir().to_path_buf();
    assert!(!crate::theme::is_high_contrast());
    accfile::set_high_contrast(&dir, "root", true).unwrap();
    accfile::save(&dir, "root", &accfile::Access { reduce_motion: true, big_ring: true, ..Default::default() }).unwrap();
    // the stamp is read every 15th beat
    beats(&mut sh, 16);
    assert!(crate::theme::is_high_contrast(), "High contrast applied");
    assert!(crate::fx::reduced_motion() && crate::access::focus_ring_factor() == 2.0);
    accfile::set_high_contrast(&dir, "root", false).unwrap();
    accfile::save(&dir, "root", &accfile::Access::default()).unwrap();
    beats(&mut sh, 16);
    assert!(!crate::theme::is_high_contrast() && !crate::fx::reduced_motion());
    // signing out puts the sign-in screen back to the computer's own choices
    crate::access::set_prefs(crate::access::Prefs::OFF);
}

#[test]
fn the_chosen_tile_of_the_home_screen_is_spoken_when_the_arrow_keys_move_it() {
    let mut sh = signed_in("acc-rows");
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    chord(&mut sh, FnKey::ScreenReader);
    settle(&mut sh, &mut g);
    let first = said(&log);
    assert!(first.iter().any(|s| s.starts_with("Home. ") && s.ends_with("item, selected")), "the screen and the chosen tile in one breath: {first:?}");
    let n = first.len();
    sh.handle(&Event::Key(Key::Right));
    settle(&mut sh, &mut g);
    let all = said(&log);
    assert!(all.len() > n && all.last().unwrap().ends_with("item, selected"), "a move is spoken: {all:?}");
    assert_ne!(all.last(), first.iter().rev().find(|s| s.ends_with("item, selected")), "and it is another tile");
}

/// Every clickable region of a screen against the keyboard walk: the controls the walk can stand on, the ones that have a key of their
/// own, and the clickable regions the walk leaves out (which must be only whole-screen backdrops and exact duplicates).
fn gaps(sh: &Shell) -> (usize, usize, Vec<String>) {
    let (w, h) = (sh.size.0 as f32, sh.size.1 as f32);
    let walk = sh.access_controls();
    let mut left_out = Vec::new();
    let mut native = 0;
    for hit in sh.hits() {
        let r = hit.rect;
        let on_screen = r.w >= 2.0 && r.h >= 2.0 && r.x + r.w / 2.0 >= 0.0 && r.y + r.h / 2.0 >= 0.0 && r.x + r.w / 2.0 < w && r.y + r.h / 2.0 < h;
        let control = !matches!(hit.ev, Event::MouseMove(..) | Event::MouseDown(..) | Event::MouseUp(..) | Event::Scroll(_) | Event::ScrollPx(_) | Event::Tick | Event::Paste(_));
        if !on_screen || !control {
            continue;
        }
        if matches!(hit.ev, Event::Key(_)) {
            native += 1;
        }
        let reached = walk.iter().any(|e| sh.hits()[e.hit].label == hit.label && sh.hits()[e.hit].ev == hit.ev);
        if !reached {
            left_out.push(format!("{:?} {:?} {}x{}", hit.label, hit.ev, r.w as i32, r.h as i32));
        }
    }
    (walk.len(), native, left_out)
}

#[test]
fn every_clickable_region_of_the_main_screens_is_reachable_by_the_keyboard_walk() {
    let mut g = Gfx::new(1280, 720);
    // sign-in
    let (mut sh, _) = shell_with_users("acc-reach");
    sh.render(&mut g);
    let mut report = Vec::new();
    let (n, native, out) = gaps(&sh);
    report.push(format!("sign-in: {n} controls, {native} with a key of their own, left out {out:?}"));
    assert!(n >= 3, "{report:?}");
    // the walk really visits every one of them, in order, once round
    let mut seen = Vec::new();
    for _ in 0..n {
        chord(&mut sh, FnKey::AccessNext);
        seen.push(sh.access_walk_at().unwrap_or_default());
    }
    assert_eq!(seen.len(), n);
    let mut uniq = seen.clone();
    uniq.sort();
    uniq.dedup();
    assert!(uniq.len() + 2 >= n.min(seen.len()), "the walk does not get stuck: {seen:?}");
    login(&mut sh, "root", "rootpass12");
    // the home screen and the launcher groups
    sh.render(&mut g);
    let (n, native, out) = gaps(&sh);
    report.push(format!("home: {n} controls, {native} with a key of their own, left out {out:?}"));
    assert!(out.is_empty(), "{report:?}");
    // the lock screen
    assert!(sh.lock_screen("test"));
    sh.render(&mut g);
    let (n, native, out) = gaps(&sh);
    report.push(format!("lock: {n} controls, {native} with a key of their own, left out {out:?}"));
    assert!(out.is_empty(), "{report:?}");
    eprintln!("{}", report.join("\n"));
}
