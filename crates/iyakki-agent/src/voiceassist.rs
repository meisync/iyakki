//! "My IYAKKI": the personal voice assistant joined to the chat agent (F-VOICE-28, `docs/development/MY-IYAKKI-PLAN.md`).
//!
//! This file is the state machine and nothing else: it decides what happens next from what the ports report. The ports (microphone,
//! speech to text, speaker, the chat agent, the thread log) are traits, so the tests drive the whole loop with fake audio and a
//! scripted model (`voiceassist_tests.rs`) and the real ones live in `voiceassist_ports.rs`.
//!
//! ```text
//! idle -> [tap | key] -> listening (PauseDetector ends it) -> speech to text -> route -> chat agent
//!      -> answer (speech, barge-in) -> follow-up window -> idle
//! ```
//!
//! Rules that are written as tests:
//!
//! * **Control words** (stop, cancel, louder, quieter, mute, that's all, repeat) are handled here, locally and at once; they never
//!   reach the model.
//! * **A transcript the assistant is unsure of** (the engine's confidence is low, or it is a single short word) gets "Did you say ...?"
//!   and nothing runs until the person says yes.
//! * **Approvals by voice**: a tool that asks shows its card and the assistant says what it wants to do. For an ordinary ask a
//!   spoken "yes, do it" or "no" is enough; for the hard-ask list (`agent::hard_ask`, plus a conservative word list in
//!   [`is_hard`]) a spoken yes is **never** enough: only a tap or a key ([`Assistant::approve`]) approves. A spoken no always denies.
//! * **No sound is kept**: the samples live in memory from the microphone to the speech engine and are dropped there; this file
//!   touches no file (a test reads this source to prove it).

use std::collections::VecDeque;

use crate::voicedesk::PauseDetector;

/// The title of the one chat thread the voice conversation joins (the chat marks it as a voice thread).
pub const THREAD_TITLE: &str = "Voice conversation";

/// The addition to the system prompt of a voice session: how to answer when the answer is spoken.
pub const SESSION_PROMPT: &str = "\nVOICE SESSION: the person is talking to you and your answer is read aloud. Answer in one to three short spoken sentences unless they ask for more. Use no markdown, no lists, no tables, no code and no links; write numbers, times and names the way a person would say them. When a result is long, give the gist aloud and say the full result is on the screen. When a tool needs a choice you do not have, ask one short clarifying question instead of guessing. Tools that ask for approval show a card and the person answers it; say plainly what you want to do. Never claim to have done something a tool did not do. The words you receive come from speech recognition and can be wrong: if they look odd, ask what they meant before acting.";

/// The longest a listening lasts (milliseconds), whatever the room does.
pub const MAX_LISTEN_MS: u64 = 20_000;
/// How long the microphone waits for the first word of a request (milliseconds).
pub const FIRST_WORD_MS: u64 = 10_000;
/// The default follow-up window after an answer (milliseconds).
pub const DEFAULT_FOLLOW_MS: u64 = 8_000;
/// How long an ordinary approval waits for a spoken answer (milliseconds).
pub const APPROVAL_WAIT_MS: u64 = 20_000;
/// How long a "did you say" waits for a yes or no (milliseconds).
pub const CONFIRM_WAIT_MS: u64 = 10_000;
/// The longest the agent may think before the assistant stops it (milliseconds).
pub const THINK_MAX_MS: u64 = 180_000;
/// A transcript with less confidence than this is read back first.
pub const MIN_CONFIDENCE: f32 = 0.55;
/// Loud time over the echo gate that counts as speaking over the answer (milliseconds).
pub const BARGE_MS: u64 = 150;
/// Spoken answers are cut after about this many characters (the rest is on the screen).
pub const SPOKEN_CHARS: usize = 420;

pub const SAID_TAP_ONLY: &str = "That one needs a tap or a key on the screen. A spoken yes is not enough.";
pub const SAID_SCREEN: &str = "The full answer is on the screen.";

// ------------------------------------------------------------------------------------------------ ports

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

#[derive(Clone, Debug)]
pub struct Config {
    /// The pause that ends a request (milliseconds, 500 to 4000).
    pub pause_ms: u64,
    /// The follow-up window after an answer; 0 turns it off.
    pub follow_ms: u64,
    /// Speaking over the answer stops it (the microphone is open, gated, while it speaks).
    pub barge_in: bool,
    pub min_confidence: f32,
}

impl Default for Config {
    fn default() -> Config {
        Config { pause_ms: 1500, follow_ms: DEFAULT_FOLLOW_MS, barge_in: true, min_confidence: MIN_CONFIDENCE }
    }
}

// ------------------------------------------------------------------------------------------------ words

/// Lower case, letters and digits only (apostrophes dropped), single spaces; a leading name or "please" removed.
pub fn normalize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    let mut t = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for p in ["hey my iyakki ", "my iyakki ", "hey iyakki ", "iyakki ", "please "] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.to_string();
        }
    }
    t
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Stop,
    Cancel,
    Louder,
    Quieter,
    Mute,
    ThatsAll,
    Repeat,
}

/// The control word a whole utterance is, if it is one. Only whole utterances count: "stop the music" is a request, "stop" is a control.
pub fn control(norm: &str) -> Option<Control> {
    Some(match norm {
        "stop" | "stop it" | "stop talking" | "be quiet" | "quiet" | "enough" | "shush" => Control::Stop,
        "cancel" | "never mind" | "nevermind" | "forget it" | "cancel that" => Control::Cancel,
        "louder" | "speak louder" | "turn it up" | "volume up" | "a bit louder" => Control::Louder,
        "quieter" | "softer" | "turn it down" | "volume down" | "speak softer" | "a bit quieter" => Control::Quieter,
        "mute" | "mute the microphone" | "mute yourself" | "mute the mic" | "mute my iyakki" => Control::Mute,
        "thats all" | "that is all" | "that will be all" | "im done" | "goodbye" | "bye" | "thats it" | "no thanks thats all" => Control::ThatsAll,
        "repeat" | "repeat that" | "say that again" | "say it again" | "what did you say" | "come again" => Control::Repeat,
        _ => return None,
    })
}

pub fn is_yes(norm: &str) -> bool {
    matches!(norm, "yes" | "yeah" | "yep" | "yes please" | "yes do it" | "do it" | "go ahead" | "okay do it" | "ok do it" | "sure" | "approve" | "yes go ahead" | "correct" | "right" | "thats right" | "yes thats right" | "affirmative")
}

pub fn is_no(norm: &str) -> bool {
    matches!(norm, "no" | "nope" | "no thanks" | "dont" | "do not" | "dont do it" | "no dont" | "stop" | "cancel" | "never mind" | "nevermind" | "deny" | "wrong" | "thats wrong" | "not that" | "no thats wrong" | "negative")
}

/// A tool whose approval a spoken yes can never give: everything on the hard-ask list, and (conservatively, because the approval
/// event carries no arguments) anything whose name says it deletes, installs, sends, publishes, pays or reaches secrets, the
/// camera, the screen or a shell.
pub fn is_hard(tool: &str) -> bool {
    if crate::agent::hard_ask(tool, &serde_json::json!({})).is_some() {
        return true;
    }
    const WORDS: [&str; 24] = ["delete", "remove", "uninstall", "install", "send", "publish", "deploy", "forget", "reset", "wipe", "format", "pay", "vault", "secret", "password", "camera", "screen", "shell", "exec", "sudo", "firmware", "share", "drop", "kill"];
    let t = tool.to_lowercase();
    WORDS.iter().any(|w| t.contains(w))
}

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

fn lower_first(s: &str) -> String {
    let mut c = s.trim().chars();
    match c.next() {
        // keep acronyms and names ("VPN", "IYAKKI") as written
        Some(f) if f.is_uppercase() && !c.clone().next().is_some_and(|n| n.is_uppercase()) => f.to_lowercase().chain(c).collect(),
        Some(f) => std::iter::once(f).chain(c).collect(),
        None => String::new(),
    }
}

/// What is said aloud of a long answer: markdown and links removed, whole sentences up to [`SPOKEN_CHARS`], and a pointer to the screen
/// when something was left out.
pub fn spoken_form(text: &str) -> Vec<String> {
    let plain = crate::voice::speakable(&strip_links(text));
    let mut out: Vec<String> = Vec::new();
    let mut used = 0;
    let mut cut = false;
    for c in crate::story::sentence_chunks(&plain, 220) {
        let t = c.text.trim().to_string();
        if t.is_empty() {
            continue;
        }
        if used + t.len() > SPOKEN_CHARS && !out.is_empty() {
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

// ------------------------------------------------------------------------------------------------ the machine

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A request after a tap.
    Request,
    /// A request in the follow-up window.
    FollowUp,
    /// The yes or no of "did you say".
    Confirm,
    /// The spoken answer to an approval.
    Approval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Idle,
    FollowUp,
    Confirm,
    Approval,
    /// Wait for a tap or a key; the microphone stays closed.
    Tap,
}

enum Phase {
    Idle,
    Listen { purpose: Purpose, started: u64, deadline: Option<u64>, det: PauseDetector },
    Think { since: u64 },
    Speak { then: After, loud_ms: u64 },
    AwaitTap,
}

/// What the screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub id: String,
    pub name: String,
    pub summary: String,
    /// A spoken yes does not approve this one: a tap or a key does.
    pub hard: bool,
}

#[derive(Clone, Debug)]
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

pub struct Assistant {
    p: Ports,
    cfg: Config,
    phase: Phase,
    muted: bool,
    now: u64,
    level: f32,
    heard: String,
    said: String,
    last_answer: String,
    note: String,
    approval: Option<PendingApproval>,
    unclear: u32,
    confirm: Option<String>,
    queue: VecDeque<String>,
}

impl Assistant {
    pub fn new(ports: Ports, cfg: Config, muted: bool) -> Assistant {
        Assistant { p: ports, cfg, phase: Phase::Idle, muted, now: 0, level: 0.0, heard: String::new(), said: String::new(), last_answer: String::new(), note: String::new(), approval: None, unclear: 0, confirm: None, queue: VecDeque::new() }
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.cfg
    }

    pub fn status(&self) -> Status {
        let state = if self.muted {
            State::Muted
        } else {
            match &self.phase {
                Phase::Idle => State::Idle,
                Phase::Listen { purpose: Purpose::Request, .. } => State::Listening,
                Phase::Listen { purpose: Purpose::FollowUp, .. } => State::FollowUp,
                Phase::Listen { purpose: Purpose::Confirm, .. } => State::Confirming,
                Phase::Listen { purpose: Purpose::Approval, .. } | Phase::AwaitTap => State::AwaitApproval,
                Phase::Think { .. } => State::Thinking,
                Phase::Speak { .. } => State::Speaking,
            }
        };
        Status { state, muted: self.muted, mic_open: self.p.audio.is_open(), level: self.level, heard: self.heard.clone(), said: self.said.clone(), note: self.note.clone(), approval: self.approval.clone(), confirm: self.confirm.clone() }
    }

    fn close_audio(&mut self) {
        self.p.audio.discard();
        self.level = 0.0;
    }

    /// Everything stops: speech, the microphone, the agent's run. Back to idle.
    pub fn stop(&mut self) {
        self.p.speaker.stop();
        self.close_audio();
        if !matches!(self.phase, Phase::Idle) {
            self.p.agent.cancel();
        }
        self.queue.clear();
        self.approval = None;
        self.confirm = None;
        self.unclear = 0;
        self.phase = Phase::Idle;
    }

    /// Mute: the microphone is closed at once and stays closed (the caller keeps the state across restarts).
    pub fn set_muted(&mut self, muted: bool) {
        if muted {
            self.stop();
            self.note = "Muted: the microphone is off.".into();
        } else {
            self.note = "Unmuted.".into();
        }
        self.muted = muted;
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    fn start_listen(&mut self, purpose: Purpose, deadline_ms: Option<u64>) {
        if self.muted {
            return;
        }
        if let Err(e) = self.p.audio.open() {
            self.note = e;
            self.phase = Phase::Idle;
            return;
        }
        let deadline = deadline_ms.map(|d| self.now + d);
        self.phase = Phase::Listen { purpose, started: self.now, deadline, det: PauseDetector::with_pause(self.cfg.pause_ms) };
    }

    /// A tap or a key on the assistant.
    pub fn tap(&mut self, now: u64) {
        self.now = now;
        if self.muted {
            self.note = "Muted: unmute first (the microphone is off).".into();
            return;
        }
        match self.phase {
            Phase::Idle => {
                self.note.clear();
                self.start_listen(Purpose::Request, Some(FIRST_WORD_MS));
            }
            Phase::Listen { purpose, .. } => self.finish_listen(purpose),
            Phase::Speak { .. } => {
                self.p.speaker.stop();
                self.queue.clear();
                self.close_audio();
                self.start_listen(Purpose::Request, Some(FIRST_WORD_MS));
            }
            Phase::Think { .. } => self.stop(),
            Phase::AwaitTap => self.note = "Tap Allow or Deny on the card.".into(),
        }
    }

    /// The person's tap or key on the approval card. This is the only way a hard-ask tool is approved.
    pub fn approve(&mut self, id: &str, yes: bool) -> bool {
        match &self.approval {
            Some(a) if a.id == id => {}
            _ => return false,
        }
        self.decide(yes);
        true
    }

    /// The person's tap or key on "did you say ...".
    pub fn confirm(&mut self, yes: bool) -> bool {
        let Some(text) = self.confirm.take() else { return false };
        self.p.speaker.stop();
        self.queue.clear();
        self.close_audio();
        if yes {
            self.send(&text);
        } else {
            self.phase = Phase::Idle;
            self.note = "Okay, I did not do anything.".into();
        }
        true
    }

    fn decide(&mut self, yes: bool) {
        let Some(a) = self.approval.take() else { return };
        self.p.speaker.stop();
        self.queue.clear();
        self.close_audio();
        self.unclear = 0;
        self.p.agent.decide(&a.id, yes);
        self.phase = Phase::Think { since: self.now };
    }

    fn send(&mut self, text: &str) {
        self.heard = text.to_string();
        self.p.log.record(Who::Person, text);
        self.p.agent.ask(text);
        self.phase = Phase::Think { since: self.now };
    }

    fn speak(&mut self, lines: Vec<String>, then: After) {
        self.p.speaker.stop();
        self.queue = lines.into();
        self.close_audio();
        // the microphone stays open (gated) while it speaks so that talking over it works
        if self.cfg.barge_in && !self.muted && self.p.audio.open().is_err() {
            self.level = 0.0;
        }
        self.phase = Phase::Speak { then, loud_ms: 0 };
    }

    fn say_one(&mut self, line: &str, then: After) {
        self.speak(vec![line.to_string()], then);
    }

    fn follow(&self) -> After {
        if self.cfg.follow_ms > 0 { After::FollowUp } else { After::Idle }
    }

    fn after(&mut self, then: After) {
        match then {
            After::Idle => self.phase = Phase::Idle,
            After::FollowUp => self.start_listen(Purpose::FollowUp, Some(self.cfg.follow_ms)),
            After::Confirm => self.start_listen(Purpose::Confirm, Some(CONFIRM_WAIT_MS)),
            After::Approval => self.start_listen(Purpose::Approval, Some(APPROVAL_WAIT_MS)),
            After::Tap => self.phase = Phase::AwaitTap,
        }
    }

    // ---------------------------------------------------------------------------------------- listening

    fn finish_listen(&mut self, purpose: Purpose) {
        let pcm = match self.p.audio.take() {
            Ok(p) => p,
            Err(e) => {
                self.note = e;
                self.phase = Phase::Idle;
                return;
            }
        };
        self.level = 0.0;
        self.phase = Phase::Idle;
        let heard = self.p.stt.hear(&pcm);
        drop(pcm);
        match heard {
            Ok(h) => self.route(purpose, h),
            Err(e) => self.speech_failed(purpose, &e),
        }
    }

    fn speech_failed(&mut self, purpose: Purpose, why: &str) {
        let none = crate::voicedesk::is_no_words(why);
        match purpose {
            Purpose::FollowUp => {
                // the window closed on silence: no fuss
                self.phase = Phase::Idle;
            }
            Purpose::Approval => self.approval_unclear(),
            Purpose::Confirm => {
                self.confirm = None;
                self.phase = Phase::Idle;
                self.note = "I did not hear a yes, so I did nothing.".into();
            }
            Purpose::Request => {
                self.note = if none { "I did not hear anything. Tap and try again.".into() } else { why.to_string() };
                self.phase = Phase::Idle;
            }
        }
    }

    fn route(&mut self, purpose: Purpose, h: Heard) {
        let text = h.text.trim().to_string();
        let norm = normalize(&text);
        if norm.is_empty() {
            return self.speech_failed(purpose, crate::voicedesk::NO_SPEECH);
        }
        match purpose {
            Purpose::Approval => self.route_approval(&norm),
            Purpose::Confirm => {
                if is_yes(&norm) {
                    if let Some(t) = self.confirm.take() {
                        return self.send(&t);
                    }
                } else if is_no(&norm) {
                    self.confirm = None;
                    return self.say_one("Okay, I will not do that.", After::Idle);
                }
                // something else: it is a new request
                self.confirm = None;
                self.route_request(&text, &norm, h.confidence);
            }
            Purpose::Request | Purpose::FollowUp => self.route_request(&text, &norm, h.confidence),
        }
    }

    fn route_request(&mut self, text: &str, norm: &str, confidence: Option<f32>) {
        self.heard = text.to_string();
        if let Some(c) = control(norm) {
            return self.do_control(c);
        }
        // a yes or a no that answers a question the assistant just asked needs no read-back
        let answers_question = (is_yes(norm) || is_no(norm)) && self.last_answer.trim_end().ends_with('?');
        let unsure = confidence.is_some_and(|c| c < self.cfg.min_confidence) || norm.split_whitespace().count() < 2 || norm.chars().count() < 4;
        if unsure && !answers_question {
            self.confirm = Some(text.to_string());
            return self.say_one(&format!("Did you say: {text}? Say yes or no."), After::Confirm);
        }
        self.send(text);
    }

    fn do_control(&mut self, c: Control) {
        match c {
            Control::Stop | Control::Cancel => {
                self.stop();
                self.note = "Stopped.".into();
            }
            Control::Louder => {
                self.p.speaker.louder();
                self.say_one("Louder.", self.follow());
            }
            Control::Quieter => {
                self.p.speaker.quieter();
                self.say_one("Quieter.", self.follow());
            }
            Control::Mute => {
                self.set_muted(true);
            }
            Control::ThatsAll => self.say_one("Okay.", After::Idle),
            Control::Repeat => {
                if self.last_answer.is_empty() {
                    self.say_one("I have nothing to repeat yet.", self.follow());
                } else {
                    let a = self.last_answer.clone();
                    let f = self.follow();
                    self.speak(spoken_form(&a), f);
                }
            }
        }
    }

    fn route_approval(&mut self, norm: &str) {
        let Some(a) = self.approval.clone() else {
            self.phase = Phase::Idle;
            return;
        };
        if is_no(norm) || control(norm) == Some(Control::Stop) || control(norm) == Some(Control::Cancel) {
            return self.decide(false);
        }
        if is_yes(norm) {
            if a.hard {
                // the hard rule: a voice cannot approve this, so a TV or a passer-by cannot either
                self.note = SAID_TAP_ONLY.into();
                return self.say_one(SAID_TAP_ONLY, After::Tap);
            }
            return self.decide(true);
        }
        self.approval_unclear();
    }

    fn approval_unclear(&mut self) {
        let hard = self.approval.as_ref().is_some_and(|a| a.hard);
        self.unclear += 1;
        if self.unclear < 2 {
            return self.say_one("Please say yes or no.", After::Approval);
        }
        if hard {
            self.say_one("Tap Allow or Deny on the card.", After::Tap);
        } else {
            self.note = "No clear answer, so I did not do it.".into();
            self.decide(false);
        }
    }

    // ---------------------------------------------------------------------------------------- the clock

    /// Advance to `now` (milliseconds on any steady clock). Call about every 50 ms.
    pub fn tick(&mut self, now: u64) {
        let dt = now.saturating_sub(self.now).min(1000);
        self.now = now;
        if self.muted {
            return;
        }
        match self.phase {
            Phase::Idle | Phase::AwaitTap => {}
            Phase::Listen { .. } => self.tick_listen(),
            Phase::Think { since } => self.tick_think(since),
            Phase::Speak { .. } => self.tick_speak(dt),
        }
    }

    fn tick_listen(&mut self) {
        let level = self.p.audio.level();
        self.level = level;
        let alive = self.p.audio.alive();
        let now = self.now;
        let Phase::Listen { purpose, started, deadline, det } = &mut self.phase else { return };
        let (purpose, started, deadline) = (*purpose, *started, *deadline);
        let pause = det.tick(level, now);
        let heard = det.has_heard();
        if pause || (heard && !alive) || now.saturating_sub(started) >= MAX_LISTEN_MS && heard {
            return self.finish_listen(purpose);
        }
        if !alive {
            self.close_audio();
            self.note = "The microphone stopped. Is it plugged in and not in use by another program?".into();
            self.phase = Phase::Idle;
            return;
        }
        if !heard && deadline.is_some_and(|d| now >= d) {
            self.close_audio();
            match purpose {
                Purpose::FollowUp => self.phase = Phase::Idle,
                Purpose::Request => {
                    self.note = "I did not hear anything. Tap and try again.".into();
                    self.phase = Phase::Idle;
                }
                Purpose::Confirm => {
                    self.confirm = None;
                    self.note = "I did not hear a yes, so I did nothing.".into();
                    self.phase = Phase::Idle;
                }
                Purpose::Approval => self.approval_unclear(),
            }
        }
    }

    fn tick_think(&mut self, since: u64) {
        while let Some(ev) = self.p.agent.poll() {
            match ev {
                AgentEvent::Approval { id, name, summary } => {
                    let hard = is_hard(&name);
                    self.approval = Some(PendingApproval { id, name, summary: summary.clone(), hard });
                    self.unclear = 0;
                    let ask = if hard {
                        format!("I would like to {}. This one needs a tap or a key on the screen; a spoken yes is not enough.", lower_first(&summary))
                    } else {
                        format!("I would like to {}. Say yes, do it, or no.", lower_first(&summary))
                    };
                    self.said = ask.clone();
                    // a hard ask listens too: a spoken no denies it, a spoken yes is refused and points to the tap
                    return self.say_one(&ask, After::Approval);
                }
                AgentEvent::Final(text) => {
                    self.p.log.record(Who::Assistant, &text);
                    self.last_answer = text.clone();
                    let lines = spoken_form(&text);
                    self.said = lines.join(" ");
                    let f = self.follow();
                    return self.speak(lines, f);
                }
                AgentEvent::Failed(e) => {
                    self.note = e.clone();
                    return self.say_one("Sorry, that did not work. The reason is on the screen.", After::Idle);
                }
                AgentEvent::Stopped => {
                    self.phase = Phase::Idle;
                    return;
                }
            }
        }
        if self.now.saturating_sub(since) >= THINK_MAX_MS {
            self.p.agent.cancel();
            self.note = "That took too long, so I stopped it.".into();
            self.say_one("That is taking too long, so I stopped.", After::Idle);
        }
    }

    fn tick_speak(&mut self, dt: u64) {
        let Phase::Speak { then, loud_ms, .. } = self.phase else { return };
        // barge-in: the microphone is gated by what the assistant itself is playing, so its own echo does not count
        let mut loud = loud_ms;
        if self.cfg.barge_in && self.p.audio.is_open() {
            let mic = self.p.audio.level();
            let gate = 0.20 + 1.5 * self.p.speaker.level();
            loud = if mic > gate { loud + dt } else { 0 };
            if loud >= BARGE_MS {
                self.p.speaker.stop();
                self.queue.clear();
                self.note = "Interrupted.".into();
                let purpose = match then {
                    After::Approval => Purpose::Approval,
                    After::Confirm => Purpose::Confirm,
                    After::Tap => Purpose::Approval,
                    _ => Purpose::Request,
                };
                // the capture that heard the interruption keeps running: it holds the first words of what the person says
                let det = PauseDetector::with_pause(self.cfg.pause_ms);
                self.phase = Phase::Listen { purpose, started: self.now, deadline: None, det };
                return;
            }
        }
        if let Phase::Speak { loud_ms, .. } = &mut self.phase {
            *loud_ms = loud;
        }
        if self.p.speaker.busy() {
            return;
        }
        match self.queue.pop_front() {
            Some(line) => {
                if let Err(e) = self.p.speaker.say(&line) {
                    // no voice: the words are on the screen; carry on
                    self.note = e;
                    self.queue.clear();
                }
            }
            None => {
                // a fresh microphone for the next listening, so the assistant's own echo is not in what is heard
                self.close_audio();
                self.after(then);
            }
        }
    }
}
