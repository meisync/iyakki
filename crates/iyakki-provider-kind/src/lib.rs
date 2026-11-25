//! Provider kinds, the picture trace and the image defaults (leaf crate; see Cargo.toml).

pub mod lemonade;

use serde::{Deserialize, Serialize};

/// How to get an NVIDIA NIM key, in the words the Settings page shows.
pub const NVIDIA_STEPS: &str = "NVIDIA NIM gives free access to many models (Llama, Qwen, DeepSeek, Mistral and more). To get a key: 1) open build.nvidia.com in a browser and sign in with a free NVIDIA account; 2) open any model and press Get API Key (or Generate Key); 3) copy the key, it starts with nvapi-; 4) paste it here. No card is needed. Limits: about 40 requests a minute, and the free tier may log requests to improve models, so do not send secrets.";


/// The Lemonade server's default base URL (the agent's `lemonade::DEFAULT_BASE` is this).
pub const LEMONADE_DEFAULT_BASE: &str = "http://localhost:13305/api/v1";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Ollama,
    OpenAi,
    OpenRouter,
    /// NVIDIA NIM (build.nvidia.com): an OpenAI-compatible endpoint with a free tier; the key starts with `nvapi-`.
    Nvidia,
    Anthropic,
    Gemini,
    /// Any server speaking the OpenAI chat-completions API; `base_url` is required.
    Compat,
    /// AMD's Lemonade Server (lemonade-sdk): OpenAI-compatible under `/api/v1`, plus models that draw pictures, listen and
    /// speak, a typed model list, and pull/load calls (the Lemonade module of the shared crate).
    Lemonade,
}

/// NVIDIA NIM's OpenAI-compatible base URL.
pub const NVIDIA_BASE: &str = "https://integrate.api.nvidia.com/v1";

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Ollama, Kind::OpenAi, Kind::OpenRouter, Kind::Nvidia, Kind::Anthropic, Kind::Gemini, Kind::Compat, Kind::Lemonade];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ollama => "Ollama",
            Kind::OpenAi => "OpenAI",
            Kind::OpenRouter => "OpenRouter",
            Kind::Nvidia => "NVIDIA NIM (free key)",
            Kind::Anthropic => "Anthropic Claude",
            Kind::Gemini => "Google Gemini",
            Kind::Compat => "OpenAI-compatible",
            Kind::Lemonade => "Lemonade",
        }
    }

    pub fn default_base(self) -> Option<&'static str> {
        match self {
            Kind::Ollama => Some("http://localhost:11434"),
            Kind::OpenAi => Some("https://api.openai.com/v1"),
            Kind::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Kind::Nvidia => Some(NVIDIA_BASE),
            Kind::Anthropic => Some("https://api.anthropic.com/v1"),
            Kind::Gemini => Some("https://generativelanguage.googleapis.com/v1beta"),
            Kind::Compat => None,
            Kind::Lemonade => Some(LEMONADE_DEFAULT_BASE),
        }
    }

    /// How to get a key for this provider (what the Settings page shows next to the key field), where there is something to explain.
    pub fn key_help(self) -> Option<&'static str> {
        match self {
            Kind::Nvidia => Some(NVIDIA_STEPS),
            _ => None,
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, Kind::Ollama | Kind::Compat | Kind::Lemonade)
    }

    pub fn cycle(self) -> Kind {
        let i = Kind::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Kind::ALL[(i + 1) % Kind::ALL.len()]
    }
}

/// What one picture request sent and got back, for the picture log. Nothing secret is in it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImageTrace {
    pub provider: String,
    pub model: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub steps: Option<u32>,
    pub seed: Option<u64>,
    pub guidance: Option<f32>,
    /// The request headers with every secret replaced by `***`.
    pub request_headers: Vec<(String, String)>,
    pub status: Option<u16>,
    /// The server's own error text (secrets removed).
    pub error: String,
    pub ms: u64,
    /// The size of the whole answer.
    pub response_bytes: u64,
    /// The size of the picture inside the answer.
    pub image_bytes: u64,
    /// `png`, `jpg`, `webp`, `gif`, or `invalid` when the bytes were none of these; empty when no picture came.
    pub format: String,
    /// The server's health after the call (verbose only).
    pub health: Option<String>,
}

/// An image model's own size and steps (`image_defaults`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageDefaults {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub steps: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_label_and_cycling_visits_all_of_them() {
        let mut k = Kind::Ollama;
        let mut seen: Vec<Kind> = Vec::new();
        for _ in 0..Kind::ALL.len() {
            assert!(!k.label().is_empty());
            if !seen.contains(&k) {
                seen.push(k);
            }
            k = k.cycle();
        }
        assert_eq!(seen.len(), Kind::ALL.len());
        assert_eq!(k, Kind::Ollama);
    }

    #[test]
    fn keys_and_addresses_follow_the_kind() {
        assert!(!Kind::Ollama.needs_key() && !Kind::Compat.needs_key() && !Kind::Lemonade.needs_key());
        assert!(Kind::OpenAi.needs_key() && Kind::Nvidia.needs_key());
        assert_eq!(Kind::Compat.default_base(), None);
        assert_eq!(Kind::Lemonade.default_base(), Some(LEMONADE_DEFAULT_BASE));
        assert_eq!(Kind::Nvidia.default_base(), Some(NVIDIA_BASE));
        assert!(Kind::Nvidia.key_help().is_some() && Kind::OpenAi.key_help().is_none());
    }

    #[test]
    fn kinds_and_trace_keep_their_wire_shape() {
        assert_eq!(serde_json::to_string(&Kind::OpenRouter).unwrap(), "\"openrouter\"");
        let t = ImageTrace { provider: "p".into(), width: 512, ..Default::default() };
        let back: ImageTrace = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!((back.provider.as_str(), back.width, back.steps), ("p", 512, None));
        let d: ImageDefaults = serde_json::from_str(r#"{"width":512,"height":null,"steps":4}"#).unwrap();
        assert_eq!((d.width, d.height, d.steps), (Some(512), None, Some(4)));
    }
}

/// What a picture request may carry besides the prompt.
#[derive(Clone, Debug, Default)]
pub struct ImageParams {
    /// The seed (the same seed and prompt give the same picture on the same model).
    pub seed: Option<u64>,
    /// A negative prompt, sent as `negative_prompt`; only set when the server is known to take it.
    pub negative: Option<String>,
    /// Also read the server's health after the call (loaded models, backend address) for the picture log.
    pub verbose: bool,
    /// Steps and size to ask for instead of the model's own (the caller keeps them inside what the model supports).
    pub steps: Option<u32>,
    pub size: Option<(u32, u32)>,
    /// Guidance, sent as `cfg_scale`; `None` leaves the model's own.
    pub guidance: Option<f32>,
}

/// A model download started from the chat.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PullStatus {
    pub provider: String,
    pub model: String,
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

impl PullStatus {
    pub fn describe(&self) -> String {
        match (&self.error, self.done) {
            (Some(e), _) => format!("{}/{}: failed: {e}", self.provider, self.model),
            (None, true) => format!("{}/{}: downloaded", self.provider, self.model),
            _ if self.total > 0 => format!("{}/{}: {} ({}%, {} of {} MB)", self.provider, self.model, self.status, self.completed * 100 / self.total, self.completed >> 20, self.total >> 20),
            _ => format!("{}/{}: {}", self.provider, self.model, self.status),
        }
    }
}

