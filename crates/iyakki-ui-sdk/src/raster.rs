//! The pixel canvas: `Canvas` over the toolkit's `Gfx` (the device screen, the desktop window, the emulator's PNG). Logical units are
//! multiplied by `scale` on the way in, so an app lays out in logical units and the screen's size factor is only applied here.
//! A clickable region becomes a `Gfx` hit with `Event::Row(action)`: the adapter turns that event back into `UiEvent::Action`.

use iyakki_uikit::gfx::{Face, Gfx};
use iyakki_uikit::ui::{Event, R};

use crate::canvas::{Canvas, Hit, Image, ImageFormat, TextStyle};
use crate::geom::{Color, Rect};
use crate::list::tracking_extra;
use crate::path::Seg;
use crate::typeface::TextOpts;

pub struct RasterCanvas<'a> {
    g: &'a mut Gfx,
    /// Where the app's (0, 0) is on the screen, in pixels.
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
    scale: f32,
    opts: TextOpts,
}

impl<'a> RasterCanvas<'a> {
    /// Draw into `area` (pixels) of `g` with `scale` pixels per logical unit.
    pub fn new(g: &'a mut Gfx, area: R, scale: f32) -> RasterCanvas<'a> {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        RasterCanvas { g, ox: area.x, oy: area.y, w: area.w / scale, h: area.h / scale, scale, opts: TextOpts::default() }
    }

    fn px(&self, r: Rect) -> (f32, f32, f32, f32) {
        (self.ox + r.x * self.scale, self.oy + r.y * self.scale, r.w * self.scale, r.h * self.scale)
    }

    fn face(s: TextStyle) -> Face {
        match s {
            TextStyle::Regular => Face::Sans,
            TextStyle::Bold => Face::Bold,
            TextStyle::Mono => Face::Mono,
        }
    }
}

impl Canvas for RasterCanvas<'_> {
    fn size(&self) -> (f32, f32) {
        (self.w, self.h)
    }
    fn rect(&mut self, r: Rect, c: Color) {
        let (x, y, w, h) = self.px(r);
        self.g.rect(x, y, w, h, c);
    }
    fn rrect(&mut self, r: Rect, radius: f32, c: Color) {
        let (x, y, w, h) = self.px(r);
        self.g.rrect(x, y, w, h, radius * self.scale, c);
    }
    fn outline(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        let (x, y, w, h) = self.px(r);
        self.g.rrect_outline(x, y, w, h, radius * self.scale, width * self.scale, c);
    }
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let s = self.scale;
        self.g.line(self.ox + x0 * s, self.oy + y0 * s, self.ox + x1 * s, self.oy + y1 * s, width * s, c);
    }
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        let (x, y, w, h) = self.px(r);
        self.g.gradient_rect(x, y, w, h, (x, y), (x, y + h), &[(0.0, top), (1.0, bottom)]);
    }
    fn shadow(&mut self, r: Rect, radius: f32, spread: f32, c: Color) {
        let (x, y, w, h) = self.px(r);
        self.g.rrect_glow(x, y, w, h, radius * self.scale, spread * self.scale, c);
    }
    fn text(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle) -> f32 {
        let k = self.scale;
        let face = Self::face(style);
        if self.opts.tracking > 0.0 && !iyakki_uikit::shaping::needs_shaping(s) {
            // letter spacing: one letter at a time, each followed by the extra space (connected scripts are drawn whole)
            let mut pen = 0.0f32;
            let mut buf = [0u8; 4];
            for ch in s.chars() {
                pen += self.g.text(self.ox + x * k + pen, self.oy + y * k, size * k, c, ch.encode_utf8(&mut buf), face) + self.opts.tracking * size * k;
            }
            return pen / k;
        }
        self.g.text(self.ox + x * k, self.oy + y * k, size * k, c, s, face) / k
    }
    fn measure(&mut self, size: f32, s: &str, style: TextStyle) -> f32 {
        let extra = if iyakki_uikit::shaping::needs_shaping(s) { 0.0 } else { tracking_extra(self.opts.tracking, size, s) };
        self.g.text_width(size * self.scale, s, Self::face(style)) / self.scale + extra
    }
    fn set_type(&mut self, o: TextOpts) {
        self.opts = o.clamped();
    }
    fn type_opts(&self) -> TextOpts {
        self.opts
    }
    fn blur(&mut self, r: Rect, radius: f32) {
        // Blur the pixels inside `r` (a box blur, twice), so what a glass surface lies on is frosted.
        let (x, y, w, h) = self.px(r);
        let (pw, ph) = (self.g.width() as i32, self.g.height() as i32);
        let (x0, y0, x1, y1) = ((x.floor() as i32).clamp(0, pw), (y.floor() as i32).clamp(0, ph), ((x + w).ceil() as i32).clamp(0, pw), ((y + h).ceil() as i32).clamp(0, ph));
        let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let rad = (radius * self.scale / 2.0).round() as usize;
        if bw == 0 || bh == 0 || rad == 0 {
            return;
        }
        let mut region = vec![0u8; bw * bh * 4];
        let data = self.g.pm.data_mut();
        for row in 0..bh {
            let src = ((y0 as usize + row) * pw as usize + x0 as usize) * 4;
            region[row * bw * 4..(row + 1) * bw * 4].copy_from_slice(&data[src..src + bw * 4]);
        }
        iyakki_uikit::blur::box_blur(&mut region, bw, bh, rad);
        iyakki_uikit::blur::box_blur(&mut region, bw, bh, rad);
        for row in 0..bh {
            let dst = ((y0 as usize + row) * pw as usize + x0 as usize) * 4;
            data[dst..dst + bw * 4].copy_from_slice(&region[row * bw * 4..(row + 1) * bw * 4]);
        }
    }
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn Canvas)) {
        let (x, y, w, h) = self.px(r);
        let (ox, oy, cw, ch, scale, opts) = (self.ox, self.oy, self.w, self.h, self.scale, self.opts);
        self.g.clipped(R { x, y, w, h }, |g| {
            let mut inner = RasterCanvas { g, ox, oy, w: cw, h: ch, scale, opts };
            f(&mut inner);
        });
    }
    fn image(&mut self, r: Rect, img: &Image) {
        // pixels are drawn; a JPEG needs a decoder this canvas does not have, so the caller asks the core for RGBA when it draws on pixels
        if img.format == ImageFormat::Rgba && img.check().is_ok() {
            let (x, y, w, h) = self.px(r);
            self.g.rgba_picture(x, y, w, h, img.width, img.height, &img.data);
        }
    }
    fn layer(&mut self, alpha: f32, f: &mut dyn FnMut(&mut dyn Canvas)) {
        let mut inner = crate::canvas::AlphaCanvas { inner: self, alpha: alpha.clamp(0.0, 1.0) };
        f(&mut inner);
    }
    fn path(&mut self, fill: Option<Color>, stroke: Option<(Color, f32)>, segs: &[Seg]) {
        let (k, ox, oy) = (self.scale, self.ox, self.oy);
        let polys: Vec<(Vec<(f32, f32)>, bool)> =
            crate::path::flatten(segs, 0.25 / k).into_iter().map(|p| (p.pts.into_iter().map(|(x, y)| (ox + x * k, oy + y * k)).collect(), p.closed)).collect();
        // a fill and a stroke in the same call: the fill first, then the outline on top
        if fill.is_some() {
            self.g.polys(&polys, fill, None);
        }
        if let Some((c, w)) = stroke {
            self.g.polys(&polys, None, Some((w * k, c)));
        }
    }
    fn hit(&mut self, r: Rect, hit: Hit) {
        let (x, y, w, h) = self.px(r);
        self.g.hit(R { x, y, w, h }, Event::Row(hit.action as usize), &hit.name);
    }
}
