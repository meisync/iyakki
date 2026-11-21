//! Software 2D drawing on a `tiny-skia` pixmap: rectangles, rounded cards, anti-aliased vector
//! icons, and TrueType text (DejaVu, bundled). No GPU, window system or Mesa needed, so the same
//! code draws to a Linux framebuffer on bare metal, to a PNG for tests, or into any window.

use crate::safeclamp::SafeClamp;
use std::collections::HashMap;

use fontdue::Font;

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
    /// The K8S tile: a ship's wheel (a rim, a hub and seven spokes).
    K8s,
    Radio,
    Scan,
    Skills,
    Usage,
    Calendar,
    System,
    Media,
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

/// A line of text as coverage (same fields as `iyakki_web::svg::TextMask`, which the shell converts it to): `alpha` is `w * h` bytes,
/// `x` and `y` are the mask's corner relative to the pen on the baseline, `advance` is how far the pen moved.
#[derive(Clone, Debug, Default)]
pub struct TextMask {
    pub advance: f64,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
}


/// Write a drawing call down instead of drawing it while the recording layer is on (see `par`).
macro_rules! rec_leaf {
    ($self:ident, $y0:expr, $y1:expr, |$g:ident| $body:expr) => {
        if $self.rec.is_some() {
            $self.rec_push($y0, $y1, move |$g: &mut Gfx| {
                $body;
            });
            return;
        }
    };
}

mod par;
pub use par::BandPool;

pub struct Gfx {
    /// The picture. Outside this crate use [`Gfx::pm_ref`] and [`Gfx::pm_mut`]: the second one first draws what the recording layer has
    /// written down (see `par`), so direct pixel work is always in order.
    pub(crate) pm: Pixmap,
    /// Everything drawn, registered as clickable or asked about with `hovered` is moved down by this many pixels (see [`Gfx::set_origin_y`]).
    oy: f32,
    /// Clickable regions registered during this frame (later = on top).
    pub hits: Vec<Hit>,
    /// Pointer position, for hover highlights.
    pub mouse: (i32, i32),
    /// Every rectangle the frame asked about with [`Gfx::hovered`]: only a pointer crossing one of their edges
    /// can change the picture.
    probes: std::cell::RefCell<Vec<R>>,
    /// The bundled faces are parsed once for the whole program and shared by every frame buffer (`crate::fonts`): parsing them took 27 ms
    /// of every new buffer. The letters drawn so far, and the fonts' generation they were drawn in (a new choice empties it).
    cache: HashMap<GlyphKey, (fontdue::Metrics, Vec<u8>)>,
    cache_fonts: u32,
    /// The pixels outside a clip rectangle while [`Gfx::clipped`] runs (kept between calls so nothing is allocated per frame).
    clip_buf: Vec<u8>,
    /// The clip rectangles of the [`Gfx::clipped`] calls that are running now (innermost last).
    clip_now: Vec<R>,
    /// The lowest point the text and click regions drawn since it was last cleared reach (those that start inside the innermost clip): lets
    /// a page that does not scroll itself learn how tall it is. Cleared by the caller.
    pub ink_bottom: f32,
    /// Made the first time text in a script that needs shaping appears (it parses a couple of MB of fonts).
    shaper: Option<std::sync::Arc<crate::shaping::Shaper>>,
    shaped_cache: HashMap<(usize, u16, u32), (fontdue::Metrics, Vec<u8>)>,
    /// Test builds only: every string passed to [`Gfx::text`] with its top-left position, for `TileHarness`.
    #[cfg(any(test, feature = "testing"))]
    pub text_log: Vec<(f32, f32, String)>,
    /// While `Some`, every string drawn is kept here (developer mode's `iyakki dev state`, which asks for it a few seconds at a time).
    /// Capped, and switched on by the shell only for the frames a computer asked for.
    pub text_cap: Option<Vec<(f32, f32, String)>>,
    /// Test builds only: what the overflow detector (`monkey_tests`, KI-015) needs: every single-line text with its box and the innermost
    /// clip it was drawn in, and every opaque fill, both in drawing order (`seq`).
    #[cfg(any(test, feature = "testing"))]
    pub layout_log: LayoutLog,
    /// While `Some`, drawing calls are written down instead of drawn (see `par`): the opt-in recording layer of F-SHELL-64.
    rec: Option<Vec<par::RecOp>>,
    /// Only in a band buffer of the parallel replay: the screen row its first row stands for.
    band_y: i32,
    /// Only in a band buffer: the saved outsides of the clips running now.
    clip_stack: Vec<(R, Vec<u8>)>,
}

/// See [`Gfx::layout_log`].
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct LayoutLog {
    pub seq: u32,
    pub clips: Vec<R>,
    pub texts: Vec<(u32, R, Option<R>, String)>,
    pub fills: Vec<(u32, R)>,
}

#[cfg(any(test, feature = "testing"))]
impl LayoutLog {
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        if c[3] >= 250 {
            self.seq += 1;
            self.fills.push((self.seq, R::new(x, y, w, h)));
        }
    }
}

/// The script fonts, parsed on first use by any frame buffer and shared (a couple of MB of fonts: not parsed again for a new buffer).
static SHAPER: std::sync::OnceLock<std::sync::Arc<crate::shaping::Shaper>> = std::sync::OnceLock::new();

fn shared_shaper() -> std::sync::Arc<crate::shaping::Shaper> {
    SHAPER.get_or_init(|| std::sync::Arc::new(crate::shaping::Shaper::new())).clone()
}

/// Does this text have characters of a script that needs shaping (Hebrew, Arabic, the Indic scripts, Thai and the like)? Cheap: only
/// code points are looked at.
pub fn needs_script_fonts(text: &str) -> bool {
    text.chars().any(|c| matches!(c as u32, 0x0590..=0x0DFF | 0x0E00..=0x0EFF | 0x0F00..=0x109F | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// Parses the script fonts on a thread of their own if they are not yet (a couple of MB: 34 ms on a fast machine, several times that on
/// an old one, which a person would otherwise feel as a stop the first time a page in such a script is drawn). Does nothing for plain
/// English: call it only when [`needs_script_fonts`] says so.
pub fn warm_script_fonts() {
    if SHAPER.get().is_none() {
        let _ = std::thread::Builder::new().name("iyakki-script-fonts".into()).spawn(|| {
            let _ = shared_shaper();
        });
    }
}

/// Have the script fonts been parsed?
pub fn script_fonts_ready() -> bool {
    SHAPER.get().is_some()
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

/// The squircle (|x|^5 + |y|^5 = 1, close to the iOS-style continuous corner) inscribed in the box.
pub fn squircle_path(x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    const N: f32 = 5.0;
    const STEPS: usize = 96;
    let (cx, cy, a, b) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
    let mut pb = PathBuilder::new();
    for i in 0..STEPS {
        let t = i as f32 / STEPS as f32 * std::f32::consts::TAU;
        let (c, s) = (t.cos(), t.sin());
        let px = cx + a * c.signum() * c.abs().powf(2.0 / N);
        let py = cy + b * s.signum() * s.abs().powf(2.0 / N);
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
    pb.finish()
}

pub fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
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
            oy: 0.0,
            hits: Vec::new(),
            mouse: (-1, -1),
            probes: std::cell::RefCell::new(Vec::new()),
            cache: HashMap::new(),
            cache_fonts: crate::fonts::generation(),
            clip_buf: Vec::new(),
            clip_now: Vec::new(),
            ink_bottom: 0.0,
            shaper: None,
            shaped_cache: HashMap::new(),
            #[cfg(any(test, feature = "testing"))]
            text_log: Vec::new(),
            text_cap: None,
            #[cfg(any(test, feature = "testing"))]
            layout_log: LayoutLog::default(),
            rec: None,
            band_y: 0,
            clip_stack: Vec::new(),
        }
    }

    /// Draw the next things `dy` pixels lower (or higher when negative), regions and hover tests included, until it is set back to 0. The shell
    /// uses it to put the whole top bar at the bottom edge (and the footer at the top) without changing a coordinate of the code that draws them.
    pub fn set_origin_y(&mut self, dy: f32) {
        self.oy = dy;
    }

    pub fn origin_y(&self) -> f32 {
        self.oy
    }

    fn ts(&self) -> Transform {
        // `band_y` is 0 except in a band buffer of the parallel replay (see `par`)
        Transform::from_translate(0.0, self.oy - self.band_y as f32)
    }

    /// Register a clickable region. `ev` is delivered to the screen on click.
    pub fn hit(&mut self, rect: R, ev: Event, label: &str) {
        let rect = R::new(rect.x, rect.y + self.oy, rect.w, rect.h);
        self.note_ink(rect.y, rect.y + rect.h);
        self.hits.push(Hit { rect, ev, label: label.into() });
    }

    /// Run `f` so that nothing it draws or registers as clickable lands outside `clip`: pixels outside the rectangle are put
    /// back afterwards and click regions are cut to it (dropped when nothing is left). For scrolled content (a transcript, a
    /// list) whose items are taller than the pane or start above it.
    pub fn clipped<T>(&mut self, clip: R, f: impl FnOnce(&mut Gfx) -> T) -> T {
        let clip = R::new(clip.x, clip.y + self.oy, clip.w, clip.h);
        if self.rec.is_some() {
            return self.clipped_recorded(clip, f);
        }
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
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.clips.push(clip);
        self.clip_now.push(clip);
        let out = f(self);
        self.clip_now.pop();
        #[cfg(any(test, feature = "testing"))]
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
        self.record_sync();
        let rec = self.rec.take();
        let clip = R::new(clip.x, clip.y + self.oy, clip.w, clip.h);
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
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.clips.push(clip);
        self.clip_now.push(clip);
        let out = f(self);
        self.clip_now.pop();
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.clips.pop();
        let mut at = 0;
        for &(x0, y0, x1, y1) in &rings {
            at += self.copy_in(x0, y0, x1, y1, &saved[at..]);
        }
        self.rec = rec;
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
    pub fn grab(&mut self, rect: (i32, i32, i32, i32)) -> Vec<u8> {
        self.record_sync();
        let _p = crate::prof::scope("gfx.grab");
        let mut v = Vec::new();
        self.copy_out(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3, &mut v);
        v
    }

    /// As [`Gfx::grab`] into a buffer the caller keeps (cleared first), so a frame does not allocate for it.
    pub fn grab_into(&mut self, rect: (i32, i32, i32, i32), out: &mut Vec<u8>) {
        self.record_sync();
        out.clear();
        self.copy_out(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3, out);
    }

    /// Put pixels taken with [`Gfx::grab`] back (same rectangle).
    pub fn put(&mut self, rect: (i32, i32, i32, i32), px: &[u8]) {
        self.record_sync();
        let _p = crate::prof::scope("gfx.put");
        self.copy_in(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3, px);
    }

    /// Draw into a small pixmap that holds only `rect`, seeded with `seed` (the pixels of the rectangle as [`Gfx::grab`] gave
    /// them) or with what is on the screen now, and write it back. `f` draws as usual but gets the rectangle's origin and
    /// has to subtract it from its coordinates. A large shape that matters only at its edges then costs the size of the edges.
    /// The pointer position is shifted too, so hover tests keep working; hit regions are not kept.
    pub fn in_region(&mut self, rect: (i32, i32, i32, i32), seed: Option<&[u8]>, f: impl FnOnce(&mut Gfx, f32, f32)) {
        let _p = crate::prof::scope("gfx.in_region");
        self.record_sync();
        let rec = self.rec.take();
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let (x0, y0) = (rect.0.clamp_safe(0, w), rect.1.clamp_safe(0, h));
        let (x1, y1) = ((rect.0 + rect.2).clamp_safe(x0, w), (rect.1 + rect.3).clamp_safe(y0, h));
        let (rw, rh) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let Some(mut scratch) = (if rw > 0 && rh > 0 { Pixmap::new(rw, rh) } else { None }) else {
            self.rec = rec;
            return;
        };
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
        self.rec = rec;
    }

    /// Drop every region registered so far (a modal calls this so the dimmed UI behind it can't be clicked).
    pub fn clear_hits(&mut self) {
        self.hits.clear();
    }

    pub fn hovered(&self, r: R) -> bool {
        let r = R::new(r.x, r.y + self.oy, r.w, r.h);
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

    /// Buttons in a row that wraps at `x + w` (a narrow window or a large text size gives more rows instead of cutting the last
    /// button off). Returns the height used and the right edge of the last button on the last row.
    pub fn button_flow(&mut self, x: f32, y: f32, w: f32, size: f32, gap: f32, items: &[(String, [u8; 4], Event)]) -> (f32, f32) {
        let (mut bx, mut by) = (x, y);
        let h = size * 2.1;
        for (label, accent, ev) in items {
            let bw = self.text_width(size, label, Face::Bold) + size * 1.7;
            if bx > x && bx + bw > x + w {
                bx = x;
                by += h + gap;
            }
            bx += self.button(bx, by, size, label, *accent, ev.clone()) + gap;
        }
        (by + h - y, bx - gap)
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

    fn note_ink(&mut self, top: f32, bottom: f32) {
        if !(top.is_finite() && bottom.is_finite()) {
            return;
        }
        if self.clip_now.last().is_some_and(|c| top >= c.y + c.h || bottom <= c.y) {
            return;
        }
        if bottom > self.ink_bottom {
            self.ink_bottom = bottom;
        }
    }

    /// The right edge of what can be seen now: the window's, or the nearest clip's when a [`Gfx::clipped`] call is running.
    pub fn visible_right(&self) -> f32 {
        self.clip_now.iter().fold(self.pm.width() as f32, |r, c| r.min(c.x + c.w))
    }

    pub fn width(&self) -> u32 {
        self.pm.width()
    }
    pub fn height(&self) -> u32 {
        self.pm.height()
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.record_cancel();
        if (w, h) != (self.width(), self.height()) {
            self.pm = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
        }
    }

    pub fn clear(&mut self, c: [u8; 4]) {
        rec_leaf!(self, f32::NEG_INFINITY, f32::INFINITY, |g| g.clear(c));
        self.pm.fill(col(c));
    }

    /// Vertical gradient background.
    pub fn gradient(&mut self, top: [u8; 4], bottom: [u8; 4]) {
        if self.rec.is_some() {
            let h = self.height() as f32;
            self.rec_push(f32::NEG_INFINITY, f32::INFINITY, move |g: &mut Gfx| g.gradient_h(top, bottom, h));
            return;
        }
        let h = self.height() as f32;
        self.gradient_h(top, bottom, h);
    }

    fn gradient_h(&mut self, top: [u8; 4], bottom: [u8; 4], h: f32) {
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.gradient");
        let shader = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, h),
            vec![GradientStop::new(0.0, col(top)), GradientStop::new(1.0, col(bottom))],
            SpreadMode::Pad,
            ts,
        );
        let mut p = Paint::default();
        p.shader = shader.unwrap_or(Shader::SolidColor(col(top)));
        if let Some(r) = Rect::from_xywh(0.0, 0.0, self.width() as f32, h) {
            self.pm.fill_rect(r, &p, ts, None);
        }
    }

    /// A squircle (superellipse, the rounded square of an app icon) in the box `x, y, w, h`, filled with a vertical gradient
    /// (`stops` are (0..1 from top, colour); one stop is a flat fill).
    pub fn squircle_gradient(&mut self, x: f32, y: f32, w: f32, h: f32, stops: &[(f32, [u8; 4])]) {
        if self.rec.is_some() {
            let stops = stops.to_vec();
            self.rec_push(y - 1.0, y + h + 1.0, move |g: &mut Gfx| g.squircle_gradient(x, y, w, h, &stops));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.squircle_gradient");
        let Some(path) = squircle_path(x, y, w, h) else { return };
        let mut p = Paint::default();
        p.anti_alias = true;
        match stops {
            [] => return,
            [(_, c)] => p.set_color(col(*c)),
            _ => {
                let gs: Vec<GradientStop> = stops.iter().map(|(t, c)| GradientStop::new(*t, col(*c))).collect();
                match LinearGradient::new(Point::from_xy(x, y), Point::from_xy(x, y + h), gs, SpreadMode::Pad, Transform::identity()) {
                    Some(sh) => p.shader = sh,
                    None => p.set_color(col(stops[0].1)),
                }
            }
        }
        self.pm.fill_path(&path, &p, FillRule::Winding, ts, None);
    }

    /// A rounded rectangle filled with a vertical gradient (`stops`: 0..1 from the top, colour).
    pub fn rrect_gradient(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, stops: &[(f32, [u8; 4])]) {
        if self.rec.is_some() {
            let stops = stops.to_vec();
            self.rec_push(y - 1.0, y + h + 1.0, move |g: &mut Gfx| g.rrect_gradient(x, y, w, h, r, &stops));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.rrect_gradient");
        let Some(path) = rrect_path(x, y, w, h, r) else { return };
        let gs: Vec<GradientStop> = stops.iter().map(|(t, c)| GradientStop::new(*t, col(*c))).collect();
        let Some(sh) = LinearGradient::new(Point::from_xy(x, y), Point::from_xy(x, y + h), gs, SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.anti_alias = true;
        p.shader = sh;
        self.pm.fill_path(&path, &p, FillRule::Winding, ts, None);
    }

    /// Any path filled with a vertical gradient from `y0` to `y1` (`stops`: 0..1, colour); one stop is a flat fill.
    pub fn path_gradient(&mut self, path: &Path, y0: f32, y1: f32, stops: &[(f32, [u8; 4])]) {
        if self.rec.is_some() {
            let (stops, path) = (stops.to_vec(), path.clone());
            let b = path.bounds();
            self.rec_push(b.top() - 1.0, b.bottom() + 1.0, move |g: &mut Gfx| g.path_gradient(&path, y0, y1, &stops));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.path_gradient");
        let mut p = Paint::default();
        p.anti_alias = true;
        match stops {
            [] => return,
            [(_, c)] => p.set_color(col(*c)),
            _ => {
                let gs: Vec<GradientStop> = stops.iter().map(|(t, c)| GradientStop::new(*t, col(*c))).collect();
                match LinearGradient::new(Point::from_xy(0.0, y0), Point::from_xy(0.0, y1.max(y0 + 1.0)), gs, SpreadMode::Pad, Transform::identity()) {
                    Some(sh) => p.shader = sh,
                    None => p.set_color(col(stops[0].1)),
                }
            }
        }
        self.pm.fill_path(path, &p, FillRule::Winding, ts, None);
    }

    /// Any path's outline.
    pub fn path_outline(&mut self, path: &Path, width: f32, c: [u8; 4]) {
        if self.rec.is_some() {
            let path = path.clone();
            let b = path.bounds();
            self.rec_push(b.top() - width - 1.0, b.bottom() + width + 1.0, move |g: &mut Gfx| g.path_outline(&path, width, c));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.path_outline");
        self.pm.stroke_path(path, &paint(c), &stroke(width), ts, None);
    }

    /// A squircle's outline.
    pub fn squircle_outline(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, c: [u8; 4]) {
        rec_leaf!(self, y - width - 1.0, y + h + width + 1.0, |g| g.squircle_outline(x, y, w, h, width, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.squircle_outline");
        if let Some(path) = squircle_path(x, y, w, h) {
            self.pm.stroke_path(&path, &paint(c), &stroke(width), ts, None);
        }
    }

    /// A rectangle filled with a gradient from (x0, y0) to (x1, y1); `stops` are (position 0..1, colour).
    pub fn gradient_rect(&mut self, x: f32, y: f32, w: f32, h: f32, from: (f32, f32), to: (f32, f32), stops: &[(f32, [u8; 4])]) {
        if self.rec.is_some() {
            let stops = stops.to_vec();
            self.rec_push(y - 1.0, y + h.max(1.0) + 1.0, move |g: &mut Gfx| g.gradient_rect(x, y, w, h, from, to, &stops));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.gradient_rect");
        let stops: Vec<GradientStop> = stops.iter().map(|(p, c)| GradientStop::new(*p, col(*c))).collect();
        let Some(shader) = LinearGradient::new(Point::from_xy(from.0, from.1), Point::from_xy(to.0, to.1), stops, SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
            self.pm.fill_rect(r, &p, ts, None);
        }
    }

    /// A soft round glow: `c` at its own alpha in the middle, fading to nothing at `radius`.
    pub fn radial_glow(&mut self, cx: f32, cy: f32, radius: f32, c: [u8; 4]) {
        rec_leaf!(self, cy - radius - 1.0, cy + radius + 1.0, |g| g.radial_glow(cx, cy, radius, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.radial_glow");
        let faded = [c[0], c[1], c[2], 0];
        let Some(shader) = tiny_skia::RadialGradient::new(Point::from_xy(cx, cy), Point::from_xy(cx, cy), radius, vec![GradientStop::new(0.0, col(c)), GradientStop::new(1.0, col(faded))], SpreadMode::Pad, Transform::identity()) else { return };
        let mut p = Paint::default();
        p.shader = shader;
        if let Some(r) = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0) {
            self.pm.fill_rect(r, &p, ts, None);
        }
    }

    /// A glow around a rounded rectangle: three translucent rings growing outwards, `c` is the strongest alpha.
    pub fn rrect_glow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, spread: f32, c: [u8; 4]) {
        let _p = crate::prof::scope("gfx.rrect_glow");
        for (k, f) in [(1.0f32, 0.35f32), (0.66, 0.6), (0.33, 1.0)] {
            let d = spread * k;
            let a = (c[3] as f32 * f * 0.5) as u8;
            self.rrect(x - d, y - d, w + 2.0 * d, h + 2.0 * d, r + d, [c[0], c[1], c[2], a]);
        }
    }

    /// Axis-aligned fill, snapped to whole pixels (crisp edges; avoids tiny-skia's thin-rect hairline path).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.rect");
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.fill(x, y, w, h, c);
        rec_leaf!(self, y - 1.0, y + h + 1.0, |g| g.rect(x, y, w, h, c));
        if self.band_y != 0 && self.oy == 0.0 {
            // a band buffer of the parallel replay: tiny-skia fills a rectangle with the identity transform on a fast path with no edge
            // smoothing, and any other transform through a smoothed path; the direct frame took the fast path, so this does too
            if let Some(r) = Rect::from_xywh(x.round(), y.round() - self.band_y as f32, w.round().max(1.0), h.round().max(1.0)) {
                self.pm.fill_rect(r, &paint(c), Transform::identity(), None);
            }
        } else if let Some(r) = Rect::from_xywh(x.round(), y.round(), w.round().max(1.0), h.round().max(1.0)) {
            self.pm.fill_rect(r, &paint(c), ts, None);
        }
    }

    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: [u8; 4]) {
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.rrect");
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.fill(x, y, w, h, c);
        rec_leaf!(self, y - 1.0, y + h + 1.0, |g| g.rrect(x, y, w, h, r, c));
        if let Some(p) = rrect_path(x, y, w, h, r) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, ts, None);
        }
    }

    /// A rounded rectangle filled with a picture that covers the whole screen at `factor` times its size (the blurred frost of the island):
    /// the picture is stretched with smoothing, so a frame costs one fill.
    pub fn rrect_picture(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, pic: &Pixmap, factor: u32) {
        if self.rec.is_some() {
            let pic = pic.clone();
            self.rec_push(y - 1.0, y + h + 1.0, move |g: &mut Gfx| g.rrect_picture(x, y, w, h, r, &pic, factor));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.rrect_picture");
        let Some(path) = rrect_path(x, y, w, h, r) else { return };
        let k = factor.max(1) as f32;
        let mut p = Paint::default();
        p.anti_alias = true;
        p.shader = tiny_skia::Pattern::new(pic.as_ref(), SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, 1.0, Transform::from_scale(k, k));
        self.pm.fill_path(&path, &p, FillRule::Winding, ts, None);
    }

    /// A picture of `pw` x `ph` pixels (straight RGBA) stretched into the rectangle, smoothed (a camera or video frame). Nothing is drawn
    /// for bytes that do not make that picture.
    pub fn rgba_picture(&mut self, x: f32, y: f32, w: f32, h: f32, pw: u32, ph: u32, rgba: &[u8]) {
        if self.rec.is_some() {
            let rgba = rgba.to_vec();
            self.rec_push_abs(y - 1.0, y + h + 1.0, move |g: &mut Gfx| g.rgba_picture(x, y, w, h, pw, ph, &rgba));
            return;
        }
        let _p = crate::prof::scope("gfx.rgba_picture");
        let (Some(size), true) = (tiny_skia::IntSize::from_wh(pw, ph), rgba.len() == pw as usize * ph as usize * 4) else { return };
        let mut v = rgba.to_vec();
        for px in v.chunks_exact_mut(4) {
            let a = px[3] as u16;
            if a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u16 * a + 127) / 255) as u8;
                }
            }
        }
        let Some(pic) = Pixmap::from_vec(v, size) else { return };
        let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else { return };
        let mut p = Paint::default();
        p.shader = tiny_skia::Pattern::new(pic.as_ref(), SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, 1.0, Transform::from_row(w / pw as f32, 0.0, 0.0, h / ph as f32, x, y));
        self.pm.fill_rect(rect, &p, Transform::from_translate(0.0, -(self.band_y as f32)), None);
    }

    pub fn rrect_outline(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: [u8; 4]) {
        rec_leaf!(self, y - width - 1.0, y + h + width + 1.0, |g| g.rrect_outline(x, y, w, h, r, width, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.rrect_outline");
        if let Some(p) = rrect_path(x, y, w, h, r) {
            self.pm.stroke_path(&p, &paint(c), &stroke(width), ts, None);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
        rec_leaf!(self, cy - r - 1.0, cy + r + 1.0, |g| g.circle(cx, cy, r, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.circle");
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, ts, None);
        }
    }

    /// A circle's outline.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: [u8; 4]) {
        rec_leaf!(self, cy - r - width - 1.0, cy + r + width + 1.0, |g| g.ring(cx, cy, r, width, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.ring");
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm.stroke_path(&p, &paint(c), &stroke(width), ts, None);
        }
    }

    /// A straight line with round ends.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: [u8; 4]) {
        rec_leaf!(self, y0.min(y1) - width - 1.0, y0.max(y1) + width + 1.0, |g| g.line(x0, y0, x1, y1, width, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.line");
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        if let Some(p) = pb.finish() {
            let mut st = stroke(width);
            st.line_cap = tiny_skia::LineCap::Round;
            self.pm.stroke_path(&p, &paint(c), &st, ts, None);
        }
    }

    /// A filled polygon (a play arrow, a skip mark).
    pub fn polygon(&mut self, pts: &[(f32, f32)], c: [u8; 4]) {
        if self.rec.is_some() {
            let pts = pts.to_vec();
            self.rec_push(f32::NEG_INFINITY, f32::INFINITY, move |g: &mut Gfx| g.polygon(&pts, c));
            return;
        }
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.polygon");
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
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, ts, None);
        }
    }

    /// Several closed outlines filled together (non-zero winding, so one wound the other way is a hole) and/or stroked: a path flattened
    /// by the UI SDK. `closed` says which outlines are closed for the stroke.
    pub fn polys(&mut self, outlines: &[(Vec<(f32, f32)>, bool)], fill: Option<[u8; 4]>, stroke_with: Option<(f32, [u8; 4])>) {
        if self.rec.is_some() {
            let outlines = outlines.to_vec();
            self.rec_push(f32::NEG_INFINITY, f32::INFINITY, move |g: &mut Gfx| g.polys(&outlines, fill, stroke_with));
            return;
        }
        let ts = self.ts();
        let mut pb = PathBuilder::new();
        for (pts, closed) in outlines {
            for (i, (x, y)) in pts.iter().enumerate() {
                if i == 0 {
                    pb.move_to(*x, *y);
                } else {
                    pb.line_to(*x, *y);
                }
            }
            if *closed || fill.is_some() {
                pb.close();
            }
        }
        let Some(p) = pb.finish() else { return };
        if let Some(c) = fill {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, ts, None);
        }
        if let Some((w, c)) = stroke_with {
            self.pm.stroke_path(&p, &paint(c), &stroke(w), ts, None);
        }
    }

    fn font(&self, f: Face) -> &'static Font {
        crate::fonts::font(f)
    }

    pub fn line_height(&self, size: f32, face: Face) -> f32 {
        self.font(face).horizontal_line_metrics(size).map_or(size * 1.3, |m| m.new_line_size)
    }

    pub fn text_width(&mut self, size: f32, s: &str, face: Face) -> f32 {
        let _p = crate::prof::scope("gfx.text_width");
        let px = size.round().max(MIN_TEXT_PX);
        if crate::shaping::needs_shaping(s) {
            return self.shaper.get_or_insert_with(shared_shaper).layout(s, px).1;
        }
        s.chars().map(|c| crate::fonts::font_for(face, c).metrics(c, px).advance_width).sum()
    }

    /// Draw `s` with its top-left at (x, y). Returns the advance width. Text is clipped to the pixmap.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: [u8; 4], s: &str, face: Face) -> f32 {
        let _p = crate::prof::scope("gfx.text");
        self.note_ink(y, y + size * 1.3);
        #[cfg(any(test, feature = "testing"))]
        {
            self.text_log.push((x, y + self.oy, s.to_string()));
            if !s.contains('\n') && !s.trim().is_empty() {
                let (tw, th) = (self.text_width(size, s, face), self.line_height(size.round().max(MIN_TEXT_PX), face));
                self.layout_log.seq += 1;
                let (seq, clip) = (self.layout_log.seq, self.layout_log.clips.last().copied());
                self.layout_log.texts.push((seq, R::new(x, y + self.oy, tw, th), clip, s.to_string()));
            }
        }
        if let Some(v) = self.text_cap.as_mut() {
            if v.len() < 4000 {
                v.push((x, y + self.oy, s.to_string()));
            }
        }
        let raw_color = color;
        let color = crate::theme::themed(color);
        let px = size.round().max(MIN_TEXT_PX);
        let ascent = self.font(face).horizontal_line_metrics(px).map_or(px, |m| m.ascent);
        let base = y + self.oy + ascent;
        let (w, h) = (self.width() as i32, self.height() as i32);
        // recording: the letters are still measured here (the answer is the advance), but drawn later, by the replay
        let recording = self.rec.is_some();
        if recording {
            let s2 = s.to_string();
            self.rec_push(y - 2.0, y + size * 1.6 + 2.0, move |g: &mut Gfx| {
                g.text(x, y, size, raw_color, &s2, face);
            });
        }
        if crate::shaping::needs_shaping(s) {
            return self.shaped_text(x, base, px, color, s, recording);
        }
        let mut pen = x;
        if self.cache_fonts != crate::fonts::generation() {
            self.cache.clear();
            self.cache_fonts = crate::fonts::generation();
        }
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let key = (ch, px as u32, face);
            if !self.cache.contains_key(&key) {
                let (m, bmp) = crate::fonts::font_for(face, ch).rasterize(ch, px);
                self.cache.insert(key, (m, bmp));
            }
            let (m, bmp) = &self.cache[&key];
            let (gx, gy) = ((pen + m.xmin as f32).round() as i32, (base - m.height as f32 - m.ymin as f32).round() as i32 - self.band_y);
            if !recording {
                blit(self.pm.data_mut(), (w, h), gx, gy, m, bmp, color);
            }
            pen += m.advance_width;
        }
        pen - x
    }

    /// `s` as coverage, for pictures that draw their own text (SVG): white text on a clear pixmap, kept as alpha. The mask's
    /// corner is given relative to the pen on the baseline. Empty when the text is too wide to be worth drawing.
    pub fn text_mask(&mut self, size: f32, s: &str, face: Face) -> TextMask {
        let _p = crate::prof::scope("gfx.text_mask");
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
        let oy = std::mem::replace(&mut self.oy, 0.0);
        let rec = self.rec.take();
        self.text(pad, pad, px, [255, 255, 255, 255], s, face);
        self.rec = rec;
        self.oy = oy;
        let drawn = std::mem::replace(&mut self.pm, old);
        let alpha: Vec<u8> = drawn.data().chunks_exact(4).map(|p| p[3]).collect();
        TextMask { advance: advance as f64, x: -(pad as i32), y: -((pad + ascent).round() as i32), w, h, alpha }
    }

    /// Draw a line in a script that needs shaping (see [`crate::shaping`]); `base` is the baseline.
    fn shaped_text(&mut self, x: f32, base: f32, px: f32, color: [u8; 4], s: &str, recording: bool) -> f32 {
        let (w, h) = (self.width() as i32, self.height() as i32);
        let sh = self.shaper.get_or_insert_with(shared_shaper);
        let (glyphs, total) = sh.layout(s, px);
        for g in glyphs {
            let key = (g.font, g.glyph, px as u32);
            if !self.shaped_cache.contains_key(&key) {
                let r = sh.font(g.font).rasterize_indexed(g.glyph, px);
                self.shaped_cache.insert(key, r);
            }
            let (m, bmp) = &self.shaped_cache[&key];
            let gx = (x + g.x + m.xmin as f32).round() as i32;
            let gy = (base - g.y - m.height as f32 - m.ymin as f32).round() as i32 - self.band_y;
            if !recording {
                blit(self.pm.data_mut(), (w, h), gx, gy, m, bmp, color);
            }
        }
        total
    }

    /// Truncate with an ellipsis so the text fits in `max_w`.
    pub fn fit(&mut self, size: f32, s: &str, face: Face, max_w: f32) -> String {
        let _p = crate::prof::scope("gfx.fit");
        if self.text_width(size, s, face) <= max_w {
            return s.to_string();
        }
        if !crate::shaping::needs_shaping(s) {
            // one pass over the glyph advances (the width of a plain string is their sum) instead of measuring every shorter prefix
            let px = size.round().max(MIN_TEXT_PX);
            let room = max_w - crate::fonts::font_for(face, '…').metrics('…', px).advance_width;
            let mut acc = 0.0;
            let mut end = 0;
            for (i, c) in s.char_indices() {
                acc += crate::fonts::font_for(face, c).metrics(c, px).advance_width;
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
        let _p = crate::prof::scope("gfx.wrap");
        let mut lines = Vec::new();
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split(' ') {
                let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if self.text_width(size, &cand, face) <= max_w || cur.is_empty() {
                    cur = cand;
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                }
                // a single over-long word: hard-break it (also the one that starts a new line: it used to be left whole and ran out of its box)
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
            }
            lines.push(cur);
        }
        lines
    }

    /// Draw an icon centred at (cx, cy) fitting a `size` square.
    pub fn icon(&mut self, icon: Icon, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
        rec_leaf!(self, cy - size - 2.0, cy + size + 2.0, |g| g.icon(icon, cx, cy, size, c));
        let ts = self.ts();
        let _p = crate::prof::scope("gfx.icon");
        let s = size;
        let (x0, y0) = (cx - s / 2.0, cy - s / 2.0);
        let lw = (s * 0.075).max(1.5);
        let st = stroke(lw);
        let id = ts;
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
            Icon::K8s => {
                // a ship's wheel: a seven-sided rim, a hub, a spoke to every corner
                let (cx, cy, r) = (x0 + s * 0.5, y0 + s * 0.5, s * 0.40);
                let corner = |k: usize| {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / 7.0;
                    (cx + r * a.cos(), cy + r * a.sin())
                };
                let mut rim = PathBuilder::new();
                for k in 0..7 {
                    let (px, py) = corner(k);
                    if k == 0 {
                        rim.move_to(px, py);
                    } else {
                        rim.line_to(px, py);
                    }
                }
                rim.close();
                if let Some(l) = rim.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                let mut spokes = PathBuilder::new();
                for k in 0..7 {
                    let (px, py) = corner(k);
                    spokes.move_to(cx, cy);
                    spokes.line_to(px, py);
                }
                if let Some(l) = spokes.finish() {
                    self.pm.stroke_path(&l, &p, &st, id, None);
                }
                let mut hub = PathBuilder::new();
                hub.push_circle(cx, cy, s * 0.09);
                if let Some(d) = hub.finish() {
                    self.pm.fill_path(&d, &p, FillRule::Winding, id, None);
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
    /// The picture, to read. Do not call while a frame is being recorded (use [`Gfx::record_sync`] first).
    pub fn pm_ref(&self) -> &Pixmap {
        debug_assert!(self.rec.as_ref().is_none_or(|v| v.is_empty()), "pixels read while calls are still written down: call record_sync first");
        &self.pm
    }

    /// The picture, to draw into or change directly: first draws what the recording layer holds, so the order is kept.
    pub fn pm_mut(&mut self) -> &mut Pixmap {
        self.record_sync();
        &mut self.pm
    }

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
        for icon in [Icon::Chat, Icon::Rag, Icon::Mcp, Icon::Cli, Icon::Users, Icon::Settings, Icon::Clipboard, Icon::Mic, Icon::Speaker, Icon::Runs, Icon::Edit, Icon::Browser, Icon::Camera, Icon::Mesh, Icon::K8s, Icon::Radio, Icon::Scan, Icon::Skills, Icon::Usage, Icon::Calendar, Icon::System, Icon::Media, Icon::Ide, Icon::Data, Icon::Tales, Icon::Bell, Icon::Lock, Icon::Power, Icon::Logout, Icon::Store, Icon::Dash, Icon::Flows, Icon::Devices, Icon::Disks, Icon::Lab, Icon::Git, Icon::Doctor, Icon::Checkup, Icon::Learn, Icon::Volume(0), Icon::Volume(1), Icon::Volume(2), Icon::Volume(3), Icon::Volume(4), Icon::Home] {
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
        // a long word that comes after other words starts a new line and is broken there too, never left to run out of its box
        let after = g.wrap(14.0, "Scaffold: tool app_scaffold {kind: tile|skill|mcp-python|mcp-node|wasm-tool, name, dir}", Face::Sans, 120.0);
        assert!(after.len() > 3 && after.iter().all(|l| g.text_width(14.0, l, Face::Sans) <= 120.5), "{after:?}");
        assert_eq!(after.join(" ").replace(' ', "").len(), "Scaffold: tool app_scaffold {kind: tile|skill|mcp-python|mcp-node|wasm-tool, name, dir}".replace(' ', "").len(), "no letter is lost");
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

    #[test]
    fn new_frame_buffers_share_the_parsed_fonts_and_the_script_fonts_and_draw_the_same() {
        let (mut a, mut b) = (Gfx::new(200, 40), Gfx::new(200, 40));
        assert!(std::ptr::eq(crate::fonts::font(Face::Sans), crate::fonts::font(Face::Sans)), "the faces are parsed once");
        // making a buffer is cheap now (it was 27 ms of font parsing in a release build): a hundred of them take a fraction of a second
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let _ = Gfx::new(64, 64);
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "100 buffers took {:?}", t.elapsed());
        a.text(4.0, 4.0, 16.0, [255, 255, 255, 255], "Same text", Face::Sans);
        b.text(4.0, 4.0, 16.0, [255, 255, 255, 255], "Same text", Face::Sans);
        assert_eq!(a.pm.data(), b.pm.data());
        // the script fonts are shared as well, and plain English still never loads them
        assert!(a.shaper.is_none());
        a.text(4.0, 24.0, 16.0, [255, 255, 255, 255], "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}", Face::Sans);
        b.text(4.0, 24.0, 16.0, [255, 255, 255, 255], "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}", Face::Sans);
        if let (Some(x), Some(y)) = (&a.shaper, &b.shaper) {
            assert!(std::sync::Arc::ptr_eq(x, y));
        }
    }

    /// A measurement, not a check: what the first text in a script that needs shaping costs (the script fonts are parsed then).
    /// `cargo test --release -p iyakki-uikit first_shaped_text_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement"]
    fn first_shaped_text_cost() {
        let mut g = Gfx::new(300, 60);
        let t = std::time::Instant::now();
        g.text(4.0, 4.0, 16.0, [255, 255, 255, 255], "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}", Face::Sans);
        println!("SHAPER first text in Devanagari: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        g.text(4.0, 30.0, 16.0, [255, 255, 255, 255], "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}", Face::Sans);
        println!("SHAPER the same text again: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        let mut h = Gfx::new(300, 60);
        h.text(4.0, 4.0, 16.0, [255, 255, 255, 255], "\u{628}\u{633}\u{645}", Face::Sans);
        println!("SHAPER first Arabic text on a new buffer (fonts already parsed): {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    #[test]
    fn the_script_fonts_are_warmed_only_for_text_that_needs_them_and_then_a_new_buffer_finds_them_ready() {
        assert!(!needs_script_fonts("Plain English, caf\u{e9} and \u{65e5}\u{672c}\u{8a9e} and \u{1f600}"));
        assert!(needs_script_fonts("\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}") && needs_script_fonts("a \u{628}\u{633}\u{645} b") && needs_script_fonts("\u{5e9}\u{5dc}\u{5d5}\u{5dd}") && needs_script_fonts("\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}"));
        warm_script_fonts();
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            if script_fonts_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(script_fonts_ready(), "the thread parsed them");
        let mut g = Gfx::new(200, 40);
        let t = std::time::Instant::now();
        g.text(4.0, 4.0, 16.0, [255, 255, 255, 255], "\u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947}", Face::Sans);
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        assert!(g.pm.data().iter().any(|&b| b != 0), "the text was drawn");
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    #[test]
    fn an_origin_moves_shapes_text_icons_regions_and_hover_tests_together() {
        let draw = |g: &mut Gfx| {
            g.clear([10, 10, 10, 255]);
            g.rrect(10.0, 10.0, 60.0, 20.0, 6.0, [200, 0, 0, 255]);
            g.rrect_gradient(10.0, 40.0, 60.0, 20.0, 6.0, &[(0.0, [0, 200, 0, 255]), (1.0, [0, 100, 0, 255])]);
            g.radial_glow(40.0, 80.0, 20.0, [0, 0, 255, 200]);
            g.circle(100.0, 20.0, 8.0, [255, 255, 0, 255]);
            g.line(80.0, 40.0, 120.0, 40.0, 3.0, [255, 255, 255, 255]);
            g.text(10.0, 70.0, 14.0, [255, 255, 255, 255], "Hi", Face::Bold);
            g.icon(Icon::Settings, 100.0, 70.0, 20.0, [255, 255, 255, 255]);
            g.hit(R::new(10.0, 10.0, 60.0, 20.0), Event::Key(crate::ui::Key::Esc), "box");
            assert!(g.hovered(R::new(10.0, 10.0, 60.0, 20.0)));
        };
        let mut a = Gfx::new(160, 200);
        a.mouse = (20, 20);
        draw(&mut a);
        let mut b = Gfx::new(160, 200);
        b.mouse = (20, 20 + 90);
        b.set_origin_y(90.0);
        draw(&mut b);
        b.set_origin_y(0.0);
        // the picture of `a` (rows 0..110) is the picture of `b` shifted down by 90 rows: compare the rows that both fully contain
        let (pa, pb) = (a.rgba(), b.rgba());
        let row = 160 * 4;
        // anti-aliased edges may differ by a few levels (the path is moved by the transform, not rebuilt); nothing else may
        let mut worst = 0u8;
        let mut off = 0usize;
        for y in 0..100 {
            for i in 0..row {
                let d = pa[y * row + i].abs_diff(pb[(y + 90) * row + i]);
                worst = worst.max(d);
                off += usize::from(d > 0);
            }
        }
        assert!(worst <= 40 && off < 1200, "worst {worst}, {off} bytes differ");
        assert_eq!(b.hits[0].rect, R::new(10.0, 100.0, 60.0, 20.0));
        assert_eq!(b.probes()[0], R::new(10.0, 100.0, 60.0, 20.0));
        assert_eq!(b.origin_y(), 0.0);
    }

    #[test]
    fn clipping_and_a_text_mask_work_at_an_origin() {
        let mut g = Gfx::new(100, 200);
        g.clear([0, 0, 0, 255]);
        g.set_origin_y(100.0);
        g.clipped(R::new(10.0, 10.0, 30.0, 30.0), |g| g.rect(0.0, 0.0, 100.0, 100.0, [255, 0, 0, 255]));
        g.set_origin_y(0.0);
        let at = |x: usize, y: usize| g.rgba()[(y * 100 + x) * 4];
        assert_eq!((at(20, 120), at(5, 105), at(50, 120), at(20, 20)), (255, 0, 0, 0), "red only inside the clip, moved down");
        g.set_origin_y(50.0);
        let m = g.text_mask(14.0, "A", Face::Bold);
        assert!(m.w > 0 && m.alpha.iter().any(|a| *a > 0), "a mask is made as at the top");
    }
}
