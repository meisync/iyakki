//! The display-list path of the client engine (F-SDK-22): the second renderer of the UI SDK's display list, after the browser's
//! JavaScript Canvas2D one (`ui/displaylist.js`). The version 1 reader is the SDK's own (`iyakki_ui_sdk::list::DisplayList::parse`, linked
//! without the pixel toolkit), so a newer, unknown or damaged list is refused by exactly the same rules. The rectangles, rounded
//! rectangles, outlines, lines, gradients, shadows and clips are rasterised here into an RGBA framebuffer; the hit regions become the
//! accessibility tree; the input becomes the SDK's input lines. The module has no fonts (the in-house fonts are more than a megabyte and
//! the module is limited to 640 KB, and it has no imports), so TEXT comes from the page (F-SDK-28): the page draws each glyph it needs once
//! with its own fonts into an alpha bitmap and hands it in (`put_glyph`); a text run whose glyphs are all in that atlas is composed here,
//! pixel for pixel in the same framebuffer. A run with a glyph still missing is handed to the page as before (place, size, colour, style),
//! and [`UiSurface::needs`] names what is missing so the page can supply it and redraw.

use iyakki_ui_sdk::canvas::{ImageFormat, Role, TextStyle};
use iyakki_ui_sdk::geom::{Color, Rect};
use iyakki_ui_sdk::list::{esc, DisplayList, Op};
use iyakki_ui_sdk::path;

/// The largest side of a picture, in pixels.
const MAX_SIDE: f32 = 16384.0;

/// One text run for the page to draw (pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Color,
    pub style: TextStyle,
    pub text: String,
    /// The clip (pixels) that was open when it was drawn, `None` for no clip.
    pub clip: Option<[f32; 4]>,
}

/// A style as a small number for the atlas key.
fn style_code(s: TextStyle) -> u8 {
    match s {
        TextStyle::Regular => 0,
        TextStyle::Bold => 1,
        TextStyle::Mono => 2,
    }
}

/// One glyph the page drew: the pen advance, and an alpha bitmap `w` x `h` whose top left sits at `(ox, oy)` from the pen and the run's top.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub adv: f32,
    pub w: u16,
    pub h: u16,
    pub ox: i16,
    pub oy: i16,
    pub alpha: Vec<u8>,
}

/// The most glyphs and bytes of bitmap the atlas keeps (a page that sends more is refused, so the module's memory is bounded).
pub const ATLAS_GLYPHS: usize = 4096;
pub const ATLAS_BYTES: usize = 4 << 20;

type GlyphKey = (char, u16, u8);

/// The glyphs, sorted by key (a sorted list and a binary search are much smaller in the module than a tree map).
#[derive(Default)]
pub struct Atlas {
    map: Vec<(GlyphKey, Glyph)>,
    bytes: usize,
}

impl Atlas {
    fn find(&self, key: &GlyphKey) -> Result<usize, usize> {
        self.map.binary_search_by(|(k, _)| k.cmp(key))
    }
    fn get(&self, c: char, size: u16, style: TextStyle) -> Option<&Glyph> {
        self.find(&(c, size, style_code(style))).ok().map(|i| &self.map[i].1)
    }
}

#[derive(Default)]
pub struct UiSurface {
    atlas: Atlas,
    missing: Vec<GlyphKey>,
    px: Vec<u8>,
    w: usize,
    h: usize,
    scale: f32,
    version: u64,
    list: Option<DisplayList>,
    texts: Vec<TextRun>,
}

impl UiSurface {
    /// Read a display list and draw it at `scale` pixels per logical unit. On an error nothing changes (the old picture stays).
    pub fn load(&mut self, text: &str, scale: f32) -> Result<(), String> {
        let list = DisplayList::parse(text)?;
        let scale = if scale.is_finite() && scale > 0.0 { scale.min(8.0) } else { 1.0 };
        if list.width * scale > MAX_SIDE || list.height * scale > MAX_SIDE {
            return Err("the display list is too large to draw".into());
        }
        let (w, h) = ((list.width * scale).round().max(1.0) as usize, (list.height * scale).round().max(1.0) as usize);
        let mut px = vec![0u8; w * h * 4];
        let mut texts = Vec::new();
        self.missing.clear();
        draw(&list, scale, w, h, &mut px, &mut texts, &self.atlas, &mut self.missing);
        self.px = px;
        self.w = w;
        self.h = h;
        self.scale = scale;
        self.texts = texts;
        self.list = Some(list);
        self.version += 1;
        Ok(())
    }

    /// Draw the list again (after glyphs arrived). `false` when there is no list.
    pub fn redraw(&mut self) -> bool {
        let Some(list) = self.list.take() else { return false };
        let (w, h) = (self.w, self.h);
        let mut px = vec![0u8; w * h * 4];
        let mut texts = Vec::new();
        self.missing.clear();
        draw(&list, self.scale, w, h, &mut px, &mut texts, &self.atlas, &mut self.missing);
        self.px = px;
        self.texts = texts;
        self.list = Some(list);
        self.version += 1;
        true
    }

    /// The glyphs the last drawing lacked: `(character, size in pixels, style)`.
    pub fn needs(&self) -> Vec<(char, u16, &'static str)> {
        self.missing.iter().map(|(c, s, st)| (*c, *s, match st { 1 => "bold", 2 => "mono", _ => "regular" })).collect()
    }

    /// Take a glyph from the page. Refused when the bitmap does not match its size, or the atlas is full.
    #[allow(clippy::too_many_arguments)]
    pub fn put_glyph(&mut self, c: char, size: u16, style: TextStyle, adv: f32, w: u16, h: u16, ox: i16, oy: i16, alpha: Vec<u8>) -> Result<(), String> {
        if alpha.len() != w as usize * h as usize || !adv.is_finite() || !(0.0..=1024.0).contains(&adv) || w > 512 || h > 512 || size == 0 || size > 1024 {
            return Err("that glyph is not valid".into());
        }
        let key = (c, size, style_code(style));
        let at = self.atlas.find(&key);
        let old = at.map_or(0, |i| self.atlas.map[i].1.alpha.len());
        if (self.atlas.map.len() >= ATLAS_GLYPHS && at.is_err()) || self.atlas.bytes - old + alpha.len() > ATLAS_BYTES {
            return Err("the glyph atlas is full".into());
        }
        self.atlas.bytes = self.atlas.bytes - old + alpha.len();
        let g = Glyph { adv, w, h, ox, oy, alpha };
        match at {
            Ok(i) => self.atlas.map[i].1 = g,
            Err(i) => self.atlas.map.insert(i, (key, g)),
        }
        Ok(())
    }

    /// How many glyphs the atlas holds.
    /// The letter advances the page has sent, as a text measure (F-SDK-35): what a client lays text out with. The advances are in the module's
    /// pixels at the glyph's size, so they are divided by it and scale to any size.
    pub fn measure_table(&self) -> iyakki_ui_sdk::list::AdvanceTable {
        let mut t = iyakki_ui_sdk::list::AdvanceTable::new();
        for ((c, size, style), g) in &self.atlas.map {
            let style = match style {
                1 => TextStyle::Bold,
                2 => TextStyle::Mono,
                _ => TextStyle::Regular,
            };
            t.put(style, *c, *size as f32, g.adv);
        }
        t
    }

    pub fn glyphs(&self) -> usize {
        self.atlas.map.len()
    }

    /// `(pixels, width, height, version)`; the pixels are straight (not premultiplied) RGBA.
    pub fn framebuffer(&self) -> (&[u8], usize, usize, u64) {
        (&self.px, self.w, self.h, self.version)
    }

    pub fn list(&self) -> Option<&DisplayList> {
        self.list.as_ref()
    }

    pub fn texts(&self) -> &[TextRun] {
        &self.texts
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The hit region at a place given in logical units: the same rule as `DisplayList::hit_test` (the last one drawn, inside its clips).
    pub fn action_at(&self, x: f32, y: f32) -> Option<u32> {
        self.list.as_ref()?.hit_test(x, y).map(|h| h.action)
    }
}

/// The SDK's input line for a pointer event (`ptr ID PHASE X Y T`); `None` for a phase it does not know.
pub fn pointer_line(id: u32, phase: &str, x: f32, y: f32, t_ms: u64) -> Option<String> {
    matches!(phase, "down" | "move" | "up" | "cancel").then(|| format!("ptr {id} {phase} {x} {y} {t_ms}"))
}

/// The line for a named key (`enter`, `backspace`, ...) or, with `ch`, one typed character; `None` for a name it does not know.
pub fn key_line(name: &str, ch: Option<&str>) -> Option<String> {
    if let Some(c) = ch {
        return (c.chars().count() == 1).then(|| format!("key char {}", esc(c)));
    }
    ["enter", "backspace", "delete", "esc", "tab", "up", "down", "left", "right"].contains(&name).then(|| format!("key {name}"))
}

pub fn text_line(s: &str) -> String {
    format!("text {}", esc(s))
}

pub fn role_name(r: Role) -> &'static str {
    r.key()
}

pub fn style_name(s: TextStyle) -> &'static str {
    match s {
        TextStyle::Regular => "regular",
        TextStyle::Bold => "bold",
        TextStyle::Mono => "mono",
    }
}

type Px = (f32, f32, f32, f32); // x0, y0, x1, y1 in pixels

struct Surf<'a> {
    px: &'a mut [u8],
    w: usize,
    h: usize,
    /// The opacity of the group being drawn (version 3 layers multiply).
    alpha: f32,
}

impl Surf<'_> {
    /// Straight RGBA `src` (`sw` x `sh`) stretched into `b` (pixels), nearest neighbour, source-over, inside `clip`.
    fn picture(&mut self, b: Px, clip: Px, sw: usize, sh: usize, src: &[u8]) {
        let (bw, bh) = (b.2 - b.0, b.3 - b.1);
        if bw < 1.0 || bh < 1.0 || sw == 0 || sh == 0 || src.len() != sw * sh * 4 {
            return;
        }
        let (x0, y0) = (b.0.max(clip.0).max(0.0) as usize, b.1.max(clip.1).max(0.0) as usize);
        let (x1, y1) = ((b.2.min(clip.2).max(0.0) as usize).min(self.w), (b.3.min(clip.3).max(0.0) as usize).min(self.h));
        for y in y0..y1 {
            let sy = (((y as f32 + 0.5 - b.1) / bh * sh as f32) as usize).min(sh - 1);
            for x in x0..x1 {
                let sx = (((x as f32 + 0.5 - b.0) / bw * sw as f32) as usize).min(sw - 1);
                let i = (sy * sw + sx) * 4;
                if src[i + 3] == 255 && self.alpha >= 1.0 {
                    let o = (y * self.w + x) * 4;
                    self.px[o..o + 4].copy_from_slice(&src[i..i + 4]);
                } else {
                    self.blend(x, y, [src[i], src[i + 1], src[i + 2], src[i + 3]], 1.0);
                }
            }
        }
    }

    /// Source-over of colour `c` at coverage `cov` (0..1) on straight RGBA.
    fn blend(&mut self, x: usize, y: usize, c: Color, cov: f32) {
        let sa = c[3] as f32 / 255.0 * cov * self.alpha;
        if sa <= 0.0 {
            return;
        }
        let i = (y * self.w + x) * 4;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        for k in 0..3 {
            let v = (c[k] as f32 * sa + self.px[i + k] as f32 * da * (1.0 - sa)) / oa;
            self.px[i + k] = v.round().clamp(0.0, 255.0) as u8;
        }
        self.px[i + 3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    /// A glyph's alpha bitmap with its top left at `(gx, gy)`, inside `clip`.
    #[inline(never)]
    fn glyph(&mut self, g: &Glyph, gx: i32, gy: i32, clip: Px, c: Color) {
        let (x0, y0) = ((clip.0.max(0.0) as i32).max(gx), (clip.1.max(0.0) as i32).max(gy));
        let (x1, y1) = ((clip.2 as i32).min(self.w as i32).min(gx + g.w as i32), (clip.3 as i32).min(self.h as i32).min(gy + g.h as i32));
        for y in y0..y1 {
            for x in x0..x1 {
                let a = g.alpha[(y - gy) as usize * g.w as usize + (x - gx) as usize];
                if a > 0 {
                    self.blend(x as usize, y as usize, c, a as f32 / 255.0);
                }
            }
        }
    }

    /// Fill the pixels of `bounds` (inside `clip`) with `c` at the coverage `cov(pixel x, pixel y)` returns.
    fn fill(&mut self, bounds: Px, clip: Px, c: Color, cov: &dyn Fn(f32, f32) -> f32) {
        let x0 = bounds.0.max(clip.0).floor().max(0.0) as usize;
        let y0 = bounds.1.max(clip.1).floor().max(0.0) as usize;
        let x1 = (bounds.2.min(clip.2).ceil().max(0.0) as usize).min(self.w);
        let y1 = (bounds.3.min(clip.3).ceil().max(0.0) as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let k = cov(x as f32, y as f32);
                if k > 0.0 {
                    self.blend(x, y, c, k);
                }
            }
        }
    }
}

fn overlap(p: f32, a: f32, b: f32) -> f32 {
    ((p + 1.0).min(b) - p.max(a)).clamp(0.0, 1.0)
}

/// Signed distance (pixels, negative inside) of a point to a rounded rectangle.
fn sdf(px: f32, py: f32, r: Px, radius: f32) -> f32 {
    let (cx, cy, hx, hy) = ((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0, (r.2 - r.0) / 2.0, (r.3 - r.1) / 2.0);
    let rad = radius.clamp(0.0, hx.min(hy));
    let (qx, qy) = ((px - cx).abs() - (hx - rad), (py - cy).abs() - (hy - rad));
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad
}

fn rrect_cov(px: f32, py: f32, r: Px, radius: f32) -> f32 {
    if r.2 <= r.0 || r.3 <= r.1 {
        return 0.0;
    }
    (0.5 - sdf(px + 0.5, py + 0.5, r, radius)).clamp(0.0, 1.0)
}

fn rect_px(r: Rect, k: f32) -> Px {
    (r.x * k, r.y * k, (r.x + r.w) * k, (r.y + r.h) * k)
}

fn lerp(a: Color, b: Color, t: f32) -> Color {
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round().clamp(0.0, 255.0) as u8;
    [m(0), m(1), m(2), m(3)]
}

#[allow(clippy::too_many_arguments)]
fn draw(list: &DisplayList, k: f32, w: usize, h: usize, px: &mut [u8], texts: &mut Vec<TextRun>, atlas: &Atlas, missing: &mut Vec<GlyphKey>) {
    let mut s = Surf { px, w, h, alpha: 1.0 };
    let mut alphas: Vec<f32> = Vec::new();
    let full: Px = (0.0, 0.0, w as f32, h as f32);
    let mut clips: Vec<Px> = vec![full];
    for op in &list.ops {
        let clip = *clips.last().unwrap_or(&full);
        match op {
            Op::Rect(r, c) => {
                // snapped to whole pixels like the device's pixel canvas
                let (x, y) = ((r.x * k).round(), (r.y * k).round());
                let b: Px = (x, y, x + (r.w * k).round().max(1.0), y + (r.h * k).round().max(1.0));
                s.fill(b, clip, *c, &|x, y| overlap(x, b.0, b.2) * overlap(y, b.1, b.3));
            }
            Op::RoundRect(r, rad, c) => {
                let b = rect_px(*r, k);
                let rad = rad * k;
                s.fill((b.0 - 1.0, b.1 - 1.0, b.2 + 1.0, b.3 + 1.0), clip, *c, &|x, y| rrect_cov(x, y, b, rad));
            }
            Op::Outline(r, rad, wd, c) => {
                let b = rect_px(*r, k);
                let (rad, wd) = (rad * k, wd * k);
                let inner = (b.0 + wd, b.1 + wd, b.2 - wd, b.3 - wd);
                s.fill((b.0 - 1.0, b.1 - 1.0, b.2 + 1.0, b.3 + 1.0), clip, *c, &|x, y| (rrect_cov(x, y, b, rad) - rrect_cov(x, y, inner, (rad - wd).max(0.0))).max(0.0));
            }
            Op::Line(p, wd, c) => {
                let (x0, y0, x1, y1, half) = (p[0] * k, p[1] * k, p[2] * k, p[3] * k, (wd * k / 2.0).max(0.5));
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len2 = dx * dx + dy * dy;
                let b: Px = (x0.min(x1) - half - 1.0, y0.min(y1) - half - 1.0, x0.max(x1) + half + 1.0, y0.max(y1) + half + 1.0);
                s.fill(b, clip, *c, &|x, y| {
                    let (qx, qy) = (x + 0.5 - x0, y + 0.5 - y0);
                    let t = if len2 > 0.0 { ((qx * dx + qy * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let d = (qx - t * dx).hypot(qy - t * dy);
                    (half + 0.5 - d).clamp(0.0, 1.0)
                });
            }
            Op::Gradient(r, top, bottom) => {
                let b = rect_px(*r, k);
                let hh = (b.3 - b.1).max(1.0);
                let x0 = b.0.max(clip.0).floor().max(0.0) as usize;
                let y0 = b.1.max(clip.1).floor().max(0.0) as usize;
                let x1 = (b.2.min(clip.2).ceil().max(0.0) as usize).min(w);
                let y1 = (b.3.min(clip.3).ceil().max(0.0) as usize).min(h);
                for y in y0..y1 {
                    let col = lerp(*top, *bottom, ((y as f32 + 0.5 - b.1) / hh).clamp(0.0, 1.0));
                    let cy = overlap(y as f32, b.1, b.3);
                    for x in x0..x1 {
                        let cov = overlap(x as f32, b.0, b.2) * cy;
                        if cov > 0.0 {
                            s.blend(x, y, col, cov);
                        }
                    }
                }
            }
            // version 2 blur: the engine has no blur pass; the surface it lies on is drawn without frost
            Op::Blur(..) => {}
            Op::PushLayer(a) => {
                alphas.push(s.alpha);
                s.alpha *= *a;
            }
            Op::PopLayer => {
                if let Some(a) = alphas.pop() {
                    s.alpha = a;
                }
            }
            Op::Path { fill, stroke, segs } => {
                let polys: Vec<iyakki_ui_sdk::path::Poly> = path::flatten(segs, 0.25 / k.max(0.25))
                    .into_iter()
                    .map(|mut p| {
                        p.pts.iter_mut().for_each(|q| *q = (q.0 * k, q.1 * k));
                        p
                    })
                    .collect();
                let grow = stroke.map_or(0.0, |(_, wd)| wd * k / 2.0 + 1.0);
                if let Some((bx0, by0, bx1, by1)) = path::bounds(&polys, grow) {
                    let (x0, y0) = ((bx0.floor().max(clip.0.floor()).max(0.0)) as i32, (by0.floor().max(clip.1.floor()).max(0.0)) as i32);
                    let (x1, y1) = ((bx1.ceil().min(clip.2).min(w as f32)) as i32, (by1.ceil().min(clip.3).min(h as f32)) as i32);
                    let (mw, mh) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
                    if mw > 0 && mh > 0 {
                        let paint = |s: &mut Surf, mask: &[f32], c: Color| {
                            for row in 0..mh {
                                for col in 0..mw {
                                    let m = mask[row * mw + col];
                                    if m > 0.0 {
                                        s.blend(x0 as usize + col, y0 as usize + row, c, m);
                                    }
                                }
                            }
                        };
                        if let Some(c) = fill {
                            paint(&mut s, &path::fill_mask(&polys, x0, y0, mw, mh), *c);
                        }
                        if let Some((c, wd)) = stroke {
                            paint(&mut s, &path::stroke_mask(&polys, wd * k, x0, y0, mw, mh), *c);
                        }
                    }
                }
            }
            Op::Shadow(r, rad, spread, c) => {
                // the device's glow: three growing rounded rectangles, each fainter outwards
                let b = rect_px(*r, k);
                for (m, f) in [(1.0f32, 0.35f32), (0.66, 0.6), (0.33, 1.0)] {
                    let d = spread * k * m;
                    let g: Px = (b.0 - d, b.1 - d, b.2 + d, b.3 + d);
                    let col = [c[0], c[1], c[2], (c[3] as f32 * f * 0.5) as u8];
                    let rr = rad * k + d;
                    s.fill((g.0 - 1.0, g.1 - 1.0, g.2 + 1.0, g.3 + 1.0), clip, col, &|x, y| rrect_cov(x, y, g, rr));
                }
            }
            Op::Text { x, y, size, color, style, text } | Op::TextEx { x, y, size, color, style, text, .. } | Op::TextC { x, y, size, color, style, text, .. } => {
                // letter spacing of a `textx` run (version 2): extra pen distance after each letter; the family is the page's to choose
                let tracking = if let Op::TextEx { opts, .. } = op { opts.tracking * size * k } else { 0.0 };
                let open = clips.len() > 1;
                // composed here when the atlas has every glyph of the run at this size; else the page draws it and is told what is missing
                let px_size = (size * k).round().clamp(1.0, 1024.0) as u16;
                let mut all = text.chars().any(|c| !c.is_control());
                for c in text.chars().filter(|c| !c.is_control()) {
                    all &= atlas.get(c, px_size, *style).is_some();
                }
                if all {
                    let mut pen = x * k;
                    if let Op::TextC { advances, .. } = op {
                        // version 3: every cluster starts where the shaper put it; its marks sit on the base letter's pen
                        for (r, adv) in path::clusters(text).into_iter().zip(advances) {
                            for c in text[r].chars().filter(|c| !c.is_control()) {
                                if let Some(g) = atlas.get(c, px_size, *style) {
                                    s.glyph(g, (pen + g.ox as f32).round() as i32, (y * k + g.oy as f32).round() as i32, clip, *color);
                                }
                            }
                            pen += adv * k;
                        }
                        continue;
                    }
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if let Some(g) = atlas.get(c, px_size, *style) {
                            s.glyph(g, (pen + g.ox as f32).round() as i32, (y * k + g.oy as f32).round() as i32, clip, *color);
                            pen += g.adv + tracking;
                        }
                    }
                    continue;
                }
                for c in text.chars().filter(|c| !c.is_control()) {
                    if atlas.get(c, px_size, *style).is_none() {
                        let key = (c, px_size, style_code(*style));
                        if let Err(i) = missing.binary_search(&key) {
                            missing.insert(i, key);
                        }
                    }
                }
                texts.push(TextRun { x: x * k, y: y * k, size: size * k, color: *color, style: *style, text: text.clone(), clip: open.then_some([clip.0, clip.1, clip.2, clip.3]) });
            }
            Op::PushClip(r) => {
                let b = rect_px(*r, k);
                let b: Px = (b.0.round(), b.1.round(), b.2.round(), b.3.round());
                clips.push((b.0.max(clip.0), b.1.max(clip.1), b.2.min(clip.2), b.3.min(clip.3)));
            }
            Op::PopClip => {
                if clips.len() > 1 {
                    clips.pop();
                }
            }
            Op::Hit(..) => {}
            Op::Image(r, img) => {
                let b = rect_px(*r, k);
                match img.format {
                    // pixels are stretched with nearest-neighbour sampling, whole pixels, inside the clip
                    ImageFormat::Rgba => s.picture(b, clip, img.width as usize, img.height as usize, &img.data),
                    // the engine module has no JPEG decoder (it stays small): the page that hosts it decodes JPEGs itself; drawn alone, a block
                    ImageFormat::Jpeg => s.fill(b, clip, [20, 20, 24, 255], &|x, y| overlap(x, b.0, b.2) * overlap(y, b.1, b.3)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;
    use iyakki_app_sample::SampleApp;
    use iyakki_ui_sdk::canvas::Canvas;
    use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
    use iyakki_ui_sdk::host::Host as SdkHost;
    use iyakki_ui_sdk::list::ListCanvas;
    use iyakki_ui_sdk::look::Look;
    use iyakki_ui_sdk::raster::RasterCanvas;
    use iyakki_ui_sdk::wire::{parse_line, Input};
    use iyakki_uikit::gfx::Gfx;
    use iyakki_uikit::ui::R;

    fn sample_list(w: u32, h: u32) -> String {
        let mut host = SdkHost::new(SampleApp::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&["sample"])), Box::new(MemWallpaper::default()));
        let mut lc = ListCanvas::new(w as f32, h as f32);
        host.draw(&mut lc);
        lc.finish().to_text()
    }

    /// The device's own pixel canvas drawing the same list, and the engine drawing it: they agree except where text is (the engine draws
    /// none) and at antialiased edges.
    fn compare(w: u32, h: u32, scale: f32) {
        let text = sample_list(w, h);
        let mut s = UiSurface::default();
        s.load(&text, scale).unwrap();
        let (px, fw, fh, _) = s.framebuffer();
        let list = DisplayList::parse(&text).unwrap();
        let (nw, nh) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
        assert_eq!((fw as u32, fh as u32), (nw, nh));
        let mut g = Gfx::new(nw, nh);
        let mut shapes = list.clone();
        shapes.ops.retain(|o| !matches!(o, Op::Text { .. }));
        shapes.replay(&mut RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: nw as f32, h: nh as f32 }, scale));
        let native = g.rgba();
        let (mut total, mut off, mut worst) = (0usize, 0usize, 0i32);
        for i in 0..(fw * fh) {
            let (a, b) = (&px[i * 4..i * 4 + 4], &native[i * 4..i * 4 + 4]);
            // the page behind is opaque on the device; compare over the engine's own colour where it drew
            total += 1;
            let d = (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).max().unwrap_or(0);
            worst = worst.max(d);
            if d > 24 {
                off += 1;
            }
        }
        assert!(total > 0 && off * 200 <= total, "{off} of {total} pixels differ by more than 24 (worst {worst}) at scale {scale}");
        // something real was drawn
        assert!(px.chunks(4).filter(|p| p[3] == 255).count() * 2 > total, "less than half the picture is opaque");
        assert!(!s.texts().is_empty());
    }

    #[test]
    fn the_engine_draws_the_sample_apps_list_like_the_devices_pixel_canvas_at_three_sizes_and_scales() {
        compare(360, 740, 1.0);
        compare(800, 480, 1.0);
        compare(360, 740, 2.0);
    }

    #[test]
    fn exact_shapes_are_exact_clips_cut_and_alpha_blends() {
        let text = "iyakki-display-list 1 40 20\nrect 0 0 40 20 102030ff\nrect 4 4 10 10 ff0000ff\nclip 20 0 10 20\nrect 0 0 40 20 00ff00ff\nendclip\nrect 32 4 4 4 ffffff80\n";
        let mut s = UiSurface::default();
        s.load(text, 1.0).unwrap();
        let (px, w, _, _) = s.framebuffer();
        let at = |x: usize, y: usize| px[(y * w + x) * 4..(y * w + x) * 4 + 4].to_vec();
        assert_eq!(at(1, 1), [0x10, 0x20, 0x30, 255]);
        assert_eq!(at(5, 5), [255, 0, 0, 255]);
        assert_eq!(at(25, 10), [0, 255, 0, 255], "inside the clip");
        assert_eq!(at(19, 10), [0x10, 0x20, 0x30, 255], "left of the clip");
        assert_eq!(at(30, 10), [0x10, 0x20, 0x30, 255], "right of the clip");
        let m = at(33, 5);
        assert!(m[0] > 0x80 && m[0] < 0xc0 && m[3] == 255, "half white over dark is mid grey: {m:?}");
        // a gradient runs top to bottom
        let g = "iyakki-display-list 1 10 40\ngrad 0 0 10 40 000000ff ffffffff\n";
        s.load(g, 1.0).unwrap();
        let (px, w, _, _) = s.framebuffer();
        assert!(px[(2 * w) * 4] < 20 && px[(37 * w) * 4] > 235 && px[(20 * w) * 4] > 100 && px[(20 * w) * 4] < 160);
    }

    #[test]
    fn a_newer_unknown_or_damaged_list_is_refused_by_the_sdks_own_rules_and_the_old_picture_stays() {
        let mut s = UiSurface::default();
        s.load("iyakki-display-list 1 10 10\nrect 0 0 10 10 ff0000ff\n", 1.0).unwrap();
        let v = s.framebuffer().3;
        for (bad, why) in [
            ("iyakki-display-list 4 10 10\n", "newer"),
            ("iyakki-display-list 1 10 10\nwobble 1 2\n", "unknown or malformed operation 'wobble'"),
            ("iyakki-display-list 1 10 10\nclip 0 0 5 5\n", "never closed"),
            ("iyakki-display-list 1 10 10\nendclip\n", "endclip without clip"),
            ("not a list", "not a display list"),
            ("iyakki-display-list 1 10 10\nrect 0 0 10 x ff0000ff\n", "line 2"),
        ] {
            let e = s.load(bad, 1.0).unwrap_err();
            assert!(e.contains(why), "{e} should mention {why}");
        }
        assert_eq!(s.framebuffer().3, v);
        assert_eq!(&s.framebuffer().0[..4], &[255, 0, 0, 255]);
        assert!(s.load("iyakki-display-list 1 100000 10\n", 1.0).is_err(), "a huge pane is refused");
    }

    #[test]
    fn the_host_commands_give_the_accessibility_tree_the_hit_action_and_input_lines_the_sdk_reads() {
        let text = sample_list(360, 740);
        let mut h = Host::new();
        let load = serde_json::json!({ "c": "ui_load", "text": text, "scale": 2.0 }).to_string();
        let n = h.command(&load);
        assert!(n > 0);
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        let list = DisplayList::parse(&text).unwrap();
        let want = list.a11y();
        let got = r["a11y"].as_array().unwrap();
        assert_eq!(got.len(), want.len());
        assert!(!want.is_empty());
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(g["name"], w.name.as_str());
            assert_eq!(g["role"], w.role.key());
            assert_eq!(g["x"].as_f64().unwrap() as f32, w.rect.x * 2.0);
        }
        assert_eq!(r["w"], 720);
        assert_eq!(r["h"], 1480);
        assert_eq!(r["texts"].as_array().unwrap().len(), list.texts().len());
        // a tap in the middle of the first region (pixels) finds the action the Rust hit test finds
        let first = &want[0];
        let (cx, cy) = (first.rect.x + first.rect.w / 2.0, first.rect.y + first.rect.h / 2.0);
        let hit = list.hit_test(cx, cy).unwrap().action;
        h.command(&serde_json::json!({ "c": "ui_hit", "x": cx * 2.0, "y": cy * 2.0 }).to_string());
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert_eq!(r["action"], hit);
        assert_eq!(parse_line(r["line"].as_str().unwrap(), 1), Ok(Input::Event(iyakki_ui_sdk::event::UiEvent::Action(hit))));
        h.command(&serde_json::json!({ "c": "ui_hit", "x": -5.0, "y": -5.0 }).to_string());
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert!(r["action"].is_null());
        // every input line is one the SDK's session reader accepts
        for cmd in [
            serde_json::json!({ "c": "ui_line", "kind": "pointer", "phase": "down", "x": 100.0, "y": 60.0, "t": 5 }),
            serde_json::json!({ "c": "ui_line", "kind": "key", "name": "enter" }),
            serde_json::json!({ "c": "ui_line", "kind": "key", "name": "x", "char": "x" }),
            serde_json::json!({ "c": "ui_line", "kind": "text", "s": "two words\nand a line" }),
            serde_json::json!({ "c": "ui_line", "kind": "action", "n": 3 }),
            serde_json::json!({ "c": "ui_line", "kind": "wheel", "n": -2 }),
            serde_json::json!({ "c": "ui_line", "kind": "resize", "w": 300, "h": 500 }),
            serde_json::json!({ "c": "ui_line", "kind": "tick", "t": 99 }),
        ] {
            assert!(h.command(&cmd.to_string()) > 0, "{cmd}");
            let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
            let line = r["line"].as_str().unwrap();
            assert!(parse_line(line, 1).is_ok(), "the SDK refused {line:?}");
        }
        // the pointer line carries logical units: 100 pixels at scale 2 is 50
        h.command(&serde_json::json!({ "c": "ui_line", "kind": "pointer", "phase": "up", "x": 100.0, "y": 60.0, "t": 9 }).to_string());
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert_eq!(r["line"], "ptr 0 up 50 30 9");
        for bad in [serde_json::json!({ "c": "ui_line", "kind": "key", "name": "f13" }), serde_json::json!({ "c": "ui_line", "kind": "pointer", "phase": "hover", "x": 1, "y": 1 }), serde_json::json!({ "c": "ui_line", "kind": "nothing" })] {
            assert_eq!(h.command(&bad.to_string()), -1, "{bad}");
        }
        // a refused list answers with the reason and the old picture stays
        assert!(h.command(&serde_json::json!({ "c": "ui_load", "text": "iyakki-display-list 9 1 1\n" }).to_string()) < -1);
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert!(r["error"].as_str().unwrap().contains("newer"));
        assert_eq!(h.ui_framebuffer().1, 720);
    }

    #[test]
    fn the_engines_list_crate_is_linked_without_the_pixel_toolkit() {
        // the whole point of the feature split: the sdk crate's pixel side is not part of what the wasm module links
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains("iyakki-ui-sdk = { path = \"../iyakki-ui-sdk\", default-features = false }"));
        let _ = ListCanvas::new(1.0, 1.0).size();
    }

    #[test]
    fn the_engine_draws_a_picture_stretched_inside_its_rectangle_and_a_block_for_a_jpeg() {
        use iyakki_ui_sdk::canvas::{Image, ImageFormat};
        let mut lc = ListCanvas::new(40.0, 20.0);
        // two columns: left red, right blue
        let px: Vec<u8> = [[255u8, 0, 0, 255], [0, 0, 255, 255]].iter().cycle().take(4).flatten().copied().collect();
        lc.image(Rect::new(0.0, 0.0, 20.0, 20.0), &Image { format: ImageFormat::Rgba, width: 2, height: 2, seq: 1, data: std::sync::Arc::new(px) });
        lc.image(Rect::new(20.0, 0.0, 20.0, 20.0), &Image { format: ImageFormat::Jpeg, width: 4, height: 4, seq: 2, data: std::sync::Arc::new(vec![0xFF, 0xD8, 0xFF, 0xD9]) });
        let mut s = UiSurface::default();
        s.load(&lc.finish().to_text(), 2.0).unwrap();
        let (px, w, _, _) = s.framebuffer();
        let at = |x: usize, y: usize| [px[(y * w + x) * 4], px[(y * w + x) * 4 + 1], px[(y * w + x) * 4 + 2], px[(y * w + x) * 4 + 3]];
        assert_eq!(at(5, 20), [255, 0, 0, 255], "left half of the picture is red");
        assert_eq!(at(30, 20), [0, 0, 255, 255], "right half is blue");
        assert_eq!(at(60, 20)[3], 255, "the JPEG is a block, not transparent");
        assert_eq!(at(75, 5)[..3], [20, 20, 24]);
    }

    #[test]
    fn the_engine_fills_and_strokes_a_path_with_a_hole_and_fades_a_layer() {
        let ring = "M1:1;L19:1;L19:19;L1:19;Z;M6:6;L6:14;L14:14;L14:6;Z";
        let text = format!("iyakki-display-list 3 20 20\nrect 0 0 20 20 000000ff\nlayer 0.5\npath ffffffff - 0 {ring}\nendlayer\npath - 00ff00ff 2 M0:19;L20:19\n");
        let mut s = UiSurface::default();
        s.load(&text, 1.0).unwrap();
        let (px, w, _, _) = s.framebuffer();
        let at = |x: usize, y: usize| px[(y * w + x) * 4];
        assert!((at(3, 3) as i32 - 128).abs() <= 2, "half white over black: {}", at(3, 3));
        assert_eq!(at(10, 10), 0, "the hole stays empty");
        assert_eq!(px[(19 * w + 10) * 4 + 1], 255, "the stroke is solid green");
        assert_eq!(at(0, 0), 0);
        // the same list at twice the size draws the same shape twice as large
        s.load(&text, 2.0).unwrap();
        let (px, w, _, _) = s.framebuffer();
        assert!((px[(6 * w + 6) * 4] as i32 - 128).abs() <= 2);
        assert_eq!(px[(20 * w + 20) * 4], 0);
    }

    #[test]
    fn the_letters_the_page_sent_are_a_text_measure_that_scales_with_the_size() {
        let mut s = UiSurface::default();
        s.load("iyakki-display-list 1 20 10\nrect 0 0 20 10 000000ff\n", 1.0).unwrap();
        s.put_glyph('i', 10, TextStyle::Regular, 4.0, 2, 6, 0, 0, vec![255; 12]).unwrap();
        s.put_glyph('W', 20, TextStyle::Bold, 30.0, 2, 6, 0, 0, vec![255; 12]).unwrap();
        let t = s.measure_table();
        assert_eq!(t.len(), 2);
        assert_eq!(t.width(20.0, "ii", TextStyle::Regular), 16.0);
        assert_eq!(t.width(10.0, "W", TextStyle::Bold), 15.0);
        // a canvas laid out with it measures the same, and a letter it lacks is the fixed estimate
        let mut lc = iyakki_ui_sdk::list::ListCanvas::with_measure(100.0, 20.0, t.clone().into_measure());
        assert_eq!(lc.measure(10.0, "i", TextStyle::Regular), 4.0);
        assert_eq!(lc.measure(10.0, "?", TextStyle::Regular), iyakki_ui_sdk::list::approx_measure(10.0, "x", TextStyle::Regular));
    }

    #[test]
    fn a_cluster_run_puts_each_cluster_where_the_shaper_did_and_marks_share_the_pen() {
        let mut s = UiSurface::default();
        // advances 10 and 20: the second cluster starts at x 12 even though the glyph advance is 5
        s.load("iyakki-display-list 3 60 20\nrect 0 0 60 20 000000ff\ntextc 2 2 10 ffffffff regular default 0 0 10,20 e%CC%81x\n", 1.0).unwrap();
        assert_eq!(s.texts().len(), 1, "no glyphs yet: the page draws it");
        for c in ['e', '\u{0301}', 'x'] {
            s.put_glyph(c, 10, TextStyle::Regular, 5.0, 4, 6, 0, 0, vec![255; 24]).unwrap();
        }
        assert!(s.redraw());
        let (px, w, _, _) = s.framebuffer();
        let at = |x: usize, y: usize| px[(y * w + x) * 4];
        assert_eq!(at(3, 3), 255, "the base letter at the pen");
        assert_eq!(at(9, 3), 0, "the mark did not move the pen: nothing at 7..11");
        assert_eq!(at(13, 3), 255, "x starts at 2 + 10 = 12");
        assert_eq!(at(17, 3), 0);
    }
}

#[cfg(test)]
mod glyph_tests {
    use super::*;

    const LIST: &str = "iyakki-display-list 1 60 20\nrect 0 0 60 20 000000ff\ntext 2 2 10 ffffffff regular Hi\nclip 0 0 4 20\ntext 2 12 10 ff0000ff regular Hi\nendclip\n";

    fn solid(s: &mut UiSurface, c: char, size: u16) {
        // a solid 4x6 block, advance 5, placed at the pen
        s.put_glyph(c, size, TextStyle::Regular, 5.0, 4, 6, 0, 0, vec![255; 24]).unwrap();
    }

    fn at(s: &UiSurface, x: usize, y: usize) -> [u8; 4] {
        let (px, w, _, _) = s.framebuffer();
        let i = (y * w + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn a_run_waits_for_the_page_until_every_glyph_is_in_the_atlas_and_then_is_composed_in_the_module() {
        let mut s = UiSurface::default();
        s.load(LIST, 1.0).unwrap();
        assert_eq!(s.texts().len(), 2, "no glyphs yet: both runs go to the page");
        assert_eq!(s.needs(), vec![('H', 10, "regular"), ('i', 10, "regular")]);
        solid(&mut s, 'H', 10);
        assert!(s.redraw());
        assert_eq!(s.texts().len(), 2, "one glyph is not enough");
        assert_eq!(s.needs(), vec![('i', 10, "regular")]);
        solid(&mut s, 'i', 10);
        assert!(s.redraw());
        assert!(s.texts().is_empty() && s.needs().is_empty(), "the module draws it all now");
        // the first run: white blocks at x 2..6 and 7..11, y 2..8
        assert_eq!(at(&s, 3, 3), [255, 255, 255, 255]);
        assert_eq!(at(&s, 8, 7), [255, 255, 255, 255]);
        assert_eq!(at(&s, 6, 3), [0, 0, 0, 255], "the gap between the glyphs");
        // the second run is cut by its clip (x < 4): only the first glyph's left part is red
        assert_eq!(at(&s, 3, 13), [255, 0, 0, 255]);
        assert_eq!(at(&s, 5, 13), [0, 0, 0, 255], "outside the clip nothing is drawn");
    }

    #[test]
    fn partial_coverage_blends_and_a_size_or_style_the_page_did_not_send_is_asked_for() {
        let mut s = UiSurface::default();
        s.load("iyakki-display-list 1 20 10\nrect 0 0 20 10 0000ffff\ntext 0 0 8 ffffffff bold A\n", 1.0).unwrap();
        assert_eq!(s.needs(), vec![('A', 8, "bold")]);
        s.put_glyph('A', 8, TextStyle::Regular, 6.0, 2, 2, 0, 0, vec![128; 4]).unwrap();
        assert!(s.redraw());
        assert_eq!(s.needs(), vec![('A', 8, "bold")], "a regular glyph does not serve a bold run");
        s.put_glyph('A', 8, TextStyle::Bold, 6.0, 2, 2, 0, 0, vec![128; 4]).unwrap();
        s.redraw();
        let p = at(&s, 0, 0);
        assert!(p[0] > 100 && p[0] < 150 && p[2] == 255, "half white over blue: {p:?}");
        // a scale changes the size the runs ask for
        s.load("iyakki-display-list 1 20 10\ntext 0 0 8 ffffffff bold A\n", 2.0).unwrap();
        assert_eq!(s.needs(), vec![('A', 16, "bold")]);
    }

    #[test]
    fn a_glyph_that_does_not_fit_its_size_or_would_fill_the_atlas_past_its_limit_is_refused() {
        let mut s = UiSurface::default();
        assert!(s.put_glyph('x', 10, TextStyle::Regular, 5.0, 4, 6, 0, 0, vec![1; 23]).is_err(), "bitmap size does not match");
        assert!(s.put_glyph('x', 10, TextStyle::Regular, f32::NAN, 1, 1, 0, 0, vec![1]).is_err());
        assert!(s.put_glyph('x', 0, TextStyle::Regular, 5.0, 1, 1, 0, 0, vec![1]).is_err());
        assert!(s.put_glyph('x', 10, TextStyle::Regular, 5.0, 600, 1, 0, 0, vec![1; 600]).is_err());
        assert_eq!(s.glyphs(), 0);
        for i in 0..ATLAS_GLYPHS {
            s.put_glyph(char::from_u32(0x4e00 + i as u32).unwrap(), 10, TextStyle::Regular, 5.0, 1, 1, 0, 0, vec![1]).unwrap();
        }
        assert!(s.put_glyph('z', 10, TextStyle::Regular, 5.0, 1, 1, 0, 0, vec![1]).is_err(), "the atlas holds at most its limit");
        assert!(s.put_glyph('\u{4e00}', 10, TextStyle::Regular, 6.0, 1, 1, 0, 0, vec![2]).is_ok(), "replacing one is fine");
        let mut t = UiSurface::default();
        assert!(!t.redraw(), "nothing to draw again");
    }

    #[test]
    fn the_commands_carry_glyphs_as_hex_and_a_redraw_answers_like_a_load() {
        let mut h = crate::host::Host::new();
        let load = serde_json::json!({ "c": "ui_load", "text": LIST, "scale": 1.0 }).to_string();
        assert!(h.command(&load) > 0);
        let n = h.command(&serde_json::json!({ "c": "ui_needs" }).to_string());
        assert!(n > 0);
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert_eq!(r["glyphs"].as_array().unwrap().len(), 2);
        for ch in ["H", "i"] {
            let c = serde_json::json!({ "c": "ui_glyph", "ch": ch, "size": 10, "style": "regular", "adv": 5.0, "w": 2, "h": 2, "ox": 0, "oy": 0, "alpha": "ffffffff" }).to_string();
            assert!(h.command(&c) > 0);
        }
        assert!(h.command(&serde_json::json!({ "c": "ui_glyph", "ch": "x", "size": 10, "adv": 5.0, "w": 2, "h": 2, "alpha": "zz" }).to_string()) < 0, "bad hex");
        assert!(h.command(&serde_json::json!({ "c": "ui_glyph", "ch": "x", "size": 10, "adv": 5.0, "w": 2, "h": 2, "alpha": "ff" }).to_string()) < 0, "wrong length");
        let n = h.command(&serde_json::json!({ "c": "ui_redraw" }).to_string());
        assert!(n > 0);
        let r: serde_json::Value = serde_json::from_slice(h.reply()).unwrap();
        assert_eq!(r["texts"].as_array().unwrap().len(), 0, "{r}");
    }
}
