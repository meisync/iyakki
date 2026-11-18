//! Types moved out of `iyakki_agent::dictfix` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use crate::settings::ModelChoice;

/// Why a correction ended as it did (the audit line and the status tool carry this word, never any text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The corrected words were used.
    Changed,
    /// The model had nothing to correct (or there were no words).
    Unchanged,
    /// The model did not answer in time.
    Timeout,
    /// The answer was refused by the checks (a rewrite, empty).
    Guard,
    /// The model could not be reached or the correction could not start.
    Error,
    /// No chat model is set up.
    NoModel,
    /// The model is on another machine the person has not agreed to send text to.
    Consent,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Changed => "changed",
            Reason::Unchanged => "unchanged",
            Reason::Timeout => "timeout",
            Reason::Guard => "guard",
            Reason::Error => "error",
            Reason::NoModel => "nomodel",
            Reason::Consent => "consent",
        }
    }

    /// The raw words were kept (not `Changed` or `Unchanged`).
    pub fn kept(self) -> bool {
        !matches!(self, Reason::Changed | Reason::Unchanged)
    }
}

/// The most chat messages (and characters of each) given for names and terms.
pub const CHAT_MESSAGES: usize = 2;

/// The audit action the callers write (never with any text).
pub const AUDIT: &str = "voice.fixwording";

/// What the correction may use besides the new text.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The text already corrected earlier in this session (only the last [`CONTEXT_CHARS`] characters are used).
    pub earlier: String,
    /// The last chat messages, for names and terms (at most [`CHAT_MESSAGES`]).
    pub chat: Vec<String>,
    /// Words the person wants spelled as given.
    pub terms: Vec<String>,
    /// The model of the current chat, when it has its own; else the chat model of Settings.
    pub model: Option<ModelChoice>,
}

impl Context {
    pub fn new() -> Context {
        Context::default()
    }

    /// With the session's text so far.
    pub fn after(earlier: &str) -> Context {
        Context { earlier: earlier.to_string(), ..Context::default() }
    }
}

/// The result of one correction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixed {
    /// What goes into the box: the corrected text, or the raw words when nothing could be corrected.
    pub text: String,
    /// What the speech engine heard.
    pub raw: String,
    /// `text` differs from `raw` (the box can offer "Show original").
    pub changed: bool,
    /// Why the raw words were kept, in plain words (None when the correction was used, or the text had nothing to correct).
    pub kept_because: Option<String>,
    /// The words were not sent because the model is on another machine the person has not agreed to yet: `(server, question)`.
    pub needs_consent: Option<(String, String)>,
    /// The kind of result, for the audit line and the status tool.
    pub reason: Reason,
}

impl Fixed {
    pub fn kept(raw: &str, reason: Reason, why: impl Into<String>) -> Fixed {
        Fixed { text: raw.to_string(), raw: raw.to_string(), changed: false, kept_because: Some(why.into()), needs_consent: None, reason }
    }

    /// The audit detail: the kind of result only.
    pub fn audit_detail(&self) -> &'static str {
        self.reason.label()
    }
}

/// Which model the correction would use, and whether it may be used now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// No chat model is set up.
    NoModel,
    /// On this computer, or on a machine the person agreed to.
    Ready(ModelChoice),
    /// On another machine the person has not agreed to yet: `(server, question)`.
    NeedsConsent(ModelChoice, String, String),
}
