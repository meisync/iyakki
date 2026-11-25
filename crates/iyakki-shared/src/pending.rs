//! Types moved out of `iyakki_agent::pending` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

/// The longest a single registered wait may last (six hours, like Autopilot's cap).
pub const MAX_WAIT_SECS: u64 = 6 * 3600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Wake the chat when work ends (off: the note waits for the person's next message, the old behaviour).
    pub auto_continue: bool,
    /// Wakes per day (the chain limit).
    pub max_wakes_per_day: u32,
    /// The longest any wait lasts, in seconds.
    pub max_wait_secs: u64,
    /// A wake opens a follow-up thread instead of continuing the current one.
    pub separate_thread: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { auto_continue: true, max_wakes_per_day: 20, max_wait_secs: MAX_WAIT_SECS, separate_thread: false }
    }
}

pub use iyakki_voice_model::facets::Wake;
