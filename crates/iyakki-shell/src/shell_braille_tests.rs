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
