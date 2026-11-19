//! A few shared widgets over `Canvas`, so every app looks the same and every control has a name, a role and a large enough target.
//! Each one draws inside the rectangle it is given (never outside it, at any size class or text size) and returns what it used.
//! Radii, borders, gaps, shadows, glass and text sizes all come from the skin's tokens through `Metrics` (`m.tokens`, `m.gap`,
//! `m.size`): this file has no number of its own for them (a test scans for that).

use iyakki_uikit::theme::Theme;
use iyakki_uikit::wallpaper;

use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::geom::{Color, Rect};
use crate::size::{Metrics, TypeRole};
use crate::tokens::{Line, Radius, Space};

/// The preferred width of a wallpaper swatch.
const SWATCH_W: f32 = 120.0;
/// The alpha of a glass surface (a translucent fill over what is behind).
const GLASS_ALPHA: u8 = 200;

/// A fill as the skin wants it: translucent when the skin has glass, else the plain colour.
pub fn glass_fill(m: &Metrics, c: Color) -> Color {
    if m.tokens.glass { [c[0], c[1], c[2], c[3].min(GLASS_ALPHA)] } else { c }
}

/// A raised surface: the skin's soft shadow (none when flat), then the fill. Returns the radius used.
pub fn surface(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, fill: Color, radius: Radius, raised: bool) -> f32 {
    let rad = m.tokens.radius_for(radius, r.h.min(r.w));
    if raised && m.tokens.shadow_spread() > 0.0 {
        c.shadow(r, rad, m.tokens.shadow_spread(), [0, 0, 0, m.tokens.shadow_alpha()]);
    }
    if m.tokens.glass {
        // frost what lies behind the translucent fill (a canvas without blur ignores it)
        c.blur(r, m.tokens.blur);
    }
    c.rrect(r, rad, glass_fill(m, fill));
    if m.tokens.glass {
        c.outline(r, rad, m.line(Line::Hair), th.border);
    }
    rad
}

/// A panel or card of the skin (large corners, raised, glass when the skin has it).
pub fn panel(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) {
    surface(c, th, m, r, th.panel, Radius::Lg, true);
}

/// A button. Its height is at least the smallest target for the pointer in use. Returns the rectangle drawn.
pub fn button(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, label: &str, action: u32, primary: bool) -> Rect {
    let r = Rect::new(r.x, r.y, r.w, r.h.max(m.min_target()));
    surface(c, th, m, r, if primary { th.accent } else { th.button }, Radius::Md, primary);
    let size = m.size(TypeRole::Label);
    let t = fit(c, size, label, TextStyle::Bold, (r.w - 2.0 * m.gap(Space::Lg) - 2.0 * m.gap(Space::Xxs)).max(0.0));
    let w = c.measure(size, &t, TextStyle::Bold);
    let ink = if primary { iyakki_uikit::skins::label_on(th, th.accent) } else { th.text };
    c.text(r.x + ((r.w - w) / 2.0).max(0.0), r.y + ((r.h - size) / 2.0).max(0.0), size, ink, &t, TextStyle::Bold);
    c.hit(r, Hit { action, name: label.to_string(), role: Role::Button });
    r
}

/// One row of a list: the text, a tint when selected, a name for the screen reader.
pub fn row(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, text: &str, selected: bool, action: u32) -> Rect {
    let r = Rect::new(r.x, r.y, r.w, r.h.max(m.min_target()));
    surface(c, th, m, r, if selected { th.card_sel } else { th.card }, Radius::Sm, false);
    let size = m.size(TypeRole::Label);
    let pad = m.gap(Space::Md) + m.gap(Space::Xxs);
    let t = fit(c, size, text, TextStyle::Regular, (r.w - 2.0 * pad).max(0.0));
    c.text(r.x + pad, r.y + ((r.h - size) / 2.0).max(0.0), size, th.text, &t, TextStyle::Regular);
    c.hit(r, Hit { action, name: text.to_string(), role: Role::ListItem });
    r
}

/// A one-line text field showing `value` (or the `label` dimmed when empty), with a caret when focused.
pub fn field(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, label: &str, value: &str, focused: bool, action: u32) -> Rect {
    let r = Rect::new(r.x, r.y, r.w, r.h.max(m.min_target()));
    let rad = m.tokens.radius_for(Radius::Sm, r.h);
    c.rrect(r, rad, glass_fill(m, th.field));
    c.outline(r, rad, if focused { m.tokens.focus_width() } else { m.line(Line::Normal) }, if focused { th.accent } else { th.border });
    let size = m.size(TypeRole::Label);
    let y = r.y + ((r.h - size) / 2.0).max(0.0);
    let pad = m.gap(Space::Md) + m.gap(Space::Xxs);
    let max_w = (r.w - 2.0 * pad).max(0.0);
    if value.is_empty() {
        let t = fit(c, size, label, TextStyle::Regular, max_w);
        c.text(r.x + pad, y, size, th.dim, &t, TextStyle::Regular);
    } else {
        // show the end of what was typed, so the caret is always visible
        let mut shown: String = value.to_string();
        while c.measure(size, &shown, TextStyle::Regular) > max_w - m.gap(Space::Sm) && shown.chars().count() > 1 {
            shown.remove(0);
        }
        let w = c.text(r.x + pad, y, size, th.text, &shown, TextStyle::Regular);
        if focused {
            c.rect(Rect::new(r.x + pad + w + m.line(Line::Normal), y, m.line(Line::Thick), size * 1.2), th.accent);
        }
    }
    c.hit(r, Hit { action, name: label.to_string(), role: Role::Field });
    r
}

/// The wallpaper picker: one swatch per wallpaper with its name; swatch `i` has the action `base + i`. The chosen one has a ring.
/// Wraps to more lines when the width is small. Returns the rectangle used.
pub fn wallpaper_picker(c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, current: wallpaper::Id, base: u32) -> Rect {
    let h = m.min_target().max(m.size(TypeRole::Display) * 1.4);
    let per_row = ((r.w / SWATCH_W).floor() as usize).clamp(1, wallpaper::ALL.len());
    let cell_w = r.w / per_row as f32;
    let mut used = 0.0f32;
    for (i, id) in wallpaper::ALL.iter().enumerate() {
        let (col, line) = (i % per_row, i / per_row);
        let gap = m.gap(Space::Xxs);
        let cell = Rect::new(r.x + col as f32 * cell_w + gap, r.y + line as f32 * (h + m.gap(Space::Sm)), (cell_w - 2.0 * gap).max(0.0), h);
        if cell.bottom() > r.bottom() {
            break;
        }
        let rad = surface(c, th, m, cell, if *id == current { th.card_sel } else { th.card }, Radius::Md, false);
        if *id == current {
            c.outline(cell, rad, m.tokens.focus_width(), th.accent);
        }
        let size = m.size(TypeRole::Body);
        let t = fit(c, size, id.name(), TextStyle::Regular, (cell.w - m.gap(Space::Md) - m.gap(Space::Lg)).max(0.0));
        c.text(cell.x + m.gap(Space::Md), cell.y + ((cell.h - size) / 2.0).max(0.0), size, th.text, &t, TextStyle::Regular);
        c.hit(cell, Hit { action: base + i as u32, name: format!("Wallpaper {}", id.name()), role: Role::Button });
        used = used.max(cell.bottom() - r.y);
    }
    Rect::new(r.x, r.y, r.w, used)
}
