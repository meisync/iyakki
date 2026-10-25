//! LAB: the logic behind the "Model lab" tile (`docs/LAB.md`): try picture-making models and picture-understanding (vision)
//! models, one by one or side by side, and sample a video into frames for a vision model.
//!
//! This file has no screen in it. It holds
//!
//! * the **catalogue** ([`catalogue`]): which of the providers' models draw pictures, which can see pictures (and how sure that
//!   is) and whether any provider lists a video model, read from the settings and the last typed model list (no network);
//! * the **backends** ([`PictureBackend`], [`VisionBackend`], [`FrameSource`]) the tile calls on a background thread, with the real
//!   ones on [`Services`] and `ffmpeg`, so tests can use fakes;
//! * the **maths and the caps**: frame times ([`sample_times`]), sizes, the history cap ([`over_cap`]), what a picture or a video
//!   frame may weigh;
//! * the **log** ([`Log`]): one line per call (provider, model, a request summary without secrets, status, time, bytes, errors,
//!   fallbacks) with Copy and Clear in the tile.
//!
//! Nothing here writes a file. Pictures, frames and answers live in memory until the person presses Save.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::chat::Message;
use crate::lemonade::{ImageDefaults, ModelType};
use crate::piclog;
use crate::providers::{ImageParams, ImageTrace, Kind};
use crate::services::Services;
use crate::speechtime::{locality, Locality};

// ---------------------------------------------------------------------------------------------------------- the caps

/// A finished picture over this many bytes is refused (a server that answers with something huge).
pub const MAX_RESULT_BYTES: usize = 16 << 20;
/// What the session keeps in memory: this many results (pictures and answers) ...
pub const HISTORY_MAX_ITEMS: usize = 16;
/// ... and at most this many bytes of picture data in all. The oldest go first.
pub const HISTORY_MAX_BYTES: usize = 48 << 20;
/// An input file over this size is not read at all.
pub const MAX_INPUT_FILE_BYTES: u64 = 25 << 20;
/// A picture sent to a vision model is shrunk so that neither side is longer than this ...
pub const VISION_MAX_SIDE: u32 = 1024;
/// ... and so that its PNG is no bigger than this (it is shrunk more until it fits).
pub const VISION_MAX_BYTES: usize = 3 << 20;
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
/// How long one ffmpeg call may take before it is stopped.
pub const FFMPEG_SECS: u64 = 40;

/// What to say when a video cannot be sampled for lack of ffmpeg. ffmpeg is a vendor tool the person installs; IYAKKI never ships it.
pub const NO_FFMPEG: &str = "Video needs ffmpeg to take pictures out of a file, and ffmpeg is not installed. Store \u{2192} Setup \u{2192} Media tools \u{2192} Media player (ffmpeg) downloads it from its builder when you ask; IYAKKI does not ship it.";
/// The one line the Video tab shows when no provider has a video model.
pub const NO_VIDEO_MODEL: &str = "No video generation model is available from your providers. Models are installed in Settings \u{2192} Models (Lemonade or Ollama); none of them lists a video model today.";

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

// ----------------------------------------------------------------------------------------------------- the catalogue

/// How sure it is that a chat model can take pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sees {
    /// The server labelled it vision, or its name says so: a vision model.
    Yes,
    /// Some other chat model: it may or may not; the tile warns and the server's answer decides.
    Unknown,
}

/// One model of one provider as the tile lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct LabModel {
    pub provider: String,
    pub kind: Kind,
    pub model: String,
    pub remote: bool,
    /// `this computer`, `the network` or `the internet`.
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
#[derive(Clone, Debug, Default, PartialEq)]
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

fn place_of(base: &str) -> (bool, &'static str) {
    match locality(base) {
        Locality::ThisComputer => (false, "this computer"),
        Locality::Network => (true, "the network"),
        Locality::Internet => (true, "the internet"),
    }
}

/// Names of models that are never chat models (they are not offered under "any chat model").
fn not_a_chat_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["embed", "rerank", "whisper", "kokoro", "tts", "moderation", "transcribe", "speech"].iter().any(|k| n.contains(k))
}

/// A video model: its labels or its recipe say so. (Lemonade lists none today and has no video call; a provider that starts to would
/// show up here.)
pub fn is_video_model(labels: &[String], recipe: &str) -> bool {
    labels.iter().any(|l| l.to_ascii_lowercase().contains("video")) || recipe.to_ascii_lowercase().contains("video")
}

/// Read the catalogue from the settings (the saved lists and the last typed Lemonade list); it never waits on the network.
pub fn catalogue(svc: &Services) -> Catalogue {
    let providers: Vec<crate::providers::ProviderCfg> = svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().providers.iter().filter(|p| p.enabled).cloned().collect();
    let mut c = Catalogue::default();
    for p in providers {
        let (remote, place) = p.base().map(|b| place_of(&b)).unwrap_or((false, "this computer"));
        let mk = |model: &str, sees: Sees, why: &str| LabModel { provider: p.id.clone(), kind: p.kind, model: model.to_string(), remote, place, size_gb: None, defaults: None, loaded: false, sees, why: why.to_string() };
        if p.kind == Kind::Lemonade {
            for m in svc.lemonade_cached(&p.id) {
                if !m.downloaded {
                    if m.kind == ModelType::Image {
                        c.pictures_not_downloaded += 1;
                    }
                    continue;
                }
                let mut e = mk(&m.id, Sees::Yes, "");
                (e.size_gb, e.defaults, e.loaded) = (m.size_gb, m.image_defaults, m.loaded);
                if is_video_model(&m.labels, &m.recipe) {
                    e.why = "listed with a video label".into();
                    c.video.push(e);
                    continue;
                }
                match m.kind {
                    ModelType::Image => {
                        e.why = "the server's list says it draws pictures".into();
                        c.pictures.push(e);
                    }
                    ModelType::Vision => {
                        e.why = "the server's list labels it vision".into();
                        c.vision.push(e);
                    }
                    ModelType::Chat => {
                        e.sees = Sees::Unknown;
                        e.why = "a chat model".into();
                        c.chat_other.push(e);
                    }
                    _ => {}
                }
            }
            continue;
        }
        for m in &p.models {
            if crate::models::is_image_model(m) {
                c.pictures.push(mk(m, Sees::Yes, "the name says it draws pictures"));
            } else if not_a_chat_model(m) {
                continue;
            } else if crate::providers::model_sees_pictures(m) {
                c.vision.push(mk(m, Sees::Yes, "the name says it sees pictures"));
            } else {
                c.chat_other.push(mk(m, Sees::Unknown, "a chat model"));
            }
        }
    }
    let order = |v: &mut Vec<LabModel>| v.sort_by(|a, b| (a.remote, &a.provider, a.model.to_ascii_lowercase()).cmp(&(b.remote, &b.provider, b.model.to_ascii_lowercase())));
    order(&mut c.pictures);
    order(&mut c.vision);
    order(&mut c.chat_other);
    order(&mut c.video);
    c
}

/// The Video tab's line about video generation: the honest one when no provider lists a video model.
pub fn video_line(c: &Catalogue) -> String {
    if c.video.is_empty() {
        return NO_VIDEO_MODEL.to_string();
    }
    let names: Vec<String> = c.video.iter().map(|m| format!("{} on {}", m.model, m.provider)).collect();
    format!("A provider lists video models ({}), but IYAKKI has no call for them yet: there is no standard video request to send, so nothing is sent and nothing is faked.", names.join(", "))
}

/// Can this model take pictures at all? `Err` says why not (an image, speech, embedding or reranking model); `Ok` says how sure.
pub fn check_vision_model(c: &Catalogue, svc: &Services, provider: &str, model: &str) -> Result<Sees, String> {
    if let Some(m) = c.vision.iter().find(|m| m.provider == provider && m.model == model) {
        return Ok(m.sees);
    }
    if c.pictures.iter().any(|m| m.provider == provider && m.model == model) || crate::models::is_image_model(model) {
        return Err(format!("{model} draws pictures from text; it cannot look at one. Pick a vision model."));
    }
    if let Some(m) = svc.lemonade_model(provider, model) {
        if !m.kind.chats() {
            return Err(format!("{model} is a {} model, not a chat model; it cannot answer questions about a picture.", m.kind.label().to_ascii_lowercase()));
        }
    }
    if not_a_chat_model(model) {
        return Err(format!("{model} is not a chat model; it cannot answer questions about a picture."));
    }
    Ok(Sees::Unknown)
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
    pub fn line(&self, zone: &crate::clock::Zone) -> String {
        let mut s = format!("{}  {}  {} / {}  {}", crate::clock::format(zone, self.ts as i64, "%H:%M:%S"), self.tab, self.provider, self.model, self.request);
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
        let z = crate::clock::Zone::system();
        self.entries.iter().map(|e| e.line(&z)).collect::<Vec<_>>().join("\n")
    }
}

// --------------------------------------------------------------------------------------------------------- pictures

/// One request to a picture model.
#[derive(Clone, Debug, PartialEq)]
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

fn short(t: &str, n: usize) -> String {
    let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { format!("{}\u{2026}", one.chars().take(n).collect::<String>()) }
}

// ---------------------------------------------------------------------------------------------------------- vision

/// One question about one or more pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct VisionRequest {
    pub provider: String,
    pub model: String,
    pub question: String,
    /// The pictures as sent (already shrunk).
    pub images: Vec<Vec<u8>>,
}

/// What a vision model answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisionAnswer {
    pub text: String,
    /// (tokens in, tokens out) when the provider said; never guessed.
    pub tokens: Option<(u64, u64)>,
    /// The answer was read as it was written; false: it came in one piece.
    pub streamed: bool,
}

/// What one call sent and got back, for the log.
#[derive(Clone, Debug, Default, PartialEq)]
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

/// The real backends: pictures and questions go through the providers set up in Settings.
pub struct ServicesBackend(pub Arc<Services>);

impl PictureBackend for ServicesBackend {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let p = ImageParams { seed: Some(r.seed), negative: r.negative.clone().filter(|n| !n.is_empty()), steps: r.steps, size: Some((r.width, r.height)), ..Default::default() };
        let (res, trace) = self.0.generate_image_traced(Some((&r.provider, &r.model)), &r.prompt, &format!("{}x{}", r.width, r.height), &p);
        (res.map(|(_, _, b, ext)| (b, ext)), trace)
    }
}

impl VisionBackend for ServicesBackend {
    fn ask(&self, r: &VisionRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace) {
        let mut t = CallTrace { request_bytes: r.images.iter().map(|i| i.len() as u64).sum::<u64>() + r.question.len() as u64, ..Default::default() };
        let client = match self.0.client_for(&r.provider, false) {
            Ok(c) => c,
            Err(e) => {
                t.error = e.clone();
                return (Err(e), t);
            }
        };
        let secrets = client.headers().map(|h| piclog::secret_values(&h)).unwrap_or_default();
        let msgs = vec![Message::user_with_images(r.question.clone(), r.images.clone())];
        let streamer = self.0.chat_streamer();
        let started = Instant::now();
        let mut got = false;
        let out = match &streamer {
            Some(s) => client.chat_turn_stream(&r.model, &msgs, &[], s.as_ref(), &mut |piece| {
                if !piece.is_empty() {
                    got = true;
                    on_text(piece);
                }
                !cancel.load(Ordering::SeqCst)
            }),
            None => {
                t.notes.push("answer arrived in one piece (no streaming connection)".into());
                client.chat_turn(&r.model, &msgs, &[])
            }
        };
        t.ms = started.elapsed().as_millis() as u64;
        let tokens = crate::providers::reported_usage();
        match out {
            Ok(turn) => {
                if cancel.load(Ordering::SeqCst) {
                    t.error = "cancelled".into();
                    return (Err("cancelled".into()), t);
                }
                t.status = Some(200);
                t.response_bytes = turn.text.len() as u64;
                if turn.text.trim().is_empty() {
                    t.error = "the model returned no text".into();
                    return (Err("the model returned no text (a model that cannot take pictures sometimes does this)".into()), t);
                }
                (Ok(VisionAnswer { text: turn.text, tokens, streamed: streamer.is_some() && got }), t)
            }
            Err(e) => {
                let e = piclog::scrub(&e, &secrets);
                t.status = status_of(&e);
                t.error = e.clone();
                (Err(cannot_see_message(&r.model, &e).unwrap_or(e)), t)
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------- video

/// How frames are picked from a video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampling {
    /// `n` frames spread over the whole video (the middle of `n` equal parts, so no black first frame and none past the end).
    Even,
    /// A frame every `k` seconds from the start, at most `n` of them.
    EveryK(f64),
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
    crate::player::clock_text(secs)
}

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

/// `ffmpeg` from the catalogue or the system (the same program the MEDIA player uses); absent when it is not installed.
pub struct Ffmpeg;

impl FrameSource for Ffmpeg {
    fn available(&self) -> Result<(), String> {
        if crate::player::available() { Ok(()) } else { Err(NO_FFMPEG.to_string()) }
    }

    fn info(&self, file: &str) -> Result<VideoInfo, String> {
        let cfg = crate::player::Config::system().ok_or_else(|| NO_FFMPEG.to_string())?;
        let i = crate::player::probe(&cfg, file)?;
        let v = i.video.ok_or("that file has no picture stream (it may be only sound)")?;
        let duration = i.duration.ok_or("ffmpeg could not tell how long the video is")?;
        Ok(VideoInfo { duration, size: (v.w, v.h) })
    }

    fn frame_rgba(&self, file: &str, t: f64, size: (u32, u32), cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        use std::os::unix::process::CommandExt;
        let cfg = crate::player::Config::system().ok_or_else(|| NO_FFMPEG.to_string())?;
        let want = size.0 as usize * size.1 as usize * 4;
        let mut child = std::process::Command::new(&cfg.ffmpeg)
            .args(frame_args(file, t, size.0, size.1))
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start ffmpeg: {e}"))?;
        let pid = child.id() as i32;
        let done = Arc::new(AtomicBool::new(false));
        // a watchdog: stops ffmpeg when the time is up or the person cancelled
        let (d2, c2) = (done.clone(), cancel.clone());
        let started = Instant::now();
        let guard = std::thread::spawn(move || {
            while !d2.load(Ordering::SeqCst) {
                if started.elapsed() > Duration::from_secs(FFMPEG_SECS) || c2.load(Ordering::SeqCst) {
                    // SAFETY: kill(2) on the process group of the child this call started
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let mut out = child.stdout.take().ok_or("no output from ffmpeg")?;
        let mut buf = Vec::with_capacity(want);
        let mut chunk = [0u8; 65536];
        while buf.len() < want {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n.min(want - buf.len())]),
            }
        }
        done.store(true, Ordering::SeqCst);
        // SAFETY: as above (a process group that has already ended is not an error here)
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = child.wait();
        let _ = guard.join();
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if buf.len() != want {
            return Err(format!("ffmpeg gave no picture at {} (the video may end sooner, or the file is damaged)", clock_text(t)));
        }
        Ok(buf)
    }
}

/// The size frames are taken at: the video's own, shrunk to fit [`FRAME_MAX_SIDE`], both sides even.
pub fn frame_size(video: (u32, u32)) -> (u32, u32) {
    crate::player::fit(video, (FRAME_MAX_SIDE, FRAME_MAX_SIDE))
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
        out.push(VideoFrame { time: *t, w: size.0, h: size.1, png: crate::browserjobs::png::encode_rgba(size.0, size.1, &rgba) });
        progress(i + 1, times.len());
    }
    Ok(out)
}

/// The question for a video: the person's own words, with what the pictures are.
pub fn frames_question(question: &str, times: &[f64]) -> String {
    let at: Vec<String> = times.iter().map(|t| clock_text(*t)).collect();
    format!("These {} pictures are frames from one video, in order, taken at {}. {}", times.len(), at.join(", "), question.trim())
}

/// The log line for a frame extraction.
pub fn video_entry(file: &str, frames: usize, bytes: u64, ms: u64, error: &str, now: u64) -> LogEntry {
    let name = file.rsplit('/').next().unwrap_or(file);
    LogEntry { ts: now, tab: "video".into(), provider: "ffmpeg".into(), model: "(this computer)".into(), request: format!("{frames} frame(s) from {}", short(name, 40)), status: None, ms, bytes, error: error.to_string(), notes: vec![] }
}

#[cfg(test)]
#[path = "lab_tests.rs"]
mod lab_tests;
