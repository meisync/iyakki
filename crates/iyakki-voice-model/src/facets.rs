//! The traits the Voice tile and the agent both name, over leaf types only: the speech kit (the sound a recording plays through, the
//! revision of a dictated text), the running voice assistant worker, the voice desk and the pending-work waker (moved from
//! `iyakki-agent-api`, which re-exports them; alpha.11 AP1, D-P3-01).

use std::sync::Arc;

use iyakki_story_model::{Chunk, Playing, Sink, Synth};

const UNAVAILABLE_B6: &str = "unavailable: this core does not serve this call";

/// What the waker hands the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Start a turn with this text.
    Turn { text: String, ids: Vec<String> },
    /// The daily chain limit was reached: say so plainly, start nothing.
    Limit { text: String },
}

/// A sound sink that plays nothing (a core without sound).
pub struct NoSink;
impl Sink for NoSink {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        Err(UNAVAILABLE_B6.into())
    }
}

/// Speech of this install: the sound a recording plays through, and the assistant that revises a dictated text (the real one is
/// `iyakki_agent::facets_b6::SpeechKitReal`).
pub trait SpeechKit: Send + Sync {
    /// Plays WAV bytes of a recording through the one way sound reaches the speaker (the mixer, or the play command of Settings -> Voice).
    fn recording_sink(&self) -> Arc<dyn Sink> {
        Arc::new(NoSink)
    }
    /// The model a revision would use and whether it may be used now.
    fn revise_target(&self, _ctx: &crate::revise::Context) -> crate::dictfix::Target {
        crate::dictfix::Target::NoModel
    }
    /// Revise `text` with the person's chat model (blocks up to the revise timeout: call it from a worker thread).
    fn revise(&self, text: &str, _kind: &crate::revise::Kind, _ctx: &crate::revise::Context) -> crate::revise::Proposal {
        crate::revise::Proposal::kept(text, UNAVAILABLE_B6)
    }
}

/// The fake: no sound, no model.
pub struct NoSpeechKit;
impl SpeechKit for NoSpeechKit {}

/// The running voice assistant worker, as the Voice tile drives it.
pub trait MineWorker: Send + Sync {
    fn send(&self, cmd: crate::voiceassist_ports::Cmd);
    fn status(&self) -> crate::voiceassist::Status;
    /// The wake phrase was heard: start listening, with what was said after it.
    fn wake(&self, text: &str);
}

/// The voice desk over the person's settings and providers: the voices to read with, the engine that listens, where text and sound go,
/// and the reading itself.
pub trait VoiceDeskFacet: Send + Sync {
    /// The reading voices that can be used now.
    fn read_voices(&self) -> Vec<crate::voicedesk::ReadVoice> {
        Vec::new()
    }
    /// The engine that turns the microphone's sound into text (None: none is set).
    fn stt_engine(&self) -> Option<crate::voice::SpeechEngine> {
        None
    }
    /// The line that says where the text or sound goes.
    fn where_text(&self, _engine: &crate::voice::SpeechEngine, _what: &str) -> String {
        String::new()
    }
    /// The card shown before text or sound goes to a machine the person has not agreed to yet (None: nothing to ask).
    fn consent_text(&self, _engine: &crate::voice::SpeechEngine, _listening: bool) -> Option<(crate::voicedesk::Dest, String)> {
        None
    }
    /// Sound to text with the engine.
    fn transcribe(&self, _engine: &crate::voice::SpeechEngine, _pcm: &[i16]) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
    /// The pieces to read: one per sentence.
    fn pieces(&self, _text: &str) -> Vec<Chunk> {
        Vec::new()
    }
    /// Plays the sound of the pieces with the play command (a child process per piece).
    fn command_sink(&self, _play_cmd: String) -> Option<Arc<dyn Sink>> {
        None
    }
    /// Makes the sound of one piece with the chosen voice and speed.
    fn choice_synth(&self, _engine: crate::voice::SpeechEngine, _opts: crate::voice::SpeakOpts) -> Option<Arc<dyn Synth>> {
        None
    }
}

/// A running watcher of the person's pending work: when something the assistant waited for has ended it says so, and gives the turn to start.
pub trait PendingWaker: Send {
    /// Something ended and the assistant has not been told (cheap: read every frame).
    fn ready(&self) -> bool;
    /// The turn to start now (call only when no run is in progress).
    fn take(&self) -> Option<Wake>;
}


#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl SpeechKit for Nothing {}

    #[test]
    fn a_speech_kit_with_no_methods_plays_nothing_and_keeps_the_text() {
        let k = Nothing;
        assert!(k.recording_sink().play(&[]).is_err(), "no sound output without a real kit");
        let p = k.revise("hello", &crate::revise::Kind::FixGrammar, &Default::default());
        assert_eq!(p.text, "hello", "the text is kept when nothing revises it");
    }

    #[test]
    fn the_wake_turns_compare() {
        assert_eq!(Wake::Turn { text: "a".into(), ids: vec![] }, Wake::Turn { text: "a".into(), ids: vec![] });
        assert_ne!(Wake::Limit { text: "a".into() }, Wake::Turn { text: "a".into(), ids: vec![] });
    }
}
