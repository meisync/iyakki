//! The real implementation of `iyakki_agent_api::facets_b7` (F-SPLIT-03, lane alpha.6/split-d).

use std::sync::Arc;

use iyakki_agent_api::{HelpersB7, ServicesB7};
use iyakki_shared::chat::Message;
use iyakki_shared::context::{ContextInfo, ContextOptions};
use iyakki_shared::modelhost::{Host, NewHost, TestReport};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::providers::ProviderCfg;
use iyakki_shared::story::{Sink, Synth, TtsStatus};
use iyakki_shared::storyvoice::QuickEngine;
use iyakki_shared::voice::{SpeakOpts, SpeechEngine, VoiceSettings};
use iyakki_shared::voicedesk::{Players, Recorders};

use crate::services::Services;

impl ServicesB7 for Services {
    fn chat_context(&self, user: &str, session_id: Option<&str>, base_system: &str, history: &[Message], user_msg: &str, opts: &ContextOptions) -> (Vec<Message>, ContextInfo) {
        crate::context::build(self, &self.skills, &self.sessions, user, session_id, base_system, history, user_msg, opts)
    }
    fn reading_synth(self: &Arc<Self>, opts: SpeakOpts) -> Arc<dyn Synth> {
        Arc::new(crate::storykit::ServicesSynth { svc: self.clone(), opts })
    }
    fn flows_panel(self: &Arc<Self>, user: &str, policy: &iyakki_sec::policy::Policy) -> Option<Arc<dyn iyakki_agent_api::FlowsPanel>> {
        Some(crate::flowspanel::open_panel(self, user, policy))
    }
    fn modelhost_add(&self, n: &NewHost, report: &TestReport) -> Result<Host, String> {
        crate::modelhost_svc::add_to_models(self, n, report)
    }
    fn modelhost_remove(&self, id: &str) -> Result<(), String> {
        crate::modelhost_svc::remove_from_models(self, id)
    }
}

/// The system microphones and speaker.
pub struct RealHelpersB7;

impl HelpersB7 for RealHelpersB7 {
    fn command_sink(&self, play_cmd: &str) -> Arc<dyn Sink> {
        Arc::new(crate::storykit::CommandSink { play_cmd: play_cmd.to_string() })
    }
    fn tts_status(&self, v: &VoiceSettings, voice_allowed: bool, path: &str, host: &HostVoice, auto: Option<&SpeechEngine>) -> TtsStatus {
        crate::story::tts_status_auto(v, voice_allowed, path, host, auto)
    }
    fn tts_status_text(&self, s: &TtsStatus) -> String {
        crate::story::status_text(s)
    }
    fn quick_engines(&self, providers: &[ProviderCfg], path: &str, voice_dirs: &[std::path::PathBuf]) -> Vec<QuickEngine> {
        iyakki_shared::story::quick_engines(providers, path, voice_dirs)
    }
    fn system_recorders(&self) -> Option<Arc<dyn Recorders>> {
        Some(Arc::new(crate::voicedesk::SystemRecorders))
    }
    fn system_players(&self) -> Option<Arc<dyn Players>> {
        Some(Arc::new(crate::voicedesk::SystemPlayers))
    }
}

impl iyakki_agent_api::AgentFacetsB7 for crate::AgentApiImpl {
    fn helpers_b7(&self) -> &dyn HelpersB7 {
        &RealHelpersB7
    }
}
