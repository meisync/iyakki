//! The wake phrase hand-off of the Gemini voice assistant (EXPERIMENTAL, opt-in twice; docs/development/GEMINI-VOICE-DESIGN.md, slice 3).
//!
//! The existing LOCAL spotter (`wakelisten`) listens; its audio is thrown away on this computer. Only when it fires does this module
//! play the chime and open a live session, and the sound that made it fire is NOT sent: the first audio Google gets is what comes
//! after the chime. This file opens no device and no connection itself: the spotter, the chime and the connector are handed in, so a
//! test with a fake spotter and a fake network sink proves that nothing leaves before the phrase is heard.
//!
//! Opt-ins, all read again as sound arrives (switching one off at once stops the hand-off and wipes the spotter): the wake phrase
//! itself (`voice.my_wake`, the caller's `wake_on`), the Gemini voice assistant (`LiveCfg::enabled`) and the hand-off
//! (`LiveCfg::wake_handoff`, off by default). The consent gate of the session still applies (no yes: a fallback line and nothing sent).

use std::sync::Arc;

use serde_json::Value;

use crate::gemini;
use crate::geminilive::{start_with, End, LiveDeps, LiveSession, ToolHandler};

/// The local spotter, fed 16-bit little-endian PCM (16 kHz mono). It never touches the network.
pub trait Detector: Send {
    /// True when the phrase was heard in what has been fed so far.
    fn feed(&mut self, pcm: &[u8]) -> bool;
    /// Forget everything heard (the ring).
    fn wipe(&mut self);
}

/// Builds the handler and declarations for a session (the chat's tools for the account); `None` offers no tools.
pub type MakeTools = Box<dyn Fn() -> Option<(Box<dyn ToolHandler>, Vec<Value>)> + Send>;

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// An opt-in is off: nothing is kept or sent.
    Off,
    /// Listening locally; the sound was dropped.
    Listening,
    /// The phrase was heard, the chime played, a session is open.
    Started,
    /// The phrase was heard but the session could not start: the one line for the person (the local voice answers instead).
    Fallback(String),
    /// Audio went into the open session.
    Sent,
    /// The session ended.
    Ended(End),
}

pub struct WakeLink {
    det: Box<dyn Detector>,
    deps: LiveDeps,
    wake_on: Arc<dyn Fn() -> bool + Send + Sync>,
    chime: Box<dyn FnMut() + Send>,
    tools: Option<MakeTools>,
    session: Option<LiveSession>,
    checked_at: u64,
    opted: bool,
}

impl WakeLink {
    /// `wake_on`: the wake phrase's own opt-in (`voice.my_wake` and My IYAKKI on).
    pub fn new(det: Box<dyn Detector>, deps: LiveDeps, wake_on: Arc<dyn Fn() -> bool + Send + Sync>, chime: Box<dyn FnMut() + Send>, tools: Option<MakeTools>) -> WakeLink {
        WakeLink { det, deps, wake_on, chime, tools, session: None, checked_at: 0, opted: false }
    }

    /// All opt-ins are on (the file is read at most twice a second).
    fn opted_in(&mut self) -> bool {
        let now = (self.deps.now_ms)();
        if self.checked_at == 0 || now.saturating_sub(self.checked_at) >= 500 {
            self.checked_at = now.max(1);
            let c = gemini::load(&self.deps.dir).voice;
            self.opted = (self.wake_on)() && c.enabled && c.wake_handoff;
        }
        self.opted
    }

    pub fn session_mut(&mut self) -> Option<&mut LiveSession> {
        self.session.as_mut()
    }

    pub fn is_live(&self) -> bool {
        self.session.as_ref().is_some_and(LiveSession::is_open)
    }

    /// One microphone chunk. Before the phrase it goes to the local spotter only.
    pub fn feed(&mut self, pcm: &[u8]) -> Step {
        if let Some(s) = self.session.as_mut() {
            return match s.send_audio(pcm) {
                Ok(()) => Step::Sent,
                Err(e) => {
                    self.session = None;
                    Step::Ended(e)
                }
            };
        }
        if !self.opted_in() {
            self.det.wipe();
            return Step::Off;
        }
        if !self.det.feed(pcm) {
            return Step::Listening;
        }
        // heard: forget the ring (that sound is never sent), chime, open the session
        self.det.wipe();
        (self.chime)();
        let (handler, decls) = match self.tools.as_ref().and_then(|m| m()) {
            Some((h, d)) => (Some(h), d),
            None => (None, Vec::new()),
        };
        match start_with(&self.deps, handler, &decls) {
            Ok(s) => {
                self.session = Some(s);
                Step::Started
            }
            Err(line) => Step::Fallback(line),
        }
    }

    /// The session ended on its own (or the caller closed it): back to listening locally.
    pub fn release(&mut self) {
        self.session = None;
        self.det.wipe();
    }
}
