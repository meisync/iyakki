//! Facets of alpha.6 lane split-d (F-SPLIT-03): what the chat and the model-host panel needed from the services, and the system
//! microphones and speaker the composition root installs in tests. The real implementation is `iyakki_agent::facets_b7`; the
//! defaults here are the fake (a core that serves none of them).

use std::sync::Arc;

use iyakki_shared::chat::Message;
use iyakki_shared::context::{ContextInfo, ContextOptions};
use iyakki_shared::modelhost::{Host, NewHost, TestReport};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::providers::ProviderCfg;
use iyakki_shared::story::{Sink, Synth, TtsStatus};
use iyakki_shared::storyvoice::QuickEngine;
use iyakki_shared::voice::{SpeakOpts, SpeechEngine, VoiceSettings};
use iyakki_shared::voicedesk::{Players, Recorders};

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B7: &str = "unavailable: this core does not serve this call";

/// What needs the process's services; implemented for `iyakki_agent::services::Services`.
pub trait ServicesB7 {
    /// The messages a chat turn sends to the model: the system prompt with the recalled memory and skills, the person's history
    /// condensed to the window, and the question (it may search the knowledge base and call an embedding service: run it off the UI thread).
    #[allow(clippy::too_many_arguments)]
    fn chat_context(&self, _user: &str, _session_id: Option<&str>, base_system: &str, history: &[Message], user_msg: &str, _opts: &ContextOptions) -> (Vec<Message>, ContextInfo) {
        let mut out = vec![Message::system(base_system)];
        out.extend(history.iter().cloned());
        out.push(Message::user(user_msg));
        (out, ContextInfo::default())
    }
    /// Register a tested model host in Models (provider `mac-<nickname>`, its key in the system vault) and keep it in the host list.
    fn modelhost_add(&self, _n: &NewHost, _report: &TestReport) -> Result<Host, String> {
        Err(UNAVAILABLE_B7.into())
    }
    /// The text-to-speech engine of Settings -> Voice with the voice and speed picked on the page, as the reader of a story, a chat answer or a lesson uses it.
    fn reading_synth(self: &Arc<Self>, _opts: SpeakOpts) -> Arc<dyn Synth> {
        Arc::new(NoSynth)
    }
    /// The FLOWS environment of the account (the one the engine runs in): the open one, or one built without a scheduler.
    fn flows_panel(self: &Arc<Self>, _user: &str, _policy: &iyakki_sec::policy::Policy) -> Option<Arc<dyn crate::flows_jobs::FlowsPanel>> {
        None
    }
    /// Take a model host out of Models and the list.
    fn modelhost_remove(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_B7.into())
    }
}

/// A voice that makes no sound (a core without speech).
pub struct NoSynth;
impl Synth for NoSynth {
    fn synth(&self, _text: &str) -> Result<Vec<u8>, String> {
        Err(UNAVAILABLE_B7.into())
    }
}

/// The system microphones and speaker of this machine (the real ones are in `iyakki_agent::voicedesk`).
pub trait HelpersB7: Send + Sync {
    /// The microphones and recordings of the system (arecord and the mixer).
    fn system_recorders(&self) -> Option<Arc<dyn Recorders>> {
        None
    }
    /// The sound sink that plays WAV bytes through the play command of Settings -> Voice, streamed.
    fn command_sink(&self, _play_cmd: &str) -> Arc<dyn Sink> {
        Arc::new(crate::NoSink)
    }
    /// What reading aloud needs now: the engine, the front (screen) and the player, in words (`auto` is the engine this computer found on its own).
    fn tts_status(&self, _v: &VoiceSettings, _voice_allowed: bool, _path: &str, _host: &HostVoice, _auto: Option<&SpeechEngine>) -> TtsStatus {
        TtsStatus::NoEngine
    }
    /// A status as one plain sentence.
    fn tts_status_text(&self, _s: &TtsStatus) -> String {
        String::new()
    }
    /// The one-click speech set-ups the providers and the computer offer.
    fn quick_engines(&self, _providers: &[ProviderCfg], _path: &str, _voice_dirs: &[std::path::PathBuf]) -> Vec<QuickEngine> {
        Vec::new()
    }
    /// The speaker of the system (aplay and the mixer).
    fn system_players(&self) -> Option<Arc<dyn Players>> {
        None
    }
}

/// The default helpers.
pub struct NoB7;
impl HelpersB7 for NoB7 {}

/// The accessor (a supertrait of `AgentFacets`).
pub trait AgentFacetsB7: Send + Sync {
    fn helpers_b7(&self) -> &dyn HelpersB7 {
        &NoB7
    }
}
impl AgentFacetsB7 for crate::FakeAgentApi {}
