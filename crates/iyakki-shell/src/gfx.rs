//! Software 2D drawing on a `tiny-skia` pixmap: rectangles, rounded cards, anti-aliased vector
//! icons, and TrueType text (DejaVu, bundled). No GPU, window system or Mesa needed, so the same
//! code draws to a Linux framebuffer on bare metal, to a PNG for tests, or into any window.

use crate::safeclamp::SafeClamp;
use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use crate::ui::{Event, Hit, R};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, Rect, Shader, SpreadMode, Stroke,
    Transform,
};

pub const fn rgb(r: u8, g: u8, b: u8) -> [u8; 4] {
    [r, g, b, 255]
}

fn col(c: [u8; 4]) -> Color {
    let c = crate::theme::themed(c);
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

/// No text is drawn smaller than this many pixels, whatever the window's scale: small windows get less room, not unreadable text.
pub const MIN_TEXT_PX: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Sans,
    Bold,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Chat,
    Rag,
    Mcp,
    Cli,
    Users,
    Lock,
    Power,
    Logout,
    Settings,
    Clipboard,
    Mic,
    Speaker,
    Runs,
    Edit,
    Browser,
    Camera,
    Mesh,
    Radio,
    Scan,
    Skills,
    Usage,
    Calendar,
    System,
    Media,
    SrePilot,
    Ide,
    Bell,
    Data,
    Tales,
    Store,
    Dash,
    Flows,
    /// The Devices button: a screen on its stand.
    Devices,
    /// The DISKS tile: a drive with its light.
    Disks,
    /// The LAB tile: a flask.
    Lab,
    /// The GIT tile: a branch with three nodes.
    Git,
    /// The DOCTOR tile: a rounded case with a plus.
    Doctor,
    /// The CHECK-UP tile: a clipboard with a tick.
    Checkup,
    Learn,
    /// The top bar's speaker: 0 muted (a cross), 1 low, 2 medium, 3 high (one to three sound waves), 4 no sound output (slashed).
    Volume(u8),
    /// The top bar's Home button: a house (roof, walls, door).
    Home,
}

type GlyphKey = (char, u32, Face);

pub struct Gfx {
    pub pm: Pixmap,
    /// Clickable regions registered during this frame (later = on top).
    pub hits: Vec<Hit>,
    /// Pointer position, for hover highlights.
    pub mouse: (i32, i32),
    /// Every rectangle the frame asked about with [`Gfx::hovered`]: only a pointer crossing one of their edges
    /// can change the picture.
    probes: std::cell::RefCell<Vec<R>>,
    sans: Font,
    bold: Font,
    mono: Font,
    cache: HashMap<GlyphKey, (fontdue::Metrics, Vec<u8>)>,
    /// The pixels outside a clip rectangle while [`Gfx::clipped`] runs (kept between calls so nothing is allocated per frame).
    clip_buf: Vec<u8>,
    /// Made the first time text in a script that needs shaping appears (it parses a couple of MB of fonts).
    shaper: Option<crate::shaping::Shaper>,
    shaped_cache: HashMap<(usize, u16, u32), (fontdue::Metrics, Vec<u8>)>,
    /// Test builds only: every string passed to [`Gfx::text`] with its top-left position, for `TileHarness`.
    #[cfg(test)]
    pub text_log: Vec<(f32, f32, String)>,
    /// While `Some`, every string drawn is kept here (developer mode's `iyakki dev state`, which asks for it a few seconds at a time).
    /// Capped, and switched on by the shell only for the frames a computer asked for.
    pub text_cap: Option<Vec<(f32, f32, String)>>,
    /// Test builds only: what the overflow detector (`monkey_tests`, KI-015) needs: every single-line text with its box and the innermost
    /// clip it was drawn in, and every opaque fill, both in drawing order (`seq`).
    #[cfg(test)]
    pub layout_log: LayoutLog,
}

/// See [`Gfx::layout_log`].
#[cfg(test)]
#[derive(Default)]
pub struct LayoutLog {
    pub seq: u32,
    pub clips: Vec<R>,
    pub texts: Vec<(u32, R, Option<R>, String)>,
    pub fills: Vec<(u32, R)>,
}

#[cfg(test)]
impl LayoutLog {
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        if c[3] >= 250 {
            self.seq += 1;
            self.fills.push((self.seq, R::new(x, y, w, h)));
        }
    }
}

fn load(bytes: &[u8]) -> Font {
    Font::from_bytes(bytes, FontSettings::default()).expect("bundled font is valid")
}

/// Blend one glyph bitmap into premultiplied RGBA pixels (clipped to the pixmap).
fn blit(data: &mut [u8], (w, h): (i32, i32), gx: i32, gy: i32, m: &fontdue::Metrics, bmp: &[u8], color: [u8; 4]) {
    for row in 0..m.height as i32 {
        let py = gy + row;
        if py < 0 || py >= h {
            continue;
        }
        for colm in 0..m.width as i32 {
            let px_ = gx + colm;
            if px_ < 0 || px_ >= w {
                continue;
            }
            let cov = bmp[(row as usize) * m.width + colm as usize] as u32;
            if cov == 0 {
                continue;
            }
            let a = cov * color[3] as u32 / 255;
            let i = ((py * w + px_) * 4) as usize;
            for k in 0..3 {
                // destination is premultiplied RGBA; source colour is straight
                let src = color[k] as u32 * a / 255;
                data[i + k] = (src + data[i + k] as u32 * (255 - a) / 255).min(255) as u8;
            }
            data[i + 3] = (a + data[i + 3] as u32 * (255 - a) / 255).min(255) as u8;
        }
    }
}

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(col(c));
    p.anti_alias = true;
    p
}

fn stroke(w: f32) -> Stroke {
    // Widths under ~1px take tiny-skia's hairline path, which can assert on some geometry.
    Stroke { width: w.max(1.25), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.5523 * r; // cubic approximation of a quarter circle
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

impl Gfx {
    pub fn new(w: u32, h: u32) -> Gfx {
        Gfx {
            pm: Pixmap::new(w.max(1), h.max(1)).expect("pixmap"),
            hits: Vec::new(),
            mouse: (-1, -1),
            probes: std::cell::RefCell::new(Vec::new()),
            sans: load(include_bytes!("../assets/DejaVuSans.ttf")),
            bold: load(include_bytes!("../assets/DejaVuSans-Bold.ttf")),
            mono: load(include_bytes!("../assets/DejaVuSansMono.ttf")),
            cache: HashMap::new(),
            clip_buf: Vec::new(),
            shaper: None,
            shaped_cache: HashMap::new(),
            #[cfg(test)]
            text_log: Vec::new(),
            text_cap: None,
            #[cfg(test)]
            layout_log: LayoutLog::default(),
        }
    }

    /// Register a clickable region. `ev` is delivered to the screen on click.
    pub fn hit(&mut self, rect: R, ev: Event, label: &str) {
        self.hits.push(Hit { rect, ev, label: label.into() });
    }

    /// Run `f` so that nothing it draws or registers as clickable lands outside `clip`: pixels outside the rectangle are put
    /// back afterwards and click regions are cut to it (dropped when nothing is left). For scrolled content (a transcript, a
    /// list) whose items are taller than the pane or start above it.
    pub fn clipped<T>(&mut self, clip: R, f: impl FnOnce(&mut Gfx) -> T) -> T {
        let (w, h) = (self.pm.width() as usize, self.pm.height() as usize);
        let x0 = (clip.x.floor().max(0.0) as usize).min(w);
        let x1 = ((clip.x + clip.w).ceil().max(0.0) as usize).min(w).max(x0);
        let y0 = (clip.y.floor().max(0.0) as usize).min(h);
        let y1 = ((clip.y + clip.h).ceil().max(0.0) as usize).min(h).max(y0);
        // only what lies outside the rectangle is kept (rows above and below, then the strips left and right of it), in a
        // buffer that is reused: copying the whole screen each time cost milliseconds per call at 2880x1800
        let mut saved = std::mem::take(&mut self.clip_buf);
        saved.clear();
        let row = w * 4;
        {
            let data = self.pm.data();
            saved.extend_from_slice(&data[..y0 * row]);
            saved.extend_from_slice(&data[y1 * row..h * row]);
            for y in y0..y1 {
                saved.extend_from_slice(&data[y * row..y * row + x0 * 4]);
                saved.extend_from_slice(&data[y * row + x1 * 4..(y + 1) * row]);
            }
        }
        let start = self.hits.len();
        #[cfg(test)]
        self.layout_log.clips.push(clip);
        let out = f(self);
        #[cfg(test)]
        self.layout_log.clips.pop();
        if self.pm.data().len() == h * row {
            let data = self.pm.data_mut();
            let mut at = 0;
            let put = |dst: &mut [u8], at: &mut usize| {
                dst.copy_from_slice(&saved[*at..*at + dst.len()]);
                *at += dst.len();
            };
            put(&mut data[..y0 * row], &mut at);
            put(&mut data[y1 * row..h * row], &mut at);
            for y in y0..y1 {
                put(&mut data[y * row..y * row + x0 * 4], &mut at);
                put(&mut data[y * row + x1 * 4..(y + 1) * row], &mut at);
            }
        }
        self.clip_buf = saved;
        if self.hits.len() >= start {
            let mut kept = Vec::with_capacity(self.hits.len() - start);
            for mut hit in self.hits.drain(start..) {
                let (nx0, ny0) = (hit.rect.x.max(clip.x), hit.rect.y.max(clip.y));
                let (nx1, ny1) = ((hit.rect.x + hit.rect.w).min(clip.x + clip.w), (hit.rect.y + hit.rect.h).min(clip.y + clip.h));
                if nx1 > nx0 && ny1 > ny0 {
                    hit.rect = R::new(nx0, ny0, nx1 - nx0, ny1 - ny0);
                    kept.push(hit);
                }
            }
            self.hits.extend(kept);
        }
        out
    }

    /// Like [`Gfx::clipped`] for a drawing that stays near `clip` (within `margin` pixels on every side): only that ring
    /// around the rectangle is saved and put back, so a small repaint costs the size of the repaint, not of the screen. What `f`
    /// draws farther away than `margin` stays (the caller promises there is nothing).
    pub fn clipped_near<T>(&mut self, clip: R, margin: f32, f: impl FnOnce(&mut Gfx) -> T) -> T {
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let cx0 = (clip.x.floor() as i32).clamp_safe(0, w);
        let cx1 = ((clip.x + clip.w).ceil() as i32).clamp_safe(cx0, w);
        let cy0 = (clip.y.floor() as i32).clamp_safe(0, h);
        let cy1 = ((clip.y + clip.h).ceil() as i32).clamp_safe(cy0, h);
        let m = margin.ceil().max(0.0) as i32;
        let (bx0, bx1) = ((cx0 - m).max(0), (cx1 + m).min(w));
        let (by0, by1) = ((cy0 - m).max(0), (cy1 + m).min(h));
        // the ring: rows above and below the rectangle (full width of the box), then the strips left and right of it
        let rings: [(i32, i32, i32, i32); 4] = [(bx0, by0, bx1, cy0), (bx0, cy1, bx1, by1), (bx0, cy0, cx0, cy1), (cx1, cy0, bx1, cy1)];
        let mut saved = std::mem::take(&mut self.clip_buf);
        saved.clear();
        for &(x0, y0, x1, y1) in &rings {
            self.copy_out(x0, y0, x1, y1, &mut saved);
        }
        let start = self.hits.len();
        #[cfg(test)]
        self.layout_log.clips.push(clip);
        let out = f(self);
        #[cfg(test)]
        self.layout_log.clips.pop();
        let mut at = 0;
        for &(x0, y0, x1, y1) in &rings {
            at += self.copy_in(x0, y0, x1, y1, &saved[at..]);
        }
        self.clip_buf = saved;
        if self.hits.len() >= start {
            let mut kept = Vec::with_capacity(self.hits.len() - start);
            for mut hit in self.hits.drain(start..) {
                let (nx0, ny0) = (hit.rect.x.max(clip.x), hit.rect.y.max(clip.y));
                let (nx1, ny1) = ((hit.rect.x + hit.rect.w).min(clip.x + clip.w), (hit.rect.y + hit.rect.h).min(clip.y + clip.h));
                if nx1 > nx0 && ny1 > ny0 {
                    hit.rect = R::new(nx0, ny0, nx1 - nx0, ny1 - ny0);
                    kept.push(hit);
                }
            }
            self.hits.extend(kept);
        }
        out
    }

    /// Append the pixels of the (clamped) box `x0..x1` x `y0..y1`, row by row, to `out`.
    fn copy_out(&self, x0: i32, y0: i32, x1: i32, y1: i32, out: &mut Vec<u8>) {
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let (x0, x1, y0, y1) = (x0.clamp_safe(0, w), x1.clamp_safe(0, w), y0.clamp_safe(0, h), y1.clamp_safe(0, h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let data = self.pm.data();
        for y in y0..y1 {
            let i = (y * w + x0) as usize * 4;
            out.extend_from_slice(&data[i..i + (x1 - x0) as usize * 4]);
        }
    }

    /// Write rows saved by `copy_out` back into the box; returns the bytes used.
    fn copy_in(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, src: &[u8]) -> usize {
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let (x0, x1, y0, y1) = (x0.clamp_safe(0, w), x1.clamp_safe(0, w), y0.clamp_safe(0, h), y1.clamp_safe(0, h));
        if x1 <= x0 || y1 <= y0 {
            return 0;
        }
        let rowb = (x1 - x0) as usize * 4;
        let data = self.pm.data_mut();
        for (k, y) in (y0..y1).enumerate() {
            let i = (y * w + x0) as usize * 4;
            data[i..i + rowb].copy_from_slice(&src[k * rowb..(k + 1) * rowb]);
        }
        rowb * (y1 - y0) as usize
    }

    /// The pixels of a rectangle (integer pixels, clamped to the screen), row by row.
    pub fn grab(&self, rect: (i32, i32, i32, i32)) -> Vec<u8> {
        let mut v = Vec::new();
        self.copy_out(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3, &mut v);
        v
    }

    /// Put pixels taken with [`Gfx::grab`] back (same rectangle).
    pub fn put(&mut self, rect: (i32, i32, i32, i32), px: &[u8]) {
        self.copy_in(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3, px);
    }

    /// Draw into a small pixmap that holds only `rect`, seeded with `seed` (the pixels of the rectangle as [`Gfx::grab`] gave
    /// them) or with what is on the screen now, and write it back. `f` draws as usual but gets the rectangle's origin and
    /// has to subtract it from its coordinates. A large shape that matters only at its edges then costs the size of the edges.
    /// The pointer position is shifted too, so hover tests keep working; hit regions are not kept.
    pub fn in_region(&mut self, rect: (i32, i32, i32, i32), seed: Option<&[u8]>, f: impl FnOnce(&mut Gfx, f32, f32)) {
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let (x0, y0) = (rect.0.clamp_safe(0, w), rect.1.clamp_safe(0, h));
        let (x1, y1) = ((rect.0 + rect.2).clamp_safe(x0, w), (rect.1 + rect.3).clamp_safe(y0, h));
        let (rw, rh) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let Some(mut scratch) = (if rw > 0 && rh > 0 { Pixmap::new(rw, rh) } else { None }) else { return };
        match seed.filter(|p| p.len() == (rw * rh * 4) as usize) {
            Some(p) => scratch.data_mut().copy_from_slice(p),
            None => {
                let now = self.grab((x0, y0, x1 - x0, y1 - y0));
                scratch.data_mut().copy_from_slice(&now);
            }
        }
        std::mem::swap(&mut self.pm, &mut scratch);
        let (mouse, nhits) = (self.mouse, self.hits.len());
        self.mouse = (mouse.0 - x0, mouse.1 - y0);
        f(self, x0 as f32, y0 as f32);
        self.mouse = mouse;
        self.hits.truncate(nhits);
        std::mem::swap(&mut self.pm, &mut scratch);
        self.put((x0, y0, x1 - x0, y1 - y0), scratch.data());
    }

    /// Drop every region registered so far (a modal calls this so the dimmed UI behind it can't be clicked).
    pub fn clear_hits(&mut self) {
        self.hits.clear();
    }

    pub fn hovered(&self, r: R) -> bool {
        self.probes.borrow_mut().push(r);
        r.contains(self.mouse.0, self.mouse.1)
    }

    /// Forget the rectangles probed so far (start of a frame).
    pub fn clear_probes(&mut self) {
        self.probes.get_mut().clear();
    }

    /// The rectangles probed during the last frame.
    pub fn probes(&self) -> Vec<R> {
        self.probes.borrow().clone()
    }

    /// Pill button; returns its width. `ev` is what a click does (usually the same key as the shortcut). The look follows
    /// the accent (see [`crate::fx::Variant`]): neon outline and glow for primary, red for danger, quiet for secondary.
    pub fn button(&mut self, x: f32, y: f32, size: f32, label: &str, accent: [u8; 4], ev: Event) -> f32 {
        let w = self.text_width(size, label, Face::Bold) + size * 1.7;
        let h = size * 2.1;
        let r = R::new(x, y, w, h);
        let hot = self.hovered(r);
        let tc = crate::fx::pill(self, r, size / 14.0, accent, hot, true);
        self.text(x + size * 0.85, y + (h - self.line_height(size, Face::Bold)) / 2.0, size, tc, label, Face::Bold);
        self.hit(r, ev, label);
        w
    }

    /// A button for fingers: drawn like [`Gfx::button`], but the area that reacts (and lights up under the pointer) is at least
    /// `hit_h` tall, centred on the drawn pill. Returns the width.
    #[allow(clippy::too_many_arguments)]
    pub fn button_touch(&mut self, x: f32, y: f32, size: f32, label: &str, accent: [u8; 4], ev: Event, hit_h: f32) -> f32 {
        let w = self.text_width(size, label, Face::Bold) + size * 1.7;
        let h = size * 2.1;
        let hit_h = hit_h.max(h);
        let pad = (hit_h - h) / 2.0;
        let r = R::new(x, y, w, h);
        let hot = self.hovered(R::new(x, y - pad, w, hit_h));
        let tc = crate::fx::pill(self, r, size / 14.0, accent, hot, true);
        self.text(x + size * 0.85, y + (h - self.line_height(size, Face::Bold)) / 2.0, size, tc, label, Face::Bold);
        self.hit(R::new(x, y - pad, w, hit_h), ev, label);
        w
    }

    /// Round icon button; `active` highlights it (e.g. recording). Returns its diameter.
    pub fn icon_button(&mut self, x: f32, y: f32, d: f32, icon: Icon, color: [u8; 4], active: bool, ev: Event, label: &str) -> f32 {
        let r = R::new(x, y, d, d);
        let hot = self.hovered(r);
        if active {
            self.rrect_glow(x, y, d, d, d / 2.0, 4.0, [255, 90, 100, 80]);
            self.rrect(x, y, d, d, d / 2.0, rgb(120, 40, 50));
            self.rrect_outline(x, y, d, d, d / 2.0, 1.4, rgb(255, 120, 120));
        } else {
            crate::fx::pill(self, r, d / 40.0, color, hot, true);
        }
        self.icon(icon, x + d / 2.0, y + d / 2.0, d * 0.5, if active { rgb(255, 200, 200) } else { color });
        self.hit(r, ev, label);
        d
    }

    /// A button that is shown but inactive.
    pub fn button_off(&mut self, x: f32, y: f32, size: f32, label: &str) -> f32 {
        let w = self.text_width(size, label, Face::Bold) + size * 1.7;
        let h = size * 2.1;
        let tc = crate::fx::pill(self, R::new(x, y, w, h), size / 14.0, [0, 0, 0, 255], false, false);
        self.text(x + size * 0.85, y + (h - self.line_height(size, Face::Bold)) / 2.0, size, tc, label, Face::Bold);
        w
    }

    pub fn width(&self) -> u32 {
        self.pm.width()
    }
    pub fn height(&self) -> u32 {
        self.pm.height()
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if (w, h) != (self.width(), self.height()) {
            self.pm = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
        }
    }

    pub fn clear(&mut self, c: [u8; 4]) {
        self.pm.fill(col(c));
    }

    /// Vertical gradient background.
    pub fn gradient(&mut self, top: [u8; 4], bottom: [u8; 4]) {
        let h = self.height() as f32;
        let shader = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, h),
            vec![GradientStop::new(0.0, col(top)), GradientStop::new(1.0, col(bottom))],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let mut p = Paint::default();
        p.shader = shader.unwrap_or(Shader::SolidColor(col(top)));
        if let Some(r) = Rect::from_xywh(0.0, 0.0, self.width() as f32, h) {
            self.pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }

    /// A rectangle filled with a gradient from (x0, y0) to (x1, y1); `stops` are (position 0..1, colour).
    pub fn gradient_rect(&mut self, x: f32, y: f32, w: f32, h: f32, from: (f32, f32), to: (f32, f32), stops: &[(f32, [u8; 4])]) {
        let stops: Vec<GradientStop> = stops.iter().map(|(p, c)| GradientStop::new(*p, col(*c))).collect();
        let Some(shader) = LinearGradient::new(Point::from_xy(from.0, from.1), Point::from_xy(to.0, to.1), stops, SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
            self.pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }

    /// A soft round glow: `c` at its own alpha in the middle, fading to nothing at `radius`.
    pub fn radial_glow(&mut self, cx: f32, cy: f32, radius: f32, c: [u8; 4]) {
        let faded = [c[0], c[1], c[2], 0];
        let Some(shader) = tiny_skia::RadialGradient::new(Point::from_xy(cx, cy), Point::from_xy(cx, cy), radius, vec![GradientStop::new(0.0, col(c)), GradientStop::new(1.0, col(faded))], SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0) {
            self.pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }

    /// A glow around a rounded rectangle: three translucent rings growing outwards, `c` is the strongest alpha.
    pub fn rrect_glow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, spread: f32, c: [u8; 4]) {
        for (k, f) in [(1.0f32, 0.35f32), (0.66, 0.6), (0.33, 1.0)] {
            let d = spread * k;
            let a = (c[3] as f32 * f * 0.5) as u8;
            self.rrect(x - d, y - d, w + 2.0 * d, h + 2.0 * d, r + d, [c[0], c[1], c[2], a]);
        }
    }

    /// Axis-aligned fill, snapped to whole pixels (crisp edges; avoids tiny-skia's thin-rect hairline path).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        #[cfg(test)]
        self.layout_log.fill(x, y, w, h, c);
        if let Some(r) = Rect::from_xywh(x.round(), y.round(), w.round().max(1.0), h.round().max(1.0)) {
            self.pm.fill_rect(r, &paint(c), Transform::identity(), None);
        }
    }

    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: [u8; 4]) {
        #[cfg(test)]
        self.layout_log.fill(x, y, w, h, c);
        if let Some(p) = rrect_path(x, y, w, h, r) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    pub fn rrect_outline(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: [u8; 4]) {
        if let Some(p) = rrect_path(x, y, w, h, r) {
            self.pm.stroke_path(&p, &paint(c), &stroke(width), Transform::identity(), None);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// A circle's outline.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: [u8; 4]) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm.stroke_path(&p, &paint(c), &stroke(width), Transform::identity(), None);
        }
    }

    /// A straight line with round ends.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: [u8; 4]) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        if let Some(p) = pb.finish() {
            let mut st = stroke(width);
            st.line_cap = tiny_skia::LineCap::Round;
            self.pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
        }
    }

    /// A filled polygon (a play arrow, a skip mark).
    pub fn polygon(&mut self, pts: &[(f32, f32)], c: [u8; 4]) {
        let mut pb = PathBuilder::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            if i == 0 {
                pb.move_to(*x, *y);
            } else {
                pb.line_to(*x, *y);
            }
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    fn font(&self, f: Face) -> &Font {
        match f {
            Face::Sans => &self.sans,
            Face::Bold => &self.bold,
            Face::Mono => &self.mono,
        }
    }

    pub fn line_height(&self, size: f32, face: Face) -> f32 {
        self.font(face).horizontal_line_metrics(size).map_or(size * 1.3, |m| m.new_line_size)
    }

    pub fn text_width(&mut self, size: f32, s: &str, face: Face) -> f32 {
        let px = size.round().max(MIN_TEXT_PX);
        if crate::shaping::needs_shaping(s) {
            return self.shaper.get_or_insert_with(crate::shaping::Shaper::new).layout(s, px).1;
        }
        s.chars().map(|c| self.font(face).metrics(c, px).advance_width).sum()
    }

    /// Draw `s` with its top-left at (x, y). Returns the advance width. Text is clipped to the pixmap.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: [u8; 4], s: &str, face: Face) -> f32 {
        #[cfg(test)]
        {
            self.text_log.push((x, y, s.to_string()));
            if !s.contains('\n') && !s.trim().is_empty() {
                let (tw, th) = (self.text_width(size, s, face), self.line_height(size.round().max(MIN_TEXT_PX), face));
                self.layout_log.seq += 1;
                let (seq, clip) = (self.layout_log.seq, self.layout_log.clips.last().copied());
                self.layout_log.texts.push((seq, R::new(x, y, tw, th), clip, s.to_string()));
            }
        }
        if let Some(v) = self.text_cap.as_mut() {
            if v.len() < 4000 {
                v.push((x, y, s.to_string()));
            }
        }
        let color = crate::theme::themed(color);
        let px = size.round().max(MIN_TEXT_PX);
        let ascent = self.font(face).horizontal_line_metrics(px).map_or(px, |m| m.ascent);
        let base = y + ascent;
        let (w, h) = (self.width() as i32, self.height() as i32);
        if crate::shaping::needs_shaping(s) {
            return self.shaped_text(x, base, px, color, s);
        }
        let mut pen = x;
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let key = (ch, px as u32, face);
            if !self.cache.contains_key(&key) {
                let (m, bmp) = self.font(face).rasterize(ch, px);
                self.cache.insert(key, (m, bmp));
            }
            let (m, bmp) = &self.cache[&key];
            let (gx, gy) = ((pen + m.xmin as f32).round() as i32, (base - m.height as f32 - m.ymin as f32).round() as i32);
            blit(self.pm.data_mut(), (w, h), gx, gy, m, bmp, color);
            pen += m.advance_width;
        }
        pen - x
    }

    /// `s` as coverage, for pictures that draw their own text (SVG): white text on a clear pixmap, kept as alpha. The mask's
    /// corner is given relative to the pen on the baseline. Empty when the text is too wide to be worth drawing.
    pub fn text_mask(&mut self, size: f32, s: &str, face: Face) -> iyakki_web::svg::TextMask {
        use iyakki_web::svg::TextMask;
        let px = size.round().max(1.0);
        let advance = self.text_width(px, s, face);
        let ascent = self.font(face).horizontal_line_metrics(px).map_or(px, |m| m.ascent);
        let lh = self.line_height(px, face).max(px * 1.2);
        let pad = 3.0f32;
        let (w, h) = ((advance.ceil() + 2.0 * pad + px * 0.3) as u32, (lh.ceil() + 2.0 * pad + px * 0.3) as u32);
        if !advance.is_finite() || w > 4096 || h > 800 {
            return TextMask { advance: advance as f64, ..TextMask::default() };
        }
        let Some(scratch) = Pixmap::new(w, h) else { return TextMask::default() };
        let old = std::mem::replace(&mut self.pm, scratch);
        self.text(pad, pad, px, [255, 255, 255, 255], s, face);
        let drawn = std::mem::replace(&mut self.pm, old);
        let alpha: Vec<u8> = drawn.data().chunks_exact(4).map(|p| p[3]).collect();
        TextMask { advance: advance as f64, x: -(pad as i32), y: -((pad + ascent).round() as i32), w, h, alpha }
    }

    /// Draw a line in a script that needs shaping (see [`crate::shaping`]); `base` is the baseline.
    fn shaped_text(&mut self, x: f32, base: f32, px: f32, color: [u8; 4], s: &str) -> f32 {
        let (w, h) = (self.width() as i32, self.height() as i32);
        let sh = self.shaper.get_or_insert_with(crate::shaping::Shaper::new);
        let (glyphs, total) = sh.layout(s, px);
        for g in glyphs {
            let key = (g.font, g.glyph, px as u32);
            if !self.shaped_cache.contains_key(&key) {
                let r = sh.font(g.font).rasterize_indexed(g.glyph, px);
                self.shaped_cache.insert(key, r);
            }
            let (m, bmp) = &self.shaped_cache[&key];
            let gx = (x + g.x + m.xmin as f32).round() as i32;
            let gy = (base - g.y - m.height as f32 - m.ymin as f32).round() as i32;
            blit(self.pm.data_mut(), (w, h), gx, gy, m, bmp, color);
        }
        total
    }

    /// Truncate with an ellipsis so the text fits in `max_w`.
    pub fn fit(&mut self, size: f32, s: &str, face: Face, max_w: f32) -> String {
        if self.text_width(size, s, face) <= max_w {
            return s.to_string();
        }
        if !crate::shaping::needs_shaping(s) {
            // one pass over the glyph advances (the width of a plain string is their sum) instead of measuring every shorter prefix
            let px = size.round().max(MIN_TEXT_PX);
            let room = max_w - self.font(face).metrics('…', px).advance_width;
            let mut acc = 0.0;
            let mut end = 0;
            for (i, c) in s.char_indices() {
                acc += self.font(face).metrics(c, px).advance_width;
                if acc > room {
                    break;
                }
                end = i + c.len_utf8();
            }
            return format!("{}…", &s[..end]);
        }
        let mut out: String = s.to_string();
        while !out.is_empty() && self.text_width(size, &format!("{out}…"), face) > max_w {
            out.pop();
        }
        format!("{out}…")
    }

    /// Word-wrap into lines no wider than `max_w`.
    pub fn wrap(&mut self, size: f32, s: &str, face: Face, max_w: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split(' ') {
                let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if self.text_width(size, &cand, face) <= max_w || cur.is_empty() {
                    cur = cand;
                    // a single over-long word: hard-break it
                    while self.text_width(size, &cur, face) > max_w && cur.chars().count() > 1 {
                        let mut head = String::new();
                        for ch in cur.chars() {
                            if self.text_width(size, &format!("{head}{ch}"), face) > max_w && !head.is_empty() {
                                break;
                            }
                            head.push(ch);
                        }
                        let rest: String = cur.chars().skip(head.chars().count()).collect();
                        lines.push(head);
                        cur = rest;
                    }
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                }
            }
            lines.push(cur);
        }
        lines
    }

    /// Draw an icon centred at (cx, cy) fitting a `size` square.
    pub fn icon(&mut self, icon: Icon, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
        let s = size;
        let (x0, y0) = (cx - s / 2.0, cy - s / 2.0);
        let lw = (s * 0.075).max(1.5);
        let st = stroke(lw);
        let id = Transform::identity();
        let p = paint(c);
        let mut pb = PathBuilder::new();
        match icon {
            Icon::Chat => {
                // speech bubble with tail + three dots
                let (bx, by, bw, bh) = (x0 + s * 0.08, y0 + s * 0.12, s * 0.84, s * 0.58);
                if let Some(b) = rrect_path(bx, by, bw, bh, s * 0.16) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                pb.move_to(x0 + s * 0.28, by + bh);
                pb.line_to(x0 + s * 0.22, y0 + s * 0.90);
                pb.line_to(x0 + s * 0.46, by + bh);
                if let Some(t) = pb.finish() {
                    self.pm.stroke_path(&t, &p, &st, id, None);
                }
                for i in 0..3 {
                    self.circle(x0 + s * (0.33 + 0.17 * i as f32), by + bh / 2.0, s * 0.045, c);
                }
            }
            Icon::Rag => {
                // document with folded corner and text lines, plus a magnifier
                let (dx, dy, dw, dh) = (x0 + s * 0.12, y0 + s * 0.08, s * 0.56, s * 0.74);
                let fold = s * 0.18;
                pb.move_to(dx, dy);
                pb.line_to(dx + dw - fold, dy);
                pb.line_to(dx + dw, dy + fold);
                pb.line_to(dx + dw, dy + dh);
                pb.line_to(dx, dy + dh);
                pb.close();
                pb.move_to(dx + dw - fold, dy);
                pb.line_to(dx + dw - fold, dy + fold);
                pb.line_to(dx + dw, dy + fold);
                if let Some(d) = pb.finish() {
                    self.pm.stroke_path(&d, &p, &st, id, None);
                }
                for i in 0..3 {
                    let ly = dy + s * (0.30 + 0.14 * i as f32);
                    let mut l = PathBuilder::new();
                    l.move_to(dx + s * 0.1, ly);
                    l.line_to(dx + s * (if i == 2 { 0.28 } else { 0.4 }), ly);
                    if let Some(l) = l.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                }
                // magnifier
                let (mx, my, mr) = (x0 + s * 0.68, y0 + s * 0.64, s * 0.17);
                if let Some(ring) = PathBuilder::from_circle(mx, my, mr) {
                    self.pm.stroke_path(&ring, &p, &st, id, None);
                }
                let mut h = PathBuilder::new();
                h.move_to(mx + mr * 0.72, my + mr * 0.72);
                h.line_to(x0 + s * 0.92, y0 + s * 0.88);
                if let Some(h) = h.finish() {
                    self.pm.stroke_path(&h, &p, &stroke(lw * 1.4), id, None);
                }
            }
            Icon::Mcp => {
                // hub-and-spoke: a central node connected to four peripheral nodes
                let (hx, hy) = (cx, cy);
                let nodes = [(0.18, 0.2), (0.82, 0.2), (0.14, 0.78), (0.86, 0.78)];
                for (nx, ny) in nodes {
                    let mut l = PathBuilder::new();
                    l.move_to(hx, hy);
                    l.line_to(x0 + s * nx, y0 + s * ny);
                    if let Some(l) = l.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                }
                for (nx, ny) in nodes {
                    self.circle(x0 + s * nx, y0 + s * ny, s * 0.085, c);
                }
                self.circle(hx, hy, s * 0.17, c);
                // socket notch in the hub
                if let Some(i) = PathBuilder::from_circle(hx, hy, s * 0.07) {
                    let mut clear = Paint::default();
                    clear.set_color(Color::from_rgba8(0, 0, 0, 90));
                    self.pm.fill_path(&i, &clear, FillRule::Winding, id, None);
                }
            }
            Icon::Cli => {
                // terminal window with a prompt chevron and cursor underscore
                if let Some(w) = rrect_path(x0 + s * 0.06, y0 + s * 0.14, s * 0.88, s * 0.72, s * 0.12) {
                    self.pm.stroke_path(&w, &p, &st, id, None);
                }
                let mut ln = PathBuilder::new();
                ln.move_to(x0 + s * 0.06, y0 + s * 0.32);
                ln.line_to(x0 + s * 0.94, y0 + s * 0.32);
                if let Some(l) = ln.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                let mut ch = PathBuilder::new();
                ch.move_to(x0 + s * 0.24, y0 + s * 0.46);
                ch.line_to(x0 + s * 0.40, y0 + s * 0.58);
                ch.line_to(x0 + s * 0.24, y0 + s * 0.70);
                if let Some(ch) = ch.finish() {
                    self.pm.stroke_path(&ch, &p, &stroke(lw * 1.3), id, None);
                }
                let mut us = PathBuilder::new();
                us.move_to(x0 + s * 0.50, y0 + s * 0.72);
                us.line_to(x0 + s * 0.72, y0 + s * 0.72);
                if let Some(us) = us.finish() {
                    self.pm.stroke_path(&us, &p, &stroke(lw * 1.3), id, None);
                }
            }
            Icon::Users => {
                // two people: head circles and shoulder arcs
                for (px, py, k) in [(0.36, 0.34, 1.0), (0.68, 0.40, 0.8)] {
                    if let Some(head) = PathBuilder::from_circle(x0 + s * px, y0 + s * py, s * 0.13 * k) {
                        self.pm.stroke_path(&head, &p, &st, id, None);
                    }
                    let mut sh = PathBuilder::new();
                    let (sx, sy, sw) = (x0 + s * px, y0 + s * (py + 0.46 * k), s * 0.26 * k);
                    sh.move_to(sx - sw, sy);
                    sh.quad_to(sx - sw, sy - s * 0.26 * k, sx, sy - s * 0.26 * k);
                    sh.quad_to(sx + sw, sy - s * 0.26 * k, sx + sw, sy);
                    if let Some(sh) = sh.finish() {
                        self.pm.stroke_path(&sh, &p, &st, id, None);
                    }
                }
            }
            Icon::Settings => {
                // gear: ring, eight teeth, hub
                let (r_out, r_in) = (s * 0.30, s * 0.12);
                if let Some(ring) = PathBuilder::from_circle(cx, cy, r_out) {
                    self.pm.stroke_path(&ring, &p, &st, id, None);
                }
                if let Some(hub) = PathBuilder::from_circle(cx, cy, r_in) {
                    self.pm.stroke_path(&hub, &p, &st, id, None);
                }
                for i in 0..8 {
                    let a = i as f32 * std::f32::consts::TAU / 8.0;
                    let (dx, dy) = (a.cos(), a.sin());
                    let mut t = PathBuilder::new();
                    t.move_to(cx + dx * r_out, cy + dy * r_out);
                    t.line_to(cx + dx * s * 0.44, cy + dy * s * 0.44);
                    if let Some(t) = t.finish() {
                        self.pm.stroke_path(&t, &p, &stroke(lw * 1.8), id, None);
                    }
                }
            }
            Icon::Clipboard => {
                // board with a clip at the top and two text lines
                if let Some(b) = rrect_path(x0 + s * 0.16, y0 + s * 0.16, s * 0.68, s * 0.76, s * 0.1) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                if let Some(c) = rrect_path(x0 + s * 0.34, y0 + s * 0.06, s * 0.32, s * 0.2, s * 0.06) {
                    self.pm.fill_path(&c, &p, FillRule::Winding, id, None);
                }
                for (i, w) in [0.34f32, 0.24].iter().enumerate() {
                    let mut l = PathBuilder::new();
                    l.move_to(x0 + s * 0.3, y0 + s * (0.5 + 0.18 * i as f32));
                    l.line_to(x0 + s * (0.3 + w), y0 + s * (0.5 + 0.18 * i as f32));
                    if let Some(l) = l.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                }
            }
            Icon::Runs => {
                // a vertical timeline: three steps joined by a line, the last one checked
                let mut line = PathBuilder::new();
                line.move_to(x0 + s * 0.22, y0 + s * 0.2);
                line.line_to(x0 + s * 0.22, y0 + s * 0.8);
                if let Some(l) = line.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                for (i, w) in [0.62f32, 0.5, 0.58].iter().enumerate() {
                    let y = y0 + s * (0.2 + 0.3 * i as f32);
                    self.circle(x0 + s * 0.22, y, s * 0.085, c);
                    let mut bar = PathBuilder::new();
                    bar.move_to(x0 + s * 0.4, y);
                    bar.line_to(x0 + s * (0.4 + w * 0.55), y);
                    if let Some(b) = bar.finish() {
                        self.pm.stroke_path(&b, &p, &st, id, None);
                    }
                }
                let mut tick = PathBuilder::new();
                tick.move_to(x0 + s * 0.68, y0 + s * 0.74);
                tick.line_to(x0 + s * 0.76, y0 + s * 0.82);
                tick.line_to(x0 + s * 0.92, y0 + s * 0.64);
                if let Some(t) = tick.finish() {
                    self.pm.stroke_path(&t, &p, &stroke(lw * 1.4), id, None);
                }
            }
            Icon::Browser => {
                // a globe: outline, equator, two parallels and a meridian
                let (gx, gy, r) = (x0 + s * 0.5, y0 + s * 0.5, s * 0.40);
                if let Some(c) = PathBuilder::from_circle(gx, gy, r) {
                    self.pm.stroke_path(&c, &p, &st, id, None);
                }
                if let Some(m) = tiny_skia::Rect::from_xywh(gx - r * 0.42, gy - r, r * 0.84, r * 2.0).and_then(PathBuilder::from_oval) {
                    self.pm.stroke_path(&m, &p, &st, id, None);
                }
                for (dy, half) in [(0.0f32, r), (-0.5, r * 0.866), (0.5, r * 0.866)] {
                    let mut l = PathBuilder::new();
                    l.move_to(gx - half, gy + dy * r);
                    l.line_to(gx + half, gy + dy * r);
                    if let Some(l) = l.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                }
                let mut v = PathBuilder::new();
                v.move_to(gx, gy - r);
                v.line_to(gx, gy + r);
                if let Some(v) = v.finish() {
                    self.pm.stroke_path(&v, &p, &st, id, None);
                }
            }
            Icon::Edit => {
                // a page with text lines and a pencil across its corner
                if let Some(b) = rrect_path(x0 + s * 0.12, y0 + s * 0.14, s * 0.56, s * 0.72, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                for (i, w) in [0.3f32, 0.22, 0.3].iter().enumerate() {
                    let y = y0 + s * (0.34 + 0.17 * i as f32);
                    let mut l = PathBuilder::new();
                    l.move_to(x0 + s * 0.24, y);
                    l.line_to(x0 + s * (0.24 + w), y);
                    if let Some(l) = l.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                }
                let mut pen = PathBuilder::new();
                pen.move_to(x0 + s * 0.84, y0 + s * 0.2);
                pen.line_to(x0 + s * 0.5, y0 + s * 0.58);
                if let Some(pn) = pen.finish() {
                    self.pm.stroke_path(&pn, &p, &stroke(lw * 2.4), id, None);
                }
                let mut tip = PathBuilder::new();
                tip.move_to(x0 + s * 0.5, y0 + s * 0.58);
                tip.line_to(x0 + s * 0.42, y0 + s * 0.74);
                tip.line_to(x0 + s * 0.6, y0 + s * 0.68);
                tip.close();
                if let Some(t) = tip.finish() {
                    self.pm.fill_path(&t, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Mic => {
                // capsule, pickup arc, stem and base
                if let Some(c) = rrect_path(x0 + s * 0.34, y0 + s * 0.06, s * 0.32, s * 0.5, s * 0.16) {
                    self.pm.fill_path(&c, &p, FillRule::Winding, id, None);
                }
                let mut arc = PathBuilder::new();
                arc.move_to(x0 + s * 0.2, y0 + s * 0.42);
                arc.cubic_to(x0 + s * 0.2, y0 + s * 0.78, x0 + s * 0.8, y0 + s * 0.78, x0 + s * 0.8, y0 + s * 0.42);
                arc.move_to(x0 + s * 0.5, y0 + s * 0.74);
                arc.line_to(x0 + s * 0.5, y0 + s * 0.92);
                arc.move_to(x0 + s * 0.34, y0 + s * 0.92);
                arc.line_to(x0 + s * 0.66, y0 + s * 0.92);
                if let Some(a) = arc.finish() {
                    self.pm.stroke_path(&a, &p, &st, id, None);
                }
            }
            Icon::Speaker => {
                // cone with two sound waves
                let mut cone = PathBuilder::new();
                cone.move_to(x0 + s * 0.12, y0 + s * 0.38);
                cone.line_to(x0 + s * 0.32, y0 + s * 0.38);
                cone.line_to(x0 + s * 0.54, y0 + s * 0.18);
                cone.line_to(x0 + s * 0.54, y0 + s * 0.82);
                cone.line_to(x0 + s * 0.32, y0 + s * 0.62);
                cone.line_to(x0 + s * 0.12, y0 + s * 0.62);
                cone.close();
                if let Some(c) = cone.finish() {
                    self.pm.fill_path(&c, &p, FillRule::Winding, id, None);
                }
                let mut waves = PathBuilder::new();
                waves.move_to(x0 + s * 0.68, y0 + s * 0.36);
                waves.cubic_to(x0 + s * 0.78, y0 + s * 0.44, x0 + s * 0.78, y0 + s * 0.56, x0 + s * 0.68, y0 + s * 0.64);
                waves.move_to(x0 + s * 0.78, y0 + s * 0.24);
                waves.cubic_to(x0 + s * 0.98, y0 + s * 0.4, x0 + s * 0.98, y0 + s * 0.6, x0 + s * 0.78, y0 + s * 0.76);
                if let Some(w) = waves.finish() {
                    self.pm.stroke_path(&w, &p, &st, id, None);
                }
            }
            Icon::Volume(level) => {
                let mut cone = PathBuilder::new();
                cone.move_to(x0 + s * 0.08, y0 + s * 0.38);
                cone.line_to(x0 + s * 0.28, y0 + s * 0.38);
                cone.line_to(x0 + s * 0.5, y0 + s * 0.18);
                cone.line_to(x0 + s * 0.5, y0 + s * 0.82);
                cone.line_to(x0 + s * 0.28, y0 + s * 0.62);
                cone.line_to(x0 + s * 0.08, y0 + s * 0.62);
                cone.close();
                if let Some(c) = cone.finish() {
                    self.pm.fill_path(&c, &p, FillRule::Winding, id, None);
                }
                let mut waves = PathBuilder::new();
                if level >= 1 && level <= 3 {
                    waves.move_to(x0 + s * 0.62, y0 + s * 0.38);
                    waves.cubic_to(x0 + s * 0.72, y0 + s * 0.45, x0 + s * 0.72, y0 + s * 0.55, x0 + s * 0.62, y0 + s * 0.62);
                }
                if level >= 2 && level <= 3 {
                    waves.move_to(x0 + s * 0.7, y0 + s * 0.27);
                    waves.cubic_to(x0 + s * 0.86, y0 + s * 0.42, x0 + s * 0.86, y0 + s * 0.58, x0 + s * 0.7, y0 + s * 0.73);
                }
                if level == 3 {
                    waves.move_to(x0 + s * 0.78, y0 + s * 0.16);
                    waves.cubic_to(x0 + s * 1.0, y0 + s * 0.38, x0 + s * 1.0, y0 + s * 0.62, x0 + s * 0.78, y0 + s * 0.84);
                }
                if level == 0 {
                    // a cross where the waves would be: muted
                    waves.move_to(x0 + s * 0.64, y0 + s * 0.36);
                    waves.line_to(x0 + s * 0.94, y0 + s * 0.64);
                    waves.move_to(x0 + s * 0.94, y0 + s * 0.36);
                    waves.line_to(x0 + s * 0.64, y0 + s * 0.64);
                }
                if level >= 4 {
                    // a slash across the whole cone: there is no sound output
                    waves.move_to(x0 + s * 0.1, y0 + s * 0.1);
                    waves.line_to(x0 + s * 0.9, y0 + s * 0.9);
                }
                if let Some(w) = waves.finish() {
                    self.pm.stroke_path(&w, &p, &st, id, None);
                }
            }
            Icon::Lock => {
                if let Some(b) = rrect_path(x0 + s * 0.2, y0 + s * 0.44, s * 0.6, s * 0.46, s * 0.08) {
                    self.pm.fill_path(&b, &p, FillRule::Winding, id, None);
                }
                let mut sh = PathBuilder::new();
                sh.move_to(x0 + s * 0.32, y0 + s * 0.44);
                sh.line_to(x0 + s * 0.32, y0 + s * 0.30);
                sh.cubic_to(x0 + s * 0.32, y0 + s * 0.04, x0 + s * 0.68, y0 + s * 0.04, x0 + s * 0.68, y0 + s * 0.30);
                sh.line_to(x0 + s * 0.68, y0 + s * 0.44);
                if let Some(sh) = sh.finish() {
                    self.pm.stroke_path(&sh, &p, &st, id, None);
                }
            }
            Icon::Power => {
                let mut arc = PathBuilder::new();
                let r = s * 0.34;
                // ring open at the top
                arc.move_to(cx - r * 0.55, cy - r * 0.84);
                arc.cubic_to(cx - r * 1.5, cy - r * 0.2, cx - r * 1.1, cy + r * 1.1, cx, cy + r * 1.05);
                arc.cubic_to(cx + r * 1.1, cy + r * 1.1, cx + r * 1.5, cy - r * 0.2, cx + r * 0.55, cy - r * 0.84);
                if let Some(a) = arc.finish() {
                    self.pm.stroke_path(&a, &p, &st, id, None);
                }
                let mut l = PathBuilder::new();
                l.move_to(cx, cy - r * 1.15);
                l.line_to(cx, cy - r * 0.1);
                if let Some(l) = l.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
            }
            Icon::Devices => {
                // a screen on a stand, with a small plug-in dot at the lower right
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.14, s * 0.84, s * 0.50, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut stand = PathBuilder::new();
                stand.move_to(x0 + s * 0.5, y0 + s * 0.64);
                stand.line_to(x0 + s * 0.5, y0 + s * 0.80);
                stand.move_to(x0 + s * 0.28, y0 + s * 0.84);
                stand.line_to(x0 + s * 0.72, y0 + s * 0.84);
                if let Some(t) = stand.finish() {
                    self.pm.stroke_path(&t, &p, &st, id, None);
                }
                let mut dot = PathBuilder::new();
                dot.push_circle(x0 + s * 0.74, y0 + s * 0.47, s * 0.05);
                if let Some(d) = dot.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Camera => {
                // body with a small bump on top, and the lens
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.28, s * 0.84, s * 0.58, s * 0.1) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut top = PathBuilder::new();
                top.move_to(x0 + s * 0.32, y0 + s * 0.28);
                top.line_to(x0 + s * 0.38, y0 + s * 0.14);
                top.line_to(x0 + s * 0.62, y0 + s * 0.14);
                top.line_to(x0 + s * 0.68, y0 + s * 0.28);
                if let Some(t) = top.finish() {
                    self.pm.stroke_path(&t, &p, &st, id, None);
                }
                let mut lens = PathBuilder::new();
                lens.push_circle(x0 + s * 0.5, y0 + s * 0.57, s * 0.17);
                if let Some(l) = lens.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
            }
            Icon::Skills => {
                // a spark: a four-pointed star and a small one beside it
                let mut star = PathBuilder::new();
                let (cx0, cy0, big, waist) = (x0 + s * 0.42, y0 + s * 0.52, s * 0.38, s * 0.1);
                star.move_to(cx0, cy0 - big);
                star.line_to(cx0 + waist, cy0 - waist);
                star.line_to(cx0 + big, cy0);
                star.line_to(cx0 + waist, cy0 + waist);
                star.line_to(cx0, cy0 + big);
                star.line_to(cx0 - waist, cy0 + waist);
                star.line_to(cx0 - big, cy0);
                star.line_to(cx0 - waist, cy0 - waist);
                star.close();
                if let Some(st2) = star.finish() {
                    self.pm.stroke_path(&st2, &p, &st, id, None);
                }
                let mut small = PathBuilder::new();
                small.push_circle(x0 + s * 0.8, y0 + s * 0.2, s * 0.06);
                small.push_circle(x0 + s * 0.8, y0 + s * 0.8, s * 0.045);
                if let Some(sm) = small.finish() {
                    self.pm.fill_path(&sm, &p, FillRule::Winding, id, None);
                }
            }
            Icon::System => {
                // a gauge: a ring open at the bottom, tick marks and a needle
                let mut ring = PathBuilder::new();
                let (cx, cy, r) = (x0 + s * 0.5, y0 + s * 0.56, s * 0.38);
                for i in 0..=24 {
                    let a = (150.0 + 240.0 * i as f32 / 24.0).to_radians();
                    let (px, py) = (cx + r * a.cos(), cy + r * a.sin());
                    if i == 0 { ring.move_to(px, py) } else { ring.line_to(px, py) }
                }
                if let Some(rp) = ring.finish() {
                    self.pm.stroke_path(&rp, &p, &st, id, None);
                }
                let mut ticks = PathBuilder::new();
                for i in 0..=6 {
                    let a = (150.0 + 240.0 * i as f32 / 6.0).to_radians();
                    ticks.move_to(cx + r * 0.78 * a.cos(), cy + r * 0.78 * a.sin());
                    ticks.line_to(cx + r * 0.62 * a.cos(), cy + r * 0.62 * a.sin());
                }
                if let Some(tp) = ticks.finish() {
                    self.pm.stroke_path(&tp, &p, &st, id, None);
                }
                let mut needle = PathBuilder::new();
                let a = 320.0f32.to_radians();
                needle.move_to(cx, cy);
                needle.line_to(cx + r * 0.6 * a.cos(), cy + r * 0.6 * a.sin());
                if let Some(np) = needle.finish() {
                    self.pm.stroke_path(&np, &p, &st, id, None);
                }
                let mut hub = PathBuilder::new();
                hub.push_circle(cx, cy, s * 0.06);
                if let Some(h) = hub.finish() {
                    self.pm.fill_path(&h, &p, FillRule::Winding, id, None);
                }
            }
            Icon::SrePilot => {
                // a radar: two rings, cross hairs, a sweep line and a blip (the pilot watching the site)
                let (cx, cy, r) = (x0 + s * 0.5, y0 + s * 0.5, s * 0.40);
                self.pm.stroke_path(&PathBuilder::from_circle(cx, cy, r).unwrap(), &p, &st, id, None);
                self.pm.stroke_path(&PathBuilder::from_circle(cx, cy, r * 0.5).unwrap(), &p, &st, id, None);
                let mut cross = PathBuilder::new();
                for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    cross.move_to(cx + dx * r * 0.82, cy + dy * r * 0.82);
                    cross.line_to(cx + dx * r * 1.18, cy + dy * r * 1.18);
                }
                if let Some(cp) = cross.finish() {
                    self.pm.stroke_path(&cp, &p, &st, id, None);
                }
                let a = (-48.0f32).to_radians();
                let mut sweep = PathBuilder::new();
                sweep.move_to(cx, cy);
                sweep.line_to(cx + r * a.cos(), cy + r * a.sin());
                if let Some(sp) = sweep.finish() {
                    self.pm.stroke_path(&sp, &p, &st, id, None);
                }
                let mut wedge = PathBuilder::new();
                wedge.move_to(cx, cy);
                for i in 0..=6 {
                    let b = (-48.0 - 34.0 * i as f32 / 6.0).to_radians();
                    wedge.line_to(cx + r * b.cos(), cy + r * b.sin());
                }
                wedge.close();
                if let Some(wp) = wedge.finish() {
                    self.pm.fill_path(&wp, &paint([c[0], c[1], c[2], (c[3] as f32 * 0.28) as u8]), FillRule::Winding, id, None);
                }
                self.circle(cx + r * 0.62, cy + r * 0.12, s * 0.07, c);
                self.circle(cx - r * 0.2, cy + r * 0.45, s * 0.045, c);
            }
            Icon::Ide => {
                // a window with a sidebar and the code chevrons </>, and a small spark (the assistant) at its corner
                if let Some(b) = rrect_path(x0 + s * 0.06, y0 + s * 0.14, s * 0.88, s * 0.7, s * 0.09) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut bars = PathBuilder::new();
                bars.move_to(x0 + s * 0.06, y0 + s * 0.3);
                bars.line_to(x0 + s * 0.94, y0 + s * 0.3);
                bars.move_to(x0 + s * 0.3, y0 + s * 0.3);
                bars.line_to(x0 + s * 0.3, y0 + s * 0.84);
                if let Some(b) = bars.finish() {
                    self.pm.stroke_path(&b, &p, &stroke(lw * 0.8), id, None);
                }
                let mut ch = PathBuilder::new();
                ch.move_to(x0 + s * 0.5, y0 + s * 0.45);
                ch.line_to(x0 + s * 0.42, y0 + s * 0.57);
                ch.line_to(x0 + s * 0.5, y0 + s * 0.69);
                ch.move_to(x0 + s * 0.76, y0 + s * 0.45);
                ch.line_to(x0 + s * 0.84, y0 + s * 0.57);
                ch.line_to(x0 + s * 0.76, y0 + s * 0.69);
                ch.move_to(x0 + s * 0.67, y0 + s * 0.43);
                ch.line_to(x0 + s * 0.59, y0 + s * 0.71);
                if let Some(c2) = ch.finish() {
                    self.pm.stroke_path(&c2, &p, &st, id, None);
                }
                // the spark: a four-point star
                let (sx, sy, r) = (x0 + s * 0.86, y0 + s * 0.14, s * 0.13);
                let mut star = PathBuilder::new();
                star.move_to(sx, sy - r);
                star.line_to(sx + r * 0.3, sy - r * 0.3);
                star.line_to(sx + r, sy);
                star.line_to(sx + r * 0.3, sy + r * 0.3);
                star.line_to(sx, sy + r);
                star.line_to(sx - r * 0.3, sy + r * 0.3);
                star.line_to(sx - r, sy);
                star.line_to(sx - r * 0.3, sy - r * 0.3);
                star.close();
                if let Some(st2) = star.finish() {
                    self.pm.fill_path(&st2, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Media => {
                // a screen with a play triangle, and a base line
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.14, s * 0.84, s * 0.62, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut tri = PathBuilder::new();
                tri.move_to(x0 + s * 0.42, y0 + s * 0.28);
                tri.line_to(x0 + s * 0.42, y0 + s * 0.62);
                tri.line_to(x0 + s * 0.68, y0 + s * 0.45);
                tri.close();
                if let Some(t) = tri.finish() {
                    self.pm.fill_path(&t, &p, FillRule::Winding, id, None);
                }
                let mut base = PathBuilder::new();
                base.move_to(x0 + s * 0.28, y0 + s * 0.88);
                base.line_to(x0 + s * 0.72, y0 + s * 0.88);
                if let Some(b) = base.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
            }
            Icon::Tales => {
                // an open book with a few lines of text on its pages
                let mut b = PathBuilder::new();
                b.move_to(x0 + s * 0.5, y0 + s * 0.26);
                b.line_to(x0 + s * 0.1, y0 + s * 0.18);
                b.line_to(x0 + s * 0.1, y0 + s * 0.78);
                b.line_to(x0 + s * 0.5, y0 + s * 0.86);
                b.line_to(x0 + s * 0.9, y0 + s * 0.78);
                b.line_to(x0 + s * 0.9, y0 + s * 0.18);
                b.line_to(x0 + s * 0.5, y0 + s * 0.26);
                b.line_to(x0 + s * 0.5, y0 + s * 0.86);
                if let Some(path) = b.finish() {
                    self.pm.stroke_path(&path, &p, &st, id, None);
                }
                let mut t = PathBuilder::new();
                for k in 0..2 {
                    let dy = s * 0.15 * k as f32;
                    t.move_to(x0 + s * 0.2, y0 + s * (0.36 + 0.0) + dy);
                    t.line_to(x0 + s * 0.4, y0 + s * 0.4 + dy);
                    t.move_to(x0 + s * 0.6, y0 + s * 0.4 + dy);
                    t.line_to(x0 + s * 0.8, y0 + s * 0.36 + dy);
                }
                if let Some(path) = t.finish() {
                    self.pm.stroke_path(&path, &p, &st, id, None);
                }
            }
            Icon::Dash => {
                // a dashboard: a frame, a rising line and two bars
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.14, s * 0.84, s * 0.72, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut line = PathBuilder::new();
                line.move_to(x0 + s * 0.16, y0 + s * 0.62);
                line.line_to(x0 + s * 0.34, y0 + s * 0.46);
                line.line_to(x0 + s * 0.5, y0 + s * 0.54);
                line.line_to(x0 + s * 0.7, y0 + s * 0.3);
                if let Some(l) = line.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                for (bx, bh) in [(0.62, 0.16), (0.76, 0.28)] {
                    let mut bar = PathBuilder::new();
                    bar.push_rect(tiny_skia::Rect::from_xywh(x0 + s * bx, y0 + s * (0.8 - bh), s * 0.08, s * bh - s * 0.02).unwrap_or(tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap()));
                    if let Some(b) = bar.finish() {
                        self.pm.fill_path(&b, &p, FillRule::Winding, id, None);
                    }
                }
            }
            Icon::Flows => {
                // a workflow: a step on the left fanning out to two steps on the right, joined by curves with arrowheads
                for (bx, by) in [(0.06, 0.38), (0.68, 0.10), (0.68, 0.66)] {
                    if let Some(b) = rrect_path(x0 + s * bx, y0 + s * by, s * 0.26, s * 0.24, s * 0.06) {
                        self.pm.stroke_path(&b, &p, &st, id, None);
                    }
                }
                for ty in [0.22, 0.78] {
                    let mut curve = PathBuilder::new();
                    curve.move_to(x0 + s * 0.32, y0 + s * 0.50);
                    curve.cubic_to(x0 + s * 0.50, y0 + s * 0.50, x0 + s * 0.50, y0 + s * ty, x0 + s * 0.66, y0 + s * ty);
                    if let Some(l) = curve.finish() {
                        self.pm.stroke_path(&l, &p, &st, id, None);
                    }
                    let mut head = PathBuilder::new();
                    head.move_to(x0 + s * 0.60, y0 + s * (ty - 0.05));
                    head.line_to(x0 + s * 0.67, y0 + s * ty);
                    head.line_to(x0 + s * 0.60, y0 + s * (ty + 0.05));
                    if let Some(h) = head.finish() {
                        self.pm.stroke_path(&h, &p, &st, id, None);
                    }
                }
                let mut dot = PathBuilder::new();
                dot.push_circle(x0 + s * 0.19, y0 + s * 0.50, s * 0.04);
                if let Some(d) = dot.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Store => {
                // a shopping bag: the body, the handle and a small tag
                if let Some(b) = rrect_path(x0 + s * 0.14, y0 + s * 0.3, s * 0.72, s * 0.6, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut handle = PathBuilder::new();
                handle.move_to(x0 + s * 0.34, y0 + s * 0.38);
                handle.cubic_to(x0 + s * 0.34, y0 + s * 0.04, x0 + s * 0.66, y0 + s * 0.04, x0 + s * 0.66, y0 + s * 0.38);
                if let Some(h) = handle.finish() {
                    self.pm.stroke_path(&h, &p, &st, id, None);
                }
                let mut dot = PathBuilder::new();
                dot.push_circle(x0 + s * 0.5, y0 + s * 0.62, s * 0.07);
                if let Some(d) = dot.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Data => {
                // a table: a frame, a header band and the grid lines
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.14, s * 0.84, s * 0.72, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut grid = PathBuilder::new();
                grid.move_to(x0 + s * 0.08, y0 + s * 0.38);
                grid.line_to(x0 + s * 0.92, y0 + s * 0.38);
                grid.move_to(x0 + s * 0.08, y0 + s * 0.62);
                grid.line_to(x0 + s * 0.92, y0 + s * 0.62);
                grid.move_to(x0 + s * 0.38, y0 + s * 0.14);
                grid.line_to(x0 + s * 0.38, y0 + s * 0.86);
                grid.move_to(x0 + s * 0.65, y0 + s * 0.14);
                grid.line_to(x0 + s * 0.65, y0 + s * 0.86);
                if let Some(gp) = grid.finish() {
                    self.pm.stroke_path(&gp, &p, &st, id, None);
                }
                let mut head = PathBuilder::new();
                head.push_rect(tiny_skia::Rect::from_xywh(x0 + s * 0.1, y0 + s * 0.16, s * 0.8, s * 0.2).unwrap_or(tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap()));
                if let Some(h) = head.finish() {
                    self.pm.fill_path(&h, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Usage => {
                // a bar chart on an axis
                let mut axes = PathBuilder::new();
                axes.move_to(x0 + s * 0.1, y0 + s * 0.08);
                axes.line_to(x0 + s * 0.1, y0 + s * 0.9);
                axes.line_to(x0 + s * 0.94, y0 + s * 0.9);
                if let Some(a) = axes.finish() {
                    self.pm.stroke_path(&a, &p, &st, id, None);
                }
                let mut bars = PathBuilder::new();
                for (i, h) in [0.3f32, 0.55, 0.42, 0.74].iter().enumerate() {
                    let bx = x0 + s * (0.22 + i as f32 * 0.19);
                    bars.push_rect(tiny_skia::Rect::from_xywh(bx, y0 + s * (0.84 - h), s * 0.11, s * h).unwrap_or(tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap()));
                }
                if let Some(b) = bars.finish() {
                    self.pm.fill_path(&b, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Bell => {
                // a bell: a dome that flares at the rim, a clapper below and a small knob on top
                let mut bell = PathBuilder::new();
                bell.move_to(x0 + s * 0.14, y0 + s * 0.74);
                bell.line_to(x0 + s * 0.22, y0 + s * 0.66);
                bell.cubic_to(x0 + s * 0.28, y0 + s * 0.58, x0 + s * 0.26, y0 + s * 0.46, x0 + s * 0.28, y0 + s * 0.38);
                bell.cubic_to(x0 + s * 0.31, y0 + s * 0.2, x0 + s * 0.69, y0 + s * 0.2, x0 + s * 0.72, y0 + s * 0.38);
                bell.cubic_to(x0 + s * 0.74, y0 + s * 0.46, x0 + s * 0.72, y0 + s * 0.58, x0 + s * 0.78, y0 + s * 0.66);
                bell.line_to(x0 + s * 0.86, y0 + s * 0.74);
                bell.close();
                if let Some(b) = bell.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut parts = PathBuilder::new();
                parts.move_to(x0 + s * 0.5, y0 + s * 0.12);
                parts.line_to(x0 + s * 0.5, y0 + s * 0.2);
                if let Some(b) = parts.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut clap = PathBuilder::new();
                clap.push_circle(x0 + s * 0.5, y0 + s * 0.84, s * 0.07);
                if let Some(b) = clap.finish() {
                    self.pm.fill_path(&b, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Calendar => {
                // a calendar page with binding rings, a header bar and a grid of days
                let r = tiny_skia::Rect::from_xywh(x0 + s * 0.1, y0 + s * 0.18, s * 0.8, s * 0.72);
                if let Some(r) = r {
                    let mut pb = PathBuilder::new();
                    pb.push_rect(r);
                    if let Some(p2) = pb.finish() {
                        self.pm.stroke_path(&p2, &p, &st, id, None);
                    }
                }
                let mut bar = PathBuilder::new();
                bar.move_to(x0 + s * 0.1, y0 + s * 0.38);
                bar.line_to(x0 + s * 0.9, y0 + s * 0.38);
                bar.move_to(x0 + s * 0.3, y0 + s * 0.08);
                bar.line_to(x0 + s * 0.3, y0 + s * 0.26);
                bar.move_to(x0 + s * 0.7, y0 + s * 0.08);
                bar.line_to(x0 + s * 0.7, y0 + s * 0.26);
                if let Some(b) = bar.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut dots = PathBuilder::new();
                for row in 0..2 {
                    for col in 0..3 {
                        dots.push_circle(x0 + s * (0.28 + col as f32 * 0.22), y0 + s * (0.52 + row as f32 * 0.17), s * 0.035);
                    }
                }
                if let Some(d) = dots.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Scan => {
                // a radar: rings, a sweep and two blips
                for r in [0.42f32, 0.26] {
                    let mut ring = PathBuilder::new();
                    ring.push_circle(x0 + s * 0.5, y0 + s * 0.5, s * r);
                    if let Some(rg) = ring.finish() {
                        self.pm.stroke_path(&rg, &p, &st, id, None);
                    }
                }
                let mut sweep = PathBuilder::new();
                sweep.move_to(x0 + s * 0.5, y0 + s * 0.5);
                sweep.line_to(x0 + s * 0.82, y0 + s * 0.24);
                if let Some(sw) = sweep.finish() {
                    self.pm.stroke_path(&sw, &p, &st, id, None);
                }
                for (px, py) in [(0.68f32, 0.62f32), (0.34, 0.32)] {
                    let mut dot = PathBuilder::new();
                    dot.push_circle(x0 + s * px, y0 + s * py, s * 0.05);
                    if let Some(d) = dot.finish() {
                        self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                    }
                }
            }
            Icon::Radio => {
                // a radio: body, aerial, dial and speaker dots
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.34, s * 0.84, s * 0.54, s * 0.1) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut aerial = PathBuilder::new();
                aerial.move_to(x0 + s * 0.30, y0 + s * 0.34);
                aerial.line_to(x0 + s * 0.74, y0 + s * 0.10);
                if let Some(a) = aerial.finish() {
                    self.pm.stroke_path(&a, &p, &st, id, None);
                }
                let mut dial = PathBuilder::new();
                dial.push_circle(x0 + s * 0.68, y0 + s * 0.61, s * 0.12);
                if let Some(d) = dial.finish() {
                    self.pm.stroke_path(&d, &p, &st, id, None);
                }
                for k in 0..3 {
                    let mut dot = PathBuilder::new();
                    dot.push_circle(x0 + s * 0.24, y0 + s * (0.50 + 0.1 * k as f32), s * 0.035);
                    if let Some(d) = dot.finish() {
                        self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                    }
                }
            }
            Icon::Disks => {
                // a drive: a flat body, a slot and its light
                if let Some(b) = rrect_path(x0 + s * 0.08, y0 + s * 0.30, s * 0.84, s * 0.44, s * 0.1) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut slot = PathBuilder::new();
                slot.move_to(x0 + s * 0.2, y0 + s * 0.52);
                slot.line_to(x0 + s * 0.56, y0 + s * 0.52);
                if let Some(l) = slot.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                let mut led = PathBuilder::new();
                led.push_circle(x0 + s * 0.76, y0 + s * 0.52, s * 0.06);
                if let Some(d) = led.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Lab => {
                // a flask: a narrow neck, a wide body, the liquid line and a bubble
                let mut f = PathBuilder::new();
                f.move_to(x0 + s * 0.38, y0 + s * 0.1);
                f.line_to(x0 + s * 0.38, y0 + s * 0.4);
                f.line_to(x0 + s * 0.12, y0 + s * 0.84);
                f.line_to(x0 + s * 0.88, y0 + s * 0.84);
                f.line_to(x0 + s * 0.62, y0 + s * 0.4);
                f.line_to(x0 + s * 0.62, y0 + s * 0.1);
                f.close();
                if let Some(b) = f.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut line = PathBuilder::new();
                line.move_to(x0 + s * 0.28, y0 + s * 0.62);
                line.line_to(x0 + s * 0.72, y0 + s * 0.62);
                if let Some(l) = line.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                let mut dot = PathBuilder::new();
                dot.push_circle(x0 + s * 0.5, y0 + s * 0.74, s * 0.05);
                if let Some(d) = dot.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                }
            }
            Icon::Doctor => {
                // a first-aid case: a rounded body with a handle and a plus
                if let Some(b) = rrect_path(x0 + s * 0.1, y0 + s * 0.28, s * 0.8, s * 0.58, s * 0.1) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut h = PathBuilder::new();
                h.move_to(x0 + s * 0.36, y0 + s * 0.28);
                h.line_to(x0 + s * 0.36, y0 + s * 0.16);
                h.line_to(x0 + s * 0.64, y0 + s * 0.16);
                h.line_to(x0 + s * 0.64, y0 + s * 0.28);
                let mut plus = PathBuilder::new();
                plus.move_to(x0 + s * 0.5, y0 + s * 0.44);
                plus.line_to(x0 + s * 0.5, y0 + s * 0.7);
                plus.move_to(x0 + s * 0.37, y0 + s * 0.57);
                plus.line_to(x0 + s * 0.63, y0 + s * 0.57);
                for b in [h.finish(), plus.finish()].into_iter().flatten() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
            }
            Icon::Learn => {
                // an open book: two pages meeting at the spine, with a line of text on each
                let mut left = PathBuilder::new();
                left.move_to(x0 + s * 0.5, y0 + s * 0.28);
                left.line_to(x0 + s * 0.14, y0 + s * 0.2);
                left.line_to(x0 + s * 0.14, y0 + s * 0.78);
                left.line_to(x0 + s * 0.5, y0 + s * 0.86);
                left.close();
                let mut right = PathBuilder::new();
                right.move_to(x0 + s * 0.5, y0 + s * 0.28);
                right.line_to(x0 + s * 0.86, y0 + s * 0.2);
                right.line_to(x0 + s * 0.86, y0 + s * 0.78);
                right.line_to(x0 + s * 0.5, y0 + s * 0.86);
                right.close();
                let mut lines = PathBuilder::new();
                lines.move_to(x0 + s * 0.24, y0 + s * 0.42);
                lines.line_to(x0 + s * 0.42, y0 + s * 0.47);
                lines.move_to(x0 + s * 0.24, y0 + s * 0.58);
                lines.line_to(x0 + s * 0.42, y0 + s * 0.63);
                lines.move_to(x0 + s * 0.76, y0 + s * 0.42);
                lines.line_to(x0 + s * 0.58, y0 + s * 0.47);
                for b in [left.finish(), right.finish(), lines.finish()].into_iter().flatten() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
            }
            Icon::Checkup => {
                // a clipboard: a rounded board with a clip, a tick and two lines
                if let Some(b) = rrect_path(x0 + s * 0.2, y0 + s * 0.18, s * 0.6, s * 0.72, s * 0.08) {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                let mut clip = PathBuilder::new();
                clip.move_to(x0 + s * 0.4, y0 + s * 0.18);
                clip.line_to(x0 + s * 0.4, y0 + s * 0.1);
                clip.line_to(x0 + s * 0.6, y0 + s * 0.1);
                clip.line_to(x0 + s * 0.6, y0 + s * 0.18);
                let mut tick = PathBuilder::new();
                tick.move_to(x0 + s * 0.32, y0 + s * 0.45);
                tick.line_to(x0 + s * 0.4, y0 + s * 0.53);
                tick.line_to(x0 + s * 0.54, y0 + s * 0.36);
                let mut lines = PathBuilder::new();
                lines.move_to(x0 + s * 0.32, y0 + s * 0.68);
                lines.line_to(x0 + s * 0.68, y0 + s * 0.68);
                lines.move_to(x0 + s * 0.32, y0 + s * 0.8);
                lines.line_to(x0 + s * 0.56, y0 + s * 0.8);
                for b in [clip.finish(), tick.finish(), lines.finish()].into_iter().flatten() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
            }
            Icon::Git => {
                // a branch: a trunk with a side branch, a node at each end
                let mut t = PathBuilder::new();
                t.move_to(x0 + s * 0.3, y0 + s * 0.2);
                t.line_to(x0 + s * 0.3, y0 + s * 0.8);
                t.move_to(x0 + s * 0.3, y0 + s * 0.6);
                t.line_to(x0 + s * 0.3, y0 + s * 0.52);
                t.cubic_to(x0 + s * 0.3, y0 + s * 0.38, x0 + s * 0.7, y0 + s * 0.46, x0 + s * 0.7, y0 + s * 0.3);
                if let Some(b) = t.finish() {
                    self.pm.stroke_path(&b, &p, &st, id, None);
                }
                for (nx, ny) in [(0.3, 0.2), (0.3, 0.8), (0.7, 0.28)] {
                    let mut d = PathBuilder::new();
                    d.push_circle(x0 + s * nx, y0 + s * ny, s * 0.08);
                    if let Some(d) = d.finish() {
                        self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                    }
                }
            }
            Icon::Mesh => {
                // three linked nodes
                let pts = [(0.5, 0.18), (0.16, 0.78), (0.84, 0.78)];
                let mut links = PathBuilder::new();
                links.move_to(x0 + s * pts[0].0, y0 + s * pts[0].1);
                links.line_to(x0 + s * pts[1].0, y0 + s * pts[1].1);
                links.line_to(x0 + s * pts[2].0, y0 + s * pts[2].1);
                links.close();
                if let Some(l) = links.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                for (px, py) in pts {
                    let mut dot = PathBuilder::new();
                    dot.push_circle(x0 + s * px, y0 + s * py, s * 0.1);
                    if let Some(d) = dot.finish() {
                        self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
                    }
                }
            }
            Icon::Home => {
                let mut house = PathBuilder::new();
                house.move_to(x0 + s * 0.10, y0 + s * 0.50);
                house.line_to(x0 + s * 0.50, y0 + s * 0.14);
                house.line_to(x0 + s * 0.90, y0 + s * 0.50);
                house.move_to(x0 + s * 0.22, y0 + s * 0.44);
                house.line_to(x0 + s * 0.22, y0 + s * 0.86);
                house.line_to(x0 + s * 0.78, y0 + s * 0.86);
                house.line_to(x0 + s * 0.78, y0 + s * 0.44);
                house.move_to(x0 + s * 0.42, y0 + s * 0.86);
                house.line_to(x0 + s * 0.42, y0 + s * 0.60);
                house.line_to(x0 + s * 0.58, y0 + s * 0.60);
                house.line_to(x0 + s * 0.58, y0 + s * 0.86);
                if let Some(h) = house.finish() {
                    self.pm.stroke_path(&h, &p, &st, id, None);
                }
            }
            Icon::Logout => {
                let mut door = PathBuilder::new();
                door.move_to(x0 + s * 0.42, y0 + s * 0.15);
                door.line_to(x0 + s * 0.15, y0 + s * 0.15);
                door.line_to(x0 + s * 0.15, y0 + s * 0.85);
                door.line_to(x0 + s * 0.42, y0 + s * 0.85);
                door.move_to(x0 + s * 0.35, y0 + s * 0.5);
                door.line_to(x0 + s * 0.88, y0 + s * 0.5);
                door.move_to(x0 + s * 0.70, y0 + s * 0.32);
                door.line_to(x0 + s * 0.88, y0 + s * 0.5);
                door.line_to(x0 + s * 0.70, y0 + s * 0.68);
                if let Some(d) = door.finish() {
                    self.pm.stroke_path(&d, &p, &st, id, None);
                }
            }
        }
    }

    /// RGBA8 (premultiplied) bytes.
    pub fn rgba(&self) -> &[u8] {
        self.pm.data()
    }

    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        self.pm.save_png(path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(g: &Gfx, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * g.width() + x) * 4) as usize;
        let d = g.rgba();
        [d[i], d[i + 1], d[i + 2], d[i + 3]]
    }

    fn nonbg(g: &Gfx, bg: [u8; 4]) -> usize {
        g.rgba().chunks(4).filter(|p| p != &bg).count()
    }

    #[test]
    fn shapes_and_text_draw() {
        let mut g = Gfx::new(200, 100);
        g.clear(rgb(10, 10, 10));
        g.rrect(10.0, 10.0, 80.0, 40.0, 8.0, rgb(200, 0, 0));
        assert_eq!(px(&g, 50, 30), rgb(200, 0, 0));
        assert_eq!(px(&g, 150, 80), rgb(10, 10, 10));
        let before = nonbg(&g, rgb(10, 10, 10));
        let w = g.text(10.0, 60.0, 20.0, rgb(255, 255, 255), "Hello", Face::Sans);
        assert!(w > 30.0 && w < 120.0);
        assert!(nonbg(&g, rgb(10, 10, 10)) > before + 50);
    }

    #[test]
    fn every_icon_draws_something() {
        for icon in [Icon::Chat, Icon::Rag, Icon::Mcp, Icon::Cli, Icon::Users, Icon::Settings, Icon::Clipboard, Icon::Mic, Icon::Speaker, Icon::Runs, Icon::Edit, Icon::Browser, Icon::Camera, Icon::Mesh, Icon::Radio, Icon::Scan, Icon::Skills, Icon::Usage, Icon::Calendar, Icon::System, Icon::Media, Icon::SrePilot, Icon::Ide, Icon::Data, Icon::Tales, Icon::Bell, Icon::Lock, Icon::Power, Icon::Logout, Icon::Store, Icon::Dash, Icon::Flows, Icon::Devices, Icon::Disks, Icon::Lab, Icon::Git, Icon::Doctor, Icon::Checkup, Icon::Learn, Icon::Volume(0), Icon::Volume(1), Icon::Volume(2), Icon::Volume(3), Icon::Volume(4), Icon::Home] {
            let mut g = Gfx::new(96, 96);
            g.clear(rgb(0, 0, 0));
            g.icon(icon, 48.0, 48.0, 80.0, rgb(255, 255, 255));
            assert!(nonbg(&g, rgb(0, 0, 0)) > 100, "{icon:?} drew nothing");
            // stays inside its box (no ink in the outer 4px margin of a 96px canvas)
            for (x, y) in [(1, 1), (94, 1), (1, 94), (94, 94)] {
                assert_eq!(px(&g, x, y), rgb(0, 0, 0), "{icon:?} overflowed");
            }
        }
    }

    #[test]
    fn wrap_and_fit() {
        let mut g = Gfx::new(10, 10);
        let lines = g.wrap(16.0, "the quick brown fox jumps over the lazy dog", Face::Sans, 120.0);
        assert!(lines.len() >= 3);
        assert!(lines.iter().all(|l| g.text_width(16.0, l, Face::Sans) <= 120.5));
        let long = g.wrap(16.0, &"w".repeat(80), Face::Sans, 100.0);
        assert!(long.len() > 3 && long.iter().all(|l| g.text_width(16.0, l, Face::Sans) <= 100.5));
        assert_eq!(g.wrap(16.0, "a\n\nb", Face::Sans, 100.0), vec!["a", "", "b"]);
        let f = g.fit(16.0, "a very long label that cannot fit", Face::Sans, 80.0);
        assert!(f.ends_with('…') && g.text_width(16.0, &f, Face::Sans) <= 80.5);
        assert_eq!(g.fit(16.0, "ok", Face::Sans, 80.0), "ok");
    }

    #[test]
    fn text_clips_at_edges_without_panicking() {
        let mut g = Gfx::new(40, 20);
        g.clear(rgb(0, 0, 0));
        g.text(-30.0, -5.0, 24.0, rgb(255, 255, 255), "clipped text far wider than the canvas", Face::Mono);
        g.text(30.0, 10.0, 24.0, rgb(255, 255, 255), "x", Face::Bold);
    }

    fn ink(g: &Gfx) -> usize {
        g.pm.data().chunks(4).filter(|p| p[0] > 40).count()
    }

    #[test]
    fn hindi_tamil_and_arabic_are_drawn_with_real_letters_not_empty_boxes() {
        for text in ["नमस्ते दुनिया", "தமிழ் மொழி", "مرحبا بالعالم", "বাংলা"] {
            let mut g = Gfx::new(300, 40);
            g.clear(rgb(0, 0, 0));
            let w = g.text(4.0, 6.0, 22.0, rgb(255, 255, 255), text, Face::Sans);
            assert!(w > 30.0 && w < 290.0, "{text}: width {w}");
            assert_eq!(w, g.text_width(22.0, text, Face::Sans), "measuring agrees with drawing");
            let n = ink(&g);
            assert!(n > 150, "{text}: only {n} lit pixels");
            // a box for a missing letter is a hollow rectangle of equal ink for every letter; shaped text varies by column
            let cols: std::collections::HashSet<usize> = (0..300usize).map(|x| (0..40usize).filter(|&y| g.pm.data()[(y * 300 + x) * 4] > 40).count()).collect();
            assert!(cols.len() > 6, "{text}: columns look uniform");
        }
    }

    #[test]
    fn english_text_is_drawn_the_way_it_was() {
        let mut g = Gfx::new(200, 30);
        g.clear(rgb(0, 0, 0));
        let w = g.text(2.0, 4.0, 18.0, rgb(255, 255, 255), "Hello world", Face::Sans);
        assert!(g.shaper.is_none(), "plain English never loads the script fonts");
        assert!(w > 60.0 && ink(&g) > 100);
    }

    #[test]
    fn clipped_puts_back_everything_outside_the_rectangle() {
        let mut g = Gfx::new(40, 30);
        g.clear(rgb(10, 20, 30));
        g.clipped(R::new(10.0, 8.0, 12.0, 10.0), |g| g.rect(0.0, 0.0, 40.0, 30.0, rgb(250, 0, 0)));
        assert_eq!(px(&g, 15, 12)[0], 250, "inside the clip the drawing stays");
        for (x, y) in [(0, 0), (9, 12), (22, 12), (15, 7), (15, 18), (39, 29)] {
            assert_eq!(px(&g, x, y), [10, 20, 30, 255], "({x}, {y}) is outside");
        }
        // nested: the inner clip is inside the outer one
        g.clipped(R::new(5.0, 5.0, 30.0, 20.0), |g| g.clipped(R::new(10.0, 8.0, 5.0, 5.0), |g| g.rect(0.0, 0.0, 40.0, 30.0, rgb(0, 250, 0))));
        assert_eq!(px(&g, 12, 10)[1], 250);
        assert_eq!(px(&g, 20, 10)[1], 0, "inside the outer clip but outside the inner one: back to the red of before");
    }

    #[test]
    fn clipped_near_restores_a_ring_around_the_rectangle_and_leaves_the_rest_alone() {
        let mut g = Gfx::new(60, 40);
        g.clear(rgb(10, 20, 30));
        g.clipped_near(R::new(20.0, 15.0, 10.0, 8.0), 4.0, |g| g.rect(0.0, 0.0, 60.0, 40.0, rgb(250, 0, 0)));
        assert_eq!(px(&g, 25, 18)[0], 250, "inside");
        for (x, y) in [(19, 18), (30, 18), (25, 14), (25, 23), (16, 11), (33, 26)] {
            assert_eq!(px(&g, x, y), [10, 20, 30, 255], "({x}, {y}) is in the ring");
        }
        assert_eq!(px(&g, 0, 0)[0], 250, "farther than the margin nothing is put back: the caller promised to draw nothing there");
    }

    #[test]
    fn a_region_can_be_grabbed_put_back_and_drawn_in_on_its_own() {
        let mut g = Gfx::new(50, 30);
        g.clear(rgb(1, 2, 3));
        let before = g.grab((10, 5, 20, 10));
        assert_eq!(before.len(), 20 * 10 * 4);
        g.in_region((10, 5, 20, 10), None, |g, ox, oy| g.rect(12.0 - ox, 7.0 - oy, 4.0, 3.0, rgb(200, 100, 50)));
        assert_eq!(px(&g, 13, 8), [200, 100, 50, 255]);
        assert_eq!(px(&g, 9, 8), [1, 2, 3, 255], "outside the region");
        assert_eq!(px(&g, 12, 6), [1, 2, 3, 255]);
        g.put((10, 5, 20, 10), &before);
        assert_eq!(px(&g, 13, 8), [1, 2, 3, 255], "put back");
        // seeded with other pixels
        let seed = vec![9u8; 20 * 10 * 4];
        g.in_region((10, 5, 20, 10), Some(&seed), |_, _, _| {});
        assert_eq!(px(&g, 11, 6), [9, 9, 9, 9]);
        // a rectangle partly off the screen is cut to it
        g.in_region((40, 20, 30, 30), None, |g, ox, oy| g.rect(45.0 - ox, 25.0 - oy, 3.0, 3.0, rgb(7, 7, 7)));
        assert_eq!(px(&g, 46, 26)[0], 7);
    }
}
