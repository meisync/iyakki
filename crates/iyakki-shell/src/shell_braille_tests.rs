//! The braille display in the shell (F-A11Y-07), with the fake core's display and a recording voice: what the reader says reaches the
//! display, its keys move the focus, turning the choice off blanks it, a missing display is looked for again later and a password is never
//! shown. Nothing here touched a real display.

use super::access_tests::*;
use super::*;
use crate::fnkeys::FnKey;
use iyakki_core_api::BrailleKey;
use iyakki_core_fake::FakeCore;
use iyakki_shared::accessibility as accfile;

fn rig(tag: &str, width: usize) -> (Shell, Arc<FakeCore>, Gfx) {
    let mut sh = signed_in(tag);
    let core = Arc::new(FakeCore::new());
    core.braille_fake().connect(width);
    sh.set_core_for_test(core.clone());
    let mut g = Gfx::new(1280, 720);
    keep(&mut sh, true);
    settle(&mut sh, &mut g);
    (sh, core, g)
}

/// The choices as Settings or the chat tool would keep them: in the person's file, picked up by the shell.
fn keep(sh: &mut Shell, braille: bool) {
    let cfg = accfile::Access { screen_reader: true, braille, ..Default::default() };
    accfile::save(sh.env.data_dir(), "root", &cfg).unwrap();
    sh.access_reload();
}

#[test]
fn what_the_reader_says_reaches_the_display_and_the_keys_move_the_focus() {
    let (mut sh, core, g) = rig("brl-shell", 12);
    let mut g = g;
    let log = voiced(&mut sh);
    chord(&mut sh, FnKey::AccessNext);
    beats(&mut sh, 3);
    let spoken = said(&log);
    assert!(!spoken.is_empty());
    let shown = core.braille_fake().shown();
    assert!(shown.iter().any(|s| s.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))), "cells were shown: {shown:?}");
    // a key of the display asks for the next control: the reader says it
    let before = said(&log).len();
    core.braille_fake().press(BrailleKey::FocusNext);
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    assert!(said(&log).len() > before, "the focus moved and was said");
}

#[test]
fn turning_the_choice_off_blanks_the_display_and_a_missing_display_is_found_later() {
    let (mut sh, core, mut g) = rig("brl-off", 8);
    assert_eq!(core.braille_fake().opened(), 1);
    keep(&mut sh, false);
    beats(&mut sh, 3);
    let shown = core.braille_fake().shown();
    assert_eq!(shown.last().unwrap(), &"\u{2800}".repeat(8), "blank cells");
    // on again with no display connected: nothing opens until it is plugged in and the retry beats have passed
    core.braille_fake().disconnect();
    keep(&mut sh, true);
    beats(&mut sh, 5);
    assert_eq!(core.braille_fake().opened(), 1);
    core.braille_fake().connect(8);
    for _ in 0..70 {
        sh.handle(&Event::Tick);
    }
    settle(&mut sh, &mut g);
    assert_eq!(core.braille_fake().opened(), 2);
}

#[test]
fn a_routing_key_over_a_text_field_puts_the_caret_at_that_character_and_a_stale_line_moves_nothing() {
    let (mut sh, _) = super::tests::shell_with_users("brl-caret");
    let core = Arc::new(FakeCore::new());
    core.braille_fake().connect(60);
    sh.set_core_for_test(core.clone());
    let mut g = Gfx::new(1280, 720);
    let log = voiced(&mut sh);
    // the sign-in screen has a plain text field (the name box); the reader and the display are on for it (after the first beat, which reads the
    // computer's own choices)
    beats(&mut sh, 2);
    sh.access_apply_for_test(accfile::Access { screen_reader: true, braille: true, ..Default::default() });
    settle(&mut sh, &mut g);
    assert_eq!(core.braille_fake().opened(), 1, "the display was opened");
    let field_value = |sh: &Shell| sh.access_controls().into_iter().find(|e| e.node.role == iyakki_uikit::access::Role::Field && !e.node.is_secret()).and_then(|e| e.node.state.value_text().map(str::to_string));
    // walk to the name box, type into it, walk away and back so the display shows the line with the text in it
    let mut found = false;
    for _ in 0..20 {
        chord(&mut sh, FnKey::AccessNext);
        beats(&mut sh, 2);
        if said(&log).last().is_some_and(|s| s.contains(", edit")) {
            found = true;
            break;
        }
    }
    assert!(found, "{:?}", said(&log));
    chord(&mut sh, FnKey::AccessActivate);
    for c in "abcdef".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
        settle(&mut sh, &mut g);
    }
    chord(&mut sh, FnKey::AccessNext);
    beats(&mut sh, 2);
    chord(&mut sh, FnKey::AccessPrev);
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    let line = said(&log).last().cloned().unwrap();
    assert!(line.ends_with("abcdef"), "{line} / {:?}", said(&log));
    // the display shows the line cell for cell in grade one: the key over the third letter of the value
    let want_char = line.chars().count() - 6 + 3;
    let at = iyakki_uikit::braille::translate(&line).1.iter().position(|&f| f == want_char).expect("a cell for the character");
    core.braille_fake().press(BrailleKey::Route(at));
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    sh.handle(&Event::Key(Key::Char('Z')));
    settle(&mut sh, &mut g);
    assert_eq!(field_value(&sh).as_deref(), Some("abcZdef"), "typing goes in at the routed place");
    // the line on the display is now out of date (the text changed): a routing key moves nothing, so the caret stays after the Z
    core.braille_fake().press(BrailleKey::Route(0));
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    sh.handle(&Event::Key(Key::Char('Q')));
    settle(&mut sh, &mut g);
    assert_eq!(field_value(&sh).as_deref(), Some("abcZQdef"));
    // the reader says the field again (walk away and back): a key over the start of the line puts the caret at the start
    chord(&mut sh, FnKey::AccessNext);
    beats(&mut sh, 2);
    chord(&mut sh, FnKey::AccessPrev);
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    core.braille_fake().press(BrailleKey::Route(0));
    beats(&mut sh, 3);
    settle(&mut sh, &mut g);
    sh.handle(&Event::Key(Key::Char('W')));
    settle(&mut sh, &mut g);
    assert_eq!(field_value(&sh).as_deref(), Some("WabcZQdef"));
}
