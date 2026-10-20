//! The shell's side: start the helper program, talk to it over a socket, and copy its frames out of the shared file.
//!
//! The helper is started without a shell: `program [args...] --socket-fd 0 --shm PATH`. The socket is the helper's standard input
//! (so no file-descriptor juggling is needed); standard output is closed and standard error is collected as log lines.
//! The pixel file is made by this side (mode 0600, sparse) and removed when the view is dropped.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::proto::{self, FromHelper, Hello, Rect, ToHelper};

#[derive(Clone, Debug)]
pub struct Config {
    /// The helper program (an executable file).
    pub program: PathBuf,
    /// Arguments before the standard `--socket-fd 0 --shm PATH` ones.
    pub args: Vec<String>,
    /// The environment of the helper: `None` = the shell's own, `Some` = exactly these variables.
    pub env: Option<Vec<(String, String)>>,
    pub cwd: Option<PathBuf>,
    /// Where the shared pixel file is made (`/dev/shm` where it exists, else a private folder).
    pub shm_dir: PathBuf,
    pub hello: Hello,
}

impl Config {
    pub fn new(program: impl Into<PathBuf>, hello: Hello) -> Config {
        let shm = PathBuf::from("/dev/shm");
        Config { program: program.into(), args: Vec::new(), env: None, cwd: None, shm_dir: if shm.is_dir() { shm } else { std::env::temp_dir() }, hello }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready { engine: String, version: String },
    /// New pixels are available in [`WebView::pixels`]; [`WebView::take_dirty`] says where.
    Frame,
    Title(String),
    Url(String),
    Loading { loading: bool, progress: u8 },
    History { can_back: bool, can_forward: bool },
    Cursor(u8),
    Download { url: String, name: String },
    Clipboard(String),
    /// The answer to a `ToHelper::Script` (protocol 1.1).
    ScriptResult { id: u32, ok: bool, json: String },
    /// The helper says its engine failed (it may recover by itself).
    Crashed(String),
    Log(String),
    /// The helper program is gone (crashed, was killed, or closed the socket). Sent once.
    Exited(String),
}

enum Incoming {
    Msg(FromHelper),
    Stderr(String),
    /// The socket ended; the string says how.
    End(String),
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
/// The largest picture side the shell ever asks for (the shared file holds two pictures of the largest size).
const MAX_DIM: u32 = 16384;
/// More unread messages than this means the helper floods the shell: it is treated as broken.
const MAX_QUEUED_IN: usize = 20_000;
/// Bytes waiting to be written to a helper that does not read (a hung helper): beyond this, messages are dropped.
const MAX_QUEUED_OUT: usize = 16 << 20;

/// Remove the shared files of shell processes that no longer exist (killed without cleaning up).
pub fn remove_stale_shm(dir: &std::path::Path) {
    {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix("iyakki-webview-").and_then(|r| r.strip_suffix(".shm")) else { continue };
            let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
            if pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

struct Shm {
    file: File,
    path: PathBuf,
    slot_bytes: u64,
}

impl Drop for Shm {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct WebView {
    cfg: Config,
    child: Option<Child>,
    tx: Option<Sender<Vec<u8>>>,
    out_bytes: Arc<std::sync::atomic::AtomicUsize>,
    in_msgs: Arc<std::sync::atomic::AtomicUsize>,
    rx: Receiver<Incoming>,
    stream: Option<UnixStream>,
    shm: Shm,
    pixels: Vec<u8>,
    size: (u32, u32),
    dirty: Option<Rect>,
    ready: Option<(String, String)>,
    dead: Option<String>,
    exit_reported: bool,
    tail: Arc<Mutex<VecDeque<String>>>,
    started: Instant,
    frames: u64,
    // what to replay after a restart
    want_size: (u32, u32),
    url: String,
    zoom: u32,
    focus: bool,
    visible: bool,
}

fn clamp_size(w: u32, h: u32, hello: &Hello) -> (u32, u32) {
    (w.clamp(1, hello.max_w.max(1)), h.clamp(1, hello.max_h.max(1)))
}

impl WebView {
    /// Start the helper and send it the `Hello`. Returns at once; [`WebView::poll`] delivers `Ready` when it is up.
    pub fn start(mut cfg: Config) -> Result<WebView, String> {
        cfg.hello.w = cfg.hello.w.clamp(1, MAX_DIM);
        cfg.hello.h = cfg.hello.h.clamp(1, MAX_DIM);
        cfg.hello.max_w = cfg.hello.max_w.clamp(1, MAX_DIM);
        cfg.hello.max_h = cfg.hello.max_h.clamp(1, MAX_DIM);
        let (w, h) = clamp_size(cfg.hello.w, cfg.hello.h, &cfg.hello);
        cfg.hello.w = w;
        cfg.hello.h = h;
        cfg.hello.max_w = cfg.hello.max_w.clamp(w, MAX_DIM);
        cfg.hello.max_h = cfg.hello.max_h.clamp(h, MAX_DIM);
        let (rx, tx, stream, child, shm, tail, out_bytes, in_msgs) = spawn(&cfg)?;
        let v = WebView {
            out_bytes,
            in_msgs,
            want_size: (w, h),
            url: String::new(),
            zoom: 100,
            focus: false,
            visible: true,
            cfg,
            child: Some(child),
            tx: Some(tx),
            rx,
            stream: Some(stream),
            shm,
            pixels: Vec::new(),
            size: (0, 0),
            dirty: None,
            ready: None,
            dead: None,
            exit_reported: false,
            tail,
            started: Instant::now(),
            frames: 0,
        };
        v.raw_send(&ToHelper::Hello(v.cfg.hello.clone()));
        Ok(v)
    }

    fn raw_send(&self, m: &ToHelper) -> bool {
        match (&self.tx, &self.dead) {
            (Some(tx), None) => {
                let buf = m.encode();
                // a helper that stopped reading must not make the shell queue without limit
                if self.out_bytes.load(Ordering::Relaxed) > MAX_QUEUED_OUT {
                    return false;
                }
                self.out_bytes.fetch_add(buf.len(), Ordering::Relaxed);
                tx.send(buf).is_ok()
            }
            _ => false,
        }
    }

    /// Send a message. Returns false when the helper is gone. `Resize` is clamped to the room in the pixel file.
    pub fn send(&mut self, m: ToHelper) -> bool {
        let m = match m {
            ToHelper::Resize { w, h } => {
                let (w, h) = clamp_size(w, h, &self.cfg.hello);
                if (w, h) == self.want_size {
                    return self.dead.is_none();
                }
                self.want_size = (w, h);
                ToHelper::Resize { w, h }
            }
            ToHelper::Zoom(z) => {
                self.zoom = z;
                ToHelper::Zoom(z)
            }
            ToHelper::Focus(f) => {
                self.focus = f;
                ToHelper::Focus(f)
            }
            ToHelper::SetVisible(v) => {
                self.visible = v;
                ToHelper::SetVisible(v)
            }
            other => other,
        };
        self.raw_send(&m)
    }

    /// Everything that happened since the last call, in order. Frames are copied from the shared file here.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut frames: Vec<(u8, u32, u32, Vec<Rect>)> = Vec::new();
        loop {
            let got = self.rx.try_recv();
            if got.is_ok() {
                let _ = self.in_msgs.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_sub(1)));
            }
            match got {
                Ok(Incoming::Msg(m)) => match m {
                    FromHelper::Ready { engine, engine_version, .. } => {
                        self.ready = Some((engine.clone(), engine_version.clone()));
                        out.push(Event::Ready { engine, version: engine_version });
                    }
                    FromHelper::Frame { slot, w, h, rects } => frames.push((slot, w, h, rects)),
                    FromHelper::Title(s) => out.push(Event::Title(s)),
                    FromHelper::Url(s) => {
                        self.url = s.clone();
                        out.push(Event::Url(s));
                    }
                    FromHelper::Loading { loading, progress } => out.push(Event::Loading { loading, progress }),
                    FromHelper::History { can_back, can_forward } => out.push(Event::History { can_back, can_forward }),
                    FromHelper::Cursor(k) => out.push(Event::Cursor(k)),
                    FromHelper::Download { url, name } => out.push(Event::Download { url, name }),
                    FromHelper::Clipboard(s) => out.push(Event::Clipboard(s)),
                    FromHelper::ScriptResult { id, ok, json } => out.push(Event::ScriptResult { id, ok, json }),
                    FromHelper::Crashed(s) => out.push(Event::Crashed(s)),
                    FromHelper::Log(s) => out.push(Event::Log(s)),
                },
                Ok(Incoming::Stderr(l)) => {
                    if let Ok(mut t) = self.tail.lock() {
                        if t.len() >= 20 {
                            t.pop_front();
                        }
                        t.push_back(l.clone());
                    }
                    out.push(Event::Log(l));
                }
                Ok(Incoming::End(why)) => {
                    if self.dead.is_none() {
                        self.dead = Some(why);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.dead.is_none() {
                        self.dead = Some("the helper's connection ended".into());
                    }
                    break;
                }
            }
        }
        if !frames.is_empty() {
            // far more pictures than can be shown: keep only the newest few (each is acknowledged all the same)
            let skip = frames.len().saturating_sub(64);
            let all_slots: Vec<u8> = frames.iter().map(|f| f.0).collect();
            frames.drain(..skip);
            if self.take_frames(&frames) {
                out.push(Event::Frame);
            }
            for slot in all_slots {
                self.raw_send(&ToHelper::Ack(slot));
            }
        }
        if self.dead.is_none() {
            if let Some(c) = &mut self.child {
                if let Ok(Some(status)) = c.try_wait() {
                    self.dead = Some(format!("the helper program stopped ({status})"));
                }
            }
        }
        if let (Some(why), false) = (self.dead.clone(), self.exit_reported) {
            self.exit_reported = true;
            // let the process finish dying so its exit status can be named; never wait long
            let mut status = None;
            if let Some(c) = &mut self.child {
                let t = Instant::now();
                loop {
                    match c.try_wait() {
                        Ok(Some(st)) => {
                            status = Some(st.to_string());
                            break;
                        }
                        Err(_) => break,
                        Ok(None) if t.elapsed() > Duration::from_millis(200) => {
                            let _ = c.kill();
                            let _ = c.wait();
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            }
            let why = match status {
                Some(st) if !why.contains(&st) => format!("{why} ({st})"),
                _ => why,
            };
            // the process is gone, but the thread reading its stderr may not have passed on the last lines yet (a loaded
            // machine): wait for the pipes to end, but never long
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(300) {
                match self.rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(Incoming::Stderr(l)) => {
                        if let Ok(mut tl) = self.tail.lock() {
                            if tl.len() >= 20 {
                                tl.pop_front();
                            }
                            tl.push_back(l);
                        }
                    }
                    Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let tail: Vec<String> = self.tail.lock().map(|t| t.iter().rev().take(3).rev().cloned().collect()).unwrap_or_default();
            let why = if tail.is_empty() { why } else { format!("{why}: {}", tail.join(" | ")) };
            self.dead = Some(why.clone());
            out.push(Event::Exited(why));
        }
        out
    }

    /// Copy the changed parts of each pending frame out of its slot, oldest first (a slot is only guaranteed to be right inside
    /// the rectangles its own frame announced). True when the picture changed.
    fn take_frames(&mut self, frames: &[(u8, u32, u32, Vec<Rect>)]) -> bool {
        let mut changed = false;
        for (slot, w, h, rects) in frames {
            let (w, h) = (*w, *h);
            if *slot > 1 || w == 0 || h == 0 || w > self.cfg.hello.max_w || h > self.cfg.hello.max_h {
                continue;
            }
            let full = Rect::new(0, 0, w, h);
            let resized = self.size != (w, h) || self.pixels.len() != (w as usize) * (h as usize) * 4;
            if resized {
                self.pixels = vec![0; (w as usize) * (h as usize) * 4];
                self.size = (w, h);
            }
            let list: Vec<Rect> = if resized || rects.is_empty() { vec![full] } else { rects.iter().map(|r| r.clip(w, h)).filter(|r| !r.is_empty()).collect() };
            let base = *slot as u64 * self.shm.slot_bytes;
            let stride = w as usize * 4;
            for rect in list {
                let ok = if rect.x == 0 && rect.w == w {
                    let off = rect.y as usize * stride;
                    let len = rect.h as usize * stride;
                    self.shm.file.read_exact_at(&mut self.pixels[off..off + len], base + off as u64).is_ok()
                } else {
                    (rect.y..rect.y + rect.h).all(|row| {
                        let off = row as usize * stride + rect.x as usize * 4;
                        self.shm.file.read_exact_at(&mut self.pixels[off..off + rect.w as usize * 4], base + off as u64).is_ok()
                    })
                };
                if ok {
                    self.dirty = Some(self.dirty.map_or(rect, |d| d.union(rect)));
                    changed = true;
                }
            }
        }
        self.frames += frames.len() as u64;
        changed
    }

    /// The latest picture: BGRA, premultiplied, rows of `width * 4` bytes, kept complete between frames.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Width and height of [`WebView::pixels`] (0 x 0 before the first frame).
    pub fn frame_size(&self) -> (u32, u32) {
        self.size
    }

    /// The part of the picture that changed since the last call.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.dirty.take()
    }

    pub fn frames_received(&self) -> u64 {
        self.frames
    }

    /// `(engine, version)` once the helper has said it is ready.
    pub fn engine(&self) -> Option<(&str, &str)> {
        self.ready.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))
    }

    pub fn is_ready(&self) -> bool {
        self.ready.is_some()
    }

    /// Why the helper is gone, if it is.
    pub fn exited(&self) -> Option<&str> {
        self.dead.as_deref()
    }

    pub fn since_start(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The address the helper last reported as committed (what a restart returns to; an address that crashed the engine is never replayed).
    pub fn last_url(&self) -> &str {
        &self.url
    }

    /// The size last asked for (after clamping).
    pub fn size_wanted(&self) -> (u32, u32) {
        self.want_size
    }

    pub fn shm_path(&self) -> &std::path::Path {
        &self.shm.path
    }

    /// Start a fresh helper (after a crash, or on request) and bring it to where this one was: size, address, zoom.
    pub fn restart(&mut self) -> Result<(), String> {
        self.stop_helper();
        let mut cfg = self.cfg.clone();
        cfg.hello.w = self.want_size.0;
        cfg.hello.h = self.want_size.1;
        let (rx, tx, stream, child, shm, tail, out_bytes, in_msgs) = spawn(&cfg)?;
        self.out_bytes = out_bytes;
        self.in_msgs = in_msgs;
        self.rx = rx;
        self.tx = Some(tx);
        self.stream = Some(stream);
        self.child = Some(child);
        self.shm = shm;
        self.tail = tail;
        self.cfg = cfg;
        self.pixels.clear();
        self.size = (0, 0);
        self.dirty = None;
        self.ready = None;
        self.dead = None;
        self.exit_reported = false;
        self.started = Instant::now();
        self.raw_send(&ToHelper::Hello(self.cfg.hello.clone()));
        if self.zoom != 100 {
            self.raw_send(&ToHelper::Zoom(self.zoom));
        }
        if !self.url.is_empty() {
            self.raw_send(&ToHelper::Navigate(self.url.clone()));
        }
        self.raw_send(&ToHelper::SetVisible(self.visible));
        self.raw_send(&ToHelper::Focus(self.focus));
        Ok(())
    }

    /// Ask the helper to quit and make sure it does: a thread waits for it and kills it after two seconds.
    fn stop_helper(&mut self) {
        if let (Some(tx), None) = (&self.tx, &self.dead) {
            let _ = tx.send(ToHelper::Quit.encode());
        }
        self.tx = None; // the writer thread ends once the quit message is written
        if let Some(mut child) = self.child.take() {
            let stream = self.stream.take();
            std::thread::spawn(move || {
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(2) {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
                drop(stream);
            });
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        self.stop_helper();
    }
}

type Spawned = (Receiver<Incoming>, Sender<Vec<u8>>, UnixStream, Child, Shm, Arc<Mutex<VecDeque<String>>>, Arc<std::sync::atomic::AtomicUsize>, Arc<std::sync::atomic::AtomicUsize>);

fn spawn(cfg: &Config) -> Result<Spawned, String> {
    // at most every few seconds: a directory listing per started view is plenty
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) >= 5 {
        LAST.store(now, Ordering::Relaxed);
        remove_stale_shm(&cfg.shm_dir);
    }
    let hello = &cfg.hello;
    let in_msgs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let out_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let slot_bytes = hello.max_w as u64 * hello.max_h as u64 * 4;
    let path = cfg.shm_dir.join(format!("iyakki-webview-{}-{}.shm", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let shm = Shm { file, path, slot_bytes };
    shm.file.set_len(slot_bytes * 2).map_err(|e| format!("{}: {e}", shm.path.display()))?;
    let (mine, theirs) = UnixStream::pair().map_err(|e| format!("socket: {e}"))?;
    let mut cmd = Command::new(&cfg.program);
    cmd.args(&cfg.args).arg("--socket-fd").arg("0").arg("--shm").arg(&shm.path);
    if let Some(env) = &cfg.env {
        cmd.env_clear().envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    if let Some(d) = &cfg.cwd {
        cmd.current_dir(d);
    }
    cmd.stdin(Stdio::from(OwnedFd::from(theirs))).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", cfg.program.display()))?;
    let (itx, rx) = channel();
    // messages from the helper
    let mut reader = mine.try_clone().map_err(|e| e.to_string())?;
    let itx2 = itx.clone();
    let counter = in_msgs.clone();
    std::thread::Builder::new()
        .name("iyakki-webview-read".into())
        .spawn(move || loop {
            match proto::read_frame(&mut reader) {
                Ok(Some((ty, p))) => match FromHelper::decode(ty, &p) {
                    Ok(m) => {
                        if counter.fetch_add(1, Ordering::Relaxed) > MAX_QUEUED_IN {
                            let _ = itx2.send(Incoming::End("the helper flooded the shell with messages".into()));
                            return;
                        }
                        if itx2.send(Incoming::Msg(m)).is_err() {
                            return;
                        }
                    }
                    // an unknown or damaged message is skipped, the conversation goes on
                    Err(proto::DecodeError::Unknown(_)) => {}
                    Err(e) => {
                        let _ = itx2.send(Incoming::Stderr(format!("(bad message from the helper: {e})")));
                    }
                },
                Ok(None) => {
                    let _ = itx2.send(Incoming::End("the helper closed the connection".into()));
                    return;
                }
                Err(e) => {
                    let _ = itx2.send(Incoming::End(format!("the connection to the helper failed: {e}")));
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    // the helper's standard error, line by line
    if let Some(err) = child.stderr.take() {
        let itx3 = itx;
        let counter3 = in_msgs.clone();
        std::thread::Builder::new()
            .name("iyakki-webview-log".into())
            .spawn(move || {
                // read bytes, not text: a line that is not UTF-8 must not stop the draining (a full pipe would hang the helper)
                let mut r = BufReader::new(err);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match std::io::BufRead::read_until(&mut r, b'\n', &mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let line: String = String::from_utf8_lossy(&buf).trim_end().chars().take(400).collect();
                    counter3.fetch_add(1, Ordering::Relaxed);
                    if itx3.send(Incoming::Stderr(line)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    // messages to the helper
    let (tx, wrx) = channel::<Vec<u8>>();
    let mut writer = mine.try_clone().map_err(|e| e.to_string())?;
    let sent = out_bytes.clone();
    std::thread::Builder::new()
        .name("iyakki-webview-write".into())
        .spawn(move || {
            for buf in wrx {
                let n = buf.len();
                let ok = writer.write_all(&buf).is_ok();
                sent.fetch_sub(n.min(sent.load(Ordering::Relaxed)), Ordering::Relaxed);
                if !ok {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((rx, tx, mine, child, shm, Arc::new(Mutex::new(VecDeque::new())), out_bytes, in_msgs))
}
