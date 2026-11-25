//! The real port of the VOICE tile: the shell's context as the tile's [`VoicePort`] (the tile is the crate `iyakki-tile-voice`, alpha.11 AP1).
//! This is the only place that reads the session and calls the services for the Voice screen; a fake in the crate hands the tile in-memory
//! answers.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::pending::PendingWaker;
use iyakki_agent_api::{DirectVoice, MineWorker, SpeechKit, VoiceDeskFacet};
use iyakki_shared::dictfix;
use iyakki_shared::voice::{SpeechEngine, VoiceSettings};
use iyakki_shared::voicedesk::Recorders;
use iyakki_tile_voice::port::{Fixer, VoicePort};

use super::AppCx;

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    /// The rig of this person's session (the microphone is the browser's in a browser session, the machine's otherwise).
    pub fn rig(&self) -> iyakki_tile_voice::Rig {
        super::voice::rig_for_user(self.0.core.clone(), &self.0.session.user)
    }
}

impl VoicePort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn voice_allowed(&self) -> bool {
        self.0.session.policy.voice
    }
    fn manage_settings(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn allows_write(&self, path: &str) -> bool {
        self.0.session.policy.allows_write(&self.0.session.user, path)
    }
    fn record(&self, action: &str, detail: &str) {
        self.0.audit.record(&self.0.session.user, action, detail);
    }
    fn voice_settings(&self) -> VoiceSettings {
        self.0.voice_settings()
    }
    fn update_voice(&self, f: &mut dyn FnMut(&mut VoiceSettings)) -> Result<(), String> {
        self.0.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| f(&mut s.voice)).map_err(|e| e.to_string())
    }
    fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        self.0.set_voice_mic(id)
    }
    fn voicedesk(&self) -> Arc<dyn VoiceDeskFacet> {
        self.0.voicedesk_facet()
    }
    fn direct_voice(&self, engine: &SpeechEngine) -> Option<Arc<dyn DirectVoice>> {
        super::direct_voice(self.0.agent.as_ref(), engine)
    }
    fn player_command(&self, configured: &str, path: &str) -> Option<String> {
        self.0.agent.hostengines_player_command(configured, path)
    }
    fn proc_root(&self) -> PathBuf {
        self.0.core.host().proc_root().to_path_buf()
    }
    fn dict_target(&self) -> dictfix::Target {
        self.0.agent.dict_fix().target(&self.0.services)
    }
    fn dict_set_enabled(&self, on: bool) -> Result<(), String> {
        self.0.agent.dict_fix().set_enabled(&self.0.services, on)
    }
    fn dict_fixer(&self) -> Fixer {
        let (svc, agent) = (self.0.services.clone(), self.0.agent.clone());
        Arc::new(move |raw, ctx| agent.dict_fix().fix_wording(&svc, raw, ctx))
    }
    fn speech_kit(&self) -> Arc<dyn SpeechKit> {
        self.0.services.speech_kit()
    }
    fn allow_speech_server(&self, url: &str) -> Result<(), String> {
        self.0.services.allow_speech_server(url)
    }
    fn home_root(&self) -> PathBuf {
        self.0.core.file_scope().home_root(&self.0.session.user, self.0.data_dir())
    }
    fn file_roots(&self) -> iyakki_filescope::FileRoots {
        self.0.core.file_scope().roots(&self.0.session.policy, &self.0.session.user, self.0.data_dir())
    }
    fn check_read(&self, path: &std::path::Path) -> Result<(), String> {
        self.0.core.file_scope().check_read(&self.0.session.policy, &self.0.session.user, path)
    }
    fn take_session_request(&self) -> bool {
        self.0.agent.wakelisten_take_session_request()
    }
    fn wake_mark(&self, kind: &str) -> Result<(), String> {
        self.0.services.wake_mark(kind)
    }
    fn wake_stats_line(&self) -> String {
        self.0.services.wake_stats_line()
    }
    fn cloud_stream_sync(&self) {
        self.0.services.cloud_stream_sync();
    }
    fn cloud_stream_press(&self, yes: bool) {
        self.0.services.cloud_stream_press(yes);
    }
    fn cloud_stream_card(&self) -> Option<String> {
        self.0.services.cloud_stream_card()
    }
    fn pending_waker(&self) -> Option<Box<dyn PendingWaker>> {
        self.0.pendingapi().pending_waker(&self.0.session.user, "", None)
    }
    fn spawn_assistant(&self, recorders: Arc<dyn Recorders>) -> Option<Box<dyn MineWorker>> {
        let audit = self.0.audit.clone();
        let audit_line: Arc<dyn Fn(&str, &str, &str) + Send + Sync> = Arc::new(move |u, a, d| audit.record(u, a, d));
        self.0.b4().voice_assistant_spawn(&self.0.session.user, self.0.session.policy.clone(), self.0.host.clone(), self.0.mcp_servers.clone(), self.0.chat.clone(), audit_line, recorders)
    }
    fn tile_recording(&self) -> Arc<dyn Fn(bool) + Send + Sync> {
        Arc::new(iyakki_shared::wakeshare::set_tile_recording)
    }
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader> {
        crate::new_reader()
    }
    fn speaks_directly(&self, engine: &SpeechEngine) -> bool {
        super::speaks_directly(engine)
    }
    fn give_saved(&self, path: &std::path::Path) {
        crate::camera::own::give_saved(path)
    }
    fn stamp(&self) -> String {
        crate::camera::stamp()
    }
    fn no_mic_text(&self, product: &str, t2_hidden: bool) -> String {
        iyakki_shared::voicedesk::no_mic_text_t2(product, t2_hidden)
    }
    fn record_cmd_for(&self, base: &str, mic: Option<&iyakki_shared::voicedesk::Mic>) -> String {
        iyakki_shared::voicedesk::record_cmd_for(base, mic)
    }
    fn reword_storage_error(&self, msg: &str) -> String {
        iyakki_shared::voicedesk::reword_storage_error(msg)
    }
}
