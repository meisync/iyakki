//! Types moved out of `iyakki_agent::providers` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Every NVIDIA key starts with this.
pub const NVIDIA_KEY_PREFIX: &str = "nvapi-";

/// How to get an NVIDIA NIM key, in the words the Settings page shows.
pub const NVIDIA_STEPS: &str = "NVIDIA NIM gives free access to many models (Llama, Qwen, DeepSeek, Mistral and more). To get a key: 1) open build.nvidia.com in a browser and sign in with a free NVIDIA account; 2) open any model and press Get API Key (or Generate Key); 3) copy the key, it starts with nvapi-; 4) paste it here. No card is needed. Limits: about 40 requests a minute, and the free tier may log requests to improve models, so do not send secrets.";

/// What the person reads when a model only thought and wrote no answer (its thinking is never shown as the answer).
pub const THINKING_ONLY_MESSAGE: &str = "The model only thought and wrote no answer. Try again, lower or turn off the thinking budget in Settings -> Models, or pick another model.";

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

/// What one picture request sent and got back, for the picture log. Nothing secret is in it.
#[derive(Clone, Debug, Default)]
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

use serde::{Deserialize, Serialize};

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
    /// speak, a typed model list, and pull/load calls (`crate::lemonade`).
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCfg {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Secret reference for the API key (e.g. `vault:openai-key`); none for keyless servers.
    #[serde(default)]
    pub key_ref: Option<String>,
    /// Models seen the last time the list was fetched.
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// What each model is, for servers that say so (Lemonade): model id -> `chat`, `vision`, `embeddings`, `reranking`,
    /// `image`, `stt`, `tts`. Saved the last time the typed list was read; `models` keeps only the chat models.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub model_kinds: std::collections::BTreeMap<String, String>,
    /// The models of this provider write PDF files by themselves, so the assistant does not offer `pdf_create` (docs/PDF.md).
    /// Additive; off unless set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_pdf_output: bool,
}

fn yes() -> bool {
    true
}

pub fn valid_provider_id(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase()) && s.len() <= 32 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '-' || x == '_')
}

/// Model ids appear in URLs (Gemini) and request bodies; keep them to a safe alphabet.
pub fn valid_model_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-' | '/' | '@' | '+'))
}

impl ProviderCfg {
    pub fn new(id: &str, kind: Kind) -> ProviderCfg {
        ProviderCfg { id: id.into(), kind, base_url: kind.default_base().map(str::to_string), key_ref: None, models: vec![], default_model: None, enabled: true, model_kinds: Default::default(), native_pdf_output: false }
    }

    pub fn base(&self) -> Result<String, String> {
        let b = self.base_url.clone().or_else(|| self.kind.default_base().map(str::to_string)).ok_or("this provider needs a base URL")?;
        iyakki_sec::url_host(&b).ok_or("base URL must be http(s)://host[:port]/…")?;
        Ok(b.trim_end_matches('/').to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_provider_id(&self.id) {
            return Err("provider id: lowercase letters, digits, - and _; must start with a letter".into());
        }
        self.base()?;
        if let Some(m) = &self.default_model {
            if !valid_model_name(m) {
                return Err("invalid model name".into());
            }
        }
        if self.kind.needs_key() && self.key_ref.is_none() {
            return Err(format!("{} needs an API key reference (e.g. vault:{}-key)", self.kind.label(), self.id));
        }
        if let Some(r) = &self.key_ref {
            if !["vault:", "env:", "file:", "ext:"].iter().any(|p| r.starts_with(p)) {
                return Err("key reference must start with vault:, env:, file: or ext:".into());
            }
        }
        Ok(())
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// Whether a model can be shown pictures, judged by its name (providers do not say so in a way every server repeats). A flow's agent
/// step refuses to send a screenshot to a model this says no to.
pub fn model_sees_pictures(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    if vision_models().lock().is_ok_and(|v| v.contains(&m)) {
        return true;
    }
    ["gpt-4o", "gpt-4.1", "gpt-5", "o4-mini", "claude-3", "claude-sonnet", "claude-opus", "claude-haiku", "sonnet", "opus", "haiku", "gemini", "llava", "vision", "-vl", "vl:", "qwen2.5vl", "qwen3-vl", "minicpm-v", "pixtral", "gemma3", "gemma-3", "llama4", "mistral-small3", "granite3.2-vision", "moondream", "bakllava"].iter().any(|k| m.contains(k))
}

pub fn vision_models() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static V: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    V.get_or_init(Default::default)
}

/// Models a server said can see pictures (Lemonade's `vision` label): [`model_sees_pictures`] says yes to them whatever
/// their name.
pub fn note_vision_models(names: &[String]) {
    if let Ok(mut v) = vision_models().lock() {
        v.extend(names.iter().map(|n| n.to_ascii_lowercase()));
    }
}

/// The answer of a provider connection test: how long it took and the models it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub latency_ms: u64,
    pub models: Vec<String>,
}
