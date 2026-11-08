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
use crate::ui::Event;

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
}

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
        iyakki_agent::touchrecover::note_event();
        let t = self.one(&dev.key);
        match r.typ {
            EV_ABS if dev.mt => match r.code {
                ABS_MT_SLOT => t.slot = (r.value.max(0) as usize).min(SLOTS - 1),
                ABS_MT_TRACKING_ID if r.value >= 0 => {
                    iyakki_agent::touchprobe::note_finger();
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
                    iyakki_agent::touchprobe::note_finger();
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
        }
    }

    /// [`One::frame`] with gestures: the press is not sent when the finger lands. A finger that lifts without moving far clicks where
    /// it landed; one that moves at once scrolls (the page follows the finger); one held for a moment and then moved is a mouse drag.
    fn frame_gestures(&mut self, dev: &TouchDev, target: Option<&OutRect>) -> TouchOut {
        let mut out = TouchOut::default();
        let rect = self.latched.clone().or_else(|| target.cloned());
        if let (Some(p), Some(rect)) = (self.primary, rect) {
            let at = map_point(dev, &rect, self.fingers[p].x, self.fingers[p].y);
            if self.pending_down {
                self.mode = Mode::Pending;
                self.start = at;
                self.since = Some(Instant::now());
                self.last = at;
            } else if self.moved {
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
                    Mode::Scroll | Mode::Idle => {}
                }
            }
            self.mode = Mode::Idle;
            self.since = None;
            self.primary = None;
            self.latched = None;
        }
        out
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
    let cmd = iyakki_storage::cmdline_text();
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
