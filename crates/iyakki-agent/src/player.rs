//! IYAKKI's own media player for the framebuffer images, which have no window system for mpv, VLC or ffplay: `ffmpeg` is only
//! the **decoder**, IYAKKI shows the picture and keeps the time.
//!
//! * Two `ffmpeg` processes per playback, each started with fixed arguments and never through a shell (BOUNDARY.md): one writes
//!   raw RGBA video frames, scaled to the player area, to its standard output (`-f rawvideo -pix_fmt rgba -`); the other writes
//!   48 kHz stereo PCM (`-f s16le`) that goes to the speaker command (`aplay -D iyakki`, so the volume of Settings -> Sound,
//!   the softvol device, applies).
//! * **The audio is the clock.** The position is the start time plus the PCM written to the speaker, less a fixed lag for what
//!   the pipe and the sound card still hold ([`Config::audio_lag`]); each video frame waits until the clock reaches its time,
//!   and one that is far behind is dropped. A file without sound (or when the speaker command cannot start) uses the wall clock.
//! * Pause holds the clock and stops feeding both; seek, a new size (fullscreen) and "start at" restart both decoders at the
//!   wanted time (`-ss` before `-i`), which also keeps the picture exact.
//! * **Tests use a fake ffmpeg** (a shell script that prints a few frames and some PCM) and a fake speaker: no media and no
//!   device is touched. The shell draws [`Player::frame`] and calls the control methods; this module knows nothing about the screen.
//!
//! Subtitles are not drawn. The pixel path costs one copy per frame; the decoder does the scaling.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 48 kHz, 16-bit, stereo.
const BYTES_PER_SECOND: f64 = 48_000.0 * 4.0;
const CHUNK: usize = 4096;

/// Folders besides `PATH` where a downloaded `ffmpeg` lives (the CLI catalogue's `bin` folder); the shell registers it.
static EXTRA_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Tell the player where the CLI catalogue links its programs (`CliManager::bin_dir`).
pub fn register_dir(dir: PathBuf) {
    let mut g = EXTRA_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if !g.contains(&dir) {
        g.push(dir);
    }
}

/// The path of a program: a name looked up in the registered folders, then `PATH` and the usual places.
pub fn find_program(name: &str) -> Option<String> {
    if name.contains('/') {
        return std::path::Path::new(name).is_file().then(|| name.to_string());
    }
    let extra: Vec<String> = EXTRA_DIRS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|d| d.display().to_string()).collect();
    let path = std::env::var("PATH").unwrap_or_default();
    let found = extra.iter().map(String::as_str).chain(path.split(':').filter(|d| !d.is_empty())).chain(["/usr/local/bin", "/usr/bin", "/bin"]).map(|d| format!("{d}/{name}")).find(|p| std::path::Path::new(p).is_file());
    found
}

/// Whether the in-house player can run here: an `ffmpeg` is installed and starts. A program whose libraries are missing (an image
/// that dropped the FFmpeg libraries but kept the program) does not count: it would only fail with a linker message.
pub fn available() -> bool {
    find_program("ffmpeg").is_some_and(|p| runs(&p))
}

/// `program -version` starts and succeeds. Checked once per file (path, size and change time) and remembered.
pub fn runs(program: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashMap<(String, u64, u64), bool>>> = OnceLock::new();
    let key = std::fs::metadata(program).map(|m| (program.to_string(), m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()))).ok();
    let Some(key) = key else { return false };
    let cache = SEEN.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *v;
    }
    let ok = std::process::Command::new(program).arg("-version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, ok);
    ok
}

/// What to tell the person when the decoder's own message is a dynamic-linker one (`Error relocating X: Y: symbol not found`,
/// `Error loading shared library libZ`, `undefined symbol`): the program is there but parts of its libraries are not.
pub fn plain_reason(text: &str) -> String {
    let l = text.to_ascii_lowercase();
    if l.contains("symbol not found") || l.contains("error relocating") || l.contains("undefined symbol") || l.contains("error loading shared library") || l.contains("cannot open shared object") {
        "the installed ffmpeg is incomplete (its libraries are missing or do not match, so it cannot start). Install a working one from Store \u{2192} Setup (Media tools)".to_string()
    } else {
        text.to_string()
    }
}

/// How the player starts its processes.
#[derive(Clone, Debug)]
pub struct Config {
    pub ffmpeg: String,
    /// The speaker command; PCM (48 kHz, S16_LE, stereo) goes to its standard input. Empty: no sound.
    pub audio_cmd: Vec<String>,
    /// Frames per second shown at most.
    pub max_fps: f64,
    /// Seconds of sound the pipe and the card hold ahead of what is heard.
    pub audio_lag: f64,
}

impl Config {
    /// The real thing: `ffmpeg` from the catalogue or the system, the speaker through IYAKKI's sound device.
    pub fn system() -> Option<Config> {
        let ffmpeg = find_program("ffmpeg").filter(|p| runs(p))?;
        Some(Config { ffmpeg, audio_cmd: speaker_command(), max_fps: 30.0, audio_lag: 0.45 })
    }
}

/// `aplay` on IYAKKI's output (the softvol device, so the volume applies), or nothing when there is no `aplay`.
pub fn speaker_command() -> Vec<String> {
    match find_program("aplay") {
        Some(a) => [&a[..], "-q", "-D", crate::softvol::PCM, "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-"].iter().map(|s| s.to_string()).collect(),
        None => Vec::new(),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioInfo {
    pub codec: String,
    pub hz: u32,
    pub layout: String,
}

/// What `ffmpeg -i` says about a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub duration: Option<f64>,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

/// `1:05` or `1:02:03`.
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl Info {
    /// The media info line: `h264 1280x720 25 fps · aac 44100 Hz stereo · 1:23`.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.video {
            parts.push(format!("{} {}x{} {} fps", v.codec, v.w, v.h, if v.fps.fract() == 0.0 { format!("{}", v.fps as u32) } else { format!("{:.2}", v.fps) }));
        }
        if let Some(a) = &self.audio {
            parts.push(format!("{} {} Hz {}", a.codec, a.hz, a.layout).trim().to_string());
        }
        if let Some(d) = self.duration {
            parts.push(clock_text(d));
        }
        parts.join(" \u{b7} ")
    }
}

/// Read the banner `ffmpeg -i FILE` prints on its error output.
pub fn parse_probe(text: &str) -> Info {
    let mut info = Info::default();
    for l in text.lines() {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("Duration:") {
            let t = rest.split(',').next().unwrap_or("").trim();
            let parts: Vec<&str> = t.split(':').collect();
            if parts.len() == 3 {
                if let (Ok(h), Ok(m), Ok(s)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
                    info.duration = Some(h * 3600.0 + m * 60.0 + s);
                }
            }
        } else if l.starts_with("Stream #") {
            let Some((_, body)) = l.split_once(": Video: ") else {
                if let Some((_, body)) = l.split_once(": Audio: ") {
                    if info.audio.is_none() {
                        let f: Vec<&str> = body.split(", ").collect();
                        let hz = f.iter().find_map(|x| x.strip_suffix(" Hz").and_then(|n| n.trim().parse().ok())).unwrap_or(0);
                        let layout = f.iter().find(|x| ["mono", "stereo", "5.1", "7.1", "2.1", "quad"].iter().any(|k| x.starts_with(k))).map(|x| x.split('(').next().unwrap_or(x).trim().to_string()).unwrap_or_default();
                        info.audio = Some(AudioInfo { codec: f[0].split_whitespace().next().unwrap_or("").to_string(), hz, layout });
                    }
                }
                continue;
            };
            if info.video.is_some() || body.contains("attached pic") {
                continue;
            }
            let f: Vec<&str> = body.split(", ").collect();
            let size = f.iter().find_map(|x| {
                let x = x.split_whitespace().next()?;
                let (w, h) = x.split_once('x')?;
                Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
            });
            let fps = f.iter().find_map(|x| x.strip_suffix(" fps").and_then(|n| n.trim().parse::<f64>().ok())).unwrap_or(0.0);
            if let Some((w, h)) = size {
                info.video = Some(VideoInfo { codec: f[0].split_whitespace().next().unwrap_or("").to_string(), w, h, fps });
            }
        }
    }
    info
}

/// The size a picture of `src` gets inside `area`: as large as fits, the proportions kept, both sides even (the decoder wants it).
pub fn fit(src: (u32, u32), area: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (src.0.max(1) as f64, src.1.max(1) as f64);
    let k = (area.0.max(2) as f64 / sw).min(area.1.max(2) as f64 / sh);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    (even(sw * k), even(sh * k))
}

fn path_arg(file: &str) -> String {
    // a name that starts with a dash would be read as an option
    if file.starts_with('-') {
        format!("./{file}")
    } else {
        file.to_string()
    }
}

/// The arguments of the probe: the banner is on the error output and the exit status is "no output file", both expected.
pub fn probe_args(file: &str) -> Vec<String> {
    ["-hide_banner", "-nostdin", "-i"].iter().map(|s| s.to_string()).chain([path_arg(file)]).collect()
}

/// The arguments of the video decoder: raw RGBA frames of `w` x `h`, `fps` a second, from `start` seconds, on standard output.
pub fn video_args(file: &str, start: f64, w: u32, h: u32, fps: f64) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-v", "error", "-ss"].iter().map(|s| s.to_string()).collect();
    a.push(format!("{start:.3}"));
    a.extend(["-i".to_string(), path_arg(file)]);
    a.extend(["-an", "-sn", "-vf"].iter().map(|s| s.to_string()));
    a.push(format!("fps={fps},scale={w}:{h}"));
    a.extend(["-f", "rawvideo", "-pix_fmt", "rgba", "-"].iter().map(|s| s.to_string()));
    a
}

/// The arguments of the sound decoder: 48 kHz stereo 16-bit PCM from `start` seconds on standard output.
pub fn audio_args(file: &str, start: f64) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-v", "error", "-ss"].iter().map(|s| s.to_string()).collect();
    a.push(format!("{start:.3}"));
    a.extend(["-i".to_string(), path_arg(file)]);
    a.extend(["-vn", "-sn", "-f", "s16le", "-ar", "48000", "-ac", "2", "-"].iter().map(|s| s.to_string()));
    a
}

fn spawn(prog: &str, args: &[String], stdin: bool) -> Result<Child, String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(prog);
    cmd.args(args).process_group(0).stdin(if stdin { Stdio::piped() } else { Stdio::null() }).stdout(if stdin { Stdio::null() } else { Stdio::piped() }).stderr(Stdio::piped());
    crate::aacpipe::spawn_retry(&mut cmd)
        .map_err(|e| format!("could not start {}: {e}", prog.rsplit('/').next().unwrap_or(prog)))
}

/// Read a process's error output on the side (a few lines at most are kept), so it never blocks on a full pipe.
fn drain(mut e: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let out = Arc::new(Mutex::new(String::new()));
    let o = out.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = e.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut g = o.lock().unwrap_or_else(|x| x.into_inner());
            if g.len() < 600 {
                g.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
    });
    out
}

/// Ask `ffmpeg` about a file (name, picture size, sound, length).
pub fn probe(cfg: &Config, file: &str) -> Result<Info, String> {
    let mut child = spawn(&cfg.ffmpeg, &probe_args(file), false)?;
    let mut err = child.stderr.take().ok_or("no output from ffmpeg")?;
    drop(child.stdout.take());
    let mut text = String::new();
    let mut raw = Vec::new();
    let _ = (&mut err).take(256 * 1024).read_to_end(&mut raw);
    text.push_str(&String::from_utf8_lossy(&raw));
    let _ = child.wait();
    let info = parse_probe(&text);
    if info.video.is_none() && info.audio.is_none() {
        let why = text.lines().rev().find(|l| !l.trim().is_empty() && !l.contains("At least one output")).unwrap_or("it has no picture or sound ffmpeg knows").trim().to_string();
        return Err(if text.lines().any(|l| plain_reason(l) != l) { plain_reason(&text) } else { why });
    }
    Ok(info)
}

/// One picture, ready to draw (RGBA, opaque).
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Grows with every new picture, so the shell can tell whether it has drawn this one.
    pub seq: u64,
    pub time: f64,
}

/// What the screen shows about the playback.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub ended: bool,
    pub error: Option<String>,
}

struct State {
    gen: u64,
    paused: bool,
    frozen: Option<f64>,
    start: f64,
    audio_active: bool,
    audio_bytes: u64,
    anchor_pos: f64,
    anchor_at: Instant,
    video_done: bool,
    audio_done: bool,
    has_video: bool,
    has_audio: bool,
    ended: bool,
    error: Option<String>,
    frame: Option<Arc<Frame>>,
    size: (u32, u32),
}

struct Shared {
    cfg: Config,
    file: String,
    info: Info,
    st: Mutex<State>,
    kids: Mutex<Vec<Child>>,
    seq: AtomicU64,
}

impl Shared {
    fn position_locked(&self, s: &State) -> f64 {
        if let Some(f) = s.frozen {
            return f;
        }
        let p = if s.audio_active { s.start + s.audio_bytes as f64 / BYTES_PER_SECOND - self.cfg.audio_lag } else { s.anchor_pos + s.anchor_at.elapsed().as_secs_f64() };
        let p = p.max(s.start);
        match self.info.duration {
            Some(d) => p.min(d),
            None => p,
        }
    }
    fn kill_all(&self) {
        let mut g = self.kids.lock().unwrap_or_else(|e| e.into_inner());
        for c in g.iter_mut() {
            crate::voice::kill_child(c);
            let _ = c.wait();
        }
        g.clear();
    }
    fn is_current(&self, gen: u64) -> bool {
        self.st.lock().unwrap_or_else(|e| e.into_inner()).gen == gen
    }
    fn finish_part(&self, gen: u64, video: bool, error: Option<String>) {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.gen != gen {
            return;
        }
        if video {
            s.video_done = true;
        } else {
            s.audio_done = true;
        }
        if let Some(e) = error {
            s.error.get_or_insert(e);
        }
        if (!s.has_video || s.video_done) && (!s.has_audio || s.audio_done) {
            s.ended = true;
        }
    }
}

/// A playback in progress. Dropping it stops the decoders.
pub struct Player {
    sh: Arc<Shared>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Player {
    /// Start `file` at `start` seconds with the picture fitted into `area` (width, height in pixels).
    pub fn start(cfg: Config, file: &str, start: f64, area: (u32, u32)) -> Result<Player, String> {
        let info = probe(&cfg, file)?;
        let size = info.video.as_ref().map_or((0, 0), |v| fit((v.w, v.h), area));
        let has_audio = info.audio.is_some();
        let sh = Arc::new(Shared {
            cfg,
            file: file.to_string(),
            info,
            st: Mutex::new(State { gen: 0, paused: false, frozen: None, start: 0.0, audio_active: false, audio_bytes: 0, anchor_pos: 0.0, anchor_at: Instant::now(), video_done: false, audio_done: false, has_video: false, has_audio, ended: false, error: None, frame: None, size }),
            kids: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
        });
        let p = Player { sh };
        p.restart(start);
        Ok(p)
    }

    pub fn info(&self) -> &Info {
        &self.sh.info
    }

    /// Start (again) both decoders at `at` seconds, keeping the paused state.
    fn restart(&self, at: f64) {
        let sh = &self.sh;
        sh.kill_all();
        let at = match sh.info.duration {
            Some(d) => at.clamp(0.0, (d - 0.3).max(0.0)),
            None => at.max(0.0),
        };
        let (gen, size, paused) = {
            let mut s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
            s.gen += 1;
            s.start = at;
            s.audio_bytes = 0;
            s.anchor_pos = at;
            s.anchor_at = Instant::now();
            s.video_done = false;
            s.audio_done = false;
            s.has_video = sh.info.video.is_some() && s.size.0 > 0;
            s.has_audio = sh.info.audio.is_some() && !sh.cfg.audio_cmd.is_empty();
            s.audio_active = s.has_audio;
            s.ended = false;
            s.error = None;
            s.frozen = if s.paused { Some(at) } else { None };
            (s.gen, s.size, s.paused)
        };
        let _ = paused;
        let (has_video, has_audio) = {
            let s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
            (s.has_video, s.has_audio)
        };
        if has_video {
            let fps = sh.info.video.as_ref().map_or(25.0, |v| if v.fps >= 1.0 { v.fps } else { 25.0 }).clamp(5.0, sh.cfg.max_fps);
            let args = video_args(&sh.file, at, size.0, size.1, fps);
            match spawn(&sh.cfg.ffmpeg, &args, false) {
                Ok(mut c) => {
                    let out = c.stdout.take();
                    let err = c.stderr.take().map(drain);
                    sh.kids.lock().unwrap_or_else(|e| e.into_inner()).push(c);
                    let sh2 = sh.clone();
                    std::thread::spawn(move || video_thread(sh2, gen, out, err, size, fps, at));
                }
                Err(e) => sh.finish_part(gen, true, Some(e)),
            }
        }
        if has_audio {
            match spawn(&sh.cfg.ffmpeg, &audio_args(&sh.file, at), false) {
                Ok(mut c) => {
                    let out = c.stdout.take();
                    let err = c.stderr.take().map(drain);
                    sh.kids.lock().unwrap_or_else(|e| e.into_inner()).push(c);
                    let sh2 = sh.clone();
                    std::thread::spawn(move || audio_thread(sh2, gen, out, err));
                }
                Err(e) => sh.finish_part(gen, false, Some(e)),
            }
        }
        if !has_video && !has_audio {
            sh.finish_part(gen, true, Some("there is nothing to play in this file".into()));
        }
    }

    pub fn status(&self) -> Status {
        let s = self.sh.st.lock().unwrap_or_else(|e| e.into_inner());
        Status { position: self.sh.position_locked(&s), duration: self.sh.info.duration, paused: s.paused, ended: s.ended, error: s.error.clone() }
    }

    /// The newest picture when it is newer than `seen` (the `seq` of the one last drawn; 0 for none).
    pub fn frame(&self, seen: u64) -> Option<Arc<Frame>> {
        self.sh.st.lock().unwrap_or_else(|e| e.into_inner()).frame.clone().filter(|f| f.seq > seen)
    }

    pub fn pause(&self) {
        let mut s = self.sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if !s.paused && !s.ended {
            let p = self.sh.position_locked(&s);
            s.frozen = Some(p);
            s.paused = true;
        }
    }

    pub fn resume(&self) {
        let mut s = self.sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.paused {
            s.paused = false;
            if let Some(p) = s.frozen.take() {
                s.anchor_pos = p;
                s.anchor_at = Instant::now();
            }
        }
    }

    pub fn toggle_pause(&self) {
        let paused = self.sh.st.lock().unwrap_or_else(|e| e.into_inner()).paused;
        if paused {
            self.resume();
        } else {
            self.pause();
        }
    }

    /// Jump to `secs` (the decoders start again there).
    pub fn seek_to(&self, secs: f64) {
        self.restart(secs);
    }

    /// Jump by `delta` seconds from where the playback is.
    pub fn seek_by(&self, delta: f64) {
        let p = self.status().position;
        self.restart(p + delta);
    }

    /// The area changed (fullscreen, a resized window): a picture that no longer fits is decoded again at the new size, from the same place.
    pub fn set_area(&self, area: (u32, u32)) {
        let Some(v) = &self.sh.info.video else { return };
        let want = fit((v.w, v.h), area);
        let (cur, p) = {
            let s = self.sh.st.lock().unwrap_or_else(|e| e.into_inner());
            (s.size, self.sh.position_locked(&s))
        };
        if (want.0 as i64 - cur.0 as i64).abs() < 16 && (want.1 as i64 - cur.1 as i64).abs() < 16 {
            return;
        }
        self.sh.st.lock().unwrap_or_else(|e| e.into_inner()).size = want;
        self.restart(p);
    }

    pub fn stop(&self) {
        {
            let mut s = self.sh.st.lock().unwrap_or_else(|e| e.into_inner());
            s.gen += 1;
            s.ended = true;
        }
        self.sh.kill_all();
    }
}

fn tail(e: &Option<Arc<Mutex<String>>>) -> Option<String> {
    let t = e.as_ref()?.lock().unwrap_or_else(|x| x.into_inner()).clone();
    t.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().chars().take(200).collect())
}

fn video_thread(sh: Arc<Shared>, gen: u64, out: Option<std::process::ChildStdout>, err: Option<Arc<Mutex<String>>>, size: (u32, u32), fps: f64, start: f64) {
    let Some(mut out) = out else { return sh.finish_part(gen, true, Some("no output from ffmpeg".into())) };
    let n = size.0 as usize * size.1 as usize * 4;
    let mut idx: u64 = 0;
    loop {
        let mut buf = vec![0u8; n];
        if out.read_exact(&mut buf).is_err() {
            break;
        }
        let pts = start + idx as f64 / fps;
        // wait for the clock; the first picture after a start or a seek is shown at once, also while paused
        loop {
            if !sh.is_current(gen) {
                return;
            }
            if idx == 0 {
                break;
            }
            let (paused, pos) = {
                let s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
                (s.paused, sh.position_locked(&s))
            };
            if paused {
                std::thread::sleep(Duration::from_millis(15));
                continue;
            }
            let ahead = pts - pos;
            if ahead > 0.004 {
                std::thread::sleep(Duration::from_secs_f64(ahead.min(0.015)));
            } else {
                break;
            }
        }
        let late = {
            let s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
            sh.position_locked(&s) - pts
        };
        idx += 1;
        if late > 0.5 && idx > 1 {
            continue; // far behind: drop it to catch up
        }
        let seq = sh.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.gen != gen {
            return;
        }
        s.frame = Some(Arc::new(Frame { w: size.0, h: size.1, rgba: buf, seq, time: pts }));
    }
    if !sh.is_current(gen) {
        return;
    }
    // no picture at all and a complaint: say it
    let why = if idx == 0 { Some(tail(&err).unwrap_or_else(|| "ffmpeg could not decode the picture of this file".into())) } else { None };
    sh.finish_part(gen, true, why);
}

fn audio_thread(sh: Arc<Shared>, gen: u64, out: Option<std::process::ChildStdout>, err: Option<Arc<Mutex<String>>>) {
    let Some(mut out) = out else { return sh.finish_part(gen, false, Some("no output from ffmpeg".into())) };
    let cmd = sh.cfg.audio_cmd.clone();
    let mut sink = match spawn(&cmd[0], &cmd[1..], true) {
        Ok(c) => c,
        Err(_) => return no_sound(&sh, gen, &mut out),
    };
    let mut stdin = sink.stdin.take();
    drain_quiet(sink.stderr.take());
    sh.kids.lock().unwrap_or_else(|e| e.into_inner()).push(sink);
    let mut buf = vec![0u8; CHUNK];
    let mut got_any = false;
    loop {
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        got_any = true;
        loop {
            if !sh.is_current(gen) {
                return;
            }
            if sh.st.lock().unwrap_or_else(|e| e.into_inner()).paused {
                std::thread::sleep(Duration::from_millis(15));
            } else {
                break;
            }
        }
        // the master volume, when the speaker command is not the softvol chain (gain() is 100 then, and when muted 0)
        let gain = crate::audioout::global().gain();
        if gain < 100 {
            for c in buf[..n].chunks_exact_mut(2) {
                let v = (i16::from_le_bytes([c[0], c[1]]) as i32 * gain as i32 / 100) as i16;
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        let ok = stdin.as_mut().is_some_and(|w| w.write_all(&buf[..n]).is_ok());
        let mut s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.gen != gen {
            return;
        }
        if ok {
            s.audio_bytes += n as u64;
        } else if s.audio_active {
            // the speaker command went away: the clock carries on by itself from here
            let p = sh.position_locked(&s);
            s.audio_active = false;
            s.anchor_pos = p;
            s.anchor_at = Instant::now();
            stdin = None;
        }
    }
    if !sh.is_current(gen) {
        return;
    }
    drop(stdin);
    {
        // the rest of the sound is in the pipe and the card: the wall clock takes over from here
        let mut s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.gen == gen && s.audio_active {
            let p = sh.position_locked(&s);
            s.audio_active = false;
            s.anchor_pos = p;
            s.anchor_at = Instant::now();
        }
    }
    let why = if !got_any { tail(&err) } else { None };
    // let the speaker finish what it holds before the playback counts as over
    if why.is_none() {
        let wait = Duration::from_secs_f64(sh.cfg.audio_lag.max(0.0));
        let until = Instant::now() + wait;
        while Instant::now() < until && sh.is_current(gen) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    sh.finish_part(gen, false, why);
}

/// The speaker command could not start: play without sound, on the wall clock.
fn no_sound(sh: &Arc<Shared>, gen: u64, out: &mut std::process::ChildStdout) {
    {
        let mut s = sh.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.gen != gen {
            return;
        }
        let p = sh.position_locked(&s);
        s.audio_active = false;
        s.anchor_pos = p;
        s.anchor_at = Instant::now();
    }
    let mut sinkhole = [0u8; 16384];
    while let Ok(n) = out.read(&mut sinkhole) {
        if n == 0 {
            break;
        }
    }
    sh.finish_part(gen, false, None);
}

fn drain_quiet(e: Option<std::process::ChildStderr>) {
    if let Some(e) = e {
        let _ = drain(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-player-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn script(path: &PathBuf, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    const BANNER: &str = "Input #0, avi, from 'cam.avi':\n  Duration: 00:00:02.50, start: 0.000000, bitrate: 5000 kb/s\n  Stream #0:0: Video: mjpeg (Baseline) (MJPG / 0x47504A4D), yuvj422p(pc, bt470bg/unknown/unknown), 64x36, 10 fps, 10 tbr, 10 tbn\n  Stream #0:1: Audio: pcm_s16le ([1][0][0][0] / 0x0001), 44100 Hz, stereo, s16, 1411 kb/s\nAt least one output file must be specified\n";

    /// A fake ffmpeg: the probe prints the banner; the video side prints 3 frames at the asked size; the sound side prints 1 s of PCM.
    fn fake_ffmpeg(d: &PathBuf) -> String {
        let p = d.join("ffmpeg");
        script(
            &p,
            &format!(
                r#"echo "$@" >> "$0.log"
case "$*" in
  *rawvideo*)
    for a in "$@"; do case "$a" in *scale=*) wh=${{a#*scale=}};; esac; done
    W=${{wh%%:*}}; H=${{wh#*:}}; S=$((W*H*4))
    head -c $S /dev/zero; head -c $S /dev/zero | tr '\0' '\377'; head -c $S /dev/zero | tr '\0' '\200' ;;
  *s16le*) head -c 192000 /dev/zero ;;
  *) cat >&2 <<'EOF'
{BANNER}EOF
     exit 1 ;;
esac
"#
            ),
        );
        p.display().to_string()
    }

    fn fake_speaker(d: &PathBuf) -> Vec<String> {
        let p = d.join("aplay");
        script(&p, "echo \"$@\" >> \"$0.log\"\ncat >> \"$0.out\"\n");
        vec![p.display().to_string(), "-D".into(), "iyakki".into()]
    }

    fn cfg(d: &PathBuf) -> Config {
        Config { ffmpeg: fake_ffmpeg(d), audio_cmd: fake_speaker(d), max_fps: 30.0, audio_lag: 0.9 }
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        for _ in 0..300 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn a_linker_message_becomes_a_plain_reason_and_a_program_that_cannot_start_is_not_available() {
        let r = plain_reason("Error relocating /usr/bin/ffmpeg: av_default_item_name: symbol not found");
        assert!(r.contains("incomplete") && !r.contains("relocating"), "{r}");
        assert_eq!(plain_reason("Invalid data found when processing input"), "Invalid data found when processing input");
        assert!(!runs("/nonexistent/ffmpeg"));
        assert!(runs("/bin/true") || runs("/usr/bin/true"));
        assert!(!runs("/bin/false") && !runs("/usr/bin/false"));
    }

    #[test]
    fn the_banner_gives_the_codecs_the_size_and_the_length() {
        let i = parse_probe(BANNER);
        assert_eq!(i.duration, Some(2.5));
        assert_eq!(i.video, Some(VideoInfo { codec: "mjpeg".into(), w: 64, h: 36, fps: 10.0 }));
        assert_eq!(i.audio, Some(AudioInfo { codec: "pcm_s16le".into(), hz: 44100, layout: "stereo".into() }));
        assert_eq!(i.line(), "mjpeg 64x36 10 fps \u{b7} pcm_s16le 44100 Hz stereo \u{b7} 0:02");
        let real = "  Duration: 01:02:03.40, start: 0.0, bitrate: 1 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 4000 kb/s, 29.97 fps, 29.97 tbr, 30k tbn (default)\n  Stream #0:1[0x2](eng): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, 5.1(side), fltp, 384 kb/s (default)\n";
        let i = parse_probe(real);
        assert_eq!(i.duration, Some(3723.4));
        assert_eq!(i.video.as_ref().map(|v| (v.codec.as_str(), v.w, v.h)), Some(("h264", 1920, 1080)));
        assert!((i.video.unwrap().fps - 29.97).abs() < 1e-9);
        assert_eq!(i.audio.unwrap().layout, "5.1");
        assert_eq!(clock_text(3723.4), "1:02:03");
        // cover art is not a video
        let art = "  Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 128 kb/s\n  Stream #0:1: Video: mjpeg (Baseline), yuvj444p, 500x500, 90k tbr (attached pic)\n";
        assert!(parse_probe(art).video.is_none());
    }

    #[test]
    fn a_picture_is_fitted_into_the_area_with_even_sides() {
        assert_eq!(fit((1920, 1080), (960, 540)), (960, 540));
        assert_eq!(fit((1920, 1080), (1000, 1000)), (1000, 562));
        assert_eq!(fit((640, 480), (1000, 480)), (640, 480));
        assert_eq!(fit((100, 100), (1, 1)), (2, 2));
    }

    #[test]
    fn the_decoders_get_fixed_arguments_no_shell_and_a_dash_name_is_not_an_option() {
        let v = video_args("/media/a b.mp4", 12.5, 640, 360, 25.0);
        assert_eq!(v.join(" "), "-hide_banner -nostdin -v error -ss 12.500 -i /media/a b.mp4 -an -sn -vf fps=25,scale=640:360 -f rawvideo -pix_fmt rgba -");
        let a = audio_args("-weird.mkv", 0.0);
        assert_eq!(a.join(" "), "-hide_banner -nostdin -v error -ss 0.000 -i ./-weird.mkv -vn -sn -f s16le -ar 48000 -ac 2 -");
        assert_eq!(probe_args("x.mp4"), ["-hide_banner", "-nostdin", "-i", "x.mp4"]);
    }

    #[test]
    fn a_file_plays_to_the_end_with_pictures_at_the_asked_size_and_the_sound_in_the_speaker() {
        let d = dir("play");
        let c = cfg(&d);
        let p = Player::start(c.clone(), "/media/cam.avi", 0.0, (64, 64)).unwrap();
        assert_eq!(p.info().duration, Some(2.5));
        wait_for("the first picture", || p.frame(0).is_some());
        let f = p.frame(0).unwrap();
        assert_eq!((f.w, f.h, f.rgba.len()), (64, 36, 64 * 36 * 4));
        wait_for("the end", || p.status().ended);
        let st = p.status();
        assert!(st.error.is_none(), "{st:?}");
        // all three pictures came, the last one is the 0x80 one, and the sound went to the speaker
        let last = p.frame(0).unwrap();
        assert_eq!(last.rgba[0], 0x80);
        assert_eq!(last.seq, 3);
        wait_for("the sound", || std::fs::metadata(d.join("aplay.out")).map(|m| m.len()).unwrap_or(0) == 192000);
        let log = std::fs::read_to_string(d.join("ffmpeg.log")).unwrap();
        assert!(log.contains("-f rawvideo -pix_fmt rgba -") && log.contains("-f s16le -ar 48000 -ac 2 -"), "{log}");
        assert!(log.contains("fps=10,scale=64:36"), "{log}");
        assert!(std::fs::read_to_string(d.join("aplay.log")).unwrap().contains("-D iyakki"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pause_holds_the_clock_resume_goes_on_and_a_seek_starts_the_decoders_at_the_new_time() {
        let d = dir("ctl");
        let p = Player::start(cfg(&d), "/media/cam.avi", 0.0, (64, 64)).unwrap();
        wait_for("a picture", || p.frame(0).is_some());
        p.pause();
        let a = p.status();
        assert!(a.paused);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(p.status().position, a.position, "the clock stands while paused");
        p.resume();
        assert!(!p.status().paused);
        p.seek_to(1.5);
        wait_for("the restart", || std::fs::read_to_string(d.join("ffmpeg.log")).unwrap_or_default().contains("-ss 1.500"));
        assert!(p.status().position >= 1.5);
        // a seek past the end lands before it
        p.seek_to(99.0);
        wait_for("the clamped restart", || std::fs::read_to_string(d.join("ffmpeg.log")).unwrap_or_default().contains("-ss 2.200"));
        // a smaller area decodes again at the new size, from the same place
        p.set_area((32, 32));
        wait_for("the new size", || std::fs::read_to_string(d.join("ffmpeg.log")).unwrap_or_default().contains("scale=32:18"));
        p.stop();
        assert!(p.status().ended);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_speaker_plays_the_picture_on_the_wall_clock_and_a_broken_file_says_why() {
        let d = dir("nosound");
        let mut c = cfg(&d);
        c.audio_cmd = vec![d.join("no-such-aplay").display().to_string()];
        let p = Player::start(c.clone(), "/media/cam.avi", 0.0, (64, 64)).unwrap();
        wait_for("the end without sound", || p.status().ended);
        assert!(p.status().error.is_none());
        assert_eq!(p.frame(0).unwrap().seq, 3);
        // no speaker command at all: the same
        c.audio_cmd.clear();
        let p = Player::start(c.clone(), "/media/cam.avi", 0.0, (64, 64)).unwrap();
        wait_for("the end", || p.status().ended);
        // ffmpeg that cannot read the file
        script(&d.join("bad"), "echo 'Invalid data found when processing input' >&2\nexit 1\n");
        c.ffmpeg = d.join("bad").display().to_string();
        let e = Player::start(c, "/media/x.bin", 0.0, (64, 64)).err().unwrap();
        assert!(e.contains("Invalid data"), "{e}");
        // no ffmpeg at all
        assert!(Player::start(Config { ffmpeg: "/nonexistent/ffmpeg".into(), audio_cmd: vec![], max_fps: 30.0, audio_lag: 0.4 }, "x", 0.0, (8, 8)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// With a real ffmpeg on this machine (`cargo test -p iyakki-agent player::tests::real -- --ignored`): makes a 3 s Motion-JPEG
    /// AVI like the camera's, plays it with a fake speaker, and checks the pictures, the length and the time it took.
    #[test]
    #[ignore]
    fn real_ffmpeg_plays_a_camera_style_avi_in_real_time() {
        let Some(ff) = find_program("ffmpeg") else { return };
        let d = dir("real");
        let avi = d.join("cam.avi");
        let ok = Command::new(&ff).args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=3", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:v", "mjpeg", "-c:a", "pcm_s16le", "-y"]).arg(&avi).status().unwrap().success();
        assert!(ok);
        // a speaker that takes the sound at real-time speed, as the sound card does
        let slow = d.join("slow-aplay");
        script(&slow, "while true; do n=$(dd bs=19200 count=1 2>/dev/null | wc -c); [ \"$n\" -gt 0 ] || break; sleep 0.1; done\n");
        let c = Config { ffmpeg: ff, audio_cmd: vec![slow.display().to_string()], max_fps: 30.0, audio_lag: 0.45 };
        let t0 = Instant::now();
        let p = Player::start(c, &avi.display().to_string(), 0.0, (640, 360)).unwrap();
        assert_eq!(p.info().video.as_ref().map(|v| (v.w, v.h)), Some((320, 240)));
        println!("{}", p.info().line());
        wait_for("the end", || p.status().ended);
        let took = t0.elapsed().as_secs_f64();
        let f = p.frame(0).unwrap();
        println!("took {took:.2}s, last picture {}x{} seq {}", f.w, f.h, f.seq);
        assert_eq!((f.w, f.h), (480, 360));
        assert!(p.status().error.is_none());
        assert!(took > 2.0 && took < 6.0, "{took}");
        assert!(f.seq >= 30, "{}", f.seq);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_player_is_found_in_a_registered_folder() {
        let d = dir("find");
        script(&d.join("ffmpeg"), "exit 0\n");
        assert_eq!(find_program(&d.join("ffmpeg").display().to_string()).as_deref(), Some(d.join("ffmpeg").display().to_string().as_str()));
        register_dir(d.clone());
        assert_eq!(find_program("ffmpeg").filter(|p| p.starts_with(&d.display().to_string())), Some(d.join("ffmpeg").display().to_string()));
        assert!(find_program("no-such-program-xyz").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
