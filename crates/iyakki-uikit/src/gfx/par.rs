//! Opt-in recording and replay of the drawing calls of one frame, so the drawing itself can run on every CPU core (F-SHELL-64,
//! decision D-SHELL-PARALLEL-DRAW).
//!
//! How it works. [`Gfx::record_begin`] switches the layer on: from then on every drawing call (`rect`, `rrect`, `text`, gradients,
//! pictures, icons, `clipped`, ...) is written down as a small closure with the origin it was called at, and nothing touches the
//! pixels. Calls that only ask (`text_width`, `fit`, `wrap`, `hit`, `hovered`, `visible_right`) still answer at once, so the screen's
//! layout code is unchanged. [`Gfx::record_finish`] then cuts the picture into horizontal bands, gives each band to its own thread
//! (a private buffer seeded with the band's rows, the calls run in order, rows written back). A call that reads pixels back (`grab`,
//! `put`, `in_region`, `clipped_near`, `resize`) or code that touches `pm` directly first calls [`Gfx::record_sync`], which replays
//! what is written down so far on one thread, in order, so the result is the same either way. The layer is off unless a caller asks.
use super::*;

pub type RecFn = Box<dyn Fn(&mut Gfx) + Send + Sync>;

/// One written-down drawing call: the origin it was made at, the screen rows it can reach (a cheap bound, not exact) and the call.
pub struct RecOp {
    oy: f32,
    y0: f32,
    y1: f32,
    f: RecFn,
    /// Which call it is (the closure's type name): for test failures.
    #[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
    what: &'static str,
}

/// The band buffers kept between frames (their glyph caches stay warm).
#[derive(Default)]
pub struct BandPool {
    bands: Vec<Gfx>,
}

impl BandPool {
    pub fn new() -> BandPool {
        BandPool::default()
    }
}

/// Fewest rows a band is worth a thread for.
const MIN_BAND_ROWS: usize = 64;
/// Rows of the neighbouring bands each band buffer also draws (see `replay_banded`).
const BAND_MARGIN: usize = 48;

impl Gfx {
    /// Start writing drawing calls down instead of drawing them. Ends with [`Gfx::record_finish`] (or `record_cancel`).
    pub fn record_begin(&mut self) {
        let mut v = self.rec.take().unwrap_or_default();
        v.clear();
        self.rec = Some(v);
    }

    pub fn recording(&self) -> bool {
        self.rec.is_some()
    }

    /// Forget what is written down and switch the layer off (nothing is drawn).
    pub fn record_cancel(&mut self) {
        self.rec = None;
    }

    pub(super) fn rec_push<F: Fn(&mut Gfx) + Send + Sync + 'static>(&mut self, y0: f32, y1: f32, f: F) {
        let oy = self.oy;
        if let Some(v) = self.rec.as_mut() {
            v.push(RecOp { oy, y0: y0 + oy, y1: y1 + oy, what: std::any::type_name::<F>(), f: Box::new(f) });
        }
    }

    /// For a call that ignores the origin (its rows are screen rows).
    pub(super) fn rec_push_abs<F: Fn(&mut Gfx) + Send + Sync + 'static>(&mut self, y0: f32, y1: f32, f: F) {
        if let Some(v) = self.rec.as_mut() {
            v.push(RecOp { oy: 0.0, y0, y1, what: std::any::type_name::<F>(), f: Box::new(f) });
        }
    }

    /// Draw everything written down so far, now, on this thread, in order; the layer stays on. Call it before touching `pm` or reading
    /// pixels while a frame is being recorded (a no-op when the layer is off).
    pub fn record_sync(&mut self) {
        let Some(mut ops) = self.rec.take() else { return };
        let oy = self.oy;
        replay(self, &ops, 0, i32::MAX);
        self.oy = oy;
        ops.clear();
        self.rec = Some(ops);
    }

    /// Draw the frame: the written-down calls run on up to `threads` bands at once (one thread when the picture is small); the layer is
    /// switched off. The pixels are the same as drawing the calls directly.
    pub fn record_finish(&mut self, pool: &mut BandPool, threads: usize) {
        let Some(ops) = self.rec.take() else { return };
        let oy = self.oy;
        self.replay_banded(&ops, pool, threads);
        self.oy = oy;
    }

    fn replay_banded(&mut self, ops: &[RecOp], pool: &mut BandPool, threads: usize) {
        let (w, h) = (self.pm.width() as usize, self.pm.height() as usize);
        let n = threads.max(1).min(h / MIN_BAND_ROWS).max(1);
        // a flush in the middle of a clip leaves it open here: its end is in these calls, so the bands could not close it
        if n == 1 || ops.is_empty() || !self.clip_stack.is_empty() {
            replay(self, ops, 0, i32::MAX);
            return;
        }
        // rows of each band; each buffer also holds `BAND_MARGIN` rows of its neighbours on both sides, so a rounded shape that crosses the
        // edge between two bands is rasterised whole, as the single-thread frame does (the rasteriser cuts a curve at the edge of its
        // buffer, and the pieces come out a little different); only the band's own rows are written back
        let per = h.div_ceil(n);
        let heights: Vec<usize> = (0..n).map(|i| per.min(h.saturating_sub(i * per))).filter(|r| *r > 0).collect();
        let row = w * 4;
        // (first own row, own rows, rows of margin above, rows of margin below)
        let mut spans = Vec::with_capacity(heights.len());
        let mut y = 0usize;
        for &bh in &heights {
            spans.push((y, bh, BAND_MARGIN.min(y), BAND_MARGIN.min(h - y - bh)));
            y += bh;
        }
        while pool.bands.len() < spans.len() {
            pool.bands.push(Gfx::new(1, 1));
        }
        for (b, &(y0, bh, up, down)) in pool.bands.iter_mut().zip(&spans) {
            let rows = up + bh + down;
            if (b.pm.width() as usize, b.pm.height() as usize) != (w, rows) {
                *b = Gfx::new(w as u32, rows as u32);
            }
            b.rec = None;
            b.clip_stack.clear();
            let from = (y0 - up) * row;
            b.pm.data_mut().copy_from_slice(&self.pm.data()[from..from + rows * row]);
        }
        std::thread::scope(|sc| {
            let mut rest = self.pm.data_mut();
            for (b, &(y0, bh, up, _)) in pool.bands.iter_mut().zip(&spans) {
                let (mine, tail) = rest.split_at_mut(bh * row);
                rest = tail;
                sc.spawn(move || {
                    let rows = b.pm.height() as i32;
                    replay(b, ops, (y0 - up) as i32, rows);
                    mine.copy_from_slice(&b.pm.data()[up * row..(up + bh) * row]);
                });
            }
        });
    }

    /// Test aid: the first written-down call after which the banded replay and the one-thread replay of the calls so far differ, with
    /// the rows it reaches; the layer is switched off.
    #[cfg(any(test, feature = "testing"))]
    pub fn record_first_difference(&mut self, pool: &mut BandPool, threads: usize) -> Option<(usize, f32, f32, &'static str)> {
        let ops = self.rec.take()?;
        let base = self.pm.data().to_vec();
        let (mut lo, mut hi) = (0usize, ops.len());
        let differs = |g: &mut Gfx, pool: &mut BandPool, k: usize| {
            g.pm.data_mut().copy_from_slice(&base);
            g.clip_stack.clear();
            replay(g, &ops[..k], 0, i32::MAX);
            let one = g.pm.data().to_vec();
            g.pm.data_mut().copy_from_slice(&base);
            g.replay_banded(&ops[..k], pool, threads);
            g.clip_stack.clear();
            one != g.pm.data()
        };
        if !differs(self, pool, hi) {
            return None;
        }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if differs(self, pool, mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some((hi - 1, ops[hi - 1].y0, ops[hi - 1].y1, ops[hi - 1].what))
    }

    /// `clipped` while recording: the saving and putting back of the outside pixels become two written-down calls; the click regions
    /// are cut at once, as always.
    pub(super) fn clipped_recorded<T>(&mut self, clip: R, f: impl FnOnce(&mut Gfx) -> T) -> T {
        self.rec_push_abs(f32::NEG_INFINITY, f32::INFINITY, move |g: &mut Gfx| g.clip_begin_band(clip));
        let start = self.hits.len();
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.clips.push(clip);
        self.clip_now.push(clip);
        let out = f(self);
        self.clip_now.pop();
        #[cfg(any(test, feature = "testing"))]
        self.layout_log.clips.pop();
        self.rec_push_abs(f32::NEG_INFINITY, f32::INFINITY, |g: &mut Gfx| g.clip_end_band());
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

    /// Replay side of `clipped`: remember what lies outside `clip` (screen coordinates, shifted to this buffer's rows).
    fn clip_begin_band(&mut self, clip: R) {
        let clip = R::new(clip.x, clip.y - self.band_y as f32, clip.w, clip.h);
        let (w, h) = (self.pm.width() as usize, self.pm.height() as usize);
        let x0 = (clip.x.floor().max(0.0) as usize).min(w);
        let x1 = ((clip.x + clip.w).ceil().max(0.0) as usize).min(w).max(x0);
        let y0 = (clip.y.floor().max(0.0) as usize).min(h);
        let y1 = ((clip.y + clip.h).ceil().max(0.0) as usize).min(h).max(y0);
        let row = w * 4;
        let mut saved = Vec::new();
        let data = self.pm.data();
        saved.extend_from_slice(&data[..y0 * row]);
        saved.extend_from_slice(&data[y1 * row..h * row]);
        for y in y0..y1 {
            saved.extend_from_slice(&data[y * row..y * row + x0 * 4]);
            saved.extend_from_slice(&data[y * row + x1 * 4..(y + 1) * row]);
        }
        self.clip_stack.push((clip, saved));
    }

    fn clip_end_band(&mut self) {
        let Some((clip, saved)) = self.clip_stack.pop() else { return };
        let (w, h) = (self.pm.width() as usize, self.pm.height() as usize);
        let x0 = (clip.x.floor().max(0.0) as usize).min(w);
        let x1 = ((clip.x + clip.w).ceil().max(0.0) as usize).min(w).max(x0);
        let y0 = (clip.y.floor().max(0.0) as usize).min(h);
        let y1 = ((clip.y + clip.h).ceil().max(0.0) as usize).min(h).max(y0);
        let row = w * 4;
        let data = self.pm.data_mut();
        let mut at = 0;
        let mut put = |dst: &mut [u8]| {
            dst.copy_from_slice(&saved[at..at + dst.len()]);
            at += dst.len();
        };
        put(&mut data[..y0 * row]);
        put(&mut data[y1 * row..h * row]);
        for y in y0..y1 {
            put(&mut data[y * row..y * row + x0 * 4]);
            put(&mut data[y * row + x1 * 4..(y + 1) * row]);
        }
    }
}

/// Run the calls on `g` as the rows `band_y .. band_y + rows` of the screen (`g` is that many rows tall; `band_y` 0 and `rows`
/// `i32::MAX` is the whole screen). Calls that cannot reach the band are skipped, except the clip pairs, which always run.
fn replay(g: &mut Gfx, ops: &[RecOp], band_y: i32, rows: i32) {
    g.band_y = band_y;
    let (lo, hi) = (band_y as f32 - 1.0, band_y as f32 + rows as f32 + 1.0);
    for op in ops {
        if op.y1 < lo || op.y0 > hi {
            continue;
        }
        g.oy = op.oy;
        (op.f)(g);
    }
    g.band_y = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that uses every kind of call, at whole and at fractional positions, with origins, nested clips and a read-back in the middle.
    fn scene(g: &mut Gfx) {
        g.clear([20, 24, 40, 255]);
        g.gradient([30, 60, 120, 255], [10, 10, 30, 255]);
        g.rect(10.0, 10.0, 300.0, 40.0, [200, 80, 80, 255]);
        g.rect(5.5, 333.3, 640.0, 90.7, [60, 160, 90, 200]);
        g.rrect(40.25, 100.5, 333.3, 77.7, 14.0, [90, 90, 200, 230]);
        g.rrect_outline(60.0, 120.0, 200.5, 300.25, 9.0, 2.5, [250, 250, 250, 255]);
        g.squircle_gradient(300.0, 20.0, 120.0, 120.0, &[(0.0, [255, 200, 0, 255]), (1.0, [200, 0, 80, 255])]);
        g.rrect_gradient(30.0, 260.0, 400.0, 150.0, 20.0, &[(0.0, [255, 255, 255, 80]), (1.0, [0, 0, 0, 120])]);
        g.gradient_rect(0.0, 0.0, 640.0, 480.0, (0.0, 0.0), (640.0, 480.0), &[(0.0, [255, 0, 0, 40]), (1.0, [0, 0, 255, 40])]);
        g.radial_glow(320.0, 240.0, 150.0, [255, 255, 200, 120]);
        g.rrect_glow(200.0, 300.0, 180.0, 60.0, 12.0, 14.0, [100, 200, 255, 160]);
        g.circle(500.3, 100.7, 40.0, [255, 255, 0, 255]);
        g.ring(500.0, 220.0, 50.0, 5.0, [0, 255, 255, 255]);
        g.line(10.3, 10.1, 630.7, 470.9, 3.0, [255, 255, 255, 255]);
        g.polygon(&[(100.0, 380.0), (180.5, 420.2), (90.0, 470.0)], [255, 128, 0, 255]);
        g.polys(&[(vec![(400.0, 380.0), (460.0, 380.0), (430.0, 440.0)], true)], Some([0, 200, 0, 255]), Some((2.0, [255, 255, 255, 255])));
        for (i, ic) in [Icon::Lock, Icon::Chat].into_iter().enumerate() {
            g.icon(ic, 40.0 + 70.0 * i as f32, 440.3, 28.0, [255, 255, 255, 255]);
        }
        let w = g.text(20.0, 18.7, 17.0, [255, 255, 255, 255], "Hello, recorded world", Face::Sans);
        assert!(w > 50.0);
        g.text(20.0, 230.0, 22.0, [255, 255, 0, 255], "Straddles a band edge: मानक தமிழ்", Face::Bold);
        let (mut px, mut seed) = (vec![0u8; 8 * 6 * 4], 7u32);
        for b in px.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (seed >> 24) as u8;
        }
        g.rgba_picture(420.0, 150.0, 160.0, 120.0, 8, 6, &px);
        let pic = {
            let mut p = Pixmap::new(80, 60).unwrap();
            p.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 255));
            p
        };
        g.rrect_picture(30.0, 30.0, 240.0, 180.0, 16.0, &pic, 8);
        g.clipped(R::new(50.0, 90.0, 300.0, 200.0), |g| {
            g.rect(0.0, 0.0, 640.0, 480.0, [255, 0, 255, 90]);
            g.clipped(R::new(100.0, 140.0, 120.0, 60.0), |g| g.text(90.0, 150.0, 18.0, [0, 0, 0, 255], "inner clip text that is long", Face::Sans));
            g.text(60.0, 270.0, 18.0, [255, 255, 255, 255], "outer clip text running past the edge of the clip", Face::Sans);
        });
        // read-back in the middle of the frame
        let now = g.grab((100, 100, 80, 80));
        g.put((300, 300, 80, 80), &now);
        g.in_region((200, 200, 120, 120), None, |g, ox, oy| g.rect(200.0 - ox, 200.0 - oy, 60.0, 60.0, [255, 255, 255, 120]));
        g.clipped_near(R::new(250.0, 250.0, 60.0, 60.0), 6.0, |g| g.circle(280.0, 280.0, 40.0, [0, 0, 0, 100]));
        g.set_origin_y(37.5);
        g.rrect(20.0, 20.0, 200.0, 30.0, 6.0, [255, 255, 255, 200]);
        g.text(30.0, 24.0, 16.0, [0, 0, 0, 255], "top bar at an origin", Face::Sans);
        g.icon(Icon::Chat, 190.0, 35.0, 18.0, [0, 0, 0, 255]);
        g.set_origin_y(0.0);
        g.pm_mut().fill_rect(tiny_skia::Rect::from_xywh(600.0, 0.0, 10.0, 10.0).unwrap(), &paint([255, 0, 0, 255]), Transform::identity(), None);
    }

    fn drawn(threads: Option<usize>) -> Vec<u8> {
        let mut g = Gfx::new(640, 480);
        match threads {
            None => scene(&mut g),
            Some(n) => {
                let mut pool = BandPool::new();
                g.record_begin();
                scene(&mut g);
                g.record_finish(&mut pool, n);
                assert!(!g.recording());
            }
        }
        g.rgba().to_vec()
    }

    #[test]
    fn a_frame_drawn_in_bands_is_the_same_pixels_as_drawn_directly() {
        let direct = drawn(None);
        assert!(direct.iter().any(|b| *b != 0));
        for n in [1, 2, 3, 4, 7] {
            let par = drawn(Some(n));
            let diff = direct.iter().zip(&par).filter(|(a, b)| a != b).count();
            assert_eq!(diff, 0, "{n} bands: {diff} bytes differ from the single-thread frame");
        }
    }

    #[test]
    fn the_layer_is_off_until_asked_and_questions_are_answered_while_recording() {
        let mut g = Gfx::new(100, 100);
        assert!(!g.recording());
        let direct = g.text_width(16.0, "abc", Face::Sans);
        g.record_begin();
        assert!(g.recording());
        assert_eq!(g.text_width(16.0, "abc", Face::Sans), direct);
        let adv = g.text(0.0, 0.0, 16.0, [255; 4], "abc", Face::Sans);
        assert!((adv - direct).abs() < 0.01);
        assert!(g.rgba().iter().all(|b| *b == 0), "nothing is drawn until the frame is finished");
        g.record_sync();
        assert!(g.rgba().iter().any(|b| *b != 0), "sync draws what is written down");
        g.record_cancel();
        assert!(!g.recording());
    }
}
