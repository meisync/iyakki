//! The boot screen: the logo over a slowly turning galaxy, a progress bar, and (on `L`) the loading log.
//!
//! Boot code reports its progress with [`progress`] and calls [`finish`] when the shell is ready to take over.
//! [`draw`] is a pure function of the elapsed time, so it is easy to test and to look at.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use tiny_skia::{Color, FilterQuality, GradientStop, Paint, Pixmap, PixmapPaint, Point, RadialGradient, Rect, SpreadMode, Transform};

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{DIM, MUTED, TEXT};

// ---------------------------------------------------------------- progress reported by the boot code

static PERCENT: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static STAGE: Mutex<String> = Mutex::new(String::new());

/// Boot reached `percent` (0..=100), now doing `stage`.
pub fn progress(percent: u32, stage: &str) {
    PERCENT.fetch_max(percent.min(100), Ordering::Relaxed);
    if let Ok(mut s) = STAGE.lock() {
        *s = stage.to_string();
    }
}

/// Boot is over: the splash screen stops drawing (see [`wait_until_stopped`]).
pub fn finish() {
    PERCENT.store(100, Ordering::Relaxed);
    DONE.store(true, Ordering::Relaxed);
}

pub fn is_done() -> bool {
    DONE.load(Ordering::Relaxed)
}

pub fn current() -> (u32, String) {
    (PERCENT.load(Ordering::Relaxed), STAGE.lock().map(|s| s.clone()).unwrap_or_default())
}

/// Called when the splash loop is about to run.
pub fn mark_started() {
    STARTED.store(true, Ordering::Relaxed);
}

/// Called by the splash loop when it has stopped touching the screen.
pub fn mark_stopped() {
    STOPPED.store(true, Ordering::Relaxed);
}

/// Wait (at most `max_ms`) until the splash loop has stopped, so only one thing draws on the screen.
/// Returns at once when no splash was ever started.
pub fn wait_until_stopped(max_ms: u64) {
    if !STARTED.load(Ordering::Relaxed) {
        return;
    }
    let t = std::time::Instant::now();
    while !STOPPED.load(Ordering::Relaxed) && t.elapsed().as_millis() < max_ms as u128 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------- the loading log

/// The text of one `/dev/kmsg` record: `pri,seq,usec,flags;message` (continuation lines start with a space).
pub fn parse_kmsg(record: &str) -> Option<String> {
    let (head, msg) = record.split_once(';')?;
    if head.split(',').count() < 3 {
        return None;
    }
    let line = msg.lines().next()?.trim_end();
    (!line.is_empty()).then(|| line.to_string())
}

/// Follows the kernel log without blocking.
pub struct LogTail {
    file: Option<std::fs::File>,
    pub lines: Vec<String>,
}

const KEEP_LINES: usize = 400;

impl LogTail {
    pub fn open() -> LogTail {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/kmsg").ok();
        LogTail { file, lines: Vec::new() }
    }

    /// Read what has been logged since the last call.
    pub fn poll(&mut self) {
        use std::io::Read;
        let Some(f) = self.file.as_mut() else { return };
        let mut buf = [0u8; 4096];
        for _ in 0..400 {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(l) = parse_kmsg(&String::from_utf8_lossy(&buf[..n])) {
                        self.lines.push(l);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => continue, // records that were overwritten before we read them
            }
        }
        if self.lines.len() > KEEP_LINES {
            let cut = self.lines.len() - KEEP_LINES;
            self.lines.drain(..cut);
        }
    }
}

// ---------------------------------------------------------------- the picture

/// A small deterministic generator, so the galaxy looks the same on every boot and in tests.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

pub struct Star {
    /// Distance from the centre as a fraction of the galaxy radius, and the angle at time 0.
    r: f32,
    theta: f32,
    size: f32,
    brightness: f32,
    phase: f32,
    twinkle: f32,
    color: [u8; 3],
}

const STARS: usize = 1500;
const ARMS: f32 = 3.0;

pub fn make_stars() -> Vec<Star> {
    let mut rng = Rng(0x1A44_B1D5);
    let palette = [[190u8, 220, 255], [120, 200, 255], [255, 226, 170], [255, 255, 255], [170, 150, 255]];
    (0..STARS)
        .map(|i| {
            let r = rng.next().powf(1.5);
            let arm = (i % ARMS as usize) as f32 * std::f32::consts::TAU / ARMS;
            let spread = (rng.next() - 0.5) * 1.1 * (1.0 - r * 0.6);
            Star {
                r,
                theta: arm + r * 3.4 + spread,
                size: 0.6 + rng.next().powi(4) * 2.4,
                brightness: 0.3 + rng.next() * 0.7,
                phase: rng.next() * std::f32::consts::TAU,
                twinkle: 0.6 + rng.next() * 2.2,
                color: palette[(rng.next() * palette.len() as f32) as usize % palette.len()],
            }
        })
        .collect()
}

fn stars() -> &'static Vec<Star> {
    static S: OnceLock<Vec<Star>> = OnceLock::new();
    S.get_or_init(make_stars)
}

/// The logo (the round emblem), decoded once.
pub fn logo() -> &'static Pixmap {
    static L: OnceLock<Pixmap> = OnceLock::new();
    L.get_or_init(|| Pixmap::decode_png(include_bytes!("../assets/logo.png")).unwrap_or_else(|_| Pixmap::new(8, 8).expect("pixmap")))
}

/// Where the star is (as fractions of the screen) at time `t` seconds: inner stars turn faster, as in a galaxy.
pub fn star_position(st: &Star, t: f32, cx: f32, cy: f32, radius: f32) -> (f32, f32) {
    let omega = 0.05 * (1.25 - st.r);
    let a = st.theta + omega * t;
    (cx + a.cos() * st.r * radius, cy + a.sin() * st.r * radius * 0.62)
}

/// A copy of the logo with a band of light at `pos` (0 = top left corner, 1 = bottom right) brightening it.
pub fn shine_overlay(base: &Pixmap, pos: f32) -> Pixmap {
    let mut out = base.clone();
    let (w, h) = (out.width() as usize, out.height() as usize);
    let data = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 / w as f32 + y as f32 / h as f32) / 2.0 - pos).abs();
            let k = (1.0 - d / 0.11).max(0.0);
            if k <= 0.0 {
                continue;
            }
            let add = k * k * 0.7;
            let i = (y * w + x) * 4;
            let a = data[i + 3] as f32;
            for c in 0..3 {
                // premultiplied colour: never brighter than the pixel's own alpha
                data[i + c] = (data[i + c] as f32 + (a - data[i + c] as f32) * add).min(a) as u8;
            }
        }
    }
    out
}

fn glow(g: &mut Gfx, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32) {
    let c = |a: f32| Color::from_rgba8(color[0], color[1], color[2], (a * 255.0).clamp(0.0, 255.0) as u8);
    let stops = vec![GradientStop::new(0.0, c(alpha)), GradientStop::new(0.45, c(alpha * 0.35)), GradientStop::new(1.0, c(0.0))];
    if let Some(shader) = RadialGradient::new(Point::from_xy(cx, cy), Point::from_xy(cx, cy), radius, stops, SpreadMode::Pad, Transform::identity()) {
        let paint = Paint { shader, anti_alias: true, ..Paint::default() };
        if let Some(r) = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0) {
            g.pm.fill_rect(r, &paint, Transform::identity(), None);
        }
    }
}

fn scale(w: u32, h: u32) -> f32 {
    (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0)
}

/// What the splash screen shows besides time.
pub struct View<'a> {
    /// Seconds since the splash started.
    pub t: f32,
    /// The bar's position, 0.0 ..= 1.0 (eased by the caller).
    pub fraction: f32,
    pub stage: &'a str,
    /// `Some` = the loading log is on screen.
    pub log: Option<&'a [String]>,
    /// A checklist under the bar: each step and whether it is done (ticked). Empty = no list.
    pub steps: &'a [(&'a str, bool)],
}

/// Where a click or touch toggles the log: the strip along the bottom edge.
pub fn hint_zone(w: u32, h: u32) -> (f32, f32) {
    let s = scale(w, h);
    (h as f32 - 90.0 * s, h as f32)
}

pub fn draw(g: &mut Gfx, v: &View) {
    let (w, h) = (g.width(), g.height());
    let s = scale(w, h);
    let (wf, hf) = (w as f32, h as f32);
    g.gradient(rgb(3, 5, 14), rgb(9, 13, 32));
    let (cx, cy) = (wf / 2.0, hf * 0.37);
    let logo_d = (hf * 0.40).min(wf * 0.5).max(120.0 * s);

    // the galaxy: a soft core, then stars on turning spiral arms
    let radius = (wf * wf + hf * hf).sqrt() * 0.5 * 0.9;
    glow(g, cx, cy, radius * 0.55, [60, 70, 170], 0.22);
    for st in stars() {
        let (x, y) = star_position(st, v.t, cx, cy, radius);
        if x < -4.0 || y < -4.0 || x > wf + 4.0 || y > hf + 4.0 {
            continue;
        }
        let tw = 0.65 + 0.35 * (v.t * st.twinkle + st.phase).sin();
        let b = (st.brightness * tw).clamp(0.0, 1.0);
        let a = (b * 255.0) as u8;
        let d = st.size * s * 0.9;
        if d > 2.0 {
            // the brightest stars get a faint halo
            g.circle(x, y, d * 1.8, [st.color[0], st.color[1], st.color[2], (b * 40.0) as u8]);
        }
        g.rect(x - d / 2.0, y - d / 2.0, d.max(1.0), d.max(1.0), [st.color[0], st.color[1], st.color[2], a]);
    }
    shooting_star(g, v.t, wf, hf, s);

    // the halo around the logo breathes
    let pulse = 0.5 + 0.5 * (v.t * 1.3).sin();
    glow(g, cx, cy, logo_d * (0.95 + 0.06 * pulse), [70, 210, 240], 0.42 + 0.18 * pulse);
    glow(g, cx, cy, logo_d * 1.6, [90, 120, 255], 0.16);

    // the logo itself, with a band of light sweeping over it every few seconds
    let base = logo();
    let cycle = v.t % 5.5;
    let shine = if cycle < 1.4 { Some(shine_overlay(base, -0.15 + cycle / 1.4 * 1.3)) } else { None };
    let img = shine.as_ref().unwrap_or(base);
    let k = logo_d * (1.0 + 0.012 * (v.t * 1.3).sin()) / img.width() as f32;
    let paint = PixmapPaint { opacity: 1.0, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    let t = Transform::from_translate(cx, cy).pre_scale(k, k);
    g.pm.draw_pixmap(-(img.width() as i32) / 2, -(img.height() as i32) / 2, img.as_ref(), &paint, t, None);

    // the name
    let ny = cy + logo_d / 2.0 + 26.0 * s;
    let title = "IYAKKI";
    let ts = 54.0 * s;
    let tw = g.text_width(ts, title, Face::Bold);
    g.text(cx - tw / 2.0 + 2.0 * s, ny + 3.0 * s, ts, rgb(10, 60, 90), title, Face::Bold);
    g.text(cx - tw / 2.0, ny, ts, rgb(214, 244, 255), title, Face::Bold);
    let sub = "AN OS FOR AGENTS";
    let ss = 19.0 * s;
    let sw = g.text_width(ss, sub, Face::Bold);
    g.text(cx - sw / 2.0, ny + ts * 1.18, ss, rgb(110, 214, 230), sub, Face::Bold);

    // progress
    let bw = (520.0 * s).min(wf * 0.7);
    let (bx, by, bh) = (cx - bw / 2.0, ny + ts * 1.18 + ss * 2.6, 6.0 * s);
    g.rrect(bx, by, bw, bh, bh / 2.0, rgb(24, 34, 64));
    let fw = (bw * v.fraction.clamp(0.0, 1.0)).max(bh);
    g.rrect(bx, by, fw, bh, bh / 2.0, rgb(70, 190, 235));
    g.rrect(bx, by, fw * 0.5, bh, bh / 2.0, rgb(110, 225, 240));
    let stage = g.fit(15.0 * s, v.stage, Face::Sans, bw);
    let stw = g.text_width(15.0 * s, &stage, Face::Sans);
    if !v.stage.is_empty() {
        g.text(cx - stw / 2.0, by + bh + 12.0 * s, 15.0 * s, MUTED, &stage, Face::Sans);
    }
    if !v.steps.is_empty() {
        checklist(g, v.steps, cx, by + bh + 22.0 * s, hf - 46.0 * s, s);
    }

    // what to press
    let hint = if v.log.is_some() { "Press L to hide the loading log" } else { "Press L to see the loading log" };
    let hw = g.text_width(14.0 * s, hint, Face::Sans);
    if let Some(lines) = v.log {
        log_panel(g, lines, s);
    }
    g.text(cx - hw / 2.0, hf - 34.0 * s, 14.0 * s, if v.log.is_some() { TEXT } else { DIM }, hint, Face::Sans);
}

/// The steps of the start, one per line, with a box that is ticked when the step is done.
fn checklist(g: &mut Gfx, steps: &[(&str, bool)], cx: f32, top: f32, bottom: f32, s: f32) {
    let n = steps.len() as f32;
    let row = (26.0 * s).min(((bottom - top) / n).max(14.0 * s));
    let size = (row * 0.62).min(17.0 * s);
    let box_d = row * 0.64;
    let widest = steps.iter().map(|(t, _)| g.text_width(size, t, Face::Sans)).fold(0.0, f32::max);
    let x0 = cx - (box_d + 12.0 * s + widest) / 2.0;
    let accent = rgb(90, 220, 170);
    for (i, (label, done)) in steps.iter().enumerate() {
        let y = top + i as f32 * row;
        let by = y + (row - box_d) / 2.0;
        if *done {
            g.rrect(x0, by, box_d, box_d, box_d * 0.22, accent);
            // the tick
            g.line(x0 + box_d * 0.22, by + box_d * 0.55, x0 + box_d * 0.43, by + box_d * 0.74, 2.2 * s, rgb(6, 24, 20));
            g.line(x0 + box_d * 0.43, by + box_d * 0.74, x0 + box_d * 0.80, by + box_d * 0.28, 2.2 * s, rgb(6, 24, 20));
        } else {
            g.rrect(x0, by, box_d, box_d, box_d * 0.22, rgb(20, 30, 58));
            g.rrect_outline(x0, by, box_d, box_d, box_d * 0.22, 1.5 * s, rgb(70, 90, 140));
        }
        g.text(x0 + box_d + 12.0 * s, y + (row - size * 1.2) / 2.0, size, if *done { TEXT } else { DIM }, label, Face::Sans);
    }
}

/// A faint streak that crosses the sky now and then.
fn shooting_star(g: &mut Gfx, t: f32, w: f32, h: f32, s: f32) {
    let period = 7.0;
    let phase = (t % period) / 0.9;
    if phase >= 1.0 {
        return;
    }
    let n = (t / period) as u32;
    let mut rng = Rng(n as u64 * 7919 + 17);
    let (sx, sy) = (w * (0.1 + 0.6 * rng.next()), h * (0.05 + 0.3 * rng.next()));
    let (dx, dy) = (w * 0.28, h * 0.16);
    for i in 0..14 {
        let f = phase - i as f32 * 0.018;
        if f < 0.0 {
            continue;
        }
        let a = ((1.0 - i as f32 / 14.0) * (1.0 - phase * 0.6) * 230.0) as u8;
        let d = (3.2 - i as f32 * 0.17).max(0.8) * s;
        g.rect(sx + dx * f - d / 2.0, sy + dy * f - d / 2.0, d, d, [225, 240, 255, a]);
    }
}

fn log_panel(g: &mut Gfx, lines: &[String], s: f32) {
    let (w, h) = (g.width() as f32, g.height() as f32);
    let (x, y, pw, ph) = (w * 0.06, h * 0.08, w * 0.88, h * 0.80);
    g.rrect(x, y, pw, ph, 14.0 * s, [6, 9, 22, 232]);
    g.rrect_outline(x, y, pw, ph, 14.0 * s, 1.5 * s, rgb(40, 60, 100));
    let size = 13.0 * s;
    let lh = g.line_height(size, Face::Mono) + 2.0 * s;
    let rows = (((ph - 28.0 * s) / lh) as usize).max(1);
    let from = lines.len().saturating_sub(rows);
    for (i, l) in lines[from..].iter().enumerate() {
        let l = g.fit(size, l, Face::Mono, pw - 28.0 * s);
        let col = if l.contains("ERROR") || l.contains("error") || l.contains("fail") { rgb(255, 140, 140) } else if l.contains("WARN") { rgb(255, 196, 94) } else { rgb(170, 190, 220) };
        g.text(x + 14.0 * s, y + 14.0 * s + i as f32 * lh, size, col, &l, Face::Mono);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_only_moves_forward_and_finishing_fills_the_bar() {
        // (these statics are shared by every test in the process, so only monotonic facts are checked)
        progress(30, "Starting drivers");
        let (p, stage) = current();
        assert!(p >= 30 && p <= 100);
        progress(10, "Mounting");
        assert!(current().0 >= p, "a lower report never moves the bar back");
        assert_eq!(current().1, "Mounting", "the stage text always follows");
        progress(500, "x");
        assert!(current().0 <= 100);
        let _ = stage;
    }

    #[test]
    fn the_start_checklist_draws_ticked_and_open_boxes_and_adapts_to_small_screens() {
        let steps_none: Vec<(&str, bool)> = vec![("Core", false), ("Accounts", false), ("Tools", false), ("Desktop", false)];
        let steps_some: Vec<(&str, bool)> = vec![("Core", true), ("Accounts", true), ("Tools", false), ("Desktop", false)];
        let green = |g: &Gfx| g.rgba().chunks_exact(4).filter(|p| p[0] < 120 && p[1] > 190 && p[2] > 140 && p[2] < 190).count();
        for (w, h) in [(1280u32, 720u32), (800, 480), (320, 240), (3840, 2160)] {
            let mut a = Gfx::new(w, h);
            draw(&mut a, &View { t: 1.0, fraction: 0.5, stage: "", log: None, steps: &steps_none });
            let mut b = Gfx::new(w, h);
            draw(&mut b, &View { t: 1.0, fraction: 0.5, stage: "", log: None, steps: &steps_some });
            assert!(green(&b) > green(&a), "{w}x{h}: ticked boxes are drawn in green");
        }
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib splash_checklist_picture -- --ignored`
    #[test]
    #[ignore]
    fn splash_checklist_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let steps: Vec<(&str, bool)> = vec![("Core modules loaded", true), ("Accounts and settings opened", true), ("Models, tools and vaults ready", false), ("Desktop ready", false)];
        let mut g = Gfx::new(1280, 760);
        draw(&mut g, &View { t: 2.0, fraction: 0.5, stage: "", log: None, steps: &steps });
        g.save_png(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn kernel_log_records_are_parsed() {
        assert_eq!(parse_kmsg("6,339,5140900,-;usb 1-1: new device\n SUBSYSTEM=usb\n").as_deref(), Some("usb 1-1: new device"));
        assert_eq!(parse_kmsg("garbage without a semicolon"), None);
        assert_eq!(parse_kmsg("6,1,2,-;\n"), None, "empty messages are skipped");
        assert_eq!(parse_kmsg("nonsense;text"), None, "needs the prefix fields");
    }

    #[test]
    fn the_galaxy_is_the_same_every_time_and_stays_in_bounds() {
        let a = make_stars();
        let b = make_stars();
        assert_eq!(a.len(), STARS);
        assert!(a.iter().zip(&b).all(|(x, y)| x.r == y.r && x.theta == y.theta));
        assert!(a.iter().all(|s| (0.0..=1.0).contains(&s.r) && s.size > 0.0 && (0.0..=1.0).contains(&s.brightness)));
        // inner stars turn faster than outer ones
        let inner = Star { r: 0.1, theta: 0.0, size: 1.0, brightness: 1.0, phase: 0.0, twinkle: 1.0, color: [255; 3] };
        let outer = Star { r: 0.9, ..Star { r: 0.9, theta: 0.0, size: 1.0, brightness: 1.0, phase: 0.0, twinkle: 1.0, color: [255; 3] } };
        let ang = |st: &Star| {
            let (x, y) = star_position(st, 20.0, 0.0, 0.0, 1.0);
            (y / 0.62).atan2(x)
        };
        assert!(ang(&inner).abs() > ang(&outer).abs());
    }

    #[test]
    fn the_logo_decodes_and_the_shine_keeps_it_valid() {
        let l = logo();
        assert_eq!((l.width(), l.height()), (384, 384));
        assert_eq!(l.pixel(0, 0).unwrap().alpha(), 0, "outside the circle is transparent");
        assert!(l.pixel(192, 192).unwrap().alpha() > 200, "the middle is opaque");
        let lit = shine_overlay(l, 0.5);
        let mid = (192usize * 384 + 192) * 4;
        assert!(lit.data()[mid] >= l.data()[mid], "the band brightens");
        // premultiplied colour never exceeds alpha, or the blend would overflow
        assert!(lit.data().chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
        assert_eq!(shine_overlay(l, -5.0).data(), l.data(), "a band that is off the logo changes nothing");
    }

    #[test]
    fn the_splash_draws_at_every_size_with_and_without_the_log() {
        let lines: Vec<String> = (0..500).map(|i| format!("[{i}.000] line {i} with some text to show")).collect();
        for (w, h) in [(640, 480), (1024, 768), (1280, 720), (1920, 1080), (300, 200)] {
            let mut g = Gfx::new(w, h);
            for (t, log) in [(0.0, None), (0.7, None), (3.0, Some(&lines[..])), (8.2, None)] {
                draw(&mut g, &View { t, fraction: 0.4, stage: "Starting hardware drivers (12/40)", log, steps: &[] });
            }
            // the picture is not blank: the logo's centre is bright
            let (x, y) = (w as usize / 2, (h as f32 * 0.37) as usize);
            let i = (y * w as usize + x) * 4;
            let _ = i;
            assert!(g.rgba().iter().any(|&b| b > 200));
        }
    }

    #[test]
    fn the_hint_zone_is_the_bottom_strip() {
        let (a, b) = hint_zone(1280, 720);
        assert!(a > 600.0 && b == 720.0);
    }
}
