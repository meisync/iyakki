//! The home screen's app icons and the frosted island they sit on (docs/design/launcher).
//!
//! An icon is a squircle in the app's own hue (a gradient from a lighter top to a deeper bottom), a gloss over its upper half, a rim light,
//! and a white glyph. **The state never recolours the icon**: it lives on the corner badge ([`badge`]) and the chip under the label
//! ([`chip`]). Everything that is "the app's hue" is drawn with raw colours (the same in Dark, Light and High contrast); everything that
//! is the surface or the state is a theme colour, so text stays readable in all three.
//!
//! The island is a translucent panel (the wallpaper blurred once into a cached picture sits behind it, `crate::blur`): 40 px radius, a white rim, a gloss band down
//! the top and a soft ambient shadow.

use crate::fx::{contrast, mix, with_alpha};
use crate::gfx::{self, Face, Gfx, Icon};
use crate::look::{IconShape, IconStyle};
use tiny_skia::{Path, PathBuilder, Rect};
use crate::theme;
use crate::ui::{self, R};

/// What an app's icon shows about the app, besides its name. Never changes the icon's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    /// Nothing to say: no chip.
    Ready,
    /// Doing something now (a station playing).
    Running,
    /// Needs a one-time step from the person (no camera, updates waiting).
    Setup,
    /// Something is wrong or waiting on the person.
    Attention,
    /// The signed-in person's role does not allow it.
    Locked,
}

impl AppState {
    /// The chip's words (`None`: no chip).
    pub fn chip_text(self) -> Option<&'static str> {
        match self {
            AppState::Ready => None,
            AppState::Running => Some("RUNNING"),
            AppState::Setup => Some("NEEDS SETUP"),
            AppState::Attention => Some("ATTENTION"),
            AppState::Locked => Some("LOCKED"),
        }
    }

    /// The theme colour that carries the state (text and tint of the chip, fill of the badge).
    pub fn colour(self) -> [u8; 4] {
        match self {
            AppState::Ready => ui::MUTED,
            AppState::Running => ui::ACCENT_MAIN,
            AppState::Setup => crate::fx::AMBER,
            AppState::Attention => ui::ERR,
            AppState::Locked => ui::DIM,
        }
    }
}

/// The two ends of an icon's gradient for the hue `accent`: lighter on top, deeper below, the bottom dark enough that the white glyph
/// reads (3:1 at least, 4.5:1 in High contrast).
pub fn hue_gradient(accent: [u8; 4]) -> ([u8; 4], [u8; 4]) {
    let white = [255, 255, 255, 255];
    let want = if theme::is_high_contrast() { 4.5 } else { 3.0 };
    let mut deep = accent;
    deep[3] = 255;
    let mut k = 0.0f32;
    while contrast(white, deep) < want && k < 0.9 {
        k += 0.06;
        deep = mix(accent, [0, 0, 0, 255], k);
    }
    // the top is a little lighter than the base, but never so light that the glyph loses the contrast
    let top = mix(deep, white, 0.20);
    let top = if contrast(white, top) >= want * 0.8 { top } else { mix(deep, white, 0.08) };
    (top, deep)
}

/// A grey-green-blue-less hue for an app that is not allowed: the icon is dimmed, never recoloured by state.
fn dimmed(c: [u8; 4], allowed: bool) -> [u8; 4] {
    if allowed {
        c
    } else {
        mix(c, [120, 124, 134, 255], 0.65)
    }
}

/// The frosted island: a translucent panel with rim, gloss band and an ambient shadow. `pop` (0.975..1) scales it around its middle.
/// At rest (`pop` 1) a finished island is kept and put back while nothing under it or in the settings changed (`crate::memo`: about 20 ms
/// of a 1080p frame painted the shadow rings, the frost, the fill, the gloss and the rim again, F-SHELL-64).
pub fn island(g: &mut Gfx, r: R, s: f32, pop: f32) {
    let _p = crate::prof::scope("appicon.island");
    if pop != 1.0 {
        return paint_island(g, r, s, pop);
    }
    let frost = if theme::is_high_contrast() { None } else { crate::blur::current() };
    let key = crate::memo::key(&[
        1,
        r.x.to_bits() as u64,
        r.y.to_bits() as u64,
        r.w.to_bits() as u64,
        r.h.to_bits() as u64,
        s.to_bits() as u64,
        theme::generation() as u64,
        crate::wallpaper::generation() as u64,
        frost.as_ref().map_or(0, |_| 1 + crate::blur::builds() as u64),
        dark() as u64,
        theme::is_high_contrast() as u64,
        theme::ring().to_bits() as u64,
        g.width() as u64,
    ]);
    let bbox = ((r.x - 24.0 * s).floor() as i32, (r.y - 4.0 * s).floor() as i32, (r.w + 48.0 * s).ceil() as i32, (r.h + 56.0 * s).ceil() as i32);
    crate::memo::draw(g, key, bbox, |g| paint_island(g, r, s, pop));
}

fn paint_island(g: &mut Gfx, r: R, s: f32, pop: f32) {
    let c = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let r = R::new(c.0 - r.w * pop / 2.0, c.1 - r.h * pop / 2.0, r.w * pop, r.h * pop);
    let rad = 40.0 * s;
    // ambient shadow: wide and soft (a few growing translucent rings), a tight one near the edge
    for (d, a) in [(46.0f32, 7u8), (32.0, 9), (20.0, 12), (10.0, 14)] {
        g.rrect(r.x - d * s * 0.4, r.y + (d * 0.9) * s, r.w + d * s * 0.8, r.h, rad + d * s * 0.4, [0, 0, 0, a]);
    }
    // the panel: the theme's card colour, see-through (the wallpaper and its blur arrive with the next slices)
    // with the frost (the wallpaper blurred once, `crate::blur`) behind it the panel can be a little more see-through
    let frost = if theme::is_high_contrast() { None } else { crate::blur::current() };
    match &frost {
        Some(f) => {
            g.rrect_picture(r.x, r.y, r.w, r.h, rad, &f.pm, f.factor);
            g.rrect(r.x, r.y, r.w, r.h, rad, with_alpha(t_card(), island_alpha_frosted()));
        }
        None => g.rrect(r.x, r.y, r.w, r.h, rad, with_alpha(t_card(), island_alpha())),
    }
    // gloss band down the top
    g.rrect_gradient(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h * 0.42, rad - 1.0, &[(0.0, [255, 255, 255, if dark() { 26 } else { 130 }]), (1.0, [255, 255, 255, 0])]);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, 1.5 * s * theme::ring().max(1.0), if theme::is_high_contrast() { ui::BORDER } else { [255, 255, 255, if dark() { 70 } else { 200 }] });
}

/// How opaque the island's fill is. Without a blur behind it, the ink on the island has to read over any wallpaper, so the fill is nearly
/// solid (a test checks the contrast over a black and a white picture in every theme); the blur of the next slice can lower it.
pub fn island_alpha() -> u8 {
    if theme::is_high_contrast() {
        255
    } else {
        224
    }
}

/// The island's fill when the blurred wallpaper is behind it: the blur takes the detail away, so the fill can let a bit more of the
/// colour through. A test checks the ink's contrast over a black and a white wallpaper at this value in every theme.
pub fn island_alpha_frosted() -> u8 {
    if theme::is_high_contrast() {
        255
    } else {
        216
    }
}

fn dark() -> bool {
    theme::current_id() == theme::ThemeId::Dark
}

fn t_card() -> [u8; 4] {
    ui::CARD
}

/// How an icon body is shaped and drawn: the Look's `icon_shape` and `icon_style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IconLook {
    pub shape: IconShape,
    pub style: IconStyle,
}

impl IconLook {
    /// The style that is actually drawn: High contrast always gets the plain gradient with its rim (a custom style never overrules it).
    pub fn effective(self) -> IconLook {
        if theme::is_high_contrast() {
            IconLook { shape: self.shape, style: IconStyle::Gradient }
        } else {
            self
        }
    }
}

/// The outline of an icon body of this shape in the box `x, y, w, h`.
pub fn shape_path(shape: IconShape, x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    match shape {
        IconShape::Squircle => gfx::squircle_path(x, y, w, h),
        IconShape::Circle => PathBuilder::from_circle(x + w / 2.0, y + h / 2.0, w.min(h) / 2.0),
        IconShape::Rounded => gfx::rrect_path(x, y, w, h, w.min(h) * 0.22),
        IconShape::Square => gfx::rrect_path(x, y, w, h, w.min(h) * 0.06),
    }
}

/// The icon's body inside the square `sq` in the default look (a squircle with a gradient); see [`body_with`].
pub fn body(g: &mut Gfx, sq: R, accent: [u8; 4], allowed: bool, grow: f32, s: f32) -> R {
    body_with(g, sq, accent, allowed, grow, s, IconLook::default())
}

/// The icon's body inside the square `sq` (its glyph is drawn by the caller on top, with [`glyph_with`]): shadow, body in the Look's
/// shape and style (gradient, flat, outline, glass or 3d), rim. `grow` scales it around its middle (the pop-in; 1.0 at rest). Returns the
/// square drawn. Every style is drawn by code; none uses a picture. State is never a parameter: it lives on the badge and the chip.
pub fn body_with(g: &mut Gfx, sq: R, accent: [u8; 4], allowed: bool, grow: f32, s: f32, look: IconLook) -> R {
    let _p = crate::prof::scope("appicon.body_with");
    let look = look.effective();
    let (cx, cy) = (sq.x + sq.w / 2.0, sq.y + sq.h / 2.0);
    let r = R::new(cx - sq.w * grow / 2.0, cy - sq.h * grow / 2.0, sq.w * grow, sq.h * grow);
    let (top, deep) = hue_gradient(dimmed(accent, allowed));
    let sh = look.shape;
    let fill = |g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, stops: &[(f32, [u8; 4])]| {
        if let Some(p) = shape_path(sh, x, y, w, h) {
            g.path_gradient(&p, y, y + h, stops);
        }
    };
    let line = |g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, lw: f32, c: [u8; 4]| {
        if let Some(p) = shape_path(sh, x, y, w, h) {
            g.path_outline(&p, lw, c);
        }
    };
    let shadow = |g: &mut Gfx, k: f32| {
        // soft shadow under the icon: several faint copies growing downwards read as a blur
        for (grow_px, drop, a) in [(8.0f32, 12.0f32, 10.0f32), (5.0, 9.0, 12.0), (2.5, 6.0, 14.0), (0.0, 3.0, 16.0)] {
            fill(g, r.x - grow_px * s * 0.5, r.y + drop * s - grow_px * s * 0.25, r.w + grow_px * s, r.h + grow_px * s * 0.5, &[(0.0, [16, 24, 40, (a * k) as u8])]);
        }
    };
    let key = crate::memo::key(&[
        2,
        sh as u64,
        look.style as u64,
        u32::from_be_bytes(top) as u64,
        u32::from_be_bytes(deep) as u64,
        r.x.to_bits() as u64,
        r.y.to_bits() as u64,
        r.w.to_bits() as u64,
        r.h.to_bits() as u64,
        s.to_bits() as u64,
        g.width() as u64,
    ]);
    let bbox = ((r.x - 12.0 * s).floor() as i32, (r.y - 6.0 * s).floor() as i32, (r.w + 24.0 * s).ceil() as i32, (r.h + 30.0 * s).ceil() as i32);
    let mut draw_body = |g: &mut Gfx| crate::theme::with_raw(|| match look.style {
        IconStyle::Gradient => {
            shadow(g, 1.0);
            fill(g, r.x, r.y, r.w, r.h, &[(0.0, top), (1.0, deep)]);
            // gloss: the upper half, brighter at the top edge
            fill(g, r.x + 1.5 * s, r.y + 1.5 * s, r.w - 3.0 * s, r.h * 0.5, &[(0.0, [255, 255, 255, 92]), (1.0, [255, 255, 255, 8])]);
            line(g, r.x, r.y, r.w, r.h, 1.2 * s, [255, 255, 255, 120]);
        }
        IconStyle::Flat => {
            shadow(g, 0.5);
            fill(g, r.x, r.y, r.w, r.h, &[(0.0, mix(top, deep, 0.85))]);
            line(g, r.x, r.y, r.w, r.h, 1.0 * s, [255, 255, 255, 70]);
        }
        IconStyle::Outline => {
            // a faint tint of the hue inside a firm ring of it; the glyph is drawn in the theme's ink, not white (see glyph_with)
            fill(g, r.x, r.y, r.w, r.h, &[(0.0, [deep[0], deep[1], deep[2], 38])]);
            line(g, r.x + 1.5 * s, r.y + 1.5 * s, r.w - 3.0 * s, r.h - 3.0 * s, 3.0 * s, deep);
        }
        IconStyle::Glass => {
            // a gradient fake of glass until the cached blur exists: a deeper, slightly see-through body, a bright inner highlight and a
            // strong rim; the body stays deep enough that the white glyph keeps its contrast over any wallpaper
            shadow(g, 0.8);
            let t = [top[0], top[1], top[2], 238];
            let d = [deep[0], deep[1], deep[2], 252];
            fill(g, r.x, r.y, r.w, r.h, &[(0.0, t), (1.0, d)]);
            fill(g, r.x + 2.0 * s, r.y + 2.0 * s, r.w - 4.0 * s, r.h * 0.46, &[(0.0, [255, 255, 255, 120]), (1.0, [255, 255, 255, 14])]);
            line(g, r.x, r.y, r.w, r.h, 1.6 * s, [255, 255, 255, 170]);
        }
        IconStyle::ThreeD => {
            // a contact shadow on the surface, a darker base with a lit bevel face on it, a specular highlight and a dark lower rim
            for (k, a) in [(1.0f32, 26u8), (0.8, 30), (0.6, 36)] {
                let (w, h) = (r.w * (0.86 * k + 0.1), r.h * 0.12 * k + 2.0 * s);
                if let Some(p) = PathBuilder::from_oval(Rect::from_xywh(cx - w / 2.0, r.y + r.h - h * 0.15 + 3.0 * s, w, h).unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap())) {
                    g.path_gradient(&p, 0.0, 1.0, &[(0.0, [10, 14, 24, a])]);
                }
            }
            fill(g, r.x, r.y + 2.0 * s, r.w, r.h, &[(0.0, mix(deep, [0, 0, 0, 255], 0.35))]);
            fill(g, r.x, r.y, r.w, r.h, &[(0.0, mix(top, [255, 255, 255, 255], 0.12)), (0.35, top), (0.8, deep), (1.0, deep)]);
            // the bevel: a light ring on the upper edge, a dark one on the lower
            fill(g, r.x + 3.0 * s, r.y + 3.0 * s, r.w - 6.0 * s, r.h * 0.55, &[(0.0, [255, 255, 255, 70]), (1.0, [255, 255, 255, 0])]);
            line(g, r.x + 0.8 * s, r.y + 0.8 * s, r.w - 1.6 * s, r.h - 1.6 * s, 1.6 * s, [0, 0, 0, 60]);
            line(g, r.x, r.y, r.w, r.h, 1.2 * s, [255, 255, 255, 110]);
            // specular highlight: a small bright oval near the top left
            if let Some(rc) = Rect::from_xywh(r.x + r.w * 0.16, r.y + r.h * 0.09, r.w * 0.38, r.h * 0.16) {
                if let Some(p) = PathBuilder::from_oval(rc) {
                    g.path_gradient(&p, r.y + r.h * 0.09, r.y + r.h * 0.25, &[(0.0, [255, 255, 255, 200]), (1.0, [255, 255, 255, 40])]);
                }
            }
        }
    });
    if grow == 1.0 {
        crate::memo::draw(g, key, bbox, draw_body);
    } else {
        draw_body(g);
    }
    r
}

/// The glyph's colour in a style: white, except in the outline style, where it is the theme's ink (the body is only a ring).
pub fn glyph_colour_for(style: IconStyle) -> [u8; 4] {
    if style == IconStyle::Outline && !theme::is_high_contrast() {
        ui::TEXT
    } else {
        [255, 255, 255, 255]
    }
}

/// The glyph's colour in the default style: white (the icon's hue is chosen so that it reads).
pub fn glyph_colour() -> [u8; 4] {
    glyph_colour_for(IconStyle::Gradient)
}

/// Draws a built-in app's glyph in the middle of an icon body in the default style.
pub fn glyph(g: &mut Gfx, icon: Icon, body: R, s: f32) {
    glyph_with(g, icon, body, s, IconStyle::Gradient);
}

/// Draws a glyph for an icon style: the 3d style gives it a little depth with a shadow under it.
pub fn glyph_with(g: &mut Gfx, icon: Icon, body: R, s: f32, style: IconStyle) {
    let _p = crate::prof::scope("appicon.glyph_with");
    let style = IconLook { shape: IconShape::Squircle, style }.effective().style;
    let (cx, cy, sz) = (body.x + body.w / 2.0, body.y + body.h / 2.0, body.w * 0.56);
    if style == IconStyle::ThreeD {
        crate::theme::with_raw(|| g.icon(icon, cx + 1.2 * s, cy + 2.4 * s, sz, [10, 14, 30, 90]));
    }
    if style == IconStyle::Outline {
        g.icon(icon, cx, cy, sz, glyph_colour_for(style));
    } else {
        crate::theme::with_raw(|| g.icon(icon, cx, cy, sz, glyph_colour_for(style)));
    }
}

/// The corner badge: a count (updates, unread), `!` when the app needs setup or attention, a lock when it is not allowed. `None` for
/// states that carry no badge (ready, running: the chip says so).
pub fn badge(g: &mut Gfx, body: R, s: f32, state: AppState, count: usize) {
    let _p = crate::prof::scope("appicon.badge");
    let text = if count > 0 {
        Some(if count > 99 { "99+".to_string() } else { count.to_string() })
    } else if matches!(state, AppState::Setup | AppState::Attention) {
        Some("!".to_string())
    } else {
        None
    };
    let locked = state == AppState::Locked;
    if text.is_none() && !locked {
        return;
    }
    let d = 22.0 * s;
    let w = match &text {
        Some(t) if t.chars().count() > 1 => d + (t.chars().count() as f32 - 1.0) * 7.0 * s,
        _ => d,
    };
    let (x, y) = (body.x + body.w - w + 5.0 * s, body.y - 5.0 * s);
    let fill = state.colour();
    // a ring in the island's colour separates the badge from the icon
    g.rrect(x - 2.5 * s, y - 2.5 * s, w + 5.0 * s, d + 5.0 * s, (d + 5.0 * s) / 2.0, ui::CARD);
    g.rrect(x, y, w, d, d / 2.0, fill);
    // the ink on the badge: whichever of black and white reads better on the colour as the theme shows it
    let shown = theme::themed(fill);
    let ink = if contrast(shown, [255, 255, 255, 255]) >= contrast(shown, [0, 0, 0, 255]) { [255, 255, 255, 255] } else { [0, 0, 0, 255] };
    if locked && text.is_none() {
        crate::theme::with_raw(|| g.icon(Icon::Lock, x + w / 2.0, y + d / 2.0, d * 0.62, ink));
    } else if let Some(t) = text {
        let ts = 12.5 * s;
        let tw = g.text_width(ts, &t, Face::Bold);
        crate::theme::with_raw(|| g.text(x + (w - tw) / 2.0, y + (d - g.line_height(ts, Face::Bold)) / 2.0, ts, ink, &t, Face::Bold));
    }
}

/// The state chip under a label, centred on `cx` with its top at `y`: a pill tinted with the state's colour. Returns its height (0 when
/// the state has none).
pub fn chip(g: &mut Gfx, cx: f32, y: f32, s: f32, state: AppState, max_w: f32) -> f32 {
    let _p = crate::prof::scope("appicon.chip");
    let Some(text) = state.chip_text() else { return 0.0 };
    let ts = 10.5 * s;
    let text = g.fit(ts, text, Face::Bold, (max_w - 16.0 * s).max(10.0 * s));
    let tw = g.text_width(ts, &text, Face::Bold);
    let (w, h) = (tw + 16.0 * s, 17.0 * s);
    let c = state.colour();
    g.rrect(cx - w / 2.0, y, w, h, 5.0 * s, with_alpha(c, 40));
    if theme::is_high_contrast() {
        g.rrect_outline(cx - w / 2.0, y, w, h, 5.0 * s, 1.2 * s, c);
    }
    g.text(cx - tw / 2.0, y + (h - g.line_height(ts, Face::Bold)) / 2.0, ts, c, &text, Face::Bold);
    h
}

/// How far into its pop-in an icon is `age_ms` after the page changed, for the icon at `index`: the scale to draw it at (0.72, 1.05,
/// then 1.0) or `None` before its turn (not drawn yet). Staggered 42 ms apart; `None` for `reduced` is never returned: the scale is 1.
pub fn pop_scale(age_ms: u64, index: usize, reduced: bool) -> Option<f32> {
    if reduced {
        return Some(1.0);
    }
    let delay = index.min(16) as u64 * 42;
    if age_ms < delay {
        return None;
    }
    let t = ((age_ms - delay) as f32 / 500.0).min(1.0);
    Some(if t >= 1.0 {
        1.0
    } else if t < 0.6 {
        // 0.72 -> 1.05 with an ease-out
        let u = t / 0.6;
        0.72 + (1.05 - 0.72) * (1.0 - (1.0 - u) * (1.0 - u))
    } else {
        // 1.05 -> 1.0
        let u = (t - 0.6) / 0.4;
        1.05 - 0.05 * u
    })
}

/// The island's scale `age_ms` after the page changed: 0.975 growing to 1 over 620 ms (ease-out); 1 when `reduced`.
pub fn island_pop(age_ms: u64, reduced: bool) -> f32 {
    if reduced || age_ms >= 620 {
        return 1.0;
    }
    let u = age_ms as f32 / 620.0;
    0.975 + 0.025 * (1.0 - (1.0 - u).powi(3))
}

/// The whole animation lasts at most this long for `n` icons (the island pop and the last icon's pop-in).
pub fn animation_ms(n: usize) -> u64 {
    (620u64).max(n.saturating_sub(1).min(16) as u64 * 42 + 500)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::over;

    #[test]
    fn a_finished_island_is_put_back_by_a_copy_and_painted_again_when_anything_under_it_or_in_the_settings_changed() {
        crate::theme::set_current(theme::ThemeId::Dark);
        let frame = |g: &mut Gfx| {
            g.clear([20, 30, 60, 255]);
            island(g, R::new(100.0, 80.0, 600.0, 300.0), 1.0, 1.0);
        };
        let mut g = Gfx::new(800, 480);
        crate::memo::clear();
        let (h0, p0) = crate::memo::counts();
        frame(&mut g);
        let first = g.rgba().to_vec();
        frame(&mut g);
        let (h1, p1) = crate::memo::counts();
        assert_eq!((h1 - h0, p1 - p0), (1, 1), "painted once, then copied");
        assert_eq!(g.rgba(), &first[..], "the copy is the picture that was painted");
        // something different under the panel: painted again, and the result is what painting from scratch gives
        let mut h = Gfx::new(800, 480);
        h.clear([20, 30, 60, 255]);
        h.rect(150.0, 100.0, 40.0, 40.0, [200, 0, 0, 255]);
        island(&mut h, R::new(100.0, 80.0, 600.0, 300.0), 1.0, 1.0);
        let (h2, p2) = crate::memo::counts();
        assert_eq!((h2 - h1, p2 - p1), (0, 1));
        let mut fresh = Gfx::new(800, 480);
        fresh.clear([20, 30, 60, 255]);
        fresh.rect(150.0, 100.0, 40.0, 40.0, [200, 0, 0, 255]);
        paint_island(&mut fresh, R::new(100.0, 80.0, 600.0, 300.0), 1.0, 1.0);
        assert_eq!(h.rgba(), fresh.rgba());
        // another size of panel, another scale, another theme: painted
        let mut g = Gfx::new(800, 480);
        frame(&mut g);
        let before = crate::memo::counts();
        g.clear([20, 30, 60, 255]);
        island(&mut g, R::new(100.0, 80.0, 610.0, 300.0), 1.0, 1.0);
        crate::theme::set_current(theme::ThemeId::Light);
        g.clear([20, 30, 60, 255]);
        island(&mut g, R::new(100.0, 80.0, 610.0, 300.0), 1.0, 1.0);
        crate::theme::set_current(theme::ThemeId::Dark);
        assert_eq!(crate::memo::counts().1 - before.1, 2);
        // while it pops in (pop below 1) nothing is kept or used
        let c = crate::memo::counts();
        let mut p = Gfx::new(800, 480);
        p.clear([20, 30, 60, 255]);
        island(&mut p, R::new(100.0, 80.0, 610.0, 300.0), 1.0, 0.98);
        assert_eq!(crate::memo::counts(), c);
    }

    #[test]
    fn the_white_glyph_reads_on_every_hue_in_every_theme() {
        let hues = [[79, 140, 255, 255], [61, 214, 163, 255], [255, 196, 94, 255], [176, 226, 92, 255], [255, 255, 255, 255], [100, 230, 255, 255]];
        for hc in [false, true] {
            crate::theme::set_current(if hc { theme::ThemeId::HighContrast } else { theme::ThemeId::Dark });
            for h in hues {
                let (top, deep) = hue_gradient(h);
                let white = [255, 255, 255, 255];
                let want = if hc { 4.5 } else { 3.0 };
                assert!(contrast(white, deep) >= want - 0.05, "{h:?} deep {deep:?} {}", contrast(white, deep));
                assert!(contrast(white, top) >= want * 0.8 - 0.05, "{h:?} top {top:?} {}", contrast(white, top));
            }
        }
        crate::theme::set_current(theme::ThemeId::Dark);
    }

    #[test]
    fn the_ink_on_the_island_reads_over_a_black_and_a_white_wallpaper_in_every_theme() {
        for id in theme::ALL {
            theme::set_current(id);
            let t = theme::current();
            for behind in [[0u8, 0, 0, 255], [255, 255, 255, 255], [128, 128, 128, 255]] {
                for (which, a) in [("plain", island_alpha()), ("frosted", island_alpha_frosted())] {
                    let isl = over(behind, with_alpha(t.card, a));
                    for (name, ink) in [("TEXT", t.text), ("MUTED", t.muted)] {
                        let c = contrast(ink, isl);
                        assert!(c >= 4.5, "{id:?}: {name} on the {which} island over {behind:?} is {c:.2}:1");
                    }
                }
            }
        }
        theme::set_current(theme::ThemeId::Dark);
    }

    fn drawn_with(shape: IconShape, style: IconStyle) -> Gfx {
        let mut g = Gfx::new(240, 240);
        g.clear([200, 200, 200, 255]);
        body_with(&mut g, R::new(60.0, 50.0, 120.0, 120.0), [79, 140, 255, 255], true, 1.0, 1.0, IconLook { shape, style });
        g
    }

    #[test]
    fn every_shape_and_style_is_drawn_inside_its_box_and_the_styles_differ() {
        let mut seen: Vec<(IconStyle, Vec<u8>)> = vec![];
        for shape in IconShape::ALL {
            for style in IconStyle::ALL {
                let g = drawn_with(*shape, *style);
                let px = |x: usize, y: usize| g.rgba()[(y * 240 + x) * 4..][..3].to_vec();
                assert_ne!(px(120, 110), vec![200, 200, 200], "{shape:?} {style:?}: the middle is painted");
                assert_eq!(px(4, 4), vec![200, 200, 200], "{shape:?} {style:?}: nothing far outside the box");
                assert_eq!(px(236, 120), vec![200, 200, 200], "{shape:?} {style:?}: nothing far to the side");
                if *shape == IconShape::Squircle {
                    seen.push((*style, g.rgba().to_vec()));
                }
            }
        }
        for (i, (a, ap)) in seen.iter().enumerate() {
            for (b, bp) in &seen[i + 1..] {
                assert_ne!(ap, bp, "{a:?} and {b:?} look the same");
            }
        }
        // a circle leaves the corner of its box empty, a square fills it
        let corner = |g: &Gfx| g.rgba()[(54 * 240 + 66) * 4..][..3].to_vec();
        assert_eq!(corner(&drawn_with(IconShape::Circle, IconStyle::Flat)), vec![200, 200, 200]);
        assert_ne!(corner(&drawn_with(IconShape::Square, IconStyle::Flat)), vec![200, 200, 200]);
    }

    #[test]
    fn the_white_glyph_reads_on_the_body_of_every_style_that_carries_it_and_high_contrast_keeps_the_plain_icon() {
        for style in [IconStyle::Gradient, IconStyle::Flat, IconStyle::Glass, IconStyle::ThreeD] {
            // sample the lower middle of the body (where the glyph's strokes can sit) over a pale island
            let g = drawn_with(IconShape::Squircle, style);
            let i = (150 * 240 + 120) * 4;
            let c = [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2], 255];
            assert!(contrast([255, 255, 255, 255], c) >= 3.0, "{style:?}: white on {c:?} is {:.2}", contrast([255, 255, 255, 255], c));
        }
        theme::set_current(theme::ThemeId::HighContrast);
        assert_eq!(drawn_with(IconShape::Squircle, IconStyle::ThreeD).rgba(), drawn_with(IconShape::Squircle, IconStyle::Gradient).rgba(), "High contrast overrides the style");
        theme::set_current(theme::ThemeId::Dark);
    }

    #[test]
    fn the_pop_in_staggers_42_ms_and_reduced_motion_removes_it() {
        assert_eq!(pop_scale(0, 0, false), Some(0.72));
        assert_eq!(pop_scale(41, 1, false), None, "the second icon waits 42 ms");
        assert!(pop_scale(42, 1, false).is_some());
        assert_eq!(pop_scale(5_000, 3, false), Some(1.0));
        let peak = (0..500).filter_map(|t| pop_scale(t, 0, false)).fold(0.0f32, f32::max);
        assert!(peak > 1.03 && peak <= 1.05 + 1e-4, "overshoots to 1.05: {peak}");
        for age in [0, 10, 300, 900] {
            assert_eq!(pop_scale(age, 9, true), Some(1.0));
            assert_eq!(island_pop(age, true), 1.0);
        }
        assert!((island_pop(0, false) - 0.975).abs() < 1e-6);
        assert_eq!(island_pop(620, false), 1.0);
        assert!(animation_ms(8) >= 620 && animation_ms(100) <= 620 + 16 * 42 + 500);
    }

    #[test]
    fn state_never_changes_the_icon_colour() {
        // the body takes the hue and whether the app is allowed, nothing else: there is no state parameter
        let mut a = Gfx::new(200, 200);
        let mut b = Gfx::new(200, 200);
        body(&mut a, R::new(40.0, 40.0, 100.0, 100.0), [79, 140, 255, 255], true, 1.0, 1.0);
        body(&mut b, R::new(40.0, 40.0, 100.0, 100.0), [79, 140, 255, 255], true, 1.0, 1.0);
        assert_eq!(a.rgba(), b.rgba());
    }

    #[test]
    fn the_icon_keeps_its_hue_in_light_and_dark() {
        let at = |id: theme::ThemeId| {
            crate::theme::set_current(id);
            let mut g = Gfx::new(200, 200);
            g.clear([128, 128, 128, 255]);
            body(&mut g, R::new(40.0, 40.0, 100.0, 100.0), [79, 140, 255, 255], true, 1.0, 1.0);
            let px = g.rgba()[((120usize * 200) + 90) * 4..][..3].to_vec();
            px
        };
        let (d, l) = (at(theme::ThemeId::Dark), at(theme::ThemeId::Light));
        crate::theme::set_current(theme::ThemeId::Dark);
        assert_eq!(d, l, "raw colours: an icon is the same in every theme");
    }
}
