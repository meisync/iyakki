//! The accessibility model of the toolkit: names, roles and states made from hits and widgets, the secret rule, the reader's queue with a
//! fake voice, the keyboard walk order and cursor, sticky and slow keys as pure machines, and the contrast of the themes computed (not
//! looked at). Nothing here has been heard by a person or seen on a real screen.

use super::*;
use crate::gfx::Gfx;
use crate::theme::{self, ThemeId};
use crate::ui::{TextField, ACCENT_MAIN};

fn hit(x: f32, y: f32, w: f32, h: f32, ev: Event, label: &str) -> Hit {
    Hit { rect: R::new(x, y, w, h), ev, label: label.into() }
}

#[derive(Default)]
struct Fake {
    said: Vec<String>,
    stops: usize,
    busy: bool,
    fail: Option<String>,
}

impl SpeechSink for Fake {
    fn say(&mut self, text: &str) -> Result<(), String> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        self.said.push(text.to_string());
        Ok(())
    }
    fn busy(&mut self) -> bool {
        self.busy
    }
    fn stop(&mut self) {
        self.stops += 1;
    }
}

#[test]
fn a_hit_becomes_a_named_node_without_the_tile_doing_anything() {
    start_frame();
    let save = node_of(&hit(0.0, 0.0, 80.0, 30.0, Event::Key(Key::Char('s')), "Save")).unwrap();
    assert_eq!((save.name.as_str(), save.role), ("Save", Role::Button));
    assert_eq!(save.speak(), "Save, button");
    let row = node_of(&hit(0.0, 0.0, 200.0, 30.0, Event::Row(2), "Inbox")).unwrap();
    assert_eq!(row.speak(), "Inbox, item");
    let field = node_of(&hit(0.0, 0.0, 200.0, 30.0, Event::Focus(0), "field")).unwrap();
    assert_eq!(field.role, Role::Field);
    // symbols a voice would skip are put into words; the toolkit's own prefix is dropped
    assert_eq!(node_of(&hit(0.0, 0.0, 30.0, 30.0, Event::Tab(911), "+")).unwrap().name, "plus");
    assert_eq!(node_of(&hit(0.0, 0.0, 30.0, 30.0, Event::Tab(1), "group Build")).unwrap().name, "Build");
    // an unnamed Esc region is the way out; an unnamed pointer region is nothing a person can press
    assert_eq!(node_of(&hit(0.0, 0.0, 30.0, 30.0, Event::Key(Key::Esc), "")).unwrap().speak(), "Close, button");
    assert!(node_of(&hit(0.0, 0.0, 30.0, 30.0, Event::MouseMove(0, 0), "x")).is_none());
    assert!(node_of(&hit(0.0, 0.0, 30.0, 30.0, Event::Tab(5), "")).is_none());
}

#[test]
fn roles_and_states_are_spoken_in_a_steady_order() {
    let toggle = Node::new("Dark mode", Role::Toggle, State::default().checked(true));
    assert_eq!(toggle.speak(), "Dark mode, switch, on");
    assert_eq!(Node::new("Dark mode", Role::Toggle, State::default().checked(false)).speak(), "Dark mode, switch, off");
    let slider = Node::new("Volume", Role::Slider, State::default().value("40 percent"));
    assert_eq!(slider.speak(), "Volume, slider, 40 percent");
    assert_eq!(Node::new("Network", Role::Tab, State::default().selected(true)).speak(), "Network, tab, selected");
    assert_eq!(Node::new("Advanced", Role::Button, State::default().expanded(false)).speak(), "Advanced, button, collapsed");
    assert_eq!(Node::new("Delete", Role::Button, State::default().disabled(true)).speak(), "Delete, button, unavailable");
    assert_eq!(Node::new("Settings", Role::Heading, State::default()).speak(), "Settings, heading");
    assert_eq!(Node::field("Name", "Ann", false).speak(), "Name, edit, Ann");
    assert_eq!(Node::field("Name", "", false).speak(), "Name, edit, empty");
    assert_eq!(Node::new("Remember me", Role::Button, State::default().checked(true)).speak(), "Remember me, button, checked");
}

#[test]
fn a_password_field_says_how_many_characters_and_never_which() {
    let n = Node::field("Password", "hunter2", true);
    assert_eq!(n.speak(), "Password, password field, 7 characters typed");
    assert!(!format!("{n:?}").contains("hunter2"), "the text is not even kept in the node");
    assert_eq!(Node::field("Password", "", true).speak(), "Password, password field, empty");
    assert_eq!(Node::field("Password", "x", true).speak(), "Password, password field, 1 character typed");
    // through the real widget: a masked TextField reports the focus without its text
    start_frame();
    let mut g = Gfx::new(400, 100);
    let mut f = TextField::new("Password", true);
    f.set("s3cret!");
    f.render(&mut g, R::new(10.0, 10.0, 300.0, 40.0), true, 1.0, true, crate::gfx::Face::Sans, 1);
    let focus = take_focus().expect("a focused field reports itself");
    assert_eq!(focus.speak(), "Password, password field, 7 characters typed");
    let from_hit = node_of(g.hits.iter().find(|h| h.label == "field").unwrap()).unwrap();
    assert!(!from_hit.speak().contains("s3cret") && from_hit.is_secret());
    // and a plain field reads its text
    start_frame();
    let mut p = TextField::new("Name", false);
    p.set("Ann");
    p.render(&mut g, R::new(10.0, 60.0, 300.0, 30.0), true, 1.0, true, crate::gfx::Face::Sans, 2);
    assert_eq!(take_focus().unwrap().speak(), "Name, edit, Ann");
    // an unfocused field reports nothing
    start_frame();
    p.render(&mut g, R::new(10.0, 60.0, 300.0, 30.0), false, 1.0, true, crate::gfx::Face::Sans, 2);
    assert!(take_focus().is_none());
}

#[test]
fn the_switch_and_the_slider_of_the_toolkit_describe_themselves() {
    start_frame();
    let mut g = Gfx::new(900, 300);
    crate::controls::switch_row(&mut g, R::new(10.0, 10.0, 600.0, 60.0), 1.0, "Large pointer", "help", true, true, Event::Key(Key::Char('p')), ACCENT_MAIN);
    crate::controls::switch_row(&mut g, R::new(10.0, 80.0, 600.0, 60.0), 1.0, "Sticky keys", "help", false, true, Event::Key(Key::Char('k')), ACCENT_MAIN);
    let sl = crate::controls::Slider { id: 7, title: "Volume", help: "", value: 40, min: 0, max: 100, unit: "%", minus: Event::Key(Key::Char('-')), plus: Event::Key(Key::Char('+')), enabled: true };
    crate::controls::slider_row(&mut g, R::new(10.0, 150.0, 800.0, 90.0), 1.0, &sl, ACCENT_MAIN);
    let said: Vec<String> = g.hits.iter().filter_map(node_of).map(|n| n.speak()).collect();
    assert!(said.contains(&"Large pointer, switch, on".to_string()), "{said:?}");
    assert!(said.contains(&"Sticky keys, switch, off".to_string()), "{said:?}");
    assert!(said.contains(&"Volume, slider, 40 percent".to_string()), "{said:?}");
    assert!(said.contains(&"Decrease Volume, button".to_string()) && said.contains(&"Increase Volume, button".to_string()), "{said:?}");
}

#[test]
fn the_reader_interrupts_at_a_new_focus_and_queues_notices_behind_what_is_said() {
    let mut r = Reader::new();
    let mut v = Fake::default();
    let a = Node::new("Save", Role::Button, State::default());
    let b = Node::new("Cancel", Role::Button, State::default());
    assert!(r.focus(&a, false));
    assert_eq!(r.pump(&mut v), None);
    assert_eq!(v.said, vec!["Save, button"]);
    r.notice("Saved");
    r.notice("Saved"); // the same notice twice in a row is said once
    assert_eq!(r.waiting(), 1);
    v.busy = true;
    r.pump(&mut v);
    assert_eq!(v.said.len(), 1, "a notice waits for the voice to finish");
    // the person moves on before it is said: everything waiting is dropped and the voice is stopped
    assert!(r.focus(&b, false));
    let stops = v.stops;
    r.pump(&mut v);
    assert_eq!(v.stops, stops + 1);
    assert_eq!(v.said.last().unwrap(), "Cancel, button", "said at once although the voice was busy");
    assert_eq!(r.waiting(), 0, "the old notice was dropped");
    // the same control again is not said again, unless asked
    assert!(!r.focus(&b, false) && r.focus(&b, true));
    // the voice free again: a notice goes through
    v.busy = false;
    r.pump(&mut v);
    r.notice("Done");
    r.pump(&mut v);
    assert_eq!(v.said.last().unwrap(), "Done");
    // say it again
    r.repeat();
    r.pump(&mut v);
    assert_eq!(v.said.last().unwrap(), "Done");
    // a new screen and its first control in one breath
    r.focus_after("Settings, Display", &a);
    r.pump(&mut v);
    assert_eq!(v.said.last().unwrap(), "Settings, Display. Save, button");
}

#[test]
fn a_voice_that_fails_is_reported_once_and_the_queue_is_dropped() {
    let mut r = Reader::new();
    let mut v = Fake { fail: Some("No reading voice is set up (Settings -> Voice).".into()), ..Default::default() };
    r.focus(&Node::new("Save", Role::Button, State::default()), false);
    r.notice("later");
    assert_eq!(r.pump(&mut v).as_deref(), Some("No reading voice is set up (Settings -> Voice)."));
    assert_eq!(r.waiting(), 0);
    r.focus(&Node::new("Cancel", Role::Button, State::default()), false);
    assert_eq!(r.pump(&mut v), None, "the same problem is not announced again");
    assert!(r.problem.is_some());
    v.fail = None;
    r.focus(&Node::new("Open", Role::Button, State::default()), false);
    assert_eq!(r.pump(&mut v), None);
    assert!(r.problem.is_none(), "a voice that works clears the problem");
    r.silence(&mut v);
    assert_eq!(r.waiting(), 0);
}

#[test]
fn the_walk_goes_in_reading_order_skips_what_is_no_control_and_wraps() {
    let screen = (1000.0, 600.0);
    let hits = vec![
        hit(0.0, 0.0, 1000.0, 600.0, Event::Key(Key::Esc), "backdrop"),
        hit(500.0, 100.0, 90.0, 30.0, Event::Key(Key::Char('b')), "Second"),
        hit(20.0, 102.0, 90.0, 30.0, Event::Key(Key::Char('a')), "First"),
        hit(20.0, 300.0, 200.0, 40.0, Event::Row(0), "Third row"),
        hit(20.0, 700.0, 100.0, 30.0, Event::Key(Key::Char('z')), "Off screen"),
        hit(20.0, 400.0, 0.5, 30.0, Event::Key(Key::Char('t')), "Hairline"),
        hit(20.0, 300.0, 200.0, 40.0, Event::Row(0), "Third row"),
        hit(20.0, 500.0, 100.0, 30.0, Event::MouseMove(0, 0), "Not a control"),
    ];
    let list = entries(&hits, screen);
    let names: Vec<&str> = list.iter().map(|e| e.node.name.as_str()).collect();
    assert_eq!(names, vec!["First", "Second", "Third row"], "rows top to bottom, left to right; the backdrop, the off-screen, the hairline and the duplicate are out");
    let mut c = Cursor::default();
    assert_eq!(c.step(&list, &hits, false), Some(0));
    assert_eq!(c.step(&list, &hits, false), Some(1));
    assert_eq!(c.step(&list, &hits, false), Some(2));
    assert_eq!(c.step(&list, &hits, false), Some(0), "wraps round");
    assert_eq!(c.step(&list, &hits, true), Some(2), "and back");
    // a redraw that moved the control keeps the cursor on it (by label and event)
    let moved = vec![hit(300.0, 50.0, 200.0, 40.0, Event::Row(0), "Third row"), hit(20.0, 102.0, 90.0, 30.0, Event::Key(Key::Char('a')), "First")];
    let list2 = entries(&moved, screen);
    assert_eq!(c.find(&list2, &moved).map(|i| list2[i].node.name.clone()).as_deref(), Some("Third row"));
    // the control is gone: the walk starts again from the first (or the last going back)
    let gone = vec![hit(20.0, 102.0, 90.0, 30.0, Event::Key(Key::Char('a')), "First")];
    let list3 = entries(&gone, screen);
    assert_eq!(c.step(&list3, &gone, true), Some(0));
    assert_eq!(c.step(&[], &gone, false), None);
    assert!(!c.is_set());
}

#[test]
fn sticky_keys_latch_lock_and_release_without_a_second_finger() {
    let mut k = StickyKeys::default();
    k.modifier(Modifier::Shift, true);
    k.modifier(Modifier::Shift, false);
    assert!(k.mods().shift && k.any_latched(), "pressed and let go: waiting for the next key");
    k.key_used();
    assert!(!k.mods().shift, "spent by the next key");
    // Super then Alt then S, one finger: both wait
    k.modifier(Modifier::Meta, true);
    k.modifier(Modifier::Meta, false);
    k.modifier(Modifier::Alt, true);
    k.modifier(Modifier::Alt, false);
    assert!(k.mods().meta && k.mods().alt && !k.mods().ctrl);
    k.key_used();
    assert_eq!(k.mods(), Mods::default());
    // held in the usual way with a key typed meanwhile: not latched afterwards
    k.modifier(Modifier::Ctrl, true);
    k.key_used();
    k.modifier(Modifier::Ctrl, false);
    assert!(!k.mods().ctrl, "a modifier used while held does not stay on");
    // twice in a row locks it; a third press frees it
    k.modifier(Modifier::Shift, true);
    k.modifier(Modifier::Shift, false);
    k.modifier(Modifier::Shift, true);
    k.modifier(Modifier::Shift, false);
    k.key_used();
    k.key_used();
    assert!(k.mods().shift, "locked: stays through ordinary keys");
    k.modifier(Modifier::Shift, true);
    k.modifier(Modifier::Shift, false);
    assert!(!k.mods().shift && !k.any_latched());
    // switching it off clears whatever was waiting
    k.set_enabled(true);
    k.modifier(Modifier::Alt, true);
    k.modifier(Modifier::Alt, false);
    k.set_enabled(false);
    assert_eq!(k.mods(), Mods::default());
}

#[test]
fn slow_keys_ignore_a_brush_and_accept_a_key_held_long_enough() {
    let mut s = SlowKeys::default();
    assert_eq!(s.filter(30, 1, 0), vec![1], "off: every key passes untouched");
    s.set_delay(500);
    // a brush: down and up within the delay
    assert_eq!(s.filter(30, 1, 1000), Vec::<i32>::new());
    assert_eq!(s.filter(30, 0, 1200), Vec::<i32>::new());
    // held 600 ms and let go before the repeat arrives: pressed and released together at the release
    assert_eq!(s.filter(30, 1, 2000), Vec::<i32>::new());
    assert_eq!(s.filter(30, 0, 2600), vec![1, 0]);
    // held through the kernel repeat: accepted at the first repeat after the delay, then repeats pass, then the release
    assert_eq!(s.filter(31, 1, 3000), Vec::<i32>::new());
    assert_eq!(s.filter(31, 2, 3250), Vec::<i32>::new(), "a repeat too early is not enough");
    assert_eq!(s.filter(31, 2, 3550), vec![1]);
    assert_eq!(s.filter(31, 2, 3600), vec![2]);
    assert_eq!(s.filter(31, 0, 3900), vec![0]);
    // two keys at once are timed apart
    assert_eq!(s.filter(1, 1, 5000), Vec::<i32>::new());
    assert_eq!(s.filter(2, 1, 5400), Vec::<i32>::new());
    assert_eq!(s.filter(1, 0, 5600), vec![1, 0]);
    assert_eq!(s.filter(2, 0, 5700), Vec::<i32>::new());
    // a release nobody pressed is nothing
    assert_eq!(s.filter(77, 0, 9000), Vec::<i32>::new());
}

#[test]
fn contrast_arithmetic_matches_wcag() {
    assert!((contrast_ratio([0, 0, 0, 255], [255, 255, 255, 255]) - 21.0).abs() < 1e-9);
    assert!((contrast_ratio([119, 119, 119, 255], [255, 255, 255, 255]) - 4.48).abs() < 0.02, "#777 on white is the textbook 4.48");
    assert_eq!(contrast_ratio([10, 20, 30, 255], [10, 20, 30, 255]), 1.0);
}

/// Every pair a person reads text in, in one theme, with the threshold the theme promises.
fn text_pairs(t: &theme::Theme) -> Vec<(&'static str, [u8; 4], &'static str, [u8; 4])> {
    let grounds = [("page top", t.bg_top), ("page bottom", t.bg_bottom), ("card", t.card), ("card under the pointer", t.card_hover), ("chosen card", t.card_sel), ("field", t.field), ("panel", t.panel), ("well", t.well), ("button", t.button), ("button under the pointer", t.button_hot)];
    let inks = [("text", t.text), ("muted", t.muted), ("dim", t.dim), ("accent", t.accent), ("error", t.err), ("ok", t.ok)];
    let mut v = Vec::new();
    for (gn, g) in grounds {
        for (iname, i) in inks {
            v.push((iname, i, gn, g));
        }
    }
    v
}

#[test]
fn high_contrast_meets_wcag_aaa_on_every_ground_and_the_others_meet_aa_for_their_text() {
    for id in theme::ALL {
        let t = id.theme();
        let need = if id == ThemeId::HighContrast { 7.0 } else { 4.5 };
        let mut worst = (f64::MAX, String::new());
        for (iname, ink, gname, ground) in text_pairs(t) {
            // the lighter and the accent inks are only promised on the grounds text sits on in that theme
            if id != ThemeId::HighContrast && !matches!(iname, "text" | "muted" | "dim") {
                continue;
            }
            if id != ThemeId::HighContrast && matches!(gname, "card under the pointer" | "chosen card" | "button under the pointer") && iname == "dim" {
                continue;
            }
            let c = contrast_ratio(ink, ground);
            if c < worst.0 {
                worst = (c, format!("{iname} on {gname}"));
            }
            assert!(c >= need, "{}: {iname} {ink:?} on {gname} {ground:?} is {c:.2}:1, needs {need}:1", id.name());
        }
        eprintln!("{}: lowest text contrast {:.2}:1 ({})", id.name(), worst.0, worst.1);
    }
}

#[test]
fn high_contrast_borders_and_the_focus_ring_are_visible_and_the_ring_is_thick() {
    let t = ThemeId::HighContrast.theme();
    for (n, ground) in [("page", t.bg_top), ("card", t.card), ("field", t.field), ("panel", t.panel), ("well", t.well), ("chosen card", t.card_sel)] {
        assert!(contrast_ratio(t.border, ground) >= 7.0, "border on {n}");
        assert!(contrast_ratio(t.accent, ground) >= 3.0, "the focus ring (accent) on {n} needs 3:1 as a non-text mark");
    }
    assert!(t.ring >= 2.0);
}

#[test]
fn pixels_of_real_widgets_in_high_contrast_keep_the_ratio() {
    theme::set_current(ThemeId::HighContrast);
    let mut g = Gfx::new(600, 200);
    crate::fx::background(&mut g);
    g.button(20.0, 20.0, 16.0, "Save", ACCENT_MAIN, Event::Key(Key::Char('s')));
    g.button(140.0, 20.0, 16.0, "Cancel", crate::ui::MUTED, Event::Key(Key::Esc));
    // the text of a button is drawn in the colour the pill returns; sample its fill beside the text and compare with that colour
    let (ink, fill) = {
        let mut probe = Gfx::new(300, 80);
        crate::fx::background(&mut probe);
        let r = R::new(20.0, 20.0, 120.0, 34.0);
        let ink = crate::fx::pill(&mut probe, r, 1.0, ACCENT_MAIN, false, true);
        let i = ((37 * probe.width() + 24) * 4) as usize;
        let d = probe.rgba();
        (ink, [d[i], d[i + 1], d[i + 2], 255])
    };
    let c = contrast_ratio(ink, fill);
    theme::set_current(ThemeId::Dark);
    assert!(c >= 7.0, "button text {ink:?} on its fill {fill:?} is {c:.2}:1");
}

#[test]
fn the_display_options_reach_the_drawing_code() {
    set_prefs(Prefs { screen_reader: true, large_pointer: true, big_ring: true, reduce_motion: true });
    assert!(crate::fx::reduced_motion());
    assert_eq!(pointer_boost(), 2);
    assert_eq!(focus_ring_factor(), 2.0);
    assert!(crate::blink::blink(0) == crate::blink::blink(15) && crate::blink::blink(0) == crate::ui::input_focus_allowed(), "a steady caret");
    set_prefs(Prefs::OFF);
    assert!(!crate::fx::reduced_motion() && pointer_boost() == 0 && focus_ring_factor() == 1.0);
    assert!(!crate::blink::blink(15), "the caret blinks again");
}

#[test]
fn announcements_are_capped_and_taken_once() {
    let _ = take_announcements();
    for i in 0..40 {
        announce(&format!("n{i}"));
    }
    let v = take_announcements();
    assert_eq!(v.len(), 16);
    assert_eq!(v.last().unwrap(), "n39");
    assert!(take_announcements().is_empty());
}
