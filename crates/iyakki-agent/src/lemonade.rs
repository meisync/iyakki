//! AMD's Lemonade Server (lemonade-sdk, <https://github.com/lemonade-sdk/lemonade>): a local OpenAI-compatible server for
//! Ryzen AI machines (NPU, iGPU, CPU) that serves more than chat. Its model list says what every model does (`labels`), so
//! IYAKKI can put each one where it belongs: chat and vision models in the model picker, embeddings for RAG, image models
//! (`sd-cpp`: SD-Turbo, SDXL) for TALES pictures and `generate_image`, whisper (`whispercpp`) for speech-to-text and Kokoro
//! (`kokoro`) as a reading voice. Lemonade has no video models (no video endpoint exists), and this module says so.
//!
//! What it speaks (from the project's docs/api/openai.md and docs/api/lemonade.md, and src/cpp/server/server.cpp):
//!
//! * port 13305 since v10.1.0, 8000 before; every route answers under `/api/v1/` (and `/v1/`, `/api/v0/`, `/v0/`);
//! * `GET /api/v1/health`: `{"status":"ok","version":…,"model_loaded":…,"all_models_loaded":[{"model_name":…,"type":…}],
//!   "max_models":{"llm":1,…}}`: the last two fields tell Lemonade apart from other OpenAI-compatible servers;
//! * `GET /api/v1/models?show_all=true`: `data[]` with `id`, `recipe`, `checkpoint`, `size` (GB), `downloaded`,
//!   `suggested`, `labels` and, for image models, `image_defaults` (`width`, `height`, `steps`);
//! * `POST /api/v1/pull` / `load` / `unload` with `{"model_name":…}` (pull streams `event: progress` lines with `stream`);
//! * models load by themselves on first use; an API key is only needed when the server sets `LEMONADE_API_KEY`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iyakki_proxy::Upstream;
use serde_json::Value;

/// Lemonade's port since v10.1.0.
pub const DEFAULT_PORT: u16 = 13305;
/// Lemonade's port before v10.1.0.
pub const LEGACY_PORT: u16 = 8000;
/// Where the OpenAI-style API is, on the machine itself.
pub const DEFAULT_BASE: &str = "http://localhost:13305/api/v1";
/// What the screens say about video.
pub const NO_VIDEO: &str = "none: Lemonade has no video models";
/// Kokoro's voice when none is chosen (Lemonade lists no voices; this one is in its docs).
pub const DEFAULT_KOKORO_VOICE: &str = "af_sky";
/// Kokoro v1's usual voices (Lemonade has no voice list call; these come with the kokoro-v1 model). American and British
/// English, women (`af_`, `bf_`) and men (`am_`, `bm_`).
pub const KOKORO_VOICES: [&str; 12] = ["af_sky", "af_heart", "af_bella", "af_nicole", "af_sarah", "am_adam", "am_echo", "am_michael", "bf_emma", "bf_isabella", "bm_george", "bm_lewis"];

/// What a model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelType {
    Chat,
    Vision,
    Embeddings,
    Reranking,
    Image,
    SpeechToText,
    TextToSpeech,
    /// Anything else Lemonade serves (music, 3D, classification): listed, not used.
    Other,
}

impl ModelType {
    /// The sections of Settings → Models, in order.
    pub const SECTIONS: [ModelType; 8] = [ModelType::Chat, ModelType::Vision, ModelType::Embeddings, ModelType::Reranking, ModelType::Image, ModelType::SpeechToText, ModelType::TextToSpeech, ModelType::Other];

    pub fn label(self) -> &'static str {
        match self {
            ModelType::Chat => "Chat",
            ModelType::Vision => "Vision",
            ModelType::Embeddings => "Embeddings",
            ModelType::Reranking => "Reranking",
            ModelType::Image => "Image",
            ModelType::SpeechToText => "Speech-to-text",
            ModelType::TextToSpeech => "Text-to-speech",
            ModelType::Other => "Other",
        }
    }

    /// The word saved in `ProviderCfg::model_kinds`.
    pub fn key(self) -> &'static str {
        match self {
            ModelType::Chat => "chat",
            ModelType::Vision => "vision",
            ModelType::Embeddings => "embeddings",
            ModelType::Reranking => "reranking",
            ModelType::Image => "image",
            ModelType::SpeechToText => "stt",
            ModelType::TextToSpeech => "tts",
            ModelType::Other => "other",
        }
    }

    pub fn from_key(k: &str) -> Option<ModelType> {
        Self::SECTIONS.into_iter().find(|t| t.key() == k)
    }

    /// Can answer the chat (a vision model can, and also sees pictures).
    pub fn chats(self) -> bool {
        matches!(self, ModelType::Chat | ModelType::Vision)
    }
}

/// An image model's own size and steps (`image_defaults`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageDefaults {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub steps: Option<u32>,
}

/// One model of a Lemonade server.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub id: String,
    pub kind: ModelType,
    pub recipe: String,
    pub checkpoint: String,
    pub labels: Vec<String>,
    /// On the server's disk (it can be used); otherwise it has to be pulled first.
    pub downloaded: bool,
    /// In memory now (the first request is fast).
    pub loaded: bool,
    pub suggested: bool,
    /// Download size in GB, when the list says.
    pub size_gb: Option<f64>,
    pub image_defaults: Option<ImageDefaults>,
}

impl Model {
    /// "loaded", "available" or "not downloaded".
    pub fn status(&self) -> &'static str {
        if self.loaded {
            "loaded"
        } else if self.downloaded {
            "available"
        } else {
            "not downloaded"
        }
    }

    /// "6.9 GB", or "size unknown".
    pub fn size_text(&self) -> String {
        match self.size_gb {
            Some(g) if g >= 1.0 => format!("{g:.1} GB"),
            Some(g) if g > 0.0 => format!("{} MB", (g * 1024.0).round() as u64),
            _ => "size unknown".into(),
        }
    }
}

/// What a model does, from its labels first (Lemonade's own deployment labels), then its recipe (the backend), then its name.
pub fn classify(recipe: &str, labels: &[String], id: &str) -> ModelType {
    let has = |l: &str| labels.iter().any(|x| x.eq_ignore_ascii_case(l));
    let r = recipe.to_ascii_lowercase();
    let n = id.to_ascii_lowercase();
    if has("transcription") || has("realtime-transcription") {
        ModelType::SpeechToText
    } else if has("tts") {
        ModelType::TextToSpeech
    } else if has("image") {
        ModelType::Image
    } else if has("embeddings") || has("embedding") {
        ModelType::Embeddings
    } else if has("reranking") {
        ModelType::Reranking
    } else if has("audio-generation") || has("3d") || has("classification") {
        ModelType::Other
    } else if r == "whispercpp" || r == "moonshine" {
        ModelType::SpeechToText
    } else if r == "kokoro" || r == "openmoss" {
        ModelType::TextToSpeech
    } else if r == "sd-cpp" {
        ModelType::Image
    } else if matches!(r.as_str(), "acestep" | "thenoise" | "thinksound" | "trellis") {
        ModelType::Other
    } else if has("vision") {
        ModelType::Vision
    } else if n.contains("whisper") {
        ModelType::SpeechToText
    } else if n.contains("kokoro") {
        ModelType::TextToSpeech
    } else if crate::models::is_image_model(&n) {
        ModelType::Image
    } else if n.contains("embed") {
        ModelType::Embeddings
    } else if n.contains("rerank") {
        ModelType::Reranking
    } else if crate::providers::model_sees_pictures(&n) {
        ModelType::Vision
    } else {
        ModelType::Chat
    }
}

fn u32_of(v: &Value, k: &str) -> Option<u32> {
    v.get(k).and_then(Value::as_u64).and_then(|x| u32::try_from(x).ok())
}

/// The models of a `GET /models` answer; `loaded` names the ones in memory (from the health answer).
pub fn parse_models(v: &Value, loaded: &[String]) -> Vec<Model> {
    let Some(list) = v.get("data").and_then(Value::as_array) else { return vec![] };
    let mut out: Vec<Model> = list
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            if !crate::providers::valid_model_name(&id) {
                return None;
            }
            let labels: Vec<String> = m.get("labels").and_then(Value::as_array).map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let recipe = m.get("recipe").and_then(Value::as_str).unwrap_or("").to_string();
            let checkpoint = match m.get("checkpoint") {
                Some(Value::String(s)) => s.clone(),
                _ => m.pointer("/checkpoints/main").and_then(Value::as_str).unwrap_or("").to_string(),
            };
            let image_defaults = m.get("image_defaults").filter(|d| d.is_object()).map(|d| ImageDefaults { width: u32_of(d, "width"), height: u32_of(d, "height"), steps: u32_of(d, "steps") });
            Some(Model {
                kind: classify(&recipe, &labels, &id),
                loaded: loaded.iter().any(|l| *l == id),
                // a server that does not say is listing what it has (the default list holds downloaded models only)
                downloaded: m.get("downloaded").and_then(Value::as_bool).unwrap_or(true),
                suggested: m.get("suggested").and_then(Value::as_bool).unwrap_or(false),
                size_gb: m.get("size").and_then(Value::as_f64).filter(|g| g.is_finite() && *g >= 0.0),
                image_defaults,
                recipe,
                checkpoint,
                labels,
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| (a.kind, !a.downloaded, a.id.to_ascii_lowercase()).cmp(&(b.kind, !b.downloaded, b.id.to_ascii_lowercase())));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// What a health answer says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub version: Option<String>,
    pub loaded: Vec<String>,
}

/// A Lemonade health answer, or `None` when the JSON is someone else's (`status` plus `all_models_loaded` or `max_models`).
pub fn parse_health(v: &Value) -> Option<Health> {
    v.get("status").and_then(Value::as_str)?;
    if !(v.get("all_models_loaded").is_some_and(Value::is_array) || v.get("max_models").is_some_and(Value::is_object) || v.get("model_loaded").is_some() && v.get("checkpoint_loaded").is_some()) {
        return None;
    }
    Some(Health { version: v.get("version").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() < 40).map(str::to_string), loaded: loaded_models(v) })
}

/// The models in memory, from a health answer.
pub fn loaded_models(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v.get("all_models_loaded").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("model_name").and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default();
    if let Some(m) = v.get("model_loaded").and_then(Value::as_str) {
        if !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out
}

/// The OpenAI-style base of a Lemonade address: `http://host:13305` becomes `http://host:13305/api/v1`; an address with a
/// path is kept as it is.
pub fn api_base(b: &str) -> String {
    let b = b.trim_end_matches('/');
    let path = b.split_once("://").map_or(b, |(_, r)| r).split_once('/').map_or("", |(_, p)| p);
    if path.is_empty() {
        format!("{b}/api/v1")
    } else if path == "api" {
        format!("{b}/v1")
    } else {
        b.to_string()
    }
}

/// `scheme://host:port` of an address (its path dropped).
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let host = rest.split('/').next()?;
    if host.is_empty() || !matches!(scheme, "http" | "https") {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

// ------------------------------------------------------------------ the typed list, remembered

fn cache() -> &'static Mutex<HashMap<String, Vec<Model>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<Model>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Keep the typed list of the server at `base` (screens draw from it without waiting on the network).
pub fn remember(base: &str, models: &[Model]) {
    if let Ok(mut c) = cache().lock() {
        c.insert(base.trim_end_matches('/').to_string(), models.to_vec());
    }
    let vision: Vec<String> = models.iter().filter(|m| m.kind == ModelType::Vision).map(|m| m.id.clone()).collect();
    crate::providers::note_vision_models(&vision);
}

/// The typed list last read from the server at `base`.
pub fn cached(base: &str) -> Option<Vec<Model>> {
    cache().lock().ok()?.get(base.trim_end_matches('/')).cloned()
}

/// `ProviderCfg::model_kinds` from a typed list.
pub fn kinds_map(models: &[Model]) -> std::collections::BTreeMap<String, String> {
    models.iter().map(|m| (m.id.clone(), m.kind.key().to_string())).collect()
}

/// A friendlier message for the usual reasons a Lemonade call fails.
pub fn explain_error(model: &str, e: &str) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("401") || l.contains("unauthorized") {
        format!("the Lemonade server asks for an API key (it was started with LEMONADE_API_KEY); set it with Key in Settings → Models ({e})")
    } else if l.contains("not found") || l.contains("not downloaded") || l.contains("404") || l.contains("not installed") {
        format!("`{model}` is not ready on the Lemonade server: pull it first in Settings → Models ({e})")
    } else {
        e.to_string()
    }
}

// ------------------------------------------------------------------ voices

/// The reading voice of a Lemonade text-to-speech model (`kokoro-v1`): its OpenAI-style speech endpoint, WAV out.
pub fn tts_engine(base: &str, model: &str, voice: Option<&str>, key_ref: Option<String>) -> crate::voice::SpeechEngine {
    crate::voice::SpeechEngine::Http { url: format!("{}/audio/speech", api_base(base)), model: model.into(), key_ref, voice: Some(voice.unwrap_or(DEFAULT_KOKORO_VOICE).to_string()), language: None }
}

/// Speech-to-text with a Lemonade whisper model (`Whisper-Base`).
pub fn stt_engine(base: &str, model: &str, key_ref: Option<String>) -> crate::voice::SpeechEngine {
    crate::voice::SpeechEngine::Http { url: format!("{}/audio/transcriptions", api_base(base)), model: model.into(), key_ref, voice: None, language: None }
}

/// The voices to offer for a Lemonade speech model (Kokoro's usual ones; Lemonade has no call that lists them).
pub fn voices_for(model: &str) -> Vec<&'static str> {
    if model.to_ascii_lowercase().contains("kokoro") { KOKORO_VOICES.to_vec() } else { vec!["alloy", "shimmer"] }
}

// ------------------------------------------------------------------ telling servers apart

/// What answers at an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Server {
    Ollama { base: String, version: Option<String> },
    Lemonade { base: String, version: Option<String> },
    /// Another OpenAI-compatible server (vLLM, LM Studio, llama.cpp, …), reached at `base` (ending in `/v1`).
    Compat { base: String },
}

impl Server {
    pub fn kind(&self) -> crate::providers::Kind {
        match self {
            Server::Ollama { .. } => crate::providers::Kind::Ollama,
            Server::Lemonade { .. } => crate::providers::Kind::Lemonade,
            Server::Compat { .. } => crate::providers::Kind::Compat,
        }
    }

    pub fn base(&self) -> &str {
        match self {
            Server::Ollama { base, .. } | Server::Lemonade { base, .. } | Server::Compat { base } => base,
        }
    }

    /// "Lemonade found at http://127.0.0.1:13305/api/v1 (version 2026.42.1)".
    pub fn found(&self) -> String {
        let v = |v: &Option<String>| v.as_ref().map_or(String::new(), |v| format!(" (version {v})"));
        match self {
            Server::Ollama { base, version } => format!("Ollama found at {base}{}", v(version)),
            Server::Lemonade { base, version } => format!("Lemonade found at {base}{}", v(version)),
            Server::Compat { base } => format!("An OpenAI-compatible server found at {base}"),
        }
    }
}

fn get_json(up: &dyn Upstream, url: &str, headers: &[(String, String)]) -> Option<Value> {
    let r = up.request("GET", url, headers, b"").ok()?;
    if !(200..300).contains(&r.status) {
        return None;
    }
    serde_json::from_slice(&r.body).ok()
}

/// Which server answers at `url` (any address of it: `http://host:13305`, `…/api/v1`, `http://host:11434`): Lemonade (its
/// health answer), Ollama (`/api/version` and `/api/tags`) or another OpenAI-compatible server (`/v1/models`). Only GET
/// requests are made.
pub fn detect(url: &str, up: &dyn Upstream, headers: &[(String, String)]) -> Result<Server, String> {
    let o = origin(url).ok_or("the address must look like http://host:port")?;
    let given = url.trim().trim_end_matches('/');
    // Lemonade first: it also answers some of Ollama's calls
    let mut lemonade_bases = vec![format!("{o}/api/v1")];
    if given != o && !lemonade_bases.iter().any(|b| b == given) {
        lemonade_bases.insert(0, given.to_string());
    }
    lemonade_bases.push(format!("{o}/v1"));
    for b in &lemonade_bases {
        if let Some(h) = get_json(up, &format!("{b}/health"), headers).as_ref().and_then(parse_health) {
            return Ok(Server::Lemonade { base: b.clone(), version: h.version });
        }
    }
    if let Some(v) = get_json(up, &format!("{o}/api/version"), headers) {
        if let Some(ver) = v.get("version").and_then(Value::as_str) {
            if get_json(up, &format!("{o}/api/tags"), headers).is_some_and(|t| t.get("models").is_some()) {
                return Ok(Server::Ollama { base: o, version: Some(ver.to_string()) });
            }
        }
    }
    let mut compat = vec![format!("{o}/v1")];
    if given != o && !compat.iter().any(|b| b == given) {
        compat.insert(0, given.to_string());
    }
    for b in compat {
        if get_json(up, &format!("{b}/models"), headers).is_some_and(|m| m.get("data").is_some_and(Value::is_array)) {
            return Ok(Server::Compat { base: b });
        }
    }
    Err(format!("no Ollama, Lemonade or OpenAI-compatible server answered at {o}"))
}

/// `GET path` on a port of this machine with short timeouts: (status, body), or `None` when nothing answers.
fn local_get(port: u16, path: &str) -> Option<(u16, String)> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\n\r\n").as_bytes()).ok()?;
    let mut text = String::new();
    s.take(64 * 1024).read_to_string(&mut text).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, body.to_string()))
}

/// A Lemonade server on a port of this machine: its API base and version. Quick: one connection, short timeouts.
pub fn probe_local(port: u16) -> Option<(String, Option<String>)> {
    let (status, body) = local_get(port, "/api/v1/health")?;
    if status != 200 {
        return None;
    }
    let h = parse_health(&serde_json::from_str(body.trim()).ok()?)?;
    Some((format!("http://127.0.0.1:{port}/api/v1"), h.version))
}

/// The ports Lemonade is looked for on this machine: today's, then the one of releases before v10.1.0.
pub fn local_ports() -> [u16; 2] {
    [DEFAULT_PORT, LEGACY_PORT]
}

fn ports_override() -> &'static Mutex<Option<(u16, Vec<u16>)>> {
    static P: OnceLock<Mutex<Option<(u16, Vec<u16>)>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// For tests of the screens: the (Ollama, Lemonade) ports Detect looks at instead of the real ones.
#[doc(hidden)]
pub fn set_local_ports_for_tests(p: Option<(u16, Vec<u16>)>) {
    if let Ok(mut o) = ports_override().lock() {
        *o = p;
    }
}

/// The ports set by [`set_local_ports_for_tests`], if any.
pub fn local_ports_override() -> Option<(u16, Vec<u16>)> {
    ports_override().lock().ok()?.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn models_are_typed_by_label_then_recipe_then_name() {
        let l = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(classify("llamacpp", &l(&["reasoning"]), "Qwen3-0.6B-GGUF"), ModelType::Chat);
        assert_eq!(classify("llamacpp", &l(&["vision"]), "Gemma-3-4b-it-GGUF"), ModelType::Vision);
        assert_eq!(classify("llamacpp", &l(&["embeddings"]), "nomic-embed-text-v1-GGUF"), ModelType::Embeddings);
        assert_eq!(classify("llamacpp", &l(&["reranking"]), "bge-reranker-v2-m3-GGUF"), ModelType::Reranking);
        assert_eq!(classify("sd-cpp", &l(&["image"]), "SDXL-Base-1.0"), ModelType::Image);
        assert_eq!(classify("sd-cpp", &[], "SD-Turbo"), ModelType::Image);
        assert_eq!(classify("whispercpp", &l(&["transcription", "realtime-transcription"]), "Whisper-Base"), ModelType::SpeechToText);
        assert_eq!(classify("kokoro", &l(&["tts"]), "kokoro-v1"), ModelType::TextToSpeech);
        assert_eq!(classify("acestep", &l(&["audio-generation"]), "ACE-Step"), ModelType::Other);
        assert_eq!(classify("", &[], "whisper-large"), ModelType::SpeechToText);
        for t in ModelType::SECTIONS {
            assert_eq!(ModelType::from_key(t.key()), Some(t));
        }
        assert!(ModelType::Vision.chats() && !ModelType::Image.chats());
    }

    #[test]
    fn the_model_list_and_the_health_answer_are_read() {
        let v = json!({"data": [
            {"id": "SDXL-Base-1.0", "recipe": "sd-cpp", "labels": ["image"], "size": 6.94, "downloaded": false, "suggested": true, "image_defaults": {"steps": 20, "cfg_scale": 7.5, "width": 1024, "height": 1024}},
            {"id": "Qwen3-0.6B-GGUF", "checkpoint": "unsloth/Qwen3-0.6B-GGUF:Q4_0", "recipe": "llamacpp", "size": 0.38, "downloaded": true, "labels": ["reasoning"]},
            {"id": "Whisper-Base", "checkpoints": {"main": "ggerganov/whisper.cpp:ggml-base.bin"}, "recipe": "whispercpp", "labels": ["transcription"]},
            {"id": "bad name with spaces", "recipe": "llamacpp"}
        ]});
        let h = json!({"status": "ok", "version": "2026.42.1", "model_loaded": "Qwen3-0.6B-GGUF", "all_models_loaded": [{"model_name": "Qwen3-0.6B-GGUF", "type": "llm"}], "max_models": {"llm": 1}});
        let health = parse_health(&h).unwrap();
        assert_eq!((health.version.as_deref(), health.loaded.clone()), (Some("2026.42.1"), vec!["Qwen3-0.6B-GGUF".to_string()]));
        let m = parse_models(&v, &health.loaded);
        assert_eq!(m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["Qwen3-0.6B-GGUF", "SDXL-Base-1.0", "Whisper-Base"]);
        assert_eq!((m[0].status(), m[1].status(), m[2].status()), ("loaded", "not downloaded", "available"));
        assert_eq!(m[1].image_defaults, Some(ImageDefaults { width: Some(1024), height: Some(1024), steps: Some(20) }));
        assert_eq!((m[1].size_text(), m[0].size_text(), m[2].size_text()), ("6.9 GB".to_string(), "389 MB".to_string(), "size unknown".to_string()));
        assert_eq!(m[2].checkpoint, "ggerganov/whisper.cpp:ggml-base.bin");
        // other servers' health answers are not Lemonade's
        assert!(parse_health(&json!({"status": "ok"})).is_none());
        assert!(parse_health(&json!({"version": "0.5.7"})).is_none());
    }

    #[test]
    fn addresses_get_the_api_path() {
        assert_eq!(api_base("http://localhost:13305"), "http://localhost:13305/api/v1");
        assert_eq!(api_base("http://h:8000/"), "http://h:8000/api/v1");
        assert_eq!(api_base("http://h:8000/api"), "http://h:8000/api/v1");
        assert_eq!(api_base("http://h:8000/api/v1/"), "http://h:8000/api/v1");
        assert_eq!(api_base("http://h:13305/v1"), "http://h:13305/v1");
        assert_eq!(origin("http://h:13305/api/v1").as_deref(), Some("http://h:13305"));
        assert!(origin("ftp://h").is_none());
        assert_eq!(tts_engine("http://h:13305", "kokoro-v1", None, None), crate::voice::SpeechEngine::Http { url: "http://h:13305/api/v1/audio/speech".into(), model: "kokoro-v1".into(), key_ref: None, voice: Some("af_sky".into()), language: None });
        assert!(voices_for("kokoro-v1").contains(&"af_heart"));
        assert!(explain_error("SDXL-Base-1.0", "HTTP 404: model not found").contains("pull it first"));
        assert!(explain_error("x", "HTTP 401: Unauthorized").contains("API key"));
    }
}

#[cfg(test)]
#[path = "lemonade_tests.rs"]
mod lemonade_tests;
