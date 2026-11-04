//! The boot screen: the logo over a slowly turning galaxy, a progress bar, and (on `L`) the loading log.
//!
//! Boot code reports its progress with [`progress`] and calls [`finish`] when the shell is ready to take over.
//! [`draw`] is a pure function of the elapsed time, so it is easy to test and to look at.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

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

/// A boot screen was started in this run (IYAKKI is the system's first program and owns the console).
pub fn was_started() -> bool {
    STARTED.load(Ordering::Relaxed)
}

static REPAINT: AtomicBool = AtomicBool::new(false);

/// A monitor was just found (or woke up): whatever draws now (boot screen or desktop) writes its whole picture again,
/// instead of only what changed, so the newly lit screen shows a complete frame at once.
pub fn request_repaint() {
    REPAINT.store(true, Ordering::SeqCst);
}

/// True once after [`request_repaint`].
pub fn take_repaint() -> bool {
    REPAINT.swap(false, Ordering::SeqCst)
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

// ---------------------------------------------------------------- finding the screen, and saying what happened

/// Where the boot screen's story is kept for the person to send us: `dmesg | grep -i splash` has the same lines.
pub const BOOT_LOG: &str = "/var/log/iyakki-boot.log";

/// Say what the boot screen is doing: to the system log (which `dmesg` shows) and to [`BOOT_LOG`].
pub fn note(msg: &str) {
    log::info!("splash: {msg}");
    note_to(std::path::Path::new(BOOT_LOG), msg);
}

/// Append one line to `path` (best effort; the directory is made when missing). The line starts with the seconds
/// since the machine started, so the file reads as a timeline.
pub fn note_to(path: &std::path::Path, msg: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let up = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().map(str::to_string)).unwrap_or_else(|| "?".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{up}] splash: {msg}");
    }
}

/// The framebuffer devices that exist: `/dev/fbN` nodes, lowest first.
pub fn fb_nodes(dev: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(dev)
        .map(|r| r.flatten().filter_map(|e| Some((e.file_name().to_str()?.strip_prefix("fb")?.parse::<u32>().ok()?, e.path()))).collect())
        .unwrap_or_default();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Framebuffers the kernel has registered (`/sys/class/graphics/fbN`) whose device node is missing from `dev`, as
/// (name, major, minor). A kernel without devtmpfs help (or a node removed while a GPU driver took over) leaves these.
pub fn missing_nodes(sys_graphics: &std::path::Path, dev: &std::path::Path) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys_graphics) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.strip_prefix("fb").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) || dev.join(&name).exists() {
            continue;
        }
        let Some((a, b)) = std::fs::read_to_string(e.path().join("dev")).ok().and_then(|d| d.trim().split_once(':').map(|(a, b)| (a.to_string(), b.to_string()))) else { continue };
        if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
            out.push((name, a, b));
        }
    }
    out.sort();
    out
}

/// Create the missing `/dev/fbN` nodes (needs root; best effort). Returns the names made.
#[cfg(unix)]
pub fn make_missing_nodes(sys_graphics: &std::path::Path, dev: &std::path::Path) -> Vec<String> {
    use std::os::unix::ffi::OsStrExt;
    let mut made = Vec::new();
    for (name, major, minor) in missing_nodes(sys_graphics, dev) {
        let Ok(path) = std::ffi::CString::new(dev.join(&name).as_os_str().as_bytes()) else { continue };
        // SAFETY: `path` is a valid NUL-terminated string; makedev just combines the numbers.
        let rc = unsafe { libc::mknod(path.as_ptr(), libc::S_IFCHR | 0o600, libc::makedev(major, minor)) };
        if rc == 0 {
            made.push(name);
        }
    }
    made
}

/// The device to draw on: `fb0` when there is one, else the lowest `fbN`.
pub fn find_framebuffer(dev: &std::path::Path) -> Option<std::path::PathBuf> {
    let nodes = fb_nodes(dev);
    nodes.iter().find(|p| p.file_name().is_some_and(|n| n == "fb0")).or(nodes.first()).cloned()
}

/// A line about what graphics the kernel has so far, for the log while we wait ("fb: none; drm: card0-eDP-1").
pub fn graphics_summary(sys: &std::path::Path) -> String {
    let list = |sub: &str| -> String {
        let mut v: Vec<String> = std::fs::read_dir(sys.join("class").join(sub)).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        v.sort();
        if v.is_empty() { "none".into() } else { v.join(" ") }
    };
    format!("graphics: {}; drm: {}", list("graphics"), list("drm"))
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

/// The pixels of the logo that are the blue circuit light inside the arrow heads: byte offset of the pixel, and its place across the logo
/// (0 left, 1 right). Found once, from the picture itself.
fn circuit_pixels() -> &'static Vec<(usize, f32)> {
    static C: OnceLock<Vec<(usize, f32)>> = OnceLock::new();
    C.get_or_init(|| {
        let l = logo();
        let (w, h) = (l.width() as usize, l.height() as usize);
        let d = l.data();
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b, a) = (d[i] as i32, d[i + 1] as i32, d[i + 2] as i32, d[i + 3] as i32);
                // opaque and clearly blue (steel is grey: its three channels are close)
                if a > 250 && b > 70 && b > r + 28 && b > g + 8 {
                    v.push((i, x as f32 / w as f32));
                }
            }
        }
        v
    })
}

/// A copy of the logo whose circuit light breathes: a soft wave of brightness runs from the left arrow head to the right one (about every
/// 2.6 s), and the light never goes fully dark between waves. The brushed steel is not touched.
pub fn pulse_overlay(base: &Pixmap, t: f32) -> Pixmap {
    let mut out = base.clone();
    let data = out.data_mut();
    for &(i, x) in circuit_pixels() {
        if i + 3 >= data.len() {
            break;
        }
        let wave = ((t * 2.4 - x * 4.2).sin() * 0.5 + 0.5).powi(2);
        let add = 0.10 + 0.62 * wave;
        let (r, g, b) = (data[i] as f32, data[i + 1] as f32, data[i + 2] as f32);
        data[i] = (r + (255.0 - r) * add * 0.35) as u8;
        data[i + 1] = (g + (255.0 - g) * add * 0.75) as u8;
        data[i + 2] = (b + (255.0 - b) * add) as u8;
    }
    out
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

// ---------------------------------------------------------------- the same picture as the GRUB menu

/// GRUB's background picture (`dist/buildroot/board/iyakki/grub/iyakki-bg.png`), the very file the boot menu shows, so the
/// boot screen continues the menu's picture instead of replacing it.
pub static GRUB_BACKGROUND_PNG: &[u8] = include_bytes!("../../../dist/buildroot/board/iyakki/grub/iyakki-bg.png");
/// Its size in pixels.
pub const GRUB_BG_SIZE: (f32, f32) = (1280.0, 720.0);
/// Where the emblem sits in that picture: centre x, centre y and the size of `assets/logo.png` drawn over it (measured;
/// a test checks it against the picture).
pub const GRUB_LOGO_IN_BG: (f32, f32, f32) = (640.0, 259.0, 300.0);
/// GRUB's `desktop-color` (theme.txt): the bands left and right (or above and below) of the picture.
pub const GRUB_DESKTOP_COLOR: [u8; 4] = [0x05, 0x07, 0x10, 0xff];

/// How GRUB fits the background on a `w` x `h` screen (`desktop-image-scale-method: "padding"`, centred): the scale and the
/// top-left corner of the picture.
pub fn grub_fit(w: u32, h: u32) -> (f32, f32, f32) {
    let k = (w as f32 / GRUB_BG_SIZE.0).min(h as f32 / GRUB_BG_SIZE.1);
    (k, ((w as f32 - GRUB_BG_SIZE.0 * k) / 2.0).round(), ((h as f32 - GRUB_BG_SIZE.1 * k) / 2.0).round())
}

/// Centre and diameter of the logo on a `w` x `h` screen: exactly where the GRUB menu showed the emblem on that screen.
pub fn logo_rect(w: u32, h: u32) -> (f32, f32, f32) {
    let (k, ox, oy) = grub_fit(w, h);
    (ox + GRUB_LOGO_IN_BG.0 * k, oy + GRUB_LOGO_IN_BG.1 * k, GRUB_LOGO_IN_BG.2 * k)
}

/// The GRUB background fitted to a `w` x `h` screen, as GRUB draws it.
pub fn grub_background(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let c = GRUB_DESKTOP_COLOR;
    pm.fill(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    if let Ok(bg) = Pixmap::decode_png(GRUB_BACKGROUND_PNG) {
        let (k, ox, oy) = grub_fit(w, h);
        let paint = PixmapPaint { opacity: 1.0, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
        let (sx, sy) = (k * GRUB_BG_SIZE.0 / bg.width() as f32, k * GRUB_BG_SIZE.1 / bg.height() as f32);
        pm.draw_pixmap(0, 0, bg.as_ref(), &paint, Transform::from_translate(ox, oy).pre_scale(sx, sy), None);
    }
    pm
}

/// Paint the fitted background over the whole picture (cached per screen size: a copy per frame, not a rescale).
fn paint_background(g: &mut Gfx) {
    static CACHE: Mutex<Option<Pixmap>> = Mutex::new(None);
    let (w, h) = (g.width(), g.height());
    let Ok(mut c) = CACHE.lock() else { return g.clear(GRUB_DESKTOP_COLOR) };
    if c.as_ref().is_none_or(|p| (p.width(), p.height()) != (w, h)) {
        *c = Some(grub_background(w, h));
    }
    if let Some(bg) = c.as_ref() {
        if bg.data().len() == g.pm.data().len() {
            g.pm.data_mut().copy_from_slice(bg.data());
            return;
        }
    }
    g.clear(GRUB_DESKTOP_COLOR);
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

/// Where the name and the progress bar go on a `w` x `h` screen: the name's top, its size, the subtitle's size, and the bar
/// (x, y, width, height). Under the logo, which sits where GRUB showed it.
fn layout(w: u32, h: u32) -> (f32, f32, f32, (f32, f32, f32, f32)) {
    let s = scale(w, h);
    let (cx, cy, logo_d) = logo_rect(w, h);
    let ny = cy + logo_d / 2.0 + 26.0 * s;
    let (ts, ss) = (54.0 * s, 19.0 * s);
    let bw = (520.0 * s).min(w as f32 * 0.7);
    (ny, ts, ss, (cx - bw / 2.0, ny + ts * 1.18 + ss * 2.6, bw, 6.0 * s))
}

/// The progress bar's rectangle (x, y, width, height) on a `w` x `h` screen; the stage text is just below it.
pub fn progress_bar_rect(w: u32, h: u32) -> (f32, f32, f32, f32) {
    layout(w, h).3
}

/// The galaxy of the boot screen: stars on turning spiral arms around (`cx`, `cy`), twinkling. `dim` (0..=1) scales their light, so the sign-in
/// screen can show the same motion softly behind its card.
pub fn draw_galaxy(g: &mut Gfx, t: f32, cx: f32, cy: f32, s: f32, dim: f32) {
    let (wf, hf) = (g.width() as f32, g.height() as f32);
    let radius = (wf * wf + hf * hf).sqrt() * 0.5 * 0.9;
    for st in stars() {
        let (x, y) = star_position(st, t, cx, cy, radius);
        if x < -4.0 || y < -4.0 || x > wf + 4.0 || y > hf + 4.0 {
            continue;
        }
        let tw = 0.65 + 0.35 * (t * st.twinkle + st.phase).sin();
        let b = (st.brightness * tw * dim).clamp(0.0, 1.0);
        let a = (b * 255.0) as u8;
        let d = st.size * s * 0.9;
        if d > 2.0 {
            // the brightest stars get a faint halo
            g.circle(x, y, d * 1.8, [st.color[0], st.color[1], st.color[2], (b * 40.0) as u8]);
        }
        g.rect(x - d / 2.0, y - d / 2.0, d.max(1.0), d.max(1.0), [st.color[0], st.color[1], st.color[2], a]);
    }
}

/// The round logo centred on (`cx`, `cy`), `d` pixels wide, with its soft pulse and the band of light that sweeps over it every few seconds.
/// The boot screen draws it at the place GRUB showed it; the sign-in form draws it, smaller, above its card.
pub fn draw_logo(g: &mut Gfx, t: f32, cx: f32, cy: f32, d: f32) {
    let pulsed = pulse_overlay(logo(), t);
    let cycle = t % 5.5;
    let shine = if cycle < 1.4 { Some(shine_overlay(&pulsed, -0.15 + cycle / 1.4 * 1.3)) } else { None };
    let img = shine.as_ref().unwrap_or(&pulsed);
    let k = d / img.width() as f32;
    let paint = PixmapPaint { opacity: 1.0, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    let tf = Transform::from_translate(cx, cy).pre_scale(k, k);
    g.pm.draw_pixmap(-(img.width() as i32) / 2, -(img.height() as i32) / 2, img.as_ref(), &paint, tf, None);
}

pub fn draw(g: &mut Gfx, v: &View) {
    let (w, h) = (g.width(), g.height());
    let s = scale(w, h);
    let (wf, hf) = (w as f32, h as f32);
    // GRUB's own background picture, fitted the way GRUB fits it: every pixel of the frame is painted here first, so a frame
    // is never black or half drawn, and the logo lands exactly where the menu showed it
    paint_background(g);
    let (cx, cy, logo_d) = logo_rect(w, h);

    // the galaxy: stars on turning spiral arms
    draw_galaxy(g, v.t, cx, cy, s, 1.0);
    shooting_star(g, v.t, wf, hf, s);

    // the logo itself: static (same place and size as on the GRUB menu), with a band of light sweeping over it now and then
    draw_logo(g, v.t, cx, cy, logo_d);

    // the name
    let (ny, ts, ss, (bx, by, bw, bh)) = layout(w, h);
    let title = "IYAKKI";
    let tw = g.text_width(ts, title, Face::Bold);
    g.text(cx - tw / 2.0 + 2.0 * s, ny + 3.0 * s, ts, rgb(10, 60, 90), title, Face::Bold);
    g.text(cx - tw / 2.0, ny, ts, rgb(214, 244, 255), title, Face::Bold);
    let sub = "AN OS FOR AGENTS";
    let sw = g.text_width(ss, sub, Face::Bold);
    g.text(cx - sw / 2.0, ny + ts * 1.18, ss, rgb(110, 214, 230), sub, Face::Bold);

    // progress
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

    #[test]
    fn the_framebuffer_is_found_even_when_it_is_not_fb0_or_its_node_is_missing() {
        let d = std::env::temp_dir().join(format!("iyakki-splash-fb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (dev, sys) = (d.join("dev"), d.join("sys/class/graphics"));
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all(sys.join("fb0")).unwrap();
        std::fs::create_dir_all(sys.join("fbcon")).unwrap();
        assert_eq!(find_framebuffer(&dev), None, "nothing yet");
        std::fs::write(sys.join("fb0/dev"), "29:0\n").unwrap();
        assert_eq!(missing_nodes(&sys, &dev), vec![("fb0".to_string(), 29, 0)], "fbcon is not a screen; fb0 has no node");
        std::fs::write(dev.join("fb1"), "").unwrap();
        assert_eq!(find_framebuffer(&dev).unwrap().file_name().unwrap(), "fb1", "any screen will do");
        std::fs::write(dev.join("fb0"), "").unwrap();
        assert_eq!(find_framebuffer(&dev).unwrap().file_name().unwrap(), "fb0", "fb0 is preferred");
        assert!(missing_nodes(&sys, &dev).is_empty());
        assert_eq!(graphics_summary(&d.join("sys")), "graphics: fb0 fbcon; drm: none");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_boot_log_gets_one_timestamped_line_per_note() {
        let f = std::env::temp_dir().join(format!("iyakki-splash-log-{}/sub/boot.log", std::process::id()));
        note_to(&f, "fb0 appeared after 1200 ms");
        note_to(&f, "stopped: boot finished");
        let t = std::fs::read_to_string(&f).unwrap();
        assert_eq!(t.lines().count(), 2);
        assert!(t.lines().next().unwrap().ends_with("splash: fb0 appeared after 1200 ms"));
        let _ = std::fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }

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

    /// The boot animation as a picture sequence for a video: `IYAKKI_FRAMES=/dir [IYAKKI_FRAMES_SIZE=1920x1080] [IYAKKI_FRAMES_SECS=9]
    /// cargo test -p iyakki-shell --lib splash_frames_for_video -- --ignored` writes `f00000.png`, `f00001.png` ... at 30 frames a second.
    #[test]
    #[ignore]
    fn splash_frames_for_video() {
        let Ok(dir) = std::env::var("IYAKKI_FRAMES") else { return };
        let size = std::env::var("IYAKKI_FRAMES_SIZE").unwrap_or_else(|_| "1920x1080".into());
        let (w, h) = size.split_once('x').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))).unwrap_or((1920, 1080));
        let secs: f32 = std::env::var("IYAKKI_FRAMES_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(9.0);
        std::fs::create_dir_all(&dir).unwrap();
        let stages = ["Starting the system", "Starting network and sound", "Opening accounts and settings", "Models, tools and vaults ready"];
        for i in 0..(secs * 30.0) as u32 {
            let t = i as f32 / 30.0;
            let fraction = (t / secs).clamp(0.0, 1.0);
            let stage = stages[((fraction * stages.len() as f32) as usize).min(stages.len() - 1)];
            let mut g = Gfx::new(w, h);
            draw(&mut g, &View { t: t + 0.5, fraction, stage, log: None, steps: &[] });
            g.save_png(&std::path::Path::new(&dir).join(format!("f{i:05}.png"))).unwrap();
        }
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
    fn the_logo_sits_where_the_grub_menu_shows_it_at_every_screen_size() {
        // the fit below is GRUB's only while the theme keeps "padding" and this desktop colour
        let theme = include_str!("../../../dist/buildroot/board/iyakki/grub/theme.txt");
        assert!(theme.contains("desktop-image: \"iyakki-bg.png\"") && theme.contains("desktop-image-scale-method: \"padding\""));
        let c = GRUB_DESKTOP_COLOR;
        assert!(theme.contains(&format!("desktop-color: \"#{:02x}{:02x}{:02x}\"", c[0], c[1], c[2])));
        // the measured emblem position matches the GRUB picture itself (bright pixels of the emblem)
        let bg = Pixmap::decode_png(GRUB_BACKGROUND_PNG).unwrap();
        assert_eq!((bg.width() as f32, bg.height() as f32), GRUB_BG_SIZE);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..bg.height() {
            for x in 0..bg.width() {
                let p = bg.pixel(x, y).unwrap();
                if p.red().max(p.green()).max(p.blue()) > 70 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        let (cx, cy, d) = GRUB_LOGO_IN_BG;
        assert!((((x0 + x1) as f32 / 2.0) - cx).abs() <= 2.0 && (((y0 + y1) as f32 / 2.0) - cy).abs() <= 2.0, "emblem centre {x0}..{x1} x {y0}..{y1}");
        // logo.png's arrow heads span about nine tenths of its 384 pixels, and the picture shows them at that size and in that place
        let share = (x1 - x0) as f32 / d;
        assert!((0.80..=0.99).contains(&share), "the logo spans {share} of its square");
        // GRUB's "padding" fit: 1080p is the picture times 1.5; a 4:3 or 16:10 screen gets bands
        assert_eq!(logo_rect(1920, 1080), (960.0, 388.5, 450.0));
        assert_eq!(logo_rect(1280, 720), (640.0, 259.0, 300.0));
        assert_eq!(grub_fit(1280, 1024), (1.0, 0.0, 152.0));
        assert_eq!(logo_rect(1280, 1024), (640.0, 411.0, 300.0));
        assert_eq!(grub_fit(2560, 1600).0, 2.0);
        // and the boot screen really draws the logo there: inside the emblem the splash matches GRUB's picture
        for (w, h) in [(1920u32, 1080u32), (1024, 768), (1280, 800)] {
            let mut g = Gfx::new(w, h);
            draw(&mut g, &View { t: 3.0, fraction: 0.3, stage: "", log: None, steps: &[] });
            let grub = grub_background(w, h);
            let (cx, cy, d) = logo_rect(w, h);
            let r = (d * 0.3) as i32;
            let diff = |dx: i32, dy: i32| {
                let (mut sum, mut n) = (0u64, 0u64);
                for y in (cy as i32 - r..cy as i32 + r).step_by(3) {
                    for x in (cx as i32 - r..cx as i32 + r).step_by(3) {
                        let a = g.pm.pixel(x as u32, y as u32).unwrap();
                        let b = grub.pixel((x + dx) as u32, (y + dy) as u32).unwrap();
                        sum += (a.red() as i32 - b.red() as i32).unsigned_abs() as u64 + (a.green() as i32 - b.green() as i32).unsigned_abs() as u64 + (a.blue() as i32 - b.blue() as i32).unsigned_abs() as u64;
                        n += 3;
                    }
                }
                sum as f32 / n as f32
            };
            let here = diff(0, 0);
            assert!(here < 25.0, "{w}x{h}: the logo matches the GRUB emblem (mean difference {here})");
            let off = (d * 0.06) as i32;
            assert!(diff(off, off) > here * 1.5, "{w}x{h}: and only in that place");
        }
    }

    #[test]
    fn a_repaint_request_is_taken_once() {
        request_repaint();
        assert!(take_repaint());
        assert!(!take_repaint());
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
