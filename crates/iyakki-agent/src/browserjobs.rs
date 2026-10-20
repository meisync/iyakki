//! The queue between the assistant's browser tools (worker threads) and the page engine (the UI thread of the shell, or a
//! headless thread for `iyakki browser-test`).
//!
//! The embedded engine lives where the shell is not `Send`, so the tools never touch it. They put an [`Op`] in
//! [`BrowserJobs`] and wait for the [`Reply`]; whoever owns the engine calls [`Servicer::service`] on every tick with a
//! [`PageEngine`] and answers. The operations are the small ones of `docs/WEBVIEW-PROTOCOL.md` (navigate, run a script, send a
//! mouse or key event, read the latest picture); everything clever (snapshots, consent, retries) happens in the tools.
//!
//! Secrets: [`Op`]'s `Debug` never prints script text or typed text, so a password typed with real input cannot reach a log.

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use iyakki_webview::proto::ToHelper;

/// The automation script bundle (`crates/iyakki-shell/assets/automation.js`), evaluated in the page before its first use.
pub const BUNDLE: &str = include_str!("../../iyakki-shell/assets/automation.js");
pub const BUNDLE_VERSION: u32 = 1;

/// What the engine knows about its page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub ready: bool,
    /// Why the engine is gone (it crashed or was closed), if it is.
    pub gone: Option<String>,
}

#[derive(Clone)]
pub enum Op {
    Navigate(String),
    Back,
    Forward,
    Reload,
    Stop,
    /// Run JavaScript in the page and answer with its JSON text.
    Eval { js: String, timeout_ms: u32 },
    /// A real mouse event in CSS pixels of the page: 0 move, 1 down, 2 up.
    Mouse { kind: u8, x: i32, y: i32 },
    Wheel { x: i32, y: i32, dy: i32 },
    Key { keysym: u32, mods: u32, text: String },
    /// Insert text into the focused field (like a paste). Never logged.
    Text(String),
    Screenshot,
    State,
    /// Resize the page's window (CSS pixels): phone and desktop layouts for QA flows.
    Resize { w: u32, h: u32 },
    /// The downloads the page started since the last call, as JSON `[{"url","name"}]`.
    Downloads,
    /// End the engine (the helper is stopped).
    Close,
}

impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Op::Navigate(u) => write!(f, "Navigate({})", iyakki_sec::url_host(u).unwrap_or_default()),
            Op::Back => write!(f, "Back"),
            Op::Forward => write!(f, "Forward"),
            Op::Reload => write!(f, "Reload"),
            Op::Stop => write!(f, "Stop"),
            Op::Eval { js, .. } => write!(f, "Eval({} chars)", js.len()),
            Op::Mouse { kind, x, y } => write!(f, "Mouse({kind},{x},{y})"),
            Op::Wheel { x, y, dy } => write!(f, "Wheel({x},{y},{dy})"),
            Op::Key { keysym, .. } => write!(f, "Key({keysym:#x})"),
            Op::Text(t) => write!(f, "Text({} chars)", t.chars().count()),
            Op::Screenshot => write!(f, "Screenshot"),
            Op::State => write!(f, "State"),
            Op::Resize { w, h } => write!(f, "Resize({w}x{h})"),
            Op::Downloads => write!(f, "Downloads"),
            Op::Close => write!(f, "Close"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Reply {
    Ok,
    Json(String),
    /// The latest picture, raw (the tool thread makes the PNG, not the thread that draws the screen).
    Frame { w: u32, h: u32, rgba: Vec<u8> },
    State(PageState),
    Err(String),
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reply::Ok => write!(f, "Ok"),
            Reply::Json(j) => write!(f, "Json({} chars)", j.len()),
            Reply::Frame { w, h, .. } => write!(f, "Frame({w}x{h})"),
            Reply::State(s) => write!(f, "State({})", s.url),
            Reply::Err(e) => write!(f, "Err({e})"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub user: String,
    pub op: Op,
}

#[derive(Default)]
struct St {
    next: u64,
    pending: VecDeque<Job>,
    done: HashMap<u64, Reply>,
    beat: HashMap<String, Instant>,
    /// Who is waiting for which job (so a dropped waiter's late answer is not kept forever).
    waiting: HashMap<u64, ()>,
}

/// The queue: tools `submit` and `wait`; the engine's owner `take`s and `complete`s.
#[derive(Default)]
pub struct BrowserJobs {
    st: Mutex<St>,
    cv: Condvar,
}

impl BrowserJobs {
    pub fn new() -> BrowserJobs {
        BrowserJobs::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn submit(&self, user: &str, op: Op) -> u64 {
        let mut g = self.lock();
        g.next += 1;
        let id = g.next;
        g.waiting.insert(id, ());
        g.pending.push_back(Job { id, user: user.to_string(), op });
        id
    }

    /// Queue a job nobody will wait for (the person's own navigation in the IDE): its answer is dropped, nothing is kept.
    pub fn submit_detached(&self, user: &str, op: Op) {
        let mut g = self.lock();
        g.next += 1;
        let id = g.next;
        g.pending.push_back(Job { id, user: user.to_string(), op });
    }

    /// Wait for the answer to a job; `None` when the time ran out (the job is withdrawn if nobody started it).
    pub fn wait(&self, id: u64, timeout: Duration) -> Option<Reply> {
        let end = Instant::now() + timeout;
        let mut g = self.lock();
        loop {
            if let Some(r) = g.done.remove(&id) {
                g.waiting.remove(&id);
                return Some(r);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                g.pending.retain(|j| j.id != id);
                g.waiting.remove(&id);
                return None;
            }
            g = self.cv.wait_timeout(g, left.min(Duration::from_millis(100))).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Submit and wait; an error reply, or no reply in time, is an `Err`.
    pub fn call(&self, user: &str, op: Op, timeout: Duration) -> Result<Reply, String> {
        let id = self.submit(user, op);
        match self.wait(id, timeout) {
            Some(Reply::Err(e)) => Err(e),
            Some(r) => Ok(r),
            None => Err("the browser did not answer in time".into()),
        }
    }

    /// The jobs waiting for this person's engine, oldest first.
    pub fn take(&self, user: &str) -> Vec<Job> {
        let mut g = self.lock();
        let (mine, rest): (Vec<Job>, Vec<Job>) = std::mem::take(&mut g.pending).into_iter().partition(|j| j.user == user);
        g.pending = rest.into();
        mine
    }

    pub fn complete(&self, id: u64, r: Reply) {
        let mut g = self.lock();
        if g.waiting.contains_key(&id) {
            g.done.insert(id, r);
            self.cv.notify_all();
        }
    }

    /// The engine's owner says it is looking at the queue (called on every tick, also when idle).
    pub fn beat(&self, user: &str) {
        self.lock().beat.insert(user.to_string(), Instant::now());
    }

    /// Someone has serviced this person's queue within `within`.
    pub fn serviced(&self, user: &str, within: Duration) -> bool {
        self.lock().beat.get(user).is_some_and(|t| t.elapsed() <= within)
    }

    pub fn pending_for(&self, user: &str) -> usize {
        self.lock().pending.iter().filter(|j| j.user == user).count()
    }

    /// Answer every waiting job of a person with an error (the engine is gone, or they signed out).
    pub fn fail_all(&self, user: &str, why: &str) {
        let mut g = self.lock();
        let mine: Vec<u64> = g.pending.iter().filter(|j| j.user == user).map(|j| j.id).collect();
        g.pending.retain(|j| j.user != user);
        for id in mine {
            if g.waiting.contains_key(&id) {
                g.done.insert(id, Reply::Err(why.to_string()));
            }
        }
        self.cv.notify_all();
    }
}

/// A picture of the page: RGBA, opaque, `w * h * 4` bytes.
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// What the servicing side needs from an engine: the shell's `Full` view, the headless one below, or a test double.
pub trait PageEngine {
    fn send(&mut self, m: ToHelper) -> bool;
    /// Take in what the helper has sent since the last call.
    fn pump(&mut self);
    fn state(&self) -> PageState;
    /// The answer to a `Script` message, once it has arrived.
    fn take_script(&mut self, id: u32) -> Option<(bool, String)>;
    fn frame(&self) -> Option<Frame>;
    /// Device pixels per CSS pixel (for real mouse positions).
    fn scale(&self) -> f32 {
        1.0
    }
    /// Downloads the page started since the last call: (address, suggested name).
    fn take_downloads(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
}

struct Inflight {
    job: u64,
    script: u32,
    deadline: Instant,
}

/// Turns queued [`Op`]s into protocol messages and helper answers back into [`Reply`]s. One per engine.
#[derive(Default)]
pub struct Servicer {
    inflight: Vec<Inflight>,
    next_script: u32,
}

/// What a call to [`Servicer::service`] did.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Serviced {
    /// Jobs were taken or answered (the engine is in use).
    pub busy: bool,
    /// A `Close` job asked for the engine to be shut down.
    pub close: bool,
}

impl Servicer {
    pub fn new() -> Servicer {
        Servicer { inflight: Vec::new(), next_script: 1 }
    }

    pub fn service(&mut self, jobs: &BrowserJobs, user: &str, eng: &mut dyn PageEngine) -> Serviced {
        let mut out = Serviced::default();
        jobs.beat(user);
        eng.pump();
        let state = eng.state();
        if self.next_script == 0 {
            self.next_script = 1;
        }
        for job in jobs.take(user) {
            out.busy = true;
            if let Some(why) = &state.gone {
                jobs.complete(job.id, Reply::Err(format!("the browser engine stopped ({why}); it is restarted for the next call")));
                continue;
            }
            match job.op {
                Op::Navigate(u) => {
                    eng.send(ToHelper::Navigate(u));
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Back => {
                    eng.send(ToHelper::Back);
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Forward => {
                    eng.send(ToHelper::Forward);
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Reload => {
                    eng.send(ToHelper::Reload { hard: false });
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Stop => {
                    eng.send(ToHelper::Stop);
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Eval { js, timeout_ms } => {
                    let id = self.next_script;
                    self.next_script = self.next_script.wrapping_add(1).max(1);
                    if eng.send(ToHelper::Script { id, js, timeout_ms }) {
                        self.inflight.push(Inflight { job: job.id, script: id, deadline: Instant::now() + Duration::from_millis(timeout_ms as u64 + 1500) });
                    } else {
                        jobs.complete(job.id, Reply::Err("the web engine is not running".into()));
                    }
                }
                Op::Mouse { kind, x, y } => {
                    let s = eng.scale();
                    let (x, y) = ((x as f32 * s).round() as i32, (y as f32 * s).round() as i32);
                    let k = match kind {
                        1 => iyakki_webview::MouseKind::Down,
                        2 => iyakki_webview::MouseKind::Up,
                        _ => iyakki_webview::MouseKind::Move,
                    };
                    let (button, buttons, clicks) = match kind {
                        1 => (iyakki_webview::proto::BUTTON_LEFT, iyakki_webview::proto::HELD_LEFT, 1),
                        2 => (iyakki_webview::proto::BUTTON_LEFT, 0, 1),
                        _ => (0, 0, 0),
                    };
                    eng.send(ToHelper::Mouse { kind: k, x, y, button, buttons, mods: 0, clicks });
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Wheel { x, y, dy } => {
                    let s = eng.scale();
                    eng.send(ToHelper::Wheel { x: (x as f32 * s) as i32, y: (y as f32 * s) as i32, dx: 0, dy, mods: 0 });
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Key { keysym, mods, text } => {
                    eng.send(ToHelper::Key { down: true, keysym, mods, text });
                    eng.send(ToHelper::Key { down: false, keysym, mods, text: String::new() });
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Text(t) => {
                    eng.send(ToHelper::Text(t));
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Screenshot => match eng.frame() {
                    Some(f) if f.w > 0 && f.h > 0 && f.rgba.len() == f.w as usize * f.h as usize * 4 => jobs.complete(job.id, Reply::Frame { w: f.w, h: f.h, rgba: f.rgba }),
                    Some(_) => jobs.complete(job.id, Reply::Err("the page's picture is empty or damaged; try again in a moment".into())),
                    None => jobs.complete(job.id, Reply::Err("the page has not been drawn yet".into())),
                },
                Op::State => jobs.complete(job.id, Reply::State(state.clone())),
                Op::Resize { w, h } => {
                    eng.send(ToHelper::Resize { w, h });
                    jobs.complete(job.id, Reply::Ok);
                }
                Op::Downloads => {
                    let list: Vec<serde_json::Value> = eng.take_downloads().into_iter().map(|(url, name)| serde_json::json!({"url": url, "name": name})).collect();
                    jobs.complete(job.id, Reply::Json(serde_json::Value::Array(list).to_string()));
                }
                Op::Close => {
                    jobs.complete(job.id, Reply::Ok);
                    out.close = true;
                }
            }
        }
        // scripts that have been answered, or ran out of time
        let now = Instant::now();
        let mut still = Vec::new();
        for f in std::mem::take(&mut self.inflight) {
            match eng.take_script(f.script) {
                Some((true, json)) => {
                    out.busy = true;
                    jobs.complete(f.job, Reply::Json(json));
                }
                Some((false, err)) => {
                    out.busy = true;
                    jobs.complete(f.job, Reply::Err(format!("the page script failed: {err}")));
                }
                None if state.gone.is_some() => {
                    jobs.complete(f.job, Reply::Err(format!("the browser engine stopped ({}); it is restarted for the next call", state.gone.clone().unwrap_or_default())));
                }
                None if now > f.deadline => {
                    jobs.complete(f.job, Reply::Err("the page did not answer a script in time".into()));
                }
                None => still.push(f),
            }
        }
        self.inflight = still;
        out
    }

    /// The engine is going away: answer what is still waiting.
    pub fn abort(&mut self, jobs: &BrowserJobs, user: &str, why: &str) {
        for f in self.inflight.drain(..) {
            jobs.complete(f.job, Reply::Err(why.to_string()));
        }
        jobs.fail_all(user, why);
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }
}

// ------------------------------------------------------------------------------------------------ pictures

impl Reply {
    /// A `Frame` answer as a PNG file (empty or inconsistent pictures are an error, not a broken file).
    pub fn into_png(self) -> Result<Vec<u8>, String> {
        match self {
            Reply::Frame { w, h, rgba } if w > 0 && h > 0 && rgba.len() == w as usize * h as usize * 4 => Ok(png::encode_rgba(w, h, &rgba)),
            Reply::Frame { .. } => Err("the page's picture is empty or damaged".into()),
            r => Err(format!("unexpected answer {r:?}")),
        }
    }
}

pub mod png {
    //! A minimal PNG writer (RGBA, 8 bits, no interlace) for screenshots. The zlib stream comes from `miniz_oxide`.

    fn crc_table() -> [u32; 256] {
        let mut t = [0u32; 256];
        for (n, slot) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8], table: &[u32; 256]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = 0xffff_ffffu32;
        for b in kind.iter().chain(data.iter()) {
            crc = table[((crc ^ *b as u32) & 0xff) as usize] ^ (crc >> 8);
        }
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
    }

    /// `rgba` holds `w * h * 4` bytes (shorter input is padded with white).
    pub fn encode_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let table = crc_table();
        let stride = w as usize * 4;
        let mut raw = Vec::with_capacity((stride + 1) * h as usize);
        for y in 0..h as usize {
            raw.push(0); // filter: none
            let a = y * stride;
            let row = rgba.get(a..(a + stride).min(rgba.len())).unwrap_or(&[]);
            raw.extend_from_slice(row);
            raw.extend(std::iter::repeat_n(255u8, stride - row.len()));
        }
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 3);
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr, &table);
        chunk(&mut out, b"IDAT", &z, &table);
        chunk(&mut out, b"IEND", &[], &table);
        out
    }
}

/// BGRA premultiplied pixels (the helper's) to opaque RGBA over white.
pub fn bgra_to_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for p in src.chunks_exact(4) {
        let a = p[3];
        if a == 255 {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        } else {
            let rest = 255 - a;
            out.extend_from_slice(&[p[2].saturating_add(rest), p[1].saturating_add(rest), p[0].saturating_add(rest), 255]);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ headless engine

/// A page engine with no window: the helper program started by this process (for `iyakki browser-test` and live tests).
pub struct Headless {
    view: iyakki_webview::WebView,
    state: PageState,
    scripts: Vec<(u32, bool, String)>,
    scale: f32,
    downloads: Vec<(String, String)>,
}

pub struct HeadlessOptions {
    pub helper: std::path::PathBuf,
    pub profile: Option<std::path::PathBuf>,
    pub env: Option<Vec<(String, String)>>,
    pub size: (u32, u32),
    pub allow_hosts: Vec<String>,
    pub user_agent: String,
}

impl Headless {
    pub fn start(o: HeadlessOptions) -> Result<Headless, String> {
        let hello = iyakki_webview::Hello {
            w: o.size.0,
            h: o.size.1,
            max_w: o.size.0.max(1920),
            max_h: o.size.1.max(1200),
            user_agent: o.user_agent,
            profile: o.profile.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            allow_hosts: o.allow_hosts,
            ..iyakki_webview::Hello::default()
        };
        let mut cfg = iyakki_webview::Config::new(&o.helper, hello);
        cfg.env = o.env;
        cfg.cwd = o.profile;
        let view = iyakki_webview::WebView::start(cfg)?;
        let mut h = Headless { view, state: PageState::default(), scripts: Vec::new(), scale: 1.0, downloads: Vec::new() };
        h.view.send(ToHelper::Focus(true));
        Ok(h)
    }
}

impl PageEngine for Headless {
    fn send(&mut self, m: ToHelper) -> bool {
        self.view.send(m)
    }
    fn pump(&mut self) {
        use iyakki_webview::Event as E;
        for ev in self.view.poll() {
            match ev {
                E::Ready { .. } => self.state.ready = true,
                E::Title(t) => self.state.title = t,
                E::Url(u) => self.state.url = u,
                E::Loading { loading, .. } => self.state.loading = loading,
                E::History { can_back, can_forward } => {
                    self.state.can_back = can_back;
                    self.state.can_forward = can_forward;
                }
                E::ScriptResult { id, ok, json } => self.scripts.push((id, ok, json)),
                E::Download { url, name } => self.downloads.push((url, name)),
                E::Exited(why) => self.state.gone = Some(why),
                _ => {}
            }
        }
        if let Some(why) = self.view.exited() {
            self.state.gone = Some(why.to_string());
        }
    }
    fn state(&self) -> PageState {
        self.state.clone()
    }
    fn take_script(&mut self, id: u32) -> Option<(bool, String)> {
        let i = self.scripts.iter().position(|s| s.0 == id)?;
        let (_, ok, j) = self.scripts.remove(i);
        Some((ok, j))
    }
    fn frame(&self) -> Option<Frame> {
        let (w, h) = self.view.frame_size();
        (w > 0 && h > 0).then(|| Frame { w, h, rgba: bgra_to_rgba(self.view.pixels()) })
    }
    fn scale(&self) -> f32 {
        self.scale
    }
    fn take_downloads(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.downloads)
    }
}

/// A thread that owns a [`Headless`] engine and services one person's queue until dropped.
pub struct HeadlessSession {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HeadlessSession {
    pub fn start(jobs: std::sync::Arc<BrowserJobs>, user: &str, o: HeadlessOptions) -> Result<HeadlessSession, String> {
        let mut eng = Headless::start(o)?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (flag, user) = (stop.clone(), user.to_string());
        let thread = std::thread::Builder::new()
            .name("iyakki-headless-web".into())
            .spawn(move || {
                let mut sv = Servicer::new();
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let r = sv.service(&jobs, &user, &mut eng);
                    if r.close {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(if r.busy { 2 } else { 8 }));
                }
                sv.abort(&jobs, &user, "the browser was closed");
            })
            .map_err(|e| e.to_string())?;
        Ok(HeadlessSession { stop, thread: Some(thread) })
    }
}

impl Drop for HeadlessSession {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_shows_script_or_typed_text() {
        let s = format!("{:?} {:?} {:?}", Op::Text("hunter22".into()), Op::Eval { js: "x='hunter22'".into(), timeout_ms: 5 }, Op::Key { keysym: 0x61, mods: 0, text: "hunter22".into() });
        assert!(!s.contains("hunter22"), "{s}");
        assert!(format!("{:?}", Op::Navigate("https://a.org/secret?token=abc".into())).contains("a.org") && !format!("{:?}", Op::Navigate("https://a.org/secret?token=abc".into())).contains("token"));
    }

    #[test]
    fn jobs_are_taken_by_their_owner_and_answered_to_the_waiter() {
        let jobs = std::sync::Arc::new(BrowserJobs::new());
        let j2 = jobs.clone();
        let t = std::thread::spawn(move || j2.call("ann", Op::State, Duration::from_secs(5)));
        let mut got = Vec::new();
        for _ in 0..200 {
            got = jobs.take("bob");
            assert!(got.is_empty(), "bob cannot take ann's job");
            got = jobs.take("ann");
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1);
        jobs.complete(got[0].id, Reply::Json("{}".into()));
        assert_eq!(t.join().unwrap(), Ok(Reply::Json("{}".into())));
        // no answer: the job is withdrawn
        assert!(jobs.call("ann", Op::State, Duration::from_millis(30)).is_err());
        assert!(jobs.take("ann").is_empty());
        // fail_all answers waiters
        let j3 = jobs.clone();
        let t = std::thread::spawn(move || j3.call("ann", Op::State, Duration::from_secs(5)));
        while jobs.pending_for("ann") == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        jobs.fail_all("ann", "signed out");
        assert_eq!(t.join().unwrap(), Err("signed out".to_string()));
    }

    #[test]
    fn heartbeats_say_whether_anyone_services_the_queue() {
        let jobs = BrowserJobs::new();
        assert!(!jobs.serviced("ann", Duration::from_secs(5)));
        jobs.beat("ann");
        assert!(jobs.serviced("ann", Duration::from_secs(5)) && !jobs.serviced("bob", Duration::from_secs(5)));
    }

    #[test]
    fn the_png_writer_makes_a_valid_file() {
        let rgba: Vec<u8> = (0..4 * 3).flat_map(|i| [i as u8 * 20, 100, 200, 255]).collect();
        let p = png::encode_rgba(4, 3, &rgba);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(p[20..24].try_into().unwrap()), 3);
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        // the picture decoder of the web crate reads it back
        assert_eq!(iyakki_web::image::sniff(&p), iyakki_web::image::Kind::Png);
    }

    #[test]
    fn premultiplied_bgra_becomes_opaque_rgba_over_white() {
        assert_eq!(bgra_to_rgba(&[10, 20, 30, 255]), vec![30, 20, 10, 255]);
        assert_eq!(bgra_to_rgba(&[0, 0, 0, 0]), vec![255, 255, 255, 255]);
    }
}
