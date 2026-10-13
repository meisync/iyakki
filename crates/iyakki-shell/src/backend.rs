//! Event loop tying the shell to the framebuffer and evdev devices.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crate::fb::Framebuffer;
use crate::gfx::{rgb, Gfx};
use crate::input::{poll_devices, scan_devices, Device, InputState};
use crate::shell::Shell;
use crate::ui::Event;

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
struct GraphicsMode(Option<std::fs::File>);

impl GraphicsMode {
    const KDSETMODE: libc::c_ulong = 0x4B3A;

    fn enter() -> GraphicsMode {
        use std::os::fd::AsRawFd;
        for path in ["/dev/tty0", "/dev/console"] {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
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

/// The on-screen rectangle (clipped to the screen) covered by the pointer drawn at (`x`, `y`).
pub fn cursor_rect(x: i32, y: i32, k: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (cw, ch) = (12 * k as i32, 17 * k as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + cw).min(w as i32), (y + ch).min(h as i32));
    if x1 <= x0 || y1 <= y0 { None } else { Some((x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize)) }
}

/// A small RGBA patch of the scene under the pointer with the pointer painted on top, and where it goes.
pub fn cursor_patch(scene: &[u8], w: usize, h: usize, x: i32, y: i32, k: usize) -> Option<((usize, usize, usize, usize), Vec<u8>)> {
    let r = cursor_rect(x, y, k, w, h)?;
    let (rx, ry, rw, rh) = r;
    let mut patch = Vec::with_capacity(rw * rh * 4);
    for row in ry..ry + rh {
        patch.extend_from_slice(&scene[(row * w + rx) * 4..(row * w + rx + rw) * 4]);
    }
    paint_cursor(&mut patch, rw, rh, (rx as i32, ry as i32), x, y, k);
    Some((r, patch))
}

/// Paint the pointer (tip at screen position `x`, `y`) into `frame`, a `fw` x `fh` buffer whose top-left
/// corner is at screen position `origin`.
pub fn paint_cursor(frame: &mut [u8], fw: usize, fh: usize, origin: (i32, i32), x: i32, y: i32, k: usize) {
    for (row, line) in CURSOR.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let rgb = match ch {
                'X' => [0, 0, 0, 255],
                '.' => [255, 255, 255, 255],
                _ => continue,
            };
            for dy in 0..k as i32 {
                for dx in 0..k as i32 {
                    let (px, py) = (x + (col * k) as i32 + dx - origin.0, y + (row * k) as i32 + dy - origin.1);
                    if px >= 0 && py >= 0 && (px as usize) < fw && (py as usize) < fh {
                        let i = (py as usize * fw + px as usize) * 4;
                        frame[i..i + 4].copy_from_slice(&rgb);
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

fn splash_loop(fb_path: &str) -> Result<(), String> {
    use crate::splash::{self, LogTail, View};
    let mut fb = Framebuffer::open(fb_path)?;
    log::info!("splash: drawing on {} ({}x{}, {} bit, pitch {})", fb.id, fb.width, fb.height, fb.format.bpp, fb.pitch);
    // leave the text console off for good: the shell that follows sets graphics mode again and restores it at its end
    std::mem::forget(GraphicsMode::enter());
    let mut gfx = Gfx::new(fb.width, fb.height);
    let mut shown: Vec<u8> = Vec::new();
    let mut input = InputState::new(fb.width, fb.height);
    let mut devs: Vec<Device> = Vec::new();
    scan_devices(&mut devs);
    let mut log = LogTail::open();
    let mut show_log = false;
    let (start, mut last_scan, mut last_check) = (Instant::now(), Instant::now(), Instant::now());
    let mut fraction = 0.0f32;
    let mut frames = 0u32;
    while !splash::is_done() {
        let frame_start = Instant::now();
        for (raw, abs) in poll_devices(&mut devs, 1) {
            for ev in input.feed(raw, abs) {
                match ev {
                    Event::Key(crate::ui::Key::Char('l' | 'L')) => show_log = !show_log,
                    Event::MouseDown(_, y) if (y as f32) >= splash::hint_zone(fb.width, fb.height).0 => show_log = !show_log,
                    _ => {}
                }
            }
        }
        if last_scan.elapsed() > Duration::from_secs(2) {
            scan_devices(&mut devs); // the keyboard may appear after the drivers load
            last_scan = Instant::now();
        }
        if last_check.elapsed() > Duration::from_millis(500) {
            last_check = Instant::now();
            if let Ok(now) = Framebuffer::open(fb_path) {
                if now.id != fb.id || now.width != fb.width || now.height != fb.height || now.pitch != fb.pitch {
                    log::info!("splash: screen changed ({} -> {}), following it", fb.id, now.id);
                    gfx = Gfx::new(now.width, now.height);
                    input = InputState::new(now.width, now.height);
                    shown.clear();
                    fb = now;
                }
            }
        }
        log.poll();
        let (percent, stage) = splash::current();
        let target = percent as f32 / 100.0;
        fraction += (target - fraction) * 0.10;
        let lines: &[String] = &log.lines;
        splash::draw(&mut gfx, &View { t: start.elapsed().as_secs_f32(), fraction, stage: &stage, log: show_log.then_some(lines), steps: &[] });
        fb.present_banded(gfx.rgba(), &mut shown)?;
        frames += 1;
        // about 20 frames a second; a slow screen simply runs slower
        let spent = frame_start.elapsed();
        if spent < Duration::from_millis(50) {
            std::thread::sleep(Duration::from_millis(50) - spent);
        }
    }
    log::info!("splash: stopped after {frames} frames in {:.1} s", start.elapsed().as_secs_f32());
    Ok(())
}

/// Frame timings, logged every few seconds so a slow machine can be diagnosed from its boot messages.
struct FrameStats {
    since: Instant,
    frames: u32,
    render: Duration,
    present: Duration,
    worst: Duration,
}

impl FrameStats {
    fn new() -> FrameStats {
        FrameStats { since: Instant::now(), frames: 0, render: Duration::ZERO, present: Duration::ZERO, worst: Duration::ZERO }
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
                "shell: {} frames in {:.0}s, draw {:.1} ms, screen write {:.1} ms, slowest {:.0} ms; cpu0 {} kHz, governor {}",
                self.frames,
                self.since.elapsed().as_secs_f64(),
                ms(self.render) / n as f64,
                ms(self.present) / n as f64,
                ms(self.worst),
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
    k: usize,
}

impl Screen {
    /// Erase the pointer from the screen, then draw it at `to`.
    fn move_pointer(&mut self, to: Option<(i32, i32)>) -> Result<(), String> {
        if self.scene.is_empty() || to == self.pointer_at {
            return Ok(());
        }
        let (wu, hu) = (self.fb.width as usize, self.fb.height as usize);
        self.take_pointer_off()?;
        if let Some((x, y)) = to {
            if let Some((r, patch)) = cursor_patch(&self.scene, wu, hu, x, y, self.k) {
                self.fb.present_patch(&patch, r)?;
            }
        }
        self.pointer_at = to;
        Ok(())
    }

    fn take_pointer_off(&mut self) -> Result<(), String> {
        let (wu, hu) = (self.fb.width as usize, self.fb.height as usize);
        if let Some((x, y)) = self.pointer_at.take() {
            if let Some(r) = cursor_rect(x, y, self.k, wu, hu) {
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
}

impl Pointer {
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
    let r = run_fb_once(shell, fb_path, stop, &remode);
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

fn run_fb_once(shell: &mut Shell, fb_path: &str, stop: &AtomicBool, remode: &AtomicBool) -> Result<(), String> {
    let fb = Framebuffer::open(fb_path)?;
    let _graphics = GraphicsMode::enter();
    let (w, h) = (fb.width, fb.height);
    let mut gfx = Gfx::new(w, h);
    let mut devs: Vec<Device> = Vec::new();
    scan_devices(&mut devs);
    log::info!("shell: {}x{} framebuffer, {} input device(s), {} CPU thread(s)", w, h, devs.len(), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    let screen = Mutex::new(Screen { fb, scene: Vec::new(), pointer_at: None, k: cursor_scale(w, h) });
    let pointer = Pointer { x: AtomicI32::new(0), y: AtomicI32::new(0), seen: AtomicBool::new(false) };
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let quit = AtomicBool::new(false);
    let (paused, idle) = (AtomicBool::new(false), AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Event>();
    let fail = |e: String| {
        *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
        quit.store(true, Ordering::Relaxed);
    };

    std::thread::scope(|sc| {
        let input_thread = std::thread::Builder::new().name("iyakki-input".into()).stack_size(INPUT_STACK).spawn_scoped(sc, || {
            let mut input = InputState::new(w, h);
            let mut last_scan = Instant::now();
            while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
                if paused.load(Ordering::SeqCst) {
                    // the screen and keyboard belong to another program: hold nothing open
                    devs.clear();
                    idle.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                if idle.swap(false, Ordering::SeqCst) {
                    scan_devices(&mut devs);
                    input = InputState::new(w, h); // no key looks held down
                    last_scan = Instant::now();
                }
                for (raw, abs) in poll_devices(&mut devs, 10) {
                    for ev in input.feed(raw, abs) {
                        if matches!(ev, Event::MouseMove(..) | Event::MouseDown(..)) {
                            pointer.seen.store(true, Ordering::Relaxed);
                        }
                        let _ = tx.send(ev);
                    }
                }
                pointer.x.store(input.x, Ordering::Relaxed);
                pointer.y.store(input.y, Ordering::Relaxed);
                if let Err(e) = lock(&screen).move_pointer(pointer.get()) {
                    fail(e);
                }
                if last_scan.elapsed() > Duration::from_secs(2) {
                    scan_devices(&mut devs); // keyboards and mice plugged in later
                    last_scan = Instant::now();
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
        while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
            let mut dirty = false;
            // wait for the next event, but wake for the tick
            let wait = Duration::from_millis(33).saturating_sub(last_tick.elapsed());
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
            }
            if let Some(job) = crate::external::take() {
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
            if dirty || first || last_render.elapsed() >= Duration::from_millis(100) {
                first = false;
                let t0 = Instant::now();
                shell.render(&mut gfx);
                let t1 = Instant::now();
                let mut s = lock(&screen);
                // the screen holds `scene` + pointer: take the pointer off, update the changed rows, put it back
                let res = s.take_pointer_off().and_then(|_| {
                    let Screen { fb, scene, .. } = &mut *s;
                    fb.present_banded(gfx.rgba(), scene).map(|_| ())
                });
                let res = res.and_then(|_| s.move_pointer(pointer.get()));
                drop(s);
                last_render = Instant::now();
                if first_picture {
                    first_picture = false;
                    // the end of "power on to sign-in screen": the kernel's own clock stamps this line (see dist/buildroot/boot-time.py)
                    log::info!("shell: first picture on screen ({}x{}, first frame took {} ms)", w, h, (t1 - t0).as_millis());
                }
                stats.frame(t1 - t0, last_render - t1);
                if let Err(e) = res {
                    fail(e);
                }
            }
        }
        quit.store(true, Ordering::Relaxed);
    });
    let err = failed.lock().unwrap_or_else(|p| p.into_inner()).take();
    err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_scales_with_the_screen() {
        assert_eq!(cursor_scale(1280, 720), 1);
        assert_eq!(cursor_scale(1920, 1080), 2, "1.5 rounds up");
        assert_eq!(cursor_scale(3840, 2160), 3);
        assert_eq!(cursor_scale(640, 480), 1);
        assert_eq!(cursor_scale(7680, 4320), 3, "capped");
    }

    #[test]
    fn the_pointer_rectangle_is_clipped_to_the_screen() {
        assert_eq!(cursor_rect(10, 10, 1, 100, 100), Some((10, 10, 12, 17)));
        assert_eq!(cursor_rect(95, 95, 1, 100, 100), Some((95, 95, 5, 5)));
        assert_eq!(cursor_rect(-5, -5, 1, 100, 100), Some((0, 0, 7, 12)));
        assert_eq!(cursor_rect(100, 50, 1, 100, 100), None, "fully off to the right");
        assert_eq!(cursor_rect(10, 10, 3, 1000, 1000), Some((10, 10, 36, 51)));
    }

    #[test]
    fn a_patch_has_the_scene_with_the_pointer_on_top() {
        let (w, h) = (40usize, 40usize);
        let scene = vec![50u8; w * h * 4];
        let (r, patch) = cursor_patch(&scene, w, h, 5, 5, 1).unwrap();
        assert_eq!(r, (5, 5, 12, 17));
        assert_eq!(patch.len(), 12 * 17 * 4);
        assert_eq!(&patch[0..4], &[0, 0, 0, 255], "the tip is the outline colour");
        let transparent = &patch[(0 * 12 + 11) * 4..(0 * 12 + 11) * 4 + 4];
        assert_eq!(transparent, &[50, 50, 50, 50], "pixels outside the arrow keep the scene");
        // scaled pointer paints k x k blocks
        let (r3, p3) = cursor_patch(&scene, w, h, 0, 0, 2).unwrap();
        assert_eq!(r3, (0, 0, 24, 34));
        assert_eq!(&p3[0..4], &[0, 0, 0, 255]);
        assert_eq!(&p3[(1 * 24 + 1) * 4..(1 * 24 + 1) * 4 + 4], &[0, 0, 0, 255], "the first sprite pixel is 2x2");
        // a pointer near the edge still produces a clipped patch
        let (r2, p2) = cursor_patch(&scene, w, h, 36, 36, 1).unwrap();
        assert_eq!((r2, p2.len()), ((36, 36, 4, 4), 4 * 4 * 4));
    }
}
