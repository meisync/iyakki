//! Night mode for pages that paint themselves light: every colour the page chose has its lightness turned over (hue and
//! colourfulness stay), so white pages become dark ones and text stays readable. Pages that do not paint a light
//! background are left alone, and pictures are not touched.

use iyakki_web::css::Rgba;
use iyakki_web::doc::{Block, Column, Doc, Span};

fn to_hsl(c: Rgba) -> (f32, f32, f32) {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - r).abs() < 1e-6 {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32, a: u8) -> Rgba {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(r), q(g), q(b), a]
}

/// A colour with its lightness turned over, kept away from pure black and white.
pub fn flip(c: Rgba) -> Rgba {
    let (h, s, l) = to_hsl(c);
    from_hsl(h, s * 0.9, (1.0 - l).clamp(0.07, 0.93), c[3])
}

fn luminance(c: Rgba) -> f32 {
    to_hsl(c).2
}

fn spans(v: &mut [Span]) {
    for s in v {
        s.style.color = s.style.color.map(flip);
        s.style.bg = s.style.bg.map(flip);
    }
}

fn blocks(v: &mut [Block]) {
    for b in v {
        match b {
            Block::Text(t) => spans(&mut t.spans),
            Block::Box(x) => {
                x.style.bg = x.style.bg.map(flip);
                x.style.border_c = x.style.border_c.map(flip);
                blocks(&mut x.children);
            }
            Block::Row(r) => {
                r.style.bg = r.style.bg.map(flip);
                r.style.border_c = r.style.border_c.map(flip);
                for c in &mut r.cols {
                    let Column { style, blocks: inner, .. } = c;
                    style.bg = style.bg.map(flip);
                    style.border_c = style.border_c.map(flip);
                    blocks(inner);
                }
            }
            Block::Table(t) => {
                t.bg = t.bg.map(flip);
                for row in &mut t.rows {
                    for cell in row {
                        cell.bg = cell.bg.map(flip);
                        blocks(&mut cell.blocks);
                    }
                }
            }
            Block::Pre(_) | Block::Rule => {}
        }
    }
}

/// Whether a page background colour is light.
pub fn is_light_color(c: Rgba) -> bool {
    c[3] > 200 && luminance(c) > 0.6
}

/// Whether the page paints a light background of its own (the only pages night mode changes).
pub fn is_light(doc: &Doc) -> bool {
    doc.canvas.is_some_and(|c| c[3] > 200 && luminance(c) > 0.6)
}

/// The page in night colours, or `None` when it does not paint a light background.
pub fn darken(doc: &Doc) -> Option<Doc> {
    if !is_light(doc) {
        return None;
    }
    let mut d = doc.clone();
    d.canvas = d.canvas.map(flip);
    blocks(&mut d.blocks);
    for b in &mut d.inlines {
        b.style.bg = b.style.bg.map(flip);
        b.style.border_c = b.style.border_c.map(flip);
        blocks(&mut b.children);
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_colours_turn_dark_and_dark_ones_light_keeping_the_hue() {
        let white = flip([255, 255, 255, 255]);
        assert!(luminance(white) < 0.12 && white[3] == 255, "{white:?}");
        let ink = flip([20, 20, 20, 255]);
        assert!(luminance(ink) > 0.85);
        let red = flip([200, 30, 30, 255]);
        assert!(red[0] > red[1] && red[0] > red[2], "still reddish: {red:?}");
        assert!(luminance(red) > 0.4, "{red:?}");
        assert_eq!(flip([10, 20, 30, 77])[3], 77, "transparency stays");
    }

    #[test]
    fn only_pages_with_a_light_background_are_changed() {
        let mut doc = iyakki_web::doc::from_html("<p>hi</p>");
        assert!(darken(&doc).is_none(), "no background of its own");
        doc.canvas = Some([250, 250, 250, 255]);
        let d = darken(&doc).unwrap();
        assert!(luminance(d.canvas.unwrap()) < 0.15);
        doc.canvas = Some([10, 10, 30, 255]);
        assert!(darken(&doc).is_none(), "a dark page stays as it is");
    }
}
