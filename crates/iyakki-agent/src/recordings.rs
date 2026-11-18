//! Voice recordings: the WAV files of the VOICE tile's Record mode, kept in the person's own `Recordings` folder. Pure file
//! logic (name, save, list, rename, delete) so it is tested without a screen; the sound itself is captured and played through
//! the shared entry points (`voicedesk::Recorders`, `audiosink`), never opened a second way here.

use crate::mixer::SourceKind;
use crate::story::{Playing, Sink};
use std::sync::Arc;
pub use iyakki_shared::recordings::*;
#[cfg(test)]
use std::path::{Path, PathBuf};

/// A sound on the speaker through the shared audio entry point (the mixer, or the play command of Settings -> Voice).
struct OnSpeaker(crate::audiosink::Output);

impl Playing for OnSpeaker {
    fn is_playing(&mut self) -> bool {
        self.0.is_playing()
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

/// Plays WAV bytes with `Services::play_wav_output` (the one way sound reaches the speaker).
pub struct ServicesSink(pub Arc<crate::services::Services>);

impl Sink for ServicesSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
        Ok(Box::new(OnSpeaker(self.0.play_wav_output(SourceKind::Media, "Voice recording", wav)?)))
    }
}

#[cfg(test)]
#[path = "recordings_tests.rs"]
mod tests;
