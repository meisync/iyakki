//! The meeting point of the speech of a browser page and the story reader (F-SPLIT-03): the bridge, its wire messages and the voice tables. The
//! agent re-exports it and adds the reader's plug-ins ([`BrowserSynth`], [`BrowserSink`]), which need the reader's traits.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};


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

    /// The id the last piece was given (tests of the reader follow it).
    #[doc(hidden)]
    pub fn last_id(&self) -> u64 {
        self.lock().next_id - 1
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


// ---- the reader's plug-ins (moved from iyakki_agent::hostvoice)

use iyakki_story_model::{Playing, Sink, Synth};

/// The marker that makes a "sound" of this engine recognisable. The reader keeps one blob per piece; here it is the text.
pub const MAGIC: &[u8] = b"IYBV\0";

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
        let words = iyakki_wordtime::words_of(&text).len();
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
        let ch = iyakki_wordtime::utf16_to_char(&self.text, units);
        let spans = iyakki_wordtime::words_of(&self.text);
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

