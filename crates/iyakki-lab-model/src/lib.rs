//! Types moved out of `iyakki_agent::lab` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// An input file over this size is not read at all.
pub const MAX_INPUT_FILE_BYTES: u64 = 25 << 20;

/// A picture sent to a vision model is shrunk so that neither side is longer than this ...
pub const VISION_MAX_SIDE: u32 = 1024;

/// ... and so that its PNG is no bigger than this (it is shrunk more until it fits).
pub const VISION_MAX_BYTES: usize = 3 << 20;

/// How sure it is that a chat model can take pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sees {
    /// The server labelled it vision, or its name says so: a vision model.
    Yes,
    /// Some other chat model: it may or may not; the tile warns and the server's answer decides.
    Unknown,
}

/// One request to a picture model.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PictureRequest {
    pub provider: String,
    pub model: String,
    pub prompt: String,
    /// Sent only when the server takes it.
    pub negative: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Asked for only when the server takes it; `None`: the model's own.
    pub steps: Option<u32>,
    pub seed: u64,
}

/// One question about one or more pictures.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VisionRequest {
    pub provider: String,
    pub model: String,
    pub question: String,
    /// The pictures as sent (already shrunk).
    pub images: Vec<Vec<u8>>,
}

/// What a vision model answered.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VisionAnswer {
    pub text: String,
    /// (tokens in, tokens out) when the provider said; never guessed.
    pub tokens: Option<(u64, u64)>,
    /// The answer was read as it was written; false: it came in one piece.
    pub streamed: bool,
}

/// How frames are picked from a video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampling {
    /// `n` frames spread over the whole video (the middle of `n` equal parts, so no black first frame and none past the end).
    Even,
    /// A frame every `k` seconds from the start, at most `n` of them.
    EveryK(f64),
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// A finished picture over this many bytes is refused (a server that answers with something huge).
pub const MAX_RESULT_BYTES: usize = 16 << 20;

/// What the session keeps in memory: this many results (pictures and answers) ...
pub const HISTORY_MAX_ITEMS: usize = 16;

/// ... and at most this many bytes of picture data in all. The oldest go first.
pub const HISTORY_MAX_BYTES: usize = 48 << 20;

/// A video frame is taken at most this wide or tall.
pub const FRAME_MAX_SIDE: u32 = 768;

/// How many frames a video question may hold.
pub const MIN_FRAMES: usize = 4;

pub const MAX_FRAMES: usize = 16;

/// The log keeps this many lines.
pub const LOG_MAX: usize = 300;

/// Picture sizes the fields accept.
pub const MIN_SIDE: u32 = 64;

pub const MAX_SIDE: u32 = 2048;

/// Steps the field accepts.
pub const MIN_STEPS: u32 = 1;

pub const MAX_STEPS: u32 = 100;

/// The longest prompt or question.
pub const MAX_TEXT: usize = 4000;

/// How many of the oldest results to drop so that `sizes` (oldest first, bytes each) fit [`HISTORY_MAX_ITEMS`] and
/// [`HISTORY_MAX_BYTES`]. The newest result is always kept.
pub fn over_cap(sizes: &[usize]) -> usize {
    let mut drop = sizes.len().saturating_sub(HISTORY_MAX_ITEMS);
    let mut total: usize = sizes[drop..].iter().sum();
    while total > HISTORY_MAX_BYTES && drop + 1 < sizes.len() {
        total -= sizes[drop];
        drop += 1;
    }
    drop
}

/// A picture side for a text field: a number made safe (64 to 2048, a multiple of 8), or `fallback` when it is not a number.
pub fn bound_side(text: &str, fallback: u32) -> u32 {
    let v = text.trim().parse::<u32>().unwrap_or(fallback).clamp(MIN_SIDE, MAX_SIDE);
    (v / 8 * 8).max(MIN_SIDE)
}

/// Steps from a text field: a number from 1 to 100, or `None` when it is empty or not a number (the model's own are used).
pub fn bound_steps(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().map(|v| v.clamp(MIN_STEPS, MAX_STEPS))
}

/// A seed from a text field; `None` when it is empty or not a number (the tile then makes one and shows it).
pub fn parse_seed(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok()
}

/// A fresh seed (not secret, not strong: only so that two runs differ and the number can be shown and reused).
pub fn fresh_seed(now_ms: u64) -> u64 {
    let mut x = now_ms ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % 4_000_000_000
}

/// The text with control characters removed and cut to [`MAX_TEXT`] characters.
pub fn clean_text(t: &str) -> String {
    t.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).take(MAX_TEXT).collect::<String>().trim().to_string()
}

/// `1.2 s`, `340 ms`.
pub fn ms_text(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{:.0} s", ms as f64 / 1000.0)
    } else if ms >= 1000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

/// `812 B`, `34 KB`, `1.2 MB`.
pub fn bytes_text(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1 << 20 {
        format!("{} KB", b.div_ceil(1024))
    } else {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    }
}

/// The times (seconds) of the frames to take.
pub fn sample_times(duration: f64, n: usize, mode: Sampling) -> Result<Vec<f64>, String> {
    if !duration.is_finite() || duration <= 0.0 {
        return Err("the video's length is not known, so frames cannot be placed".into());
    }
    let n = n.clamp(MIN_FRAMES, MAX_FRAMES);
    let round = |t: f64| (t * 1000.0).round() / 1000.0;
    Ok(match mode {
        Sampling::Even => (0..n).map(|i| round(duration * (i as f64 + 0.5) / n as f64)).collect(),
        Sampling::EveryK(k) => {
            let k = if k.is_finite() { k.max(0.5) } else { 0.5 };
            let v: Vec<f64> = (0..n).map(|i| round(i as f64 * k)).take_while(|t| *t < duration).collect();
            if v.is_empty() { vec![0.0] } else { v }
        }
    })
}

/// `0:03` or `1:02:03`.
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The question for a video: the person's own words, with what the pictures are.
pub fn frames_question(question: &str, times: &[f64]) -> String {
    let at: Vec<String> = times.iter().map(|t| clock_text(*t)).collect();
    format!("These {} pictures are frames from one video, in order, taken at {}. {}", times.len(), at.join(", "), question.trim())
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
pub use iyakki_provider_kind::ImageDefaults;
pub use iyakki_provider_kind::Kind;

/// What a [`LabModel`] looks like on the wire (`place` arrives as text and is interned).
#[derive(serde::Deserialize)]
struct LabModelWire {
    provider: String,
    kind: Kind,
    model: String,
    remote: bool,
    place: String,
    size_gb: Option<f64>,
    defaults: Option<ImageDefaults>,
    loaded: bool,
    sees: Sees,
    why: String,
}

impl From<LabModelWire> for LabModel {
    fn from(w: LabModelWire) -> LabModel {
        LabModel { provider: w.provider, kind: w.kind, model: w.model, remote: w.remote, place: iyakki_serde_static::intern(&w.place), size_gb: w.size_gb, defaults: w.defaults, loaded: w.loaded, sees: w.sees, why: w.why }
    }
}

/// One model of one provider as the tile lists it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "LabModelWire")]
pub struct LabModel {
    pub provider: String,
    pub kind: Kind,
    pub model: String,
    pub remote: bool,
    /// `this computer`, `the network` or `the internet`.
    #[serde(skip_deserializing)]
    pub place: &'static str,
    pub size_gb: Option<f64>,
    pub defaults: Option<ImageDefaults>,
    pub loaded: bool,
    pub sees: Sees,
    /// Why it is in the list (the server's label, or the name).
    pub why: String,
}

impl LabModel {
    /// What a list row says: `SDXL-Base-1.0 · local · 6.9 GB · 1024x1024, 20 steps`.
    pub fn label(&self) -> String {
        let mut parts = vec![self.model.clone(), format!("on {}", self.provider)];
        if self.remote {
            parts.push("remote".into());
        }
        if let Some(g) = self.size_gb.filter(|g| *g > 0.0) {
            parts.push(if g >= 1.0 { format!("{g:.1} GB") } else { format!("{} MB", (g * 1024.0).round() as u64) });
        }
        if let Some(d) = self.defaults {
            if let (Some(w), Some(h)) = (d.width, d.height) {
                parts.push(format!("{w}x{h}{}", d.steps.map_or(String::new(), |s| format!(", {s} steps"))));
            } else if let Some(s) = d.steps {
                parts.push(format!("{s} steps"));
            }
        }
        if self.sees == Sees::Unknown {
            parts.push("may not see pictures".into());
        }
        parts.join(" \u{b7} ")
    }

    /// The line next to the Run button: where the picture or the question goes.
    pub fn runs_on(&self) -> String {
        if self.remote {
            format!("Runs on {} (remote, {})", self.provider, self.place)
        } else {
            format!("Runs on {} (this computer)", self.provider)
        }
    }

    /// Does the picture call of this model's server take a negative prompt? (Only a Lemonade server is known to.)
    pub fn takes_negative(&self) -> bool {
        self.kind == Kind::Lemonade
    }

    /// Does it take the number of steps? (Only a Lemonade server is known to; the others choose their own.)
    pub fn takes_steps(&self) -> bool {
        self.kind == Kind::Lemonade
    }
}

/// The models of every enabled provider, sorted into what the tile needs.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Catalogue {
    pub pictures: Vec<LabModel>,
    /// Chat models that see pictures (labelled or by name).
    pub vision: Vec<LabModel>,
    /// The other chat models: offered under "any chat model", with a warning.
    pub chat_other: Vec<LabModel>,
    /// Models a provider lists as making video (a label or recipe that says video).
    pub video: Vec<LabModel>,
    /// Picture models a Lemonade server knows but has not downloaded.
    pub pictures_not_downloaded: usize,
}

impl Catalogue {
    /// The vision list, or with `any_chat` also the other chat models (after the vision ones).
    pub fn vision_choices(&self, any_chat: bool) -> Vec<LabModel> {
        let mut v = self.vision.clone();
        if any_chat {
            v.extend(self.chat_other.iter().cloned());
        }
        v
    }

    pub fn find_picture(&self, provider: &str, model: &str) -> Option<&LabModel> {
        self.pictures.iter().find(|m| m.provider == provider && m.model == model)
    }
}

/// The Video tab's line about video generation: the honest one when no provider lists a video model.
pub fn video_line(c: &Catalogue) -> String {
    if c.video.is_empty() {
        return NO_VIDEO_MODEL.to_string();
    }
    let names: Vec<String> = c.video.iter().map(|m| format!("{} on {}", m.model, m.provider)).collect();
    format!("A provider lists video models ({}), but IYAKKI has no call for them yet: there is no standard video request to send, so nothing is sent and nothing is faked.", names.join(", "))
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// The one line the Video tab shows when no provider has a video model.
pub const NO_VIDEO_MODEL: &str = "No video generation model is available from your providers. Models are installed in Settings \u{2192} Models (Lemonade or Ollama); none of them lists a video model today.";

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// LAB: the logic behind the "Model lab" tile (`docs/LAB.md`): try picture-making models and picture-understanding (vision)
// models, one by one or side by side, and sample a video into frames for a vision model.
//
// This file has no screen in it. It holds
//
// * the **catalogue** ([`catalogue`]): which of the providers' models draw pictures, which can see pictures (and how sure that
//   is) and whether any provider lists a video model, read from the settings and the last typed model list (no network);
// * the **backends** ([`PictureBackend`], [`VisionBackend`], [`FrameSource`]) the tile calls on a background thread, with the real
//   ones on [`Services`] and `ffmpeg`, so tests can use fakes;
// * the **maths and the caps**: frame times ([`sample_times`]), sizes, the history cap ([`over_cap`]), what a picture or a video
//   frame may weigh;
// * the **log** ([`Log`]): one line per call (provider, model, a request summary without secrets, status, time, bytes, errors,
//   fallbacks) with Copy and Clear in the tile.
//
// Nothing here writes a file. Pictures, frames and answers live in memory until the person presses Save.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use iyakki_provider_kind::ImageTrace;
use iyakki_speechtime::{locality, Locality};

// ---------------------------------------------------------------------------------------------------------- the caps

/// How long one ffmpeg call may take before it is stopped.
pub const FFMPEG_SECS: u64 = 40;

/// What to say when a video cannot be sampled for lack of ffmpeg. ffmpeg is a vendor tool the person installs; IYAKKI never ships it.
pub const NO_FFMPEG: &str = "Video needs ffmpeg to take pictures out of a file, and ffmpeg is not installed. Store \u{2192} Setup \u{2192} Media tools \u{2192} Media player (ffmpeg) downloads it from its builder when you ask; IYAKKI does not ship it.";









// ----------------------------------------------------------------------------------------------------- the catalogue






pub fn place_of(base: &str) -> (bool, &'static str) {
    match locality(base) {
        Locality::ThisComputer => (false, "this computer"),
        Locality::Network => (true, "the network"),
        Locality::Internet => (true, "the internet"),
    }
}

/// Names of models that are never chat models (they are not offered under "any chat model").
pub fn not_a_chat_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["embed", "rerank", "whisper", "kokoro", "tts", "moderation", "transcribe", "speech"].iter().any(|k| n.contains(k))
}

/// A video model: its labels or its recipe say so. (Lemonade lists none today and has no video call; a provider that starts to would
/// show up here.)
pub fn is_video_model(labels: &[String], recipe: &str) -> bool {
    labels.iter().any(|l| l.to_ascii_lowercase().contains("video")) || recipe.to_ascii_lowercase().contains("video")
}




/// The message for a server error that looks like "this model does not take images"; `None` when it looks like something else.
pub fn cannot_see_message(model: &str, error: &str) -> Option<String> {
    let l = error.to_ascii_lowercase();
    let hint = ["image", "vision", "multimodal", "multi-modal", "does not support", "not support", "unsupported content", "mmproj", "modality"].iter().any(|k| l.contains(k));
    hint.then(|| format!("{model} cannot take pictures (the server said: {}). Pick a model marked as a vision model.", error.trim().chars().take(200).collect::<String>()))
}

// ---------------------------------------------------------------------------------------------------------- the log

/// One call: what was asked of whom and what came of it. Nothing in it is secret: the request is summarised, headers are not kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogEntry {
    pub ts: u64,
    /// `picture`, `vision` or `video`.
    pub tab: String,
    pub provider: String,
    pub model: String,
    pub request: String,
    pub status: Option<u16>,
    pub ms: u64,
    pub bytes: u64,
    pub error: String,
    /// Fallbacks and adjustments (`downscaled`, `server used its own size`, `answer came in one piece`).
    pub notes: Vec<String>,
}

impl LogEntry {
    /// `12:03:44  picture  gpu / SDXL  512x512 seed 7  HTTP 200  3.1 s  34 KB  [notes]` as one line.
    pub fn line(&self, zone: &iyakki_clock::Zone) -> String {
        let mut s = format!("{}  {}  {} / {}  {}", iyakki_clock::format(zone, self.ts as i64, "%H:%M:%S"), self.tab, self.provider, self.model, self.request);
        s.push_str(&format!("  {}  {}  {}", self.status.map_or("no HTTP status".to_string(), |c| format!("HTTP {c}")), ms_text(self.ms), bytes_text(self.bytes)));
        if !self.error.is_empty() {
            s.push_str(&format!("  ERROR: {}", self.error));
        }
        if !self.notes.is_empty() {
            s.push_str(&format!("  [{}]", self.notes.join("; ")));
        }
        s
    }
}

/// The log of the session (bounded; the oldest lines go first).
#[derive(Clone, Debug, Default)]
pub struct Log {
    entries: VecDeque<LogEntry>,
}

impl Log {
    pub fn push(&mut self, e: LogEntry) {
        self.entries.push_back(e);
        while self.entries.len() > LOG_MAX {
            self.entries.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// The log as text, one line per call, oldest first (what Copy puts on the clipboard).
    pub fn text(&self) -> String {
        let z = iyakki_clock::Zone::system();
        self.entries.iter().map(|e| e.line(&z)).collect::<Vec<_>>().join("\n")
    }
}

// --------------------------------------------------------------------------------------------------------- pictures


/// What makes pictures (the real one is [`ServicesBackend`]; tests use a fake that returns a tiny PNG).
pub trait PictureBackend: Send + Sync {
    /// Draw one picture: its bytes and extension (or why not), and what was sent and came back. Blocks until the server answers.
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace);
}

/// The log line for a picture call.
pub fn picture_entry(r: &PictureRequest, trace: &ImageTrace, result: &Result<(Vec<u8>, &'static str), String>, now: u64) -> LogEntry {
    let mut notes = Vec::new();
    if trace.width != 0 && (trace.width, trace.height) != (r.width, r.height) {
        notes.push(format!("the server drew at the model's own size {}x{}, not the {}x{} asked for", trace.width, trace.height, r.width, r.height));
    }
    if let Some(s) = trace.steps {
        notes.push(if r.steps == Some(s) { format!("{s} steps (asked for)") } else { format!("{s} steps (the model's own)") });
    }
    if r.negative.as_deref().is_some_and(|n| !n.is_empty()) && !trace.url.is_empty() {
        notes.push("negative prompt sent".into());
    }
    let neg = r.negative.as_deref().is_some_and(|n| !n.is_empty());
    LogEntry {
        ts: now,
        tab: "picture".into(),
        provider: r.provider.clone(),
        model: r.model.clone(),
        request: format!("{}x{} seed {}{} prompt \"{}\"", r.width, r.height, r.seed, if neg { " +negative" } else { "" }, short(&r.prompt, 60)),
        status: trace.status,
        ms: trace.ms,
        bytes: if trace.image_bytes > 0 { trace.image_bytes } else { result.as_ref().map_or(0, |(b, _)| b.len() as u64) },
        error: result.as_ref().err().cloned().unwrap_or_default(),
        notes,
    }
}

pub fn short(t: &str, n: usize) -> String {
    let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}\u{2026}", one.chars().take(n).collect::<String>()) }
}

// ---------------------------------------------------------------------------------------------------------- vision



/// What one call sent and got back, for the log.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CallTrace {
    pub status: Option<u16>,
    pub ms: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub error: String,
    pub notes: Vec<String>,
}

pub trait VisionBackend: Send + Sync {
    /// Ask the model. `on_text` gets the text as it arrives (when the provider streams); `cancel` set stops the read.
    fn ask(&self, r: &VisionRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace);
}

/// The log line for a vision call.
pub fn vision_entry(r: &VisionRequest, trace: &CallTrace, result: &Result<VisionAnswer, String>, now: u64) -> LogEntry {
    let mut notes = trace.notes.clone();
    if let Ok(a) = result {
        match a.tokens {
            Some((i, o)) => notes.push(format!("{i} tokens in, {o} out")),
            None => notes.push("the provider did not report token counts".into()),
        }
    }
    LogEntry {
        ts: now,
        tab: "vision".into(),
        provider: r.provider.clone(),
        model: r.model.clone(),
        request: format!("{} picture{} ({}) question \"{}\"", r.images.len(), if r.images.len() == 1 { "" } else { "s" }, bytes_text(trace.request_bytes), short(&r.question, 60)),
        status: trace.status,
        ms: trace.ms,
        bytes: trace.response_bytes,
        error: trace.error.clone(),
        notes,
    }
}

/// The error text's HTTP status (`HTTP 400: ...`), when it has one.
pub fn status_of(error: &str) -> Option<u16> {
    error.strip_prefix("HTTP ")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}




// ------------------------------------------------------------------------------------------------------------- video




/// One frame taken from a video.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    pub time: f64,
    pub w: u32,
    pub h: u32,
    /// The frame as a PNG.
    pub png: Vec<u8>,
}

/// What a video file is, as far as the decoder says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoInfo {
    pub duration: f64,
    pub size: (u32, u32),
}

/// Where video frames come from (the real one runs `ffmpeg`; tests use a fake).
pub trait FrameSource: Send + Sync {
    /// `Ok` when frames can be taken, else the plain reason (ffmpeg missing).
    fn available(&self) -> Result<(), String>;
    fn info(&self, file: &str) -> Result<VideoInfo, String>;
    /// One frame at `t` seconds, scaled to `size`, as raw RGBA (`size.0 * size.1 * 4` bytes).
    fn frame_rgba(&self, file: &str, t: f64, size: (u32, u32), cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String>;
}

/// The arguments for one frame: seek, decode one picture, scale it, raw RGBA on standard output. Never a shell.
pub fn frame_args(file: &str, t: f64, w: u32, h: u32) -> Vec<String> {
    let name = if file.starts_with('-') { format!("./{file}") } else { file.to_string() };
    ["-hide_banner", "-nostdin", "-v", "error", "-ss"].iter().map(|s| s.to_string()).chain([format!("{t:.3}"), "-i".into(), name]).chain(["-frames:v", "1", "-an", "-sn", "-vf"].iter().map(|s| s.to_string())).chain([format!("scale={w}:{h}"), "-f".into(), "rawvideo".into(), "-pix_fmt".into(), "rgba".into(), "-".into()]).collect()
}




/// Take every frame in `times`. `progress(done, total)` is called after each one; a cancelled run returns `Err("cancelled")`.
pub fn extract(src: &dyn FrameSource, file: &str, info: &VideoInfo, times: &[f64], cancel: &Arc<AtomicBool>, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<VideoFrame>, String> {
    let size = frame_size(info.size);
    let mut out = Vec::with_capacity(times.len());
    for (i, t) in times.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let rgba = src.frame_rgba(file, (*t).min((info.duration - 0.05).max(0.0)), size, cancel)?;
        out.push(VideoFrame { time: *t, w: size.0, h: size.1, png: iyakki_frames::png::encode_rgba(size.0, size.1, &rgba) });
        progress(i + 1, times.len());
    }
    Ok(out)
}


/// The log line for a frame extraction.
pub fn video_entry(file: &str, frames: usize, bytes: u64, ms: u64, error: &str, now: u64) -> LogEntry {
    let name = file.rsplit('/').next().unwrap_or(file);
    LogEntry { ts: now, tab: "video".into(), provider: "ffmpeg".into(), model: "(this computer)".into(), request: format!("{frames} frame(s) from {}", short(name, 40)), status: None, ms, bytes, error: error.to_string(), notes: vec![] }
}


/// The size frames are taken at: the video's own, shrunk to fit [`FRAME_MAX_SIDE`], both sides even.
pub fn frame_size(video: (u32, u32)) -> (u32, u32) {
    iyakki_frames::fit(video, (FRAME_MAX_SIDE, FRAME_MAX_SIDE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: u64) -> LogEntry {
        LogEntry { ts, tab: "picture".into(), provider: "gpu".into(), model: "sdxl".into(), request: "512x512".into(), status: Some(200), ms: 3100, bytes: 34 * 1024, error: String::new(), notes: vec![] }
    }

    #[test]
    fn text_fields_are_bounded_and_parsed_safely() {
        assert_eq!(bound_side("500", 512), 496);
        assert_eq!(bound_side("5", 512), MIN_SIDE);
        assert_eq!(bound_side("99999", 512), MAX_SIDE);
        assert_eq!(bound_side("abc", 512), 512);
        assert_eq!(bound_steps(" 7 "), Some(7));
        assert_eq!(bound_steps("0"), Some(MIN_STEPS));
        assert_eq!(bound_steps("500"), Some(MAX_STEPS));
        assert_eq!(bound_steps(""), None);
        assert_eq!(parse_seed("42"), Some(42));
        assert_eq!(parse_seed("-1"), None);
        assert!(fresh_seed(1) < 4_000_000_000 && fresh_seed(1) != fresh_seed(2));
        assert_eq!(clean_text("  a\u{0}b\nc\u{7}  "), "ab\nc");
        assert_eq!(clean_text(&"x".repeat(MAX_TEXT + 10)).chars().count(), MAX_TEXT);
    }

    #[test]
    fn numbers_read_as_people_say_them() {
        assert_eq!(ms_text(340), "340 ms");
        assert_eq!(ms_text(1200), "1.2 s");
        assert_eq!(ms_text(12_000), "12 s");
        assert_eq!(bytes_text(812), "812 B");
        assert_eq!(bytes_text(34 * 1024), "34 KB");
        assert_eq!(bytes_text(3 << 19), "1.5 MB");
        assert_eq!(clock_text(3.0), "0:03");
        assert_eq!(clock_text(3723.0), "1:02:03");
        assert_eq!(short("a  b\n c", 10), "a b c");
        assert_eq!(short("abcdefghij", 4), "abcd\u{2026}");
        assert_eq!(status_of("HTTP 503: busy"), Some(503));
        assert_eq!(status_of("timeout"), None);
    }

    #[test]
    fn history_drops_the_oldest_first_and_keeps_the_newest() {
        assert_eq!(over_cap(&[1, 2, 3]), 0);
        let many = vec![1usize; HISTORY_MAX_ITEMS + 3];
        assert_eq!(over_cap(&many), 3);
        assert_eq!(over_cap(&[HISTORY_MAX_BYTES, HISTORY_MAX_BYTES]), 1);
        assert_eq!(over_cap(&[HISTORY_MAX_BYTES * 2]), 0);
    }

    #[test]
    fn frames_are_placed_inside_the_video() {
        let even = sample_times(100.0, 4, Sampling::Even).unwrap();
        assert_eq!(even, vec![12.5, 37.5, 62.5, 87.5]);
        let every = sample_times(10.0, 16, Sampling::EveryK(3.0)).unwrap();
        assert_eq!(every, vec![0.0, 3.0, 6.0, 9.0]);
        assert_eq!(sample_times(10.0, 4, Sampling::EveryK(f64::NAN)).unwrap().len(), 4);
        assert!(sample_times(0.0, 4, Sampling::Even).is_err());
        assert!(sample_times(f64::NAN, 4, Sampling::Even).is_err());
        assert_eq!(sample_times(100.0, 1000, Sampling::Even).unwrap().len(), MAX_FRAMES);
        assert_eq!(frame_size((1920, 1080)), (768, 432));
    }

    #[test]
    fn frame_arguments_never_make_a_shell_or_an_option_of_a_file_name() {
        let a = frame_args("-evil.mp4", 1.5, 320, 180);
        assert!(a.contains(&"./-evil.mp4".to_string()) && a.contains(&"1.500".to_string()) && a.contains(&"scale=320:180".to_string()));
        assert_eq!(a.last().map(String::as_str), Some("-"));
    }

    struct Frames {
        calls: std::sync::Mutex<Vec<f64>>,
    }
    impl FrameSource for Frames {
        fn available(&self) -> Result<(), String> {
            Ok(())
        }
        fn info(&self, _file: &str) -> Result<VideoInfo, String> {
            Ok(VideoInfo { duration: 10.0, size: (4, 2) })
        }
        fn frame_rgba(&self, _file: &str, t: f64, size: (u32, u32), _cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
            self.calls.lock().unwrap().push(t);
            Ok(vec![255; size.0 as usize * size.1 as usize * 4])
        }
    }

    #[test]
    fn extraction_reports_progress_clamps_the_last_time_and_honours_cancel() {
        let src = Frames { calls: Default::default() };
        let info = src.info("v.mp4").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut seen = Vec::new();
        let out = extract(&src, "v.mp4", &info, &[1.0, 10.0], &cancel, &mut |d, t| seen.push((d, t))).unwrap();
        assert_eq!((out.len(), seen.clone()), (2, vec![(1, 2), (2, 2)]));
        assert_eq!(&out[0].png[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(src.calls.lock().unwrap().clone(), vec![1.0, 9.95]);
        cancel.store(true, Ordering::SeqCst);
        assert_eq!(extract(&src, "v.mp4", &info, &[1.0], &cancel, &mut |_, _| {}).unwrap_err(), "cancelled");
    }

    #[test]
    fn the_log_is_bounded_oldest_first_and_prints_one_line_per_call() {
        let mut log = Log::default();
        assert!(log.is_empty());
        for i in 0..(LOG_MAX as u64 + 5) {
            log.push(entry(i));
        }
        assert_eq!(log.len(), LOG_MAX);
        assert_eq!(log.entries().next().unwrap().ts, 5);
        let z = iyakki_clock::Zone::utc();
        let mut e = entry(0);
        e.error = "boom".into();
        e.notes = vec!["downscaled".into()];
        let line = e.line(&z);
        assert!(line.contains("picture  gpu / sdxl  512x512  HTTP 200  3.1 s  34 KB") && line.ends_with("ERROR: boom  [downscaled]"), "{line}");
        let v = video_entry("/a/b/clip.mp4", 4, 2048, 900, "", 0);
        assert!(v.request.contains("4 frame(s) from clip.mp4") && v.provider == "ffmpeg");
        log.clear();
        assert_eq!(log.text(), "");
    }

    #[test]
    fn places_and_model_kinds_are_told_apart() {
        assert_eq!(place_of("http://localhost:11434"), (false, "this computer"));
        assert_eq!(place_of("http://192.168.1.7:8000"), (true, "the network"));
        assert_eq!(place_of("https://api.example.com/v1"), (true, "the internet"));
        assert!(is_video_model(&["Text-To-Video".to_string()], ""));
        assert!(is_video_model(&[], "video-gen"));
        assert!(!is_video_model(&["chat".to_string()], "llamacpp"));
        assert!(not_a_chat_model("nomic-embed-text") && not_a_chat_model("Whisper-Large") && !not_a_chat_model("llama3"));
    }
}
