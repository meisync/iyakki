//! Who holds the sound card (moved out of `iyakki_agent::soundhold`, F-SPLIT-03); the agent keeps the registry.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::quiet::Env;
use crate::sounddoctor::Holder;

/// What an announced player says it holds right now (None: nothing).
pub type Holds = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Stops the player cleanly.
pub type StopFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    /// One of IYAKKI's own players, by the key it announced itself with.
    Own(String),
    /// A program outside IYAKKI that owns an open playback stream.
    Outside { pid: u32, name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub who: Who,
    /// "Radio is playing Radio X", "TALES is reading", "mpv is using the sound card".
    pub says: String,
}

/// The audit detail of a stop: what kind of holder and its name or key, nothing else.
pub fn audit_detail(found: &Found, answer: &str) -> String {
    match &found.who {
        Who::Own(k) => format!("own:{k} {answer}"),
        Who::Outside { name, .. } => format!("outside:{name} {answer}"),
    }
}

/// The dialog's question, for `wanter_name` ("the browser", "the radio").
pub fn question(found: &Found, wanter_name: &str) -> String {
    format!("{}. Stop it so {wanter_name} can play?", found.says)
}


/// The players that announced themselves; the process has one (`iyakki_agent::soundhold::global`), tests make their own.
pub trait HoldRegistry: Send + Sync {
    fn announce(&self, key: &str, holds: Holds, stop: StopFn);
    fn forget(&self, key: &str);
    fn holds(&self, key: &str) -> Option<String>;
    fn find(&self, wanter: &str, outside: Option<&Holder>, ours: &dyn Fn(u32) -> bool) -> Option<Found>;
    fn conflict_for_new_player(&self, wanter: &str, mixing: bool) -> Option<Found>;
    fn stop(&self, found: &Found, env: &dyn Env, asound: &Path, wait: Duration) -> Result<String, String>;
    /// [`HoldRegistry::stop`] with this machine's own environment (the real `amixer`, signals, `/proc/asound`).
    fn stop_here(&self, _found: &Found, _wait: Duration) -> Result<String, String> {
        Err("unavailable: this core does not serve this call".into())
    }
}
