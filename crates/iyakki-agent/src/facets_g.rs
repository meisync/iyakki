//! The real implementation of `iyakki_agent_api::facets_g` (alpha.6 split G): the speech engines of this computer and the model side of the services.

use std::sync::Arc;

use iyakki_agent_api::{AiRead, HostTtsApi, VaultRead};
use iyakki_shared::vault::VaultError;
use iyakki_shared::hostengines::{Env, Report};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::providers::TestReport;
use iyakki_shared::storyillus::Cand;
use iyakki_shared::voice::SpeechEngine;
use iyakki_shared::voicehelper::HelperState;

use crate::hostengines::HostTts;
use crate::services::Services;

/// The speech engines of the services as the trait the apps hold.
pub struct HostTtsHandle(pub Arc<HostTts>);

impl HostTtsApi for HostTtsHandle {
    fn is_enabled(&self) -> bool {
        HostTts::is_enabled(&self.0)
    }
    fn configure(&self, env: Env) {
        HostTts::configure(&self.0, env)
    }
    fn set_opener(&self, opener: Option<Vec<String>>) {
        HostTts::set_opener(&self.0, opener)
    }
    fn ensure(&self) {
        HostTts::ensure(&self.0)
    }
    fn recheck(&self) {
        HostTts::recheck(&self.0)
    }
    fn report(&self) -> Option<Arc<Report>> {
        HostTts::report(&self.0)
    }
    fn scanning(&self) -> bool {
        HostTts::scanning(&self.0)
    }
    fn scan_now(&self) -> Arc<Report> {
        HostTts::scan_now(&self.0)
    }
    fn helper_open(&self, hv: Arc<HostVoice>, opener: Option<&[String]>) -> Result<String, String> {
        HostTts::helper_open(&self.0, hv, opener)
    }
    fn helper_state(&self) -> Option<HelperState> {
        HostTts::helper_state(&self.0)
    }
    fn helper_stop(&self) {
        HostTts::helper_stop(&self.0)
    }
    fn auto_engine(&self, lang: Option<&str>) -> Option<SpeechEngine> {
        HostTts::auto_engine(&self.0, lang)
    }
}

pub struct ServicesAi {
    svc: Arc<Services>,
}

impl Services {
    /// The model-side read facet over these services (`cx.ai()`).
    pub fn ai(self: &Arc<Self>) -> Arc<dyn AiRead> {
        Arc::new(ServicesAi { svc: self.clone() })
    }
}

impl AiRead for ServicesAi {
    fn host_tts(&self) -> Arc<dyn HostTtsApi> {
        Arc::new(HostTtsHandle(self.svc.host_tts.clone()))
    }
    fn ext_session(&self, chat: &str, agent: &str) -> Option<String> {
        self.svc.ext_sessions.get(chat, agent)
    }
    fn ext_session_set(&self, chat: &str, agent: &str, id: &str) {
        self.svc.ext_sessions.set(chat, agent, id)
    }
    fn usage_record(&self, user: &str, provider: &str, model: &str, input: u64, output: u64) {
        self.svc.usage.record(user, provider, model, input, output)
    }
    fn provider_check(&self, provider: &str) -> Result<(), String> {
        self.svc.client_for(provider, true).map(|_| ())
    }
    fn provider_test(&self, provider: &str) -> Result<TestReport, String> {
        let client = self.svc.client_for(provider, true)?;
        let report = client.test()?;
        if client.cfg.kind == crate::providers::Kind::Lemonade {
            let _ = self.svc.refresh_lemonade(provider);
        }
        Ok(report)
    }
    fn provider_chat(&self, provider: &str, model: &str, history: &[iyakki_shared::chat::Message]) -> Result<String, String> {
        self.svc.client_for(provider, false)?.chat(model, history)
    }
    fn image_models(&self) -> Vec<(String, String)> {
        self.svc.image_models()
    }
    fn picture_choices(&self, found: &[(String, String)]) -> Vec<Cand> {
        self.svc.picture_choices(found)
    }
    fn unavailable_models(&self, provider: &str) -> Vec<String> {
        self.svc.unavailable_models(provider)
    }
}

pub struct ServicesVault {
    svc: Arc<Services>,
}

impl Services {
    /// The vault read facet over these services (`cx.vault()`).
    pub fn vault_facet(self: &Arc<Self>) -> Arc<dyn VaultRead> {
        Arc::new(ServicesVault { svc: self.clone() })
    }
}

impl VaultRead for ServicesVault {
    fn system_names(&self) -> Result<Vec<String>, VaultError> {
        self.svc.system_names()
    }
    fn system_set(&self, name: &str, value: &str) -> Result<(), VaultError> {
        self.svc.system_set(name, value)
    }
    fn system_delete(&self, name: &str) -> Result<(), VaultError> {
        self.svc.system_delete(name)
    }
    fn user_names(&self, user: &str) -> Result<Vec<String>, VaultError> {
        self.svc.user_names(user)
    }
    fn user_set(&self, user: &str, name: &str, value: &str) -> Result<(), VaultError> {
        self.svc.user_set(user, name, value)
    }
    fn user_get(&self, user: &str, name: &str) -> Result<zeroize::Zeroizing<String>, VaultError> {
        self.svc.user_get(user, name)
    }
    fn user_delete(&self, user: &str, name: &str) -> Result<(), VaultError> {
        self.svc.user_delete(user, name)
    }
    fn user_vault_unlocked(&self, user: &str) -> bool {
        self.svc.user_vault_unlocked(user)
    }
    fn reset_user_vault(&self, user: &str) -> Result<(), String> {
        self.svc.reset_user_vault(user)
    }
    fn answer_secret_request(&self, user: &str, id: &str, name: &str, value: &str) -> Result<String, String> {
        self.svc.answer_secret_request(user, id, name, value)
    }
    fn cancel_secret_request(&self, user: &str, id: &str) -> Result<(), String> {
        self.svc.cancel_secret_request(user, id)
    }
}
