//! The on-screen keyboard for touch screens.
//!
//! It shows itself when a text field has the focus and the last thing the person used was a finger; a physical key hides it again,
//! so a laptop with a touch screen and a keyboard never gets in its own way. A small keyboard button (shown whenever a field has the
//! focus, whoever is using the machine) opens it by hand, and its own "hide" key closes it until another field is chosen.
//!
//! This file is the model: the layers, shift and caps lock, the key layout for a given screen, which key a finger is on, and the
//! repeat of a held backspace or arrow. The shell draws it and feeds it (`shell_osk.rs`). A key becomes the same `Event::Key` a
//! physical keyboard would send, so every field, form and editor works with it unchanged.

use crate::safeclamp::SafeClamp;
use crate::ui::{Event, Key, R};
use std::sync::atomic::{AtomicBool, Ordering};

/// A touch screen produced a press since the shell last asked (set by the input threads).
static TOUCHED: AtomicBool = AtomicBool::new(false);

/// The input side calls this when a finger comes down.
pub fn note_touch() {
    TOUCHED.store(true, Ordering::SeqCst);
}

/// Has a finger come down since the last call?
pub fn take_touch() -> bool {
    TOUCHED.swap(false, Ordering::SeqCst)
}

/// The shell's view of "the last thing used was a finger" (mirrors `Osk::touch`), for tiles that must tell a tap from a click.
static TOUCH_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_touch_mode(on: bool) {
    TOUCH_MODE.store(on, Ordering::SeqCst);
}

/// Is the last input a finger (not a mouse or a physical key)?
pub fn touch_mode() -> bool {
    TOUCH_MODE.load(Ordering::SeqCst)
}

/// Heartbeats a finger must stay on backspace or an arrow before it repeats (30 a second).
const REPEAT_AFTER: u64 = 14;
/// Heartbeats between two repeats.
const REPEAT_EVERY: u64 = 3;
/// Heartbeats within which a physical key right after a tap on a button is not taken for a keyboard being used.
const SAME_MOMENT: u64 = 30;
/// Heartbeats within which a second tap on Shift is caps lock.
const DOUBLE_TAP: u64 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Letters,
    /// Digits and the common punctuation.
    Symbols,
    /// The rest of the punctuation.
    More,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Off,
    /// The next letter is a capital.
    Once,
    Lock,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// A character; letters follow the shift state.
    Char(char),
    /// Any other key a physical keyboard has.
    Key(Key),
    Shift,
    Layer(Layer),
    Hide,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cap {
    pub rect: R,
    pub label: String,
    pub act: Act,
    /// Held down, it repeats.
    pub repeat: bool,
    /// Drawn in the accent colour (Enter).
    pub accent: bool,
}

#[derive(Clone, Debug)]
struct Held {
    ev: Event,
    since: u64,
    last: u64,
}

#[derive(Clone, Debug)]
pub struct Osk {
    pub layer: Layer,
    pub shift: Shift,
    /// A finger has been used on this machine (a touch screen exists and works).
    pub touch: bool,
    /// Opened by the keyboard button, shown even when the last input was not a finger.
    pub manual: bool,
    /// Closed by its hide key; stays closed until another field is chosen.
    pub dismissed: bool,
    /// Where the keyboard was drawn in the last frame (`None`: not shown).
    pub shown: Option<R>,
    /// Where the keyboard button was drawn in the last frame.
    pub button: Option<R>,
    caps: Vec<Cap>,
    focus: Option<usize>,
    last_touch: u64,
    last_shift: Option<u64>,
    held: Option<Held>,
    /// A press was taken by the keyboard; its release must not reach the screen below.
    pressed: bool,
    /// A key of this keyboard is being delivered: it is not a physical key.
    pub typing: bool,
}

impl Default for Osk {
    fn default() -> Osk {
        Osk { layer: Layer::Letters, shift: Shift::Off, touch: false, manual: false, dismissed: false, shown: None, button: None, caps: Vec::new(), focus: None, last_touch: 0, last_shift: None, held: None, pressed: false, typing: false }
    }
}

/// The three rows of characters of a layer, as strings of single-character keys.
fn rows(layer: Layer) -> [&'static str; 3] {
    match layer {
        Layer::Letters => ["qwertyuiop", "asdfghjkl", "zxcvbnm"],
        Layer::Symbols => ["1234567890", "-/:;()$&@\"", ".,?!'"],
        Layer::More => ["[]{}#%^*+=", "_\\|~<>`", ".,?!'"],
    }
}

/// Keys per row at 10 units of width: the unit all rows are measured in.
const UNITS: f32 = 10.0;

impl Osk {
    /// The keyboard's rectangle on a `w` by `h` screen at text scale `s`: along the bottom edge, as wide as is useful.
    pub fn area(w: f32, h: f32, s: f32) -> R {
        let kh = Self::key_height(h, s);
        let gap = kh * 0.12;
        let total = 4.0 * kh + 5.0 * gap + gap * 0.5;
        let width = w.min(h * 1.9).min(1600.0 * s.max(1.0));
        R::new((w - width) / 2.0, h - total, width, total)
    }

    fn key_height(h: f32, s: f32) -> f32 {
        (h * 0.07).clamp_safe(44.0 * s.min(1.5), 110.0)
    }

    /// The keys for the current layer and shift state inside `area`.
    pub fn layout(&self, area: R) -> Vec<Cap> {
        let kh = (area.h / 4.66).floor();
        let gap = kh * 0.12;
        let unit = (area.w - 2.0 * gap) / UNITS - gap;
        let rows = rows(self.layer);
        let mut out = Vec::new();
        let mut y = area.y + gap;
        // a row of single keys with optional wide keys at either end: (units, label, act, repeat, accent)
        let row = |out: &mut Vec<Cap>, y: f32, items: &[(f32, String, Act, bool, bool)]| {
            let units: f32 = items.iter().map(|i| i.0).sum();
            let width = units * (unit + gap) - gap;
            let mut x = area.x + (area.w - width) / 2.0;
            for (u, label, act, repeat, accent) in items {
                let w = u * (unit + gap) - gap;
                out.push(Cap { rect: R::new(x, y, w, kh), label: label.clone(), act: act.clone(), repeat: *repeat, accent: *accent });
                x += w + gap;
            }
        };
        let single = |c: char, up: bool| -> (f32, String, Act, bool, bool) {
            let shown = if up { c.to_uppercase().next().unwrap_or(c) } else { c };
            (1.0, shown.to_string(), Act::Char(c), false, false)
        };
        let up = self.layer == Layer::Letters && self.shift != Shift::Off;
        // row 1 and 2: the characters
        for r in 0..2 {
            let items: Vec<_> = rows[r].chars().map(|c| single(c, up)).collect();
            row(&mut out, y, &items);
            y += kh + gap;
        }
        // row 3: a modifier key, the characters, backspace
        let mut items: Vec<(f32, String, Act, bool, bool)> = Vec::new();
        items.push(match self.layer {
            Layer::Letters => (1.5, if self.shift == Shift::Lock { "caps".into() } else { "shift".into() }, Act::Shift, false, false),
            Layer::Symbols => (1.5, "#+=".into(), Act::Layer(Layer::More), false, false),
            Layer::More => (1.5, "123".into(), Act::Layer(Layer::Symbols), false, false),
        });
        items.extend(rows[2].chars().map(|c| single(c, up)));
        items.push((1.5, "backspace".into(), Act::Key(Key::Backspace), true, false));
        row(&mut out, y, &items);
        y += kh + gap;
        // row 4: layer switch, caret keys, space, Enter, hide
        let switch = if self.layer == Layer::Letters { ("?123", Layer::Symbols) } else { ("ABC", Layer::Letters) };
        let items = [
            (1.5, switch.0.to_string(), Act::Layer(switch.1), false, false),
            (1.0, "left".into(), Act::Key(Key::Left), true, false),
            (1.0, ",".into(), Act::Char(','), false, false),
            (2.0, "space".into(), Act::Char(' '), false, false),
            (1.0, ".".into(), Act::Char('.'), false, false),
            (1.0, "right".into(), Act::Key(Key::Right), true, false),
            (1.5, "enter".into(), Act::Key(Key::Enter), false, true),
            (1.0, "hide".into(), Act::Hide, false, false),
        ];
        row(&mut out, y, &items);
        out
    }

    /// The keyboard button: bottom right, out of the way of the keyboard itself.
    pub fn button_rect(w: f32, h: f32, s: f32) -> R {
        let d = (48.0 * s).clamp(44.0, 84.0);
        R::new(w - d - 14.0 * s, h - d - 14.0 * s, d, d)
    }

    /// Keep the state in step with the screen: `focus` is the field that owns the keyboard (`None`: no text field has the focus).
    /// Returns whether the keyboard is to be shown.
    pub fn update(&mut self, focus: Option<usize>) -> bool {
        if focus.is_none() {
            self.manual = false;
            self.dismissed = false;
            self.focus = None;
            return false;
        }
        if focus != self.focus {
            self.focus = focus;
            self.dismissed = false;
        }
        !self.dismissed && (self.manual || self.touch)
    }

    /// Should the small button that opens the keyboard be shown?
    /// Always while a field has the focus: a person with a mouse, or a touch screen the system did not recognise as one, can still ask for it.
    pub fn wants_button(&self, focus: Option<usize>, visible: bool) -> bool {
        focus.is_some() && !visible
    }

    /// A finger came down at `tick`.
    pub fn touched(&mut self, tick: u64) {
        self.touch = true;
        self.last_touch = tick;
    }

    /// A physical key was pressed: the keyboard steps aside (not when it is the shell's own re-delivery of a tap).
    pub fn physical_key(&mut self, tick: u64) {
        if tick.saturating_sub(self.last_touch) > SAME_MOMENT && self.touch {
            self.touch = false;
            self.manual = false;
        }
    }

    /// Remember the keys of the frame just drawn (so a press can find its key).
    pub fn set_frame(&mut self, area: Option<R>, caps: Vec<Cap>) {
        self.shown = area;
        self.caps = caps;
    }

    /// A press at (`x`, `y`). Returns what to do: the event for the field, or nothing (a layer or shift change, the hide key, a miss).
    /// `None` outside the keyboard means the press belongs to the screen below.
    pub fn press(&mut self, x: i32, y: i32, tick: u64) -> Option<Option<Event>> {
        let area = self.shown?;
        if !area.contains(x, y) {
            return None;
        }
        self.pressed = true;
        let Some(c) = self.caps.iter().find(|c| c.rect.contains(x, y)).cloned() else { return Some(None) };
        Some(match c.act {
            Act::Shift => {
                self.shift = match (self.shift, self.last_shift) {
                    (Shift::Off, _) => Shift::Once,
                    (Shift::Once, Some(t)) if tick.saturating_sub(t) <= DOUBLE_TAP => Shift::Lock,
                    _ => Shift::Off,
                };
                self.last_shift = Some(tick);
                None
            }
            Act::Layer(l) => {
                self.layer = l;
                if l != Layer::Letters {
                    self.shift = Shift::Off;
                }
                None
            }
            Act::Hide => {
                self.dismissed = true;
                self.manual = false;
                None
            }
            Act::Char(ch) => {
                let ch = if self.layer == Layer::Letters && self.shift != Shift::Off { ch.to_uppercase().next().unwrap_or(ch) } else { ch };
                if self.shift == Shift::Once {
                    self.shift = Shift::Off;
                }
                // a number or a symbol typed once: a one-off symbol layer goes back to the letters after a space
                if ch == ' ' && self.layer != Layer::Letters {
                    self.layer = Layer::Letters;
                }
                Some(Event::Key(Key::Char(ch)))
            }
            Act::Key(k) => {
                let ev = Event::Key(k);
                if c.repeat {
                    self.held = Some(Held { ev: ev.clone(), since: tick, last: tick });
                }
                Some(ev)
            }
        })
    }

    /// The finger lifted. Returns whether the press had been taken by the keyboard (its release then goes nowhere else).
    pub fn release(&mut self) -> bool {
        self.held = None;
        std::mem::take(&mut self.pressed)
    }

    /// A heartbeat: a held backspace or arrow repeats.
    pub fn repeat(&mut self, tick: u64) -> Option<Event> {
        let h = self.held.as_mut()?;
        if tick.saturating_sub(h.since) >= REPEAT_AFTER && tick.saturating_sub(h.last) >= REPEAT_EVERY {
            h.last = tick;
            return Some(h.ev.clone());
        }
        None
    }

    /// Is a key being held (the shell then keeps drawing and ticking)?
    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// Test only: the key labelled `label` in the last frame.
    #[cfg(test)]
    pub fn cap_named(&self, label: &str) -> Option<R> {
        self.caps.iter().find(|c| c.label == label).map(|c| c.rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(r: R) -> (i32, i32) {
        ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
    }

    fn shown(o: &mut Osk, w: f32, h: f32) -> R {
        let area = Osk::area(w, h, 1.0);
        let caps = o.layout(area);
        o.set_frame(Some(area), caps);
        area
    }

    fn tap(o: &mut Osk, label: &str, tick: u64) -> Option<Event> {
        let (x, y) = centre(o.cap_named(label).unwrap_or_else(|| panic!("no key `{label}`")));
        let out = o.press(x, y, tick).expect("inside the keyboard");
        o.release();
        out
    }

    #[test]
    fn the_keyboard_fits_the_screen_at_common_sizes_and_no_key_overlaps() {
        for (w, h) in [(800.0, 480.0), (1024.0, 600.0), (1280.0, 720.0), (1920.0, 1080.0), (3840.0, 2160.0), (720.0, 1280.0)] {
            let mut o = Osk::default();
            for layer in [Layer::Letters, Layer::Symbols, Layer::More] {
                o.layer = layer;
                let area = Osk::area(w, h, 1.0);
                assert!(area.y >= 0.0 && area.y + area.h <= h + 0.5 && area.x >= 0.0 && area.x + area.w <= w + 0.5, "{w}x{h}");
                assert!(area.h <= h * 0.55, "{w}x{h}: the keyboard takes {} of {h}", area.h);
                let caps = o.layout(area);
                for (i, a) in caps.iter().enumerate() {
                    assert!(a.rect.x >= area.x - 0.5 && a.rect.x + a.rect.w <= area.x + area.w + 0.5 && a.rect.y >= area.y && a.rect.y + a.rect.h <= area.y + area.h + 0.5, "{w}x{h} `{}` leaves the keyboard", a.label);
                    for b in caps.iter().skip(i + 1) {
                        let apart = a.rect.x + a.rect.w <= b.rect.x + 0.01 || b.rect.x + b.rect.w <= a.rect.x + 0.01 || a.rect.y + a.rect.h <= b.rect.y + 0.01 || b.rect.y + b.rect.h <= a.rect.y + 0.01;
                        assert!(apart, "{w}x{h}: `{}` and `{}` overlap", a.label, b.label);
                    }
                    assert!(a.rect.h >= 30.0 && a.rect.w >= 30.0, "{w}x{h}: `{}` is too small to touch", a.label);
                }
            }
        }
    }

    #[test]
    fn every_printable_ascii_character_can_be_typed() {
        let mut typed = std::collections::BTreeSet::new();
        for layer in [Layer::Letters, Layer::Symbols, Layer::More] {
            let mut o = Osk { layer, ..Osk::default() };
            let area = Osk::area(1280.0, 720.0, 1.0);
            for c in o.layout(area) {
                if let Act::Char(ch) = c.act {
                    typed.insert(ch);
                }
            }
            o.shift = Shift::Once;
            for c in o.layout(area) {
                if let (Act::Char(ch), Layer::Letters) = (c.act, layer) {
                    typed.extend(ch.to_uppercase());
                }
            }
        }
        for b in 0x20u8..0x7f {
            assert!(typed.contains(&(b as char)), "`{}` has no key", b as char);
        }
    }

    #[test]
    fn a_tap_on_a_letter_sends_the_key_a_physical_keyboard_would() {
        let mut o = Osk::default();
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "q", 1), Some(Event::Key(Key::Char('q'))));
        assert_eq!(tap(&mut o, "space", 2), Some(Event::Key(Key::Char(' '))));
        assert_eq!(tap(&mut o, "backspace", 3), Some(Event::Key(Key::Backspace)));
        assert_eq!(tap(&mut o, "enter", 4), Some(Event::Key(Key::Enter)));
        assert_eq!(tap(&mut o, "left", 5), Some(Event::Key(Key::Left)));
    }

    #[test]
    fn shift_capitalises_one_letter_and_two_quick_taps_lock_it() {
        let mut o = Osk::default();
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "shift", 10), None);
        assert_eq!(o.shift, Shift::Once);
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "A", 11), Some(Event::Key(Key::Char('A'))), "the key is shown and typed as a capital");
        assert_eq!(o.shift, Shift::Off, "one capital, then back to small letters");
        // double tap: caps lock
        shown(&mut o, 1280.0, 720.0);
        tap(&mut o, "shift", 20);
        shown(&mut o, 1280.0, 720.0);
        tap(&mut o, "shift", 22);
        assert_eq!(o.shift, Shift::Lock);
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "caps", 100), None);
        assert_eq!(o.shift, Shift::Off, "a tap on the lock key lets it go");
    }

    #[test]
    fn the_layers_reach_digits_and_every_symbol_and_come_back() {
        let mut o = Osk::default();
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "?123", 1), None);
        assert_eq!(o.layer, Layer::Symbols);
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(tap(&mut o, "7", 2), Some(Event::Key(Key::Char('7'))));
        assert_eq!(tap(&mut o, "#+=", 3), None);
        shown(&mut o, 1280.0, 720.0);
        assert_eq!(o.layer, Layer::More);
        assert_eq!(tap(&mut o, "~", 4), Some(Event::Key(Key::Char('~'))));
        assert_eq!(tap(&mut o, "ABC", 5), None);
        assert_eq!(o.layer, Layer::Letters);
    }

    #[test]
    fn a_held_backspace_repeats_after_a_pause_and_stops_when_the_finger_lifts() {
        let mut o = Osk::default();
        shown(&mut o, 1280.0, 720.0);
        let (x, y) = centre(o.cap_named("backspace").expect("backspace"));
        assert_eq!(o.press(x, y, 100), Some(Some(Event::Key(Key::Backspace))));
        assert_eq!(o.repeat(105), None, "not yet");
        assert_eq!(o.repeat(100 + REPEAT_AFTER), Some(Event::Key(Key::Backspace)));
        assert_eq!(o.repeat(100 + REPEAT_AFTER + 1), None, "one repeat per few beats");
        assert_eq!(o.repeat(100 + REPEAT_AFTER + REPEAT_EVERY), Some(Event::Key(Key::Backspace)));
        assert!(o.release(), "the release belongs to the keyboard");
        assert_eq!(o.repeat(500), None);
        assert!(!o.holding());
    }

    #[test]
    fn a_press_outside_belongs_to_the_screen_below_and_inside_it_does_not() {
        let mut o = Osk::default();
        let area = shown(&mut o, 1280.0, 720.0);
        assert!(o.press(10, 10, 1).is_none(), "the top of the screen is not the keyboard");
        assert!(!o.release());
        assert!(o.press((area.x + 1.0) as i32, (area.y + 1.0) as i32, 2).is_some(), "the gap between keys is still the keyboard's");
        assert!(o.release());
    }

    #[test]
    fn it_appears_for_a_finger_not_for_a_mouse_and_steps_aside_for_a_physical_key() {
        let mut o = Osk::default();
        assert!(!o.update(Some(1)), "a mouse user clicking a field gets no keyboard");
        assert!(o.wants_button(Some(1), false), "but the button is there to ask for it");
        assert!(!o.wants_button(None, false), "and only while a field has the focus");
        o.touched(100);
        assert!(o.update(Some(1)), "a finger on a field opens it");
        assert!(!o.update(None), "no field focused: nothing to type into");
        assert!(o.update(Some(1)));
        // a key from the keyboard right after the tap is the shell's own and changes nothing
        o.physical_key(105);
        assert!(o.update(Some(1)));
        // later a real key press: the person is typing on the keyboard
        o.physical_key(400);
        assert!(!o.update(Some(1)));
    }

    #[test]
    fn hide_stays_hidden_until_another_field_is_chosen_and_the_button_brings_it_back() {
        let mut o = Osk::default();
        o.touched(1);
        assert!(o.update(Some(1)));
        shown(&mut o, 1280.0, 720.0);
        tap(&mut o, "hide", 2);
        assert!(!o.update(Some(1)), "hidden by its own key");
        assert!(o.wants_button(Some(1), false), "the button offers it again");
        assert!(o.update(Some(2)), "another field was chosen");
    }

    #[test]
    fn the_keyboard_button_opens_it_by_hand_for_a_pointer_user_with_a_touch_screen_too() {
        let mut o = Osk::default();
        o.touch = true;
        assert!(o.update(Some(3)));
        o.dismissed = true;
        assert!(!o.update(Some(3)));
        o.dismissed = false;
        o.manual = true;
        assert!(o.update(Some(3)));
        assert!(!o.update(None));
        assert!(!o.manual, "gone with the field");
    }
}
