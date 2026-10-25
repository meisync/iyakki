//! The AI IDE's density and its shared chrome: one `Metrics` value says how big text, rows, tabs and headers are, and the
//! small drawing helpers (flat tabs, text buttons, pane headers, tooltips) read it, so one change scales the whole IDE.
//!
//! Sizes are in unscaled units; the IDE draws at `s * metrics.k`, where `s` already carries the window and the person's UI
//! size (Settings → Display). Compact is the default: about 12-13 px text at 1280x720, 22 px rows, 26 px tabs.

use crate::safeclamp::SafeClamp;
use crate::fx::{self, with_alpha, Glyph};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, MUTED, R, TEXT};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Comfortable,
    Compact,
    Dense,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Comfortable, Density::Compact, Density::Dense];
    pub fn name(self) -> &'static str {
        match self {
            Density::Comfortable => "Comfortable",
            Density::Compact => "Compact",
            Density::Dense => "Dense",
        }
    }
    pub fn index(self) -> usize {
        Density::ALL.iter().position(|d| *d == self).unwrap_or(1)
    }
    pub fn from_index(i: usize) -> Density {
        Density::ALL[i.min(2)]
    }
    /// The next denser (`+1`: smaller text) or looser (`-1`) level, stopping at the ends.
    pub fn step(self, denser: bool) -> Density {
        let i = self.index();
        Density::from_index(if denser { (i + 1).min(2) } else { i.saturating_sub(1) })
    }
}

/// Everything the IDE sizes, for one density. Lengths are unscaled units (multiply by the IDE's `s`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub density: Density,
    /// The IDE draws at the window scale times this.
    pub k: f32,
    /// The embedded chat draws at the window scale times this (its own type is larger than the IDE's).
    pub chat_k: f32,
    pub top_h: f32,
    pub status_h: f32,
    pub rail_w: f32,
    pub tab_h: f32,
    pub head_h: f32,
    pub row_h: f32,
    pub tool_h: f32,
    pub pad: f32,
    pub gap: f32,
    pub icon: f32,
    pub divider: f32,
    pub radius: f32,
    pub font_ui: f32,
    pub font_small: f32,
    pub font_title: f32,
    pub font_mono: f32,
}

impl Metrics {
    pub fn of(d: Density) -> Metrics {
        let (k, chat_k) = match d {
            Density::Comfortable => (1.12, 1.0),
            Density::Compact => (1.0, 0.9),
            Density::Dense => (0.9, 0.82),
        };
        Metrics { density: d, k, chat_k, top_h: 36.0, status_h: 22.0, rail_w: 36.0, tab_h: 29.0, head_h: 27.0, row_h: 25.0, tool_h: 24.0, pad: 8.0, gap: 6.0, icon: 15.0, divider: 6.0, radius: 6.0, font_ui: 14.0, font_small: 12.0, font_title: 11.5, font_mono: 13.0 }
    }
}

thread_local! {
    static CUR: Cell<Metrics> = Cell::new(Metrics::of(Density::Compact));
}

/// The metrics of the IDE being drawn (set at the start of each frame and when keys are handled).
pub fn cur() -> Metrics {
    CUR.with(|c| c.get())
}

pub fn set(m: Metrics) {
    CUR.with(|c| c.set(m));
}

pub const CONTRAST: f32 = 4.5;

/// The height of a small control at scale `s`: never under 20 px, so it stays easy to hit with a mouse.
pub fn tool(s: f32) -> f32 {
    (cur().tool_h * s).max(20.0)
}

/// Muted text that reads at 4.5:1 on the panel.
pub fn muted() -> [u8; 4] {
    fx::readable(MUTED, fx::PANEL, CONTRAST)
}

/// A flat text tab with an underline under the active one. Returns its width; `hit` names the click region.
pub fn flat_tab(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, active: bool, accent: [u8; 4], ev: Event, hit: &str) -> f32 {
    let m = cur();
    let sz = m.font_ui * s;
    let face = if active { Face::Bold } else { Face::Sans };
    let w = g.text_width(sz, label, face) + 2.0 * 10.0 * s;
    let r = R::new(x, y, w, (m.tab_h * s).max(20.0));
    let hot = g.hovered(r);
    if hot && !active {
        g.rrect(r.x, r.y + 2.0 * s, r.w, r.h - 4.0 * s, 4.0 * s, with_alpha(accent, 26));
    }
    let col = if active { TEXT } else if hot { fx::readable(TEXT, fx::PANEL, CONTRAST) } else { muted() };
    g.text(r.x + 10.0 * s, r.y + (r.h - g.line_height(sz, face)) / 2.0, sz, col, label, face);
    if active {
        g.rect(r.x + 4.0 * s, r.y + r.h - 2.0 * s, r.w - 8.0 * s, 2.0 * s.max(1.0), accent);
    }
    g.hit(r, ev, hit);
    w
}

/// A small text button (no outline; a soft background on hover). `enabled` false draws it dim and does nothing.
/// `tip` shows near it on hover. Returns the width.
#[allow(clippy::too_many_arguments)]
pub fn text_btn(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, color: [u8; 4], enabled: bool, ev: Event, hit: &str, tip: Option<&str>, bounds: R) -> f32 {
    let m = cur();
    let sz = m.font_small * s + 1.0 * s;
    let w = g.text_width(sz, label, Face::Bold) + 2.0 * 8.0 * s;
    let r = R::new(x, y, w, tool(s));
    let hot = g.hovered(r);
    if hot && enabled {
        g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, rgb(34, 44, 78));
    }
    let col = if !enabled { rgb(112, 122, 150) } else { fx::readable(color, if hot { rgb(34, 44, 78) } else { fx::PANEL }, CONTRAST) };
    g.text(r.x + 8.0 * s, r.y + (r.h - g.line_height(sz, Face::Bold)) / 2.0, sz, col, label, Face::Bold);
    g.hit(r, if enabled { ev } else { Event::Tick }, hit);
    if hot {
        if let Some(t) = tip {
            tooltip(g, r, s, t, bounds);
        }
    }
    w
}

/// The side of a tab or button picture at scale `s`: 14 to 18 px, following the density.
pub fn icon_size(s: f32) -> f32 {
    (cur().icon * s).clamp(14.0, 18.0)
}

/// The width of a tab with a picture: the picture and the text (when `show_text`), or the picture alone.
pub fn icon_tab_width(g: &mut Gfx, s: f32, label: &str, show_text: bool, active: bool, badge: Option<usize>) -> f32 {
    let m = cur();
    let sz = m.font_ui * s;
    let face = if active { Face::Bold } else { Face::Sans };
    let badge_w = badge.map_or(0.0, |n| badge_width(g, s, n) + 4.0 * s);
    if show_text {
        2.0 * 9.0 * s + icon_size(s) + 6.0 * s + g.text_width(sz, label, face) + badge_w
    } else {
        (2.0 * 8.0 * s + icon_size(s)).max(28.0 * s)
    }
}

fn badge_width(g: &mut Gfx, s: f32, n: usize) -> f32 {
    let sz = cur().font_small * s * 0.9;
    (g.text_width(sz, &n.min(99).to_string(), Face::Bold) + 8.0 * s).max(16.0 * s)
}

/// A count in a small amber pill (the number of problems). Returns its width.
fn badge(g: &mut Gfx, x: f32, cy: f32, s: f32, n: usize) -> f32 {
    let sz = cur().font_small * s * 0.9;
    let w = badge_width(g, s, n);
    let h = (sz * 1.35).max(14.0 * s);
    g.rrect(x, cy - h / 2.0, w, h, h / 2.0, fx::AMBER);
    let t = n.min(99).to_string();
    let tw = g.text_width(sz, &t, Face::Bold);
    g.text(x + (w - tw) / 2.0, cy - g.line_height(sz, Face::Bold) / 2.0, sz, rgb(30, 22, 4), &t, Face::Bold);
    w
}

/// A flat tab with a picture, the text after it (hidden when `show_text` is false: the picture alone, with the name as a hover
/// hint), an optional count and the underline under the open one. Returns its width.
#[allow(clippy::too_many_arguments)]
pub fn icon_tab(g: &mut Gfx, x: f32, y: f32, s: f32, gl: Glyph, label: &str, show_text: bool, badge_n: Option<usize>, active: bool, accent: [u8; 4], ev: Event, hit: &str, bounds: R) -> f32 {
    let m = cur();
    let sz = m.font_ui * s;
    let face = if active { Face::Bold } else { Face::Sans };
    let w = icon_tab_width(g, s, label, show_text, active, badge_n);
    let r = R::new(x, y, w, (m.tab_h * s).max(20.0));
    let hot = g.hovered(r);
    if hot && !active {
        g.rrect(r.x, r.y + 2.0 * s, r.w, r.h - 4.0 * s, 4.0 * s, with_alpha(accent, 26));
    }
    let col = if active { TEXT } else if hot { fx::readable(TEXT, fx::PANEL, CONTRAST) } else { muted() };
    let isz = icon_size(s);
    let cy = r.y + r.h / 2.0;
    if show_text {
        fx::glyph(g, gl, r.x + 9.0 * s + isz / 2.0, cy, isz, if active { accent_on(accent) } else { col });
        let tx = r.x + 9.0 * s + isz + 6.0 * s;
        g.text(tx, r.y + (r.h - g.line_height(sz, face)) / 2.0, sz, col, label, face);
        if let Some(n) = badge_n {
            let lw = g.text_width(sz, label, face);
            badge(g, tx + lw + 4.0 * s, cy, s, n);
        }
    } else {
        fx::glyph(g, gl, r.x + r.w / 2.0, cy, isz, if active { accent_on(accent) } else { col });
        if let Some(n) = badge_n {
            // the count sits on the picture's upper right corner
            let bw = badge_width(g, s, n);
            badge(g, (r.x + r.w - bw - 1.0 * s).max(r.x), r.y + 6.0 * s, s, n);
        }
    }
    if active {
        g.rect(r.x + 4.0 * s, r.y + r.h - 2.0 * s, r.w - 8.0 * s, 2.0 * s.max(1.0), accent);
    }
    g.hit(r, ev, hit);
    if hot && (!show_text || badge_n.is_some()) {
        let tip = match badge_n {
            Some(n) => format!("{label}: {n}"),
            None => label.to_string(),
        };
        tooltip(g, r, s, &tip, bounds);
    }
    w
}

/// The colour of the picture of the open tab: the accent, lifted until it reads on the panel.
fn accent_on(accent: [u8; 4]) -> [u8; 4] {
    fx::readable(accent, fx::PANEL, CONTRAST)
}

/// The width of a button with a picture (see [`icon_btn_glyph`]).
pub fn icon_btn_width(g: &mut Gfx, s: f32, label: &str, show_text: bool) -> f32 {
    let m = cur();
    if show_text {
        2.0 * 7.0 * s + icon_size(s) + 5.0 * s + g.text_width(m.font_small * s + 1.0 * s, label, Face::Bold)
    } else {
        (2.0 * 5.0 * s + icon_size(s)).max(tool(s))
    }
}

/// A small button with a picture and, when `show_text`, its text. Icon-only it shows `tip` (or the label) on hover.
/// `enabled` false draws it dim and does nothing. Returns the width.
#[allow(clippy::too_many_arguments)]
pub fn icon_btn_glyph(g: &mut Gfx, x: f32, y: f32, s: f32, gl: Glyph, label: &str, show_text: bool, color: [u8; 4], on: bool, enabled: bool, ev: Event, hit: &str, tip: Option<&str>, bounds: R) -> f32 {
    let m = cur();
    let sz = m.font_small * s + 1.0 * s;
    let w = icon_btn_width(g, s, label, show_text);
    let r = R::new(x, y, w, tool(s));
    let hot = g.hovered(r);
    let bg = if on { fx::over(fx::WELL, with_alpha(color, 60)) } else if hot && enabled { rgb(34, 44, 78) } else { fx::PANEL };
    if (hot && enabled) || on {
        g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, bg);
    }
    let col = if !enabled { rgb(112, 122, 150) } else if on { TEXT } else { fx::readable(color, bg, CONTRAST) };
    let isz = icon_size(s);
    if show_text {
        fx::glyph(g, gl, r.x + 7.0 * s + isz / 2.0, r.y + r.h / 2.0, isz, col);
        g.text(r.x + 7.0 * s + isz + 5.0 * s, r.y + (r.h - g.line_height(sz, Face::Bold)) / 2.0, sz, col, label, Face::Bold);
    } else {
        fx::glyph(g, gl, r.x + r.w / 2.0, r.y + r.h / 2.0, isz, col);
    }
    g.hit(r, if enabled { ev } else { Event::Tick }, hit);
    if hot {
        match (tip, show_text) {
            (Some(t), _) => tooltip(g, r, s, t, bounds),
            (None, false) => tooltip(g, r, s, label, bounds),
            _ => {}
        }
    }
    w
}

/// A small icon-sized button showing `glyph` (a symbol or a short word); on when `on`.
#[allow(clippy::too_many_arguments)]
pub fn icon_btn(g: &mut Gfx, x: f32, y: f32, s: f32, glyph: &str, color: [u8; 4], on: bool, ev: Event, hit: &str, bounds: R) -> f32 {
    let m = cur();
    let sz = m.font_ui * s;
    let w = (g.text_width(sz, glyph, Face::Bold) + 2.0 * 7.0 * s).max(tool(s));
    let r = R::new(x, y, w, tool(s));
    let hot = g.hovered(r);
    let bg = if on { fx::over(fx::WELL, with_alpha(color, 60)) } else if hot { rgb(34, 44, 78) } else { fx::PANEL };
    if on || hot {
        g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, bg);
    }
    let col = if on { TEXT } else { fx::readable(color, bg, CONTRAST) };
    let gw = g.text_width(sz, glyph, Face::Bold);
    g.text(r.x + (r.w - gw) / 2.0, r.y + (r.h - g.line_height(sz, Face::Bold)) / 2.0, sz, col, glyph, Face::Bold);
    g.hit(r, ev, hit);
    if hot {
        tooltip(g, r, s, hit, bounds);
    }
    w
}

/// A hover hint under `anchor`, kept inside `bounds`.
pub fn tooltip(g: &mut Gfx, anchor: R, s: f32, text: &str, bounds: R) {
    let sz = cur().font_small * s;
    let tw = (g.text_width(sz, text, Face::Sans) + 16.0 * s).min(bounds.w - 8.0 * s).max(20.0);
    let t = g.fit(sz, text, Face::Sans, tw - 16.0 * s);
    let h = 22.0 * s;
    let x = (anchor.x + anchor.w - tw).clamp_safe(bounds.x + 4.0 * s, (bounds.x + bounds.w - tw - 4.0 * s).max(bounds.x));
    let below = anchor.y + anchor.h + 4.0 * s;
    let y = if below + h <= bounds.y + bounds.h { below } else { anchor.y - h - 4.0 * s };
    g.rrect(x, y, tw, h, 5.0 * s, rgb(34, 40, 66));
    g.rrect_outline(x, y, tw, h, 5.0 * s, 1.0, rgb(90, 104, 150));
    g.text(x + 8.0 * s, y + (h - g.line_height(sz, Face::Sans)) / 2.0, sz, TEXT, &t, Face::Sans);
}

/// A pane's header line: a small upper-case title on the left, a hairline under it. Returns the header rectangle.
pub fn pane_head(g: &mut Gfx, r: R, s: f32, title: &str) -> R {
    let m = cur();
    let h = m.head_h * s;
    let sz = m.font_title * s;
    g.text(r.x + m.pad * s + 2.0 * s, r.y + (h - g.line_height(sz, Face::Bold)) / 2.0, sz, muted(), &title.to_uppercase(), Face::Bold);
    g.rect(r.x + 1.0, r.y + h - 1.0, r.w - 2.0, 1.0, [255, 255, 255, 16]);
    R::new(r.x, r.y, r.w, h)
}

/// A hairline under a tab strip.
pub fn strip_line(g: &mut Gfx, x: f32, y: f32, w: f32) {
    g.rect(x, y, w, 1.0, [255, 255, 255, 18]);
}

/// The hairline under a pane's header or tab strip: a 2 px accent line while the pane owns the keyboard, else the plain hairline.
pub fn focus_line(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, focused: bool, accent: [u8; 4]) {
    if focused {
        g.rect(x, y - 1.0 * s.max(1.0), w, 2.0 * s.max(1.0), accent);
    } else {
        strip_line(g, x, y, w);
    }
}

pub fn dim() -> [u8; 4] {
    fx::readable(DIM, fx::PANEL, CONTRAST)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn densities_step_and_scale_in_order() {
        let (c, p, d) = (Metrics::of(Density::Comfortable), Metrics::of(Density::Compact), Metrics::of(Density::Dense));
        assert!(c.k > p.k && p.k > d.k && c.chat_k > p.chat_k && p.chat_k > d.chat_k);
        assert_eq!(Density::Compact.step(true), Density::Dense);
        assert_eq!(Density::Dense.step(true), Density::Dense);
        assert_eq!(Density::Comfortable.step(false), Density::Comfortable);
        assert_eq!(Density::from_index(Density::Dense.index()), Density::Dense);
        // at the window scale of a 1280x720 screen (0.9): rows about 22 px in Compact and 20 in Dense, tabs about 26
        let s = 0.9;
        assert!((p.row_h * s * p.k - 22.5).abs() < 1.0 && (d.row_h * s * d.k - 20.2).abs() < 1.0 && (p.tab_h * s * p.k - 26.0).abs() < 1.0);
        // type: 12-13 px in Compact, 11-12 in Dense
        assert!((p.font_ui * s * p.k - 12.6).abs() < 0.5 && (d.font_ui * s * d.k - 11.3).abs() < 0.5);
    }

    #[test]
    fn muted_and_dim_text_reads_at_4_5_to_1_on_the_panel() {
        assert!(fx::contrast(muted(), fx::PANEL) >= CONTRAST);
        assert!(fx::contrast(dim(), fx::PANEL) >= CONTRAST);
    }

    #[test]
    fn pictures_follow_the_density_between_14_and_18_px_and_read_at_4_5_to_1() {
        for d in Density::ALL {
            set(Metrics::of(d));
            for scale in [0.6f32, 0.9, 1.35, 2.0, 3.0] {
                let px = icon_size(scale * Metrics::of(d).k);
                assert!((14.0..=18.0).contains(&px), "{d:?} at {scale}: {px}");
            }
        }
        set(Metrics::of(Density::Compact));
        assert!(icon_size(0.9) <= icon_size(1.5), "bigger windows draw bigger pictures");
        // the colours the tabs and buttons draw their pictures in, on the panel and on the hover wash
        for bg in [fx::PANEL, rgb(34, 44, 78)] {
            for c in [fx::CYAN, fx::MAGENTA, fx::AMBER, crate::ui::OK, crate::ui::ERR, TEXT] {
                assert!(fx::contrast(fx::readable(c, bg, CONTRAST), bg) >= CONTRAST, "{c:?} on {bg:?}");
            }
        }
        assert!(fx::contrast(accent_on(rgb(60, 70, 200)), fx::PANEL) >= CONTRAST);
    }
}
