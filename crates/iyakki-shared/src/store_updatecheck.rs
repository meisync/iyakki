//! The periodic update check, the data part (F-UPD-04): the mode and settings, the answer of the last check and the rule that says when the
//! next check is due. The check itself (it asks the store sources and the system update index) stays in `iyakki_agent::store::updatecheck`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "update-check.json";
pub const DAY: i64 = 86_400;
/// How often to look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// About once a day (a stable jitter of up to an hour is added).
    #[default]
    Daily,
    /// Only when the person presses Check now.
    Manual,
    /// Never, not even from the page.
    Off,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Daily => Mode::Manual,
            Mode::Manual => Mode::Off,
            Mode::Off => Mode::Daily,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Daily => "every day",
            Mode::Manual => "only when I press Check now",
            Mode::Off => "off",
        }
    }
}

/// The choices of Settings -> System update, kept in the settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckSettings {
    pub mode: Mode,
    /// Where the system update index is (an administrator sets it; empty = no system check).
    pub system_url: String,
    /// Look on a metered connection too.
    pub metered_ok: bool,
}

/// A usable index address: https, or http for a mirror inside a network; no spaces, no user name or password in it.
pub fn check_url(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(String::new());
    }
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).ok_or("the address starts with https:// (or http:// for a mirror inside your network)")?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains('@') || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("that is not an address (no spaces, no user name or password in it)".into());
    }
    Ok(s.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    System,
    App,
}

/// One thing that can be updated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub kind: Kind,
    /// The package kind and name, or `system`.
    pub id: String,
    pub title: String,
    pub from: String,
    pub to: String,
    /// The catalog or index it was found in.
    pub source: String,
    /// Permissions the new version asks for and the installed one did not (plain words).
    pub new_permissions: Vec<String>,
    pub note: String,
    /// A driver pack (counted apart in the grouped notice).
    #[serde(default)]
    pub driver: bool,
}

impl Update {
    /// An app update with new permissions must be read before it is applied; the check never applies anything.
    pub fn needs_review(&self) -> bool {
        !self.new_permissions.is_empty()
    }

    /// The oldest IYAKKI this update works with, when the running IYAKKI is older (F-CORE-26: the badge text "needs a newer IYAKKI"). The
    /// note starts with [`NEEDS_NEWER`] for such an update.
    pub fn needs_newer_iyakki(&self) -> Option<&str> {
        self.note.strip_prefix(NEEDS_NEWER)?.split(' ').next().filter(|v| !v.is_empty())
    }
}

/// How the note of an update that needs a newer IYAKKI begins: `Needs IYAKKI 0.3.0 or newer first: ...`.
pub const NEEDS_NEWER: &str = "Needs IYAKKI ";

/// The answer of the last check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub checked_at: i64,
    pub updates: Vec<Update>,
    /// (source, what went wrong)
    pub errors: Vec<(String, String)>,
    /// Sources and indexes that were asked.
    pub asked: Vec<String>,
}

impl Report {
    pub fn count(&self) -> usize {
        self.updates.len()
    }
    /// The words for a badge or the top bar: "3 updates", "1 update", or nothing.
    pub fn badge(&self) -> Option<String> {
        match self.count() {
            0 => None,
            1 => Some("1 update".into()),
            n => Some(format!("{n} updates")),
        }
    }
    pub fn load(dir: &Path) -> Option<Report> {
        serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let tmp: PathBuf = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
    }
}

/// What `due` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Look now.
    Now,
    /// Seconds until the next look.
    Later(i64),
    /// The person chose Manual: only the button looks.
    Manual,
    Off,
    /// A metered connection and the person did not allow it.
    Metered,
}

/// A stable delay of 0 to 3599 seconds for this device and this day (FNV-1a of the device key and the day number).
pub fn jitter_secs(device: &str, day: i64) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in device.bytes().chain(day.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % 3600) as i64
}

/// Is a check due? `metered` is `None` when the machine cannot tell.
pub fn due(s: &CheckSettings, last: i64, now: i64, metered: Option<bool>, device: &str) -> Due {
    match s.mode {
        Mode::Off => return Due::Off,
        Mode::Manual => return Due::Manual,
        Mode::Daily => {}
    }
    if metered == Some(true) && !s.metered_ok {
        return Due::Metered;
    }
    if last <= 0 {
        return Due::Now;
    }
    let next = last + DAY + jitter_secs(device, last / DAY);
    if now >= next { Due::Now } else { Due::Later(next - now) }
}

