//! Touchpads: multi-touch positions in, pointer movement, clicks and scrolling out.
//!
//! A touchpad (the Apple SPI one of a 2016-2017 MacBook, I2C and USB pads of other laptops) does not move a pointer by itself: it
//! reports where each finger is (`ABS_MT_POSITION_X/Y` per slot, `ABS_MT_TRACKING_ID`, the contact size `ABS_MT_TOUCH_MAJOR`)
//! and the physical click as `BTN_LEFT`. This module turns that into what a person expects:
//!
//! * one finger moves the pointer, with a little acceleration (slow = precise, fast = far);
//! * a short single-finger tap is a left click (when tap-to-click is on), a short two-finger tap a right click;
//! * two fingers moving up or down scroll (natural direction optional);
//! * the physical click is a left click, with two fingers down a right click;
//! * contacts larger than half of the pad's contact-size range are a palm and ignored;
//! * the first 40 ms of a touch do nothing (a finger settling, a brush of the pad), and a touch that ends within them is no tap.
//!
//! Not done: tap-and-drag, three-finger gestures, edge scrolling. The code is a pure state machine fed with raw events and a clock
//! in milliseconds, so the tests below replay synthetic sequences. Real values (contact size of a finger on the 14,1 pad, how
//! fast the feel is) need the hardware; the constants are at the top.

use crate::input::{Raw, EV_ABS, EV_KEY, EV_SYN};

/// A touch shorter than this is ignored; pointer movement starts after it.
pub const MIN_TOUCH_MS: u64 = 40;
/// A tap is a touch no longer than this.
pub const TAP_MAX_MS: u64 = 220;
/// A touch that wandered farther than this (fraction of the pad's width) is not a tap.
pub const TAP_SLOP: f32 = 0.03;
/// Two-finger travel (fraction of the pad's width) per wheel notch.
pub const SCROLL_STEP: f32 = 0.05;
/// Two-finger travel (a share of the pad's width) before scrolling starts: two resting fingers wobble a little.
pub const SCROLL_SLOP: f32 = 0.004;
/// Contact size above this share of the axis maximum is a palm.
pub const PALM_SHARE: f32 = 0.5;
/// Pointer travel for a slow swipe across the whole pad, as a share of the screen width (acceleration comes on top).
pub const BASE_GAIN: f32 = 0.7;

/// Bottom-right corner of the pad (share of width from the left, of height from the top) where a physical click is a right click.
pub const CORNER_X: f32 = 0.75;
pub const CORNER_Y: f32 = 0.7;

const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const BTN_LEFT: u16 = 0x110;
const SYN_REPORT: u16 = 0;
const SLOTS: usize = 16;

/// The pad's axis ranges, read from the device when it is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadRange {
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
    /// Maximum of `ABS_MT_TOUCH_MAJOR` (0 = the pad has none: no palm detection).
    pub major_max: i32,
}

impl PadRange {
    fn width(&self) -> f32 {
        (self.x_max - self.x_min).max(1) as f32
    }
}

/// The user's choices (Settings -> Keyboard). The scrolling direction and speed are the scroll engine's (`scrolleng.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadConfig {
    pub tap_to_click: bool,
}

impl Default for PadConfig {
    fn default() -> PadConfig {
        PadConfig { tap_to_click: true }
    }
}

/// What the pad wants done.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadOut {
    /// Pointer movement in pixels.
    Move(i32, i32),
    LeftDown,
    LeftUp,
    RightClick,
    /// Two-finger travel since the last frame, as a share of the pad's width, positive = fingers moved down the pad (the scroll
    /// engine turns it into pixels, with the direction and speed chosen).
    Scroll(f32),
    /// The scrolling fingers lifted (or only one is left): momentum may start.
    ScrollEnd,
}

#[derive(Clone, Copy)]
struct Slot {
    id: i32,
    x: i32,
    y: i32,
    major: i32,
    /// Where this contact was last frame / when it landed (same tracking id only).
    prev: Option<(i32, i32)>,
    start: Option<(i32, i32)>,
}

const EMPTY: Slot = Slot { id: -1, x: 0, y: 0, major: 0, prev: None, start: None };

pub struct Touchpad {
    slots: [Slot; SLOTS],
    cur: usize,
    /// Gesture in progress: when the first finger landed.
    down_ms: Option<u64>,
    max_fingers: usize,
    moved: bool,
    clicked: bool,
    /// The physical click was a right click (its release is swallowed).
    click_right: bool,
    button_down: bool,
    last_ms: u64,
    rem: (f32, f32),
    /// Two fingers have been scrolling (so a lift ends the scroll).
    scrolling: bool,
    /// Two-finger travel not yet sent while it is still under [`SCROLL_SLOP`] (a resting pair of fingers wobbles), and its total.
    scroll_pend: f32,
    scroll_travel: f32,
}

impl Default for Touchpad {
    fn default() -> Self {
        Touchpad::new()
    }
}

impl Touchpad {
    pub fn new() -> Touchpad {
        Touchpad { slots: [EMPTY; SLOTS], cur: 0, down_ms: None, max_fingers: 0, moved: false, clicked: false, click_right: false, button_down: false, last_ms: 0, rem: (0.0, 0.0), scrolling: false, scroll_pend: 0.0, scroll_travel: 0.0 }
    }

    /// One raw event at `now_ms` (any monotonic clock). `screen_w` is the width of the screen in pixels. Output comes at `SYN_REPORT`
    /// (the end of a frame) and for the button.
    pub fn feed(&mut self, r: Raw, now_ms: u64, range: &PadRange, cfg: &PadConfig, screen_w: i32) -> Vec<PadOut> {
        let mut out = Vec::new();
        match (r.typ, r.code) {
            (EV_ABS, ABS_MT_SLOT) => self.cur = (r.value.max(0) as usize).min(SLOTS - 1),
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                let s = &mut self.slots[self.cur];
                if r.value < 0 {
                    *s = EMPTY;
                } else if s.id != r.value {
                    *s = Slot { id: r.value, ..EMPTY }; // a new contact (the kernel repeats an id only when it changes)
                }
            }
            (EV_ABS, ABS_MT_POSITION_X) => self.slots[self.cur].x = r.value,
            (EV_ABS, ABS_MT_POSITION_Y) => self.slots[self.cur].y = r.value,
            (EV_ABS, ABS_MT_TOUCH_MAJOR) => self.slots[self.cur].major = r.value,
            (EV_KEY, BTN_LEFT) => self.button(r.value, range, &mut out),
            (EV_SYN, SYN_REPORT) => self.frame(now_ms, range, cfg, screen_w, &mut out),
            _ => {}
        }
        out
    }

    fn active(&self, range: &PadRange) -> Vec<usize> {
        let palm = (range.major_max as f32 * PALM_SHARE) as i32;
        (0..SLOTS).filter(|&i| self.slots[i].id >= 0 && !(range.major_max > 0 && self.slots[i].major > palm)).collect()
    }

    fn button(&mut self, value: i32, range: &PadRange, out: &mut Vec<PadOut>) {
        // a pad whose click is a button under the surface: the physical press is a click whatever the fingers do
        if value == 1 {
            let fingers = self.slots.iter().filter(|s| s.id >= 0).count();
            self.clicked = true;
            self.button_down = true;
            // a clickpad (the whole surface is the button): one finger resting in the bottom-right corner clicks "right"
            let corner = self.slots.iter().filter(|s| s.id >= 0).any(|s| {
                (s.x - range.x_min) as f32 > CORNER_X * range.width() && (s.y - range.y_min) as f32 > CORNER_Y * (range.y_max - range.y_min).max(1) as f32
            });
            if fingers >= 2 || (fingers == 1 && corner) {
                self.click_right = true;
                out.push(PadOut::RightClick);
            } else {
                self.click_right = false;
                out.push(PadOut::LeftDown);
            }
        } else if value == 0 && self.button_down {
            self.button_down = false;
            if !self.click_right {
                out.push(PadOut::LeftUp);
            }
            self.click_right = false;
        }
    }

    fn frame(&mut self, now: u64, range: &PadRange, cfg: &PadConfig, screen_w: i32, out: &mut Vec<PadOut>) {
        let act = self.active(range);
        let dt_ms = now.saturating_sub(self.last_ms).clamp(1, 200);
        self.last_ms = now;
        if act.is_empty() {
            if let Some(t0) = self.down_ms.take() {
                let dur = now.saturating_sub(t0);
                if cfg.tap_to_click && !self.clicked && !self.moved && (MIN_TOUCH_MS..=TAP_MAX_MS).contains(&dur) {
                    match self.max_fingers {
                        1 => out.extend([PadOut::LeftDown, PadOut::LeftUp]),
                        2 => out.push(PadOut::RightClick),
                        _ => {}
                    }
                }
            }
            self.max_fingers = 0;
            self.moved = false;
            self.clicked = false;
            self.rem = (0.0, 0.0);
            (self.scroll_pend, self.scroll_travel) = (0.0, 0.0);
            if std::mem::take(&mut self.scrolling) {
                out.push(PadOut::ScrollEnd);
            }
            for s in self.slots.iter_mut() {
                s.prev = None;
                s.start = None;
            }
            return;
        }
        let t0 = *self.down_ms.get_or_insert(now);
        self.max_fingers = self.max_fingers.max(act.len());
        if act.len() != 2 {
            (self.scroll_pend, self.scroll_travel) = (0.0, 0.0);
            if std::mem::take(&mut self.scrolling) {
                out.push(PadOut::ScrollEnd);
            }
        }
        let w = range.width();
        // movement of each active contact since the last frame; a contact seen for the first time has none
        let mut deltas = Vec::new();
        for &i in &act {
            let s = &mut self.slots[i];
            let pos = (s.x, s.y);
            let start = *s.start.get_or_insert(pos);
            let travel = (((pos.0 - start.0) as f32).hypot((pos.1 - start.1) as f32)) / w;
            if travel > TAP_SLOP {
                self.moved = true;
            }
            if let Some(p) = s.prev {
                deltas.push(((pos.0 - p.0) as f32, (pos.1 - p.1) as f32));
            }
            s.prev = Some(pos);
        }
        if now.saturating_sub(t0) < MIN_TOUCH_MS || deltas.len() != act.len() {
            return; // settling, or a finger just landed or lifted (its first frame has no delta)
        }
        match act.len() {
            1 => {
                let (dx, dy) = (deltas[0].0 / w, deltas[0].1 / w);
                let speed = dx.hypot(dy) / (dt_ms as f32 / 1000.0); // pad widths per second
                let gain = BASE_GAIN + speed.min(2.0) * 0.9;
                let k = gain * screen_w.max(1) as f32;
                self.rem.0 += dx * k;
                self.rem.1 += dy * k;
                let (px, py) = (self.rem.0.trunc(), self.rem.1.trunc());
                self.rem.0 -= px;
                self.rem.1 -= py;
                if px != 0.0 || py != 0.0 {
                    out.push(PadOut::Move(px as i32, py as i32));
                }
            }
            2 => {
                let dy = (deltas[0].1 + deltas[1].1) / 2.0 / w;
                self.scroll_pend += dy;
                self.scroll_travel += dy.abs();
                if self.scroll_travel >= SCROLL_SLOP {
                    self.scrolling = true;
                    out.push(PadOut::Scroll(std::mem::take(&mut self.scroll_pend)));
                }
            }
            _ => {}
        }
    }
}

/// Kernel capability words as `/proc/bus/input/devices` prints them (`B: KEY=e520 10000 0 0 0 0`): 64-bit hex words, the highest
/// first, separated by spaces. Returns the bit set as little-endian bytes like `EVIOCGBIT` does.
pub fn bits_from_proc(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for w in text.split_whitespace().rev() {
        let v = u64::from_str_radix(w, 16).unwrap_or(0);
        out.extend(v.to_le_bytes());
    }
    out
}

fn bit(bits: &[u8], n: u16) -> bool {
    bits.get(n as usize / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

/// How a device with these capabilities is used: a multi-touch touchpad (finger tools, multi-touch position, not a touch screen
/// (`INPUT_PROP_DIRECT`, property bit 1), not a keyboard) gets the gesture code; anything else keeps its old handling.
pub fn is_touchpad(key_bits: &[u8], prop_bits: &[u8], abs_bits: &[u8]) -> bool {
    let finger = bit(key_bits, 0x145);
    let keyboard = bit(key_bits, 30) && bit(key_bits, 28);
    let direct = bit(prop_bits, 1);
    finger && !direct && !keyboard && bit(abs_bits, ABS_MT_POSITION_X) && bit(abs_bits, ABS_MT_POSITION_Y)
}

#[cfg(test)]
#[path = "touchpad_tests.rs"]
mod tests;
