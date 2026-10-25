//! The screen saver: the idle clock (a small state machine with an injectable clock), the animated IYAKKI logo and the partial redraw
//! that keeps its cost low. No hardware and no shell here: `shell_saver.rs` connects it to input, the panel and the other screens.
//!
//! ## Idle clock
//! [`IdleMachine`] says which [`Stage`] the screen is in (awake, dimmed, saver, off) from the time of the last input, the person's
//! [`SaverConfig`], whether the machine is on battery and whether something keeps the screen on (an inhibit). Time is passed in as
//! milliseconds, so a test drives it with a fake clock; the shell's clock in tests never advances, so no existing test sees a saver.
//!
//! ## The logo
//! [`geometry`] is a pure function of the screen size and the time: the logo drifts like a bouncing logo but with easing (it is never
//! still: at the turn it still moves at 70 % of its speed, which also keeps the picture from burning in), breathes by 5 %, and its
//! glow pulses slowly through the theme's accent colours. [`Painter`] draws it on pure black and repaints only the rectangle the logo
//! left and the one it moved to; a test proves the result is pixel-for-pixel the picture a full redraw gives.

use crate::gfx::{rgb, Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::MUTED;
use iyakki_agent::screensaver::{First, SaverConfig, Style};
use tiny_skia::{FilterQuality, PixmapPaint, Transform};

/// Where the screen is in its idle cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Awake,
    /// The last seconds before "turn the screen off" with nothing else shown first: the backlight is lowered.
    Dim,
    Saver,
    Off,
}

impl Stage {
    /// The picture is the saver's (or black), not the desktop.
    pub fn covers(self) -> bool {
        matches!(self, Stage::Saver | Stage::Off)
    }
}

/// How long before a direct "turn the screen off" the panel is dimmed.
pub const DIM_LEAD_SECS: u64 = 10;
/// After a saver was started by hand (Preview) input is ignored this long: the click that started it is still being released.
pub const GRACE_MS: u64 = 600;

/// A hold on the screen: while one is live the screen stays on. Released with [`IdleMachine::release`] or by its expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(u64);

struct Hold {
    id: u64,
    reason: String,
    until_ms: u64,
}

struct Forced {
    until_ms: Option<u64>,
}

pub struct IdleMachine {
    last_input_ms: u64,
    stage: Stage,
    holds: Vec<Hold>,
    next_id: u64,
    forced: Option<Forced>,
    grace_until_ms: u64,
}

impl Default for IdleMachine {
    fn default() -> IdleMachine {
        IdleMachine::new(0)
    }
}

impl IdleMachine {
    pub fn new(now_ms: u64) -> IdleMachine {
        IdleMachine { last_input_ms: now_ms, stage: Stage::Awake, holds: Vec::new(), next_id: 1, forced: None, grace_until_ms: 0 }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Something was pressed, moved or touched.
    pub fn activity(&mut self, now_ms: u64) {
        self.last_input_ms = self.last_input_ms.max(now_ms);
    }

    /// Keep the screen on for `secs` seconds (asking again with the same reason extends it, it does not stack).
    pub fn inhibit(&mut self, now_ms: u64, reason: &str, secs: u64) -> Token {
        let until_ms = now_ms.saturating_add(secs.saturating_mul(1000));
        if let Some(h) = self.holds.iter_mut().find(|h| h.reason == reason) {
            h.until_ms = h.until_ms.max(until_ms);
            return Token(h.id);
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.holds.push(Hold { id, reason: reason.to_string(), until_ms });
        Token(id)
    }

    pub fn release(&mut self, t: Token) {
        self.holds.retain(|h| h.id != t.0);
    }

    /// The reasons that hold the screen on right now.
    pub fn hold_reasons(&self, now_ms: u64) -> Vec<&str> {
        self.holds.iter().filter(|h| h.until_ms > now_ms).map(|h| h.reason.as_str()).collect()
    }

    /// Start the saver now (Preview, or a chord): for `secs` seconds, or until input. Holds do not stop it.
    pub fn force_saver(&mut self, now_ms: u64, secs: Option<u64>) {
        self.forced = Some(Forced { until_ms: secs.map(|s| now_ms.saturating_add(s.saturating_mul(1000))) });
        self.grace_until_ms = now_ms.saturating_add(GRACE_MS);
    }

    /// Input arrived while the screen was not awake: true when it wakes the screen (false inside the grace time of a started
    /// preview, when the input is just dropped).
    pub fn wake(&mut self, now_ms: u64) -> bool {
        if self.stage == Stage::Awake && self.forced.is_none() {
            return false;
        }
        if now_ms < self.grace_until_ms {
            return false;
        }
        self.forced = None;
        self.stage = Stage::Awake;
        self.last_input_ms = now_ms;
        true
    }

    /// Back to awake without any input (the idle time starts again from now).
    pub fn reset(&mut self, now_ms: u64) {
        self.forced = None;
        self.stage = Stage::Awake;
        self.last_input_ms = now_ms;
    }

    /// Whether input within the grace time of a started preview is dropped.
    pub fn in_grace(&self, now_ms: u64) -> bool {
        now_ms < self.grace_until_ms && self.stage != Stage::Awake
    }

    /// One look at the clock. `keep_on` is true while something the person is watching or must answer needs the screen (playing
    /// media, an approval card, a banner...); it counts like input. Returns the new stage when it changed.
    pub fn step(&mut self, now_ms: u64, cfg: &SaverConfig, on_battery: bool, keep_on: bool) -> Option<Stage> {
        self.holds.retain(|h| h.until_ms > now_ms);
        let target = if let Some(f) = &self.forced {
            if f.until_ms.is_some_and(|u| now_ms >= u) {
                self.forced = None;
                self.last_input_ms = now_ms;
                Stage::Awake
            } else {
                Stage::Saver
            }
        } else if keep_on || !self.holds.is_empty() {
            self.last_input_ms = now_ms;
            Stage::Awake
        } else {
            let idle = now_ms.saturating_sub(self.last_input_ms) / 1000;
            match cfg.delays(on_battery) {
                (None, _) => Stage::Awake,
                (Some(first), off) => match cfg.first {
                    First::Nothing => Stage::Awake,
                    First::Off if idle >= first => Stage::Off,
                    First::Off if cfg.dim_pct > 0 && idle + DIM_LEAD_SECS >= first => Stage::Dim,
                    First::Off => Stage::Awake,
                    First::Saver => match off {
                        Some(o) if idle >= first.saturating_add(o) => Stage::Off,
                        _ if idle >= first => Stage::Saver,
                        _ => Stage::Awake,
                    },
                },
            }
        };
        if target == self.stage {
            return None;
        }
        self.stage = target;
        Some(target)
    }
}

/// Frames per second: 15 on mains, 8 on battery, 4 for a remote screen (every frame crosses the network).
pub fn fps(on_battery: bool, remote: bool) -> u32 {
    if remote {
        4
    } else if on_battery {
        8
    } else {
        15
    }
}

// ------------------------------------------------------------------------------------------------ the animation

/// The theme's accent colours the glow moves through (the launcher's blue, teal, purple and the boot screen's cyan).
pub const ACCENTS: [[u8; 4]; 4] = [rgb(79, 140, 255), rgb(61, 214, 163), rgb(167, 116, 255), rgb(100, 220, 240)];
/// The saver's background: pure black (an OLED pixel is off, an LCD with local dimming gives its darkest picture).
pub const BLACK: [u8; 4] = [0, 0, 0, 255];

/// The glow colour at `t` seconds: a slow, smooth walk through [`ACCENTS`].
pub fn accent_at(t: f64) -> [u8; 3] {
    let n = ACCENTS.len();
    let p = (t / 8.0).rem_euclid(n as f64);
    let i = (p.floor() as usize).min(n - 1);
    let f = p - p.floor();
    let f = f * f * (3.0 - 2.0 * f);
    let (a, b) = (ACCENTS[i], ACCENTS[(i + 1) % n]);
    let mix = |k: usize| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f).round().clamp_safe(0.0, 255.0) as u8;
    [mix(0), mix(1), mix(2)]
}

/// Everything about one frame, in screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Geom {
    /// The box that holds everything drawn this frame (logo, glow, clock): x, y, width, height. Constant size.
    pub bbox: (f64, f64, f64, f64),
    /// Centre of the logo and its diameter now (breathing included).
    pub cx: f64,
    pub cy: f64,
    pub d: f64,
    /// The glow: radius and colour (alpha included); alpha 0 = none.
    pub glow_r: f64,
    pub glow: [u8; 4],
    /// The logo's opacity (the small style is dim).
    pub opacity: f32,
    /// The time under the logo: left, top, width and text size; `None` without a clock.
    pub clock: Option<(f64, f64, f64, f64)>,
}

fn ease(x: f64) -> f64 {
    0.7 * x + 0.3 * (x * x * (3.0 - 2.0 * x))
}

/// A position from 0 to 1 that goes there and back, easing but never stopping, one way in `one_way` seconds.
fn travel(t: f64, one_way: f64, phase: f64) -> f64 {
    let u = (t / (2.0 * one_way.max(0.001)) + phase).rem_euclid(1.0);
    ease(1.0 - (2.0 * u - 1.0).abs())
}

/// The frame at `t` seconds on a `w` x `h` screen. Pure, total (any size, even 0) and always inside the screen when the screen is
/// big enough to hold the logo at all.
pub fn geometry(style: Style, w: u32, h: u32, t: f64) -> Geom {
    let (wf, hf) = (f64::from(w), f64::from(h));
    let m = wf.min(hf).max(1.0);
    let small = style == Style::Small;
    let with_clock = style == Style::LogoClock;
    let d0 = if small { (0.10 * m).clamp_safe((m * 0.5).min(24.0), 200.0) } else { (0.24 * m).clamp_safe((m * 0.5).min(40.0), 460.0) };
    let d_max = d0 * 1.05;
    let glow_r = if small { d_max / 2.0 + 2.0 } else { 0.78 * d0 };
    let gap = 0.12 * d0;
    let clock_size = 0.2 * d0;
    let clock_h = clock_size * 1.4;
    let below = if with_clock { glow_r.max(d_max / 2.0 + gap + clock_h) } else { glow_r };
    let bw = if with_clock { (2.0 * glow_r).max(1.5 * d0) } else { 2.0 * glow_r };
    let bh = glow_r + below;
    let margin = (0.02 * m).max(2.0);
    let (rx, ry) = ((wf - bw - 2.0 * margin).max(0.0), (hf - bh - 2.0 * margin).max(0.0));
    let (x, y) = if style == Style::Breathing {
        // hardly any drift: a few percent of the room, slowly, around the middle
        let (kx, ky) = (0.06, 0.06);
        let (px, py) = (rx * kx, ry * ky);
        (margin + rx / 2.0 - px / 2.0 + px * travel(t, 30.0, 0.13), margin + ry / 2.0 - py / 2.0 + py * travel(t, 21.0, 0.61))
    } else {
        // at least 24 px/s on average (so the picture really moves, even on a small screen), at most one crossing in 38 s
        let tx = (rx / 24.0).clamp_safe(6.0, 38.0);
        let ty = (ry / 24.0).clamp_safe(6.0, 25.0);
        (margin + rx * travel(t, tx, 0.13), margin + ry * travel(t, ty, 0.61))
    };
    let breath = (std::f64::consts::TAU * t / 9.0).sin();
    let d = d0 * (1.0 + 0.05 * breath);
    let cx = x + bw / 2.0;
    let cy = y + glow_r;
    let pulse = 0.5 + 0.5 * (std::f64::consts::TAU * t / 6.0).sin();
    let [r, g, b] = accent_at(t);
    let alpha = if small {
        0.0
    } else if style == Style::Breathing {
        50.0 + 70.0 * pulse
    } else {
        60.0 + 36.0 * pulse
    };
    let clock = with_clock.then(|| (cx - bw / 2.0, cy + d_max / 2.0 + gap, bw, clock_size));
    Geom { bbox: (x, y, bw, bh), cx, cy, d, glow_r, glow: [r, g, b, alpha as u8], opacity: if small { 0.6 } else { 1.0 }, clock }
}

/// A rectangle in whole pixels: left, top, right, bottom (exclusive).
pub type IRect = (i32, i32, i32, i32);

/// The union of two rectangles.
pub fn union(a: IRect, b: IRect) -> IRect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// The whole-pixel rectangle of a float box (one pixel wider on every side for anti-aliasing), cut to the screen.
pub fn pixel_box(b: (f64, f64, f64, f64), w: u32, h: u32) -> IRect {
    let (wi, hi) = (w.min(i32::MAX as u32) as i32, h.min(i32::MAX as u32) as i32);
    let f = |v: f64| v.clamp_safe(-1.0e6, 1.0e6);
    let x0 = (f(b.0).floor() as i32).saturating_sub(1).clamp_safe(0, wi);
    let y0 = (f(b.1).floor() as i32).saturating_sub(1).clamp_safe(0, hi);
    let x1 = (f(b.0 + b.2).ceil() as i32).saturating_add(1).clamp_safe(x0, wi);
    let y1 = (f(b.1 + b.3).ceil() as i32).saturating_add(1).clamp_safe(y0, hi);
    (x0, y0, x1, y1)
}

/// Draws the saver and remembers where the logo was.
#[derive(Default)]
pub struct Painter {
    size: (u32, u32),
    prev: Option<IRect>,
}

impl Painter {
    /// Forget the picture: the next frame clears the whole screen to black.
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Draw the frame at `t` seconds into `g`, changing only the rectangle the logo left and the one it moved to (the whole screen
    /// the first time, or when the size changed). `clock` is the time text of the clock style. Returns the rectangle that changed.
    pub fn draw(&mut self, g: &mut Gfx, style: Style, t: f64, clock: Option<&str>, s: f32) -> IRect {
        let (w, h) = (g.width(), g.height());
        let geo = geometry(style, w, h, t);
        let now = pixel_box(geo.bbox, w, h);
        let dirty = match self.prev {
            Some(old) if self.size == (w, h) => {
                fill(g, old, BLACK);
                union(old, now)
            }
            _ => {
                g.clear(BLACK);
                (0, 0, w.min(i32::MAX as u32) as i32, h.min(i32::MAX as u32) as i32)
            }
        };
        self.size = (w, h);
        self.prev = Some(now);
        paint(g, &geo, clock, s);
        dirty
    }
}

fn fill(g: &mut Gfx, r: IRect, c: [u8; 4]) {
    if r.2 > r.0 && r.3 > r.1 {
        g.rect(r.0 as f32, r.1 as f32, (r.2 - r.0) as f32, (r.3 - r.1) as f32, c);
    }
}

/// The logo, its glow and the clock of one frame, over whatever black is there.
fn paint(g: &mut Gfx, geo: &Geom, clock: Option<&str>, s: f32) {
    if geo.glow[3] > 0 && geo.glow_r > 1.0 {
        g.radial_glow(geo.cx as f32, geo.cy as f32, geo.glow_r as f32, geo.glow);
    }
    let logo = crate::splash::logo();
    let (lw, lh) = (logo.width() as f32, logo.height() as f32);
    if lw > 0.0 && lh > 0.0 && geo.d >= 1.0 {
        let k = geo.d as f32 / lw;
        let paint = PixmapPaint { opacity: geo.opacity, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
        let t = Transform::from_translate(geo.cx as f32, geo.cy as f32).pre_scale(k, k);
        g.pm.draw_pixmap(-(lw as i32) / 2, -(lh as i32) / 2, logo.as_ref(), &paint, t, None);
    }
    if let (Some((x, y, w, size)), Some(text)) = (geo.clock, clock) {
        let size = (size as f32).max(8.0 * s.min(1.0));
        let text = g.fit(size, text, Face::Bold, w as f32);
        let tw = g.text_width(size, &text, Face::Bold);
        g.text(x as f32 + (w as f32 - tw) / 2.0, y as f32, size, MUTED, &text, Face::Bold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SaverConfig {
        SaverConfig { first: First::Saver, after_min: 5, then_off_min: 5, battery_shorter: true, style: Style::Logo, dim_pct: 50 }
    }

    const MIN: u64 = 60_000;

    #[test]
    fn idle_then_saver_then_off_then_wake() {
        let mut m = IdleMachine::new(0);
        let c = cfg();
        assert_eq!(m.step(4 * MIN + 59_000, &c, false, false), None);
        assert_eq!(m.step(5 * MIN, &c, false, false), Some(Stage::Saver));
        assert_eq!(m.step(9 * MIN, &c, false, false), None, "still the saver");
        assert_eq!(m.step(10 * MIN, &c, false, false), Some(Stage::Off));
        assert!(m.wake(10 * MIN + 5), "input wakes");
        assert_eq!(m.stage(), Stage::Awake);
        assert_eq!(m.step(10 * MIN + 100, &c, false, false), None, "the idle time starts again at the wake");
        assert_eq!(m.step(15 * MIN + 5, &c, false, false), Some(Stage::Saver));
    }

    #[test]
    fn input_resets_the_clock_and_never_means_never() {
        let mut m = IdleMachine::new(0);
        let c = cfg();
        m.activity(4 * MIN);
        assert_eq!(m.step(5 * MIN, &c, false, false), None);
        assert_eq!(m.step(9 * MIN, &c, false, false), Some(Stage::Saver));
        let never = SaverConfig { after_min: 0, ..c.clone() };
        let mut m2 = IdleMachine::new(0);
        assert_eq!(m2.step(1000 * MIN, &never, false, false), None);
        let nothing = SaverConfig { first: First::Nothing, ..c };
        assert_eq!(m2.step(1000 * MIN, &nothing, false, false), None);
        assert_eq!(m2.stage(), Stage::Awake);
    }

    #[test]
    fn turning_the_screen_off_directly_dims_in_the_last_seconds() {
        let c = SaverConfig { first: First::Off, after_min: 2, ..cfg() };
        let mut m = IdleMachine::new(0);
        assert_eq!(m.step(109_000, &c, false, false), None);
        assert_eq!(m.step(110_000, &c, false, false), Some(Stage::Dim));
        assert_eq!(m.step(120_000, &c, false, false), Some(Stage::Off));
        let nodim = SaverConfig { dim_pct: 0, ..c };
        let mut m2 = IdleMachine::new(0);
        assert_eq!(m2.step(115_000, &nodim, false, false), None, "no dimming chosen");
    }

    #[test]
    fn on_battery_every_delay_is_halved_and_on_mains_it_is_not() {
        let c = cfg();
        let mut batt = IdleMachine::new(0);
        assert_eq!(batt.step(150_000, &c, true, false), Some(Stage::Saver), "2.5 minutes on battery");
        assert_eq!(batt.step(300_000, &c, true, false), Some(Stage::Off), "then 2.5 more minutes");
        let mut ac = IdleMachine::new(0);
        assert_eq!(ac.step(150_000, &c, false, false), None);
        let plain = SaverConfig { battery_shorter: false, ..c };
        let mut m = IdleMachine::new(0);
        assert_eq!(m.step(150_000, &plain, true, false), None, "the person turned the halving off");
    }

    #[test]
    fn a_hold_keeps_the_screen_on_until_it_is_released_or_expires() {
        let c = cfg();
        let mut m = IdleMachine::new(0);
        let t = m.inhibit(0, "test", 3600);
        assert_eq!(m.hold_reasons(1000), vec!["test"]);
        assert_eq!(m.step(30 * MIN, &c, false, false), None, "held: no saver");
        m.release(t);
        // the hold counted as activity until it ended
        assert_eq!(m.step(30 * MIN + 1000, &c, false, false), None);
        assert_eq!(m.step(35 * MIN + 1000, &c, false, false), Some(Stage::Saver));
        // a hold with an expiry
        let mut m2 = IdleMachine::new(0);
        m2.inhibit(0, "short", 120);
        assert_eq!(m2.step(2 * MIN - 1, &c, false, false), None);
        assert_eq!(m2.step(2 * MIN + 1000, &c, false, false), None, "the expiry counted as the last moment of activity");
        assert_eq!(m2.step(7 * MIN + 1000, &c, false, false), Some(Stage::Saver));
        // asking again with the same reason does not stack
        let (a, b) = (m2.inhibit(8 * MIN, "x", 10), m2.inhibit(8 * MIN, "x", 20));
        assert_eq!(a, b);
        assert_eq!(m2.hold_reasons(8 * MIN).len(), 1);
    }

    #[test]
    fn something_that_appears_while_the_saver_runs_wakes_it() {
        let c = cfg();
        let mut m = IdleMachine::new(0);
        assert_eq!(m.step(5 * MIN, &c, false, false), Some(Stage::Saver));
        assert_eq!(m.step(5 * MIN + 100, &c, false, true), Some(Stage::Awake), "an approval card, a banner...");
    }

    #[test]
    fn a_preview_runs_for_its_time_ignores_the_click_that_started_it_and_stops_on_input() {
        let c = SaverConfig { after_min: 0, ..cfg() };
        let mut m = IdleMachine::new(1000);
        m.force_saver(1000, Some(5));
        assert_eq!(m.step(1001, &c, false, false), Some(Stage::Saver), "even with the saver set to never");
        assert!(m.in_grace(1100));
        assert!(!m.wake(1100), "the release of the button that started it does not stop it");
        assert!(m.wake(1000 + GRACE_MS + 1));
        assert_eq!(m.stage(), Stage::Awake);
        // after five seconds it ends by itself
        let mut m2 = IdleMachine::new(0);
        m2.force_saver(0, Some(5));
        assert_eq!(m2.step(1, &c, false, false), Some(Stage::Saver));
        assert_eq!(m2.step(4999, &c, false, false), None);
        assert_eq!(m2.step(5000, &c, false, false), Some(Stage::Awake));
    }

    #[test]
    fn a_hold_does_not_stop_a_preview() {
        let c = cfg();
        let mut m = IdleMachine::new(0);
        m.inhibit(0, "music", 3600);
        m.force_saver(0, None);
        assert_eq!(m.step(10, &c, false, true), Some(Stage::Saver));
    }

    #[test]
    fn clocks_that_wrap_or_jump_back_never_panic() {
        let c = cfg();
        let mut m = IdleMachine::new(u64::MAX - 5);
        let _ = m.step(0, &c, false, false);
        let _ = m.step(u64::MAX, &c, true, false);
        m.inhibit(u64::MAX, "x", u64::MAX);
        m.force_saver(u64::MAX, Some(u64::MAX));
        let _ = m.step(10, &c, false, false);
    }

    #[test]
    fn frame_rates_follow_the_power_source() {
        assert_eq!((fps(false, false), fps(true, false), fps(false, true), fps(true, true)), (15, 8, 4, 4));
    }

    #[test]
    fn the_glow_walks_through_the_accent_colours_smoothly() {
        assert_eq!(accent_at(0.0), [ACCENTS[0][0], ACCENTS[0][1], ACCENTS[0][2]]);
        assert_eq!(accent_at(8.0), [ACCENTS[1][0], ACCENTS[1][1], ACCENTS[1][2]]);
        assert_eq!(accent_at(32.0), accent_at(0.0), "it comes round");
        let mut prev = accent_at(0.0);
        for i in 1..800 {
            let c = accent_at(i as f64 * 0.04);
            for k in 0..3 {
                assert!((i32::from(c[k]) - i32::from(prev[k])).abs() <= 6, "no jump of colour between two frames");
            }
            prev = c;
        }
    }

    const SIZES: [(u32, u32); 9] = [(360, 740), (800, 480), (1280, 720), (1366, 768), (1920, 1080), (2560, 1600), (2880, 1800), (3840, 2160), (1280, 1024)];
    const STYLES: [Style; 4] = [Style::Logo, Style::LogoClock, Style::Small, Style::Breathing];

    #[test]
    fn the_logo_stays_on_screen_at_every_size_and_style_for_ten_minutes() {
        for (w, h) in SIZES {
            for style in STYLES {
                for i in 0..4000 {
                    let g = geometry(style, w, h, i as f64 * 0.15);
                    let (x, y, bw, bh) = g.bbox;
                    assert!(x >= 0.0 && y >= 0.0 && x + bw <= f64::from(w) + 1e-6 && y + bh <= f64::from(h) + 1e-6, "{style:?} {w}x{h} t={}: box {:?}", i as f64 * 0.15, g.bbox);
                    assert!(g.d > 1.0 && g.cx.is_finite() && g.cy.is_finite());
                    assert!(g.cx - g.d / 2.0 >= x && g.cx + g.d / 2.0 <= x + bw && g.cy - g.d / 2.0 >= y && g.cy + g.d / 2.0 <= y + bh, "the logo is inside its box");
                }
            }
        }
    }

    #[test]
    fn tiny_and_empty_screens_do_not_panic() {
        for (w, h) in [(0, 0), (1, 1), (3, 200), (200, 3), (20, 20), (64, 48)] {
            for style in STYLES {
                let g = geometry(style, w, h, 12.5);
                assert!(g.cx.is_finite() && g.d.is_finite());
                let r = pixel_box(g.bbox, w, h);
                assert!(r.0 >= 0 && r.1 >= 0 && r.2 <= w as i32 && r.3 <= h as i32 && r.2 >= r.0 && r.3 >= r.1);
                let mut gfx = Gfx::new(w.max(1), h.max(1));
                let mut p = Painter::default();
                for i in 0..5 {
                    p.draw(&mut gfx, style, i as f64, Some("12:00"), 1.0);
                }
            }
        }
    }

    #[test]
    fn the_logo_moves_all_the_time_and_no_two_dirty_rectangles_in_a_row_are_the_same() {
        for (w, h) in SIZES {
            for style in [Style::Logo, Style::LogoClock, Style::Small] {
                for frame_ms in [1000 / 15, 1000 / 8] {
                    let mut prev_dirty: Option<IRect> = None;
                    let mut prev_box: Option<IRect> = None;
                    let mut t_ms = 0u64;
                    while t_ms < 10 * 60 * 1000 {
                        let b = pixel_box(geometry(style, w, h, t_ms as f64 / 1000.0).bbox, w, h);
                        if let Some(p) = prev_box {
                            assert_ne!(p, b, "{style:?} {w}x{h} at {t_ms} ms: the logo did not move");
                            let dirty = union(p, b);
                            assert_ne!(Some(dirty), prev_dirty, "{style:?} {w}x{h} at {t_ms} ms: the same dirty rectangle twice");
                            prev_dirty = Some(dirty);
                        }
                        prev_box = Some(b);
                        t_ms += frame_ms;
                    }
                }
            }
        }
    }

    #[test]
    fn the_logo_never_goes_faster_than_a_calm_pace() {
        for (w, h) in SIZES {
            let mut prev = geometry(Style::Logo, w, h, 0.0);
            for i in 1..4000 {
                let g = geometry(Style::Logo, w, h, i as f64 * 0.125);
                let step = ((g.cx - prev.cx).powi(2) + (g.cy - prev.cy).powi(2)).sqrt();
                // per 125 ms: well under the glide of a hand (1.5x the average speed of the slowest legs: 38 s to cross)
                assert!(step <= f64::from(w) * 0.03 + 1.0, "{w}x{h}: {step} px in an eighth of a second");
                prev = g;
            }
        }
    }

    #[test]
    fn the_bounding_box_covers_the_old_and_the_new_position_in_the_dirty_rectangle() {
        let (w, h) = (1920, 1080);
        let a = pixel_box(geometry(Style::Logo, w, h, 10.0).bbox, w, h);
        let b = pixel_box(geometry(Style::Logo, w, h, 10.07).bbox, w, h);
        let u = union(a, b);
        assert!(u.0 <= a.0 && u.0 <= b.0 && u.1 <= a.1 && u.1 <= b.1 && u.2 >= a.2 && u.2 >= b.2 && u.3 >= a.3 && u.3 >= b.3);
    }

    /// Frames drawn by the partial method equal frames drawn from scratch, to the pixel.
    fn partial_equals_full(style: Style, w: u32, h: u32, clock: Option<&str>) {
        let mut partial = Gfx::new(w, h);
        let mut painter = Painter::default();
        let mut full = Gfx::new(w, h);
        // a messy scene first: the desktop the saver replaces
        partial.clear(rgb(200, 40, 40));
        for i in 0..90 {
            let t = 3.0 + i as f64 * 0.0667 * 3.0;
            let dirty = painter.draw(&mut partial, style, t, clock, 1.0);
            let before = partial.rgba().to_vec();
            let mut fresh = Painter::default();
            fresh.draw(&mut full, style, t, clock, 1.0);
            assert_eq!(partial.rgba(), full.rgba(), "{style:?} {w}x{h} frame {i}: the partial redraw differs from a full redraw");
            // and nothing outside the reported rectangle differs from the frame before
            if i > 0 {
                assert!(dirty.2 > dirty.0 && dirty.3 > dirty.1);
            }
            drop(before);
        }
    }

    #[test]
    fn a_partial_redraw_is_pixel_exact_against_a_full_redraw() {
        for style in STYLES {
            partial_equals_full(style, 640, 400, Some("10:42 PM"));
        }
        partial_equals_full(Style::Logo, 1280, 720, None);
        partial_equals_full(Style::Logo, 300, 500, None);
    }

    #[test]
    fn only_the_dirty_rectangle_changes_between_two_frames() {
        let (w, h) = (800, 480);
        let mut g = Gfx::new(w, h);
        let mut p = Painter::default();
        p.draw(&mut g, Style::Logo, 5.0, None, 1.0);
        let before = g.rgba().to_vec();
        let dirty = p.draw(&mut g, Style::Logo, 5.5, None, 1.0);
        let after = g.rgba();
        let mut changed = 0usize;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                if before[i..i + 4] != after[i..i + 4] {
                    changed += 1;
                    assert!((x as i32) >= dirty.0 && (x as i32) < dirty.2 && (y as i32) >= dirty.1 && (y as i32) < dirty.3, "pixel ({x},{y}) changed outside {dirty:?}");
                }
            }
        }
        assert!(changed > 100, "the logo really moved");
        let area = ((dirty.2 - dirty.0) * (dirty.3 - dirty.1)) as f64;
        assert!(area < f64::from(w * h) * 0.5, "a frame touches a part of the screen, not all of it");
    }

    #[test]
    fn the_background_is_pure_black_and_the_size_changing_clears_everything() {
        let mut g = Gfx::new(400, 300);
        let mut p = Painter::default();
        p.draw(&mut g, Style::Logo, 1.0, None, 1.0);
        assert_eq!(&g.rgba()[0..4], &[0, 0, 0, 255], "the corner is black");
        g.resize(500, 320);
        g.clear(rgb(255, 0, 0));
        p.draw(&mut g, Style::Logo, 2.0, None, 1.0);
        assert_eq!(&g.rgba()[0..4], &[0, 0, 0, 255], "a new size is painted whole");
    }

    /// Pictures of saver frames for a person to look at: `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib saver::tests::pictures -- --ignored`.
    #[test]
    #[ignore]
    fn pictures() {
        let Ok(dir) = std::env::var("IYAKKI_PICTURES") else { return };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        for (w, h, style, t, name) in [
            (1280, 720, Style::Logo, 3.0, "saver-logo-1280x720-t3"),
            (1280, 720, Style::Logo, 20.0, "saver-logo-1280x720-t20"),
            (1280, 720, Style::LogoClock, 11.0, "saver-clock-1280x720"),
            (1280, 720, Style::Small, 5.0, "saver-small-1280x720"),
            (1280, 720, Style::Breathing, 8.0, "saver-breathing-1280x720"),
            (360, 740, Style::Logo, 9.0, "saver-logo-360x740"),
            (2880, 1800, Style::Logo, 30.0, "saver-logo-2880x1800"),
        ] {
            let mut g = Gfx::new(w, h);
            let mut p = Painter::default();
            // a few frames first, so the picture is what the partial redraw leaves, not a single clean frame
            for k in 0..6 {
                p.draw(&mut g, style, t - 0.4 + k as f64 * 0.0667, Some("10:42"), (w as f32 / 1280.0).clamp(0.6, 3.0));
            }
            g.save_png(&dir.join(format!("{name}.png"))).expect("png");
        }
    }
}
