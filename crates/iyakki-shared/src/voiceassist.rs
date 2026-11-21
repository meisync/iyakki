//! Types moved out of `iyakki_agent::voiceassist` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What a turn the person SPOKE starts with in the Voice conversation thread, so it can be told from one typed into the same thread
/// later (F-VOICE-40). A transcript can hold recognition mistakes; the model sees the mark too and may ask when a word looks wrong.
pub const SPOKEN_MARK: &str = "(spoken) ";

/// The default follow-up window after an answer (milliseconds).
pub const DEFAULT_FOLLOW_MS: u64 = 8_000;

/// A transcript with less confidence than this is read back first.
pub const MIN_CONFIDENCE: f32 = 0.55;

#[derive(Clone, Debug)]
pub struct Config {
    /// The pause that ends a request (milliseconds, 500 to 4000).
    pub pause_ms: u64,
    /// The follow-up window after an answer; 0 turns it off.
    pub follow_ms: u64,
    /// Speaking over the answer stops it (the microphone is open, gated, while it speaks).
    pub barge_in: bool,
    pub min_confidence: f32,
    /// A spoken "yes" may approve an ordinary ask (Settings -> Voice, off by default). Never for the hard-ask list ([`is_hard`]).
    pub spoken_approvals: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { pause_ms: 1500, follow_ms: DEFAULT_FOLLOW_MS, barge_in: true, min_confidence: MIN_CONFIDENCE, spoken_approvals: false }
    }
}

/// What the screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum State {
    Idle,
    Muted,
    Listening,
    FollowUp,
    Confirming,
    AwaitApproval,
    Thinking,
    Speaking,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Idle => "ready",
            State::Muted => "muted",
            State::Listening => "listening",
            State::FollowUp => "listening for a follow-up",
            State::Confirming => "waiting for your yes or no",
            State::AwaitApproval => "waiting for your answer",
            State::Thinking => "thinking",
            State::Speaking => "speaking",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingApproval {
    pub id: String,
    pub name: String,
    pub summary: String,
    /// On the hard-ask list: a spoken yes can never approve it.
    pub hard: bool,
    /// A spoken yes may approve this one: it is not hard AND the person turned spoken approvals on. Otherwise only a tap or a key approves.
    pub voice_ok: bool,
}

/// What the permanent microphone glyph in the top bar shows (F-VOICE-39): nothing while My IYAKKI is off or its microphone is closed,
/// amber while it is muted, red while the microphone is really open. It follows the port's own answer, never a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MicGlyph {
    Off,
    Muted,
    Listening,
}

impl MicGlyph {
    /// Whether the top bar shows the microphone glyph at all.
    pub fn shown(self) -> bool {
        self != MicGlyph::Off
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Status {
    pub state: State,
    pub muted: bool,
    /// The microphone is open right now (the screen's recording indicator follows this, nothing else).
    pub mic_open: bool,
    pub level: f32,
    /// The last thing the person said, as text.
    pub heard: String,
    /// The last thing the assistant said.
    pub said: String,
    /// A line about what just happened (an error, "I did not hear anything").
    pub note: String,
    pub approval: Option<PendingApproval>,
    /// The transcript waiting for a yes ("did you say ...").
    pub confirm: Option<String>,
}


/// The microphone. One owner at a time; the samples are 16 kHz mono and never leave memory.
pub trait AudioIn: Send {
    fn open(&mut self) -> Result<(), String>;
    fn is_open(&self) -> bool;
    /// Loudness of the last moment, 0 to 1.
    fn level(&mut self) -> f32;
    /// The capture is still running.
    fn alive(&mut self) -> bool;
    /// Stop and hand over the samples (closing the microphone).
    fn take(&mut self) -> Result<Vec<i16>, String>;
    /// Stop and drop everything heard.
    fn discard(&mut self);
}

/// What the speech engine heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub text: String,
    /// 0 to 1 when the engine says; `None` when it does not.
    pub confidence: Option<f32>,
}

/// Speech to text. `Err` is the engine's plain-words reason; "no words" is `voicedesk::is_no_words`.
pub trait Stt: Send {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String>;
}

/// The speaker: one sentence at a time.
pub trait Speaker: Send {
    /// Start saying `text` (returns at once; [`Speaker::busy`] says when it ended).
    fn say(&mut self, text: &str) -> Result<(), String>;
    fn busy(&mut self) -> bool;
    fn stop(&mut self);
    /// How loud the assistant's own voice is right now, 0 to 1 (compared with the microphone so its echo is not a barge-in).
    fn level(&self) -> f32;
    fn louder(&mut self);
    fn quieter(&mut self);
}

/// What the chat agent reports back.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// A tool wants the person's yes. `name` is the tool, `summary` what it will do.
    Approval { id: String, name: String, summary: String },
    Final(String),
    Failed(String),
    Stopped,
}

/// The chat agent in a voice session (the same agent, tools and approvals as the chat).
pub trait AgentPort: Send {
    fn ask(&mut self, text: &str);
    fn poll(&mut self) -> Option<AgentEvent>;
    fn decide(&mut self, id: &str, yes: bool);
    fn cancel(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Person,
    Assistant,
}

/// The voice thread of the chat: what was said, as text (never sound).
pub trait TurnLog: Send {
    fn record(&mut self, who: Who, text: &str);
}

pub struct Ports {
    pub audio: Box<dyn AudioIn>,
    pub stt: Box<dyn Stt>,
    pub speaker: Box<dyn Speaker>,
    pub agent: Box<dyn AgentPort>,
    pub log: Box<dyn TurnLog>,
}

pub const SAID_SCREEN: &str = "The full answer is on the screen.";
/// Spoken answers are cut after about this many characters (the rest is on the screen).
pub const SPOKEN_CHARS: usize = 420;
/// Markdown links `[words](address)` become `words`.
fn strip_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        match after.find("](").and_then(|j| after[j + 2..].find(')').map(|k| (j, k))) {
            Some((j, k)) if !after[..j].contains('\n') => {
                out.push_str(&rest[..i]);
                out.push_str(&after[..j]);
                rest = &after[j + 2 + k + 1..];
            }
            _ => {
                out.push_str(&rest[..i + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What is said aloud of a long answer: markdown and links removed, whole sentences up to [`SPOKEN_CHARS`], and a pointer to the screen
/// when something was left out.
pub fn spoken_form(text: &str) -> Vec<String> {
    spoken_form_cap(text, SPOKEN_CHARS)
}

/// [`spoken_form`] with another cap (the Gemini voice session has its own, set by the person).
pub fn spoken_form_cap(text: &str, cap: usize) -> Vec<String> {
    let plain = crate::voice::speakable(&strip_links(text));
    let mut out: Vec<String> = Vec::new();
    let mut used = 0;
    let mut cut = false;
    for c in crate::story::sentence_chunks(&plain, 220) {
        let t = c.text.trim().to_string();
        if t.is_empty() {
            continue;
        }
        if used + t.len() > cap && !out.is_empty() {
            cut = true;
            break;
        }
        used += t.len();
        out.push(t);
    }
    if out.is_empty() {
        out.push("Done.".into());
    } else if cut || text.contains("```") {
        out.push(SAID_SCREEN.into());
    }
    out
}

