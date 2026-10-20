//! A live camera: one thread reads frames from the device (and writes them to the recording when there is one), another
//! turns the newest frame into a picture for the screen, dropping frames it cannot keep up with. The app only looks at
//! the newest results, so drawing never waits on the camera.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::avi::{AviWriter, PART_LIMIT};
use super::{pixels, Capture, Mode, Pix, RawFrame};

/// The live picture is made at most this wide (a 4 MP camera is shrunk for the screen; photos use the full frame).
pub const PREVIEW_MAX_W: u32 = 1280;

/// A frame ready to draw.
pub struct Preview {
    pub pm: tiny_skia::Pixmap,
    /// The frame's own size (the pixmap may be smaller).
    pub width: u32,
    pub height: u32,
}

enum Cmd {
    Control(u32, i32),
}

/// A recording in progress: MJPEG frames into AVI files, a new part before a file reaches 1 GB.
struct Recording {
    writer: Option<AviWriter<BufWriter<File>>>,
    base: PathBuf,
    part: u32,
    started: Instant,
    part_started: Instant,
    part_frames: u64,
    done_bytes: u64,
    frames: u64,
    files: Vec<PathBuf>,
    error: Option<String>,
}

fn part_path(base: &Path, part: u32) -> PathBuf {
    if part <= 1 {
        return base.to_path_buf();
    }
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    base.with_file_name(format!("{stem}-part{part}.avi"))
}

fn open_part(path: &Path, mode: Mode) -> Result<AviWriter<BufWriter<File>>, String> {
    let f = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
    AviWriter::new(BufWriter::with_capacity(1 << 20, f), mode.width, mode.height, 30.0).map_err(|e| e.to_string())
}

impl Recording {
    fn finish_part(&mut self) {
        if let Some(w) = self.writer.take() {
            let secs = self.part_started.elapsed().as_secs_f32();
            let fps = if secs > 0.2 { self.part_frames as f32 / secs } else { 15.0 };
            self.done_bytes += w.bytes() + 8 + 16 * w.frames() as u64;
            if let Err(e) = w.finish(fps) {
                self.error.get_or_insert(format!("could not finish the video: {e}"));
            }
        }
    }

    fn add(&mut self, jpeg: &[u8], mode: Mode) {
        if self.error.is_some() {
            return;
        }
        if self.writer.as_ref().is_some_and(|w| w.bytes() + jpeg.len() as u64 > PART_LIMIT) {
            self.finish_part();
            self.part += 1;
            let p = part_path(&self.base, self.part);
            match open_part(&p, mode) {
                Ok(w) => {
                    self.writer = Some(w);
                    self.files.push(p);
                    self.part_started = Instant::now();
                    self.part_frames = 0;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(w) = self.writer.as_mut() {
            match w.frame(jpeg) {
                Ok(()) => {
                    self.frames += 1;
                    self.part_frames += 1;
                }
                Err(e) => self.error = Some(format!("could not write the video: {e}")),
            }
        }
    }

    fn bytes(&self) -> u64 {
        self.done_bytes + self.writer.as_ref().map_or(0, |w| w.bytes())
    }
}

/// How a recording is going.
#[derive(Clone, Debug, PartialEq)]
pub struct RecStatus {
    pub secs: f64,
    pub bytes: u64,
    pub frames: u64,
    pub path: PathBuf,
    pub error: Option<String>,
}

struct Shared {
    stop: AtomicBool,
    raw: Mutex<(u64, Option<Arc<RawFrame>>)>,
    fresh: Condvar,
    preview: Mutex<(u64, Option<Arc<Preview>>)>,
    error: Mutex<Option<String>>,
    recording: AtomicBool,
    rec: Mutex<Option<Recording>>,
    /// Frames per second, times 100 (smoothed).
    fps100: AtomicU64,
}

pub struct Live {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    cmd: Sender<Cmd>,
    pub mode: Mode,
}

/// The picture to show for a frame: decoded, shrunk to at most [`PREVIEW_MAX_W`] wide.
pub fn preview_of(f: &RawFrame) -> Result<Preview, String> {
    let (w, h, rgba) = f.to_rgba()?;
    let (pw, ph) = if w > PREVIEW_MAX_W { (PREVIEW_MAX_W, ((h as u64 * PREVIEW_MAX_W as u64) / w as u64).max(1) as u32) } else { (w, h) };
    let rgba = if (pw, ph) == (w, h) { rgba } else { pixels::crop_scale(w, h, &rgba, pixels::RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, pw, ph, false) };
    let pm = pixels::pixmap(pw, ph, rgba).ok_or("an empty frame")?;
    Ok(Preview { pm, width: w, height: h })
}

fn capture_loop(mut cap: Box<dyn Capture>, sh: Arc<Shared>, cmds: Receiver<Cmd>) {
    let mut last: Option<Instant> = None;
    while !sh.stop.load(Ordering::Relaxed) {
        while let Ok(c) = cmds.try_recv() {
            match c {
                Cmd::Control(id, v) => {
                    if let Err(e) = cap.set_control(id, v) {
                        log::warn!("camera control {id:#x}: {e}");
                    }
                }
            }
        }
        match cap.next(200) {
            Ok(Some(f)) => {
                let now = Instant::now();
                if let Some(t) = last {
                    let dt = now.duration_since(t).as_secs_f64().max(1e-3);
                    let cur = sh.fps100.load(Ordering::Relaxed) as f64 / 100.0;
                    let inst = 1.0 / dt;
                    let next = if cur <= 0.0 { inst } else { cur * 0.9 + inst * 0.1 };
                    sh.fps100.store((next * 100.0) as u64, Ordering::Relaxed);
                }
                last = Some(now);
                if sh.recording.load(Ordering::Relaxed) {
                    // the JPEG is made before taking the lock, so the screen never waits on an encode
                    match f.to_jpeg() {
                        Ok(j) => {
                            if let Some(r) = sh.rec.lock().unwrap().as_mut() {
                                r.add(&j, cap.mode());
                            }
                        }
                        Err(e) => log::warn!("camera frame not recorded: {e}"),
                    }
                }
                let mut g = sh.raw.lock().unwrap();
                g.0 += 1;
                g.1 = Some(Arc::new(f));
                sh.fresh.notify_all();
            }
            Ok(None) => {}
            Err(e) => {
                *sh.error.lock().unwrap() = Some(e);
                sh.stop.store(true, Ordering::Relaxed);
                sh.fresh.notify_all();
                break;
            }
        }
    }
}

fn decode_loop(sh: Arc<Shared>) {
    let mut seen = 0u64;
    loop {
        let frame = {
            let mut g = sh.raw.lock().unwrap();
            while g.0 == seen && !sh.stop.load(Ordering::Relaxed) {
                g = sh.fresh.wait_timeout(g, std::time::Duration::from_millis(250)).unwrap().0;
            }
            if sh.stop.load(Ordering::Relaxed) {
                return;
            }
            seen = g.0;
            g.1.clone()
        };
        if let Some(f) = frame {
            match preview_of(&f) {
                Ok(p) => {
                    let mut g = sh.preview.lock().unwrap();
                    *g = (seen, Some(Arc::new(p)));
                }
                Err(e) => log::warn!("camera frame not shown: {e}"),
            }
        }
    }
}

impl Live {
    pub fn start(cap: Box<dyn Capture>) -> Live {
        let mode = cap.mode();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            raw: Mutex::new((0, None)),
            fresh: Condvar::new(),
            preview: Mutex::new((0, None)),
            error: Mutex::new(None),
            recording: AtomicBool::new(false),
            rec: Mutex::new(None),
            fps100: AtomicU64::new(0),
        });
        let (tx, rx) = channel();
        let a = shared.clone();
        let b = shared.clone();
        let t1 = std::thread::Builder::new().name("camera-capture".into()).spawn(move || capture_loop(cap, a, rx)).expect("camera thread");
        let t2 = std::thread::Builder::new().name("camera-decode".into()).spawn(move || decode_loop(b)).expect("camera thread");
        Live { shared, threads: vec![t1, t2], cmd: tx, mode }
    }

    /// The newest picture to draw, with its number (a bigger number is a newer picture).
    pub fn preview(&self) -> Option<(u64, Arc<Preview>)> {
        let g = self.shared.preview.lock().unwrap();
        g.1.clone().map(|p| (g.0, p))
    }

    /// The newest frame as the camera sent it (for photos: full size, no shrinking).
    pub fn latest(&self) -> Option<Arc<RawFrame>> {
        self.shared.raw.lock().unwrap().1.clone()
    }

    /// How many frames have arrived so far.
    pub fn frames(&self) -> u64 {
        self.shared.raw.lock().unwrap().0
    }

    /// Why the camera stopped, if it did.
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    pub fn fps(&self) -> f32 {
        self.shared.fps100.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn set_control(&self, id: u32, value: i32) {
        let _ = self.cmd.send(Cmd::Control(id, value));
    }

    /// Start recording into `path` (an `.avi` that must not exist yet).
    pub fn start_recording(&self, path: &Path) -> Result<(), String> {
        let mut g = self.shared.rec.lock().unwrap();
        if g.is_some() {
            return Err("already recording".into());
        }
        let w = open_part(path, self.mode)?;
        let now = Instant::now();
        *g = Some(Recording { writer: Some(w), base: path.to_path_buf(), part: 1, started: now, part_started: now, part_frames: 0, done_bytes: 0, frames: 0, files: vec![path.to_path_buf()], error: None });
        self.shared.recording.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn recording(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }

    pub fn rec_status(&self) -> Option<RecStatus> {
        let g = self.shared.rec.lock().unwrap();
        g.as_ref().map(|r| RecStatus { secs: r.started.elapsed().as_secs_f64(), bytes: r.bytes(), frames: r.frames, path: r.files.last().cloned().unwrap_or_default(), error: r.error.clone() })
    }

    /// Stop recording: the files written (several when it went past 1 GB), or why it failed. `None` when not recording.
    pub fn stop_recording(&self) -> Option<Result<(Vec<PathBuf>, u64), String>> {
        self.shared.recording.store(false, Ordering::Relaxed);
        let mut r = self.shared.rec.lock().unwrap().take()?;
        r.finish_part();
        Some(match r.error {
            Some(e) => Err(e),
            None => Ok((r.files, r.frames)),
        })
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.stop_recording();
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.fresh.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Is this frame a picture the person would want (MJPEG frames from some cameras start black while exposure settles).
pub fn usable(f: &RawFrame) -> bool {
    match f.pix {
        Pix::Mjpeg => f.data.len() > 64,
        Pix::Yuyv => f.data.len() >= (f.width * f.height * 2) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{device, FakeOpener};
    use super::super::Opener;
    use super::*;

    fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        for _ in 0..400 {
            if let Some(v) = f() {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn frames_flow_to_the_preview_and_a_recording_is_a_playable_avi() {
        let d = std::env::temp_dir().join(format!("iyakki-cam-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for yuyv in [false, true] {
            let op = FakeOpener { yuyv_only: yuyv, ..Default::default() };
            let o = op.open(&device("video0", "Fake"), None).unwrap();
            let live = Live::start(o.cap);
            let (n, p) = wait("a picture", || live.preview());
            assert!(n > 0 && p.pm.width() == live.mode.width);
            assert!(live.latest().is_some_and(|f| usable(&f)));
            let path = d.join(format!("v-{yuyv}.avi"));
            live.start_recording(&path).unwrap();
            assert!(live.start_recording(&path).is_err());
            wait("frames recorded", || live.rec_status().filter(|s| s.frames >= 4));
            let st = live.rec_status().unwrap();
            assert!(st.bytes > 0 && st.path == path && st.error.is_none());
            let (files, frames) = live.stop_recording().unwrap().unwrap();
            assert_eq!(files, vec![path.clone()]);
            assert!(live.stop_recording().is_none());
            let info = super::super::avi::parse(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(info.frames as u64, frames);
            assert_eq!((info.width, info.height), (live.mode.width, live.mode.height));
            assert!(info.fps > 1.0 && info.fps < 120.0, "{}", info.fps);
            // every frame is a complete JPEG (tables included) that decodes to the right size
            let b = super::super::jpeg::decode(&info.chunks[0]).unwrap();
            assert!(super::super::jpeg::has_huffman(&info.chunks[0]));
            assert_eq!((b.width, b.height), (live.mode.width, live.mode.height));
            live.set_control(super::super::v4l2::CID_BRIGHTNESS, 10);
            wait("the control", || op.controls.lock().unwrap().contains(&(super::super::v4l2::CID_BRIGHTNESS, 10)).then_some(()));
            // the file is never overwritten
            assert!(live.start_recording(&path).is_err());
        }
        // a camera pulled out says so
        let op = FakeOpener { fail_after: Some(2), ..Default::default() };
        let live = Live::start(op.open(&device("video0", "Fake"), None).unwrap().cap);
        let e = wait("the error", || live.error());
        assert!(e.contains("disconnected"));
        drop(live);
        assert_eq!(part_path(Path::new("/x/video-1.avi"), 1), Path::new("/x/video-1.avi"));
        assert_eq!(part_path(Path::new("/x/video-1.avi"), 3), Path::new("/x/video-1-part3.avi"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
