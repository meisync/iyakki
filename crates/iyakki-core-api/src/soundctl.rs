//! The sound output of the machine as the UI sees it (F-SPLIT-03, sound cluster): which output is chosen and why, the person's sound
//! settings the core holds now, the notices of the output watcher, and the short system sounds (cues). Two narrow facets, each with a
//! "no sound" default, so a core without sound answers quietly: [`SoundControl`] (read the output, never change it: changes go through
//! the settings) and [`CuePlayer`] (play a cue).
//!
//! The real implementation is `iyakki_agent::corehandle::AgentCore` (over the process-wide output selector), the fake is
//! `iyakki_core_fake::FakeSound`.

use crate::soundtypes::{Cue, CueHandle, CueOut, Output, SoundSettings};
use std::path::Path;
use std::sync::Arc;

/// What the output selector sees now (a copy: nothing here changes the machine).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundState {
    /// The selector is installed (a real front end started it). `false`: the default device is left alone and the other fields are empty.
    pub active: bool,
    /// The output IYAKKI plays on.
    pub chosen: Option<Output>,
    /// Why that output (or none) was chosen.
    pub reason: String,
    /// The person chose it in Settings -> Sound (not automatic).
    pub by_person: bool,
    /// A sound server (PulseAudio, PipeWire) runs on this computer.
    pub sound_server: bool,
    /// Channels of the shared device behind the software volume, when there is one.
    pub mix_channels: Option<u32>,
    /// A sound card is still starting up: the sentence to show.
    pub starting: Option<String>,
    /// The card whose own hardware volume control is not used although it has one.
    pub hw_ignored: Option<u32>,
    /// The card is held by another program, so the volume control could not be made: the sentence to show.
    pub busy: Option<String>,
    /// The card whose software volume carries the volume.
    pub softvol_card: Option<u32>,
}

impl SoundState {
    /// Is there an output to change the volume of? (Not when the selector looked and found none.)
    pub fn output_present(&self) -> bool {
        !self.active || self.chosen.is_some() || self.sound_server
    }
}

/// Reading the sound output.
pub trait SoundControl: Send + Sync {
    /// The selector's picture now (no new look at the hardware).
    fn state(&self) -> SoundState {
        SoundState::default()
    }
    /// A new look at the hardware, then the line about a card that is still starting.
    fn refresh_starting(&self) -> Option<String> {
        None
    }
    /// The settings the selector holds now.
    fn prefs(&self) -> SoundSettings {
        SoundSettings::default()
    }
    /// The playback devices under `proc_root` (`/proc` on a real machine).
    fn outputs_at(&self, _proc_root: &Path) -> Vec<Output> {
        Vec::new()
    }
    /// A sound card that is still starting (the Mac's built-in speakers come slowly): its number.
    fn card_starting(&self, _proc_root: &Path, _sys: &Path) -> Option<u32> {
        None
    }
    /// Why the volume keys do nothing yet (no control moves the sound), if that is so.
    fn volume_note(&self) -> Option<String> {
        None
    }
    /// What the output watcher wants the person to read, taken (each line comes once).
    fn take_notices(&self) -> Vec<String> {
        Vec::new()
    }
    /// A sound server runs on this computer.
    fn server_running(&self) -> bool {
        false
    }
    /// `play_cmd` with the ALSA device of `device` put in.
    fn command_with_device(&self, play_cmd: &str, _device: &str) -> Result<String, String> {
        Ok(play_cmd.to_string())
    }
}

/// The default: a core with no sound.
pub struct NoSound;
impl SoundControl for NoSound {}

/// Playing the short system sounds (sign-in, sign-out, a notification, a finished job).
pub trait CuePlayer: Send + Sync {
    /// The real output for cues (the play command of Settings -> Voice through the chain of Settings -> Sound).
    fn out(&self) -> Arc<dyn CueOut>;
    /// Start `cue` now on `out` if the person's settings allow it; returns at once.
    fn play(&self, cue: Cue, out: Arc<dyn CueOut>) -> CueHandle;
}

/// The default: nothing is played.
pub struct NoCues;

struct Silent;
impl CueOut for Silent {
    fn ready(&self) -> Result<(), String> {
        Err("this core has no sound".into())
    }
    fn play(&self, _wav: &[u8]) -> Result<(), String> {
        Err("this core has no sound".into())
    }
}

impl CuePlayer for NoCues {
    fn out(&self) -> Arc<dyn CueOut> {
        Arc::new(Silent)
    }
    fn play(&self, _cue: Cue, _out: Arc<dyn CueOut>) -> CueHandle {
        CueHandle::finished(crate::soundtypes::Outcome::NoOutput("this core has no sound".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_core_without_sound_answers_quietly() {
        let s = NoSound;
        assert!(!s.state().active && s.state().output_present());
        assert!(s.outputs_at(Path::new("/proc")).is_empty() && s.take_notices().is_empty() && s.volume_note().is_none());
        assert_eq!(s.prefs(), SoundSettings::default());
        let c = NoCues;
        let h = c.play(Cue::Notify, c.out());
        assert!(matches!(h.wait_short(), Some(crate::soundtypes::Outcome::NoOutput(_))));
    }

    #[test]
    fn the_selector_is_present_unless_it_looked_and_found_nothing() {
        let mut st = SoundState { active: true, ..Default::default() };
        assert!(!st.output_present());
        st.sound_server = true;
        assert!(st.output_present());
    }
}
