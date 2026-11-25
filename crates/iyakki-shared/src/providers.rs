//! Types moved out of `iyakki_agent::providers` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Every NVIDIA key starts with this.
pub const NVIDIA_KEY_PREFIX: &str = "nvapi-";

pub use iyakki_provider_kind::{ImageParams, ImageTrace, Kind, LEMONADE_DEFAULT_BASE, NVIDIA_BASE, NVIDIA_STEPS};

/// What the person reads when a model only thought and wrote no answer (its thinking is never shown as the answer).
pub const THINKING_ONLY_MESSAGE: &str = "The model only thought and wrote no answer. Try again, lower or turn off the thinking budget in Settings -> Models, or pick another model.";


use serde::{Deserialize, Serialize};


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
pub use iyakki_voice_model::valid_model_name;

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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestReport {
    pub latency_ms: u64,
    pub models: Vec<String>,
}
