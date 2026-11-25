//! What the VOICE tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile names no `Services`, no session, no audit log and no agent handle: it asks a [`VoicePort`] for the signed-in person, the voice
//! rights, the voice settings (one place to change them), the reading and dictation facet, the wording fix, the revision kit, the wake
//! phrase (experimental, off by default) and My IYAKKI helpers, the person's folders and the audit line. The real port is in the shell
//! (`crates/iyakki-apps/src/apps/voice_port.rs`), the only place that reads the session and calls the services; a fake hands the tile
//! in-memory answers (`fake_tests.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_story_model::DirectVoice;
use iyakki_voice_model::dictfix;
use iyakki_voice_model::facets::{MineWorker, PendingWaker, SpeechKit, VoiceDeskFacet};
use iyakki_voice_model::voice::{SpeechEngine, VoiceSettings};
use iyakki_voice_model::voicedesk::Recorders;

/// Corrects one dictated chunk (blocks: call it from a worker thread).
pub type Fixer = Arc<dyn Fn(&str, &dictfix::Context) -> dictfix::Fixed + Send + Sync>;

pub trait VoicePort {
    /// The signed-in person.
    fn user(&self) -> String;
    /// The person's rights include Voice.
    fn voice_allowed(&self) -> bool;
    /// The person manages the system settings (turns My IYAKKI on and off).
    fn manage_settings(&self) -> bool;
    /// The person may write in this path.
    fn allows_write(&self, path: &str) -> bool;
    /// A line in the audit log for the signed-in person.
    fn record(&self, action: &str, detail: &str);

    /// A copy of the voice settings.
    fn voice_settings(&self) -> VoiceSettings;
    /// Change the voice settings and save them.
    fn update_voice(&self, f: &mut dyn FnMut(&mut VoiceSettings)) -> Result<(), String>;
    /// Remember the one microphone for Voice and the Browser.
    fn set_voice_mic(&self, id: &str) -> Result<(), String>;

    /// The reading voices, the speech engines and the dictation worker.
    fn voicedesk(&self) -> Arc<dyn VoiceDeskFacet>;
    /// The engine that speaks by itself, when the voice has one.
    fn direct_voice(&self, engine: &SpeechEngine) -> Option<Arc<dyn DirectVoice>>;
    /// The command that plays a sound file, when the settings name none that works.
    fn player_command(&self, configured: &str, path: &str) -> Option<String>;
    /// Where the process table lives (decides whether a hidden microphone is explained).
    fn proc_root(&self) -> PathBuf;

    /// The model "Correct my wording" would use and whether it may be used now.
    fn dict_target(&self) -> dictfix::Target;
    /// Turn "Correct my wording" on or off.
    fn dict_set_enabled(&self, on: bool) -> Result<(), String>;
    /// The corrector a worker thread runs.
    fn dict_fixer(&self) -> Fixer;
    /// The revision kit of the assistant pane (the proposal worker and the player of saved recordings).
    fn speech_kit(&self) -> Arc<dyn SpeechKit>;
    /// The person said yes to sending speech to this server.
    fn allow_speech_server(&self, url: &str) -> Result<(), String>;

    /// The person's home folder and the folders the file picker may reach.
    fn home_root(&self) -> PathBuf;
    fn file_roots(&self) -> iyakki_filescope::FileRoots;
    fn check_read(&self, path: &std::path::Path) -> Result<(), String>;

    /// Wake phrase (experimental, off by default): a request to open a session, the person's counts, the cloud-stream card.
    fn take_session_request(&self) -> bool;
    fn wake_mark(&self, kind: &str) -> Result<(), String>;
    fn wake_stats_line(&self) -> String;
    fn cloud_stream_sync(&self);
    fn cloud_stream_press(&self, yes: bool);
    fn cloud_stream_card(&self) -> Option<String>;

    /// My IYAKKI: the pending-work waker and the session worker (None: this core does not serve one).
    fn pending_waker(&self) -> Option<Box<dyn PendingWaker>>;
    fn spawn_assistant(&self, recorders: Arc<dyn Recorders>) -> Option<Box<dyn MineWorker>>;

    /// The wake listener is told whether this tile records (it gives the microphone up while it does), as a function the tile keeps.
    fn tile_recording(&self) -> Arc<dyn Fn(bool) + Send + Sync>;
    /// A new story reader (it reads pieces aloud while the next ones are made).
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader>;
    /// Does this engine speak by itself (a sink that says the text, no sound file)?
    fn speaks_directly(&self, engine: &SpeechEngine) -> bool;
    /// A saved recording is handed to the person (its owner and mode).
    fn give_saved(&self, path: &std::path::Path);
    /// The time stamp in file names, now.
    fn stamp(&self) -> String;
    /// What to tell the person when no microphone was found (the machine's model decides the words), knowing whether a T2 card's microphones are off.
    fn no_mic_text(&self, product: &str, t2_hidden: bool) -> String;
    /// The record command aimed at one microphone.
    fn record_cmd_for(&self, base: &str, mic: Option<&iyakki_voice_model::voicedesk::Mic>) -> String;
    /// A storage error in words a person can act on.
    fn reword_storage_error(&self, msg: &str) -> String;
}
