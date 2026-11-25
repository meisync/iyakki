//! The speech engines of this computer as the tile and the agent both see them (moved from `iyakki-agent-api`, which re-exports it).

use std::sync::Arc;

use crate::hostengines::{Env, Report};
use crate::hostvoice::HostVoice;
use crate::voice::SpeechEngine;
use crate::voicehelper::HelperState;

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

