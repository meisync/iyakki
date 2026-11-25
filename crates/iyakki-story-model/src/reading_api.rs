//! The reading traits of TALES that the tile and the agent both name: the story reader, the engine of this computer that speaks by itself,
//! and a picture accepted from a link (moved from `iyakki-agent-api`, which re-exports them; alpha.11 AP1, D-P6-03).

use std::sync::Arc;

use crate::*;

/// A speech engine of this computer that speaks by itself (espeak, say, ...): its text-to-sound stage is empty and its sink speaks.
pub trait DirectVoice: Send + Sync {
    /// The synth of this engine (it makes no sound file; the sink speaks the text).
    fn synth(&self) -> Arc<dyn Synth>;
    /// The sink that speaks; `voice` replaces the engine's own voice when given.
    fn sink(&self, voice: Option<&str>, lang: Option<&str>, speed: f32) -> Arc<dyn Sink>;
}

/// Reads pieces aloud: while piece N plays, the next ones are made. All clones of the `Arc` are the same reader.
pub trait StoryReader: Send + Sync {
    /// Start reading `chunks` from piece `from`; `open`: more pieces follow through `append` (the story is still being written).
    fn start(&self, _chunks: Vec<Chunk>, _from: usize, _open: bool, _voice_key: &str, _synth: Arc<dyn Synth>, _sink: Arc<dyn Sink>) {}
    fn append(&self, _more: Vec<Chunk>) {}
    fn close_input(&self) {}
    fn pause(&self) {}
    fn resume(&self) {}
    fn stop(&self) {}
    fn seek(&self, _chunk: usize) {}
    fn is_active(&self) -> bool {
        false
    }
    fn announce_holder(&self) {}
    fn status(&self) -> Status {
        Status::Idle
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot { status: Status::Idle, chunk: 0, unit: 0, range: (0, 0), chunks: 0, fraction: 0.0, ready: 0, word: None, word_exact: false, timing: Default::default() }
    }
    fn timeline_note(&self) -> String {
        String::new()
    }
    fn now_ms(&self) -> f32 {
        0.0
    }
    /// Replace the real clock (milliseconds): tests move time themselves.
    fn set_time_source(&self, _time: Arc<dyn Fn() -> f32 + Send + Sync>) {}
    fn set_words(&self, _on: bool) {}
    fn set_speed(&self, _speed: f32) {}
    fn level(&self) -> f32 {
        0.0
    }
    fn sounds(&self) -> (Vec<Chunk>, Vec<Option<Arc<Vec<u8>>>>) {
        (Vec::new(), Vec::new())
    }
}

/// A reader that reads nothing.
pub struct NoReader;
impl StoryReader for NoReader {}

/// A picture accepted from a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPicture {
    pub ext: String,
    pub bytes: Vec<u8>,
}

/// What came out: the file, its pages, and how many pictures went in or were left out (videos, undecodable files).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StoryPdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub pictures: usize,
    pub left_out: usize,
}

