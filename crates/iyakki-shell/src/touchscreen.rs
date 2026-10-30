//! Fingers on a touch screen: raw events of one device in, pointer events on the display it belongs to out.
//!
//! One [`Touches`] holds the state of every touch device (by key): the multi-touch slots, which finger drives the pointer (the
//! first one down; the others are followed but do not move it) and the display rectangle a touch started on, which stays
//! with it until the finger lifts even when the mapping changes meanwhile. Positions are desktop positions
//! ([`crate::touchmap::map_point`]); the caller turns them into display-local ones as for a mouse.

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
}

#[derive(Default)]
pub struct Touches {
    devs: Vec<(String, One)>,
}

/// What a frame of one device did.
#[derive(Default, Debug, PartialEq)]
pub struct TouchOut {
    pub events: Vec<Event>,
    /// A finger came down in this frame (the calibration listens for it).
    pub down: bool,
}

impl Touches {
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
        if t.primary.is_some() {
            vec![Event::MouseUp(t.last.0, t.last.1)]
        } else {
            Vec::new()
        }
    }

    pub fn feed(&mut self, r: Raw, dev: &TouchDev, target: Option<&OutRect>) -> TouchOut {
        let t = self.one(&dev.key);
        match r.typ {
            EV_ABS if dev.mt => match r.code {
                ABS_MT_SLOT => t.slot = (r.value.max(0) as usize).min(SLOTS - 1),
                ABS_MT_TRACKING_ID if r.value >= 0 => {
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
                    t.fingers[0].down = true;
                    t.begin(0, target);
                } else if r.value == 0 {
                    t.fingers[0].down = false;
                    t.end(0);
                }
            }
            EV_SYN if r.code == 0 => return t.frame(dev, target),
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
