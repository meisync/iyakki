//! Read facets of alpha.6 split G (the model and speech members of `Services`): the speech engines of this computer, the vendor
//! session ids, the model providers (test, one chat), the picture models. The real implementation is
//! `iyakki_agent::facets_g`; the apps hold these through `AppCx::host_tts()` and `AppCx::ai()`, never through `Services`.

use std::sync::Arc;

use iyakki_shared::hostengines::{Env, Report};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::providers::TestReport;
use iyakki_shared::storyillus::Cand;
use iyakki_shared::vault::VaultError;
use iyakki_shared::voice::SpeechEngine;
use iyakki_shared::voicehelper::HelperState;

/// The speech engines of this computer (detection, the browser voice helper). Implemented for `Arc<iyakki_agent::hostengines::HostTts>`.
pub trait HostTtsApi: Send + Sync {
    fn is_enabled(&self) -> bool;
    /// Detect in this environment from now on (tests, programs that know where the engines are).
    fn configure(&self, env: Env);
    fn set_opener(&self, opener: Option<Vec<String>>);
    /// Start the detection in the background if it has not run. Cheap to call every frame.
    fn ensure(&self);
    fn recheck(&self);
    fn report(&self) -> Option<Arc<Report>>;
    fn scanning(&self) -> bool;
    fn scan_now(&self) -> Arc<Report>;
    fn helper_open(&self, hv: Arc<HostVoice>, opener: Option<&[String]>) -> Result<String, String>;
    fn helper_state(&self) -> Option<HelperState>;
    fn helper_stop(&self);
    fn auto_engine(&self, lang: Option<&str>) -> Option<SpeechEngine>;
}

/// The model side of the services: providers, picture models, vendor sessions and vendor sessions.
pub trait AiRead: Send + Sync {
    fn host_tts(&self) -> Arc<dyn HostTtsApi>;
    /// The vendor session id kept for this chat and agent.
    fn ext_session(&self, chat: &str, agent: &str) -> Option<String>;
    /// Keep the vendor session id for this chat and agent.
    fn ext_session_set(&self, chat: &str, agent: &str, id: &str);
    /// Count one model call of this person in the usage log.
    fn usage_record(&self, user: &str, provider: &str, model: &str, input: u64, output: u64);
    /// Can a client for this provider be made (is it configured)?
    fn provider_check(&self, provider: &str) -> Result<(), String>;
    /// Test the connection of a provider; a Lemonade server's typed list is refreshed too. Blocking.
    fn provider_test(&self, provider: &str) -> Result<TestReport, String>;
    /// One chat turn with a provider's model (the slow client). Blocking.
    fn provider_chat(&self, provider: &str, model: &str, history: &[iyakki_shared::chat::Message]) -> Result<String, String>;
    /// The models that draw pictures, as (provider, model).
    fn image_models(&self) -> Vec<(String, String)>;
    fn picture_choices(&self, found: &[(String, String)]) -> Vec<Cand>;
    /// The models of a provider this person's key cannot use (NVIDIA).
    fn unavailable_models(&self, provider: &str) -> Vec<String>;
}

/// The vaults of the services: the system vault, each person's vault and the secret requests the assistant made. Values never pass
/// through here except `user_get` (a zeroized string). Implemented for `iyakki_agent::services::Services`.
pub trait VaultRead: Send + Sync {
    fn system_names(&self) -> Result<Vec<String>, VaultError>;
    fn system_set(&self, name: &str, value: &str) -> Result<(), VaultError>;
    fn system_delete(&self, name: &str) -> Result<(), VaultError>;
    fn user_names(&self, user: &str) -> Result<Vec<String>, VaultError>;
    fn user_set(&self, user: &str, name: &str, value: &str) -> Result<(), VaultError>;
    fn user_get(&self, user: &str, name: &str) -> Result<zeroize::Zeroizing<String>, VaultError>;
    fn user_delete(&self, user: &str, name: &str) -> Result<(), VaultError>;
    fn user_vault_unlocked(&self, user: &str) -> bool;
    /// An administrator reset of a person's vault (they lose what was in it).
    fn reset_user_vault(&self, user: &str) -> Result<(), String>;
    /// The person's answer to a secret request: the value goes into their vault under `name`, the waiting tool is told.
    fn answer_secret_request(&self, user: &str, id: &str, name: &str, value: &str) -> Result<String, String>;
    fn cancel_secret_request(&self, user: &str, id: &str) -> Result<(), String>;
}
