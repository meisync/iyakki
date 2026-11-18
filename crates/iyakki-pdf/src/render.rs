//! Page rendering to a bitmap. Text is drawn with the glyphs a [`GlyphSource`] supplies (the viewer passes its own fonts; without one
//! boxes are drawn), at the position and width the PDF gives. Paths, strokes, clips, images and form XObjects follow the PDF model;
//! patterns, shadings and soft masks do not.

use crate::geom::*;
use crate::image::{self, Failure};
use crate::interp::*;
use crate::obj::*;
use crate::raster::*;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

pub struct GlyphBitmap {
    /// Offset of the bitmap's top-left from the pen position on the baseline (y grows downward, so `top` is usually negative).
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontStyle {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub serif: bool,
}

pub trait GlyphSource {
    /// A glyph for `ch` at `px` pixels per em, or None when the source has none (a box is drawn instead).
    fn glyph(&self, ch: char, style: FontStyle, px: f32) -> Option<GlyphBitmap>;
}

/// No fonts: every glyph is a box. Used in tests (stable output) and when a viewer has no font.
pub struct BoxGlyphs;

impl GlyphSource for BoxGlyphs {
    fn glyph(&self, ch: char, _s: FontStyle, px: f32) -> Option<GlyphBitmap> {
        if ch.is_whitespace() {
            return Some(GlyphBitmap { left: 0, top: 0, w: 0, h: 0, alpha: vec![] });
        }
        let w = ((px * 0.5).round() as i32).clamp(2, 400);
        let h = ((px * 0.7).round() as i32).clamp(2, 400);
        let mut a = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    a[(y * w + x) as usize] = 255;
                }
            }
        }
        Some(GlyphBitmap { left: 0, top: -h, w: w as u32, h: h as u32, alpha: a })
    }
}

pub struct RenderSink<'g> {
    pub canvas: Canvas,
    clip: Clip,
    stack: Vec<Clip>,
    glyphs: &'g dyn GlyphSource,
    cache: HashMap<(char, FontStyle, u32), Option<Rc<GlyphBitmap>>>,
    pub notes: BTreeSet<String>,
    pub draws: usize,
}

const MAX_DRAWS: usize = 3_000_000;

impl<'g> RenderSink<'g> {
    pub fn new(w: u32, h: u32, glyphs: &'g dyn GlyphSource) -> RenderSink<'g> {
        RenderSink { canvas: Canvas::new(w, h), clip: Clip::full(w, h), stack: vec![], glyphs, cache: HashMap::new(), notes: BTreeSet::new(), draws: 0 }
    }

    fn bounds(&self) -> (i32, i32, i32, i32) {
        (self.clip.x0, self.clip.y0, self.clip.x1, self.clip.y1)
    }

    fn glyph_bitmap(&mut self, ch: char, st: FontStyle, px: f32) -> Option<Rc<GlyphBitmap>> {
        let key = (ch, st, (px * 2.0).round() as u32);
        if let Some(c) = self.cache.get(&key) {
            return c.clone();
        }
        let g = self.glyphs.glyph(ch, st, px).map(Rc::new);
        if self.cache.len() > 5000 {
            self.cache.clear();
        }
        self.cache.insert(key, g.clone());
        g
    }
}

impl<'g> Sink for RenderSink<'g> {
    fn save(&mut self) {
        if self.stack.len() < 1024 {
            self.stack.push(self.clip.clone());
        }
    }
    fn restore(&mut self) {
        if let Some(c) = self.stack.pop() {
            self.clip = c;
        }
    }
    fn clip(&mut self, path: &[Seg], even_odd: bool) {
        if self.clip.empty() {
            return;
        }
        let polys = polygons_of(path);
        self.clip = clip_with(&self.clip, &polys, even_odd);
    }
    fn fill(&mut self, path: &[Seg], even_odd: bool, gs: &GState) {
        if self.clip.empty() || self.draws >= MAX_DRAWS {
            return;
        }
        self.draws += 1;
        if matches!(gs.fill_cs, crate::color::Cs::Pattern) {
            self.notes.insert("pattern fills (drawn grey)".into());
        }
        let polys = polygons_of(path);
        if let Some(c) = coverage(&polys, even_odd, self.bounds()) {
            let clip = self.clip.clone();
            self.canvas.fill_cov(&c, &clip, gs.fill, gs.fill_alpha);
        }
    }
    fn stroke(&mut self, path: &[Seg], gs: &GState) {
        if self.clip.empty() || self.draws >= MAX_DRAWS {
            return;
        }
        self.draws += 1;
        let sc = gs.ctm.scale() as f32;
        let st = StrokeStyle { width: (gs.line_width as f32 * sc).max(0.8), cap: gs.cap, join: gs.join, miter: gs.miter as f32, dash: gs.dash.iter().map(|d| (*d as f32 * sc).max(0.2)).collect(), phase: gs.phase as f32 * sc };
        let polys = stroke_polys(path, &st);
        if let Some(c) = coverage(&polys, false, self.bounds()) {
            let clip = self.clip.clone();
            self.canvas.fill_cov(&c, &clip, gs.stroke, gs.stroke_alpha);
        }
    }
    fn glyph(&mut self, g: &Glyph, gs: &GState) {
        if self.clip.empty() || self.draws >= MAX_DRAWS || matches!(gs.text.mode, 3 | 7) {
            return;
        }
        self.draws += 1;
        let m = &g.trm.0;
        let (ox, oy) = g.trm.apply(0.0, 0.0);
        let px = (m[2] * m[2] + m[3] * m[3]).sqrt() as f32;
        if !px.is_finite() || px < 0.5 || px > 2000.0 || !ox.is_finite() || !oy.is_finite() {
            return;
        }
        let rotated = (m[1].abs() + m[2].abs()) > 0.2 * (m[0].abs() + m[3].abs());
        let clip = self.clip.clone();
        if rotated {
            self.notes.insert("rotated text (drawn as a line)".into());
            let (ex, ey) = g.trm.apply(g.advance, 0.0);
            let st = StrokeStyle { width: (px * 0.08).max(1.0), cap: 0, join: 0, miter: 4.0, dash: vec![], phase: 0.0 };
            let path = [Seg::Move(ox, oy), Seg::Line(ex, ey)];
            if let Some(c) = coverage(&stroke_polys(&path, &st), false, self.bounds()) {
                self.canvas.fill_cov(&c, &clip, gs.fill, gs.fill_alpha * 0.8);
            }
            return;
        }
        let style = FontStyle { bold: g.font.bold, italic: g.font.italic, mono: g.font.mono, serif: g.font.serif };
        let chars: Vec<char> = g.text.chars().collect();
        if chars.is_empty() {
            self.notes.insert("text in fonts without a Unicode map (drawn as boxes)".into());
        }
        let n = chars.len().max(1);
        let step = g.advance * m[0] / n as f64;
        for i in 0..n {
            let ch = chars.get(i).copied().unwrap_or('\u{25a1}');
            let gx = ox + step * i as f64;
            let flip = m[3] > 0.0; // y axis pointing down on screen: upside-down page content is not mirrored here
            let _ = flip;
            if let Some(bm) = self.glyph_bitmap(ch, style, px) {
                let (bx, by) = ((gx.round() as i32).saturating_add(bm.left), (oy.round() as i32).saturating_add(bm.top));
                for yy in 0..bm.h as i32 {
                    for xx in 0..bm.w as i32 {
                        let a = bm.alpha[(yy as usize) * bm.w as usize + xx as usize];
                        if a == 0 {
                            continue;
                        }
                        let (x, y) = (bx + xx, by + yy);
                        let mk = clip.alpha(x, y);
                        if mk == 0 {
                            continue;
                        }
                        self.canvas.blend(x, y, gs.fill, a as f32 / 255.0 * mk as f32 / 255.0 * gs.fill_alpha);
                    }
                }
            }
        }
    }
    fn image(&mut self, f: &crate::file::File, src: ImageSrc, res: &Dict, gs: &GState) {
        if self.clip.empty() || self.draws >= MAX_DRAWS {
            return;
        }
        self.draws += 1;
        let result = match &src {
            ImageSrc::Xobject(o) => match o.dict() {
                Some(d) => image::decode(f, d, o, None, res),
                None => Err(Failure::Bad("no dictionary".into())),
            },
            ImageSrc::Inline(d, data) => image::decode(f, d, &Obj::Null, Some(data), res),
        };
        let img = match result {
            Ok(i) => i,
            Err(e) => {
                let why = match e {
                    Failure::Unsupported(s) => format!("{s} are not shown (a grey box is drawn)"),
                    Failure::Bad(_) => "pictures that could not be read (a grey box is drawn)".to_string(),
                };
                self.notes.insert(why);
                let m = gs.ctm;
                let pts: Vec<(f32, f32)> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|(x, y)| m.apply(*x, *y)).map(|(x, y)| (x as f32, y as f32)).collect();
                if let Some(c) = coverage(&[pts], false, self.bounds()) {
                    let clip = self.clip.clone();
                    self.canvas.fill_cov(&c, &clip, [0.82, 0.82, 0.84], 1.0);
                }
                return;
            }
        };
        let Some(inv) = gs.ctm.invert() else { return };
        let corners: Vec<(f64, f64)> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|(x, y)| gs.ctm.apply(*x, *y)).collect();
        let minx = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor().max(self.clip.x0 as f64) as i32;
        let maxx = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil().min(self.clip.x1 as f64) as i32;
        let miny = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor().max(self.clip.y0 as f64) as i32;
        let maxy = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil().min(self.clip.y1 as f64) as i32;
        let (iw, ih) = (img.w as f64, img.h as f64);
        // source pixels per device pixel along each image axis
        let sx = (inv.0[0].hypot(inv.0[1]) * iw).max(0.01);
        let sy = (inv.0[2].hypot(inv.0[3]) * ih).max(0.01);
        let kx = (sx.floor() as usize).clamp(1, 6);
        let ky = (sy.floor() as usize).clamp(1, 6);
        let clip = self.clip.clone();
        for y in miny..maxy {
            for x in minx..maxx {
                let (u, v) = inv.apply(x as f64 + 0.5, y as f64 + 0.5);
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let mk = clip.alpha(x, y);
                if mk == 0 {
                    continue;
                }
                let (cx, cy) = (u * iw, (1.0 - v) * ih);
                let (mut r, mut g, mut b, mut a, mut cnt) = (0f32, 0f32, 0f32, 0f32, 0f32);
                for j in 0..ky {
                    for i in 0..kx {
                        let px = cx + ((i as f64 + 0.5) / kx as f64 - 0.5) * sx;
                        let py = cy + ((j as f64 + 0.5) / ky as f64 - 0.5) * sy;
                        let (ix, iy) = ((px.floor().max(0.0) as usize).min(img.w as usize - 1), (py.floor().max(0.0) as usize).min(img.h as usize - 1));
                        let p = (iy * img.w as usize + ix) * 4;
                        let al = img.rgba[p + 3] as f32;
                        r += img.rgba[p] as f32 * al;
                        g += img.rgba[p + 1] as f32 * al;
                        b += img.rgba[p + 2] as f32 * al;
                        a += al;
                        cnt += 1.0;
                    }
                }
                if a <= 0.0 {
                    continue;
                }
                let rgb = if img.stencil { gs.fill } else { [r / a / 255.0, g / a / 255.0, b / a / 255.0] };
                self.canvas.blend(x, y, rgb, a / cnt / 255.0 * mk as f32 / 255.0 * gs.fill_alpha);
            }
        }
    }
}
