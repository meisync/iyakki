//! The scroll engine: raw wheel and trackpad movement in, smooth pixel steps out, one place for every tile and the browser.
//!
//! What it does, in pixels of a nominal screen ([`LINE_PX`] a line, [`NOTCH_PX`] the three lines of a classic wheel click):
//!
//! * a mouse wheel click (or a hi-res wheel's fraction of one) adds `lines per click * LINE_PX * speed` to a target, and the engine
//!   walks towards the target with an exponential ease (about 30 ms time constant: 95 % in 90 ms, 99 % in 140 ms), at the frame
//!   rate. The steps add up to exactly the target and never pass it, so a list that clamps at its ends is never overshot;
//! * a trackpad's two-finger travel is already smooth: it goes out at once (times the speed), and when the fingers lift the speed
//!   they had decays exponentially (momentum) until it is tiny, a new touch or the end of the list stops it;
//! * direction is chosen per device kind (the wheel is classic by default, the trackpad natural).
//!
//! The engine is pure: the caller feeds events and a clock in milliseconds and asks [`ScrollEngine::tick`] for the whole pixels to
//! move now (positive = up, towards the start, like [`crate::ui::Event::Scroll`]). It lives in the input thread
//! (`input.rs`); the shell turns the pixels into what each tile understands (`shell_scroll.rs`).

use std::collections::VecDeque;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(test))]
use std::sync::Mutex;

/// One text line of a nominal screen, in pixels.
pub const LINE_PX: f32 = 18.0;
/// A classic wheel click: three lines.
pub const NOTCH_PX: f32 = 3.0 * LINE_PX;
/// How far a page of the shell moves on screen for one wheel click, in pixels at UI scale 1 (Settings' lists and the chat use it; the
/// browser's page uses it too, divided by its zoom, so every tile scrolls the same distance per click).
pub const SCREEN_NOTCH_PX: f32 = 40.0;
/// Trackpad travel (a share of the pad's width) that equals one wheel click at 100 %.
pub const PAD_STEP: f32 = 0.05;
/// How often steps are made while a scroll is going on (60 frames a second).
pub const FRAME_MS: u64 = 16;
/// Time constant of the ease, in milliseconds.
pub const EASE_MS: f32 = 30.0;
/// Time constant of the momentum's decay, in milliseconds.
pub const MOMENTUM_MS: f32 = 325.0;
/// A lift slower than this (pixels a second) does not start momentum; momentum ends below [`STOP_SPEED`].
pub const FLING_SPEED: f32 = 150.0;
pub const STOP_SPEED: f32 = 20.0;
/// The fingers' speed is read from this much of the last movement before the lift.
const VELOCITY_WINDOW_MS: u64 = 90;
/// More than this much waiting to be scrolled is dropped (a runaway wheel must not scroll for seconds after it stopped).
const MAX_QUEUE_PX: f32 = 6000.0;

/// What the person chose (Settings -> Keyboard & pointer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollConfig {
    /// 50 to 300.
    pub speed_pct: u32,
    pub wheel_natural: bool,
    pub pad_natural: bool,
    pub smooth: bool,
    pub momentum: bool,
    /// Lines a wheel click moves, 1 to 10.
    pub wheel_lines: u32,
}

impl Default for ScrollConfig {
    fn default() -> ScrollConfig {
        ScrollConfig { speed_pct: 100, wheel_natural: false, pad_natural: true, smooth: true, momentum: true, wheel_lines: 3 }
    }
}

impl ScrollConfig {
    fn speed(&self) -> f32 {
        self.speed_pct.clamp(50, 300) as f32 / 100.0
    }
}

impl From<iyakki_agent::keyboard::ScrollPrefs> for ScrollConfig {
    fn from(p: iyakki_agent::keyboard::ScrollPrefs) -> ScrollConfig {
        ScrollConfig { speed_pct: p.speed_pct, wheel_natural: p.wheel_natural, pad_natural: p.pad_natural, smooth: p.smooth, momentum: p.momentum, wheel_lines: p.wheel_lines }
    }
}

#[derive(Debug, Default)]
pub struct ScrollEngine {
    /// Wheel movement still to be walked, eased (pixels, positive = up).
    remain: f32,
    /// Movement to go out at the next tick as it is (trackpad, or a wheel without smoothing).
    direct: f32,
    /// The part of a pixel not yet sent.
    carry: f32,
    /// Fingers on the pad now, and the last movements (time, pixels) for the speed at the lift.
    touching: bool,
    samples: VecDeque<(u64, f32)>,
    /// Momentum speed (pixels a second, positive = up).
    vel: f32,
    last: Option<u64>,
}

impl ScrollEngine {
    pub fn new() -> ScrollEngine {
        ScrollEngine::default()
    }

    /// A wheel moved by `v120` (a click is 120, a hi-res wheel sends fractions of it; positive = rolled away from the person).
    pub fn wheel(&mut self, v120: i32, cfg: &ScrollConfig) {
        if v120 == 0 {
            return;
        }
        let sign = if cfg.wheel_natural { -1.0 } else { 1.0 };
        let px = sign * v120 as f32 / 120.0 * cfg.wheel_lines.clamp(1, 10) as f32 * LINE_PX * cfg.speed();
        self.vel = 0.0;
        if cfg.smooth {
            self.remain = (self.remain + px).clamp(-MAX_QUEUE_PX, MAX_QUEUE_PX);
        } else {
            self.direct += px;
        }
    }

    /// Two fingers moved by `d` pad widths (positive = down the pad).
    pub fn pad(&mut self, d: f32, now_ms: u64, cfg: &ScrollConfig) {
        if d == 0.0 {
            return;
        }
        let px = (if cfg.pad_natural { d } else { -d }) * (NOTCH_PX / PAD_STEP) * cfg.speed();
        self.touching = true;
        self.vel = 0.0;
        self.direct += px;
        self.samples.push_back((now_ms, px));
        while self.samples.front().is_some_and(|(t, _)| now_ms.saturating_sub(*t) > VELOCITY_WINDOW_MS) {
            self.samples.pop_front();
        }
    }

    /// A finger touched the pad: momentum stops at once.
    pub fn touch(&mut self) {
        self.vel = 0.0;
        if self.remain == 0.0 && self.direct == 0.0 {
            self.carry = 0.0;
        }
    }

    /// The scrolling fingers lifted (or one of them did): momentum starts from the speed they had.
    pub fn pad_end(&mut self, now_ms: u64, cfg: &ScrollConfig) {
        self.touching = false;
        let recent: Vec<_> = self.samples.drain(..).filter(|(t, _)| now_ms.saturating_sub(*t) <= VELOCITY_WINDOW_MS).collect();
        if !cfg.momentum || recent.len() < 2 {
            return;
        }
        let span = recent.last().map_or(0, |l| l.0).saturating_sub(recent[0].0).max(FRAME_MS);
        // the first sample's movement happened before the window began: leave it out of the sum
        let sum: f32 = recent.iter().skip(1).map(|(_, p)| p).sum();
        let v = sum / span as f32 * 1000.0;
        self.vel = if v.abs() >= FLING_SPEED { v } else { 0.0 };
    }

    /// The tile reached the end of its content: nothing more is wanted in that direction.
    pub fn bound(&mut self) {
        self.remain = 0.0;
        self.vel = 0.0;
        self.carry = 0.0;
    }

    /// Something is still going on: the caller should keep asking for steps every [`FRAME_MS`] (and the shell for frames).
    pub fn wants_frames(&self) -> bool {
        self.remain != 0.0 || self.direct != 0.0 || self.vel != 0.0
    }

    /// The whole pixels to move now (positive = up). With `raw` the wheel's movement is not eased (the browser's engine makes its
    /// own pictures: easing twice would blur it).
    pub fn tick(&mut self, now_ms: u64, raw: bool) -> i32 {
        let dt = self.last.map_or(FRAME_MS, |l| now_ms.saturating_sub(l)).clamp(1, 100) as f32;
        let mut out = std::mem::take(&mut self.direct);
        if self.remain != 0.0 {
            if raw {
                out += std::mem::take(&mut self.remain);
            } else {
                let step = self.remain * (1.0 - (-dt / EASE_MS).exp());
                self.remain -= step;
                out += step;
                if self.remain.abs() < 0.5 {
                    out += std::mem::take(&mut self.remain); // the last half pixel: settled
                }
            }
        }
        if self.vel != 0.0 && !self.touching {
            out += self.vel * dt / 1000.0;
            self.vel *= (-dt / MOMENTUM_MS).exp();
            if self.vel.abs() < STOP_SPEED {
                self.vel = 0.0;
            }
        }
        self.carry += out;
        self.last = self.wants_frames().then_some(now_ms);
        // settled: the last fraction goes out too, so the steps add up to the target and the next scroll starts clean
        let whole = if self.wants_frames() { self.carry.trunc() } else { self.carry.round() };
        self.carry = if self.wants_frames() { self.carry - whole } else { 0.0 };
        whole as i32
    }
}

// ------------------------------------------------------------------ what the rest of the shell tells the input thread

// In the program these are shared with the input thread; in tests each test thread has its own, so tests do not disturb each other.
#[cfg(not(test))]
static CONFIG: Mutex<Option<ScrollConfig>> = Mutex::new(None);
#[cfg(not(test))]
static RAW: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static BOUND: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static CONFIG: std::cell::Cell<Option<ScrollConfig>> = const { std::cell::Cell::new(None) };
    static RAW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BOUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The person's choices, for every scroll from now on (Settings and the ten-second sync of the saved settings call this).
pub fn request_config(c: ScrollConfig) {
    #[cfg(not(test))]
    {
        *CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = Some(c);
    }
    #[cfg(test)]
    CONFIG.with(|v| v.set(Some(c)));
}

pub fn config() -> ScrollConfig {
    #[cfg(not(test))]
    {
        CONFIG.lock().unwrap_or_else(|p| p.into_inner()).unwrap_or_default()
    }
    #[cfg(test)]
    CONFIG.with(|v| v.get()).unwrap_or_default()
}

/// The open tile makes its own smooth pictures (the browser's engine): the wheel's steps go out without easing.
pub fn set_raw(on: bool) {
    #[cfg(not(test))]
    RAW.store(on, Ordering::Relaxed);
    #[cfg(test)]
    RAW.with(|v| v.set(on));
}

pub fn raw() -> bool {
    #[cfg(not(test))]
    {
        RAW.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    RAW.with(|v| v.get())
}

/// A tile scrolled as far as it can go: momentum and what is still queued stop (the input thread looks at this flag).
pub fn note_bound() {
    #[cfg(not(test))]
    BOUND.store(true, Ordering::Relaxed);
    #[cfg(test)]
    BOUND.with(|v| v.set(true));
}

pub fn take_bound() -> bool {
    #[cfg(not(test))]
    {
        BOUND.swap(false, Ordering::Relaxed)
    }
    #[cfg(test)]
    BOUND.with(|v| v.replace(false))
}

#[cfg(test)]
#[path = "scrolleng_tests.rs"]
mod tests;
