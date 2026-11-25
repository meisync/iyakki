//! The speech types and rules of voice (the engine and settings of Settings, Voice, the text to speech and speech to text helpers, the
//! host engines, the browser voice, WAV reading and writing), pure data and small functions: no agent, no shell, no network.
//! `iyakki_shared::{voice, hostvoice, hostengines, audioout}` re-export every module under the old paths (alpha.10 Q3, D-P6-03).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod dictfix;
pub mod pcm;
pub mod revise;
pub mod hostengines;
pub mod hostvoice;
pub mod voice;
pub mod hosttts;
pub mod voicedesk;
pub mod facets;
pub mod recordings;
pub mod voiceassist;
pub mod voiceassist_ports;
pub mod voicehelper;

/// The vault reference of the Gemini key.
pub const KEY_REF: &str = "vault:gemini-key";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Google's free tier: small limits; Google may use what is sent to improve its products.
    #[default]
    Free,
    /// The person's own paid key.
    Paid,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Paid => "paid",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim().to_ascii_lowercase().as_str() {
            "free" => Some(Tier::Free),
            "paid" => Some(Tier::Paid),
            _ => None,
        }
    }
}

/// A model name a provider or engine may carry: letters, digits and . _ : - / @ + only, no `..`, at most 128 bytes.
pub fn valid_model_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-' | '/' | '@' | '+'))
}

/// The program `prog` on a `:` separated search path (a name with a slash is taken as it is).
pub fn which(prog: &str, path: &str) -> Option<PathBuf> {
    if prog.contains('/') {
        return Path::new(prog).is_file().then(|| PathBuf::from(prog));
    }
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(prog)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_checked_and_tiers_parse() {
        assert!(valid_model_name("a/b:c-1.2@x+y") && !valid_model_name("") && !valid_model_name("a..b") && !valid_model_name("a b") && !valid_model_name(&"x".repeat(129)));
        assert_eq!((Tier::parse(" Paid "), Tier::parse("x"), Tier::Free.label()), (Some(Tier::Paid), None, "free"));
    }

    #[test]
    fn which_finds_a_file_on_the_path_or_by_its_own_path() {
        let d = std::env::temp_dir().join(format!("iyakki-which-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("tool"), b"x").unwrap();
        let path = format!("/nonexistent::{}", d.display());
        assert_eq!(which("tool", &path), Some(d.join("tool")));
        assert_eq!(which("nothing", &path), None);
        assert_eq!(which(&d.join("tool").display().to_string(), ""), Some(d.join("tool")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
