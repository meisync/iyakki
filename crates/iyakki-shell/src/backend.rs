//! Event loop tying the shell to the framebuffer and evdev devices.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crate::fb::Framebuffer;
use crate::gfx::{rgb, Gfx};
use crate::input::{poll_devices, BackgroundScan, Device, InputState};
use crate::shell::Shell;
use crate::pointer_sprites::{shape_from_u8, shape_to_u8, sprite};
use crate::ui::Event;
use crate::ui::Pointer as Shape;

/// The input thread only reads devices and paints a small rectangle.
const INPUT_STACK: usize = 8 * 1024 * 1024;

const CURSOR: [&str; 17] = [
    "X", "XX", "X.X", "X..X", "X...X", "X....X", "X.....X", "X......X", "X.......X", "X........X", "X.....XXXXX", "X..X..X", "X.X X..X", "XX  X..X", "X    X..X", "     X..X", "      XX",
];

/// Draw an arrow pointer (white with a black outline) with its tip at (x, y).
pub fn draw_cursor(g: &mut Gfx, x: i32, y: i32) {
    for (row, line) in CURSOR.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            match ch {
                'X' => g.rect((x + col as i32) as f32, (y + row as i32) as f32, 1.0, 1.0, rgb(0, 0, 0)),
                '.' => g.rect((x + col as i32) as f32, (y + row as i32) as f32, 1.0, 1.0, rgb(255, 255, 255)),
                _ => {}
            }
        }
    }
}

/// While alive, the kernel text console stops drawing on the screen (no blinking cursor, no stray
/// kernel messages over our UI). Restored on drop. Best effort: needs a VT and permission.
pub(crate) struct GraphicsMode(Option<std::fs::File>);

impl GraphicsMode {
    const KDSETMODE: libc::c_ulong = 0x4B3A;
    const KDGETMODE: libc::c_ulong = 0x4B3B;

    /// Make sure the console is (still) in graphics mode, without ever leaving it: when a GPU driver replaces the firmware's
    /// screen the text console moves to the new screen, and it must not paint text over the picture there. Returns
    /// `Some(true)` when it already was, `Some(false)` when it had to be set again, `None` when there is no console.
    fn reassert() -> Option<bool> {
        use std::os::fd::AsRawFd;
        for path in ["/dev/tty0", "/dev/console"] {
            let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) else { continue };
            let mut mode: libc::c_int = -1;
            // SAFETY: KDGETMODE writes one int to the pointer given.
            if unsafe { libc::ioctl(f.as_raw_fd(), Self::KDGETMODE as _, &mut mode as *mut libc::c_int) } != 0 {
                continue;
            }
            if mode == 1 {
                return Some(true);
            }
            // SAFETY: KDSETMODE takes an integer argument (1 = KD_GRAPHICS).
            if unsafe { libc::ioctl(f.as_raw_fd(), Self::KDSETMODE as _, 1 as libc::c_ulong) } == 0 {
                return Some(false);
            }
        }
        None
    }

    pub(crate) fn enter() -> GraphicsMode {
        use std::os::fd::AsRawFd;
        for path in ["/dev/tty0", "/dev/console"] {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
                let mut mode: libc::c_int = -1;
                // SAFETY: KDGETMODE writes one int to the pointer given.
                let got = unsafe { libc::ioctl(f.as_raw_fd(), Self::KDGETMODE as _, &mut mode as *mut libc::c_int) } == 0;
                if !crate::fb::kd_set_needed(got.then_some(mode)) {
                    log::info!("console: already in graphics mode, KDSETMODE skipped ({path})");
                    return GraphicsMode(Some(f));
                }
                log::info!("console: KDSETMODE graphics set ({path}, was {})", if got { mode.to_string() } else { "unknown".into() });
                // SAFETY: KDSETMODE takes an integer argument (1 = KD_GRAPHICS).
                if unsafe { libc::ioctl(f.as_raw_fd(), Self::KDSETMODE as _, 1 as libc::c_ulong) } == 0 {
                    return GraphicsMode(Some(f));
                }
            }
        }
        GraphicsMode(None)
    }
}

impl Drop for GraphicsMode {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        if let Some(f) = &self.0 {
            // SAFETY: as above, 0 = KD_TEXT.
            unsafe { libc::ioctl(f.as_raw_fd(), Self::KDSETMODE as _, 0 as libc::c_ulong) };
        }
    }
}

/// Pointer size in whole pixels per sprite pixel: 1 on a 720p screen, 3 on 4K.
pub fn cursor_scale(w: u32, h: u32) -> usize {
    ((w as f32 / 1280.0).min(h as f32 / 720.0).round() as usize).clamp(1, 3)
}

/// Where the sprite of `shape` drawn for a pointer at (`x`, `y`) starts on the screen (its top-left corner, may be
/// negative: the hotspot sits on the pointer position) and its size, both in screen pixels. `None` for a hidden pointer.
fn sprite_box(shape: Shape, x: i32, y: i32, k: usize) -> Option<(i64, i64, i64, i64)> {
    let s = sprite(shape)?;
    let k = k.max(1) as i64;
    Some((x as i64 - s.hot.0 as i64 * k, y as i64 - s.hot.1 as i64 * k, s.w as i64 * k, s.h as i64 * k))
}

/// The on-screen rectangle (clipped to the screen) covered by the pointer of `shape` drawn at (`x`, `y`).
pub fn cursor_rect(shape: Shape, x: i32, y: i32, k: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (ox, oy, cw, ch) = sprite_box(shape, x, y, k)?;
    let (x0, y0) = (ox.max(0), oy.max(0));
    let (x1, y1) = ((ox + cw).min(w as i64), (oy + ch).min(h as i64));
    if x1 <= x0 || y1 <= y0 { None } else { Some((x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize)) }
}

/// A small RGBA patch of the scene under the pointer with the pointer painted on top, and where it goes.
pub fn cursor_patch(scene: &[u8], w: usize, h: usize, shape: Shape, x: i32, y: i32, k: usize) -> Option<((usize, usize, usize, usize), Vec<u8>)> {
    if scene.len() < w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }
    let r = cursor_rect(shape, x, y, k, w, h)?;
    let (rx, ry, rw, rh) = r;
    let mut patch = Vec::with_capacity(rw * rh * 4);
    for row in ry..ry + rh {
        patch.extend_from_slice(&scene[(row * w + rx) * 4..(row * w + rx + rw) * 4]);
    }
    paint_cursor(&mut patch, rw, rh, (rx as i32, ry as i32), shape, x, y, k);
    Some((r, patch))
}

/// Paint the pointer of `shape` (hotspot at screen position `x`, `y`) into `frame`, a `fw` x `fh` buffer whose top-left
/// corner is at screen position `origin`.
pub fn paint_cursor(frame: &mut [u8], fw: usize, fh: usize, origin: (i32, i32), shape: Shape, x: i32, y: i32, k: usize) {
    let (Some(sp), Some((ox, oy, _, _))) = (sprite(shape), sprite_box(shape, x, y, k)) else { return };
    let k = k.max(1) as i64;
    let (ox, oy) = (ox - origin.0 as i64, oy - origin.1 as i64);
    for (row, line) in sp.rows.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let rgb = match ch {
                'X' => [0, 0, 0, 255],
                '.' => [255, 255, 255, 255],
                _ => continue,
            };
            for dy in 0..k {
                for dx in 0..k {
                    let (px, py) = (ox + col as i64 * k + dx, oy + row as i64 * k + dy);
                    if px >= 0 && py >= 0 && (px as usize) < fw && (py as usize) < fh {
                        let i = (py as usize * fw + px as usize) * 4;
                        if let Some(dst) = frame.get_mut(i..i + 4) {
                            dst.copy_from_slice(&rgb);
                        }
                    }
                }
            }
        }
    }
}

/// The boot screen: draws [`crate::splash`] until boot calls [`crate::splash::finish`], then returns. `L` (or a touch
/// along the bottom edge) shows and hides the loading log. Follows the screen when a GPU driver replaces the simple
/// framebuffer the kernel started with.
pub fn run_splash(fb_path: &str) -> Result<(), String> {
    let r = splash_loop(fb_path);
    crate::splash::mark_stopped();
    r
}

/// What the boot screen keeps between frames: the picture and a copy of what is on the screen already.
///
/// A new screen (the GPU driver replacing the firmware's, or a monitor found later) gets its first frame drawn in full in
/// memory and then written whole: never a cleared (black) frame first, and no old-size picture.
pub struct SplashCanvas {
    gfx: Gfx,
    shown: Vec<u8>,
    /// How the last [`Framebuffer::scan_out`] went (after a whole frame), until [`SplashCanvas::take_scan_out`].
    scanned: Option<Result<(), String>>,
}

impl SplashCanvas {
    pub fn new(w: u32, h: u32) -> SplashCanvas {
        SplashCanvas { gfx: Gfx::new(w, h), shown: Vec::new(), scanned: None }
    }

    /// The result of putting the last whole frame on the monitor, once.
    pub fn take_scan_out(&mut self) -> Option<Result<(), String>> {
        self.scanned.take()
    }

    /// The screen changed (or must be repainted): the next frame writes every row.
    pub fn follow(&mut self, w: u32, h: u32) {
        if (self.gfx.width(), self.gfx.height()) != (w, h) {
            self.gfx = Gfx::new(w, h);
        }
        self.shown.clear();
    }

    /// Draw the frame and write what changed (everything after [`SplashCanvas::follow`]). Returns the rows written.
    /// A whole frame (the first on a screen) is then put on the monitor with [`Framebuffer::scan_out`]: a GPU driver's
    /// framebuffer is not shown until then (see there), and doing it after the write means the monitor goes from GRUB's
    /// picture straight to a complete boot screen.
    pub fn frame(&mut self, fb: &mut Framebuffer, view: &crate::splash::View) -> Result<usize, String> {
        crate::splash::draw(&mut self.gfx, view);
        let whole = self.shown.is_empty();
        let rows = fb.present_banded(self.gfx.rgba(), &mut self.shown)?;
        if whole {
            self.scanned = Some(fb.scan_out());
        }
        Ok(rows)
    }
}

/// A screen the boot screen stops using is never closed: it is usually the firmware's screen that a GPU driver just
/// unplugged, and the last close of that device frees its buffer, which on this kernel (6.12, simpledrm) spins forever in
/// the kernel (`shmem_undo_range` under `close`): the boot screen froze right after "screen changed" and a CPU core stayed
/// busy for good. Keeping it costs one buffer of memory (8 MB at 1920x1080).
fn keep_open(old: Framebuffer) {
    std::mem::forget(old);
}

fn splash_loop(fb_path: &str) -> Result<(), String> {
    use crate::splash::{self, LogTail, View};
    let mut fb = Framebuffer::open(fb_path)?;
    splash::note(&format!("fbset: iyakki.fbset={}", crate::fb::init_fbset_from_cmdline().word()));
    splash::note(&format!("drawing on {fb_path}: {}", fb.describe()));
    splash::note(&fb.display_line());
    // leave the text console off for good (the shell that follows keeps it so too): switching back to text would make
    // the console set its mode again, which on some GPUs is a full mode change and a dark screen
    std::mem::forget(GraphicsMode::enter());
    let mut canvas = SplashCanvas::new(fb.width, fb.height);
    crate::touchscreen::init_from_cmdline();
    let mut input = InputState::new(fb.width, fb.height);
    // the devices are found on a thread of its own: opening one the kernel is still setting up can block for seconds, and the
    // animation must not stop for it
    let mut devs: Vec<Device> = Vec::new();
    let scan = BackgroundScan::start();
    let mut log = LogTail::open();
    let mut show_log = false;
    let (start, mut last_check) = (Instant::now(), Instant::now());
    let mut fraction = 0.0f32;
    let mut frames = 0u32;
    let mut failures = 0u32;
    while !splash::is_done() {
        let frame_start = Instant::now();
        for (raw, abs, pad, touch) in poll_devices(&mut devs, 1) {
            for ev in match (pad, touch) { (Some(range), _) => input.feed_pad(raw, &range), (None, Some(t)) => input.feed_touch_full(raw, &t), _ => input.feed(raw, abs) } {
                match ev {
                    Event::Key(crate::ui::Key::Char('l' | 'L')) => show_log = !show_log,
                    Event::MouseDown(_, y) if (y as f32) >= splash::hint_zone(fb.width, fb.height).0 => show_log = !show_log,
                    _ => {}
                }
            }
        }
        scan.take_into(&mut devs); // the keyboard may appear after the drivers load
        // a GPU driver taking over is noticed within 100 ms (a cheap query, no mapping), and the very next frame on the new
        // screen is complete
        if last_check.elapsed() >= Duration::from_millis(100) {
            last_check = Instant::now();
            if fb.screen_changed(fb_path) {
                if let Ok(now) = Framebuffer::open(fb_path) {
                    let console = match GraphicsMode::reassert() {
                        Some(true) => "console still in graphics mode",
                        Some(false) => "console was back in text mode; graphics mode set again",
                        None => "no console",
                    };
                    splash::note(&format!("screen changed ({} -> {}), following it; {console}", fb.describe(), now.describe()));
                    splash::note(&now.display_line());
                    canvas.follow(now.width, now.height);
                    input = InputState::new(now.width, now.height);
                    keep_open(std::mem::replace(&mut fb, now));
                }
            }
        }
        if splash::take_repaint() {
            splash::note("a monitor was found; writing the whole picture again");
            canvas.follow(fb.width, fb.height);
        }
        log.poll();
        let (percent, stage) = splash::current();
        let target = percent as f32 / 100.0;
        fraction += (target - fraction) * 0.10;
        let lines: &[String] = &log.lines;
        let view = View { t: start.elapsed().as_secs_f32(), fraction, stage: &stage, log: show_log.then_some(lines), steps: &[] };
        if let Err(e) = canvas.frame(&mut fb, &view) {
            // the screen may be going away for a moment while a GPU driver takes over: look again instead of giving up
            failures += 1;
            if failures == 1 {
                splash::note(&format!("could not draw ({e}); looking for the screen again"));
            }
            if failures > 150 {
                return Err(format!("could not draw for {failures} frames: {e}"));
            }
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(now) = Framebuffer::open(fb_path) {
                canvas.follow(now.width, now.height);
                input = InputState::new(now.width, now.height);
                keep_open(std::mem::replace(&mut fb, now));
                let console = if GraphicsMode::reassert() == Some(false) { "; console was back in text mode, graphics mode set again" } else { "" };
                splash::note(&format!("screen is back: {}{console}", fb.describe()));
            }
            continue;
        }
        match canvas.take_scan_out() {
            Some(Ok(())) => splash::note(&format!("whole frame written and put on the monitor ({})", fb.describe())),
            Some(Err(e)) => splash::note(&format!("whole frame written; putting it on the monitor failed: {e}")),
            None => {}
        }
        failures = 0;
        frames += 1;
        // about 20 frames a second; a slow screen simply runs slower
        let spent = frame_start.elapsed();
        if spent < Duration::from_millis(50) {
            std::thread::sleep(Duration::from_millis(50) - spent);
        }
    }
    splash::note(&format!("stopped after {frames} frames in {:.1} s (boot finished)", start.elapsed().as_secs_f32()));
    if fb.screen_changed(fb_path) {
        keep_open(fb); // replaced since the last look: closing it could be the unplugged screen's last close
    } else if crate::fb::fbset() == crate::fb::FbSet::Skip {
        // iyakki.fbset=skip: the last close of /dev/fb0 (and the unmap) at the end of the boot screen is one of the candidates for KI-077
        splash::note("fbset=skip: /dev/fb0 left open at the end of the boot screen (no close, no unmap)");
        keep_open(fb);
    } else {
        splash::note("boot screen ends: /dev/fb0 is unmapped and closed now (iyakki.fbset=skip keeps it open)");
    }
    Ok(())
}

/// Frame timings, logged every few seconds so a slow machine can be diagnosed from its boot messages.
struct FrameStats {
    since: Instant,
    frames: u32,
    /// How many of `frames` painted only the live picture (the browser's page area) instead of the whole scene.
    live: u32,
    render: Duration,
    present: Duration,
    worst: Duration,
    /// The slowest of the tile-only frames (`worst` is the slowest of all): tells a hitch of the whole-scene draw from the live paint.
    worst_live: Duration,
}

impl FrameStats {
    fn new() -> FrameStats {
        FrameStats { since: Instant::now(), frames: 0, live: 0, render: Duration::ZERO, present: Duration::ZERO, worst: Duration::ZERO, worst_live: Duration::ZERO }
    }

    /// A frame that painted only the live picture: `draw` is the time to copy it into the scene.
    fn live_frame(&mut self, draw: Duration, present: Duration) {
        self.live += 1;
        self.worst_live = self.worst_live.max(draw + present);
        self.frame(draw, present);
    }

    fn frame(&mut self, render: Duration, present: Duration) {
        self.frames += 1;
        self.render += render;
        self.present += present;
        self.worst = self.worst.max(render + present);
        if self.since.elapsed() >= Duration::from_secs(10) {
            let n = self.frames.max(1);
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let sys = |p: &str| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into());
            log::info!(
                "shell: {} frames in {:.0}s, draw {:.1} ms, screen write {:.1} ms, slowest {:.0} ms (tile-only {:.0} ms), {} tile-only; cpu0 {} kHz, governor {}",
                self.frames,
                self.since.elapsed().as_secs_f64(),
                ms(self.render) / n as f64,
                ms(self.present) / n as f64,
                ms(self.worst),
                ms(self.worst_live),
                self.live,
                sys("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
                sys("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
            );
            *self = FrameStats::new();
        }
    }
}

/// What both threads of the display loop share: the screen, the last scene drawn and where the pointer is on it.
struct Screen {
    fb: Framebuffer,
    /// The last rendered scene (without the pointer): what is on screen apart from the pointer.
    scene: Vec<u8>,
    pointer_at: Option<(i32, i32)>,
    /// The shape drawn at `pointer_at` (what to erase when it moves or changes).
    shape: Shape,
    k: usize,
}

impl Screen {
    /// Erase the pointer from the screen (the rectangle of the shape drawn before), then draw `shape` at `to`.
    /// Does nothing when neither the position nor the shape changed.
    fn move_pointer(&mut self, to: Option<(i32, i32)>, shape: Shape) -> Result<(), String> {
        if self.scene.is_empty() || (to == self.pointer_at && shape == self.shape) {
            return Ok(());
        }
        let (wu, hu) = (self.fb.width as usize, self.fb.height as usize);
        self.take_pointer_off()?;
        if let Some((x, y)) = to {
            if let Some((r, patch)) = cursor_patch(&self.scene, wu, hu, shape, x, y, self.k) {
                self.fb.present_patch(&patch, r)?;
            }
        }
        self.pointer_at = to;
        self.shape = shape;
        Ok(())
    }

    fn take_pointer_off(&mut self) -> Result<(), String> {
        let (wu, hu) = (self.fb.width as usize, self.fb.height as usize);
        if let Some((x, y)) = self.pointer_at.take() {
            if let Some(r) = cursor_rect(self.shape, x, y, self.k, wu, hu) {
                self.fb.present_rect(&self.scene, r)?;
            }
        }
        Ok(())
    }
}

fn lock(m: &Mutex<Screen>) -> std::sync::MutexGuard<'_, Screen> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The pointer as the input thread last saw it.
struct Pointer {
    x: AtomicI32,
    y: AtomicI32,
    seen: AtomicBool,
    /// The shape the shell wants (a `pointer_sprites::shape_to_u8` byte), set by the drawing thread.
    shape: AtomicU8,
}

impl Pointer {
    fn shape(&self) -> Shape {
        shape_from_u8(self.shape.load(Ordering::Relaxed))
    }

    fn get(&self) -> Option<(i32, i32)> {
        self.seen.load(Ordering::Relaxed).then(|| (self.x.load(Ordering::Relaxed), self.y.load(Ordering::Relaxed)))
    }
}

/// Run until `stop` is set. Blocks; call from a dedicated thread.
///
/// Two threads keep a big (4K) screen responsive. The input thread reads the devices and moves the pointer at
/// once (by repainting a tiny rectangle), whatever the other thread is doing. The drawing thread handles the
/// events, re-renders the scene only when something changed (or every 100 ms for animations), and writes the
/// changed rows to the framebuffer, spread over all CPU cores.
pub fn run_fb(shell: &mut Shell, fb_path: &str, stop: &AtomicBool) -> Result<(), String> {
    // two or more displays on a machine that may use them: the shell drives them itself (`backend_kms.rs`); otherwise on as before
    if let crate::backend_kms::Outcome::Ran(r) = crate::backend_kms::maybe_run(shell, stop) {
        return r;
    }
    // a full-screen program (the optional full web engine) may ask for the screen: see `external`
    crate::external::set_supported(true);
    struct NotSupported;
    impl Drop for NotSupported {
        fn drop(&mut self) {
            crate::external::set_supported(false);
        }
    }
    let _not_supported_after = NotSupported;
    let remode = AtomicBool::new(false);
    shell.set_live_paint(true);
    let r = run_fb_once(shell, fb_path, stop, &remode);
    shell.set_live_paint(false);
    if r.is_ok() && remode.load(Ordering::SeqCst) && !stop.load(Ordering::Relaxed) {
        log::info!("shell: the screen mode changed while the full browser ran; starting the display again");
        return run_fb(shell, fb_path, stop);
    }
    r
}

/// Give the screen and the input devices to a separate program for as long as it runs, then take them back.
/// The display is unmapped and closed first (the program's compositor must become the display's master), the input
/// thread closes every device (nothing is typed into the shell meanwhile), and afterwards both are reopened.
/// Returns the program's result, and `Ok(false)` in the second place when the screen came back with another size.
#[allow(clippy::too_many_arguments)]
fn hand_over(screen: &Mutex<Screen>, paused: &AtomicBool, idle: &AtomicBool, fb_path: &str, size: (u32, u32), job: &crate::external::Job, stop: &AtomicBool) -> (Result<i32, String>, bool) {
    paused.store(true, Ordering::SeqCst);
    let t = Instant::now();
    while !idle.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    {
        let mut s = lock(screen);
        s.pointer_at = None;
        s.fb.release();
    }
    log::info!("shell: handing the screen to the full browser for {}", job.url);
    let result = iyakki_agent::webengine::run(&job.data, &job.user, &job.url, stop);
    log::info!("shell: the full browser ended: {result:?}");
    // the display driver needs a moment to hand the screen back to the framebuffer console
    let mut same = true;
    let mut reopened = false;
    for _ in 0..50 {
        match Framebuffer::open(fb_path) {
            Ok(fb) => {
                same = (fb.width, fb.height) == size;
                if same {
                    let mut s = lock(screen);
                    s.fb = fb;
                    s.scene.clear(); // the next frame is written in full
                }
                reopened = true;
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let result = if reopened { result } else { result.and(Err("the screen did not come back".to_string())) };
    paused.store(false, Ordering::SeqCst);
    (result, same && reopened)
}

/// Make the blank calls the shell asked for (the screen saver's screen-off stage and the wake), in order. Failures are logged:
/// the scene is black anyway, and waking never depends on them.
fn apply_blanks(shell: &mut Shell, screen: &Mutex<Screen>) {
    for off in shell.take_blank_requests() {
        let mut s = lock(screen);
        match crate::panel::blank_screen(&mut s.fb, off) {
            Ok(()) => log::info!("screen: framebuffer {}", if off { "powered down" } else { "unblanked" }),
            Err(e) => log::warn!("screen: {e}"),
        }
    }
}

fn run_fb_once(shell: &mut Shell, fb_path: &str, stop: &AtomicBool, remode: &AtomicBool) -> Result<(), String> {
    let t_start = Instant::now();
    let fb = Framebuffer::open(fb_path)?;
    crate::fb::init_fbset_from_cmdline();
    let t_open = t_start.elapsed();
    let graphics = GraphicsMode::enter();
    let t_mode = t_start.elapsed();
    // after a boot screen the console stays in graphics mode for good: going back to text when the shell stops or restarts
    // would make the console set its mode, a visible mode change (and text) on some GPUs
    let _graphics = if crate::splash::was_started() {
        std::mem::forget(graphics);
        None
    } else {
        Some(graphics)
    };
    let (w, h) = (fb.width, fb.height);
    log::info!("shell: {}", fb.display_line());
    let mut gfx = Gfx::new(w, h);
    let t_gfx = t_start.elapsed();
    // The first scan of the input devices is done by the input thread right after the first picture is on the screen: opening an
    // event device the kernel is still setting up can block for seconds (5.3 s seen on a Mac mini 2018 at boot), and the screen
    // must not wait for it.
    let mut devs: Vec<Device> = Vec::new();
    let t_scan = t_start.elapsed();
    log::info!("shell: {}x{} framebuffer, {} input device(s) so far (scanned in the background), {} CPU thread(s); display set up in {} ms (open {} ms, console mode {} ms, drawing surface {} ms, input scan {} ms)", w, h, devs.len(), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), t_scan.as_millis(), t_open.as_millis(), (t_mode - t_open).as_millis(), (t_gfx - t_mode).as_millis(), (t_scan - t_gfx).as_millis());
    let screen = Mutex::new(Screen { fb, scene: Vec::new(), pointer_at: None, shape: Shape::Default, k: cursor_scale(w, h) });
    // the machine's own screen: the saver may switch the backlight and the framebuffer's blank state; a screen a crashed shell left
    // dark is switched back on here
    shell.enable_panel_control();
    {
        let mut s = lock(&screen);
        if let Err(e) = crate::panel::blank_screen(&mut s.fb, false) {
            log::debug!("shell: unblank at start: {e}");
        }
    }
    let pointer = Pointer { x: AtomicI32::new(0), y: AtomicI32::new(0), seen: AtomicBool::new(false), shape: AtomicU8::new(0) };
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let screen_changed = AtomicBool::new(false);
    let quit = AtomicBool::new(false);
    let (paused, idle) = (AtomicBool::new(false), AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Event>();
    let fail = |e: String| {
        *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
        quit.store(true, Ordering::Relaxed);
    };

    std::thread::scope(|sc| {
        let input_thread = std::thread::Builder::new().name("iyakki-input".into()).stack_size(INPUT_STACK).spawn_scoped(sc, || {
            crate::touchscreen::init_from_cmdline();
            let mut input = InputState::new(w, h);
            let mut repeat = crate::input::RepeatApplier::default();
            let mut reopen_seen = iyakki_agent::touchrecover::reopen_generation();
            // the devices are found on a thread of its own, now and every 2 s: opening an event node the kernel is still setting up
            // (a hub that resets, a monitor's touch panel) can block for seconds, and this thread must keep reading the keyboard
            let mut bscan: Option<BackgroundScan> = None;
            while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
                if paused.load(Ordering::SeqCst) {
                    // the screen and keyboard belong to another program: hold nothing open
                    devs.clear();
                    bscan = None;
                    idle.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                if idle.swap(false, Ordering::SeqCst) {
                    input = InputState::new(w, h); // no key looks held down
                }
                let touch_keys = |d: &[crate::input::Device]| -> Vec<String> { d.iter().filter_map(|x| x.touch.as_ref().map(|t| t.key.clone())).collect() };
                let n0 = devs.len();
                bscan.get_or_insert_with(BackgroundScan::start).take_into(&mut devs); // keyboards and mice found since the last pass
                // a touch screen that came (again) starts clean (`InputState::touch_devices_changed`)
                for ev in input.touch_devices_changed(&[], &[], &touch_keys(&devs[n0..])) {
                    let _ = tx.send(ev);
                }
                repeat.tick(&devs);
                crate::input::apply_reopen(&mut devs, &mut reopen_seen);
                let held = touch_keys(&devs);
                for (raw, abs, pad, touch) in poll_devices(&mut devs, 10) {
                    for ev in match (pad, touch) { (Some(range), _) => input.feed_pad(raw, &range), (None, Some(t)) => input.feed_touch_full(raw, &t), _ => input.feed(raw, abs) } {
                        if matches!(ev, Event::MouseMove(..) | Event::MouseDown(..)) {
                            pointer.seen.store(true, Ordering::Relaxed);
                        }
                        let _ = tx.send(ev);
                    }
                }
                // a touch screen that left during this pass (dead nodes are dropped by the poll) lifts its finger
                for ev in input.touch_devices_changed(&held, &touch_keys(&devs), &[]) {
                    let _ = tx.send(ev);
                }
                // the smooth scroll steps due now (about 60 a second while a scroll goes on, none once it settles)
                for ev in input.scroll_tick() {
                    let _ = tx.send(ev);
                }
                pointer.x.store(input.x, Ordering::Relaxed);
                pointer.y.store(input.y, Ordering::Relaxed);
                if let Err(e) = lock(&screen).move_pointer(pointer.get(), pointer.shape()) {
                    fail(e);
                }
            }
        });
        if let Err(e) = input_thread {
            fail(format!("input thread: {e}"));
        }

        let mut last_render = Instant::now();
        let mut last_tick = Instant::now();
        let mut first = true;
        let mut first_picture = true;
        let mut stats = FrameStats::new();
        let mut last_look = Instant::now();
        while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
            let mut dirty = false;
            // wait for the next event, but wake for the tick
            // (while the open app wants to know about changes between the beats, such as the word being read aloud, only a few ms)
            let fast = shell.fast_poll();
            let wait = Duration::from_millis(if fast { 6 } else { 33 }).min(Duration::from_millis(33).saturating_sub(last_tick.elapsed()));
            let mut next = rx.recv_timeout(wait).ok();
            while let Some(ev) = next {
                shell.handle(&ev);
                dirty |= shell.take_stale();
                next = rx.try_recv().ok();
            }
            if last_tick.elapsed() >= Duration::from_millis(33) {
                last_tick = Instant::now();
                shell.handle(&Event::Tick);
                // an app with live pictures (the browser's full engine) asks for a redraw on the heartbeat
                dirty |= shell.take_stale();
            } else if fast {
                dirty |= shell.poll_live();
            }
            // the screen saver's power steps: the screen goes dark or comes back before anything is drawn (see `panel.rs`)
            apply_blanks(shell, &screen);
            // the shape the pointer should have (a hand over a link, ...): the input thread draws it; when it changes under a
            // still pointer, repaint just the pointer's rectangle here (the screen lock is held for that patch only)
            let want = shape_to_u8(shell.pointer());
            if pointer.shape.swap(want, Ordering::Relaxed) != want {
                if let Err(e) = lock(&screen).move_pointer(pointer.get(), shape_from_u8(want)) {
                    fail(e);
                }
            }
            if let Some(job) = crate::external::take() {
                // another program takes the screen: it must not find it dark
                shell.panel_shutdown();
                apply_blanks(shell, &screen);
                let (result, same_size) = hand_over(&screen, &paused, &idle, fb_path, (w, h), &job, stop);
                while rx.try_recv().is_ok() {} // whatever was typed before the hand-over
                crate::external::finish(result);
                shell.handle(&Event::Tick); // lets the app report how it went
                first = true;
                if !same_size {
                    remode.store(true, Ordering::SeqCst);
                    break;
                }
            }
            // a monitor plugged in or out, a mode change or a GPU driver taking over changes what is visible: start the display
            // again for the new size (sysfs only, no device access; once a second)
            if last_look.elapsed() >= Duration::from_secs(1) {
                last_look = Instant::now();
                let changed = lock(&screen).fb.screen_changed(fb_path);
                if changed {
                    log::info!("shell: the screen changed ({}); starting the display again", lock(&screen).fb.display_line());
                    screen_changed.store(true, Ordering::SeqCst);
                    remode.store(true, Ordering::SeqCst);
                    break;
                }
            }
            if crate::splash::take_repaint() {
                // a monitor was just found: write the whole picture on it, not only what changed
                let mut s = lock(&screen);
                s.scene.clear(); // the next write is the whole frame
                s.pointer_at = None; // drawn again on top of it
                drop(s);
                first = true;
            }
            // only the open app's live picture changed (a page scrolled or a video playing): paint just that rectangle
            let timed = last_render.elapsed() >= Duration::from_millis(shell.redraw_interval_ms());
            let mut tried_live = false;
            if !(dirty || first || timed) && shell.live_pending() && !lock(&screen).scene.is_empty() {
                tried_live = true;
                let t0 = Instant::now();
                if let Some((_, y, _, rh)) = shell.live_update(&mut gfx) {
                    let t1 = Instant::now();
                    let rects = shell.take_live_rects();
                    let mut s = lock(&screen);
                    let (y0, y1) = (y.max(0) as usize, (y + rh).max(0) as usize);
                    let res = s.take_pointer_off().and_then(|_| {
                        let Screen { fb, scene, .. } = &mut *s;
                        if !rects.is_empty() {
                            // several small rectangles (the highlighted words and the progress bar of a story): each is compared
                            // and copied over its own columns, not over whole rows
                            fb.present_rects(gfx.rgba(), scene, &rects).map(|_| ())
                        } else {
                            fb.present_banded_rows(gfx.rgba(), scene, y0, y1).map(|_| ())
                        }
                    });
                    let res = res.and_then(|_| s.move_pointer(pointer.get(), pointer.shape()));
                    drop(s);
                    stats.live_frame(t1 - t0, t1.elapsed());
                    crate::apps::browser_media::feed_screen(gfx.width(), gfx.height(), gfx.rgba());
                    if let Err(e) = res {
                        fail(e);
                    }
                    continue;
                }
            }
            if dirty || first || timed || tried_live {
                first = false;
                let t0 = Instant::now();
                shell.render(&mut gfx);
                let t1 = Instant::now();
                let mut s = lock(&screen);
                // the screen holds `scene` + pointer: take the pointer off, update the changed rows, put it back
                let res = s.take_pointer_off().and_then(|_| {
                    let Screen { fb, scene, .. } = &mut *s;
                    let whole = scene.is_empty();
                    fb.present_banded(gfx.rgba(), scene)?;
                    if whole {
                        // a whole frame: make sure it is the framebuffer the monitor shows (see `Framebuffer::scan_out`)
                        if let Err(e) = fb.scan_out() {
                            log::warn!("shell: {e}");
                        }
                    }
                    Ok(())
                });
                let res = res.and_then(|_| s.move_pointer(pointer.get(), pointer.shape()));
                drop(s);
                last_render = Instant::now();
                if first_picture {
                    first_picture = false;
                    // the boot counter of a system update (F-UPD-02): this start reached its first picture, so the boot menu keeps it
                    let data = shell.services_for_demo().dir().to_path_buf();
                    std::thread::spawn(move || {
                        if let Some(line) = iyakki_agent::devmode::centre::Centre::real(&data).confirm_boot(&iyakki_agent::devmode::centre::boot_id_now()) {
                            log::info!("shell: {line}");
                        }
                    });
                    // the end of "power on to sign-in screen": the kernel's own clock stamps this line (see dist/buildroot/boot-time.py)
                    log::info!("shell: first picture on screen ({}x{}, first frame took {} ms, {} ms after the display was opened)", w, h, (t1 - t0).as_millis(), t_start.elapsed().as_millis());
                }
                stats.frame(t1 - t0, last_render - t1);
                crate::apps::browser_media::feed_screen(gfx.width(), gfx.height(), gfx.rgba());
                if let Err(e) = res {
                    fail(e);
                }
            }
        }
        quit.store(true, Ordering::Relaxed);
    });
    // leaving: the backlight is put back and the screen switched on whatever stage it was in
    shell.panel_shutdown();
    apply_blanks(shell, &screen);
    if screen_changed.load(Ordering::SeqCst) {
        // never closed: it may be the firmware's screen a GPU driver just unplugged (see `keep_open`)
        std::mem::forget(screen);
    }
    let err = failed.lock().unwrap_or_else(|p| p.into_inner()).take();
    err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_screen_gets_a_complete_boot_screen_frame_and_never_a_black_one() {
        use crate::splash::View;
        let view = |t: f32| View { t, fraction: 0.5, stage: "Starting hardware drivers", log: None, steps: &[] };
        let black = |mem: &[u8]| mem.chunks_exact(4).filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0).count();
        // the firmware's screen (simpledrm) at one size, then the GPU driver's screen at another, starting out black
        let (aw, ah, bw, bh) = (480u32, 270u32, 640u32, 400u32);
        let mut a = vec![0u8; (aw * ah * 4) as usize];
        let mut b = vec![0u8; (bw * bh * 4) as usize];
        let mut canvas = SplashCanvas::new(aw, ah);
        let mut fa = Framebuffer::in_memory(&mut a, aw, ah);
        assert_eq!(canvas.frame(&mut fa, &view(0.0)).unwrap(), ah as usize, "first frame: every row");
        assert!(canvas.frame(&mut fa, &view(0.05)).unwrap() <= ah as usize);
        drop(fa);
        assert_eq!(black(&a), 0, "no black pixel on the first screen");
        // the screen changes: the very first write on it is a whole, drawn frame
        canvas.follow(bw, bh);
        let mut fb = Framebuffer::in_memory(&mut b, bw, bh);
        assert_eq!(canvas.frame(&mut fb, &view(0.1)).unwrap(), bh as usize, "every row of the new screen is written at once");
        drop(fb);
        assert_eq!(black(&b), 0, "not a single black pixel between the two screens");
        // the logo is where the GRUB menu had it on both screens: its centre is bright
        for (mem, w, h) in [(&a, aw, ah), (&b, bw, bh)] {
            let (cx, cy, _) = crate::splash::logo_rect(w, h);
            let i = (cy as usize * w as usize + cx as usize) * 4;
            assert!(mem[i..i + 3].iter().any(|&c| c > 60), "{w}x{h}: logo centre is lit");
        }
        // a repaint request (a monitor found later) writes the whole frame again on the same screen
        let mut fb = Framebuffer::in_memory(&mut b, bw, bh);
        canvas.follow(bw, bh);
        assert_eq!(canvas.frame(&mut fb, &view(0.15)).unwrap(), bh as usize);
    }

    /// The boot screen as it lands in screen memory of the sizes, strides and colour layouts real screens have (the Mac
    /// mini's 1920x1080 and 2560x1440, QEMU's 800x600 and 1280x720; BGRX as simpledrm and i915 use, RGBX), decoded back to RGB.
    #[test]
    fn the_boot_screen_lands_in_screen_memory_whole_at_every_size_stride_and_colour_layout() {
        use crate::fb::PixelFormat;
        use crate::splash::{logo_rect, progress_bar_rect, View};
        let bgrx = PixelFormat { bpp: 32, r: (16, 8), g: (8, 8), b: (0, 8) };
        let rgbx = PixelFormat { bpp: 32, r: (0, 8), g: (8, 8), b: (16, 8) };
        for (w, h) in [(800u32, 600u32), (1280, 720), (1920, 1080), (2560, 1440)] {
            for (name, fmt) in [("BGRX", bgrx), ("RGBX", rgbx)] {
                // a padded stride (as GPUs align rows), and memory that starts out as GRUB's leftovers: mid grey
                let pitch = w as usize * 4 + 256;
                let mut mem = vec![0x80u8; pitch * h as usize];
                let mut canvas = SplashCanvas::new(w, h);
                let mut fb = Framebuffer::in_memory_as(&mut mem, w, h, pitch, fmt);
                let view = View { t: 2.0, fraction: 0.5, stage: "Starting drivers…", log: None, steps: &[] };
                assert_eq!(canvas.frame(&mut fb, &view).unwrap(), h as usize, "{w}x{h} {name}: the first frame writes every row");
                drop(fb);
                let at = |x: u32, y: u32| -> [u8; 3] {
                    let i = y as usize * pitch + x as usize * 4;
                    let v = u32::from_le_bytes([mem[i], mem[i + 1], mem[i + 2], mem[i + 3]]);
                    [(v >> fmt.r.0) as u8, (v >> fmt.g.0) as u8, (v >> fmt.b.0) as u8]
                };
                let count = |x0: f32, y0: f32, x1: f32, y1: f32, pred: &dyn Fn([u8; 3]) -> bool| -> f32 {
                    let (mut n, mut hit) = (0u32, 0u32);
                    for y in (y0.max(0.0) as u32..(y1 as u32).min(h)).step_by(2) {
                        for x in (x0.max(0.0) as u32..(x1 as u32).min(w)).step_by(2) {
                            n += 1;
                            hit += pred(at(x, y)) as u32;
                        }
                    }
                    hit as f32 / n.max(1) as f32
                };
                let tag = format!("{w}x{h} {name}");
                // the emblem's cyan, where GRUB showed it
                let (cx, cy, d) = logo_rect(w, h);
                let cyan = count(cx - d / 2.0, cy - d / 2.0, cx + d / 2.0, cy + d / 2.0, &|p| p[1] > 150 && p[2] > 150 && p[0] < 200);
                assert!(cyan > 0.08, "{tag}: the logo's cyan ({cyan})");
                // the progress bar: half filled in the bar's blue, the rest its dark track
                let (bx, by, bw, bh) = progress_bar_rect(w, h);
                let mid = by + bh / 2.0;
                let fill = count(bx + bw * 0.1, mid - 1.0, bx + bw * 0.4, mid + 1.0, &|p| p[2] > 200 && p[1] > 160 && p[0] < 140);
                let track = count(bx + bw * 0.6, mid - 1.0, bx + bw * 0.9, mid + 1.0, &|p| p == [24, 34, 64]);
                assert!(fill > 0.9 && track > 0.9, "{tag}: the progress bar (fill {fill}, track {track})");
                // the stage text under the bar, and the name above it: light pixels where they are written
                let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
                let stage = count(bx, by + bh + 8.0 * s, bx + bw, by + bh + 36.0 * s, &|p| p.iter().all(|&c| c > 110));
                assert!(stage > 0.01, "{tag}: the stage text ({stage})");
                let name = count(cx - 160.0 * s, cy + d / 2.0 + 26.0 * s, cx + 160.0 * s, cy + d / 2.0 + 80.0 * s, &|p| p[0] > 190 && p[1] > 220);
                assert!(name > 0.05, "{tag}: the name IYAKKI ({name})");
                // stars around (the animation), and nothing black or left over anywhere: GRUB's picture and colour at worst
                let stars = count(0.0, 0.0, w as f32, h as f32, &|p| p.iter().any(|&c| c > 150));
                assert!(stars > 0.002, "{tag}: stars ({stars})");
                for gy in 0..8 {
                    for gx in 0..8 {
                        let (x0, y0) = (w as f32 * gx as f32 / 8.0, h as f32 * gy as f32 / 8.0);
                        let black = count(x0, y0, x0 + w as f32 / 8.0, y0 + h as f32 / 8.0, &|p| p.iter().all(|&c| c < 3));
                        let grey = count(x0, y0, x0 + w as f32 / 8.0, y0 + h as f32 / 8.0, &|p| p == [0x80, 0x80, 0x80]);
                        assert!(black < 0.01 && grey == 0.0, "{tag}: cell {gx},{gy} is black ({black}) or not painted ({grey})");
                    }
                }
                // the padding past each row is the driver's, untouched
                assert!((0..h as usize).step_by(97).all(|y| mem[y * pitch + w as usize * 4..(y + 1) * pitch].iter().all(|&b| b == 0x80)), "{tag}: stride padding untouched");
            }
        }
    }

    #[test]
    fn our_display_code_never_sets_a_mode_or_switches_consoles_and_pans_only_in_place() {
        // FBIOPUT_VSCREENINFO 0x4601, FBIOPAN_DISPLAY 0x4606, FBIOBLANK 0x4611, VT_ACTIVATE 0x5606, VT_SETMODE 0x5602; KDSETMODE
        // only ever with 1 (graphics) outside of the restore at the very end of a shell that had no boot screen
        for (name, src) in [("backend.rs", include_str!("backend.rs")), ("fb.rs", include_str!("fb.rs")), ("splash.rs", include_str!("splash.rs"))] {
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
            for bad in ["0x4601", "0x4611", "0x5606", "0x5602", "FBIOPUT", "VT_ACTIVATE"] {
                assert!(!code.contains(bad), "{name} must not use {bad}");
            }
        }
        // the one pan: to the offsets the screen already has (read just before), in `scan_out`, which puts the framebuffer
        // on the monitor after a GPU driver took over while the console was in graphics mode
        let fb = include_str!("fb.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        assert_eq!(fb.matches("0x4606").count(), 1);
        assert_eq!(fb.matches("FBIOPAN_DISPLAY as _").count(), 1);
        let pan = fb.split("pub fn scan_out").nth(1).unwrap();
        assert!(pan.find("FBIOGET_VSCREENINFO as _").unwrap() < pan.find("FBIOPAN_DISPLAY as _").unwrap(), "the pan reuses the current settings");
        assert!(!include_str!("backend.rs").split("#[cfg(test)]\nmod tests").next().unwrap().contains("FBIOPAN"));
        let code = include_str!("backend.rs").split("#[cfg(test)]\nmod tests").next().unwrap().to_string();
        assert_eq!(code.matches("0 as libc::c_ulong").count(), 1, "text mode only in the guard's drop");
        assert!(code.contains("if crate::splash::was_started()"), "after a boot screen the guard is never dropped");
    }

    #[test]
    fn the_pointer_scales_with_the_screen() {
        assert_eq!(cursor_scale(1280, 720), 1);
        assert_eq!(cursor_scale(1920, 1080), 2, "1.5 rounds up");
        assert_eq!(cursor_scale(3840, 2160), 3);
        assert_eq!(cursor_scale(640, 480), 1);
        assert_eq!(cursor_scale(7680, 4320), 3, "capped");
    }

    use crate::pointer_sprites::{sprite, ALL};

    #[test]
    fn the_pointer_rectangle_is_clipped_to_the_screen() {
        let a = Shape::Default;
        assert_eq!(cursor_rect(a, 10, 10, 1, 100, 100), Some((10, 10, 12, 17)));
        assert_eq!(cursor_rect(a, 95, 95, 1, 100, 100), Some((95, 95, 5, 5)));
        assert_eq!(cursor_rect(a, -5, -5, 1, 100, 100), Some((0, 0, 7, 12)));
        assert_eq!(cursor_rect(a, 100, 50, 1, 100, 100), None, "fully off to the right");
        assert_eq!(cursor_rect(a, 10, 10, 3, 1000, 1000), Some((10, 10, 36, 51)));
        assert_eq!(cursor_rect(Shape::Hidden, 10, 10, 1, 100, 100), None, "a hidden pointer covers nothing");
    }

    #[test]
    fn a_patch_has_the_scene_with_the_pointer_on_top() {
        let (w, h) = (40usize, 40usize);
        let scene = vec![50u8; w * h * 4];
        let (r, patch) = cursor_patch(&scene, w, h, Shape::Default, 5, 5, 1).unwrap();
        assert_eq!(r, (5, 5, 12, 17));
        assert_eq!(patch.len(), 12 * 17 * 4);
        assert_eq!(&patch[0..4], &[0, 0, 0, 255], "the tip is the outline colour");
        let transparent = &patch[(0 * 12 + 11) * 4..(0 * 12 + 11) * 4 + 4];
        assert_eq!(transparent, &[50, 50, 50, 50], "pixels outside the arrow keep the scene");
        // scaled pointer paints k x k blocks
        let (r3, p3) = cursor_patch(&scene, w, h, Shape::Default, 0, 0, 2).unwrap();
        assert_eq!(r3, (0, 0, 24, 34));
        assert_eq!(&p3[0..4], &[0, 0, 0, 255]);
        assert_eq!(&p3[(1 * 24 + 1) * 4..(1 * 24 + 1) * 4 + 4], &[0, 0, 0, 255], "the first sprite pixel is 2x2");
        // a pointer near the edge still produces a clipped patch
        let (r2, p2) = cursor_patch(&scene, w, h, Shape::Default, 36, 36, 1).unwrap();
        assert_eq!((r2, p2.len()), ((36, 36, 4, 4), 4 * 4 * 4));
        assert!(cursor_patch(&scene[..10], w, h, Shape::Hand, 5, 5, 1).is_none(), "a scene that is too short is never indexed");
    }

    #[test]
    fn every_sprite_is_well_formed() {
        for shape in ALL {
            let Some(sp) = sprite(shape) else {
                assert_eq!(shape, Shape::Hidden, "only the hidden pointer has no sprite");
                continue;
            };
            assert_eq!(sp.h as usize, sp.rows.len(), "{shape:?}");
            assert!((7..=20).contains(&sp.w) && (7..=20).contains(&sp.h), "{shape:?}: about 16-20 px, not {}x{}", sp.w, sp.h);
            let widest = sp.rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as i32;
            assert!(widest <= sp.w && sp.w <= widest + 1, "{shape:?}: width {} vs rows {}", sp.w, widest);
            for r in sp.rows {
                assert!(r.chars().all(|c| matches!(c, 'X' | '.' | ' ')), "{shape:?}: stray character in {r:?}");
            }
            assert!(sp.hot.0 >= 0 && sp.hot.0 < sp.w && sp.hot.1 >= 0 && sp.hot.1 < sp.h, "{shape:?}: hotspot inside the sprite");
            assert!(sp.rows.iter().any(|r| r.contains('X')) && sp.rows.iter().any(|r| r.contains('.')), "{shape:?}: outline and fill");
        }
        // the arrow is exactly today's sprite, tip on the hotspot
        assert_eq!(CURSOR.to_vec(), sprite(Shape::Default).unwrap().rows.to_vec());
        assert_eq!(sprite(Shape::Default).unwrap().hot, (0, 0));
    }

    #[test]
    fn the_shape_byte_is_total_and_round_trips() {
        for (i, shape) in ALL.iter().enumerate() {
            assert_eq!(shape_to_u8(*shape) as usize, i);
            assert_eq!(shape_from_u8(shape_to_u8(*shape)), *shape);
        }
        for b in 0..=255u8 {
            let s = shape_from_u8(b);
            if (b as usize) < ALL.len() { assert_eq!(shape_to_u8(s), b) } else { assert_eq!(s, Shape::Default, "unknown bytes are the arrow") }
        }
    }

    #[test]
    fn the_rectangle_of_every_shape_follows_its_hotspot_at_every_scale_and_edge() {
        let (w, h) = (200usize, 120usize);
        for shape in ALL {
            let Some(sp) = sprite(shape) else { continue };
            for k in 1..=3usize {
                let ki = k as i32;
                for (x, y) in [(100, 60), (0, 0), (w as i32 - 1, h as i32 - 1), (0, h as i32 - 1), (w as i32 - 1, 0), (-3, -3), (-200, 50), (w as i32 + 5, 50), (i32::MAX, i32::MIN)] {
                    let got = cursor_rect(shape, x, y, k, w, h);
                    // brute force: the screen pixels the sprite box covers
                    let (ox, oy) = (x as i64 - (sp.hot.0 * ki) as i64, y as i64 - (sp.hot.1 * ki) as i64);
                    let (x0, y0) = (ox.max(0), oy.max(0));
                    let (x1, y1) = ((ox + (sp.w * ki) as i64).min(w as i64), (oy + (sp.h * ki) as i64).min(h as i64));
                    let want = (x1 > x0 && y1 > y0).then(|| (x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize));
                    assert_eq!(got, want, "{shape:?} k={k} at ({x},{y})");
                    if let Some((rx, ry, rw, rh)) = got {
                        assert!(rx + rw <= w && ry + rh <= h);
                    }
                }
                // the hotspot pixel is the one under the pointer
                let r = cursor_rect(shape, 100, 60, k, w, h).unwrap();
                assert!(r.0 as i32 <= 100 && 100 < (r.0 + r.2) as i32 && r.1 as i32 <= 60 && 60 < (r.1 + r.3) as i32, "{shape:?} k={k}: hotspot inside the rectangle");
            }
        }
    }

    #[test]
    fn tiny_screens_and_odd_numbers_never_panic() {
        for shape in ALL {
            for (w, h) in [(0usize, 0usize), (1, 1), (3, 40), (40, 3), (7, 7)] {
                let scene = vec![9u8; w * h * 4];
                for (x, y) in [(0, 0), (-5, 4), (2, 2), (i32::MAX, 0), (0, i32::MIN)] {
                    for k in [0usize, 1, 3] {
                        let _ = cursor_rect(shape, x, y, k, w, h);
                        if let Some((r, patch)) = cursor_patch(&scene, w, h, shape, x, y, k) {
                            assert_eq!(patch.len(), r.2 * r.3 * 4);
                        }
                    }
                }
            }
        }
        let mut small = vec![0u8; 4 * 4 * 4];
        paint_cursor(&mut small, 4, 4, (0, 0), Shape::Move, 2, 2, 3);
        paint_cursor(&mut small, 100, 100, (0, 0), Shape::Move, 2, 2, 3); // a frame buffer shorter than it says: skipped, not a panic
    }

    /// The colour (r, g, b) of screen pixel (x, y) of a BGRX screen memory.
    fn px(mem: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 4;
        [mem[i + 2], mem[i + 1], mem[i]]
    }

    #[test]
    fn the_pixel_under_the_hotspot_is_the_same_for_every_shape() {
        let (w, h) = (120usize, 90usize);
        let scene = vec![50u8; w * h * 4];
        for shape in ALL {
            let Some(sp) = sprite(shape) else { continue };
            for k in 1..=3usize {
                let (x, y) = (60i32, 45i32);
                let (r, patch) = cursor_patch(&scene, w, h, shape, x, y, k).unwrap();
                // the sprite pixel at the hotspot lands on the pointer position (its k x k block starts there)
                let ch = sp.rows[sp.hot.1 as usize].chars().nth(sp.hot.0 as usize).unwrap_or(' ');
                let i = ((y as usize - r.1) * r.2 + (x as usize - r.0)) * 4;
                let want: &[u8] = match ch {
                    'X' => &[0, 0, 0, 255],
                    '.' => &[255, 255, 255, 255],
                    _ => &[50, 50, 50, 50],
                };
                assert_eq!(&patch[i..i + 4], want, "{shape:?} k={k}: pixel under the hotspot");
            }
        }
    }

    /// A screen over memory, with a scene of one flat colour on it and no pointer.
    fn screen_over(mem: &mut [u8], w: u32, h: u32, k: usize) -> Screen {
        let mut fb = Framebuffer::in_memory(mem, w, h);
        let scene = vec![50u8; (w * h * 4) as usize];
        fb.present_rect(&scene, (0, 0, w as usize, h as usize)).unwrap();
        Screen { fb, scene, pointer_at: None, shape: Shape::Default, k }
    }

    #[test]
    fn a_shape_change_under_a_still_pointer_erases_the_old_and_paints_the_new_pixel_exactly() {
        let (w, h) = (64u32, 48u32);
        for k in 1..=3usize {
            for at in [(30, 20), (0, 0), (-4, 10), (63, 47), (60, 3)] {
                let mut mem = vec![0u8; (w * h * 4) as usize];
                let (ptr, len) = (mem.as_ptr(), mem.len());
                let mut s = screen_over(&mut mem, w, h, k);
                for shape in [Shape::Default, Shape::Hand, Shape::Text, Shape::Hidden, Shape::NotAllowed, Shape::Wait, Shape::Default] {
                    s.move_pointer(Some(at), shape).unwrap();
                    // expected: the scene with only this shape on top (nothing of the one before)
                    let mut want = vec![50u8; (w * h * 4) as usize];
                    paint_cursor(&mut want, w as usize, h as usize, (0, 0), shape, at.0, at.1, k);
                    // SAFETY: `mem` is alive and only the framebuffer (which points into it) wrote to it
                    let got = unsafe { std::slice::from_raw_parts(ptr, len) };
                    for y in 0..h as usize {
                        for x in 0..w as usize {
                            let i = (y * w as usize + x) * 4;
                            assert_eq!(px(got, w as usize, x, y), [want[i], want[i + 1], want[i + 2]], "{shape:?} k={k} at {at:?}: pixel ({x}, {y})");
                        }
                    }
                }
                // moving to another place with another shape leaves nothing of the old one
                s.move_pointer(Some((10, 10)), Shape::Move).unwrap();
                s.move_pointer(None, Shape::Move).unwrap();
                let got = unsafe { std::slice::from_raw_parts(ptr, len) };
                assert!(got.chunks_exact(4).all(|p| p[..3] == [50, 50, 50]), "k={k}: pointer taken off, the scene alone is left");
            }
        }
    }

    #[test]
    fn the_hand_over_a_link_in_the_browser_reaches_the_screen_as_a_hand() {
        // the whole path without the shell: a shape byte is what the input thread reads, and a hand is not an arrow
        let (w, h) = (80u32, 60u32);
        let mut mem = vec![0u8; (w * h * 4) as usize];
        let (ptr, len) = (mem.as_ptr(), mem.len());
        let mut s = screen_over(&mut mem, w, h, 1);
        let shared = Pointer { x: AtomicI32::new(30), y: AtomicI32::new(30), seen: AtomicBool::new(true), shape: AtomicU8::new(0) };
        s.move_pointer(shared.get(), shared.shape()).unwrap();
        let arrow: Vec<u8> = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        shared.shape.store(shape_to_u8(Shape::Hand), Ordering::Relaxed);
        s.move_pointer(shared.get(), shared.shape()).unwrap();
        let hand: Vec<u8> = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        assert_ne!(arrow, hand, "the picture changed without a mouse move");
        // the finger tip is on the click point
        let hot = sprite(Shape::Hand).unwrap().hot;
        assert_eq!(hot, (4, 0));
        assert_eq!(px(&hand, w as usize, 30, 30), [0, 0, 0], "the outline of the finger tip is at the pointer");
    }

    /// A picture of every pointer at 1x, 2x and 3x on a grey scene and a white one: `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib backend::tests::pictures -- --ignored`.
    #[test]
    #[ignore]
    fn pictures() {
        let Ok(dir) = std::env::var("IYAKKI_PICTURES") else { return };
        let (cell, w, h) = (64usize, 12 * 64 * 2, 3 * 80usize);
        let mut img = vec![0u8; w * h * 4];
        for (i, p) in img.chunks_exact_mut(4).enumerate() {
            let left = (i % w) < w / 2;
            p.copy_from_slice(if left { &[70, 90, 130, 255] } else { &[240, 240, 240, 255] });
        }
        for (n, shape) in ALL.iter().enumerate() {
            for k in 1..=3usize {
                for half in 0..2 {
                    let x = (n * cell + cell / 2 + half * w / 2) as i32;
                    paint_cursor(&mut img, w, h, (0, 0), *shape, x, ((k - 1) * 80 + 30) as i32, k);
                }
            }
        }
        std::fs::create_dir_all(&dir).unwrap();
        let png = crate::camera::pixels::png_with_dpi(w as u32, h as u32, &img, 96.0).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("pointers.png"), png).unwrap();
    }
}
