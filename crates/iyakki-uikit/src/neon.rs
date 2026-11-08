//! The futuristic look shared by RADIO and MEDIA: neon cyan and magenta on dark glass, glowing cards, round transport
//! buttons and level bars. Only drawing helpers; the apps decide what they show.

use crate::gfx::{rgb, Gfx};
use crate::ui::{CARD_HOVER, MUTED, R, TEXT};

// the palette and colour maths live in the shared design system (crate::fx); RADIO and MEDIA keep using them from here
pub use crate::fx::{mix, with_alpha, CYAN, MAGENTA, PANEL};

/// A dark glass card with a soft glow around it. `glow` is 0..1 (how strongly it shines).
pub fn glow_card(g: &mut Gfx, card: R, s: f32, a: [u8; 4], b: [u8; 4], glow: f32) {
    let _ = glow_card_keep(g, card, s, a, b, glow);
}

/// The old way of drawing the card: three shapes over the whole card. Kept for small cards and as the reference the fast
/// version is tested against.
pub fn glow_card_whole(g: &mut Gfx, card: R, s: f32, a: [u8; 4], b: [u8; 4], glow: f32) {
    g.rrect(card.x - 3.0 * s, card.y - 3.0 * s, card.w + 6.0 * s, card.h + 6.0 * s, 24.0 * s, with_alpha(a, (28.0 * glow) as u8));
    g.rrect(card.x, card.y, card.w, card.h, 22.0 * s, PANEL);
    g.rrect_outline(card.x, card.y, card.w, card.h, 22.0 * s, 1.5 * s, with_alpha(mix(a, b, 0.35), (90.0 + 120.0 * glow) as u8));
}

/// What was under the ring of a glowing card, so that the glow can be drawn again (it pulses with the sound level) without
/// drawing the card again: only the ring around the card's edge changes with `glow`, the inside is opaque.
pub struct GlowRing {
    card: R,
    s: f32,
    /// The four strips of the ring (top, bottom, left, right) as integer rectangles `(x, y, w, h)`, and what was under them.
    strips: [(i32, i32, i32, i32); 4],
    under: [Vec<u8>; 4],
    depth: i32,
}

/// The ring around a card's edge: outward the glow's 3 px, inward far enough that the rest of the card is plain panel colour
/// (the corners are round: a point must be inside the corner circle for the fill below to match the shape).
/// How far the ring reaches into the card: a point this deep in from both sides of a corner is inside the round corner, so
/// everything deeper is plain panel colour.
pub fn ring_depth(s: f32) -> i32 {
    (22.0 * s * 0.2929).ceil() as i32 + 1
}

/// The pixels a glowing card and its glow can touch: the glow reaches 3 px beyond the card.
fn outer_box(card: R, s: f32) -> (i32, i32, i32, i32) {
    ((card.x - 3.0 * s).floor() as i32, (card.y - 3.0 * s).floor() as i32, (card.x + card.w + 3.0 * s).ceil() as i32, (card.y + card.h + 3.0 * s).ceil() as i32)
}

/// The rectangle `(x, y, w, h)` a glowing card and its glow can touch.
pub fn glow_bounds(card: R, s: f32) -> (i32, i32, i32, i32) {
    let (x0, y0, x1, y1) = outer_box(card, s);
    (x0, y0, x1 - x0, y1 - y0)
}

fn ring_strips(card: R, s: f32, screen: (i32, i32)) -> Option<([(i32, i32, i32, i32); 4], (i32, i32, i32, i32))> {
    let inn = ring_depth(s);
    let (ox0, oy0, ox1, oy1) = outer_box(card, s);
    let (x0, y0, x1, y1) = (card.x.floor() as i32, card.y.floor() as i32, (card.x + card.w).ceil() as i32, (card.y + card.h).ceil() as i32);
    if x1 - x0 < 8 * inn || y1 - y0 < 8 * inn || ox0 < 0 || oy0 < 0 || ox1 > screen.0 || oy1 > screen.1 {
        return None;
    }
    let strips = [(ox0, oy0, ox1 - ox0, y0 + inn - oy0), (ox0, y1 - inn, ox1 - ox0, oy1 - (y1 - inn)), (ox0, y0 + inn, x0 + inn - ox0, y1 - y0 - 2 * inn), (x1 - inn, y0 + inn, ox1 - (x1 - inn), y1 - y0 - 2 * inn)];
    Some((strips, (x0 + inn, y0 + inn, x1 - x0 - 2 * inn, y1 - y0 - 2 * inn)))
}

fn paint_ring(g: &mut Gfx, card: R, s: f32, a: [u8; 4], b: [u8; 4], glow: f32, strips: &[(i32, i32, i32, i32); 4], under: Option<&[Vec<u8>; 4]>) {
    for (k, &rect) in strips.iter().enumerate() {
        g.in_region(rect, under.map(|u| u[k].as_slice()), |g, ox, oy| {
            g.rrect(card.x - 3.0 * s - ox, card.y - 3.0 * s - oy, card.w + 6.0 * s, card.h + 6.0 * s, 24.0 * s, with_alpha(a, (28.0 * glow) as u8));
            g.rrect(card.x - ox, card.y - oy, card.w, card.h, 22.0 * s, PANEL);
            g.rrect_outline(card.x - ox, card.y - oy, card.w, card.h, 22.0 * s, 1.5 * s, with_alpha(mix(a, b, 0.35), (90.0 + 120.0 * glow) as u8));
        });
    }
}

/// [`glow_card`] that also returns what it needs to draw the glow again later ([`glow_ring_again`]). Draws the three shapes only
/// inside a ring around the edge and fills the rest with the panel colour: the same picture for a big card at a fraction of
/// the cost (a 2800x1500 card took 8 ms). `None` for a small card, which is drawn the old way.
pub fn glow_card_keep(g: &mut Gfx, card: R, s: f32, a: [u8; 4], b: [u8; 4], glow: f32) -> Option<GlowRing> {
    let Some((strips, inner)) = ring_strips(card, s, (g.width() as i32, g.height() as i32)).filter(|_| PANEL[3] == 255) else {
        glow_card_whole(g, card, s, a, b, glow);
        return None;
    };
    // what is under the ring first (the strips do not overlap, so nothing is blended twice), then the inside, then the ring
    let under: [Vec<u8>; 4] = std::array::from_fn(|k| g.grab(strips[k]));
    g.rect(inner.0 as f32, inner.1 as f32, inner.2 as f32, inner.3 as f32, PANEL);
    paint_ring(g, card, s, a, b, glow, &strips, Some(&under));
    Some(GlowRing { card, s, strips, under, depth: ring_depth(s) })
}

/// Draw the glow of a card again with another strength: only the ring is painted, over what was under it.
pub fn glow_ring_again(g: &mut Gfx, ring: &GlowRing, a: [u8; 4], b: [u8; 4], glow: f32) {
    paint_ring(g, ring.card, ring.s, a, b, glow, &ring.strips, Some(&ring.under));
}

impl GlowRing {
    /// How far the ring reaches into the card: everything this close to the edge is repainted with the glow.
    pub fn depth(&self) -> i32 {
        self.depth
    }

    /// The rectangles that [`glow_ring_again`] writes.
    pub fn rects(&self) -> [(i32, i32, i32, i32); 4] {
        self.strips
    }
}

/// The two alpha values a glow strength draws with: equal values draw the same picture.
pub fn glow_alphas(glow: f32) -> (u8, u8) {
    ((28.0 * glow) as u8, (90.0 + 120.0 * glow) as u8)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn backdrop(w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        for y in 0..h {
            for x in 0..w {
                g.rect(x as f32, y as f32, 1.0, 1.0, rgb(((x * 7 + y * 3) % 90) as u8 + 10, ((x * 2 + y * 5) % 80) as u8 + 20, ((x + y) % 70) as u8 + 30));
            }
        }
        g
    }

    /// The two pictures agree except for the soft edge of the card: a pixel that differs by more than 3 lies within a few pixels
    /// of the card's outline (where a shape's edge falls on the border between two sub-pixel steps, a different rounding of
    /// the coordinates gives the neighbouring coverage), and by no more than 120 (half a coverage step of the bright outline); the inside and the far outside are equal.
    fn same_picture(a: &Gfx, b: &Gfx, card: R, s: f32) {
        let w = a.width() as usize;
        let near = 8.0 * s + 2.0;
        let mut worst = 0;
        for (i, (p, q)) in a.rgba().chunks_exact(4).zip(b.rgba().chunks_exact(4)).enumerate() {
            let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
            worst = worst.max(d);
            if d > 3 {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let inside = x > card.x + near && x < card.x + card.w - near && y > card.y + near && y < card.y + card.h - near;
                let outside = x < card.x - near || x > card.x + card.w + near || y < card.y - near || y > card.y + card.h + near;
                assert!(!inside && !outside, "pixel ({x}, {y}) differs by {d} away from the outline of {card:?}");
            }
        }
        assert!(worst <= 120, "worst difference {worst}");
    }

    #[test]
    fn the_fast_glow_card_is_the_same_picture_as_the_three_shapes_over_the_whole_card() {
        for (s, glow) in [(0.9f32, 0.15f32), (1.0, 0.3), (1.147, 1.0), (2.0, 0.65)] {
            let card = R::new(20.0 * s, 15.0 * s + 0.4, 520.0 * s + 0.3, 260.0 * s);
            let (w, h) = ((card.x + card.w) as u32 + 40, (card.y + card.h) as u32 + 40);
            let (mut fast, mut whole) = (backdrop(w, h), backdrop(w, h));
            let ring = glow_card_keep(&mut fast, card, s, CYAN, MAGENTA, glow);
            glow_card_whole(&mut whole, card, s, CYAN, MAGENTA, glow);
            assert!(ring.is_some(), "scale {s}: a big card takes the fast path");
            same_picture(&fast, &whole, card, s);
        }
    }

    #[test]
    fn drawing_the_glow_again_gives_the_picture_of_that_strength_and_touches_only_the_ring() {
        let s = 1.0;
        let card = R::new(30.0, 30.0, 400.0, 200.0);
        let (w, h) = (470, 270);
        let mut g = backdrop(w, h);
        let ring = glow_card_keep(&mut g, card, s, CYAN, MAGENTA, 0.3).expect("fast path");
        // something drawn on the card afterwards (the text) must survive a new glow
        g.rect(60.0, 80.0, 100.0, 20.0, rgb(250, 10, 10));
        let text = g.rgba().to_vec();
        glow_ring_again(&mut g, &ring, CYAN, MAGENTA, 1.0);
        let mut want = backdrop(w, h);
        glow_card_whole(&mut want, card, s, CYAN, MAGENTA, 1.0);
        want.rect(60.0, 80.0, 100.0, 20.0, rgb(250, 10, 10));
        same_picture(&g, &want, card, s);
        // pixels away from the ring are exactly as they were
        let row = |g: &[u8], y: u32| g[(y * w * 4) as usize..((y + 1) * w * 4) as usize].to_vec();
        assert_eq!(row(g.rgba(), 130)[(100 * 4) as usize..(300 * 4) as usize], row(&text, 130)[(100 * 4) as usize..(300 * 4) as usize], "the middle of the card was not drawn again");
        for r in ring.rects() {
            assert!(r.0 >= 0 && r.1 >= 0 && r.0 + r.2 <= w as i32 && r.1 + r.3 <= h as i32, "{r:?} is on the screen");
        }
        assert_eq!(glow_alphas(0.3), glow_alphas(0.3));
        assert_ne!(glow_alphas(0.3), glow_alphas(1.0));
    }

    #[test]
    fn a_small_card_is_drawn_the_old_way() {
        let mut g = backdrop(200, 120);
        assert!(glow_card_keep(&mut g, R::new(10.0, 10.0, 60.0, 40.0), 1.0, CYAN, MAGENTA, 0.5).is_none());
        assert!(glow_bounds(R::new(10.0, 10.0, 60.0, 40.0), 1.0).0 <= 7);
    }
}

