//! Word times for reading aloud that come from the sound itself, not from a model of speech: what lights the word in TALES
//! and the chat while a Lemonade (or any OpenAI-compatible) voice reads.
//!
//! Best first, every source ending in the same [`Timed`] (where each word of the text starts, as `wordtime` uses it):
//!
//! 1. **Native**: the engine says when each word is said. Kokoro-FastAPI's `/dev/captioned_speech` does (an HTTP engine whose
//!    address ends in `captioned_speech`). Lemonade's `/audio/speech` does not: it forwards to `Kokoros`, whose OpenAI server
//!    answers with audio only (the word times of its timestamped model are a command-line feature), so there is nothing to ask.
//! 2. **Aligned**: the sound is given to a speech-to-text model (Lemonade's whisper.cpp, `response_format=verbose_json`, which
//!    answers with `segments[].words[]`, whisper tokens with `start`/`end`), and what it heard is matched to the text that was
//!    read with an edit-distance alignment. Only the times are used: the words shown are always the text's own. Words it
//!    misheard, skipped or added are placed between the matched ones by their length (`wordtime::word_units`).
//! 3. **Estimated**: `wordtime`'s fitted model, while an alignment is still running, when none is possible, or when too few
//!    words matched (another language, a model that answers without times).
//!
//! Sending text and sound to a server on another machine needs the person's yes once per server (`remote_ok` in Settings →
//! Voice); servers on this computer are used without asking. Made sound and its times are kept (by server, model, voice,
//! speed and text) so a replay or a seek costs nothing.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_proxy::Upstream;
use serde_json::{json, Value};

use crate::vault::{LocalVault, Secrets};
use crate::voice::{self, SpeakOpts, SpeechEngine};
use crate::wordtime;

// ------------------------------------------------------------------ where the times come from

/// Where the word times of a piece come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimingSource {
    /// The engine's own word times.
    Native,
    /// A speech-to-text model's word times, matched to the text.
    Aligned,
    /// The browser voice's boundary events.
    Voice,
    /// A model of speech fitted to the length of the sound (or to the speed).
    #[default]
    Estimated,
}

impl TimingSource {
    pub fn label(self) -> &'static str {
        match self {
            TimingSource::Native => "native",
            TimingSource::Aligned => "aligned",
            TimingSource::Voice => "from the voice",
            TimingSource::Estimated => "estimated",
        }
    }
}

/// Word times of one piece: where each word of its text starts (milliseconds), then the end of the sound.
#[derive(Clone, Debug, PartialEq)]
pub struct Timed {
    pub begin: Vec<f32>,
    pub source: TimingSource,
}

impl Timed {
    /// The timeline of `text` with these times (None when they do not fit its words).
    pub fn timeline(&self, text: &str) -> Option<wordtime::WordTimeline> {
        wordtime::WordTimeline::with_starts(text, self.begin.clone())
    }
}

// ------------------------------------------------------------------ what a recogniser heard

/// One word heard, with its time in milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct RecWord {
    pub text: String,
    pub start_ms: f32,
    pub end_ms: f32,
}

fn secs(v: Option<&Value>) -> Option<f32> {
    v.and_then(Value::as_f64).map(|s| (s * 1000.0) as f32).filter(|m| m.is_finite() && *m >= 0.0)
}

/// A whisper special token (`[_BEG_]`, `[_TT_150]`, `<|en|>`), not speech.
fn special(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || (t.starts_with("[_") && t.ends_with(']')) || (t.starts_with("<|") && t.ends_with("|>"))
}

/// The words of a transcription answer with their times, whatever its shape:
///
/// * OpenAI `verbose_json` with `timestamp_granularities=word`: `words: [{word, start, end}]` (seconds);
/// * whisper.cpp's server (what Lemonade forwards): `segments[].words[]` are tokens (`" hel"`, `"lo"`); a token that starts
///   with a space starts a word, the others continue it;
/// * Kokoro-FastAPI captioned speech: `timestamps: [{word, start_time, end_time}]`;
/// * segments with times but no words: each segment's words are spread over it by their length.
///
/// Empty when the answer has no times (Lemonade's FLM backend, or a compact `{"text"}`).
pub fn parse_words(v: &Value) -> Vec<RecWord> {
    let mut out = Vec::new();
    if let Some(ws) = v.get("words").and_then(Value::as_array) {
        for w in ws {
            if let (Some(t), Some(s), Some(e)) = (w.get("word").and_then(Value::as_str), secs(w.get("start")), secs(w.get("end"))) {
                if !special(t) {
                    out.push(RecWord { text: t.trim().to_string(), start_ms: s, end_ms: e.max(s) });
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(ts) = v.get("timestamps").and_then(Value::as_array) {
        for w in ts {
            if let (Some(t), Some(s), Some(e)) = (w.get("word").and_then(Value::as_str), secs(w.get("start_time")), secs(w.get("end_time"))) {
                if !special(t) {
                    out.push(RecWord { text: t.trim().to_string(), start_ms: s, end_ms: e.max(s) });
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    for seg in v.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let tokens = seg.get("words").and_then(Value::as_array).filter(|a| !a.is_empty());
        match tokens {
            Some(toks) => {
                let mut cur: Option<RecWord> = None;
                for tk in toks {
                    let Some(t) = tk.get("word").and_then(Value::as_str) else { continue };
                    if special(t) {
                        continue;
                    }
                    let (s, e) = (secs(tk.get("start")), secs(tk.get("end")));
                    let starts_word = t.starts_with(' ') || cur.is_none();
                    // a token of marks only (",", ".") belongs to the word before it
                    let marks_only = wordtime::words_of(t).is_empty();
                    if starts_word && !(marks_only && cur.is_some()) {
                        if let Some(w) = cur.take() {
                            out.push(w);
                        }
                        let (Some(s), Some(e)) = (s, e) else { continue };
                        cur = Some(RecWord { text: t.trim().to_string(), start_ms: s, end_ms: e.max(s) });
                    } else if let Some(w) = cur.as_mut() {
                        w.text.push_str(t.trim());
                        if let Some(e) = e {
                            w.end_ms = w.end_ms.max(e);
                        }
                    }
                }
                if let Some(w) = cur.take() {
                    out.push(w);
                }
            }
            None => {
                let (Some(s), Some(e), Some(text)) = (secs(seg.get("start")), secs(seg.get("end")), seg.get("text").and_then(Value::as_str)) else { continue };
                out.extend(spread(text, s, e.max(s)));
            }
        }
    }
    out.retain(|w| !wordtime::words_of(&w.text).is_empty());
    out
}

/// The words of `text` spread over `start..end` by their length.
fn spread(text: &str, start: f32, end: f32) -> Vec<RecWord> {
    let c: Vec<char> = text.chars().collect();
    let words: Vec<String> = wordtime::words_of(text).iter().map(|w| c[w.start..w.end].iter().collect()).collect();
    let weights: Vec<f32> = words.iter().map(|w| wordtime::word_units(w)).collect();
    let total: f32 = weights.iter().sum::<f32>().max(0.001);
    let mut t = start;
    words
        .into_iter()
        .zip(weights)
        .map(|(w, k)| {
            let d = (end - start) * k / total;
            let r = RecWord { text: w, start_ms: t, end_ms: t + d };
            t += d;
            r
        })
        .collect()
}

// ------------------------------------------------------------------ matching what was heard to what was read

/// Letters and digits only, lower case (Indic vowel signs and viramas kept): what two spellings of a word share.
fn norm(w: &str) -> Vec<char> {
    w.chars().filter(|c| c.is_alphanumeric() || ('\u{0900}'..='\u{0DFF}').contains(c)).flat_map(char::to_lowercase).collect()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// How alike two words are, 0 to 1.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    1.0 - levenshtein(&a, &b) as f32 / a.len().max(b.len()) as f32
}

/// A heard word is taken as the read word at this similarity or more ("colour" / "color", "Tamil" / "tamil", a dropped
/// letter in a long word); short words must be the same.
const ANCHOR_SIM: f32 = 0.6;
/// Cost of a word read but not heard, or heard but not read.
const GAP: f32 = 0.7;
/// At least this share of the read words must be matched for the times to be trusted.
const MIN_MATCHED: f32 = 0.34;
/// Larger alignments are not tried (a piece is a sentence or a paragraph; this is thousands of words).
const MAX_CELLS: usize = 4_000_000;

fn is_anchor(read: &str, heard: &str, sim: f32) -> bool {
    if norm(read).len() <= 2 { norm(read) == norm(heard) } else { sim >= ANCHOR_SIM }
}

/// Split heard words that hold several words of ours ("well-known" said as one, "don't" kept as one is fine) and drop marks.
fn heard_words(rec: &[RecWord]) -> Vec<RecWord> {
    let mut out = Vec::new();
    for r in rec {
        let parts = spread(&r.text, r.start_ms, r.end_ms);
        out.extend(parts);
    }
    out
}

/// Which heard word each read word is (None when it was not heard, or heard as something else): a minimum-cost
/// alignment (substitution costs `1 - similarity`, a missing or extra word [`GAP`]), keeping only pairs alike enough.
pub fn match_words(read: &[String], heard: &[String]) -> Option<Vec<Option<usize>>> {
    let (n, m) = (read.len(), heard.len());
    if n == 0 || m == 0 || (n + 1) * (m + 1) > MAX_CELLS {
        return None;
    }
    let w = m + 1;
    let mut cost = vec![0f32; (n + 1) * w];
    let mut sims = vec![0f32; n * m];
    for i in 0..n {
        for j in 0..m {
            sims[i * m + j] = similarity(&read[i], &heard[j]);
        }
    }
    for i in 0..=n {
        cost[i * w] = i as f32 * GAP;
    }
    for j in 0..=m {
        cost[j] = j as f32 * GAP;
    }
    for i in 1..=n {
        for j in 1..=m {
            let s = sims[(i - 1) * m + j - 1];
            let sub = cost[(i - 1) * w + j - 1] + (1.0 - s);
            cost[i * w + j] = sub.min(cost[(i - 1) * w + j] + GAP).min(cost[i * w + j - 1] + GAP);
        }
    }
    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let s = sims[(i - 1) * m + j - 1];
        let here = cost[i * w + j];
        if (here - (cost[(i - 1) * w + j - 1] + (1.0 - s))).abs() < 1e-4 {
            if is_anchor(&read[i - 1], &heard[j - 1], s) {
                out[i - 1] = Some(j - 1);
            }
            i -= 1;
            j -= 1;
        } else if (here - (cost[(i - 1) * w + j] + GAP)).abs() < 1e-4 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    Some(out)
}

/// Where each word of `text` starts (milliseconds, then `total_ms`), from what a recogniser heard in its sound. The words
/// it matched take its times; the others are placed between them by their length; before the first match and after the
/// last one the same. None when too few words matched to trust it.
pub fn align(text: &str, rec: &[RecWord], total_ms: f32) -> Option<Vec<f32>> {
    let c: Vec<char> = text.chars().collect();
    let read: Vec<String> = wordtime::words_of(text).iter().map(|w| c[w.start..w.end].iter().collect()).collect();
    let heard = heard_words(rec);
    let n = read.len();
    let total = if total_ms.is_finite() && total_ms > 0.0 { total_ms } else { heard.last().map_or(0.0, |h| h.end_ms) };
    if n == 0 || heard.is_empty() || total <= 0.0 {
        return None;
    }
    let heard_text: Vec<String> = heard.iter().map(|h| h.text.clone()).collect();
    let m = match_words(&read, &heard_text)?;
    // anchors in reading order whose times rise and fall inside the sound
    let mut anchors: Vec<(usize, f32, f32)> = Vec::new();
    for (i, j) in m.iter().enumerate() {
        let Some(j) = j else { continue };
        let h = &heard[*j];
        if h.start_ms > total + 250.0 || anchors.last().is_some_and(|a| h.start_ms < a.1) {
            continue;
        }
        anchors.push((i, h.start_ms.min(total), h.end_ms.min(total)));
    }
    let needed = ((n as f32 * MIN_MATCHED).ceil() as usize).max(1);
    if anchors.len() < needed {
        return None;
    }
    let weight = |i: usize| wordtime::word_units(&read[i]);
    let mut begin = vec![0f32; n];
    // spread words a..b over from..to by weight
    let fill = |a: usize, b: usize, from: f32, to: f32, begin: &mut Vec<f32>| {
        let total_w: f32 = (a..b).map(weight).sum::<f32>().max(0.001);
        let mut t = from;
        for k in a..b {
            begin[k] = t;
            t += (to - from).max(0.0) * weight(k) / total_w;
        }
    };
    let (i0, s0, _) = anchors[0];
    if i0 > 0 {
        fill(0, i0, 0.0, s0, &mut begin);
    }
    for k in 0..anchors.len() {
        let (i, s, e) = anchors[k];
        let (next_i, next_s) = anchors.get(k + 1).map_or((n, total), |a| (a.0, a.1));
        begin[i] = s;
        if next_i > i + 1 {
            // the matched word keeps its heard length when there is room; the unheard words after it share the rest
            let room = (next_s - s).max(0.0);
            let own = (e - s).clamp(0.0, room);
            let rest = room - own;
            let min_rest = 60.0 * (next_i - i - 1) as f32;
            if rest >= min_rest {
                fill(i + 1, next_i, s + own, next_s, &mut begin);
            } else {
                fill(i, next_i, s, next_s, &mut begin);
            }
        }
    }
    let mut out = begin;
    out.push(total.max(out.last().copied().unwrap_or(0.0)));
    for k in 1..out.len() {
        if out[k] < out[k - 1] {
            out[k] = out[k - 1];
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ where a server is

/// Where a speech server runs, as far as privacy goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    ThisComputer,
    Network,
    Internet,
}

impl Locality {
    pub fn label(self) -> &'static str {
        match self {
            Locality::ThisComputer => "on this computer",
            Locality::Network => "on the network",
            Locality::Internet => "on the internet",
        }
    }
}

/// Where the server at `url` is: this computer (loopback, `localhost`), the local network (private and link-local
/// addresses, `.local`, `.lan`, `.home.arpa`, names without a dot) or the internet (everything else).
pub fn locality(url: &str) -> Locality {
    let host = iyakki_sec::url_host(url).unwrap_or_default().to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if host == "localhost" || host.ends_with(".localhost") {
        return Locality::ThisComputer;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v) if v.is_loopback() || v.is_unspecified() => Locality::ThisComputer,
            IpAddr::V4(v) if v.is_private() || v.is_link_local() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])) => Locality::Network,
            IpAddr::V6(v) if v.is_loopback() || v.is_unspecified() => Locality::ThisComputer,
            IpAddr::V6(v) if (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80 => Locality::Network,
            _ => Locality::Internet,
        };
    }
    if host.is_empty() || !host.contains('.') || [".local", ".lan", ".home.arpa", ".internal", ".home"].iter().any(|s| host.ends_with(s)) {
        return Locality::Network;
    }
    Locality::Internet
}

/// `scheme://host:port` of an engine address: what a yes to sending is given for.
pub fn server_of(url: &str) -> String {
    crate::lemonade::origin(url).unwrap_or_else(|| url.to_string())
}

// ------------------------------------------------------------------ servers that did not answer

fn down() -> &'static Mutex<HashMap<String, Instant>> {
    static D: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

/// How long a server that did not answer is left alone before it is tried again.
pub const DOWN_FOR: Duration = Duration::from_secs(60);

pub fn mark_down(url: &str) {
    if let Ok(mut d) = down().lock() {
        d.insert(server_of(url), Instant::now());
    }
}

pub fn is_down(url: &str) -> bool {
    down().lock().ok().and_then(|d| d.get(&server_of(url)).copied()).is_some_and(|t| t.elapsed() < DOWN_FOR)
}

#[doc(hidden)]
pub fn clear_down_for_tests() {
    if let Ok(mut d) = down().lock() {
        d.clear();
    }
}

/// The server could not be reached, or failed inside (worth one more try, then another engine): not an answer that says
/// the request was wrong (a 4xx).
pub fn unreachable(err: &str) -> bool {
    !err.starts_with("HTTP 4") && !err.contains("nothing to say") && !err.contains("API key unavailable") && !err.contains("did not return WAV")
}

// ------------------------------------------------------------------ the cache

struct Entry {
    wav: Arc<Vec<u8>>,
    timed: Option<Timed>,
    at: Instant,
}

const CACHE_BYTES: usize = 96 << 20;

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static C: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn aligned_cache() -> &'static Mutex<HashMap<String, Option<Timed>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Timed>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The key of a made sound: server, model, voice, speed and text.
pub fn cache_key(engine: &SpeechEngine, opts: &SpeakOpts, text: &str) -> String {
    let (server, model, voice) = match engine {
        SpeechEngine::Http { url, model, voice, .. } => (url.clone(), model.clone(), opts.voice.clone().filter(|v| !v.trim().is_empty()).or(voice.clone()).unwrap_or_default()),
        other => (other.label(), String::new(), opts.voice.clone().unwrap_or_default()),
    };
    format!("{server}\u{1}{model}\u{1}{voice}\u{1}{:.2}\u{1}{}\u{1}{text}", opts.speed, opts.lang.as_deref().unwrap_or(""))
}

pub fn cached(key: &str) -> Option<(Arc<Vec<u8>>, Option<Timed>)> {
    let mut c = cache().lock().ok()?;
    let e = c.get_mut(key)?;
    e.at = Instant::now();
    Some((e.wav.clone(), e.timed.clone()))
}

pub fn remember(key: &str, wav: Arc<Vec<u8>>, timed: Option<Timed>) {
    let Ok(mut c) = cache().lock() else { return };
    c.insert(key.to_string(), Entry { wav, timed, at: Instant::now() });
    let mut size: usize = c.values().map(|e| e.wav.len()).sum();
    while size > CACHE_BYTES && c.len() > 1 {
        let Some(old) = c.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) else { break };
        if let Some(e) = c.remove(&old) {
            size -= e.wav.len();
        }
    }
}

/// Forget every made sound and alignment (tests, and a change of engine settings that the key cannot see).
pub fn clear_cache() {
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
    if let Ok(mut c) = aligned_cache().lock() {
        c.clear();
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for x in b {
        h = (h ^ *x as u64).wrapping_mul(0x100000001b3);
    }
    h
}

// ------------------------------------------------------------------ requests

/// Length of a WAV in milliseconds (0 when it is not one).
pub fn wav_ms(wav: &[u8]) -> f32 {
    voice::parse_wav(wav).map_or(0.0, |w| w.samples.len() as f32 * 1000.0 / w.rate as f32 / w.channels.max(1) as f32)
}

fn headers(key_ref: &Option<String>, secrets: &Secrets) -> Result<Vec<(String, String)>, String> {
    let mut h = Vec::new();
    if let Some(r) = key_ref {
        let k = secrets.resolve(r, None::<&LocalVault>).map_err(|e| format!("API key unavailable: {e}"))?;
        h.push(("Authorization".to_string(), format!("Bearer {}", &*k)));
    }
    Ok(h)
}

/// What a speech-to-text server heard in `wav`, with word times (`verbose_json`, word granularity). The sound is sent as
/// 16 kHz mono WAV (what whisper wants; times do not change).
pub fn transcribe_words(engine: &SpeechEngine, wav: &[u8], lang: Option<&str>, secrets: &Secrets, up: &dyn Upstream) -> Result<Vec<RecWord>, String> {
    let SpeechEngine::Http { url, model, key_ref, language, .. } = engine else { return Err("word times need an HTTP speech-to-text server".into()) };
    let w = voice::parse_wav(wav)?;
    let wav16 = voice::encode_wav(&voice::to_mono_16k(&w), voice::SAMPLE_RATE);
    let lang = lang.map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase()).filter(|l| l.len() == 2 || l.len() == 3).or(language.clone());
    let mut fields = vec![("model", model.as_str()), ("response_format", "verbose_json"), ("timestamp_granularities[]", "word")];
    if let Some(l) = lang.as_deref() {
        fields.push(("language", l));
    }
    let (body, boundary) = voice::multipart(&fields, "file", "speech.wav", "audio/wav", &wav16);
    let mut h = headers(key_ref, secrets)?;
    h.push(("Content-Type".into(), format!("multipart/form-data; boundary={boundary}")));
    let r = up.request("POST", url, &h, &body)?;
    if !(200..300).contains(&r.status) {
        return Err(format!("HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim()));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| "the speech-to-text server did not answer with JSON".to_string())?;
    Ok(parse_words(&v))
}

/// Kokoro-FastAPI's captioned speech: `POST …/captioned_speech` answers `{audio: base64, timestamps: [...]}`.
pub fn captioned(engine: &SpeechEngine, text: &str, opts: &SpeakOpts, secrets: &Secrets, up: &dyn Upstream) -> Result<(Vec<u8>, Vec<RecWord>), String> {
    let SpeechEngine::Http { url, model, key_ref, voice, .. } = engine else { return Err("not an HTTP engine".into()) };
    let said = voice::speakable(text);
    if said.is_empty() {
        return Err("nothing to say".into());
    }
    let mut h = headers(key_ref, secrets)?;
    h.push(("Content-Type".into(), "application/json".into()));
    let voice = opts.voice.as_deref().filter(|v| !v.trim().is_empty()).or(voice.as_deref()).unwrap_or("af_heart");
    let mut body = json!({"model": model, "input": said, "voice": voice, "response_format": "wav", "return_timestamps": true, "stream": false});
    if (opts.speed - 1.0).abs() > 0.01 && opts.speed.is_finite() {
        body["speed"] = json!(opts.speed.clamp(0.5, 2.0));
    }
    let r = up.request("POST", url, &h, body.to_string().as_bytes())?;
    if !(200..300).contains(&r.status) {
        return Err(format!("HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim()));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| "captioned speech did not answer with JSON".to_string())?;
    let wav = v.get("audio").and_then(Value::as_str).and_then(crate::models::b64_decode).ok_or("captioned speech has no audio")?;
    if wav.len() < 44 || &wav[..4] != b"RIFF" {
        return Err("the speech engine did not return WAV audio (set its output format to wav)".into());
    }
    Ok((wav, parse_words(&v)))
}

fn is_captioned(engine: &SpeechEngine) -> bool {
    matches!(engine, SpeechEngine::Http { url, .. } if url.trim_end_matches('/').ends_with("captioned_speech"))
}

// ------------------------------------------------------------------ the services

impl crate::services::Services {
    /// Whether text and sound may go to the server at `url`: always for this computer, otherwise only after the person said
    /// yes for that server (Settings → Models, when a remote Lemonade is chosen for reading or listening).
    pub fn speech_server_allowed(&self, url: &str) -> bool {
        locality(url) == Locality::ThisComputer || self.voice_settings().remote_ok.iter().any(|s| *s == server_of(url))
    }

    /// The person said yes to sending text and sound to the server at `url`.
    pub fn allow_speech_server(&self, url: &str) -> Result<(), String> {
        let s = server_of(url);
        self.settings
            .lock()
            .unwrap()
            .update(|st| {
                if !st.voice.remote_ok.contains(&s) {
                    st.voice.remote_ok.push(s.clone());
                }
            })
            .map_err(|e| e.to_string())
    }

    /// A Lemonade server's reading voice used when nothing is set up in Settings → Voice: the first Lemonade provider on this
    /// computer (or one the person allowed) whose typed list holds a text-to-speech model, for English (Kokoro's voices are
    /// English) or text of no stated language. Servers that did not answer lately are skipped.
    pub fn lemonade_auto_voice(&self, lang: Option<&str>) -> Option<SpeechEngine> {
        if lang.is_some_and(|l| !l.to_ascii_lowercase().starts_with("en")) {
            return None;
        }
        let providers = self.settings.lock().ok()?.get().providers.clone();
        for p in providers.iter().filter(|p| p.enabled && p.kind == crate::providers::Kind::Lemonade) {
            let Ok(base) = p.base() else { continue };
            if !self.speech_server_allowed(&base) || is_down(&base) {
                continue;
            }
            if let Some(m) = self.lemonade_cached(&p.id).into_iter().find(|m| m.kind == crate::lemonade::ModelType::TextToSpeech && m.downloaded) {
                return Some(crate::lemonade::tts_engine(&base, &m.id, None, p.key_ref.clone()));
            }
        }
        None
    }

    /// The speech-to-text server that times the words of made speech: the one of Settings → Voice when it is an HTTP server
    /// (allowed for this use), else a Lemonade whisper model on this computer or on a server the person allowed. None when
    /// word alignment is off.
    pub fn speech_aligner(&self) -> Option<SpeechEngine> {
        let v = self.voice_settings();
        if !v.align_words {
            return None;
        }
        if let Some(e @ SpeechEngine::Http { url, .. }) = &v.stt {
            if self.speech_server_allowed(url) && !is_down(url) {
                return Some(e.clone());
            }
        }
        let providers = self.settings.lock().ok()?.get().providers.clone();
        for p in providers.iter().filter(|p| p.enabled && p.kind == crate::providers::Kind::Lemonade) {
            let Ok(base) = p.base() else { continue };
            if !self.speech_server_allowed(&base) || is_down(&base) {
                continue;
            }
            let list = self.lemonade_cached(&p.id);
            let mut ms: Vec<&String> = list.iter().filter(|m| m.kind == crate::lemonade::ModelType::SpeechToText && m.downloaded).map(|m| &m.id).collect();
            // a small whisper is enough to find words in clean speech, and quick
            ms.sort_by_key(|m| {
                let l = m.to_ascii_lowercase();
                ["tiny", "base", "small", "medium", "large"].iter().position(|s| l.contains(s)).unwrap_or(9)
            });
            if let Some(m) = ms.first() {
                return Some(crate::lemonade::stt_engine(&base, m, p.key_ref.clone()));
            }
        }
        None
    }

    /// Text to speech with its word times when the engine gives them (native), kept in the cache. A server that cannot be
    /// reached is tried once more, then left alone for a minute while the engine of this computer reads instead.
    pub fn synthesize_timed(&self, text: &str, opts: &SpeakOpts) -> Result<(Vec<u8>, Option<Timed>), String> {
        let engine = self.effective_tts(opts.lang.as_deref()).ok_or("No text-to-speech engine is set up. Open Settings → Voice.")?;
        let key = cache_key(&engine, opts, text);
        if let Some((w, t)) = cached(&key) {
            return Ok(((*w).clone(), t));
        }
        let up = self.slow_upstream();
        let once = |e: &SpeechEngine| -> Result<(Vec<u8>, Option<Timed>), String> {
            if is_captioned(e) {
                let (wav, rec) = captioned(e, text, opts, &self.secrets, up.as_ref())?;
                let t = align(text, &rec, wav_ms(&wav)).map(|begin| Timed { begin, source: TimingSource::Native });
                return Ok((wav, t));
            }
            voice::synthesize_with(e, text, opts, &self.secrets, up.as_ref()).map(|w| (w, None))
        };
        let mut r = once(&engine);
        if let (Err(e), SpeechEngine::Http { url, .. }) = (&r, &engine) {
            if unreachable(e) {
                log::info!("speech: {} did not answer ({e}); trying once more", server_of(url));
                std::thread::sleep(Duration::from_millis(300));
                r = once(&engine);
                if let Err(e2) = &r {
                    if unreachable(e2) {
                        mark_down(url);
                        if let Some(local) = self.host_tts.auto_engine(opts.lang.as_deref()) {
                            log::info!("speech: {} is unreachable; reading with {} instead", server_of(url), local.label());
                            return voice::synthesize_with(&local, text, opts, &self.secrets, up.as_ref()).map(|w| (w, None));
                        }
                    }
                }
            }
        }
        let (wav, timed) = r?;
        if let Some(t) = &timed {
            log::debug!("speech: word timing {} for {} words", t.source.label(), t.begin.len().saturating_sub(1));
        }
        remember(&key, Arc::new(wav.clone()), timed.clone());
        Ok((wav, timed))
    }

    /// Word times for made speech from the speech-to-text server (see [`Services::speech_aligner`]), matched to `text`.
    /// None when there is no server, it fails, it answers without times or too few words matched. Kept in a cache.
    pub fn align_speech(&self, text: &str, wav: &[u8], lang: Option<&str>) -> Option<Timed> {
        let engine = self.speech_aligner()?;
        let SpeechEngine::Http { url, model, .. } = &engine else { return None };
        let key = format!("{url}\u{1}{model}\u{1}{:016x}\u{1}{text}", fnv(wav));
        if let Some(t) = aligned_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
            return t;
        }
        let up = self.slow_upstream();
        let t0 = Instant::now();
        let rec = match transcribe_words(&engine, wav, lang, &self.secrets, up.as_ref()) {
            Ok(r) => r,
            Err(e) => {
                if unreachable(&e) {
                    mark_down(url);
                }
                log::info!("speech: word alignment with {model} failed: {e}; the word times stay estimated");
                return None;
            }
        };
        let t = align(text, &rec, wav_ms(wav)).map(|begin| Timed { begin, source: TimingSource::Aligned });
        log::debug!("speech: word timing {} ({} words heard, {} ms, {model})", t.as_ref().map_or("estimated", |t| t.source.label()), rec.len(), t0.elapsed().as_millis());
        if let Ok(mut c) = aligned_cache().lock() {
            if c.len() > 2000 {
                c.clear();
            }
            c.insert(key, t.clone());
        }
        t
    }
}

#[cfg(test)]
#[path = "speechtime_tests.rs"]
mod tests;
