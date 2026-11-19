//! Types moved out of `iyakki_agent::camvision` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::Path;
use std::path::PathBuf;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Setting {
    pub on: bool,
}

impl Setting {
    pub fn path(data: &Path, user: &str) -> PathBuf {
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
        data.join(format!("camera-vision-{who}.json"))
    }

    pub fn load(data: &Path, user: &str) -> Setting {
        std::fs::read_to_string(Setting::path(data, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, data: &Path, user: &str) -> Result<(), String> {
        iyakki_schema::write_atomic(&Setting::path(data, user), serde_json::to_string(self).map_err(|e| e.to_string())?.as_bytes(), 0o600)
    }
}
