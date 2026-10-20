//! Browser voice: reading aloud with the speech synthesis of the browser (or desktop window) the person is using.
//!
//! Nothing is spoken on this machine and no audio is made here. The text of the piece being read goes to the page over the
//! front end's own connection (`docs/REMOTE-PROTOCOL.md`), the page calls `speechSynthesis.speak`, and the page reports back
//! when it started, ended, failed or passed a word. This module is the meeting point of the two sides and has no I/O of
//! its own:
//!
//! * [`HostVoice`] is one person's bridge, kept per shell thread ([`HostVoice::current`]). The front end drains what is to be
//!   sent ([`HostVoice::take_outgoing`]) and feeds what came back ([`HostVoice::event`]); the STORY reader speaks through
//!   [`BrowserSynth`] and [`BrowserSink`], which plug into the same reader the sound engines use.
//! * [`ToClient`] and [`FromClient`] are the wire messages (JSON, one per WebSocket text frame) with their encoders and
//!   decoders. Everything the page sends is checked and size-limited: it is the person's own session, but the server never
//!   trusts a page's text blindly.
//!
//! Only the text of the story pieces that are being read is ever sent, never anything else of the person's.

use crate::story::{Playing, Sink, Synth};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The page side of the browser voice (speechSynthesis), shared by the web front end's page and the voice helper page.
pub const SPEECH_JS: &str = include_str!("speech.js");

/// Most characters of one piece sent to the page (pieces are at most `story::MAX_CHUNK_CHARS`; this is the hard limit).
pub const MAX_PIECE_CHARS: usize = 1500;
/// Most voices kept from the page's list.
pub const MAX_VOICES: usize = 400;
/// How long the page may take to start speaking before the piece is reported as failed.
pub const START_TIMEOUT: Duration = Duration::from_secs(8);

/// A voice the browser offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceInfo {
    pub name: String,
    /// BCP 47 language tag such as `en-US` or `ta-IN`.
    pub lang: String,
    /// The voice is on the person's device (works offline).
    pub local: bool,
}

/// What the server asks of the page.
#[derive(Clone, Debug, PartialEq)]
pub enum ToClient {
    /// Say `text` as utterance `id`; a new utterance replaces the one that is still being spoken.
    Speak { id: u64, text: String, voice: Option<String>, lang: Option<String>, rate: f32 },
    /// Stop utterance `id` (when it is the one being spoken), or whatever is being spoken when `id` is `None`.
    Cancel { id: Option<u64> },
    Pause,
    Resume,
    /// Send the list of voices again.
    ListVoices,
}

impl ToClient {
    /// The JSON text of the message.
    pub fn encode(&self) -> String {
        match self {
            ToClient::Speak { id, text, voice, lang, rate } => json!({"t": "speak", "id": id, "text": text, "voice": voice, "lang": lang, "rate": rate}).to_string(),
            ToClient::Cancel { id } => json!({"t": "speak_cancel", "id": id}).to_string(),
            ToClient::Pause => r#"{"t":"speak_pause"}"#.to_string(),
            ToClient::Resume => r#"{"t":"speak_resume"}"#.to_string(),
            ToClient::ListVoices => r#"{"t":"speak_voices"}"#.to_string(),
        }
    }

    /// The message from its JSON text (what the page's own code and the tests use to read it back).
    pub fn decode(text: &str) -> Option<ToClient> {
        let v: Value = serde_json::from_str(text).ok()?;
        let id = v.get("id").and_then(Value::as_u64);
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(match v.get("t")?.as_str()? {
            "speak" => ToClient::Speak { id: id?, text: s("text")?, voice: s("voice"), lang: s("lang"), rate: v.get("rate").and_then(Value::as_f64).unwrap_or(1.0) as f32 },
            "speak_cancel" => ToClient::Cancel { id },
            "speak_pause" => ToClient::Pause,
            "speak_resume" => ToClient::Resume,
            "speak_voices" => ToClient::ListVoices,
            _ => return None,
        })
    }
}

/// What the page tells the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromClient {
    /// The page's speech: `supported` false when it has no `speechSynthesis`; the voices it offers (also sent again when
    /// they change, browsers load them late).
    Voices { supported: bool, voices: Vec<VoiceInfo> },
    Started { id: u64 },
    Ended { id: u64 },
    Error { id: u64, message: String },
    /// A word was reached: the character position in the utterance's text.
    Boundary { id: u64, char_index: usize },
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

impl FromClient {
    /// One message from the page, or `None` when it is not something we understand (ignored, never trusted).
    pub fn decode(text: &str) -> Option<FromClient> {
        if text.len() > 256 * 1024 {
            return None;
        }
        let v: Value = serde_json::from_str(text).ok()?;
        let id = v.get("id").and_then(Value::as_u64);
        Some(match v.get("t")?.as_str()? {
            "sp_voices" => {
                let supported = v.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let voices = v
                    .get("voices")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| {
                                let name = clean(x.get("n")?.as_str()?, 120);
                                (!name.trim().is_empty()).then(|| VoiceInfo { name, lang: clean(x.get("l").and_then(Value::as_str).unwrap_or(""), 20), local: x.get("d").and_then(Value::as_bool).unwrap_or(false) })
                            })
                            .take(MAX_VOICES)
                            .collect()
                    })
                    .unwrap_or_default();
                FromClient::Voices { supported, voices }
            }
            "sp_start" => FromClient::Started { id: id? },
            "sp_end" => FromClient::Ended { id: id? },
            "sp_error" => FromClient::Error { id: id?, message: clean(v.get("e").and_then(Value::as_str).unwrap_or("unknown error"), 200) },
            "sp_boundary" => FromClient::Boundary { id: id?, char_index: v.get("c").and_then(Value::as_u64).unwrap_or(0).min(100_000) as usize },
            _ => return None,
        })
    }

    pub fn encode(&self) -> String {
        match self {
            FromClient::Voices { supported, voices } => json!({"t": "sp_voices", "ok": supported, "voices": voices.iter().map(|v| json!({"n": v.name, "l": v.lang, "d": v.local})).collect::<Vec<_>>()}).to_string(),
            FromClient::Started { id } => json!({"t": "sp_start", "id": id}).to_string(),
            FromClient::Ended { id } => json!({"t": "sp_end", "id": id}).to_string(),
            FromClient::Error { id, message } => json!({"t": "sp_error", "id": id, "e": message}).to_string(),
            FromClient::Boundary { id, char_index } => json!({"t": "sp_boundary", "id": id, "c": char_index}).to_string(),
        }
    }
}

/// Which kind of screen this person is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// The text or framebuffer console (and anything that never said otherwise): no browser, no browser voice.
    Console,
    /// A web browser showing the shell (`iyakki-remote`).
    Browser,
    /// The desktop window (`iyakki-gui`) with its web engine.
    Window,
}

/// Whether the screen can speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// Not known (a console, or a front end that has not answered yet).
    Unknown,
    /// The front end is asking its engine.
    Checking,
    Yes,
    /// The engine cannot speak, and why.
    No(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UttState {
    /// Sent to the page, not started yet.
    Sent,
    Started,
    Ended,
    Failed(String),
}

struct Utt {
    state: UttState,
    since: Instant,
    boundary: usize,
    /// A boundary event came for this utterance (some voices never fire one).
    boundary_seen: bool,
}

struct Inner {
    front: Front,
    support: Support,
    voices: Vec<VoiceInfo>,
    out: VecDeque<ToClient>,
    utts: HashMap<u64, Utt>,
    next_id: u64,
    probe_wanted: bool,
    start_timeout: Duration,
    /// The voice helper page (a tab of the person's browser, see `voicehelper`) is connected: what the front end was before.
    helper: Option<(Front, Support)>,
}

/// One person's link to the speech of their screen. Cheap to share (an `Arc`); the reader's threads and the front end's loop
/// all use the same one.
pub struct HostVoice {
    inner: Mutex<Inner>,
}

thread_local! {
    static CURRENT: Arc<HostVoice> = Arc::new(HostVoice::new());
}

impl Default for HostVoice {
    fn default() -> Self {
        HostVoice::new()
    }
}

impl HostVoice {
    pub fn new() -> HostVoice {
        HostVoice { inner: Mutex::new(Inner { front: Front::Console, support: Support::Unknown, voices: Vec::new(), out: VecDeque::new(), utts: HashMap::new(), next_id: 1, probe_wanted: false, start_timeout: START_TIMEOUT, helper: None }) }
    }

    /// The bridge of this thread's shell (each person's shell runs on a thread of its own, and the front end that serves it
    /// runs on the same thread).
    pub fn current() -> Arc<HostVoice> {
        CURRENT.with(Arc::clone)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A bridge whose pieces fail after `t` without a start (tests).
    pub fn with_start_timeout(t: Duration) -> HostVoice {
        let h = HostVoice::new();
        h.lock().start_timeout = t;
        h
    }

    // ---- the front end's side

    /// A front end that can show a page with speech says what it is. The page then reports whether it can speak.
    pub fn connect(&self, front: Front) {
        let mut i = self.lock();
        i.front = front;
        i.support = if front == Front::Console { Support::Unknown } else { Support::Checking };
        i.out.clear();
        if front == Front::Browser {
            i.out.push_back(ToClient::ListVoices);
        }
    }

    /// The front end is gone (the page was closed or the connection broke): what was being said fails, the rest is dropped.
    pub fn disconnect(&self) {
        let mut i = self.lock();
        i.out.clear();
        i.support = Support::Unknown;
        i.front = Front::Console;
        for u in i.utts.values_mut() {
            if matches!(u.state, UttState::Sent | UttState::Started) {
                u.state = UttState::Failed("the browser was closed or lost the connection".into());
            }
        }
    }

    /// The front end found out by itself that its engine cannot speak (the desktop window's probe).
    pub fn set_unsupported(&self, why: &str) {
        let mut i = self.lock();
        match &mut i.helper {
            Some(saved) => saved.1 = Support::No(why.to_string()), // the helper page speaks now; this is what to go back to
            None => i.support = Support::No(why.to_string()),
        }
    }

    pub fn set_supported(&self) {
        let mut i = self.lock();
        match &mut i.helper {
            Some(saved) => saved.1 = Support::Yes,
            None => i.support = Support::Yes,
        }
    }

    /// The voice helper page connected: it is the person's speaker like a browser front end is (it announces its voices),
    /// until [`HostVoice::detach_helper`].
    pub fn attach_helper(&self) {
        let mut i = self.lock();
        if i.helper.is_none() {
            i.helper = Some((i.front, i.support.clone()));
        }
        i.front = Front::Browser;
        i.support = Support::Checking;
        i.voices.clear();
        i.out.clear();
        i.out.push_back(ToClient::ListVoices);
    }

    /// The helper page went away: what it was saying fails and the front end is what it was before.
    pub fn detach_helper(&self) {
        let mut i = self.lock();
        let Some((front, support)) = i.helper.take() else { return };
        i.out.clear();
        i.front = front;
        i.support = support;
        i.voices.clear();
        for u in i.utts.values_mut() {
            if matches!(u.state, UttState::Sent | UttState::Started) {
                u.state = UttState::Failed("the voice helper in the browser was closed".into());
            }
        }
    }

    pub fn helper_active(&self) -> bool {
        self.lock().helper.is_some()
    }

    /// The window front end asks to be told when the first reading is wanted (so the probe starts only then).
    pub fn probe_wanted(&self) -> bool {
        self.lock().probe_wanted
    }

    pub fn clear_probe_wanted(&self) {
        self.lock().probe_wanted = false;
    }

    /// Something that wants to speak asks the window front end to check its engine, once.
    pub fn want_probe(&self) {
        let mut i = self.lock();
        if i.front == Front::Window && i.support == Support::Checking {
            i.probe_wanted = true;
        }
    }

    /// Messages for the page, oldest first (the front end sends them in this order).
    pub fn take_outgoing(&self) -> Vec<ToClient> {
        self.lock().out.drain(..).collect()
    }

    /// Something the page said.
    pub fn event(&self, e: FromClient) {
        let mut i = self.lock();
        match e {
            FromClient::Voices { supported, voices } => {
                i.support = if supported { Support::Yes } else { Support::No("this browser has no speech synthesis (window.speechSynthesis)".into()) };
                if supported {
                    i.voices = voices;
                }
            }
            FromClient::Started { id } => {
                if let Some(u) = i.utts.get_mut(&id) {
                    if u.state == UttState::Sent {
                        u.state = UttState::Started;
                    }
                }
            }
            FromClient::Ended { id } => {
                if let Some(u) = i.utts.get_mut(&id) {
                    if matches!(u.state, UttState::Sent | UttState::Started) {
                        u.state = UttState::Ended;
                    }
                }
            }
            FromClient::Error { id, message } => {
                if let Some(u) = i.utts.get_mut(&id) {
                    if matches!(u.state, UttState::Sent | UttState::Started) {
                        u.state = UttState::Failed(message);
                    }
                }
            }
            FromClient::Boundary { id, char_index } => {
                if let Some(u) = i.utts.get_mut(&id) {
                    u.boundary = char_index;
                    u.boundary_seen = true;
                }
            }
        }
    }

    // ---- the reader's side

    pub fn front(&self) -> Front {
        self.lock().front
    }

    pub fn support(&self) -> Support {
        self.lock().support.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.lock().support == Support::Yes
    }

    pub fn voices(&self) -> Vec<VoiceInfo> {
        self.lock().voices.clone()
    }

    /// Say `text`; the id to follow it by. Fails when the screen cannot speak.
    pub fn speak(&self, text: &str, voice: Option<&str>, lang: Option<&str>, rate: f32) -> Result<u64, String> {
        let mut i = self.lock();
        if i.support != Support::Yes {
            return Err("the browser voice is not available on this screen".into());
        }
        let text: String = text.chars().take(MAX_PIECE_CHARS).collect();
        let id = i.next_id;
        i.next_id += 1;
        // keep the table small: finished utterances nobody follows any more
        if i.utts.len() > 64 {
            i.utts.retain(|_, u| matches!(u.state, UttState::Sent | UttState::Started));
        }
        i.utts.insert(id, Utt { state: UttState::Sent, since: Instant::now(), boundary: 0, boundary_seen: false });
        let rate = if rate.is_finite() { rate.clamp(0.5, 2.0) } else { 1.0 };
        i.out.push_back(ToClient::Speak { id, text, voice: voice.filter(|v| !v.is_empty()).map(str::to_string), lang: lang.filter(|l| !l.is_empty()).map(str::to_string), rate });
        Ok(id)
    }

    /// Stop utterance `id` if it is still being said.
    pub fn cancel(&self, id: u64) {
        let mut i = self.lock();
        if i.utts.get(&id).is_some_and(|u| matches!(u.state, UttState::Sent | UttState::Started)) {
            i.out.push_back(ToClient::Cancel { id: Some(id) });
        }
        i.utts.remove(&id);
    }

    pub fn cancel_all(&self) {
        let mut i = self.lock();
        i.utts.clear();
        i.out.push_back(ToClient::Cancel { id: None });
    }

    pub fn pause(&self) {
        self.lock().out.push_back(ToClient::Pause);
    }

    pub fn resume(&self) {
        self.lock().out.push_back(ToClient::Resume);
    }

    /// Where utterance `id` is: a piece the page has not started within the timeout is reported as failed (the usual reason
    /// is a browser that wants a click inside the page first).
    pub fn state(&self, id: u64) -> UttState {
        let i = self.lock();
        match i.utts.get(&id) {
            None => UttState::Ended,
            Some(u) if u.state == UttState::Sent && u.since.elapsed() > i.start_timeout => UttState::Failed("the browser did not start speaking (click once inside the page, then press play again; some browsers only speak after a click)".into()),
            Some(u) => u.state.clone(),
        }
    }

    /// The character the page last reported for utterance `id`.
    pub fn progress(&self, id: u64) -> Option<usize> {
        self.lock().utts.get(&id).map(|u| u.boundary)
    }

    /// The character (UTF-16 units of the text sent, as the browser counts) where the voice last said a word starts: None
    /// until the page has reported one. Some voices (network voices of Chrome) never do, so None is not an error.
    pub fn boundary(&self, id: u64) -> Option<usize> {
        self.lock().utts.get(&id).filter(|u| u.boundary_seen).map(|u| u.boundary)
    }
}

/// The voices of `voices` that speak `lang` (a tag such as `ta` or `en-US`; only the language part is compared).
pub fn voices_for(voices: &[VoiceInfo], lang: &str) -> Vec<VoiceInfo> {
    let want = lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    voices.iter().filter(|v| v.lang.split(['-', '_']).next().unwrap_or("").eq_ignore_ascii_case(&want)).cloned().collect()
}

/// The language tag for a story language of `story::LANGUAGES`.
pub fn lang_tag(language: &str) -> &'static str {
    match language.to_ascii_lowercase().as_str() {
        "tamil" => "ta-IN",
        "hindi" => "hi-IN",
        "telugu" => "te-IN",
        "malayalam" => "ml-IN",
        "kannada" => "kn-IN",
        "bengali" => "bn-IN",
        "marathi" => "mr-IN",
        "spanish" => "es-ES",
        "french" => "fr-FR",
        "german" => "de-DE",
        "portuguese" => "pt-PT",
        "arabic" => "ar-SA",
        "japanese" => "ja-JP",
        _ => "en-US",
    }
}

// ------------------------------------------------------------------------------------------------------------ the reader's plug-ins

/// The marker that makes a "sound" of this engine recognisable. The reader keeps one blob per piece; here it is the text.
const MAGIC: &[u8] = b"IYBV\0";

/// Stands in for a synthesiser: nothing is made, the "sound" of a piece is its text, which the sink then sends to the page.
pub struct BrowserSynth;

impl Synth for BrowserSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        let t = crate::voice::speakable(text);
        if t.is_empty() {
            // nothing to say in this piece (only symbols): a pause the length of nothing
            return Ok([MAGIC, b" "].concat());
        }
        Ok([MAGIC, t.as_bytes()].concat())
    }
}

/// Sends a piece to the page and follows it until the page says it ended.
pub struct BrowserSink {
    pub hv: Arc<HostVoice>,
    pub voice: Option<String>,
    pub lang: Option<String>,
    pub rate: f32,
}

impl Sink for BrowserSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        let text = wav.strip_prefix(MAGIC).map(|t| String::from_utf8_lossy(t).to_string()).ok_or("not a browser voice piece")?;
        if text.trim().is_empty() {
            return Ok(Box::new(Silent));
        }
        let id = self.hv.speak(&text, self.voice.as_deref(), self.lang.as_deref(), self.rate)?;
        let words = crate::wordtime::words_of(&text).len();
        Ok(Box::new(BrowserPlaying { hv: self.hv.clone(), id, text, words }))
    }
}

struct Silent;
impl Playing for Silent {
    fn is_playing(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
}

pub struct BrowserPlaying {
    hv: Arc<HostVoice>,
    id: u64,
    /// What the page was asked to say (boundary events count into it) and how many words it has.
    text: String,
    words: usize,
}

impl Playing for BrowserPlaying {
    fn is_playing(&mut self) -> bool {
        matches!(self.hv.state(self.id), UttState::Sent | UttState::Started)
    }

    fn stop(&mut self) {
        self.hv.cancel(self.id);
    }

    fn error(&mut self) -> Option<String> {
        match self.hv.state(self.id) {
            UttState::Failed(m) => Some(m),
            _ => None,
        }
    }

    fn spoken_word(&mut self) -> Option<(usize, usize)> {
        let units = self.hv.boundary(self.id)?;
        let ch = crate::wordtime::utf16_to_char(&self.text, units);
        let spans = crate::wordtime::words_of(&self.text);
        let k = spans.partition_point(|w| w.end <= ch);
        (k < spans.len()).then_some((k, self.words))
    }
}

impl Drop for BrowserPlaying {
    fn drop(&mut self) {
        // a finished piece leaves nothing behind (cancel of an ended one sends nothing)
        self.hv.cancel(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{Chunk, Reader, Status};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn chunk(i: usize, text: &str) -> Chunk {
        Chunk { unit: i, start: 0, end: text.chars().count(), text: text.into() }
    }

    fn pieces() -> Vec<Chunk> {
        ["The fox woke early.", "She looked at the river.", "The moon was hiding.", "She went home."].iter().enumerate().map(|(i, t)| chunk(i, t)).collect()
    }

    /// A page that is only a thread: it takes what the server sends, "speaks" each piece for `talk`, and answers the way a
    /// browser would. `fail_on` makes it report an error for the piece with that text; `silent` makes it never start.
    struct FakePage {
        hv: Arc<HostVoice>,
        log: Arc<Mutex<Vec<ToClient>>>,
        stop: Arc<AtomicBool>,
    }

    impl FakePage {
        fn start(hv: &Arc<HostVoice>, talk: Duration, fail_on: Option<&'static str>, silent: bool) -> FakePage {
            hv.connect(Front::Browser);
            let log = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (h, l, s) = (hv.clone(), log.clone(), stop.clone());
            std::thread::spawn(move || {
                // the page answers the voice list request like the real one
                let mut current: Option<(u64, Instant, bool)> = None;
                while !s.load(Ordering::Relaxed) {
                    for m in h.take_outgoing() {
                        // the wire format is what travels: every message goes through its text form
                        let m = ToClient::decode(&m.encode()).expect("decodes");
                        l.lock().unwrap().push(m.clone());
                        match m {
                            ToClient::ListVoices => h.event(FromClient::decode(&FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "Test Voice".into(), lang: "en-US".into(), local: true }] }.encode()).unwrap()),
                            ToClient::Speak { id, text, .. } => {
                                if silent {
                                    continue;
                                }
                                h.event(FromClient::Started { id });
                                if fail_on.is_some_and(|f| text.contains(f)) {
                                    h.event(FromClient::Error { id, message: "synthesis-failed".into() });
                                } else {
                                    current = Some((id, Instant::now(), false));
                                }
                            }
                            ToClient::Cancel { id } => {
                                if id.is_none() || current.is_some_and(|c| Some(c.0) == id) {
                                    current = None;
                                }
                            }
                            ToClient::Pause => {}
                            ToClient::Resume => {}
                        }
                    }
                    if let Some((id, t0, _)) = current {
                        if t0.elapsed() >= talk {
                            h.event(FromClient::Boundary { id, char_index: 3 });
                            h.event(FromClient::Ended { id });
                            current = None;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            });
            FakePage { hv: hv.clone(), log, stop }
        }

        fn spoken(&self) -> Vec<String> {
            self.log.lock().unwrap().iter().filter_map(|m| if let ToClient::Speak { text, .. } = m { Some(text.clone()) } else { None }).collect()
        }

        fn cancels(&self) -> usize {
            self.log.lock().unwrap().iter().filter(|m| matches!(m, ToClient::Cancel { .. })).count()
        }
    }

    impl Drop for FakePage {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = &self.hv;
        }
    }

    fn sink(hv: &Arc<HostVoice>) -> Arc<dyn Sink> {
        Arc::new(BrowserSink { hv: hv.clone(), voice: Some("Test Voice".into()), lang: Some("en-US".into()), rate: 1.0 })
    }

    /// Wait for a state. The limit is wall time that only matters when something is wrong (a loaded machine is slow, not wrong).
    fn wait(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out: {what}");
    }

    /// A piece that the page never finishes by itself (the test ends it by pausing, seeking, stopping or closing the page).
    const NEVER: Duration = Duration::from_secs(3600);

    #[test]
    fn boundary_events_become_the_word_number_counting_utf16_like_the_browser() {
        let hv = Arc::new(HostVoice::new());
        hv.connect(Front::Browser);
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        let mut p = sink(&hv).play(&[MAGIC, "😀 Hello big world".as_bytes()].concat()).expect("plays");
        assert_eq!(p.spoken_word(), None, "no boundary event yet: not an error, just unknown");
        let id = hv.take_outgoing().iter().find_map(|m| if let ToClient::Speak { id, .. } = m { Some(*id) } else { None }).expect("sent");
        // the browser counts the emoji as two units: "big" starts at unit 9 (😀=2, space, Hello=5, space)
        hv.event(FromClient::Boundary { id, char_index: 9 });
        assert_eq!(p.spoken_word(), Some((1, 3)), "Hello is word 0 (the emoji is no word), big is word 1... of 3");
        hv.event(FromClient::Boundary { id, char_index: 13 });
        assert_eq!(p.spoken_word(), Some((2, 3)));
        // an event pointing past the text names no word
        hv.event(FromClient::Boundary { id, char_index: 999 });
        assert_eq!(p.spoken_word(), None);
        drop(p);
    }

    #[test]
    fn every_message_survives_its_wire_form_and_bad_ones_are_dropped() {
        let to = [
            ToClient::Speak { id: 7, text: "Hello, \"fox\" — நன்றி".into(), voice: Some("Google US English".into()), lang: Some("en-US".into()), rate: 1.25 },
            ToClient::Speak { id: 8, text: "x".into(), voice: None, lang: None, rate: 1.0 },
            ToClient::Cancel { id: Some(7) },
            ToClient::Cancel { id: None },
            ToClient::Pause,
            ToClient::Resume,
            ToClient::ListVoices,
        ];
        for m in to {
            assert_eq!(ToClient::decode(&m.encode()), Some(m.clone()), "{m:?}");
        }
        let from = [
            FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "Lekha".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "Samantha".into(), lang: "en-US".into(), local: false }] },
            FromClient::Voices { supported: false, voices: vec![] },
            FromClient::Started { id: 1 },
            FromClient::Ended { id: 2 },
            FromClient::Error { id: 3, message: "not-allowed".into() },
            FromClient::Boundary { id: 4, char_index: 12 },
        ];
        for m in from {
            assert_eq!(FromClient::decode(&m.encode()), Some(m.clone()), "{m:?}");
        }
        for bad in ["", "nope", "{}", r#"{"t":"sp_end"}"#, r#"{"t":"sp_end","id":-1}"#, r#"{"t":"sp_start","id":"x"}"#, r#"{"t":"unknown","id":1}"#, r#"{"t":"speak","text":"no id"}"#] {
            assert!(FromClient::decode(bad).is_none(), "{bad}");
        }
        assert!(ToClient::decode(r#"{"t":"speak","text":"no id"}"#).is_none());
        // the page's lists are bounded and cleaned
        let many: Vec<String> = (0..1000).map(|i| format!(r#"{{"n":"v{i}\u0007","l":"en"}}"#)).collect();
        let FromClient::Voices { voices, .. } = FromClient::decode(&format!(r#"{{"t":"sp_voices","ok":true,"voices":[{}]}}"#, many.join(","))).unwrap() else { panic!() };
        assert_eq!(voices.len(), MAX_VOICES);
        assert_eq!(voices[0].name, "v0", "control characters are removed");
        let FromClient::Error { message, .. } = FromClient::decode(&format!(r#"{{"t":"sp_error","id":1,"e":"{}"}}"#, "e".repeat(5000))).unwrap() else { panic!() };
        assert_eq!(message.len(), 200);
    }

    #[test]
    fn the_voice_is_not_available_until_the_page_says_it_can_speak() {
        let hv = HostVoice::new();
        assert_eq!(hv.support(), Support::Unknown);
        assert!(hv.speak("hello", None, None, 1.0).is_err(), "a console cannot speak");
        hv.connect(Front::Browser);
        assert_eq!(hv.support(), Support::Checking);
        assert!(hv.speak("hello", None, None, 1.0).is_err(), "not before the page answered");
        assert_eq!(hv.take_outgoing(), vec![ToClient::ListVoices], "the page is asked for its voices on connecting");
        hv.event(FromClient::Voices { supported: false, voices: vec![] });
        assert!(matches!(hv.support(), Support::No(ref why) if why.contains("speechSynthesis")));
        hv.event(FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "A".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "B".into(), lang: "en-GB".into(), local: true }] });
        assert!(hv.is_ready());
        assert_eq!(voices_for(&hv.voices(), "ta-IN").len(), 1);
        assert_eq!(voices_for(&hv.voices(), "en-US")[0].name, "B", "only the language part is compared");
        assert_eq!(lang_tag("Tamil"), "ta-IN");
        assert_eq!(lang_tag("English"), "en-US");
        let id = hv.speak("a very long piece ".repeat(500).as_str(), None, None, 99.0).unwrap();
        let ToClient::Speak { text, rate, .. } = hv.take_outgoing().remove(0) else { panic!() };
        assert!(text.chars().count() <= MAX_PIECE_CHARS && rate <= 2.0, "what is sent is bounded");
        hv.disconnect();
        assert!(matches!(hv.state(id), UttState::Failed(_)), "a piece in flight fails when the page goes away");
        assert_eq!(hv.support(), Support::Unknown);
        assert!(hv.take_outgoing().is_empty());
    }

    #[test]
    fn pieces_are_spoken_in_order_and_each_end_moves_on_to_the_next() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(30), None, false);
        wait("the page's voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("done", || r.status() == Status::Done);
        let want: Vec<String> = pieces().iter().map(|c| c.text.clone()).collect();
        assert_eq!(page.spoken(), want, "every piece once, in order");
        assert!(page.log.lock().unwrap().iter().all(|m| match m {
            ToClient::Speak { voice, lang, rate, .. } => voice.as_deref() == Some("Test Voice") && lang.as_deref() == Some("en-US") && *rate == 1.0,
            _ => true,
        }));
        assert_eq!(r.snapshot().fraction, 1.0);
    }

    #[test]
    fn reading_can_start_while_the_story_is_still_being_written() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(20), None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        let all = pieces();
        r.start(all[..1].to_vec(), 0, true, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("the first piece", || page.spoken().len() == 1);
        // the piece ends and the reader waits for more text: not finished
        wait("waiting for more", || matches!(r.status(), Status::Buffering));
        assert!(!matches!(r.status(), Status::Done), "waiting for more text, not finished: {:?}", r.status());
        r.append(all[1..3].to_vec());
        wait("two more", || page.spoken().len() == 3);
        r.append(all[3..].to_vec());
        r.close_input();
        wait("done", || r.status() == Status::Done);
        assert_eq!(page.spoken().len(), 4);
    }

    #[test]
    fn pause_stops_the_page_and_play_says_the_piece_again_stop_and_seek_cancel() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, NEVER, None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("playing", || r.status() == Status::Playing);
        r.pause();
        wait("the cancel", || page.cancels() >= 1);
        assert_eq!(r.status(), Status::Paused);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(page.spoken().len(), 1, "nothing new while paused");
        r.resume();
        wait("said again", || page.spoken().len() == 2);
        assert_eq!(page.spoken()[0], page.spoken()[1], "the interrupted piece starts again from its beginning");
        // seek: the piece in progress is cancelled and the chosen one is spoken
        r.seek(2);
        wait("piece 3", || page.spoken().last().is_some_and(|t| t == "The moon was hiding."));
        assert!(page.cancels() >= 2);
        // stop
        let before = page.cancels();
        r.stop();
        wait("the stop reaches the page", || page.cancels() > before);
        assert_eq!(r.status(), Status::Idle);
        let n = page.spoken().len();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(page.spoken().len(), n, "nothing is said after stop");
    }

    /// A sink whose first `play` takes a while to reach the page (a busy thread), like a thread that was just scheduled out.
    struct SlowFirst {
        inner: Arc<dyn Sink>,
        entered: Arc<AtomicBool>,
        first: AtomicBool,
    }

    impl Sink for SlowFirst {
        fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
            if !self.first.swap(true, Ordering::SeqCst) {
                self.entered.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
            }
            self.inner.play(wav)
        }
    }

    #[test]
    fn a_seek_while_the_previous_piece_is_on_its_way_to_the_page_still_reads_the_chosen_piece() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(500), None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        let entered = Arc::new(AtomicBool::new(false));
        let slow: Arc<dyn Sink> = Arc::new(SlowFirst { inner: sink(&hv), entered: entered.clone(), first: AtomicBool::new(false) });
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), slow);
        wait("the first piece is about to be sent", || entered.load(Ordering::SeqCst));
        // the old reading is committed to sending piece 1 when the seek replaces it: the page must never be left with the
        // old piece taking the place of the new one
        r.seek(3);
        wait("done", || r.status() == Status::Done);
        assert_eq!(page.spoken().last().map(String::as_str), Some("She went home."));
        assert!(matches!(hv.state(hv_last_id(&hv)), UttState::Ended));
    }

    fn hv_last_id(hv: &HostVoice) -> u64 {
        hv.lock().next_id - 1
    }

    #[test]
    fn a_failed_piece_is_reported_with_its_number_and_the_reason() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(10), Some("moon"), false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("failure", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("part 3 of 4") && m.contains("synthesis-failed"), "{m}");
        assert_eq!(page.spoken().len(), 3, "the earlier pieces were said, nothing after the failure");
    }

    #[test]
    fn a_page_that_never_starts_speaking_is_reported_and_a_closed_page_ends_reading() {
        let hv = Arc::new(HostVoice::with_start_timeout(Duration::from_millis(150)));
        let _page = FakePage::start(&hv, Duration::from_millis(10), None, true);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("timeout", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("part 1 of 4") && m.contains("click once inside the page"), "{m}");
        // the page goes away mid-piece
        let hv = Arc::new(HostVoice::new());
        let _page = FakePage::start(&hv, NEVER, None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("playing", || r.status() == Status::Playing);
        hv.disconnect();
        wait("failure", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("closed or lost the connection"), "{m}");
    }

    #[test]
    fn the_synth_makes_no_audio_and_symbols_only_pieces_are_skipped_not_sent() {
        let blob = BrowserSynth.synth("**Bold** text, see https://example.com now").unwrap();
        assert!(blob.starts_with(MAGIC) && !blob.starts_with(b"RIFF"));
        assert_eq!(&blob[MAGIC.len()..], "Bold text, see now".as_bytes(), "markup and links are not read out");
        let hv = Arc::new(HostVoice::new());
        hv.connect(Front::Browser);
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        let _ = hv.take_outgoing();
        let s = sink(&hv);
        let mut p = s.play(&BrowserSynth.synth("***").unwrap()).unwrap();
        assert!(!p.is_playing(), "nothing to say, nothing waited for");
        assert!(hv.take_outgoing().is_empty());
        assert!(s.play(b"RIFF....").is_err(), "a sound from another engine is refused, not spoken");
    }

    #[test]
    fn the_reading_status_offers_the_browser_voice_first_and_says_why_it_cannot_be_used() {
        use crate::story::{effective_engine, status_text, tts_status_for, TtsStatus, NO_ENGINE_HELP};
        use crate::voice::{SpeechEngine, VoiceSettings};
        let path = "/nonexistent-dir";
        let none = VoiceSettings::default();
        let hv = HostVoice::new();
        // a console, nothing set up: the help leads with the browser voice and how to get it
        assert_eq!(tts_status_for(&none, true, path, &hv), TtsStatus::NoEngine);
        let help = status_text(&TtsStatus::NoEngine);
        assert_eq!(help, NO_ENGINE_HELP);
        assert!(help.find("browser voice").unwrap() < help.find("Settings → Voice").unwrap(), "{help}");
        assert!(help.contains("iyakki web") && help.contains("iyakki serve --web"), "{help}");
        // a browser that can speak and nothing else set up: the browser voice is used by itself
        hv.connect(Front::Browser);
        assert_eq!(tts_status_for(&none, true, path, &hv), TtsStatus::NoEngine, "still asking the page: not offered yet");
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        assert_eq!(effective_engine(&none, &hv), Some(SpeechEngine::Browser));
        assert!(matches!(tts_status_for(&none, true, path, &hv), TtsStatus::Ready { ref engine } if engine.contains("browser voice")));
        // the account's voice permission still applies
        assert_eq!(tts_status_for(&none, false, path, &hv), TtsStatus::NotAllowed);
        // another engine that is set up wins over the default
        let piper = VoiceSettings { tts: Some(SpeechEngine::Command { command: "piper -f {out}".into() }), ..VoiceSettings::default() };
        assert_eq!(effective_engine(&piper, &hv), piper.tts);
        // chosen explicitly but this screen cannot speak
        let chosen = VoiceSettings { tts: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        hv.disconnect();
        let TtsStatus::BrowserUnavailable(why) = tts_status_for(&chosen, true, path, &hv) else { panic!() };
        assert!(why.contains("console") && why.contains("iyakki web"), "{why}");
        hv.connect(Front::Window);
        hv.set_unsupported("this window's web engine has no speech synthesis");
        let text = status_text(&tts_status_for(&chosen, true, path, &hv));
        assert!(text.contains("no speech synthesis") && text.contains("Settings → Voice"), "{text}");
        // the window asks its engine only when somebody wants to read
        let hw = HostVoice::new();
        hw.connect(Front::Window);
        assert!(!hw.probe_wanted());
        hw.want_probe();
        assert!(hw.probe_wanted());
    }

    #[test]
    fn settings_written_before_the_browser_voice_existed_still_load_and_the_new_kind_round_trips() {
        use crate::voice::{SpeechEngine, VoiceSettings};
        let old = r#"{"stt":{"kind":"http","url":"http://127.0.0.1:8080/v1/audio/transcriptions","model":"tiny.en"},"tts":{"kind":"command","command":"piper -f {out}"},"record_cmd":"arecord -q","play_cmd":"aplay -q","auto_send":false,"speak_replies":false}"#;
        let v: VoiceSettings = serde_json::from_str(old).unwrap();
        assert!(matches!(v.tts, Some(SpeechEngine::Command { .. })) && v.validate().is_ok());
        let none: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert!(none.tts.is_none(), "nothing set up stays nothing set up");
        let b = VoiceSettings { tts: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        let text = serde_json::to_string(&b).unwrap();
        assert!(text.contains(r#""tts":{"kind":"browser"}"#), "{text}");
        assert_eq!(serde_json::from_str::<VoiceSettings>(&text).unwrap(), b);
        assert!(b.validate().is_ok());
        // it only speaks: it cannot be the listening engine, and it makes no audio for anyone who asks for a file
        let bad = VoiceSettings { stt: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        assert!(bad.validate().unwrap_err().contains("cannot listen"));
        assert_eq!(SpeechEngine::Browser.label(), "browser voice");
        assert!(crate::story::engine_takes_speed(&SpeechEngine::Browser) && crate::story::voice_choices(&SpeechEngine::Browser).is_empty());
    }
}
