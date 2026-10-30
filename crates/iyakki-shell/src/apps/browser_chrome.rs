//! The look of the Browser tile's chrome: the tab strip (tabs, group chips, the new-tab and overview buttons), the tool bar
//! (navigation, the address field with the lock chip, the star and Go inside it, the small chips after it), the thin progress bar,
//! the calm placeholder before a page's first picture, the "taking long" hint and the page-error card. Geometry is computed in
//! plain functions ([`strip_layout`], [`bar_layout`], [`hint_layout`]) so it can be tested for overlaps and clipping at every size;
//! `browser.rs` registers the click regions from the rectangles and draws with the helpers here.

use crate::safeclamp::SafeClamp;
use super::browser_groups::{StripItem, COLORS};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{DIM, ERR, MUTED, R, TEXT};

/// Height of the tab strip and of the tool bar at scale 1.
pub const TABS_H: f32 = 40.0;
pub const BAR_H: f32 = 52.0;
/// What shows where the engine has not painted yet: dark, not the page's white.
pub const PLACEHOLDER: [u8; 4] = rgb(14, 18, 34);
pub const AMBER: [u8; 4] = rgb(255, 196, 94);
/// Seconds of loading after which the page area offers Stop and Retry.
pub const SLOW_SECS: f32 = 15.0;
/// The tab strip's colours.
pub const STRIP_BG: [u8; 4] = rgb(10, 13, 26);
const TAB_ON: [u8; 4] = rgb(30, 40, 72);
const TAB_OFF: [u8; 4] = rgb(18, 23, 42);
const TAB_HOT: [u8; 4] = rgb(27, 34, 60);

// ------------------------------------------------------------------------------------------------------------ tab strip

#[derive(Clone, Copy, Debug)]
pub struct TabGeom {
    pub idx: usize,
    pub tab: R,
    pub icon: R,
    /// Where the title may be drawn: never reaches the close button.
    pub title: R,
    /// The close button (zero width on a tab too narrow for one that is not in use).
    pub close: R,
}

#[derive(Clone, Debug)]
pub struct StripGeom {
    pub tabs: Vec<TabGeom>,
    pub chips: Vec<(u32, R)>,
    pub plus: R,
    pub overview: R,
}

/// Lay the strip out inside `area` (its top `TABS_H * s` rows). `extra_right` is room kept at the right end (the assistant's tab).
pub fn strip_layout(area: R, s: f32, items: &[StripItem], active: usize, chip_w: &dyn Fn(u32) -> f32, extra_right: f32) -> StripGeom {
    let top = area.y + 3.0 * s;
    let th = 34.0 * s;
    let gap = 3.0 * s;
    let overview = R::new(area.x + area.w - extra_right - 8.0 * s - 36.0 * s, area.y + 2.0 * s, 36.0 * s, 36.0 * s);
    let chips_w: f32 = items.iter().filter_map(|i| if let StripItem::Chip(g) = i { Some(chip_w(*g) + gap) } else { None }).sum();
    let nt = items.iter().filter(|i| matches!(i, StripItem::Tab(_))).count().max(1);
    let plus_w = 36.0 * s;
    let avail = (overview.x - 6.0 * s - (area.x + 10.0 * s) - plus_w - chips_w).max(30.0 * s);
    let roomy = 76.0 * s;
    let mut base = (avail / nt as f32).clamp_safe(30.0 * s, 220.0 * s);
    let mut act_w = base;
    let active_shown = items.contains(&StripItem::Tab(active));
    if base < roomy && active_shown && nt > 1 {
        act_w = roomy;
        base = ((avail - roomy) / (nt - 1) as f32).clamp_safe(30.0 * s, roomy);
    }
    let mut x = area.x + 10.0 * s;
    let (mut tabs, mut chips) = (Vec::new(), Vec::new());
    for it in items {
        match *it {
            StripItem::Chip(g) => {
                let w = chip_w(g);
                chips.push((g, R::new(x, top + 3.0 * s, w, 28.0 * s)));
                x += w + gap;
            }
            StripItem::Tab(i) => {
                let w = if i == active { act_w } else { base };
                let tab = R::new(x, top, w - gap, th);
                let has_close = i == active || tab.w >= 70.0 * s;
                let cw = if has_close { 22.0 * s } else { 0.0 };
                let close = R::new(tab.x + tab.w - 5.0 * s - cw, tab.y + (th - 22.0 * s) / 2.0, cw, 22.0 * s);
                let icon = R::new(tab.x + if tab.w < 44.0 * s { (tab.w - 16.0 * s) / 2.0 } else { 10.0 * s }, tab.y + (th - 16.0 * s) / 2.0, 16.0 * s, 16.0 * s);
                let tx = icon.x + icon.w + 8.0 * s;
                let tr = if has_close { close.x - 4.0 * s } else { tab.x + tab.w - 8.0 * s };
                let title = R::new(tx, tab.y, (tr - tx).max(0.0), th);
                tabs.push(TabGeom { idx: i, tab, icon, title, close });
                x += w;
            }
        }
    }
    let plus = R::new(x + 2.0 * s, area.y + 2.0 * s, plus_w, 36.0 * s);
    StripGeom { tabs, chips, plus, overview }
}

/// The width of a group's chip: dot, name (cut at 120) and the number of tabs.
pub fn group_chip_w(g: &mut Gfx, s: f32, name: &str, count: usize) -> f32 {
    let w = g.text_width(13.0 * s, &format!("{name} {count}"), Face::Bold).min(130.0 * s);
    w + 34.0 * s
}

fn initial_of(title: &str) -> char {
    title.chars().find(|c| c.is_alphanumeric()).map_or('•', |c| c.to_ascii_uppercase())
}

/// A stable colour from the title (the favicon's place-holder circle).
fn tint_of(title: &str) -> [u8; 4] {
    let h = title.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[(h as usize) % COLORS.len()]
}

/// A small spinner: eight dots around a centre, the lit one moving with `tick`.
pub fn spinner(g: &mut Gfx, cx: f32, cy: f32, r: f32, tick: u64, c: [u8; 4]) {
    let lit = (tick / 3 % 8) as usize;
    for k in 0..8usize {
        let a = k as f32 * std::f32::consts::TAU / 8.0;
        let age = (k + 8 - lit) % 8;
        let alpha = 255 - (age as u32 * 28).min(200);
        g.circle(cx + a.cos() * r, cy + a.sin() * r, (r * 0.22).max(1.0), [c[0], c[1], c[2], alpha as u8]);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tab(g: &mut Gfx, t: &TabGeom, title: &str, active: bool, loading: bool, tick: u64, group: Option<[u8; 4]>, accent: [u8; 4], s: f32) {
    let hot = g.hovered(t.tab);
    let fill = if active { fx::over(TAB_ON, [accent[0], accent[1], accent[2], 26]) } else if hot { TAB_HOT } else { TAB_OFF };
    let rad = 9.0 * s;
    g.rrect(t.tab.x, t.tab.y, t.tab.w, t.tab.h, rad, fill);
    if active {
        g.rrect_outline(t.tab.x, t.tab.y, t.tab.w, t.tab.h, rad, 1.2 * s, [accent[0], accent[1], accent[2], 170]);
    }
    // the underline: the group's colour, or the accent on the tab in use
    if let Some(c) = group.or(active.then_some(accent)) {
        g.rrect(t.tab.x + 8.0 * s, t.tab.y + t.tab.h - 3.5 * s, (t.tab.w - 16.0 * s).max(4.0), 2.5 * s, 1.2 * s, c);
    }
    let (cx, cy) = (t.icon.x + t.icon.w / 2.0, t.icon.y + t.icon.h / 2.0);
    if loading {
        spinner(g, cx, cy, 6.5 * s, tick, accent);
    } else {
        let c = tint_of(title);
        g.circle(cx, cy, 8.0 * s, [c[0] / 3, c[1] / 3, c[2] / 3, 255]);
        let ch = initial_of(title).to_string();
        let tw = g.text_width(11.0 * s, &ch, Face::Bold);
        let lh = g.line_height(11.0 * s, Face::Bold);
        g.text(cx - tw / 2.0, cy - lh / 2.0, 11.0 * s, c, &ch, Face::Bold);
    }
    if t.title.w > 12.0 * s {
        let face = if active { Face::Bold } else { Face::Sans };
        let label = g.fit(14.0 * s, title, face, t.title.w);
        let lh = g.line_height(14.0 * s, face);
        g.text(t.title.x, t.title.y + (t.title.h - lh) / 2.0, 14.0 * s, if active { TEXT } else { MUTED }, &label, face);
    }
    if t.close.w > 0.0 {
        let chot = g.hovered(t.close);
        if chot {
            g.rrect(t.close.x, t.close.y, t.close.w, t.close.h, 6.0 * s, rgb(58, 66, 104));
        }
        let col = if chot || active { TEXT } else if hot { MUTED } else { DIM };
        let (cx, cy, d) = (t.close.x + t.close.w / 2.0, t.close.y + t.close.h / 2.0, 4.2 * s);
        g.line(cx - d, cy - d, cx + d, cy + d, 1.6 * s, col);
        g.line(cx - d, cy + d, cx + d, cy - d, 1.6 * s, col);
    }
}

pub fn draw_group_chip(g: &mut Gfx, r: R, name: &str, count: usize, color: [u8; 4], collapsed: bool, s: f32) {
    let hot = g.hovered(r);
    let fill = fx::over(rgb(18, 23, 42), [color[0], color[1], color[2], if collapsed { 70 } else { 40 }]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, if hot { fx::over(fill, [255, 255, 255, 14]) } else { fill });
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.1 * s, [color[0], color[1], color[2], 200]);
    g.circle(r.x + 12.0 * s, r.y + r.h / 2.0, 4.0 * s, color);
    let label = g.fit(13.0 * s, &format!("{name} {count}"), Face::Bold, r.w - 28.0 * s);
    let lh = g.line_height(13.0 * s, Face::Bold);
    g.text(r.x + 22.0 * s, r.y + (r.h - lh) / 2.0, 13.0 * s, fx::readable(color, fill, 4.5), &label, Face::Bold);
}

pub fn draw_plus(g: &mut Gfx, r: R, s: f32) {
    let hot = g.hovered(r);
    let v = R::new(r.x + 4.0 * s, r.y + 5.0 * s, r.w - 8.0 * s, r.h - 10.0 * s);
    g.rrect(v.x, v.y, v.w, v.h, 9.0 * s, if hot { TAB_HOT } else { rgb(15, 19, 36) });
    let (cx, cy, d) = (v.x + v.w / 2.0, v.y + v.h / 2.0, 5.5 * s);
    let c = if hot { TEXT } else { MUTED };
    g.line(cx - d, cy, cx + d, cy, 1.8 * s, c);
    g.line(cx, cy - d, cx, cy + d, 1.8 * s, c);
}

/// The overview button: four small squares.
pub fn draw_overview_button(g: &mut Gfx, r: R, on: bool, accent: [u8; 4], s: f32) {
    let hot = g.hovered(r);
    let v = R::new(r.x + 4.0 * s, r.y + 5.0 * s, r.w - 8.0 * s, r.h - 10.0 * s);
    g.rrect(v.x, v.y, v.w, v.h, 9.0 * s, if on { fx::over(TAB_ON, [accent[0], accent[1], accent[2], 40]) } else if hot { TAB_HOT } else { rgb(15, 19, 36) });
    let c = if on { accent } else if hot { TEXT } else { MUTED };
    let (cx, cy, q, gap) = (v.x + v.w / 2.0, v.y + v.h / 2.0, 4.2 * s, 1.6 * s);
    for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (-1.0, 0.0), (0.0, 0.0)] {
        g.rrect(cx + dx * (q + gap) + if dx < 0.0 { 0.0 } else { gap }, cy + dy * (q + gap) + if dy < 0.0 { 0.0 } else { gap }, q, q, 1.2 * s, c);
    }
}

// ------------------------------------------------------------------------------------------------------------ tool bar

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Reader,
    Notes,
    Scripts,
    Play,
    Dev,
}

/// What the bar needs to know about its contents (widths are measured by the caller).
#[derive(Clone, Debug)]
pub struct BarIn {
    /// The camera / microphone sign before the field (0: none).
    pub cap_w: f32,
    /// The lock chip inside the field's left edge (0: none).
    pub lock_w: f32,
    pub star: bool,
    pub go_w: f32,
    /// The chips after the field, left to right; the last ones give way first when the bar is narrow.
    pub chips: Vec<(Chip, f32)>,
}

#[derive(Clone, Debug)]
pub struct BarGeom {
    pub nav: [R; 4],
    pub cap: Option<R>,
    pub field: R,
    pub lock: Option<R>,
    pub star: Option<R>,
    pub go: R,
    pub chips: Vec<(Chip, R)>,
    /// Room kept free inside the field at its left and right for the lock chip, the star and Go.
    pub inset: (f32, f32),
}

/// Lay the tool bar out in the bar starting at (`x`, `y`) and `w` wide.
pub fn bar_layout(x: f32, y: f32, w: f32, s: f32, b: &BarIn) -> BarGeom {
    let (top, bh) = (y + 6.0 * s, 40.0 * s);
    let mut cx = x + 12.0 * s;
    let nav: [R; 4] = std::array::from_fn(|i| R::new(cx + i as f32 * 44.0 * s, top, 40.0 * s, bh));
    cx += 4.0 * 44.0 * s + 6.0 * s;
    let cap = (b.cap_w > 0.0).then(|| {
        let r = R::new(cx, top, b.cap_w, bh);
        cx += b.cap_w + 6.0 * s;
        r
    });
    let right = x + w - 12.0 * s;
    let go_w = b.go_w.max(44.0 * s);
    let star_w = if b.star { 36.0 * s } else { 0.0 };
    let inset_r = go_w + star_w + 10.0 * s;
    let inset_l = if b.lock_w > 0.0 { b.lock_w + 12.0 * s } else { 0.0 };
    let min_field = 230.0 * s;
    // keep as many chips as leave the field its minimum; the later ones go first
    let mut keep = b.chips.len();
    let chips_w = |n: usize| -> f32 { b.chips.iter().take(n).map(|(_, cw)| cw + 6.0 * s).sum() };
    while keep > 0 && right - cx - chips_w(keep) < min_field {
        keep -= 1;
    }
    let fw = (right - cx - chips_w(keep)).max(60.0 * s);
    let field = R::new(cx, top, fw, bh);
    let go = R::new(field.x + field.w - go_w - 3.0 * s, top + 3.0 * s, go_w, bh - 6.0 * s);
    let star = b.star.then(|| R::new(go.x - star_w - 2.0 * s, top + 2.0 * s, star_w, bh - 4.0 * s));
    let lock = (b.lock_w > 0.0).then(|| R::new(field.x + 8.0 * s, top + (bh - 26.0 * s) / 2.0, b.lock_w, 26.0 * s));
    let mut chips = Vec::new();
    let mut x2 = field.x + field.w + 6.0 * s;
    for (c, cw) in b.chips.iter().take(keep) {
        chips.push((*c, R::new(x2, top, *cw, bh)));
        x2 += cw + 6.0 * s;
    }
    BarGeom { nav, cap, field, lock, star, go, chips, inset: (inset_l, inset_r) }
}

/// A rounded button of the bar with an icon and/or a text (the hit rectangle `r` is taller than the pill drawn in it).
pub fn draw_chip(g: &mut Gfx, r: R, s: f32, label: &str, shield: Option<bool>, color: [u8; 4], strong: bool) {
    let hot = g.hovered(r);
    let pill = R::new(r.x, r.y + (r.h - 30.0 * s) / 2.0, r.w, 30.0 * s);
    let fill = fx::over(if hot { rgb(40, 50, 86) } else { rgb(22, 28, 52) }, [color[0], color[1], color[2], if strong { 44 } else { 14 }]);
    g.rrect(pill.x, pill.y, pill.w, pill.h, pill.h / 2.0, fill);
    g.rrect_outline(pill.x, pill.y, pill.w, pill.h, pill.h / 2.0, 1.1 * s, [color[0], color[1], color[2], if strong { 220 } else if hot { 170 } else { 100 }]);
    let mut tx = pill.x + 12.0 * s;
    if let Some(on) = shield {
        draw_shield(g, pill.x + 12.0 * s + 6.0 * s, pill.y + pill.h / 2.0, 7.0 * s, color, on, s);
        tx += 18.0 * s;
    }
    if !label.is_empty() {
        let lh = g.line_height(13.5 * s, Face::Bold);
        g.text(tx, pill.y + (pill.h - lh) / 2.0, 13.5 * s, fx::readable(color, fill, 4.5), label, Face::Bold);
    }
}

/// The width of a chip with `label` (and an icon when `icon`).
pub fn chip_width(g: &mut Gfx, s: f32, label: &str, icon: bool) -> f32 {
    (g.text_width(13.5 * s, label, Face::Bold) + 24.0 * s + if icon { 18.0 * s } else { 0.0 }).max(40.0 * s)
}

/// A shield (scripts): outlined, with a tick when scripts are allowed and a slash when they are blocked.
pub fn draw_shield(g: &mut Gfx, cx: f32, cy: f32, h: f32, c: [u8; 4], on: bool, s: f32) {
    let (w, top, bot) = (h * 0.85, cy - h, cy + h);
    let pts = [(cx - w, top + h * 0.25), (cx, top), (cx + w, top + h * 0.25), (cx + w * 0.9, cy + h * 0.2), (cx, bot), (cx - w * 0.9, cy + h * 0.2), (cx - w, top + h * 0.25)];
    for k in 0..pts.len() - 1 {
        g.line(pts[k].0, pts[k].1, pts[k + 1].0, pts[k + 1].1, 1.5 * s, c);
    }
    if on {
        g.line(cx - h * 0.4, cy, cx - h * 0.1, cy + h * 0.35, 1.6 * s, c);
        g.line(cx - h * 0.1, cy + h * 0.35, cx + h * 0.45, cy - h * 0.3, 1.6 * s, c);
    } else {
        g.line(cx - w * 1.1, cy + h * 0.9, cx + w * 1.1, cy - h * 0.9, 1.8 * s, c);
    }
}

/// The developer-tools chip's colour: quiet unless something went wrong.
pub fn dev_color(open: bool, failed: usize, errors: usize, collecting: bool, accent: [u8; 4]) -> ([u8; 4], bool) {
    if open {
        (accent, true)
    } else if collecting && errors > 0 {
        (ERR, true)
    } else if collecting && failed > 0 {
        (AMBER, true)
    } else {
        (MUTED, false)
    }
}

/// The developer-tools chip's text: a small `</>` and the number of problems (nothing while there are none).
pub fn dev_text(open: bool, failed: usize, errors: usize, collecting: bool) -> String {
    if open {
        "</> Esc".into()
    } else if collecting && failed + errors > 0 {
        format!("</> {}", failed + errors)
    } else {
        "</>".into()
    }
}

/// The lock chip in the field: a padlock and the word.
pub fn draw_lock(g: &mut Gfx, r: R, text: &str, color: [u8; 4], s: f32) {
    let fill = fx::over(rgb(16, 20, 36), [color[0], color[1], color[2], 30]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, fill);
    g.icon(Icon::Lock, r.x + 12.0 * s, r.y + r.h / 2.0, 12.0 * s, color);
    let lh = g.line_height(12.5 * s, Face::Bold);
    g.text(r.x + 22.0 * s, r.y + (r.h - lh) / 2.0, 12.5 * s, fx::readable(color, fill, 4.5), text, Face::Bold);
}

pub fn lock_width(g: &mut Gfx, s: f32, text: &str) -> f32 {
    g.text_width(12.5 * s, text, Face::Bold) + 32.0 * s
}

pub fn draw_nav_button(g: &mut Gfx, r: R, icon: &str, on: bool, accent: [u8; 4], s: f32) {
    let hot = on && g.hovered(r);
    let v = R::new(r.x + 1.0 * s, r.y + 2.0 * s, r.w - 2.0 * s, r.h - 4.0 * s);
    fx::row_at(g, v.x, v.y, v.w, v.h, 10.0 * s, false, hot, accent);
    let tw = g.text_width(20.0 * s, icon, Face::Bold);
    let lh = g.line_height(20.0 * s, Face::Bold);
    g.text(v.x + (v.w - tw) / 2.0, v.y + (v.h - lh) / 2.0, 20.0 * s, if on { TEXT } else { DIM }, icon, Face::Bold);
}

/// The "Go" button at the right end of the address field.
pub fn draw_go(g: &mut Gfx, r: R, accent: [u8; 4], s: f32) {
    let hot = g.hovered(r);
    let fill = fx::over(rgb(22, 40, 62), [accent[0], accent[1], accent[2], if hot { 90 } else { 50 }]);
    g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, fill);
    let tw = g.text_width(14.5 * s, "Go", Face::Bold);
    let lh = g.line_height(14.5 * s, Face::Bold);
    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, 14.5 * s, fx::readable(accent, fill, 4.5), "Go", Face::Bold);
}

pub fn draw_star(g: &mut Gfx, r: R, starred: bool, s: f32) {
    let hot = g.hovered(r);
    if hot {
        g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(34, 42, 70));
    }
    let star = if starred { "★" } else { "☆" };
    let tw = g.text_width(20.0 * s, star, Face::Bold);
    let lh = g.line_height(20.0 * s, Face::Bold);
    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, 20.0 * s, if starred { rgb(255, 205, 90) } else { MUTED }, star, Face::Bold);
}

// ------------------------------------------------------------------------------------------------------------ loading

/// The thin bar under the tool bar. `progress` is `None` while the length is not known (a sliding segment); otherwise 0-100,
/// shown as at least a small head start so that "0%" is still visibly alive.
pub fn draw_progress(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, progress: Option<u8>, tick: u64, accent: [u8; 4]) {
    let h = 3.0 * s;
    g.rect(x, y, w, h, [accent[0], accent[1], accent[2], 40]);
    match progress {
        Some(p) => {
            let f = (p as f32 / 100.0).clamp(0.06, 1.0);
            g.rect(x, y, w * f, h, accent);
            // a bright sweep running over the filled part says that it is still going
            let sweep = 90.0 * s;
            let pos = (tick as f32 * 6.0 * s) % (w * f + sweep) - sweep;
            let (a, b) = (pos.max(0.0), (pos + sweep).min(w * f));
            if b > a {
                g.rect(x + a, y, b - a, h, [255, 255, 255, 110]);
            }
        }
        None => {
            let seg = (w * 0.25).max(60.0 * s);
            let pos = (tick as f32 * 8.0 * s) % (w + seg) - seg;
            let (a, b) = (pos.max(0.0), (pos + seg).min(w));
            if b > a {
                g.rect(x + a, y, b - a, h, accent);
            }
        }
    }
}

/// The page area before the engine has painted: calm and dark, with a spinner and what is being opened.
pub fn draw_placeholder(g: &mut Gfx, view: R, s: f32, tick: u64, host: &str, accent: [u8; 4]) {
    g.rect(view.x, view.y, view.w, view.h, PLACEHOLDER);
    let cy = view.y + view.h * 0.42;
    spinner(g, view.x + view.w / 2.0, cy, 14.0 * s, tick, accent);
    let msg = if host.is_empty() { "Opening the page…".to_string() } else { format!("Opening {host}…") };
    let msg = g.fit(17.0 * s, &msg, Face::Sans, view.w - 40.0 * s);
    let tw = g.text_width(17.0 * s, &msg, Face::Sans);
    g.text(view.x + (view.w - tw) / 2.0, cy + 30.0 * s, 17.0 * s, MUTED, &msg, Face::Sans);
}

#[derive(Clone, Copy, Debug)]
pub struct HintGeom {
    pub card: R,
    pub text: R,
    pub stop: R,
    pub retry: R,
}

/// The "taking long" card at the top of the page area: a sentence, Stop and Retry.
pub fn hint_layout(view: R, s: f32, stop_w: f32, retry_w: f32) -> HintGeom {
    let w = (view.w - 24.0 * s).min(660.0 * s).max(0.0);
    let card = R::new(view.x + (view.w - w) / 2.0, view.y + 10.0 * s, w, 56.0 * s);
    let retry = R::new(card.x + card.w - 10.0 * s - retry_w, card.y + 8.0 * s, retry_w, 40.0 * s);
    let stop = R::new(retry.x - 8.0 * s - stop_w, card.y + 8.0 * s, stop_w, 40.0 * s);
    let text = R::new(card.x + 16.0 * s, card.y, (stop.x - 12.0 * s - card.x - 16.0 * s).max(0.0), card.h);
    HintGeom { card, text, stop, retry }
}

pub fn draw_button(g: &mut Gfx, r: R, label: &str, color: [u8; 4], s: f32) {
    let hot = g.hovered(r);
    let fill = fx::over(if hot { rgb(40, 50, 86) } else { rgb(24, 31, 56) }, [color[0], color[1], color[2], 36]);
    g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, fill);
    g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.2 * s, [color[0], color[1], color[2], if hot { 255 } else { 190 }]);
    let tw = g.text_width(15.0 * s, label, Face::Bold);
    let lh = g.line_height(15.0 * s, Face::Bold);
    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, 15.0 * s, fx::readable(color, fill, 4.5), label, Face::Bold);
}

pub fn button_width(g: &mut Gfx, s: f32, label: &str) -> f32 {
    (g.text_width(15.0 * s, label, Face::Bold) + 28.0 * s).max(72.0 * s)
}

/// Draw the card and return where its buttons are (the caller registers the click regions).
pub fn draw_hint(g: &mut Gfx, view: R, s: f32, text: &str, accent: [u8; 4]) -> HintGeom {
    let (sw, rw) = (button_width(g, s, "Stop"), button_width(g, s, "Retry"));
    let h = hint_layout(view, s, sw, rw);
    g.rrect_glow(h.card.x, h.card.y, h.card.w, h.card.h, 12.0 * s, 6.0 * s, [0, 0, 0, 90]);
    g.rrect(h.card.x, h.card.y, h.card.w, h.card.h, 12.0 * s, rgb(20, 26, 48));
    g.rrect_outline(h.card.x, h.card.y, h.card.w, h.card.h, 12.0 * s, 1.2 * s, [AMBER[0], AMBER[1], AMBER[2], 170]);
    let t = g.fit(15.0 * s, text, Face::Sans, h.text.w);
    let lh = g.line_height(15.0 * s, Face::Sans);
    g.text(h.text.x, h.text.y + (h.text.h - lh) / 2.0, 15.0 * s, TEXT, &t, Face::Sans);
    draw_button(g, h.stop, "Stop", MUTED, s);
    draw_button(g, h.retry, "Retry", accent, s);
    h
}

// ------------------------------------------------------------------------------------------------------------ error card

/// What went wrong, in plain words: (title, what to try).
pub fn explain_failure(host: &str, detail: &str) -> (String, String) {
    let l = detail.to_lowercase();
    let h = if host.is_empty() { "this site" } else { host };
    if l.contains("certificate") || l.contains("tls") || l.contains("unknownissuer") || l.contains("notvalidforname") || l.contains("expired") {
        (format!("The certificate of {h} cannot be trusted"), "The connection is not private, so nothing was loaded. Check the address and the clock; do not continue if it is a bank or a sign-in page.".into())
    } else if l.contains("network is unreachable") || l.contains("no route") || l.contains("offline") || l.contains("no internet") {
        ("You are offline".into(), "This device has no internet connection right now. Check the network cable or Wi-Fi, then try again.".into())
    } else if l.contains("timed out") || l.contains("timeout") {
        (format!("{h} took too long to answer"), "The site or the network is slow. Try again in a moment.".into())
    } else {
        (format!("Can't reach {h}"), "The site did not answer. It may be down, or the address may be wrong. Check it and try again.".into())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ErrorGeom {
    pub card: R,
    pub retry: R,
}

pub fn error_layout(view: R, s: f32, retry_w: f32) -> ErrorGeom {
    let w = (view.w - 32.0 * s).min(560.0 * s).max(0.0);
    let h = (230.0 * s).min(view.h - 16.0 * s).max(0.0);
    let card = R::new(view.x + (view.w - w) / 2.0, view.y + ((view.h - h) / 2.0).max(8.0 * s) * 0.8, w, h);
    let retry = R::new(card.x + 24.0 * s, card.y + card.h - 24.0 * s - 44.0 * s, retry_w, 44.0 * s);
    ErrorGeom { card, retry }
}

/// A plain page-error card (title, explanation, the address, a Retry button) over a calm dark page area.
pub fn draw_error(g: &mut Gfx, view: R, s: f32, title: &str, body: &str, url: &str, accent: [u8; 4]) -> ErrorGeom {
    g.rect(view.x, view.y, view.w, view.h, PLACEHOLDER);
    let rw = button_width(g, s, "Retry");
    let e = error_layout(view, s, rw);
    g.rrect(e.card.x, e.card.y, e.card.w, e.card.h, 16.0 * s, rgb(20, 26, 48));
    g.rrect_outline(e.card.x, e.card.y, e.card.w, e.card.h, 16.0 * s, 1.2 * s, [ERR[0], ERR[1], ERR[2], 150]);
    let inner = e.card.w - 48.0 * s;
    let (x, mut y) = (e.card.x + 24.0 * s, e.card.y + 22.0 * s);
    let t = g.fit(21.0 * s, title, Face::Bold, inner);
    g.text(x, y, 21.0 * s, TEXT, &t, Face::Bold);
    y += 34.0 * s;
    for line in wrap_words(g, body, 15.0 * s, inner).into_iter().take(3) {
        g.text(x, y, 15.0 * s, MUTED, &line, Face::Sans);
        y += 21.0 * s;
    }
    let u = g.fit(13.0 * s, url, Face::Sans, inner);
    g.text(x, y + 2.0 * s, 13.0 * s, DIM, &u, Face::Sans);
    draw_button(g, e.retry, "Retry", accent, s);
    e
}

fn wrap_words(g: &mut Gfx, text: &str, size: f32, w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if g.text_width(size, &next, Face::Sans) > w && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = next;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// ------------------------------------------------------------------------------------------------------------ overview

#[derive(Clone, Copy, Debug)]
pub struct CardGeom {
    pub idx: usize,
    pub card: R,
    pub close: R,
    pub group: R,
    /// The small button that starts a new group with this tab.
    pub new_group: R,
}

#[derive(Clone, Debug)]
pub struct OverviewGeom {
    pub panel: R,
    pub search: R,
    pub done: R,
    pub cards: Vec<CardGeom>,
    /// Group headers: (group, rectangle) above the first card of each group.
    pub headers: Vec<(u32, R)>,
}

/// Lay out the tab overview: a search box on top and a grid of cards, the tabs of each group under a header of its own. `order`
/// is the tabs as (index, group) in strip order. Cards that do not fit the panel are left out (the search narrows the list).
pub fn overview_layout(view: R, s: f32, order: &[(usize, Option<u32>)]) -> OverviewGeom {
    let panel = R::new(view.x + 12.0 * s, view.y + 8.0 * s, (view.w - 24.0 * s).max(0.0), (view.h - 16.0 * s).max(0.0));
    let pad = 16.0 * s;
    let done = R::new(panel.x + panel.w - pad - 90.0 * s, panel.y + pad, 90.0 * s, 40.0 * s);
    let search = R::new(panel.x + pad, panel.y + pad, (done.x - 10.0 * s - panel.x - pad).max(0.0), 40.0 * s);
    let top = search.y + search.h + 14.0 * s;
    let cols = (((panel.w - 2.0 * pad) / (230.0 * s)).floor() as usize).clamp(1, 6);
    let cw = (panel.w - 2.0 * pad - (cols as f32 - 1.0) * 10.0 * s) / cols as f32;
    let ch = 92.0 * s;
    let (mut x_col, mut y) = (0usize, top);
    let mut cards = Vec::new();
    let mut headers = Vec::new();
    let mut last: Option<Option<u32>> = None;
    for (idx, grp) in order {
        if last != Some(*grp) {
            if x_col != 0 {
                x_col = 0;
                y += ch + 10.0 * s;
            }
            if let Some(g) = grp {
                headers.push((*g, R::new(panel.x + pad, y, panel.w - 2.0 * pad, 26.0 * s)));
                y += 32.0 * s;
            }
            last = Some(*grp);
        }
        let card = R::new(panel.x + pad + x_col as f32 * (cw + 10.0 * s), y, cw, ch);
        if card.y + card.h > panel.y + panel.h - 6.0 * s {
            break;
        }
        let close = R::new(card.x + card.w - 6.0 * s - 34.0 * s, card.y + 6.0 * s, 34.0 * s, 34.0 * s);
        let new_group = R::new(card.x + card.w - 10.0 * s - 34.0 * s, card.y + card.h - 6.0 * s - 34.0 * s, 34.0 * s, 34.0 * s);
        let group = R::new(card.x + 10.0 * s, new_group.y, (new_group.x - 6.0 * s - card.x - 10.0 * s).max(0.0), 34.0 * s);
        cards.push(CardGeom { idx: *idx, card, close, group, new_group });
        x_col += 1;
        if x_col == cols {
            x_col = 0;
            y += ch + 10.0 * s;
        }
    }
    OverviewGeom { panel, search, done, cards, headers }
}
