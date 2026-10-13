//! The futuristic look shared by RADIO and MEDIA: neon cyan and magenta on dark glass, glowing cards, round transport
//! buttons and level bars. Only drawing helpers; the apps decide what they show.

use crate::gfx::{rgb, Gfx};
use crate::ui::{CARD_HOVER, MUTED, R, TEXT};

// the palette and colour maths live in the shared design system (crate::fx); RADIO and MEDIA keep using them from here
pub use crate::fx::{mix, with_alpha, CYAN, MAGENTA, PANEL};

/// A dark glass card with a soft glow around it. `glow` is 0..1 (how strongly it shines).
pub fn glow_card(g: &mut Gfx, card: R, s: f32, a: [u8; 4], b: [u8; 4], glow: f32) {
    g.rrect(card.x - 3.0 * s, card.y - 3.0 * s, card.w + 6.0 * s, card.h + 6.0 * s, 24.0 * s, with_alpha(a, (28.0 * glow) as u8));
    g.rrect(card.x, card.y, card.w, card.h, 22.0 * s, PANEL);
    g.rrect_outline(card.x, card.y, card.w, card.h, 22.0 * s, 1.5 * s, with_alpha(mix(a, b, 0.35), (90.0 + 120.0 * glow) as u8));
}

/// The glyph on a transport button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Previous,
    Play,
    Stop,
    Next,
}

/// A round transport button with its glyph; `lit` is the accent of the main button (None = a quiet one).
/// Returns the rectangle so the caller can register a hit region.
pub fn round_button(g: &mut Gfx, r: R, s: f32, glyph: Glyph, lit: Option<[u8; 4]>) -> R {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let hot = g.hovered(r);
    match lit {
        Some(c) => {
            g.circle(cx, cy, r.w / 2.0 + 5.0 * s, with_alpha(c, if hot { 40 } else { 22 }));
            g.circle(cx, cy, r.w / 2.0, with_alpha(c, if hot { 90 } else { 55 }));
            g.ring(cx, cy, r.w / 2.0, 1.5 * s, c);
        }
        None => {
            g.circle(cx, cy, r.w / 2.0, if hot { CARD_HOVER } else { rgb(18, 24, 46) });
            g.ring(cx, cy, r.w / 2.0, 1.5 * s, with_alpha(MUTED, 160));
        }
    }
    let k = r.w * 0.17;
    let c = if lit.is_some() { TEXT } else { MUTED };
    match glyph {
        Glyph::Previous => {
            g.polygon(&[(cx + k, cy - k), (cx + k, cy + k), (cx - k * 0.6, cy)], c);
            g.rect(cx - k * 1.1, cy - k, 2.5 * s, 2.0 * k, c);
        }
        Glyph::Stop => g.rect(cx - k * 0.9, cy - k * 0.9, k * 1.8, k * 1.8, c),
        Glyph::Play => g.polygon(&[(cx - k * 0.7, cy - k * 1.1), (cx - k * 0.7, cy + k * 1.1), (cx + k * 1.2, cy)], c),
        Glyph::Next => {
            g.polygon(&[(cx - k, cy - k), (cx - k, cy + k), (cx + k * 0.6, cy)], c);
            g.rect(cx + k * 0.9, cy - k, 2.5 * s, 2.0 * k, c);
        }
    }
    r
}

/// Glowing level bars along `base` (the bottom edge), `levels` 0..1 each, coloured from `a` to `b`.
pub fn level_bars(g: &mut Gfx, x: f32, base: f32, w: f32, max_h: f32, s: f32, levels: &[f32], a: [u8; 4], b: [u8; 4]) {
    let n = levels.len().max(1);
    let bw = w / n as f32;
    for (i, l) in levels.iter().enumerate() {
        let c = mix(a, b, i as f32 / (n.max(2) - 1) as f32);
        let hgt = (l.clamp(0.0, 1.0) * max_h).max(3.0 * s);
        g.rrect(x + i as f32 * bw - 1.0 * s, base - hgt - 1.0 * s, bw - 1.0 * s, hgt + 2.0 * s, 3.0 * s, with_alpha(c, 40));
        g.rrect(x + i as f32 * bw, base - hgt, bw - 4.0 * s, hgt, 2.0 * s, c);
        g.rect(x + i as f32 * bw, base - hgt - 5.0 * s, bw - 4.0 * s, 2.0 * s, with_alpha(TEXT, 170));
    }
}
