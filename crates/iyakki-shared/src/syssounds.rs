//! Types moved out of `iyakki_agent::syssounds` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The longest sign-out waits for its cue to end.
pub use iyakki_core_api::soundtypes::LOGOUT_WAIT;

/// The offset range of Settings -> System sounds: never louder than IYAKKI's volume, at most 30 dB quieter.
pub const MIN_OFFSET_DB: i32 = -30;
pub const MAX_OFFSET_DB: i32 = 0;
pub const DEFAULT_OFFSET_DB: i32 = -12;

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use iyakki_core_api::soundtypes::Cue;
use serde::Deserialize;
use serde::Serialize;

/// Settings -> System sounds (kept in the settings file; old files without it load with these defaults).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemSounds {
    /// The master switch.
    pub enabled: bool,
    /// How much quieter than IYAKKI's volume the cues are, in dB (-30 to 0).
    pub offset_db: i32,
    pub login: bool,
    pub logout: bool,
    pub notify: bool,
    pub job_done: bool,
}

impl Default for SystemSounds {
    fn default() -> Self {
        SystemSounds { enabled: true, offset_db: DEFAULT_OFFSET_DB, login: true, logout: true, notify: true, job_done: true }
    }
}

impl SystemSounds {
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_OFFSET_DB..=MAX_OFFSET_DB).contains(&self.offset_db) {
            return Err(format!("the system sounds level must be between {MIN_OFFSET_DB} and {MAX_OFFSET_DB} dB"));
        }
        Ok(())
    }

    /// Whether this cue is switched on (the master switch and its own).
    pub fn allows(&self, c: Cue) -> bool {
        self.enabled && self.get(c)
    }

    pub fn get(&self, c: Cue) -> bool {
        match c {
            Cue::Login => self.login,
            Cue::Logout => self.logout,
            Cue::Notify => self.notify,
            Cue::JobDone => self.job_done,
        }
    }

    pub fn set(&mut self, c: Cue, on: bool) {
        match c {
            Cue::Login => self.login = on,
            Cue::Logout => self.logout = on,
            Cue::Notify => self.notify = on,
            Cue::JobDone => self.job_done = on,
        }
    }
}
