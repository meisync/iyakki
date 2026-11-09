//! Controls for configuration pages: a slider with − and + buttons, an on/off switch, a section title. They draw into a
//! rectangle and register their click regions; what a click does is up to the page (a button sends the same key the
//! page already understands, a click on a slider's track arrives as `Event::AreaAt(id, x, y)` and [`value_at`] turns the
//! position into a value).

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, CARD_HOVER, DIM, MUTED, R, TEXT};

const TRACK: [u8; 4] = rgb(40, 48, 78);

/// Everything one slider row shows.
pub struct Slider<'a> {
    /// Identifies the track in `Event::Area` / `Event::AreaAt`.
    pub id: usize,
    pub title: &'a str,
    pub help: &'a str,
    pub value: i64,
    pub min: i64,
    pub max: i64,
    /// Shown after the value ("characters", "%").
    pub unit: &'a str,
    /// Sent by the − and + buttons.
    pub minus: Event,
    pub plus: Event,
    pub enabled: bool,
}

/// The value a click at `x` pixels along a track `width` wide stands for: the nearest multiple of `step` from `min`.
pub fn value_at(x: f32, width: f32, min: i64, max: i64, step: i64) -> i64 {
    if max <= min || width <= 0.0 {
        return min;
    }
    let frac = (x / width).clamp(0.0, 1.0) as f64;
    let raw = min as f64 + frac * (max - min) as f64;
    let step = step.max(1);
    let snapped = min + ((raw - min as f64) / step as f64).round() as i64 * step;
    snapped.clamp_safe(min, max)
}

/// How far along its track a value sits, 0.0 to 1.0.
pub fn fraction(value: i64, min: i64, max: i64) -> f32 {
    if max <= min { 0.0 } else { ((value - min) as f64 / (max - min) as f64).clamp(0.0, 1.0) as f32 }
}

/// Help text wrapped to `max` lines; if there is more, the last line ends in an ellipsis instead of stopping mid-word.
fn help_lines(g: &mut Gfx, text: &str, size: f32, width: f32, max: usize) -> Vec<String> {
    let lines = g.wrap(size, text, Face::Sans, width);
    if lines.len() <= max {
        return lines;
    }
    let mut out: Vec<String> = lines[..max - 1].to_vec();
    let rest = lines[max - 1..].join(" ");
    out.push(g.fit(size, &rest, Face::Sans, width));
    out
}

fn round_button(g: &mut Gfx, r: R, s: f32, glyph: &str, on: bool, ev: Event, label: &str) {
    let hot = on && g.hovered(r);
    g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, if hot { CARD_HOVER } else { TRACK });
    let size = 20.0 * s;
    let tw = g.text_width(size, glyph, Face::Bold);
    let lh = g.line_height(size, Face::Bold);
    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, size, if on { TEXT } else { DIM }, glyph, Face::Bold);
    if on {
        g.hit(r, ev, label);
    }
}

/// A card with a title and help on the left and, on the right, `[−] ───●─── [+]` with the value above it. Returns the
/// track's rectangle.
pub fn slider_row(g: &mut Gfx, r: R, s: f32, sl: &Slider, accent: [u8; 4]) -> R {
    crate::fx::glass(g, r.x, r.y, r.w, r.h, 14.0 * s, accent);
    let split = (r.w * 0.44).min(380.0 * s).max(160.0 * s);
    let text_w = (split - 20.0 * s).max(60.0 * s);
    let title = g.fit(16.0 * s, sl.title, Face::Bold, text_w);
    g.text(r.x + 18.0 * s, r.y + 12.0 * s, 16.0 * s, if sl.enabled { TEXT } else { MUTED }, &title, Face::Bold);
    for (i, l) in help_lines(g, sl.help, 13.0 * s, text_w, 2).iter().enumerate() {
        g.text(r.x + 18.0 * s, r.y + 38.0 * s + i as f32 * 17.0 * s, 13.0 * s, DIM, l, Face::Sans);
    }
    let bs = 34.0 * s;
    let (cx0, cx1) = (r.x + split, r.x + r.w - 18.0 * s);
    let by = r.y + r.h - bs - 14.0 * s;
    let track = R::new(cx0 + bs + 12.0 * s, by + bs / 2.0 - 4.0 * s, (cx1 - cx0 - 2.0 * bs - 24.0 * s).max(40.0 * s), 8.0 * s);
    let value = format!("{} {}", sl.value, sl.unit).trim().to_string();
    let vw = g.text_width(17.0 * s, &value, Face::Bold);
    g.text(cx1 - vw, r.y + 12.0 * s, 17.0 * s, if sl.enabled { accent } else { MUTED }, &value, Face::Bold);
    let can_minus = sl.enabled && sl.value > sl.min;
    let can_plus = sl.enabled && sl.value < sl.max;
    round_button(g, R::new(cx0, by, bs, bs), s, "−", can_minus, sl.minus.clone(), "decrease");
    round_button(g, R::new(cx1 - bs, by, bs, bs), s, "+", can_plus, sl.plus.clone(), "increase");
    let f = fraction(sl.value, sl.min, sl.max);
    let col = if sl.enabled { accent } else { DIM };
    crate::fx::progress_bar(g, track, f, col, s);
    let kx = track.x + track.w * f;
    if sl.enabled {
        g.circle(kx, track.y + track.h / 2.0, 15.0 * s, [accent[0], accent[1], accent[2], 40]);
    }
    g.circle(kx, track.y + track.h / 2.0, 11.0 * s, rgb(10, 14, 28));
    g.circle(kx, track.y + track.h / 2.0, 9.0 * s, if sl.enabled { TEXT } else { MUTED });
    g.ring(kx, track.y + track.h / 2.0, 9.0 * s, 1.5 * s, col);
    if sl.enabled {
        // a generous click area around the thin track
        g.hit(R::new(track.x, by, track.w, bs), Event::Area(sl.id), "slider");
        let said = if sl.unit == "%" { format!("{} percent", sl.value) } else { value.clone() };
        crate::access::describe("slider", &Event::Area(sl.id), crate::access::Meta::new(crate::access::Role::Slider, crate::access::State::default().value(said)).named(sl.title));
        crate::access::describe("decrease", &sl.minus, crate::access::Meta::new(crate::access::Role::Button, crate::access::State::default().disabled(!can_minus)).named(&format!("Decrease {}", sl.title)));
        crate::access::describe("increase", &sl.plus, crate::access::Meta::new(crate::access::Role::Button, crate::access::State::default().disabled(!can_plus)).named(&format!("Increase {}", sl.title)));
    }
    R::new(track.x, by, track.w, bs)
}

/// A card with a title and help and an on/off switch on the right. A click anywhere on it sends `toggle`.
pub fn switch_row(g: &mut Gfx, r: R, s: f32, title: &str, help: &str, on: bool, enabled: bool, toggle: Event, accent: [u8; 4]) {
    let hot = enabled && g.hovered(r);
    crate::fx::row_at(g, r.x, r.y, r.w, r.h, 14.0 * s, false, hot, accent);
    let text_w = (r.w - 110.0 * s).max(80.0 * s);
    let title = g.fit(16.0 * s, title, Face::Bold, text_w);
    g.text(r.x + 18.0 * s, r.y + 12.0 * s, 16.0 * s, if enabled { TEXT } else { MUTED }, &title, Face::Bold);
    for (i, l) in help_lines(g, help, 13.0 * s, text_w, 2).iter().enumerate() {
        g.text(r.x + 18.0 * s, r.y + 38.0 * s + i as f32 * 17.0 * s, 13.0 * s, DIM, l, Face::Sans);
    }
    let (sw, sh) = (52.0 * s, 28.0 * s);
    let (sx, sy) = (r.x + r.w - 18.0 * s - sw, r.y + (r.h - sh) / 2.0);
    crate::fx::switch(g, R::new(sx, sy, sw, sh), on, enabled, accent, s);
    if enabled {
        g.hit(r, toggle.clone(), title.as_str());
        crate::access::describe(title.as_str(), &toggle, crate::access::Meta::new(crate::access::Role::Toggle, crate::access::State::default().checked(on)).named(title.as_str()));
    }
}

/// A plain card with a title and a value, nothing to change.
pub fn info_row(g: &mut Gfx, r: R, s: f32, title: &str, value: &str) {
    crate::fx::glass(g, r.x, r.y, r.w, r.h, 14.0 * s, crate::fx::CYAN);
    g.text(r.x + 18.0 * s, r.y + (r.h - g.line_height(16.0 * s, Face::Bold)) / 2.0, 16.0 * s, TEXT, title, Face::Bold);
    let value = g.fit(14.0 * s, value, Face::Mono, r.w * 0.55);
    let vw = g.text_width(14.0 * s, &value, Face::Mono);
    g.text(r.x + r.w - 18.0 * s - vw, r.y + (r.h - g.line_height(14.0 * s, Face::Mono)) / 2.0, 14.0 * s, MUTED, &value, Face::Mono);
}

/// A small heading above a group of rows; returns the height it used.
pub fn section_title(g: &mut Gfx, x: f32, y: f32, s: f32, title: &str) -> f32 {
    let h = crate::fx::section_title(g, x, y, 640.0 * s, s, title, crate::fx::CYAN);
    #[cfg(any(test, feature = "testing"))]
    HEADINGS.with(|v| v.borrow_mut().push(R::new(x, y, 120.0 * s, h)));
    h
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    /// Test aid: every heading drawn on this thread (layout checks of the Settings pages).
    pub static HEADINGS: std::cell::RefCell<Vec<R>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Key;

    #[test]
    fn a_click_on_the_track_becomes_the_nearest_step() {
        assert_eq!(value_at(0.0, 200.0, 100, 4000, 100), 100);
        assert_eq!(value_at(200.0, 200.0, 100, 4000, 100), 4000);
        assert_eq!(value_at(100.0, 200.0, 100, 4000, 100), 2100, "the middle is 2050, halfway between two steps: it rounds up");
        assert_eq!(value_at(-50.0, 200.0, 1, 50, 1), 1, "left of the track is the minimum");
        assert_eq!(value_at(999.0, 200.0, 1, 50, 1), 50, "right of it the maximum");
        assert_eq!(value_at(50.0, 100.0, 0, 100, 5), 50);
        assert_eq!(value_at(51.0, 100.0, 0, 100, 5), 50);
        assert_eq!(value_at(53.0, 100.0, 0, 100, 5), 55);
        assert_eq!(value_at(10.0, 0.0, 0, 100, 5), 0, "a track with no width is the minimum");
        assert_eq!(value_at(10.0, 100.0, 7, 7, 1), 7, "a track with nothing to choose");
        assert_eq!(value_at(30.0, 100.0, 0, 10, 0), 3, "a zero step is treated as one");
    }

    #[test]
    fn long_help_ends_in_an_ellipsis_not_mid_word() {
        let mut g = Gfx::new(100, 100);
        let short = help_lines(&mut g, "Short help.", 13.0, 300.0, 2);
        assert_eq!(short, vec!["Short help.".to_string()]);
        let long = help_lines(&mut g, "A long explanation that certainly needs more than two lines of this narrow column to be said in full, word by word.", 13.0, 160.0, 2);
        assert_eq!(long.len(), 2);
        assert!(long[1].ends_with('…'), "{long:?}");
        assert!(help_lines(&mut g, "", 13.0, 160.0, 2).len() <= 1);
    }

    #[test]
    fn the_thumb_position_follows_the_value() {
        assert_eq!(fraction(100, 100, 4000), 0.0);
        assert_eq!(fraction(4000, 100, 4000), 1.0);
        assert!((fraction(2050, 100, 4000) - 0.5).abs() < 1e-6);
        assert_eq!(fraction(5, 5, 5), 0.0);
        assert_eq!(fraction(-3, 0, 10), 0.0);
        assert_eq!(fraction(30, 0, 10), 1.0);
    }

    #[test]
    fn the_controls_draw_register_their_click_regions_and_stay_in_their_card() {
        for (w, h, s) in [(900.0, 80.0, 1.0f32), (400.0, 80.0, 0.6), (1600.0, 150.0, 2.0)] {
            let mut g = Gfx::new(2000, 400);
            g.clear(rgb(0, 0, 0));
            let sl = Slider { id: 7001, title: "Chunk size", help: "How many characters of a document go into one searchable piece.", value: 700, min: 100, max: 4000, unit: "characters", minus: Event::Key(Key::Char('-')), plus: Event::Key(Key::Char('=')), enabled: true };
            let r = R::new(20.0, 20.0, w, h);
            let track = slider_row(&mut g, r, s, &sl, rgb(120, 170, 255));
            assert!(track.x >= r.x && track.x + track.w <= r.x + r.w && track.y >= r.y && track.y + track.h <= r.y + r.h + 1.0, "{track:?} inside {r:?}");
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            assert!(labels.contains(&"decrease") && labels.contains(&"increase") && labels.contains(&"slider"), "{labels:?}");
            assert!(g.hits.iter().any(|h| h.ev == Event::Area(7001)));
            // at the ends only the possible button works; disabled shows no regions
            let at_min = Slider { value: 100, ..Slider { id: 1, title: "t", help: "", value: 0, min: 100, max: 4000, unit: "", minus: Event::Tick, plus: Event::Tick, enabled: true } };
            g.hits.clear();
            slider_row(&mut g, r, s, &at_min, rgb(1, 2, 3));
            assert!(!g.hits.iter().any(|h| h.label == "decrease") && g.hits.iter().any(|h| h.label == "increase"));
            g.hits.clear();
            slider_row(&mut g, r, s, &Slider { enabled: false, ..at_min }, rgb(1, 2, 3));
            assert!(g.hits.is_empty(), "a slider you may not change cannot be clicked");
            g.hits.clear();
            switch_row(&mut g, r, s, "External RAG", "Also search the backends below.", true, true, Event::Key(Key::Char('u')), rgb(1, 2, 3));
            assert_eq!(g.hits.len(), 1);
            g.hits.clear();
            switch_row(&mut g, r, s, "External RAG", "", false, false, Event::Key(Key::Char('u')), rgb(1, 2, 3));
            assert!(g.hits.is_empty());
            info_row(&mut g, r, s, "Embedding", "hash-256");
            assert!(section_title(&mut g, 10.0, 10.0, s, "Retrieval") > 0.0);
        }
    }
}
