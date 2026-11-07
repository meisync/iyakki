//! The shared look of every app: the "futuristic" design language the RADIO player introduced (dark glass panels, cyan
//! and magenta neon accents, soft glows), as small drawing helpers so no app copies colours and radii around.
//!
//! Everything here only draws (plus the click regions the caller passes in); nothing changes behaviour. All sizes take
//! the UI scale `s` the app was given. Animation is limited to small accents driven by the shell's tick, and the big
//! static background is drawn once per screen size and copied (see [`background`]). The full description of the look,
//! with the sizes and radii, is in `docs/DESIGN.md`.

use std::cell::{Cell, RefCell};

use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, R, BG_BOTTOM, BG_TOP, CARD, CARD_HOVER, DIM, ERR, MUTED, OK, TEXT};

// ---------------------------------------------------------------------------------------------------------- palette

/// The two neon colours of the language. Apps use their own accent for borders and highlights, and these two for the
/// moving or "live" details (level bars, scan lines, progress).
pub const CYAN: [u8; 4] = rgb(70, 225, 255);
pub const MAGENTA: [u8; 4] = rgb(255, 80, 190);
/// The darkest glass: players, charts, the sides of a screen.
pub const PANEL: [u8; 4] = rgb(12, 14, 19);
/// A step lighter than [`PANEL`]: inputs, chips and buttons at rest.
pub const WELL: [u8; 4] = rgb(21, 23, 31);
/// Amber for "needs attention" (warnings), next to [`ERR`] and [`OK`].
pub const AMBER: [u8; 4] = rgb(240, 192, 104);

/// Sizes (before scaling) that every screen uses.
pub mod size {
    pub const TITLE: f32 = 28.0;
    pub const HEADER_TITLE: f32 = 20.0;
    pub const HEAD: f32 = 17.0;
    pub const BODY: f32 = 14.0;
    pub const CAPTION: f32 = 12.0;
    pub const TINY: f32 = 11.0;
    /// Radii: a big card, a list row, a chip/button (full pill), a field.
    pub const R_CARD: f32 = 16.0;
    pub const R_ROW: f32 = 12.0;
    pub const R_FIELD: f32 = 8.0;
    /// Gap between neighbouring cards and rows.
    pub const GAP: f32 = 12.0;
    pub const MARGIN: f32 = 24.0;
}

pub fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), m(a[3], b[3])]
}

pub fn with_alpha(c: [u8; 4], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

/// `c` painted over an opaque `bg` at its own alpha (what the eye sees), opaque.
pub fn over(bg: [u8; 4], c: [u8; 4]) -> [u8; 4] {
    let a = c[3] as f32 / 255.0;
    let m = |x: u8, y: u8| (y as f32 * a + x as f32 * (1.0 - a)).round() as u8;
    [m(bg[0], c[0]), m(bg[1], c[1]), m(bg[2], c[2]), 255]
}

fn channel(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.03928 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
pub fn luminance(c: [u8; 4]) -> f32 {
    0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
}

/// WCAG contrast ratio between two opaque colours (1.0 to 21.0).
pub fn contrast(a: [u8; 4], b: [u8; 4]) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// `fg` nudged towards white (on a dark `bg`) until it reads at least `min`:1 on `bg`.
pub fn readable(fg: [u8; 4], bg: [u8; 4], min: f32) -> [u8; 4] {
    let mut c = [fg[0], fg[1], fg[2], 255];
    for k in 0..=10 {
        c = mix([fg[0], fg[1], fg[2], 255], [255, 255, 255, 255], k as f32 / 10.0);
        if contrast(c, bg) >= min {
            break;
        }
    }
    c
}

// ---------------------------------------------------------------------------------------------------- motion setting

thread_local! {
    static REDUCED_MOTION: Cell<bool> = const { Cell::new(false) };
    static BG_CACHE: RefCell<Option<((u32, u32, u32), Vec<u8>)>> = const { RefCell::new(None) };
}

/// Switch the small header animation off (people who dislike motion). The background never moves.
pub fn set_reduced_motion(on: bool) {
    REDUCED_MOTION.with(|c| c.set(on));
}

pub fn reduced_motion() -> bool {
    REDUCED_MOTION.with(|c| c.get()) || std::env::var_os("IYAKKI_REDUCED_MOTION").is_some()
}

// ------------------------------------------------------------------------------------------------------ background

/// Paint the whole-screen backdrop: a soft vertical gradient, two faint neon glows in the corners, a fine grid and a few
/// "particles". It is drawn once per size into a cache and copied afterwards (a memory copy, well under a millisecond),
/// so a frame never pays for the gradients again.
pub fn background(g: &mut Gfx) {
    let size = (g.width(), g.height(), crate::theme::generation());
    BG_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let fresh = matches!(&*c, Some((sz, _)) if *sz == size);
        if !fresh {
            *c = Some((size, build_background(g)));
        }
        if let Some((_, data)) = &*c {
            let dst = g.pm.data_mut();
            if dst.len() == data.len() {
                dst.copy_from_slice(data);
            }
        }
    });
}

/// How many sizes are cached (one: the last), for tests.
pub fn background_cached_size() -> Option<(u32, u32)> {
    BG_CACHE.with(|c| c.borrow().as_ref().map(|(s, _)| (s.0, s.1)))
}

fn build_background(g: &mut Gfx) -> Vec<u8> {
    let (w, h) = (g.width() as f32, g.height() as f32);
    g.gradient(BG_TOP, BG_BOTTOM);
    g.radial_glow(w * 0.92, -h * 0.05, w * 0.55, [70, 225, 255, 20]);
    g.radial_glow(w * 0.05, h * 1.05, w * 0.5, [255, 80, 190, 14]);
    // a faint grid
    let step = (56.0 * (w / 1280.0).clamp(0.6, 2.0)).round().max(24.0);
    let mut x = step;
    while x < w {
        g.rect(x, 0.0, 1.0, h, [120, 150, 255, 7]);
        x += step;
    }
    let mut y = step;
    while y < h {
        g.rect(0.0, y, w, 1.0, [120, 150, 255, 7]);
        y += step;
    }
    // scanlines: every fourth row a hair darker
    let mut y = 0.0;
    while y < h {
        g.rect(0.0, y, w, 1.0, [0, 0, 0, 14]);
        y += 4.0;
    }
    // particles: a fixed pseudo-random scatter
    let mut seed = 0x9E37_79B9u32 ^ (w as u32).wrapping_mul(31) ^ (h as u32);
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    for _ in 0..((w * h / 24_000.0) as usize).clamp(20, 140) {
        let (px, py, a) = (next() * w, next() * h, 18.0 + next() * 40.0);
        let tint = if next() > 0.5 { CYAN } else { MAGENTA };
        g.rect(px, py, 2.0, 2.0, with_alpha(tint, a as u8));
    }
    g.pm.data().to_vec()
}

// --------------------------------------------------------------------------------------------------------- header

/// The band across the top of every app: dark gradient, the app's icon in a soft glow, its title, a quiet subtitle, and a
/// thin accent line with a small highlight that slides along it as frames are drawn. Height is `hh`.
pub fn app_header(g: &mut Gfx, w: f32, hh: f32, s: f32, title: &str, subtitle: &str, icon: Icon, accent: [u8; 4], tick: u64, right_limit: f32) {
    g.gradient_rect(0.0, 0.0, w, hh, (0.0, 0.0), (0.0, hh), &[(0.0, rgb(19, 25, 48)), (1.0, rgb(11, 14, 28))]);
    g.radial_glow(34.0 * s, hh / 2.0, 70.0 * s, with_alpha(accent, 34));
    g.circle(34.0 * s, hh / 2.0, 17.0 * s, with_alpha(accent, 40));
    g.ring(34.0 * s, hh / 2.0, 17.0 * s, 1.0 * s, with_alpha(accent, 150));
    g.icon(icon, 34.0 * s, hh / 2.0, 22.0 * s, accent);
    let ty = (hh - 22.0 * s) / 2.0 - 2.0 * s;
    g.text(60.0 * s, ty, size::HEADER_TITLE * s, TEXT, title, Face::Bold);
    if !subtitle.is_empty() {
        let tw = g.text_width(size::HEADER_TITLE * s, title, Face::Bold);
        let sx = 60.0 * s + tw + 14.0 * s;
        let avail = right_limit - sx;
        if avail > 80.0 * s {
            g.rect(sx - 8.0 * s, hh * 0.3, 1.0, hh * 0.4, with_alpha(accent, 110));
            let sub = g.fit(size::BODY * s, subtitle, Face::Sans, avail);
            g.text(sx, ty + 4.0 * s, size::BODY * s, MUTED, &sub, Face::Sans);
        }
    }
    accent_line(g, 0.0, hh, w, 2.0 * s, accent, tick, s);
}

/// A 2px line in `accent` that fades towards the right, with a short bright highlight that moves with `tick`.
pub fn accent_line(g: &mut Gfx, x: f32, y: f32, w: f32, th: f32, accent: [u8; 4], tick: u64, s: f32) {
    g.gradient_rect(x, y, w, th, (x, 0.0), (x + w, 0.0), &[(0.0, with_alpha(accent, 255)), (0.6, with_alpha(accent, 170)), (1.0, with_alpha(accent, 70))]);
    g.rect(x, y + th, w, 1.0, with_alpha(accent, 28));
    if !reduced_motion() {
        let len = 180.0 * s;
        let pos = x - len + ((tick as f32 * 6.0 * s) % (w + len * 2.0));
        let (a, b) = (pos.max(x), (pos + len).min(x + w));
        if b > a {
            g.gradient_rect(a, y, b - a, th, (pos, 0.0), (pos + len, 0.0), &[(0.0, [255, 255, 255, 0]), (0.5, [255, 255, 255, 190]), (1.0, [255, 255, 255, 0])]);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- cards

/// A glass card: translucent dark fill, a soft shadow below, a 1px highlight inside the top edge and a thin border in
/// the accent colour with a faint outer glow. `glow` (0..1) strengthens the glow (a playing player, a selected card).
pub fn card(g: &mut Gfx, r: R, s: f32, accent: [u8; 4], glow: f32) {
    let rad = size::R_CARD * s;
    // the shadow is a soft edge under the card, not a second copy of it (a big translucent fill costs real time)
    g.rrect_outline(r.x, r.y + 3.0 * s, r.w, r.h, rad, 5.0 * s, [0, 0, 0, 60]);
    if glow > 0.02 {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 7.0 * s, with_alpha(accent, (70.0 * glow.min(1.0)) as u8));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, CARD);
    g.rect(r.x + rad * 0.6, r.y + 1.0, r.w - rad * 1.2, 1.0, [255, 255, 255, 16]);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.0 * s, with_alpha(mix(accent, MAGENTA, 0.25), (60.0 + 90.0 * glow.min(1.0)) as u8));
    always_border(g, r.x, r.y, r.w, r.h, rad, s);
}

/// High contrast only: a solid, thick border so no card, row or field ever depends on a faint tint to be seen.
pub fn always_border(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, rad: f32, s: f32) {
    if crate::theme::is_high_contrast() {
        g.rrect_outline(x, y, w, h, rad, 1.2 * s.max(0.8), crate::ui::BORDER);
    }
}

/// The darkest panel (a chart well, a side list, a code box): [`PANEL`] with a hairline border.
pub fn panel(g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
    let rad = size::R_CARD * s;
    g.rrect(r.x, r.y, r.w, r.h, rad, PANEL);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.0 * s, with_alpha(accent, 48));
    always_border(g, r.x, r.y, r.w, r.h, rad, s);
}

/// A recessed well inside a card (a sparkline, an input background).
pub fn well(g: &mut Gfx, r: R, s: f32) {
    g.rrect(r.x, r.y, r.w, r.h, size::R_FIELD * s, rgb(10, 14, 30));
    g.rrect_outline(r.x, r.y, r.w, r.h, size::R_FIELD * s, 1.0, [255, 255, 255, 14]);
}

/// A list row: glass at rest, brighter on hover, and with a neon outline and an accent edge when selected.
pub fn list_row(g: &mut Gfx, r: R, s: f32, selected: bool, hot: bool, accent: [u8; 4]) {
    row_at(g, r.x, r.y, r.w, r.h, size::R_ROW * s, selected, hot, accent);
}

/// [`list_row`] with the radius chosen by the caller (apps keep their own corner sizes).
pub fn row_at(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, rad: f32, selected: bool, hot: bool, accent: [u8; 4]) {
    let s = (rad / size::R_ROW).max(0.5);
    if selected {
        g.rrect(x, y, w, h, rad, over(rgb(30, 38, 68), with_alpha(accent, 44)));
        g.rrect_outline(x, y, w, h, rad, 1.2 * s * crate::theme::ring(), with_alpha(accent, 190));
        g.rrect(x + 1.5 * s, y + h * 0.2, 3.0 * s, h * 0.6, 1.5 * s, accent);
    } else if hot {
        g.rrect(x, y, w, h, rad, CARD_HOVER);
        g.rrect_outline(x, y, w, h, rad, 1.0, with_alpha(accent, 90));
    } else {
        glass(g, x, y, w, h, rad, accent);
    }
}

/// A resting glass tile: [`CARD`] fill, a highlight along the top edge and a hairline border tinted by the accent. The
/// cheap cousin of [`card`] for rows and settings cards that come many to a screen.
pub fn glass(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, rad: f32, accent: [u8; 4]) {
    g.rrect(x, y, w, h, rad, CARD);
    if w > rad * 2.0 {
        g.rect(x + rad * 0.7, y + 1.0, w - rad * 1.4, 1.0, [255, 255, 255, 12]);
    }
    g.rrect_outline(x, y, w, h, rad, 1.0, with_alpha(accent, 34));
    always_border(g, x, y, w, h, rad, 1.0);
}

/// The fill a text on a [`list_row`] is read against (for contrast checks).
pub fn list_row_fill(selected: bool, accent: [u8; 4]) -> [u8; 4] {
    if selected {
        over(rgb(30, 38, 68), with_alpha(accent, 44))
    } else {
        CARD
    }
}

// ----------------------------------------------------------------------------------------------- chips and buttons

/// How a button looks: the accent decides. Red is a danger button, the muted greys are secondary, anything else primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Danger,
}

impl Variant {
    pub fn of(accent: [u8; 4]) -> Variant {
        let a = [accent[0], accent[1], accent[2]];
        if a == [ERR[0], ERR[1], ERR[2]] {
            Variant::Danger
        } else if [MUTED, DIM, TEXT].iter().any(|c| [c[0], c[1], c[2]] == a) {
            Variant::Secondary
        } else {
            Variant::Primary
        }
    }
}

/// A pill (full-radius) button body: dark glass, a tinted fill and neon outline for primary/danger, a glow when hovered.
/// Returns the colour the label should be drawn in. The caller draws the label and registers the hit.
pub fn pill(g: &mut Gfx, r: R, s: f32, accent: [u8; 4], hot: bool, enabled: bool) -> [u8; 4] {
    let rad = r.h / 2.0;
    let v = Variant::of(accent);
    if !enabled {
        g.rrect(r.x, r.y, r.w, r.h, rad, rgb(24, 29, 48));
        return rgb(104, 114, 142);
    }
    let base = if hot { rgb(44, 49, 64) } else { rgb(28, 31, 40) };
    let (fill, line) = match v {
        Variant::Primary => (over(base, with_alpha(accent, if hot { 54 } else { 30 })), with_alpha(accent, if hot { 255 } else { 190 })),
        Variant::Danger => (over(base, with_alpha(ERR, if hot { 60 } else { 34 })), with_alpha(ERR, if hot { 255 } else { 190 })),
        Variant::Secondary => (base, if hot { with_alpha(accent, 200) } else { rgb(66, 72, 90) }),
    };
    if hot {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 4.0 * s, with_alpha(if v == Variant::Danger { ERR } else { accent }, 80));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, fill);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.2 * s.max(0.8) * if hot { crate::theme::ring() } else { 1.0 }, line);
    always_border(g, r.x, r.y, r.w, r.h, rad, s);
    readable(accent, fill, 4.5)
}

/// A neon tag: a small pill with text, outlined in `accent` (filled when `filled`). Returns its width. Not clickable.
pub fn chip(g: &mut Gfx, x: f32, y: f32, s: f32, text: &str, accent: [u8; 4], filled: bool) -> f32 {
    let sz = size::CAPTION * s;
    let w = g.text_width(sz, text, Face::Bold) + sz * 1.4;
    let h = sz * 1.9;
    let fill = if filled { over(WELL, with_alpha(accent, 70)) } else { WELL };
    g.rrect(x, y, w, h, h / 2.0, fill);
    g.rrect_outline(x, y, w, h, h / 2.0, 1.0 * s.max(0.8), with_alpha(accent, if filled { 230 } else { 140 }));
    g.text(x + sz * 0.7, y + (h - g.line_height(sz, Face::Bold)) / 2.0, sz, readable(accent, fill, 4.5), text, Face::Bold);
    w
}

/// A strip of page tabs: pill chips with the chosen one lit. Each item is (label, click event); the label is also the
/// click region's name. Returns the strip's width. Tabs that do not fit are left out (`max_w`).
pub fn tab_strip(g: &mut Gfx, x: f32, y: f32, s: f32, max_w: f32, items: &[(String, Event)], selected: usize, accent: [u8; 4]) -> f32 {
    let size = 14.0 * s;
    let mut bx = x;
    for (i, (label, ev)) in items.iter().enumerate() {
        let w = g.text_width(size, label, Face::Bold) + size * 1.7;
        if bx + w > x + max_w {
            break;
        }
        let h = size * 2.1;
        let r = R::new(bx, y, w, h);
        let on = i == selected;
        let hot = g.hovered(r);
        let fill = if on { over(WELL, with_alpha(accent, 56)) } else if hot { CARD_HOVER } else { WELL };
        g.rrect(r.x, r.y, r.w, r.h, h / 2.0, fill);
        g.rrect_outline(r.x, r.y, r.w, r.h, h / 2.0, if on { 1.5 * s } else { 1.0 }, if on { accent } else if hot { with_alpha(accent, 120) } else { rgb(50, 60, 92) });
        if on {
            g.rect(r.x + w * 0.3, r.y + h - 3.0 * s, w * 0.4, 2.0 * s, accent);
        }
        let tc = if on { readable(accent, fill, 4.5) } else { readable(MUTED, fill, 4.5) };
        g.text(r.x + size * 0.85, r.y + (h - g.line_height(size, Face::Bold)) / 2.0, size, tc, label, Face::Bold);
        g.hit(r, ev.clone(), label);
        bx += w + 8.0 * s;
    }
    bx - x
}

// -------------------------------------------------------------------------------------------- sections, rows, bars

/// Small-caps section title: spaced capitals in a muted tone, then a thin accent rule to the right edge. Returns the
/// height used.
pub fn section_title(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, title: &str, accent: [u8; 4]) -> f32 {
    let sz = size::CAPTION * s;
    let mut cx = x + 4.0 * s;
    for ch in title.to_uppercase().chars() {
        let t = ch.to_string();
        cx += g.text(cx, y, sz, readable(MUTED, rgb(14, 18, 34), 4.5), &t, Face::Bold) + 1.2 * s;
    }
    let ry = y + g.line_height(sz, Face::Bold) / 2.0;
    if x + w - cx - 14.0 * s > 10.0 * s {
        g.gradient_rect(cx + 10.0 * s, ry, x + w - cx - 10.0 * s, 1.0, (cx, 0.0), (x + w, 0.0), &[(0.0, with_alpha(accent, 150)), (1.0, with_alpha(accent, 0))]);
    }
    26.0 * s
}

/// A key and its value on one line: the key in muted text on the left, the value (mono) right-aligned in `value_color`.
pub fn kv_row(g: &mut Gfx, r: R, s: f32, key: &str, value: &str, value_color: [u8; 4]) {
    let kh = g.line_height(14.0 * s, Face::Bold);
    g.text(r.x, r.y + (r.h - kh) / 2.0, 14.0 * s, readable(MUTED, rgb(30, 37, 62), 4.5), key, Face::Bold);
    let v = g.fit(14.0 * s, value, Face::Mono, r.w * 0.6);
    let vw = g.text_width(14.0 * s, &v, Face::Mono);
    g.text(r.x + r.w - vw, r.y + (r.h - g.line_height(14.0 * s, Face::Mono)) / 2.0, 14.0 * s, value_color, &v, Face::Mono);
}

/// A progress bar with a soft glow at its head. `frac` 0..1.
pub fn progress_bar(g: &mut Gfx, r: R, frac: f32, color: [u8; 4], s: f32) {
    let rad = r.h / 2.0;
    g.rrect(r.x, r.y, r.w, r.h, rad, rgb(22, 29, 54));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let w = (r.w * f).max(r.h);
        g.rrect(r.x - 1.0, r.y - 1.0, w + 2.0, r.h + 2.0, rad + 1.0, with_alpha(color, 36));
        g.rrect(r.x, r.y, w, r.h, rad, color);
        g.rect(r.x + rad, r.y + r.h * 0.2, (w - rad * 2.0).max(0.0), (r.h * 0.2).max(1.0), [255, 255, 255, 56]);
        g.circle(r.x + w - rad, r.y + r.h / 2.0, rad + 1.5 * s, with_alpha(color, 50));
    }
}

/// A ring gauge: a dim track and a bright arc from the top, clockwise, `frac` of the way round.
pub fn ring_gauge(g: &mut Gfx, cx: f32, cy: f32, radius: f32, thickness: f32, frac: f32, color: [u8; 4]) {
    g.ring(cx, cy, radius, thickness, rgb(26, 34, 62));
    let f = frac.clamp(0.0, 1.0);
    if f <= 0.0 {
        return;
    }
    let n = ((f * 72.0).ceil() as usize).max(2);
    let a0 = -std::f32::consts::FRAC_PI_2;
    let mut prev = (cx + a0.cos() * radius, cy + a0.sin() * radius);
    for k in 1..=n {
        let a = a0 + f * std::f32::consts::TAU * k as f32 / n as f32;
        let p = (cx + a.cos() * radius, cy + a.sin() * radius);
        g.line(prev.0, prev.1, p.0, p.1, thickness * 2.4, with_alpha(color, 30));
        g.line(prev.0, prev.1, p.0, p.1, thickness, color);
        prev = p;
    }
}

/// An on/off switch drawn inside `r` (the knob slides, the track glows when on).
pub fn switch(g: &mut Gfx, r: R, on: bool, enabled: bool, accent: [u8; 4], s: f32) {
    let rad = r.h / 2.0;
    if !enabled {
        g.rrect(r.x, r.y, r.w, r.h, rad, rgb(34, 40, 64));
        let kx = if on { r.x + r.w - rad } else { r.x + rad };
        g.circle(kx, r.y + rad, rad - 3.0 * s, rgb(112, 122, 150));
        return;
    }
    if on {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 3.0 * s, with_alpha(OK, 70));
        g.rrect(r.x, r.y, r.w, r.h, rad, over(rgb(14, 40, 40), with_alpha(OK, 150)));
        g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.2 * s, OK);
    } else {
        g.rrect(r.x, r.y, r.w, r.h, rad, rgb(26, 33, 60));
        g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.0 * s, rgb(70, 82, 120));
    }
    let kx = if on { r.x + r.w - rad } else { r.x + rad };
    g.circle(kx, r.y + rad, rad - 3.5 * s, if on { TEXT } else { readable(accent, rgb(26, 33, 60), 3.0) });
}

/// A vertical scrollbar in `track` for a list showing `visible` of `total` rows from `first`.
pub fn scrollbar(g: &mut Gfx, track: R, s: f32, first: usize, visible: usize, total: usize, accent: [u8; 4]) {
    if total <= visible || total == 0 {
        return;
    }
    let th = 4.0 * s;
    g.rrect(track.x, track.y, th, track.h, th / 2.0, rgb(20, 26, 48));
    let bh = (track.h * visible as f32 / total as f32).max(18.0 * s);
    let by = track.y + (track.h - bh) * (first as f32 / (total - visible).max(1) as f32).clamp(0.0, 1.0);
    g.rrect(track.x, by, th, bh, th / 2.0, with_alpha(accent, 190));
}

/// A table's header line: capital column names in muted text, a hairline under them. `cols` are (name, x offset).
pub fn table_header(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, cols: &[(&str, f32)], accent: [u8; 4]) -> f32 {
    for (t, off) in cols {
        g.text(x + off * s, y, size::TINY * s, readable(MUTED, rgb(14, 18, 34), 4.5), &t.to_uppercase(), Face::Bold);
    }
    g.gradient_rect(x, y + 17.0 * s, w, 1.0, (x, 0.0), (x + w, 0.0), &[(0.0, with_alpha(accent, 140)), (1.0, with_alpha(accent, 10))]);
    20.0 * s
}

/// An empty list: an icon in a soft ring, a title and a hint, centred in `r`.
pub fn empty_state(g: &mut Gfx, r: R, s: f32, icon: Icon, title: &str, hint: &str, accent: [u8; 4]) {
    let cx = r.x + r.w / 2.0;
    let cy = r.y + (r.h / 2.0 - 40.0 * s).max(40.0 * s);
    g.radial_glow(cx, cy, 70.0 * s, with_alpha(accent, 30));
    g.ring(cx, cy, 28.0 * s, 1.2 * s, with_alpha(accent, 130));
    g.icon(icon, cx, cy, 26.0 * s, accent);
    let t = g.fit(17.0 * s, title, Face::Bold, r.w - 40.0 * s);
    let tw = g.text_width(17.0 * s, &t, Face::Bold);
    g.text(cx - tw / 2.0, cy + 42.0 * s, 17.0 * s, TEXT, &t, Face::Bold);
    let mut y = cy + 70.0 * s;
    for line in g.wrap(14.0 * s, hint, Face::Sans, (r.w - 60.0 * s).min(560.0 * s)).into_iter().take(3) {
        let lw = g.text_width(14.0 * s, &line, Face::Sans);
        g.text(cx - lw / 2.0, y, 14.0 * s, readable(MUTED, rgb(14, 18, 34), 4.5), &line, Face::Sans);
        y += 20.0 * s;
    }
}

/// The answer to the last action, one line: a coloured dot and the text.
pub fn status_line(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, text: &str, ok: bool) {
    let c = if ok { OK } else { ERR };
    // every app's failure line: an I/O error caused by a lost boot drive names the drive (iyakki_storage::loss, healthy: unchanged)
    let text = if ok { text.to_string() } else { iyakki_storage::loss::reword(text) };
    let text = text.as_str();
    // not colour alone: a tick in a disc for success, an exclamation mark in a disc for a problem
    let (cx, cy) = (x + 7.0 * s, y + 8.0 * s);
    g.circle(cx, cy, 9.0 * s, with_alpha(c, 40));
    g.circle(cx, cy, 7.0 * s, c);
    let ink = rgb(10, 14, 26);
    if ok {
        g.line(cx - 3.2 * s, cy + 0.2 * s, cx - 1.0 * s, cy + 2.6 * s, 1.8 * s, ink);
        g.line(cx - 1.0 * s, cy + 2.6 * s, cx + 3.4 * s, cy - 2.6 * s, 1.8 * s, ink);
    } else {
        g.line(cx, cy - 4.0 * s, cx, cy + 1.2 * s, 1.9 * s, ink);
        g.circle(cx, cy + 3.5 * s, 1.1 * s, ink);
    }
    let t = g.fit(14.0 * s, text, Face::Sans, w - 24.0 * s);
    g.text(x + 22.0 * s, y, 14.0 * s, c, &t, Face::Sans);
}

/// A permission/feature toggle chip (label, on or off): green neon when on, a dim outline when off, accent outline on hover.
/// Draws only; the caller registers the hit with the same rectangle (x, y, returned width, 34 * s).
pub fn toggle_chip(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, on: bool, accent: [u8; 4]) -> f32 {
    let w = g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s;
    let h = 34.0 * s;
    let hot = g.hovered(R::new(x, y, w, h));
    if on {
        g.rrect(x, y, w, h, h / 2.0, over(rgb(10, 22, 30), with_alpha(OK, 56)));
        g.rrect_outline(x, y, w, h, h / 2.0, 1.2 * s, with_alpha(OK, if hot { 255 } else { 170 }));
    } else {
        g.rrect(x, y, w, h, h / 2.0, rgb(24, 22, 36));
        g.rrect_outline(x, y, w, h, h / 2.0, 1.0, if hot { with_alpha(accent, 200) } else { rgb(70, 60, 84) });
    }
    g.text(x + 14.0 * s, y + (h - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, if on { OK } else { readable(MUTED, rgb(24, 22, 36), 4.5) }, label, Face::Bold);
    w
}

/// A launcher tile: a big glass card with a soft shadow, a hairline in the app's colour, and a neon outline with a glow
/// when it is the chosen one. `allowed` false greys the outline out.
pub fn tile(g: &mut Gfx, r: R, s: f32, accent: [u8; 4], selected: bool, hot: bool, allowed: bool) {
    let rad = 26.0 * s;
    g.rrect(r.x + 4.0 * s, r.y + 10.0 * s, r.w, r.h, rad, [0, 0, 0, 120]);
    let a = if allowed { accent } else { DIM };
    if selected {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 10.0 * s, with_alpha(a, 80));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, if selected || hot { CARD_HOVER } else { CARD });
    g.rect(r.x + rad * 0.7, r.y + 1.0, r.w - rad * 1.4, 1.0, [255, 255, 255, 18]);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, if selected { 2.5 * s } else { 1.0 }, with_alpha(a, if selected { 255 } else if hot { 130 } else { 46 }));
}

/// The DEMO ribbon of a launcher tile: a small amber pill that straddles the tile's top edge, centred, so it reads in
/// every group and at every size. Returns its rectangle.
pub fn demo_ribbon(g: &mut Gfx, r: R, s: f32) -> R {
    ribbon(g, r, s, "DEMO")
}

/// The amber ribbon across the top edge of a tile, with a short word on it (DEMO, DEV).
pub fn ribbon(g: &mut Gfx, r: R, s: f32, word: &str) -> R {
    let (w, h) = (r.w.min(66.0 * s).max(30.0), 19.0 * s);
    let x = r.x + (r.w - w) / 2.0;
    let y = r.y - h * 0.45;
    g.rrect_glow(x, y, w, h, h / 2.0, 4.0 * s, with_alpha(AMBER, 90));
    g.rrect(x, y, w, h, h / 2.0, AMBER);
    g.rrect_outline(x, y, w, h, h / 2.0, 1.0, [255, 232, 170, 255]);
    let sz = 12.0 * s;
    let tw = g.text_width(sz, word, Face::Bold);
    g.text(x + (w - tw) / 2.0, y + (h - g.line_height(sz, Face::Bold)) / 2.0, sz, rgb(40, 24, 0), word, Face::Bold);
    R::new(x, y, w, h)
}

/// A small square-ish button in the app header (clipboard, camera, help, back): dark glass with a hairline, accent on hover.
pub fn chrome_button(g: &mut Gfx, r: R, s: f32, hot: bool, accent: [u8; 4]) {
    let rad = 8.0 * s;
    if hot {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 3.0 * s, with_alpha(accent, 70));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, if hot { rgb(36, 46, 80) } else { rgb(22, 29, 54) });
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.0, if hot { with_alpha(accent, 200) } else { rgb(52, 62, 96) });
}

/// The floating message pill (bottom centre of the screen).
pub fn toast(g: &mut Gfx, w: f32, h: f32, s: f32, text: &str, info: bool) {
    let tw = g.text_width(17.0 * s, text, Face::Sans) + 44.0 * s;
    let (x, y) = ((w - tw) / 2.0, h - 90.0 * s);
    let c = if info { OK } else { ERR };
    g.rrect_glow(x, y, tw, 46.0 * s, 23.0 * s, 6.0 * s, with_alpha(c, 70));
    g.rrect(x, y, tw, 46.0 * s, 23.0 * s, if info { rgb(14, 40, 36) } else { rgb(52, 26, 36) });
    g.rrect_outline(x, y, tw, 46.0 * s, 23.0 * s, 1.5 * s, c);
    g.circle(x + 20.0 * s, y + 23.0 * s, 4.0 * s, c);
    g.text(x + 32.0 * s, y + 12.0 * s, 17.0 * s, TEXT, text, Face::Sans);
}

// ---------------------------------------------------------------------------------------------------------- dialogs

/// The frame of a modal dialog (forms, pickers, confirmations): the UI behind is dimmed and made unclickable, and a glass
/// panel with a glowing accent border is drawn. `danger` makes the border red. Returns the panel's rectangle (centred in
/// `area`, `w` by `h`, clamped to fit). Callers draw their content inside it.
pub fn modal(g: &mut Gfx, area: R, s: f32, w: f32, h: f32, accent: [u8; 4], danger: bool) -> R {
    g.clear_hits();
    g.rect(area.x, area.y, area.w, area.h, [3, 5, 14, 190]);
    g.hit(area, Event::Tick, "backdrop");
    let w = w.min(area.w - 40.0 * s);
    let h = h.min(area.h - 16.0 * s);
    let (x, y) = (area.x + (area.w - w) / 2.0, area.y + ((area.h - h) / 2.0).max(8.0 * s));
    modal_panel(g, R::new(x, y, w, h), s, accent, danger);
    R::new(x, y, w, h)
}

/// Just the picture of a dialog panel at `r` (shadow, glow, glass, border), for screens that handle their own click regions.
pub fn modal_panel(g: &mut Gfx, r: R, s: f32, accent: [u8; 4], danger: bool) {
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    let c = if danger { ERR } else { accent };
    let rad = 18.0 * s;
    g.rrect(x, y + 6.0 * s, w, h, rad, [0, 0, 0, 90]);
    g.rrect_glow(x, y, w, h, rad, 10.0 * s, with_alpha(c, 80));
    g.rrect(x, y, w, h, rad, if danger { rgb(30, 18, 30) } else { rgb(17, 22, 42) });
    g.rect(x + rad * 0.6, y + 1.0, w - rad * 1.2, 1.0, [255, 255, 255, 18]);
    g.rrect_outline(x, y, w, h, rad, 1.6 * s, c);
}

/// A dialog's title with a short accent rule under it. Returns the y where content can start.
pub fn modal_title(g: &mut Gfx, p: R, s: f32, title: &str, accent: [u8; 4]) -> f32 {
    let t = g.fit(21.0 * s, title, Face::Bold, p.w - 52.0 * s);
    g.text(p.x + 26.0 * s, p.y + 18.0 * s, 21.0 * s, TEXT, &t, Face::Bold);
    g.gradient_rect(p.x + 26.0 * s, p.y + 48.0 * s, 70.0 * s, 2.0 * s, (p.x + 26.0 * s, 0.0), (p.x + 96.0 * s, 0.0), &[(0.0, accent), (1.0, with_alpha(accent, 0))]);
    p.y + 56.0 * s
}

/// The drawn picture of a text field's frame (used by the shared field renderer): `focused` lights the border.
pub fn field_frame(g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
    let rad = size::R_FIELD * s;
    if focused {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 3.0 * s, with_alpha(accent, 60));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, crate::ui::FIELD);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, if focused { 1.8 * s * crate::theme::ring() } else { 1.0 }, if focused { accent } else { rgb(62, 72, 108) });
    if !focused {
        always_border(g, r.x, r.y, r.w, r.h, rad, s);
    }
}

// ------------------------------------------------------------------------------------------------------------ glyphs

/// Small vector pictures for tabs and buttons (the AI IDE and its cluster view), drawn with strokes and polygons so they
/// are crisp at 14-18 px, need no font and take the colour of the text next to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Editor,
    Board,
    K8s,
    Terminal,
    Problems,
    Search,
    Files,
    Panel,
    Chat,
    Run,
    Tests,
    Open,
    Workloads,
    Services,
    Ingresses,
    Pods,
    Config,
    Storage,
    Nodes,
    Refresh,
    Namespace,
    Cluster,
    Registry,
    Deploy,
    Restart,
    Globe,
    /// A page with a folded corner and lines: the Notes tab.
    Note,
    /// A branch: two commits on a line and a fork: the Git tab.
    Git,
}

impl Glyph {
    pub const ALL: [Glyph; 28] = [
        Glyph::Editor,
        Glyph::Board,
        Glyph::K8s,
        Glyph::Terminal,
        Glyph::Problems,
        Glyph::Search,
        Glyph::Files,
        Glyph::Panel,
        Glyph::Chat,
        Glyph::Run,
        Glyph::Tests,
        Glyph::Open,
        Glyph::Workloads,
        Glyph::Services,
        Glyph::Ingresses,
        Glyph::Pods,
        Glyph::Config,
        Glyph::Storage,
        Glyph::Nodes,
        Glyph::Refresh,
        Glyph::Namespace,
        Glyph::Cluster,
        Glyph::Registry,
        Glyph::Deploy,
        Glyph::Restart,
        Glyph::Globe,
        Glyph::Note,
        Glyph::Git,
    ];
}

/// Draw `glyph` centred on (`cx`, `cy`) inside a `size` x `size` square, in colour `c`.
pub fn glyph(g: &mut Gfx, glyph: Glyph, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let (x0, y0) = (cx - size / 2.0, cy - size / 2.0);
    let lw = (size * 0.1).max(1.4);
    // a point of the unit square
    let p = |x: f32, y: f32| (x0 + x * size, y0 + y * size);
    let poly = |g: &mut Gfx, pts: &[(f32, f32)], close: bool| {
        let n = pts.len();
        for i in 0..n.saturating_sub(1) + usize::from(close && n > 2) {
            let (a, b) = (p(pts[i].0, pts[i].1), p(pts[(i + 1) % n].0, pts[(i + 1) % n].1));
            g.line(a.0, a.1, b.0, b.1, lw, c);
        }
    };
    // an arc of the ellipse at (ex, ey) with radii (rx, ry), from angle a to b in degrees (0 = right, 90 = down)
    let arc = |ex: f32, ey: f32, rx: f32, ry: f32, a: f32, b: f32| -> Vec<(f32, f32)> {
        let n = (((b - a).abs() / 18.0).ceil() as usize).max(2);
        (0..=n)
            .map(|i| {
                let t = (a + (b - a) * i as f32 / n as f32).to_radians();
                (ex + rx * t.cos(), ey + ry * t.sin())
            })
            .collect()
    };
    let hexagon = |r: f32| -> Vec<(f32, f32)> {
        (0..6)
            .map(|i| {
                let t = (-90.0 + 60.0 * i as f32).to_radians();
                (0.5 + r * t.cos(), 0.5 + r * t.sin())
            })
            .collect()
    };
    match glyph {
        Glyph::Editor => {
            poly(g, &[(0.36, 0.25), (0.1, 0.5), (0.36, 0.75)], false);
            poly(g, &[(0.64, 0.25), (0.9, 0.5), (0.64, 0.75)], false);
            poly(g, &[(0.56, 0.2), (0.44, 0.8)], false);
        }
        Glyph::Board => {
            for (x, h) in [(0.1, 0.7), (0.4, 0.5), (0.7, 0.62)] {
                let (a, w, hh) = (p(x, 0.15), 0.2 * size, h * size);
                g.rrect_outline(a.0, a.1, w, hh, 1.2, lw * 0.9, c);
            }
        }
        Glyph::K8s => {
            poly(g, &hexagon(0.44), true);
            let ctr = p(0.5, 0.5);
            g.ring(ctr.0, ctr.1, size * 0.13, lw * 0.9, c);
            for i in 0..7 {
                let t = (-90.0 + 360.0 / 7.0 * i as f32).to_radians();
                let (a, b) = (p(0.5 + 0.13 * t.cos(), 0.5 + 0.13 * t.sin()), p(0.5 + 0.27 * t.cos(), 0.5 + 0.27 * t.sin()));
                g.line(a.0, a.1, b.0, b.1, lw * 0.8, c);
            }
        }
        Glyph::Terminal => {
            poly(g, &[(0.14, 0.28), (0.44, 0.5), (0.14, 0.72)], false);
            poly(g, &[(0.54, 0.76), (0.88, 0.76)], false);
        }
        Glyph::Problems => {
            poly(g, &[(0.5, 0.1), (0.93, 0.86), (0.07, 0.86)], true);
            poly(g, &[(0.5, 0.38), (0.5, 0.6)], false);
            let d = p(0.5, 0.74);
            g.circle(d.0, d.1, lw * 0.6, c);
        }
        Glyph::Search => {
            let r = p(0.42, 0.42);
            g.ring(r.0, r.1, size * 0.27, lw, c);
            poly(g, &[(0.62, 0.62), (0.9, 0.9)], false);
        }
        Glyph::Git => {
            poly(g, &[(0.3, 0.2), (0.3, 0.8)], false);
            poly(g, &[(0.3, 0.55), (0.7, 0.4)], false);
            for (x, y) in [(0.3, 0.2), (0.3, 0.8), (0.7, 0.4)] {
                let d = p(x, y);
                g.ring(d.0, d.1, size * 0.1, lw * 0.9, c);
            }
        }
        Glyph::Note => {
            poly(g, &[(0.2, 0.1), (0.62, 0.1), (0.84, 0.32), (0.84, 0.9), (0.2, 0.9)], true);
            poly(g, &[(0.62, 0.1), (0.62, 0.32), (0.84, 0.32)], false);
            poly(g, &[(0.32, 0.5), (0.72, 0.5)], false);
            poly(g, &[(0.32, 0.68), (0.72, 0.68)], false);
        }
        Glyph::Files => poly(g, &[(0.08, 0.2), (0.4, 0.2), (0.5, 0.32), (0.92, 0.32), (0.92, 0.82), (0.08, 0.82)], true),
        Glyph::Panel => {
            let a = p(0.08, 0.15);
            g.rrect_outline(a.0, a.1, 0.84 * size, 0.7 * size, 1.5, lw * 0.9, c);
            poly(g, &[(0.08, 0.62), (0.92, 0.62)], false);
        }
        Glyph::Chat => {
            let a = p(0.08, 0.14);
            g.rrect_outline(a.0, a.1, 0.84 * size, 0.56 * size, 2.5, lw * 0.9, c);
            poly(g, &[(0.26, 0.7), (0.22, 0.9), (0.46, 0.7)], false);
        }
        Glyph::Run => g.polygon(&[p(0.24, 0.12), p(0.86, 0.5), p(0.24, 0.88)], c),
        Glyph::Tests => poly(g, &[(0.12, 0.54), (0.38, 0.78), (0.88, 0.24)], false),
        Glyph::Open => {
            poly(g, &[(0.42, 0.18), (0.14, 0.18), (0.14, 0.86), (0.82, 0.86), (0.82, 0.58)], false);
            poly(g, &[(0.46, 0.54), (0.88, 0.12)], false);
            poly(g, &[(0.62, 0.12), (0.88, 0.12), (0.88, 0.38)], false);
        }
        Glyph::Workloads => {
            for y in [0.14, 0.4, 0.66] {
                let a = p(0.1, y);
                g.rrect_outline(a.0, a.1, 0.8 * size, 0.2 * size, 1.5, lw * 0.9, c);
            }
        }
        Glyph::Services => {
            poly(g, &[(0.1, 0.34), (0.9, 0.34)], false);
            poly(g, &[(0.7, 0.14), (0.9, 0.34), (0.7, 0.54)], false);
            poly(g, &[(0.9, 0.7), (0.1, 0.7)], false);
            poly(g, &[(0.3, 0.5), (0.1, 0.7), (0.3, 0.9)], false);
        }
        Glyph::Ingresses => {
            let a = p(0.5, 0.16);
            g.rrect_outline(a.0, a.1, 0.4 * size, 0.68 * size, 1.5, lw * 0.9, c);
            poly(g, &[(0.06, 0.5), (0.7, 0.5)], false);
            poly(g, &[(0.5, 0.3), (0.7, 0.5), (0.5, 0.7)], false);
        }
        Glyph::Pods => {
            let a = p(0.1, 0.2);
            g.rrect_outline(a.0, a.1, 0.8 * size, 0.6 * size, 0.2 * size, lw * 0.9, c);
            for x in [0.3, 0.5, 0.7] {
                let d = p(x, 0.5);
                g.circle(d.0, d.1, lw * 0.6, c);
            }
        }
        Glyph::Config => {
            for (y, k) in [(0.22, 0.34), (0.5, 0.66), (0.78, 0.42)] {
                poly(g, &[(0.1, y), (0.9, y)], false);
                let d = p(k, y);
                g.circle(d.0, d.1, size * 0.1, c);
            }
        }
        Glyph::Storage => {
            poly(g, &[(0.16, 0.24), (0.16, 0.76)], false);
            poly(g, &[(0.84, 0.24), (0.84, 0.76)], false);
            poly(g, &arc(0.5, 0.24, 0.34, 0.14, 0.0, 360.0), true);
            poly(g, &arc(0.5, 0.5, 0.34, 0.14, 0.0, 180.0), false);
            poly(g, &arc(0.5, 0.76, 0.34, 0.14, 0.0, 180.0), false);
        }
        Glyph::Nodes => {
            for y in [0.14, 0.56] {
                let a = p(0.1, y);
                g.rrect_outline(a.0, a.1, 0.8 * size, 0.3 * size, 1.5, lw * 0.9, c);
                let d = p(0.26, y + 0.15);
                g.circle(d.0, d.1, lw * 0.6, c);
            }
        }
        Glyph::Refresh => {
            poly(g, &arc(0.5, 0.5, 0.34, 0.34, -40.0, 250.0), false);
            g.polygon(&[p(0.9, 0.12), p(0.92, 0.5), p(0.55, 0.38)], c);
        }
        Glyph::Namespace => {
            poly(g, &[(0.3, 0.14), (0.12, 0.14), (0.12, 0.86), (0.3, 0.86)], false);
            poly(g, &[(0.7, 0.14), (0.88, 0.14), (0.88, 0.86), (0.7, 0.86)], false);
            let d = p(0.5, 0.5);
            g.circle(d.0, d.1, size * 0.1, c);
        }
        Glyph::Cluster => {
            let ctr = p(0.5, 0.5);
            g.ring(ctr.0, ctr.1, size * 0.4, lw * 0.9, c);
            poly(g, &[(0.1, 0.5), (0.9, 0.5)], false);
            poly(g, &arc(0.5, 0.5, 0.18, 0.4, 0.0, 360.0), true);
        }
        Glyph::Registry => {
            poly(g, &hexagon(0.44), true);
            poly(g, &[(0.5, 0.5), (0.5, 0.94)], false);
            poly(g, &[(0.5, 0.5), (0.12, 0.28)], false);
            poly(g, &[(0.5, 0.5), (0.88, 0.28)], false);
        }
        Glyph::Deploy => {
            poly(g, &[(0.5, 0.7), (0.5, 0.1)], false);
            poly(g, &[(0.26, 0.34), (0.5, 0.1), (0.74, 0.34)], false);
            poly(g, &[(0.12, 0.66), (0.12, 0.88), (0.88, 0.88), (0.88, 0.66)], false);
        }
        Glyph::Restart => {
            poly(g, &arc(0.5, 0.56, 0.32, 0.32, -55.0, 235.0), false);
            poly(g, &[(0.5, 0.1), (0.5, 0.5)], false);
        }
        // a globe: the outline, the equator, a parallel above and below, and the meridian
        Glyph::Globe => {
            let ctr = p(0.5, 0.5);
            g.ring(ctr.0, ctr.1, size * 0.42, lw * 0.9, c);
            poly(g, &[(0.08, 0.5), (0.92, 0.5)], false);
            poly(g, &[(0.17, 0.28), (0.83, 0.28)], false);
            poly(g, &[(0.17, 0.72), (0.83, 0.72)], false);
            poly(g, &arc(0.5, 0.5, 0.2, 0.42, 0.0, 360.0), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_maths() {
        assert_eq!(mix([0, 0, 0, 255], [200, 100, 50, 255], 0.5), [100, 50, 25, 255]);
        assert_eq!(mix([10, 10, 10, 255], [20, 20, 20, 255], 9.0), [20, 20, 20, 255], "t is clamped");
        assert_eq!(with_alpha([1, 2, 3, 255], 9), [1, 2, 3, 9]);
        assert_eq!(over([0, 0, 0, 255], [200, 200, 200, 128]), [100, 100, 100, 255]);
        assert!((contrast([255, 255, 255, 255], [0, 0, 0, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([10, 20, 30, 255], [10, 20, 30, 255]) - 1.0).abs() < 1e-4);
        let c = readable([60, 70, 100, 255], CARD, 4.5);
        assert!(contrast(c, CARD) >= 4.5, "{c:?}");
        let already = readable(TEXT, CARD, 4.5);
        assert_eq!(already, [TEXT[0], TEXT[1], TEXT[2], 255], "bright text is left alone");
    }

    #[test]
    fn the_house_text_colours_read_on_cards() {
        for bg in [CARD, CARD_HOVER, PANEL, WELL, BG_TOP, rgb(10, 14, 30)] {
            assert!(contrast(TEXT, bg) >= 7.0, "TEXT on {bg:?}");
            assert!(contrast(MUTED, bg) >= 4.5, "MUTED on {bg:?}: {}", contrast(MUTED, bg));
        }
        assert!(contrast(DIM, CARD) >= 4.5, "DIM on CARD: {}", contrast(DIM, CARD));
        assert!(contrast(DIM, BG_TOP) >= 4.5, "DIM on the background: {}", contrast(DIM, BG_TOP));
        for accent in [CYAN, MAGENTA, OK, ERR, AMBER] {
            assert!(contrast(accent, CARD) >= 4.5, "{accent:?} on CARD");
        }
    }

    #[test]
    fn the_background_is_drawn_once_per_size() {
        let mut g = Gfx::new(320, 200);
        background(&mut g);
        assert_eq!(background_cached_size(), Some((320, 200)));
        let first = g.rgba().to_vec();
        g.rect(0.0, 0.0, 50.0, 50.0, rgb(255, 0, 0));
        background(&mut g);
        assert_eq!(g.rgba(), &first[..], "the second call restores the cached picture exactly");
        let mut big = Gfx::new(400, 300);
        background(&mut big);
        assert_eq!(background_cached_size(), Some((400, 300)), "a new size builds a new picture");
        assert_eq!(big.rgba().len(), 400 * 300 * 4);
        // it is a backdrop, not blank: it varies from top to bottom and has the grid
        let px = |g: &Gfx, x: usize, y: usize| g.rgba()[(y * g.width() as usize + x) * 4..][..3].to_vec();
        assert_ne!(px(&big, 5, 1), px(&big, 5, 298));
    }

    #[test]
    fn variants_follow_the_accent() {
        assert_eq!(Variant::of(ERR), Variant::Danger);
        assert_eq!(Variant::of(MUTED), Variant::Secondary);
        assert_eq!(Variant::of(CYAN), Variant::Primary);
    }

    #[test]
    fn drawing_helpers_register_only_what_they_are_told_to() {
        let mut g = Gfx::new(900, 400);
        g.clear(rgb(9, 11, 20));
        let r = R::new(20.0, 20.0, 300.0, 80.0);
        card(&mut g, r, 1.0, CYAN, 0.5);
        panel(&mut g, r, 1.0, CYAN);
        well(&mut g, r, 1.0);
        list_row(&mut g, r, 1.0, true, false, CYAN);
        list_row(&mut g, r, 1.0, false, true, CYAN);
        chip(&mut g, 10.0, 10.0, 1.0, "LIVE", MAGENTA, true);
        section_title(&mut g, 10.0, 10.0, 300.0, 1.0, "Retrieval", CYAN);
        progress_bar(&mut g, r, 0.4, CYAN, 1.0);
        ring_gauge(&mut g, 100.0, 100.0, 30.0, 4.0, 0.7, MAGENTA);
        switch(&mut g, R::new(10.0, 10.0, 52.0, 28.0), true, true, CYAN, 1.0);
        scrollbar(&mut g, r, 1.0, 3, 10, 40, CYAN);
        table_header(&mut g, 10.0, 10.0, 300.0, 1.0, &[("PID", 0.0), ("USER", 70.0)], CYAN);
        empty_state(&mut g, r, 1.0, Icon::Radio, "Nothing here", "Add something to get started.", CYAN);
        status_line(&mut g, 10.0, 10.0, 300.0, 1.0, "saved", true);
        toast(&mut g, 900.0, 400.0, 1.0, "Done", true);
        assert!(g.hits.is_empty(), "decoration never adds click regions: {:?}", g.hits.len());
    }

    #[test]
    fn tab_strip_and_modal_register_exactly_the_regions_asked_for() {
        let mut g = Gfx::new(900, 400);
        g.clear(rgb(9, 11, 20));
        let items = vec![("Overview".to_string(), Event::Tab(0)), ("Processes".to_string(), Event::Tab(1)), ("Agents".to_string(), Event::Tab(2))];
        let w = tab_strip(&mut g, 10.0, 10.0, 1.0, 800.0, &items, 1, CYAN);
        assert!(w > 100.0);
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["Overview", "Processes", "Agents"]);
        assert_eq!(g.hits[1].ev, Event::Tab(1));
        // each tab's region is where its pill is, and they do not overlap
        assert!(g.hits[0].rect.x + g.hits[0].rect.w < g.hits[1].rect.x);
        // a strip that is too narrow drops the tabs that do not fit instead of overflowing
        g.hits.clear();
        tab_strip(&mut g, 10.0, 10.0, 1.0, 200.0, &items, 0, CYAN);
        assert!(g.hits.len() < 3 && !g.hits.is_empty());
        // a modal clears the UI behind it and leaves a single backdrop region
        g.hit(R::new(0.0, 0.0, 10.0, 10.0), Event::Tick, "behind");
        let p = modal(&mut g, R::new(0.0, 0.0, 900.0, 400.0), 1.0, 600.0, 200.0, CYAN, false);
        assert_eq!(g.hits.len(), 1);
        assert_eq!(g.hits[0].label, "backdrop");
        assert!(p.x > 0.0 && p.x + p.w < 900.0);
        let small = modal(&mut g, R::new(0.0, 0.0, 300.0, 100.0), 1.0, 600.0, 400.0, CYAN, true);
        assert!(small.w <= 300.0 && small.h <= 100.0, "a modal never outgrows its area: {small:?}");
    }

    #[test]
    fn the_header_accent_line_moves_with_the_tick_unless_motion_is_reduced() {
        let draw = |tick: u64| {
            let mut g = Gfx::new(800, 60);
            g.clear(rgb(0, 0, 0));
            app_header(&mut g, 800.0, 46.0, 1.0, "RADIO", "Online stations", Icon::Radio, CYAN, tick, 600.0);
            g.rgba().to_vec()
        };
        assert_ne!(draw(0), draw(40), "the highlight slides");
        set_reduced_motion(true);
        assert_eq!(draw(0), draw(40), "reduced motion freezes it");
        set_reduced_motion(false);
    }

    #[test]
    fn buttons_keep_their_size_and_label_when_restyled() {
        let mut g = Gfx::new(600, 200);
        let w = g.button(10.0, 10.0, 14.0, "Save", CYAN, Event::Key(crate::ui::Key::Ctrl('s')));
        assert!((w - (g.text_width(14.0, "Save", Face::Bold) + 14.0 * 1.7)).abs() < 0.01);
        assert_eq!(g.hits.len(), 1);
        assert_eq!(g.hits[0].label, "Save");
        assert!((g.hits[0].rect.h - 14.0 * 2.1).abs() < 0.01);
    }

    #[test]
    fn every_glyph_draws_inside_its_box_at_every_size() {
        for size in [14.0f32, 15.0, 16.0, 18.0] {
            for gl in Glyph::ALL {
                let mut g = Gfx::new(60, 60);
                g.clear(rgb(0, 0, 0));
                glyph(&mut g, gl, 30.0, 30.0, size, rgb(255, 255, 255));
                let (mut lit, mut outside) = (0, 0);
                for y in 0..60u32 {
                    for x in 0..60u32 {
                        if g.rgba()[((y * 60 + x) * 4) as usize] > 40 {
                            lit += 1;
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                            if (px - 30.0).abs() > size / 2.0 + 1.5 || (py - 30.0).abs() > size / 2.0 + 1.5 {
                                outside += 1;
                            }
                        }
                    }
                }
                assert!(lit as f32 > size * 1.5, "{gl:?} at {size} draws something ({lit} px)");
                assert_eq!(outside, 0, "{gl:?} at {size} stays in its box");
            }
        }
    }
}
