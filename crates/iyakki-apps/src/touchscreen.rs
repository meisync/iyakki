//! Fingers on a touch screen: raw events of one device in, pointer events on the display it belongs to out.
//!
//! One [`Touches`] holds the state of every touch device (by key): the multi-touch slots, which finger drives the pointer (the
//! first one down; the others are followed but do not move it) and the display rectangle a touch started on, which stays
//! with it until the finger lifts even when the mapping changes meanwhile. Positions are desktop positions
//! ([`crate::touchmap::map_point`]); the caller turns them into display-local ones as for a mouse.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::input::{Raw, EV_ABS, EV_KEY, EV_SYN};
use crate::touchmap::{map_point, OutRect, TouchDev};
use crate::ui::{Event, Key, Nav};

const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const BTN_TOUCH: u16 = 0x14a;
const SLOTS: usize = 10;

/// A finger that moves more than this many desktop pixels from where it landed is a drag, not a tap.
const SLOP: i32 = 12;
/// A finger that stays this long before it moves keeps the mouse meaning: the drag is a mouse drag (a slider, a text selection, a window).
const LONG_PRESS: Duration = Duration::from_millis(450);

/// A finger that stays this long without moving beyond the slop opens the context menu when it lifts, instead of clicking. The menu
/// comes on release, not while the finger is still down: a still finger sends no events, so nothing would wake the code up at
/// 600 ms, and a menu that pops up under the finger would be tapped by accident. Moving after the hold (from 450 ms on) is still the
/// mouse drag, so a hold-drag selects text; only a hold that never moves ends in the menu.
const MENU_HOLD: Duration = Duration::from_millis(600);

/// Set when the last context menu was asked for by a finger, so the shell makes its entries big enough to tap.
static MENU_BY_TOUCH: AtomicBool = AtomicBool::new(false);

/// Whether the context menu about to open was asked for by a finger (read once per menu).
pub fn take_touch_menu() -> bool {
    MENU_BY_TOUCH.swap(false, Ordering::SeqCst)
}

/// Whether new [`Touches`] start with finger gestures on. The real backends turn it on once at start; the tests leave it off
/// (a finger is then a plain mouse: press at once, move, release).
static GESTURES_BY_DEFAULT: AtomicBool = AtomicBool::new(false);

/// Finger gestures for every [`Touches`] made from now on: a tap clicks when the finger lifts, a quick drag scrolls the page under
/// it, and a drag that starts after a long press is a mouse drag. `iyakki.touchscroll=0` on the kernel command line keeps the plain mouse.
pub fn enable_gestures_by_default(on: bool) {
    GESTURES_BY_DEFAULT.store(on, Ordering::SeqCst);
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Mode {
    #[default]
    Idle,
    /// The finger landed; nothing has been sent yet (gestures on).
    Pending,
    /// It moved quickly: its movement scrolls.
    Scroll,
    /// It was held, then moved: a mouse drag; the press has been sent.
    Mouse,
    /// A second finger came down while the first had sent nothing: a pinch or a two-finger swipe, and no click.
    Two,
}

/// Two fingers: how far apart they were and where their middle was when they began, and whether a swipe has been sent already.
#[derive(Clone, Copy, Debug)]
struct Pair {
    d0: f64,
    mid0: (i32, i32),
    swiped: bool,
    /// How far apart the fingers are now against when the gesture began (1.0: not at all); the pinch the tile hears (F-SDK-34).
    total: f64,
    /// Whether a pinch step was sent, so the end of the gesture is told too.
    pinched: bool,
}

/// Two fingers that move this much further apart (or closer together) than they started are a pinch.
const PINCH_RATIO: f64 = 1.3;
/// Two fingers whose middle moves this many desktop pixels sideways, without pinching, are a swipe.
const SWIPE_PX: i32 = 160;
/// One finger that moves quickly this many desktop pixels sideways, at least twice as far as up or down, is a swipe between the groups
/// of Home (the shell takes it only on Home, from a touch that began on the group area).
pub const HOME_SWIPE_PX: i32 = 140;

#[derive(Clone, Copy, Default)]
struct Finger {
    down: bool,
    x: i32,
    y: i32,
}

#[derive(Default)]
struct One {
    slot: usize,
    fingers: [Finger; SLOTS],
    /// The finger that drives the pointer.
    primary: Option<usize>,
    pending_down: bool,
    pending_up: bool,
    moved: bool,
    /// Where the touch in progress started (kept until it ends).
    latched: Option<OutRect>,
    /// Last pointer position (desktop).
    last: (i32, i32),
    mode: Mode,
    /// Where and when the finger landed, and the height the scroll has followed it to (gestures on).
    start: (i32, i32),
    since: Option<Instant>,
    scroll_y: i32,
    /// Two-finger gestures (gestures on): the pair being tracked, and whether a finger other than the first moved in this frame.
    pair: Option<Pair>,
    other_moved: bool,
    /// A sideways swipe was already sent for the touch in progress.
    swiped: bool,
}

pub struct Touches {
    devs: Vec<(String, One)>,
    gestures: bool,
}

impl Default for Touches {
    fn default() -> Self {
        Touches { devs: Vec::new(), gestures: GESTURES_BY_DEFAULT.load(Ordering::SeqCst) }
    }
}

/// What a frame of one device did.
#[derive(Default, Debug, PartialEq)]
pub struct TouchOut {
    pub events: Vec<Event>,
    /// A finger came down in this frame (the calibration listens for it).
    pub down: bool,
}

impl Touches {
    /// Turn finger gestures on or off for this set of touch screens.
    pub fn set_gestures(&mut self, on: bool) {
        self.gestures = on;
    }

    fn one(&mut self, key: &str) -> &mut One {
        if let Some(i) = self.devs.iter().position(|d| d.0 == key) {
            return &mut self.devs[i].1;
        }
        self.devs.push((key.to_string(), One::default()));
        &mut self.devs.last_mut().expect("just pushed").1
    }

    /// The device went away: a finger still down is lifted.
    pub fn gone(&mut self, key: &str) -> Vec<Event> {
        let Some(i) = self.devs.iter().position(|d| d.0 == key) else { return Vec::new() };
        let (_, t) = self.devs.remove(i);
        // with gestures on, a press was sent only for a mouse drag
        if t.primary.is_some() && (!self.gestures || t.mode == Mode::Mouse) {
            vec![Event::MouseUp(t.last.0, t.last.1)]
        } else {
            Vec::new()
        }
    }

    pub fn feed(&mut self, r: Raw, dev: &TouchDev, target: Option<&OutRect>) -> TouchOut {
        let gestures = self.gestures;
        crate::agent().touchrecover_note_event();
        let t = self.one(&dev.key);
        match r.typ {
            EV_ABS if dev.mt => match r.code {
                ABS_MT_SLOT => t.slot = (r.value.max(0) as usize).min(SLOTS - 1),
                ABS_MT_TRACKING_ID if r.value >= 0 => {
                    crate::agent().touchprobe_note_finger();
                    let s = t.slot;
                    t.fingers[s].down = true;
                    t.begin(s, target);
                }
                ABS_MT_TRACKING_ID => {
                    let s = t.slot;
                    t.fingers[s].down = false;
                    t.end(s);
                }
                ABS_MT_POSITION_X => t.pos(r.value, true),
                ABS_MT_POSITION_Y => t.pos(r.value, false),
                _ => {}
            },
            EV_ABS => match r.code {
                ABS_X => t.pos_at(0, r.value, true),
                ABS_Y => t.pos_at(0, r.value, false),
                _ => {}
            },
            EV_KEY if !dev.mt && r.code == BTN_TOUCH => {
                if r.value == 1 {
                    crate::agent().touchprobe_note_finger();
                    t.fingers[0].down = true;
                    t.begin(0, target);
                } else if r.value == 0 {
                    t.fingers[0].down = false;
                    t.end(0);
                }
            }
            EV_SYN if r.code == 0 => return if gestures { t.frame_gestures(dev, target) } else { t.frame(dev, target) },
            _ => {}
        }
        TouchOut::default()
    }
}

impl One {
    fn begin(&mut self, slot: usize, target: Option<&OutRect>) {
        if self.primary.is_none() {
            self.primary = Some(slot);
            self.pending_down = true;
            self.latched = target.cloned();
        }
    }

    fn end(&mut self, slot: usize) {
        if self.primary == Some(slot) {
            self.pending_up = true;
        }
    }

    fn pos(&mut self, v: i32, horizontal: bool) {
        let s = self.slot;
        self.pos_at(s, v, horizontal);
    }

    fn pos_at(&mut self, slot: usize, v: i32, horizontal: bool) {
        if horizontal {
            self.fingers[slot].x = v;
        } else {
            self.fingers[slot].y = v;
        }
        if self.primary == Some(slot) {
            self.moved = true;
        } else if self.fingers[slot].down {
            self.other_moved = true;
        }
    }

    /// [`One::frame`] with gestures: the press is not sent when the finger lands. A finger that lifts without moving far clicks where
    /// it landed; one that moves at once scrolls (the page follows the finger); one held for a moment and then moved is a mouse drag.
    fn frame_gestures(&mut self, dev: &TouchDev, target: Option<&OutRect>) -> TouchOut {
        let mut out = TouchOut::default();
        let rect = self.latched.clone().or_else(|| target.cloned());
        if let Some(rect) = rect.as_ref().filter(|_| self.latched.is_some()) {
            self.two_fingers(dev, rect, &mut out);
        }
        if let (Some(p), Some(rect)) = (self.primary, rect) {
            let at = map_point(dev, &rect, self.fingers[p].x, self.fingers[p].y);
            if self.pending_down {
                self.mode = Mode::Pending;
                self.swiped = false;
                self.start = at;
                self.since = Some(Instant::now());
                self.last = at;
            } else if self.moved && self.mode != Mode::Two {
                self.last = at;
                if self.mode == Mode::Pending {
                    let (dx, dy) = (at.0 - self.start.0, at.1 - self.start.1);
                    if dx * dx + dy * dy > SLOP * SLOP && self.latched.is_some() {
                        if self.since.is_some_and(|t| t.elapsed() >= LONG_PRESS) {
                            self.mode = Mode::Mouse;
                            out.events.push(Event::MouseMove(self.start.0, self.start.1));
                            out.events.push(Event::MouseDown(self.start.0, self.start.1));
                        } else {
                            self.mode = Mode::Scroll;
                            self.scroll_y = self.start.1;
                            // the pointer sits where the finger landed, so the scroll goes to what is under it
                            out.events.push(Event::MouseMove(self.start.0, self.start.1));
                        }
                    }
                }
                match self.mode {
                    Mode::Mouse => out.events.push(Event::MouseMove(at.0, at.1)),
                    Mode::Scroll => {
                        let (sx, sy) = (at.0 - self.start.0, at.1 - self.start.1);
                        if !self.swiped && sx.abs() >= HOME_SWIPE_PX && sx.abs() >= 2 * sy.abs() {
                            self.swiped = true;
                            out.events.push(Event::Swipe { back: sx > 0, at: Some(self.start) });
                        }
                        // positive scrolls up, like a wheel turned away from the person: the page follows a finger that moves down
                        let dy = at.1 - self.scroll_y;
                        if dy != 0 {
                            out.events.push(Event::ScrollPx(dy));
                            self.scroll_y = at.1;
                        }
                    }
                    _ => {}
                }
            }
        }
        if std::mem::take(&mut self.pending_down) {
            out.down = true; // a calibration hears the touch even when no display owns the device
        }
        self.moved = false;
        if std::mem::take(&mut self.pending_up) {
            if self.latched.is_some() {
                match self.mode {
                    Mode::Pending => {
                        out.events.push(Event::MouseMove(self.start.0, self.start.1));
                        if self.since.is_some_and(|t| t.elapsed() >= MENU_HOLD) {
                            // a press held still: the right-click menu, and no click
                            MENU_BY_TOUCH.store(true, Ordering::SeqCst);
                            out.events.push(Event::MouseRight(self.start.0, self.start.1));
                        } else {
                            // a tap: the click is made now, where the finger landed
                            out.events.push(Event::MouseDown(self.start.0, self.start.1));
                            out.events.push(Event::MouseUp(self.start.0, self.start.1));
                        }
                    }
                    Mode::Mouse => out.events.push(Event::MouseUp(self.last.0, self.last.1)),
                    Mode::Two => {
                        if let Some(p) = self.pair.filter(|p| p.pinched) {
                            out.events.push(Event::PinchEnd { scale_milli: (p.total * 1000.0).round() as u32 });
                        }
                    }
                    Mode::Scroll | Mode::Idle => {}
                }
            }
            self.mode = Mode::Idle;
            self.since = None;
            self.primary = None;
            self.latched = None;
            self.pair = None;
        }
        self.other_moved = false;
        out
    }

    /// A second finger on the screen while the first has sent nothing (or only scrolled): a pinch (the distance between the fingers
    /// changes by a third) is Ctrl+plus or Ctrl+minus, a swipe (their middle moves sideways, the distance stays) is Ctrl+Left or
    /// Ctrl+Right; the app under the fingers decides what those mean (the PDF viewer zooms and turns pages). One gesture sends a
    /// key once and starts measuring again from there, so a long pinch zooms step by step.
    fn two_fingers(&mut self, dev: &TouchDev, rect: &OutRect, out: &mut TouchOut) {
        let down: Vec<usize> = (0..SLOTS).filter(|&i| self.fingers[i].down).collect();
        if down.len() < 2 || self.primary.is_none() {
            return;
        }
        if !matches!(self.mode, Mode::Pending | Mode::Scroll | Mode::Two) {
            return;
        }
        let at = |i: usize| map_point(dev, rect, self.fingers[i].x, self.fingers[i].y);
        let (a, b) = (at(down[0]), at(down[1]));
        let d = (((a.0 - b.0) as f64).powi(2) + ((a.1 - b.1) as f64).powi(2)).sqrt().max(1.0);
        let mid = ((a.0 + b.0) / 2, (a.1 + b.1) / 2);
        if self.mode != Mode::Two {
            self.mode = Mode::Two;
            self.pair = Some(Pair { d0: d, mid0: mid, swiped: false, total: 1.0, pinched: false });
            return;
        }
        if !(self.moved || self.other_moved) {
            return;
        }
        let Some(p) = self.pair.as_mut() else { return };
        let ratio = d / p.d0;
        if ratio >= PINCH_RATIO || ratio <= 1.0 / PINCH_RATIO {
            out.events.push(Event::Key(Key::Ctrl(if ratio > 1.0 { '+' } else { '-' })));
            // the same gesture as a pinch for a tile that asked for it (it ignores the keys): the scale since the fingers landed
            let total = (p.total * ratio).clamp(0.001, 100.0);
            out.events.push(Event::Pinch { scale_milli: (total * 1000.0).round() as u32, x: mid.0, y: mid.1 });
            *p = Pair { d0: d, mid0: mid, swiped: false, total, pinched: true };
        } else if !p.swiped && (mid.0 - p.mid0.0).abs() >= SWIPE_PX && (mid.0 - p.mid0.0).abs() > 2 * (mid.1 - p.mid0.1).abs() {
            // fingers moving left turn to the next page, as a page is pulled across
            let nav = if mid.0 < p.mid0.0 { Nav::Right } else { Nav::Left };
            out.events.push(Event::Key(Key::Nav { nav, shift: false, ctrl: true }));
            p.swiped = true;
        }
    }

    fn frame(&mut self, dev: &TouchDev, target: Option<&OutRect>) -> TouchOut {
        let mut out = TouchOut::default();
        let rect = self.latched.clone().or_else(|| target.cloned());
        if let (Some(p), Some(rect)) = (self.primary, rect) {
            if self.pending_down || self.moved {
                self.last = map_point(dev, &rect, self.fingers[p].x, self.fingers[p].y);
                out.events.push(Event::MouseMove(self.last.0, self.last.1));
            }
        }
        if std::mem::take(&mut self.pending_down) {
            out.down = true; // a calibration hears the touch even when no display owns the device
            if self.latched.is_some() {
                out.events.push(Event::MouseDown(self.last.0, self.last.1));
            }
        }
        self.moved = false;
        if std::mem::take(&mut self.pending_up) {
            if self.latched.is_some() {
                out.events.push(Event::MouseUp(self.last.0, self.last.1));
            }
            self.primary = None;
            self.latched = None;
            // fingers that stayed down do not take over the pointer
        }
        out
    }
}

/// Turn the finger gestures on for the real backends, unless `iyakki.touchscroll=0` is on the kernel command line.
pub fn init_from_cmdline() {
    let cmd = iyakki_core_api::device_types::storage::cmdline_text();
    let off = cmd.split_whitespace().any(|w| w == "iyakki.touchscroll=0");
    enable_gestures_by_default(!off);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touchmap::Rot;

    const SLOT: u16 = 0x2f;
    const X: u16 = 0x35;
    const Y: u16 = 0x36;
    const TID: u16 = 0x39;

    fn dev() -> TouchDev {
        TouchDev { key: "27c0:0859@usb-gesture".into(), id: "27c0:0859".into(), name: "TouchScreen".into(), usb: true, mt: true, x: (0, 32767), y: (0, 32767) }
    }

    fn screen() -> OutRect {
        OutRect { name: "DP-1".into(), x: 0, y: 0, w: 1920, h: 1080, internal: false, rot: Rot::R0 }
    }

    /// Feed the raw events of one or more frames; the events that came out, and whether a finger landed.
    fn feed(t: &mut Touches, evs: &[(u16, u16, i32)]) -> (Vec<Event>, bool) {
        let (d, r) = (dev(), screen());
        let (mut out, mut down) = (Vec::new(), false);
        for (typ, code, value) in evs {
            let o = t.feed(Raw { typ: *typ, code: *code, value: *value }, &d, Some(&r));
            out.extend(o.events);
            down |= o.down;
        }
        (out, down)
    }

    fn land(t: &mut Touches, x: i32, y: i32) -> (Vec<Event>, bool) {
        feed(t, &[(EV_ABS, SLOT, 0), (EV_ABS, TID, 5), (EV_ABS, X, x), (EV_ABS, Y, y), (EV_SYN, 0, 0)])
    }

    fn slide(t: &mut Touches, x: i32, y: i32) -> Vec<Event> {
        feed(t, &[(EV_ABS, X, x), (EV_ABS, Y, y), (EV_SYN, 0, 0)]).0
    }

    fn lift(t: &mut Touches) -> Vec<Event> {
        feed(t, &[(EV_ABS, TID, -1), (EV_SYN, 0, 0)]).0
    }

    fn scrolled(evs: &[Event]) -> i32 {
        evs.iter().map(|e| if let Event::ScrollPx(p) = e { *p } else { 0 }).sum()
    }

    fn gestures() -> Touches {
        let mut t = Touches::default();
        t.set_gestures(true);
        t
    }

    /// A second finger lands in slot 1 (the first stays where it is).
    fn land_second(t: &mut Touches, x: i32, y: i32) -> Vec<Event> {
        feed(t, &[(EV_ABS, SLOT, 1), (EV_ABS, TID, 6), (EV_ABS, X, x), (EV_ABS, Y, y), (EV_SYN, 0, 0)]).0
    }

    fn move_finger(t: &mut Touches, slot: i32, x: i32, y: i32) -> Vec<Event> {
        feed(t, &[(EV_ABS, SLOT, slot), (EV_ABS, X, x), (EV_ABS, Y, y), (EV_SYN, 0, 0)]).0
    }

    fn move_both(t: &mut Touches, a: (i32, i32), b: (i32, i32)) -> Vec<Event> {
        feed(t, &[(EV_ABS, SLOT, 0), (EV_ABS, X, a.0), (EV_ABS, Y, a.1), (EV_ABS, SLOT, 1), (EV_ABS, X, b.0), (EV_ABS, Y, b.1), (EV_SYN, 0, 0)]).0
    }

    fn lift_slot(t: &mut Touches, slot: i32) -> Vec<Event> {
        feed(t, &[(EV_ABS, SLOT, slot), (EV_ABS, TID, -1), (EV_SYN, 0, 0)]).0
    }

    fn keys(evs: &[Event]) -> Vec<Key> {
        evs.iter().filter_map(|e| if let Event::Key(k) = e { Some(k.clone()) } else { None }).collect()
    }

    #[test]
    fn two_fingers_moving_apart_or_together_are_a_pinch_and_click_nothing() {
        let mut t = gestures();
        land(&mut t, 14000, 16384);
        assert!(land_second(&mut t, 18000, 16384).is_empty());
        // out: the second finger goes much further away
        let out = move_finger(&mut t, 1, 26000, 16384);
        assert_eq!(keys(&out), vec![Key::Ctrl('+')], "{out:?}");
        assert!(out.iter().all(|e| !matches!(e, Event::MouseDown(..) | Event::MouseUp(..) | Event::ScrollPx(_))), "{out:?}");
        // a small move is nothing; coming back in is the other way
        assert!(keys(&move_finger(&mut t, 1, 27000, 16384)).is_empty());
        let back = move_finger(&mut t, 1, 15500, 16384);
        assert_eq!(keys(&back), vec![Key::Ctrl('-')], "{back:?}");
        // lifting both sends no click and no menu
        let lifted: Vec<Event> = lift_slot(&mut t, 1).into_iter().chain(lift_slot(&mut t, 0)).collect();
        assert!(lifted.iter().all(|e| matches!(e, Event::PinchEnd { .. })), "only the end of the pinch is told: {lifted:?}");
        // the next single touch is a tap again
        land(&mut t, 100, 100);
        assert_eq!(lift(&mut t).len(), 3);
    }

    fn swipes(evs: &[Event]) -> Vec<(bool, Option<(i32, i32)>)> {
        evs.iter().filter_map(|e| if let Event::Swipe { back, at } = e { Some((*back, *at)) } else { None }).collect()
    }

    #[test]
    fn one_finger_swiping_sideways_sends_one_swipe_with_its_starting_point_and_a_vertical_drag_sends_none() {
        let mut t = gestures();
        land(&mut t, 20000, 16384);
        let mut all = Vec::new();
        for i in 1..=8 {
            all.extend(slide(&mut t, 20000 - i * 1000, 16384 + i * 40)); // about 235 px left in total
        }
        let got = swipes(&all);
        assert_eq!(got.len(), 1, "{all:?}");
        assert!(!got[0].0 && got[0].1.is_some(), "fingers moving left: forward, from where it landed");
        assert!(lift(&mut t).iter().all(|e| !matches!(e, Event::MouseDown(..))), "no click after a swipe");
        // to the right
        land(&mut t, 8000, 16384);
        let all: Vec<Event> = (1..=8).flat_map(|i| slide(&mut t, 8000 + i * 1000, 16384)).collect();
        assert_eq!(swipes(&all).iter().map(|s| s.0).collect::<Vec<_>>(), vec![true]);
        lift(&mut t);
        // a vertical-dominant drag scrolls and never swipes, even with some drift sideways
        land(&mut t, 16384, 8000);
        let all: Vec<Event> = (1..=8).flat_map(|i| slide(&mut t, 16384 + i * 300, 8000 + i * 1500)).collect();
        assert!(swipes(&all).is_empty() && scrolled(&all) > 0, "{all:?}");
    }

    #[test]
    fn a_pinch_is_also_told_as_a_scale_since_the_fingers_landed_and_its_end_once() {
        let mut t = gestures();
        land(&mut t, 14000, 16384);
        land_second(&mut t, 18000, 16384);
        // the fingers go from 4000 to 8000 apart (a bit over 1.3 times), about 2.0 in all
        let out = move_both(&mut t, (12000, 16384), (20000, 16384));
        let pinches: Vec<(u32, i32)> = out.iter().filter_map(|e| if let Event::Pinch { scale_milli, x, .. } = e { Some((*scale_milli, *x)) } else { None }).collect();
        assert_eq!(pinches.len(), 1, "{out:?}");
        assert_eq!(pinches[0].0, 2000);
        assert_eq!(keys(&out), vec![Key::Ctrl('+')], "the key of the old gesture is still sent");
        // a second step builds on the first: from 8000 apart to 16000 is 2.0 again, 4.0 in all
        let out = move_both(&mut t, (8000, 16384), (24000, 16384));
        assert!(out.iter().any(|e| matches!(e, Event::Pinch { scale_milli: 3990..=4010, .. })), "{out:?}");
        let up = lift_slot(&mut t, 0);
        assert!(up.iter().filter(|e| matches!(e, Event::PinchEnd { scale_milli: 3990..=4010 })).count() <= 1);
        let rest = lift_slot(&mut t, 1);
        let ends = up.iter().chain(rest.iter()).filter(|e| matches!(e, Event::PinchEnd { scale_milli: 3990..=4010 })).count();
        assert_eq!(ends, 1, "the end is told once: {up:?} {rest:?}");
        // a swipe or a plain tap sends no pinch
        let mut t = gestures();
        land(&mut t, 20000, 16384);
        land_second(&mut t, 24000, 16384);
        let out = move_both(&mut t, (15000, 16384), (19000, 16384));
        assert!(!out.iter().any(|e| matches!(e, Event::Pinch { .. } | Event::PinchEnd { .. })));
        let rest: Vec<Event> = lift_slot(&mut t, 1).into_iter().chain(lift_slot(&mut t, 0)).collect();
        assert!(!rest.iter().any(|e| matches!(e, Event::PinchEnd { .. })));
    }

    #[test]
    fn two_fingers_sliding_sideways_together_are_a_swipe_once_per_gesture() {
        let mut t = gestures();
        land(&mut t, 20000, 16384);
        land_second(&mut t, 24000, 16384);
        // both go left by about 330 pixels of the 1920 wide screen
        let out = move_both(&mut t, (15000, 16384), (19000, 16384));
        assert_eq!(keys(&out), vec![Key::Nav { nav: Nav::Right, shift: false, ctrl: true }], "{out:?}");
        assert!(keys(&move_both(&mut t, (13000, 16384), (17000, 16384))).is_empty(), "one swipe is one key");
        lift_slot(&mut t, 1);
        lift_slot(&mut t, 0);
        // the other way
        land(&mut t, 8000, 16384);
        land_second(&mut t, 12000, 16384);
        let out = move_both(&mut t, (15000, 16384), (19000, 16384));
        assert_eq!(keys(&out), vec![Key::Nav { nav: Nav::Left, shift: false, ctrl: true }], "{out:?}");
    }

    #[test]
    fn a_second_finger_after_a_scroll_began_or_down_the_page_is_not_a_scroll_and_one_finger_is_unchanged() {
        let mut t = gestures();
        land(&mut t, 16384, 8000);
        slide(&mut t, 16384, 12000);
        land_second(&mut t, 20000, 12000);
        let out = move_finger(&mut t, 0, 16384, 16000);
        assert_eq!(scrolled(&out), 0, "the pair does not scroll: {out:?}");
        lift_slot(&mut t, 1);
        lift_slot(&mut t, 0);
        // without gestures the second finger changes nothing
        let mut m = Touches::default();
        m.set_gestures(false);
        land(&mut m, 14000, 16384);
        assert!(keys(&land_second(&mut m, 18000, 16384)).is_empty());
        assert!(keys(&move_finger(&mut m, 1, 28000, 16384)).is_empty());
    }

    #[test]
    fn a_tap_clicks_where_the_finger_landed_when_it_lifts() {
        let mut t = gestures();
        let (landed, down) = land(&mut t, 16384, 16384);
        assert!(landed.is_empty() && down, "nothing is pressed yet, but the calibration hears the touch: {landed:?}");
        // a wobble inside the slop is still a tap
        assert!(slide(&mut t, 16400, 16390).is_empty());
        let (x, y) = (960, 540);
        let up = lift(&mut t);
        assert_eq!(up.len(), 3, "{up:?}");
        assert!(matches!(up[1], Event::MouseDown(a, b) if (a - x).abs() <= 1 && (b - y).abs() <= 1), "{up:?}");
        assert!(matches!(up[2], Event::MouseUp(a, b) if (a - x).abs() <= 1 && (b - y).abs() <= 1), "{up:?}");
    }

    #[test]
    fn a_quick_drag_scrolls_with_the_finger_and_never_presses() {
        let mut t = gestures();
        land(&mut t, 16384, 8000);
        // 9830 raw units are about 324 pixels of the 1080 high screen; the finger moves down, the page follows it
        let down = slide(&mut t, 16384, 8000 + 9830);
        assert!(down.iter().all(|e| !matches!(e, Event::MouseDown(..) | Event::MouseUp(..))), "{down:?}");
        let got = scrolled(&down);
        assert!((got - 324).abs() <= 3, "scrolled {got}");
        // and back up: negative
        let up = slide(&mut t, 16384, 8000);
        assert!(scrolled(&up) < -300, "{up:?}");
        assert!(lift(&mut t).is_empty(), "lifting a scroll clicks nothing");
        // the next touch starts fresh: a tap again
        land(&mut t, 100, 100);
        assert_eq!(lift(&mut t).len(), 3);
    }

    #[test]
    fn a_finger_held_before_it_moves_drags_like_a_mouse() {
        let mut t = gestures();
        land(&mut t, 2000, 2000);
        std::thread::sleep(LONG_PRESS + Duration::from_millis(60));
        let moved = slide(&mut t, 12000, 2000);
        assert!(matches!(moved.first(), Some(Event::MouseMove(..))) && moved.iter().any(|e| matches!(e, Event::MouseDown(..))), "{moved:?}");
        assert_eq!(scrolled(&moved), 0, "a held drag does not scroll");
        assert!(matches!(moved.last(), Some(Event::MouseMove(x, _)) if *x > 300));
        assert!(matches!(lift(&mut t).as_slice(), [Event::MouseUp(..)]));
    }

    #[test]
    fn a_device_that_goes_away_mid_scroll_sends_no_release_but_mid_drag_it_does() {
        let mut t = gestures();
        land(&mut t, 2000, 2000);
        slide(&mut t, 2000, 12000);
        assert!(t.gone(&dev().key).is_empty(), "nothing was pressed");
        let mut t = gestures();
        land(&mut t, 2000, 2000);
        std::thread::sleep(LONG_PRESS + Duration::from_millis(60));
        slide(&mut t, 12000, 2000);
        assert!(matches!(t.gone(&dev().key).as_slice(), [Event::MouseUp(..)]));
    }

    #[test]
    fn a_finger_held_still_opens_the_context_menu_on_release_and_does_not_click() {
        let mut t = gestures();
        land(&mut t, 16384, 16384);
        std::thread::sleep(MENU_HOLD + Duration::from_millis(60));
        assert!(slide(&mut t, 16390, 16390).is_empty(), "a wobble inside the slop changes nothing");
        let up = lift(&mut t);
        assert!(matches!(up.as_slice(), [Event::MouseMove(..), Event::MouseRight(x, y)] if (*x - 960).abs() <= 1 && (*y - 540).abs() <= 1), "{up:?}");
        assert!(take_touch_menu() && !take_touch_menu(), "the shell is told once that a finger asked");
        // the next touch is a plain tap again
        land(&mut t, 100, 100);
        assert!(matches!(lift(&mut t).as_slice(), [Event::MouseMove(..), Event::MouseDown(..), Event::MouseUp(..)]));
    }

    #[test]
    fn a_hold_longer_than_the_drag_time_but_shorter_than_the_menu_time_still_clicks() {
        let mut t = gestures();
        land(&mut t, 16384, 16384);
        std::thread::sleep(LONG_PRESS + Duration::from_millis(40));
        assert_eq!(lift(&mut t).len(), 3);
    }

    #[test]
    fn a_hold_then_a_move_is_a_drag_not_a_menu() {
        let mut t = gestures();
        land(&mut t, 2000, 2000);
        std::thread::sleep(MENU_HOLD + Duration::from_millis(60));
        let moved = slide(&mut t, 12000, 2000);
        assert!(moved.iter().any(|e| matches!(e, Event::MouseDown(..))), "{moved:?}");
        let up = lift(&mut t);
        assert!(matches!(up.as_slice(), [Event::MouseUp(..)]), "{up:?}");
    }

    #[test]
    fn without_gestures_a_finger_is_a_mouse_as_before() {
        let mut t = Touches::default();
        t.set_gestures(false);
        let (landed, _) = land(&mut t, 16384, 16384);
        assert!(matches!(landed.as_slice(), [Event::MouseMove(..), Event::MouseDown(..)]), "{landed:?}");
        let moved = slide(&mut t, 16384, 26000);
        assert!(matches!(moved.as_slice(), [Event::MouseMove(..)]) && scrolled(&moved) == 0);
    }
}
