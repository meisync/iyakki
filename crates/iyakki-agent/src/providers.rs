//! Model providers: Ollama, OpenAI, OpenRouter, Anthropic (Claude), Google Gemini, and any
//! OpenAI-compatible server (vLLM, LM Studio, llama.cpp, LiteLLM, …).
//!
//! API keys are never stored here: a provider holds a *secret reference* (`vault:…`, `env:…`,
//! `file:…`, `ext:…`) that is resolved at the moment of each request, so a locked vault or a rotated
//! key takes effect immediately and keys never sit in settings files or logs.

use std::sync::Arc;
use std::time::Instant;

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chat::{ChatBackend, Message, ModelTurn, Role, ToolCall, ToolSpec};
use crate::vault::{LocalVault, Secrets};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Ollama,
    OpenAi,
    OpenRouter,
    Anthropic,
    Gemini,
    /// Any server speaking the OpenAI chat-completions API; `base_url` is required.
    Compat,
    /// AMD's Lemonade Server (lemonade-sdk): OpenAI-compatible under `/api/v1`, plus models that draw pictures, listen and
    /// speak, a typed model list, and pull/load calls (`crate::lemonade`).
    Lemonade,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Ollama, Kind::OpenAi, Kind::OpenRouter, Kind::Anthropic, Kind::Gemini, Kind::Compat, Kind::Lemonade];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ollama => "Ollama",
            Kind::OpenAi => "OpenAI",
            Kind::OpenRouter => "OpenRouter",
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
            Kind::Anthropic => Some("https://api.anthropic.com/v1"),
            Kind::Gemini => Some("https://generativelanguage.googleapis.com/v1beta"),
            Kind::Compat => None,
            Kind::Lemonade => Some(crate::lemonade::DEFAULT_BASE),
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
        ProviderCfg { id: id.into(), kind, base_url: kind.default_base().map(str::to_string), key_ref: None, models: vec![], default_model: None, enabled: true, model_kinds: Default::default() }
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

/// Strip `<think>…</think>` reasoning blocks some local models put inline in `content`.
pub fn strip_think(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") {
            Some(j) => rest = &rest[i + j + 8..],
            None => return out.trim().to_string(),
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// How long Ollama keeps a model loaded after its last use.
pub const OLLAMA_KEEP_ALIVE: &str = "30m";

thread_local! {
    static REASONING: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

type ThinkSink = Box<dyn FnMut(&str)>;

thread_local! {
    static THINK_SINK: std::cell::RefCell<Option<ThinkSink>> = const { std::cell::RefCell::new(None) };
    static THINKING_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Where the model's thinking goes while its answer is read on this thread (the chat shows it as a "Thinking" block).
pub fn set_thinking_sink(sink: Option<ThinkSink>) {
    THINK_SINK.with(|s| *s.borrow_mut() = sink);
}

pub fn report_thinking(t: &str) {
    if !t.is_empty() {
        THINK_SINK.with(|s| {
            if let Some(f) = s.borrow_mut().as_mut() {
                f(t);
            }
        });
    }
}

/// Whether the last answer on this thread was only thinking (no answer text, no tool call): its thinking was returned as the
/// answer text, and the caller should say so.
pub fn thinking_only() -> bool {
    THINKING_ONLY.with(|f| f.get())
}

/// Models whose server said they cannot think: the thinking switch is not sent to them again.
fn no_think_models() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// An error that says the thinking switch was refused ("... does not support thinking").
fn refused_thinking(e: &str) -> bool {
    let l = e.to_lowercase();
    l.contains("400") && l.contains("think")
}

/// Ask Ollama models on this thread to skip (or limit) their thinking: `Some("none")`, `Some("low")`, ... A model that thinks
/// first spends seconds on it before the first word; for tool-using assistant turns that is rarely worth it. Models without a
/// thinking mode ignore it. `None` leaves the model's own default.
pub fn set_thread_reasoning(effort: Option<&'static str>) {
    REASONING.with(|r| r.set(effort));
}

/// One OpenAI-style `tool_calls` entry as a [`ToolCall`].
fn openai_call(i: usize, c: &Value) -> Option<ToolCall> {
    let f = c.get("function")?;
    let args = match f.get("arguments") {
        Some(Value::String(s)) if s.trim().is_empty() => json!({}),
        // a model that writes broken JSON: keep the raw text so the agent can report it
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_raw_arguments": s})),
        Some(o) => o.clone(),
        None => json!({}),
    };
    Some(ToolCall { id: c.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("call_{i}")), name: f.get("name")?.as_str()?.to_string(), args })
}

/// Takes `<think>...</think>` out of a stream of text pieces (a tag may be split between two pieces).
#[derive(Default)]
struct ThinkFilter {
    in_think: bool,
    hold: String,
}

fn partial_tag(s: &str, tag: &str) -> usize {
    (1..tag.len()).rev().find(|&k| s.as_bytes().ends_with(&tag.as_bytes()[..k])).unwrap_or(0)
}

impl ThinkFilter {
    /// (text outside the think tags, text inside them) for this piece.
    fn push(&mut self, piece: &str) -> (String, String) {
        self.hold.push_str(piece);
        let (mut out, mut thought) = (String::new(), String::new());
        loop {
            if self.in_think {
                match self.hold.find("</think>") {
                    Some(i) => {
                        thought.push_str(&self.hold[..i]);
                        self.hold.drain(..i + 8);
                        self.in_think = false;
                    }
                    None => {
                        let keep = partial_tag(&self.hold, "</think>");
                        let cut = self.hold.len() - keep;
                        thought.push_str(&self.hold[..cut]);
                        self.hold.drain(..cut);
                        return (out, thought);
                    }
                }
            } else {
                match self.hold.find("<think>") {
                    Some(i) => {
                        out.push_str(&self.hold[..i]);
                        self.hold.drain(..i + 7);
                        self.in_think = true;
                    }
                    None => {
                        let keep = partial_tag(&self.hold, "<think>");
                        let cut = self.hold.len() - keep;
                        out.push_str(&self.hold[..cut]);
                        self.hold.drain(..cut);
                        return (out, thought);
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> String {
        if self.in_think { String::new() } else { std::mem::take(&mut self.hold) }
    }
}

/// `image/png` or `image/jpeg` from the first bytes of a picture.
fn image_mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xFF, 0xD8]) { "image/jpeg" } else { "image/png" }
}

fn b64_of(b: &[u8]) -> String {
    iyakki_flow::report::base64(b)
}

/// Whether a model can be shown pictures, judged by its name (providers do not say so in a way every server repeats). A flow's agent
/// step refuses to send a screenshot to a model this says no to.
pub fn model_sees_pictures(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    if vision_models().lock().is_ok_and(|v| v.contains(&m)) {
        return true;
    }
    ["gpt-4o", "gpt-4.1", "gpt-5", "o4-mini", "claude-3", "claude-sonnet", "claude-opus", "claude-haiku", "sonnet", "opus", "haiku", "gemini", "llava", "vision", "-vl", "vl:", "qwen2.5vl", "qwen3-vl", "minicpm-v", "pixtral", "gemma3", "gemma-3", "llama4", "mistral-small3", "granite3.2-vision", "moondream", "bakllava"].iter().any(|k| m.contains(k))
}

fn vision_models() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
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

/// The request body of Anthropic's Messages API.
fn anthropic_body(model: &str, system: &[&str], turns: &[&Message], tools: &[ToolSpec], stream: bool) -> Value {
        let mut msgs: Vec<Value> = Vec::new();
        for m in turns {
            match m.role {
                Role::User if !m.images.is_empty() => {
                    let mut blocks: Vec<Value> = m.images.iter().map(|b| json!({"type": "image", "source": {"type": "base64", "media_type": image_mime(b), "data": b64_of(b)}})).collect();
                    blocks.push(json!({"type": "text", "text": m.content}));
                    msgs.push(json!({"role": "user", "content": blocks}));
                }
                Role::User => msgs.push(json!({"role": "user", "content": m.content})),
                Role::Assistant if m.tool_calls.is_empty() => msgs.push(json!({"role": "assistant", "content": m.content})),
                Role::Assistant => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(json!({"type": "text", "text": m.content}));
                    }
                    blocks.extend(m.tool_calls.iter().map(|c| json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.args})));
                    msgs.push(json!({"role": "assistant", "content": blocks}));
                }
                Role::Tool => {
                    let block = json!({"type": "tool_result", "tool_use_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content});
                    // consecutive tool results belong to one user turn
                    match msgs.last_mut() {
                        Some(last) if last["role"] == "user" && last["content"].as_array().is_some_and(|a| a.first().is_some_and(|b| b["type"] == "tool_result")) => {
                            last["content"].as_array_mut().expect("array").push(block)
                        }
                        _ => msgs.push(json!({"role": "user", "content": [block]})),
                    }
                }
                Role::System => {}
            }
        }
        let mut body = json!({"model": model, "max_tokens": 4096, "messages": msgs});
        if !system.is_empty() {
            body["system"] = json!(system.join("\n\n"));
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect::<Vec<_>>());
        }
    if stream {
        body["stream"] = json!(true);
    }
    body
}

fn snippet(body: &[u8]) -> String {
    let t: String = String::from_utf8_lossy(body).chars().filter(|c| !c.is_control() || *c == ' ').take(200).collect();
    t.trim().to_string()
}

/// Client for one configured provider.
pub struct ProviderClient {
    pub cfg: ProviderCfg,
    secrets: Secrets,
    upstream: Arc<dyn Upstream>,
}

impl ProviderClient {
    pub fn new(cfg: ProviderCfg, secrets: Secrets, upstream: Arc<dyn Upstream>) -> ProviderClient {
        ProviderClient { cfg, secrets, upstream }
    }

    fn key(&self) -> Result<Option<zeroize::Zeroizing<String>>, String> {
        match &self.cfg.key_ref {
            None => Ok(None),
            Some(r) => self.secrets.resolve(r, None::<&LocalVault>).map(Some).map_err(|e| format!("API key unavailable: {e}")),
        }
    }

    pub(crate) fn headers(&self) -> Result<Vec<(String, String)>, String> {
        let key = self.key()?;
        let mut h = vec![("Content-Type".to_string(), "application/json".to_string())];
        match (self.cfg.kind, key) {
            (Kind::Anthropic, Some(k)) => {
                h.push(("x-api-key".into(), k.to_string()));
                h.push(("anthropic-version".into(), "2023-06-01".into()));
            }
            (Kind::Anthropic, None) => return Err("API key unavailable".into()),
            (Kind::Gemini, Some(k)) => h.push(("x-goog-api-key".into(), k.to_string())),
            (Kind::Gemini, None) => return Err("API key unavailable".into()),
            (_, Some(k)) => h.push(("Authorization".into(), format!("Bearer {}", &*k))),
            (_, None) => {}
        }
        Ok(h)
    }

    /// Base of the OpenAI-style endpoints (Ollama serves them under /v1).
    fn openai_base(&self) -> Result<String, String> {
        let b = self.cfg.base()?;
        Ok(match self.cfg.kind {
            Kind::Ollama if !b.ends_with("/v1") => format!("{b}/v1"),
            Kind::Lemonade => crate::lemonade::api_base(&b),
            _ => b,
        })
    }

    fn call(&self, method: &str, url: &str, body: &[u8]) -> Result<Value, String> {
        let v = self.call_raw(method, url, body)?;
        note_usage(&v);
        Ok(v)
    }

    fn call_raw(&self, method: &str, url: &str, body: &[u8]) -> Result<Value, String> {
        let r = self.upstream.request(method, url, &self.headers()?, body)?;
        if !(200..300).contains(&r.status) {
            // surface the provider's own message ("Incorrect API key…") without echoing secrets
            let msg = serde_json::from_slice::<Value>(&r.body)
                .ok()
                .and_then(|v| v.pointer("/error/message").or_else(|| v.get("error").filter(|e| e.is_string())).and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| snippet(&r.body));
            return Err(format!("HTTP {}: {}", r.status, msg));
        }
        serde_json::from_slice(&r.body).map_err(|_| "provider returned something that is not JSON".to_string())
    }

    /// Draws a picture. Ollama is asked with its own generate call first and then with the OpenAI-style images call;
    /// OpenAI and compatible servers use the images call. Returns the file's bytes and its extension.
    pub fn generate_image(&self, model: &str, prompt: &str, size: &str) -> Result<(Vec<u8>, &'static str), String> {
        let (w, h) = size.split_once('x').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))).filter(|(a, b)| (64..=4096).contains(a) && (64..=4096).contains(b)).ok_or("the size must look like 1024x1024 (64 to 4096)")?;
        let mut why = String::new();
        if self.cfg.kind == Kind::Lemonade {
            return self.lemonade_image(model, prompt, w, h);
        }
        if self.cfg.kind == Kind::Ollama {
            let body = json!({"model": model, "prompt": prompt, "stream": false, "width": w, "height": h});
            match self.call("POST", &format!("{}/api/generate", self.cfg.base()?), body.to_string().as_bytes()) {
                Ok(v) => match crate::models::image_from_response(&v) {
                    Some(b) => return crate::models::sniff_image(b).ok_or_else(|| "the model answered, but not with a picture".to_string()),
                    None => why = "Ollama's generate call returned no picture".into(),
                },
                Err(e) => why = e,
            }
        }
        if matches!(self.cfg.kind, Kind::Ollama | Kind::OpenAi | Kind::Compat | Kind::OpenRouter) {
            let body = json!({"model": model, "prompt": prompt, "n": 1, "size": format!("{w}x{h}"), "response_format": "b64_json"});
            match self.call("POST", &format!("{}/images/generations", self.openai_base()?), body.to_string().as_bytes()) {
                Ok(v) => {
                    if let Some(b) = crate::models::image_from_response(&v) {
                        return crate::models::sniff_image(b).ok_or_else(|| "the model answered, but not with a picture".to_string());
                    }
                    if let Some(u) = v.pointer("/data/0/url").and_then(Value::as_str) {
                        let r = self.upstream.request("GET", u, &[], b"")?;
                        if (200..300).contains(&r.status) {
                            return crate::models::sniff_image(r.body).ok_or_else(|| "the address did not hold a picture".to_string());
                        }
                    }
                    return Err("the server answered without a picture".into());
                }
                Err(e) => {
                    return Err(if why.is_empty() { e } else { format!("{why}; then {e}") });
                }
            }
        }
        Err(format!("pictures from {} are not supported here yet", self.cfg.kind.label()))
    }

    /// Where and how to ask an Ollama (or Lemonade) server to download a model.
    pub fn pull_request(&self, model: &str) -> Result<(String, Vec<(String, String)>, Vec<u8>), String> {
        match self.cfg.kind {
            Kind::Ollama => Ok((format!("{}/api/pull", self.cfg.base()?), self.headers()?, json!({"model": model, "stream": true}).to_string().into_bytes())),
            Kind::Lemonade => Ok((format!("{}/pull", self.openai_base()?), self.headers()?, json!({"model_name": model, "stream": true}).to_string().into_bytes())),
            _ => Err("only Ollama and Lemonade servers can download models".into()),
        }
    }

    /// A Lemonade server's picture call: OpenAI's images API with the model's own size and steps when its list gives them
    /// (SD-Turbo draws at 512x512 in 4 steps, SDXL at 1024x1024 in 20). Lemonade loads the model by itself on the first call.
    fn lemonade_image(&self, model: &str, prompt: &str, w: u32, h: u32) -> Result<(Vec<u8>, &'static str), String> {
        if !valid_model_name(model) {
            return Err("invalid model name".into());
        }
        let base = self.openai_base()?;
        let defaults = crate::lemonade::cached(&base).and_then(|c| c.into_iter().find(|m| m.id == model)).and_then(|m| m.image_defaults);
        let (w, h, steps) = match defaults {
            Some(d) => (d.width.unwrap_or(w), d.height.unwrap_or(h), d.steps),
            None => (w, h, None),
        };
        let mut body = json!({"model": model, "prompt": prompt, "n": 1, "size": format!("{w}x{h}"), "response_format": "b64_json"});
        if let Some(s) = steps {
            body["steps"] = json!(s);
        }
        let v = self.call("POST", &format!("{base}/images/generations"), body.to_string().as_bytes()).map_err(|e| crate::lemonade::explain_error(model, &e))?;
        let b = crate::models::image_from_response(&v).ok_or("Lemonade answered without a picture")?;
        crate::models::sniff_image(b).ok_or_else(|| "Lemonade answered, but not with a PNG, JPEG, GIF or WebP picture".to_string())
    }

    /// Lemonade's typed model list (every model it knows, downloaded or not, with what it does and whether it is loaded). It
    /// is also kept in [`crate::lemonade::cached`] for screens that may not wait on the network.
    pub fn lemonade_catalog(&self) -> Result<Vec<crate::lemonade::Model>, String> {
        if self.cfg.kind != Kind::Lemonade {
            return Err("only a Lemonade server has a typed model list".into());
        }
        let base = self.openai_base()?;
        let v = self.call_raw("GET", &format!("{base}/models?show_all=true"), b"")?;
        let loaded = self.call_raw("GET", &format!("{base}/health"), b"").ok().map(|h| crate::lemonade::loaded_models(&h)).unwrap_or_default();
        let list = crate::lemonade::parse_models(&v, &loaded);
        crate::lemonade::remember(&base, &list);
        Ok(list)
    }

    /// Ask a Lemonade server to load (`load`) or unload (`unload`) a model; returns the server's message.
    pub fn lemonade_call(&self, what: &str, model: &str) -> Result<String, String> {
        if self.cfg.kind != Kind::Lemonade || !matches!(what, "load" | "unload") || !valid_model_name(model) {
            return Err("not a Lemonade load or unload".into());
        }
        let v = self.call_raw("POST", &format!("{}/{what}", self.openai_base()?), json!({"model_name": model}).to_string().as_bytes()).map_err(|e| crate::lemonade::explain_error(model, &e))?;
        if v.get("status").and_then(Value::as_str) == Some("error") {
            return Err(v.get("message").and_then(Value::as_str).unwrap_or("Lemonade refused").to_string());
        }
        Ok(v.get("message").and_then(Value::as_str).unwrap_or("done").to_string())
    }

    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = match self.cfg.kind {
            // the chat models only (the others are listed by type in Settings → Models and used where each type belongs)
            Kind::Lemonade => self.lemonade_catalog()?.into_iter().filter(|m| m.downloaded && m.kind.chats()).map(|m| m.id).collect(),
            Kind::Anthropic => {
                let v = self.call("GET", &format!("{}/models?limit=1000", self.cfg.base()?), b"")?;
                v.get("data").and_then(Value::as_array).ok_or("unexpected models response")?.iter().filter_map(|m| m.get("id")?.as_str().map(str::to_string)).collect()
            }
            Kind::Gemini => {
                let v = self.call("GET", &format!("{}/models?pageSize=1000", self.cfg.base()?), b"")?;
                v.get("models")
                    .and_then(Value::as_array)
                    .ok_or("unexpected models response")?
                    .iter()
                    .filter(|m| m.get("supportedGenerationMethods").and_then(Value::as_array).is_none_or(|a| a.iter().any(|x| x == "generateContent")))
                    .filter_map(|m| Some(m.get("name")?.as_str()?.trim_start_matches("models/").to_string()))
                    .collect()
            }
            _ => {
                let v = self.call("GET", &format!("{}/models", self.openai_base()?), b"")?;
                v.get("data").and_then(Value::as_array).ok_or("unexpected models response")?.iter().filter_map(|m| m.get("id")?.as_str().map(str::to_string)).collect()
            }
        };
        out.retain(|m| valid_model_name(m));
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn chat(&self, model: &str, history: &[Message]) -> Result<String, String> {
        let turn = self.chat_turn(model, history, &[])?;
        if turn.text.is_empty() { Err("the model returned no text".into()) } else { Ok(turn.text) }
    }

    /// One model turn, optionally offering tools. Messages and tools are converted to each
    /// provider's native tool-calling format and tool requests are parsed back out.
    pub fn chat_turn(&self, model: &str, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        let r = self.chat_turn_once(model, history, tools);
        if self.retry_without_thinking(model, &r) {
            return self.chat_turn_once(model, history, tools);
        }
        r
    }

    /// The server refused the thinking switch (the model cannot think): remember it and try again without.
    fn retry_without_thinking(&self, model: &str, r: &Result<ModelTurn, String>) -> bool {
        match r {
            Err(e) if self.cfg.kind == Kind::Ollama && REASONING.with(|x| x.get()).is_some() && refused_thinking(e) => {
                if let Ok(mut m) = no_think_models().lock() {
                    m.insert(model.to_string());
                }
                true
            }
            _ => false,
        }
    }

    fn chat_turn_once(&self, model: &str, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        forget_usage();
        THINKING_ONLY.with(|f| f.set(false));
        if !valid_model_name(model) {
            return Err("invalid model name".into());
        }
        let system: Vec<&str> = history.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect();
        let turns: Vec<&Message> = history.iter().filter(|m| m.role != Role::System).collect();
        match self.cfg.kind {
            Kind::Anthropic => {
                let body = anthropic_body(model, &system, &turns, tools, false);
                let v = self.call("POST", &format!("{}/messages", self.cfg.base()?), body.to_string().as_bytes())?;
                let blocks = v.get("content").and_then(Value::as_array).ok_or("unexpected response shape")?;
                let text: Vec<&str> = blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text")?.as_str()).collect();
                let calls: Vec<ToolCall> = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .filter_map(|b| Some(ToolCall { id: b.get("id")?.as_str()?.to_string(), name: b.get("name")?.as_str()?.to_string(), args: b.get("input").cloned().unwrap_or(json!({})) }))
                    .collect();
                if text.is_empty() && calls.is_empty() {
                    return Err("the model returned no text".into());
                }
                Ok(ModelTurn { text: text.join(""), calls })
            }
            Kind::Gemini => {
                let mut contents: Vec<Value> = Vec::new();
                for m in &turns {
                    match m.role {
                        Role::User if !m.images.is_empty() => {
                            let mut parts: Vec<Value> = vec![json!({"text": m.content})];
                            parts.extend(m.images.iter().map(|b| json!({"inline_data": {"mime_type": image_mime(b), "data": b64_of(b)}})));
                            contents.push(json!({"role": "user", "parts": parts}));
                        }
                        Role::User => contents.push(json!({"role": "user", "parts": [{"text": m.content}]})),
                        Role::Assistant => {
                            let mut parts: Vec<Value> = Vec::new();
                            if !m.content.is_empty() || m.tool_calls.is_empty() {
                                parts.push(json!({"text": m.content}));
                            }
                            parts.extend(m.tool_calls.iter().map(|c| json!({"functionCall": {"name": c.name, "args": c.args}})));
                            contents.push(json!({"role": "model", "parts": parts}));
                        }
                        Role::Tool => {
                            let part = json!({"functionResponse": {"name": m.tool_name.clone().unwrap_or_default(), "response": {"result": m.content}}});
                            match contents.last_mut() {
                                Some(last) if last["role"] == "user" && last["parts"].as_array().is_some_and(|a| a.first().is_some_and(|p| p.get("functionResponse").is_some())) => last["parts"].as_array_mut().expect("array").push(part),
                                _ => contents.push(json!({"role": "user", "parts": [part]})),
                            }
                        }
                        Role::System => {}
                    }
                }
                let mut body = json!({"contents": contents});
                if !system.is_empty() {
                    body["systemInstruction"] = json!({"parts": [{"text": system.join("\n\n")}]});
                }
                if !tools.is_empty() {
                    body["tools"] = json!([{"functionDeclarations": tools.iter().map(|t| json!({"name": t.name, "description": t.description, "parameters": gemini_schema(&t.schema)})).collect::<Vec<_>>()}]);
                }
                let v = self.call("POST", &format!("{}/models/{}:generateContent", self.cfg.base()?, model.trim_start_matches("models/")), body.to_string().as_bytes())?;
                if let Some(r) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
                    return Err(format!("blocked by the provider: {r}"));
                }
                let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array).ok_or("the model returned no content")?;
                let text: String = parts.iter().filter_map(|p| p.get("text")?.as_str()).collect::<Vec<_>>().join("");
                // Gemini has no call ids; synthesise stable ones so results can be matched
                let calls: Vec<ToolCall> = parts
                    .iter()
                    .filter_map(|p| p.get("functionCall"))
                    .enumerate()
                    .filter_map(|(i, c)| Some(ToolCall { id: format!("gemini_{i}_{}", c.get("name")?.as_str()?), name: c.get("name")?.as_str()?.to_string(), args: c.get("args").cloned().unwrap_or(json!({})) }))
                    .collect();
                Ok(ModelTurn { text, calls })
            }
            _ => {
                let body = self.openai_body(model, history, tools, false);
                let v = self.call("POST", &format!("{}/chat/completions", self.openai_base()?), body.to_string().as_bytes())?;
                let msg = v.pointer("/choices/0/message").ok_or("unexpected response shape (no choices[0].message)")?;
                let mut text = msg.get("content").and_then(Value::as_str).map(strip_think).unwrap_or_default();
                let calls: Vec<ToolCall> = msg
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().enumerate().filter_map(|(i, c)| openai_call(i, c)).collect())
                    .unwrap_or_default();
                let thought = ["reasoning", "reasoning_content", "thinking"].iter().find_map(|k| msg.get(*k).and_then(Value::as_str)).unwrap_or("").trim().to_string();
                if text.is_empty() && calls.is_empty() && !thought.is_empty() {
                    // some models put the whole answer in their thinking
                    text = thought;
                    THINKING_ONLY.with(|f| f.set(true));
                }
                if text.is_empty() && calls.is_empty() && msg.get("content").is_none() {
                    return Err("unexpected response shape (no choices[0].message.content)".into());
                }
                Ok(ModelTurn { text, calls })
            }
        }
    }

    /// The request body of an OpenAI-style chat call.
    fn openai_body(&self, model: &str, history: &[Message], tools: &[ToolSpec], stream: bool) -> Value {
        let msgs: Vec<Value> = history
            .iter()
            .map(|m| match m.role {
                Role::Assistant if !m.tool_calls.is_empty() => json!({
                    "role": "assistant",
                    "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                    "tool_calls": m.tool_calls.iter().map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}})).collect::<Vec<_>>(),
                }),
                Role::Tool => json!({"role": "tool", "tool_call_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content}),
                Role::User if !m.images.is_empty() => {
                    let mut parts: Vec<Value> = vec![json!({"type": "text", "text": m.content})];
                    parts.extend(m.images.iter().map(|b| json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", image_mime(b), b64_of(b))}})));
                    json!({"role": "user", "content": parts})
                }
                r => json!({"role": r.as_str(), "content": m.content}),
            })
            .collect();
        let mut body = json!({"model": model, "messages": msgs, "stream": stream});
        if stream {
            body["stream_options"] = json!({"include_usage": true});
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}})).collect::<Vec<_>>());
        }
        if self.cfg.kind == Kind::Ollama {
            // keep a local model in memory between turns (loading it again costs seconds)
            body["keep_alive"] = json!(OLLAMA_KEEP_ALIVE);
            if let Some(r) = REASONING.with(|r| r.get()) {
                if !no_think_models().lock().map(|m| m.contains(model)).unwrap_or(false) {
                    body["reasoning_effort"] = json!(r);
                }
            }
        }
        body
    }

    /// Load a local Ollama model into memory and keep it there, so the first question is not slow. Other kinds do nothing.
    pub fn warm_up(&self, model: &str) -> Result<u64, String> {
        if self.cfg.kind != Kind::Ollama || !valid_model_name(model) {
            return Ok(0);
        }
        let t = Instant::now();
        let url = format!("{}/api/generate", self.cfg.base()?);
        self.call_raw("POST", &url, json!({"model": model, "keep_alive": OLLAMA_KEEP_ALIVE, "stream": false}).to_string().as_bytes())?;
        Ok(t.elapsed().as_millis() as u64)
    }

    /// Like [`chat_turn`](ProviderClient::chat_turn) but the answer is read as the server writes it (server-sent events of
    /// OpenAI-style APIs, which includes Ollama's `/v1`). `on_text` gets each piece of visible text, and `""` while the model
    /// is busy without showing anything (thinking, writing a tool call); `false` from it stops the read. Other providers
    /// answer in one piece.
    pub fn chat_turn_stream(&self, model: &str, history: &[Message], tools: &[ToolSpec], streamer: &dyn iyakki_proxy::Streamer, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let r = self.chat_turn_stream_once(model, history, tools, streamer, on_text);
        if self.retry_without_thinking(model, &r) {
            return self.chat_turn_stream_once(model, history, tools, streamer, on_text);
        }
        r
    }

    fn chat_turn_stream_once(&self, model: &str, history: &[Message], tools: &[ToolSpec], streamer: &dyn iyakki_proxy::Streamer, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        use std::io::BufRead;
        THINKING_ONLY.with(|f| f.set(false));
        if self.cfg.kind == Kind::Gemini {
            return self.chat_turn(model, history, tools);
        }
        forget_usage();
        if !valid_model_name(model) {
            return Err("invalid model name".into());
        }
        if self.cfg.kind == Kind::Anthropic {
            return self.anthropic_stream(model, history, tools, streamer, on_text);
        }
        let body = self.openai_body(model, history, tools, true);
        let url = format!("{}/chat/completions", self.openai_base()?);
        let head = streamer.post(&url, &self.headers()?, body.to_string().as_bytes())?;
        if !(200..300).contains(&head.status) {
            let mut raw = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(head.body, 64 * 1024), &mut raw);
            let msg = serde_json::from_slice::<Value>(&raw)
                .ok()
                .and_then(|v| v.pointer("/error/message").or_else(|| v.get("error").filter(|e| e.is_string())).and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| snippet(&raw));
            return Err(format!("HTTP {}: {}", head.status, msg));
        }
        let (mut content, mut visible) = (String::new(), ThinkFilter::default());
        let mut thought = String::new();
        let mut calls: Vec<(String, String, String)> = Vec::new(); // id, name, arguments (by index)
        let mut stopped = false;
        let mut reader = std::io::BufReader::new(head.body);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    if content.is_empty() && calls.is_empty() {
                        return Err(format!("the model connection broke: {e}"));
                    }
                    break; // keep what arrived
                }
            }
            let l = line.trim();
            let Some(data) = l.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(m) = v.pointer("/error/message").and_then(Value::as_str) {
                return Err(format!("model error: {m}"));
            }
            note_usage(&v);
            let Some(d) = v.pointer("/choices/0/delta") else { continue };
            let mut alive = false;
            if let Some(t) = d.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                content.push_str(t);
                let (shown, thought_inline) = visible.push(t);
                if !thought_inline.is_empty() {
                    report_thinking(&thought_inline);
                    thought.push_str(&thought_inline);
                }
                if !shown.is_empty() && !on_text(&shown) {
                    stopped = true;
                }
                alive = shown.is_empty();
            }
            // the model's thinking: Ollama's `reasoning`, DeepSeek/OpenAI-style `reasoning_content`, or `thinking`
            for k in ["reasoning", "reasoning_content", "thinking"] {
                if let Some(t) = d.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    report_thinking(t);
                    if thought.len() < 1 << 20 {
                        thought.push_str(t);
                    }
                    alive = true;
                }
            }
            if d.get("tool_calls").is_some() {
                alive = true;
            }
            if let Some(tc) = d.get("tool_calls").and_then(Value::as_array) {
                for c in tc {
                    let i = c.get("index").and_then(Value::as_u64).unwrap_or(calls.len().saturating_sub(1) as u64) as usize;
                    while calls.len() <= i {
                        calls.push((String::new(), String::new(), String::new()));
                    }
                    if let Some(id) = c.get("id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                        calls[i].0 = id.to_string();
                    }
                    if let Some(f) = c.get("function") {
                        if let Some(n) = f.get("name").and_then(Value::as_str) {
                            calls[i].1.push_str(n);
                        }
                        match f.get("arguments") {
                            Some(Value::String(a)) => calls[i].2.push_str(a),
                            Some(o @ Value::Object(_)) => calls[i].2 = o.to_string(),
                            _ => {}
                        }
                    }
                }
            }
            if alive && !on_text("") {
                stopped = true;
            }
            if stopped || content.len() > 4 << 20 {
                break;
            }
        }
        let tail = visible.finish();
        if !tail.is_empty() {
            on_text(&tail);
        }
        let calls: Vec<ToolCall> = calls
            .into_iter()
            .enumerate()
            .filter(|(_, c)| !c.1.is_empty())
            .map(|(i, (id, name, args))| {
                let args = if args.trim().is_empty() { json!({}) } else { serde_json::from_str(&args).unwrap_or_else(|_| json!({"_raw_arguments": args})) };
                ToolCall { id: if id.is_empty() { format!("call_{i}") } else { id }, name, args }
            })
            .collect();
        let mut text = strip_think(&content);
        if text.is_empty() && calls.is_empty() && !thought.trim().is_empty() {
            // the whole answer is in the thinking (the model never wrote a reply): show it rather than nothing
            text = thought.trim().to_string();
            THINKING_ONLY.with(|f| f.set(true));
        }
        Ok(ModelTurn { text, calls })
    }

    /// Anthropic's Messages API with `stream: true` (server-sent events: text deltas and tool inputs arrive in pieces).
    fn anthropic_stream(&self, model: &str, history: &[Message], tools: &[ToolSpec], streamer: &dyn iyakki_proxy::Streamer, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        use std::io::BufRead;
        THINKING_ONLY.with(|f| f.set(false));
        let system: Vec<&str> = history.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect();
        let turns: Vec<&Message> = history.iter().filter(|m| m.role != Role::System).collect();
        let body = anthropic_body(model, &system, &turns, tools, true);
        let head = streamer.post(&format!("{}/messages", self.cfg.base()?), &self.headers()?, body.to_string().as_bytes())?;
        if !(200..300).contains(&head.status) {
            let mut raw = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(head.body, 64 * 1024), &mut raw);
            let msg = serde_json::from_slice::<Value>(&raw).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| snippet(&raw));
            return Err(format!("HTTP {}: {}", head.status, msg));
        }
        let mut text = String::new();
        let mut thought = String::new();
        // by block index: Some((id, name, partial json)) for a tool_use block
        let mut blocks: std::collections::BTreeMap<u64, (String, String, String)> = Default::default();
        let (mut tin, mut tout) = (0u64, 0u64);
        let mut reader = std::io::BufReader::new(head.body);
        let mut line = String::new();
        let mut stopped = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    if text.is_empty() && blocks.is_empty() {
                        return Err(format!("the model connection broke: {e}"));
                    }
                    break;
                }
            }
            let Some(data) = line.trim().strip_prefix("data:") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
            match v.get("type").and_then(Value::as_str) {
                Some("message_start") => tin = v.pointer("/message/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0),
                Some("message_delta") => tout = v.pointer("/usage/output_tokens").and_then(Value::as_u64).unwrap_or(tout),
                Some("content_block_start") => {
                    if v.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                        blocks.insert(idx, (v.pointer("/content_block/id").and_then(Value::as_str).unwrap_or("").into(), v.pointer("/content_block/name").and_then(Value::as_str).unwrap_or("").into(), String::new()));
                        stopped |= !on_text("");
                    }
                }
                Some("content_block_delta") => match v.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            text.push_str(t);
                            stopped |= !on_text(t);
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = v.pointer("/delta/thinking").and_then(Value::as_str) {
                            report_thinking(t);
                            if thought.len() < 1 << 20 {
                                thought.push_str(t);
                            }
                        }
                        stopped |= !on_text("");
                    }
                    Some("input_json_delta") => {
                        if let (Some(b), Some(p)) = (blocks.get_mut(&idx), v.pointer("/delta/partial_json").and_then(Value::as_str)) {
                            b.2.push_str(p);
                        }
                    }
                    _ => stopped |= !on_text(""),
                },
                Some("error") => return Err(format!("model error: {}", v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown"))),
                _ => {}
            }
            if stopped {
                break;
            }
        }
        note_usage(&json!({"usage": {"input_tokens": tin, "output_tokens": tout}}));
        let calls: Vec<ToolCall> = blocks
            .into_values()
            .filter(|(_, n, _)| !n.is_empty())
            .map(|(id, name, args)| ToolCall { id, name, args: if args.trim().is_empty() { json!({}) } else { serde_json::from_str(&args).unwrap_or_else(|_| json!({"_raw_arguments": args})) } })
            .collect();
        if text.is_empty() && calls.is_empty() {
            if thought.trim().is_empty() {
                return Err("the model returned no text".into());
            }
            text = thought.trim().to_string();
            THINKING_ONLY.with(|f| f.set(true));
        }
        Ok(ModelTurn { text, calls })
    }

    /// Embeddings (OpenAI-compatible servers, Ollama, OpenRouter, Gemini). Anthropic has no embeddings API.
    pub fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if !valid_model_name(model) {
            return Err("invalid model name".into());
        }
        match self.cfg.kind {
            Kind::Anthropic => Err("Anthropic does not offer an embeddings API; choose another embedding provider".into()),
            Kind::Gemini => {
                let m = model.trim_start_matches("models/");
                let reqs: Vec<Value> = texts.iter().map(|t| json!({"model": format!("models/{m}"), "content": {"parts": [{"text": t}]}})).collect();
                let v = self.call("POST", &format!("{}/models/{m}:batchEmbedContents", self.cfg.base()?), json!({"requests": reqs}).to_string().as_bytes())?;
                v.get("embeddings").and_then(Value::as_array).ok_or("unexpected embeddings response")?.iter().map(|e| floats(e.get("values"))).collect()
            }
            _ => {
                let v = self.call("POST", &format!("{}/embeddings", self.openai_base()?), json!({"model": model, "input": texts}).to_string().as_bytes())?;
                let data = v.get("data").and_then(Value::as_array).ok_or("unexpected embeddings response")?;
                if data.len() != texts.len() {
                    return Err("embeddings response has the wrong number of vectors".into());
                }
                data.iter().map(|e| floats(e.get("embedding"))).collect()
            }
        }
    }

    /// Reachability + credentials check that costs nothing: fetches the model list.
    pub fn test(&self) -> Result<TestReport, String> {
        let t = Instant::now();
        let models = self.list_models()?;
        Ok(TestReport { latency_ms: t.elapsed().as_millis() as u64, models })
    }
}

thread_local! {
    static REPORTED: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
}

/// What the provider said the last model call on this thread cost: (tokens in, tokens out). `None` when the provider
/// did not say (then callers estimate from the text). Cleared at the start of every model turn.
pub fn reported_usage() -> Option<(u64, u64)> {
    REPORTED.with(|r| r.get())
}

pub(crate) fn forget_usage() {
    REPORTED.with(|r| r.set(None));
}

/// Read the token counts out of a reply, whichever provider's shape it has.
pub fn usage_in(v: &Value) -> Option<(u64, u64)> {
    let n = |p: &str| v.pointer(p).and_then(Value::as_u64);
    let pairs = [
        ("/usage/prompt_tokens", "/usage/completion_tokens"),       // OpenAI and compatible servers, Ollama's compatible API
        ("/usage/input_tokens", "/usage/output_tokens"),             // Anthropic
        ("/usageMetadata/promptTokenCount", "/usageMetadata/candidatesTokenCount"), // Gemini
        ("/prompt_eval_count", "/eval_count"),                       // Ollama's own API
    ];
    pairs.iter().find_map(|(a, b)| match (n(a), n(b)) {
        (Some(i), Some(o)) => Some((i, o)),
        (Some(i), None) => Some((i, 0)),
        (None, Some(o)) => Some((0, o)),
        _ => None,
    })
}

pub(crate) fn note_usage(v: &Value) {
    if let Some(u) = usage_in(v) {
        REPORTED.with(|r| r.set(Some(u)));
    }
}

/// Gemini accepts an OpenAPI subset of JSON Schema: drop the keys it rejects.
fn gemini_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.iter().filter(|(k, _)| !matches!(k.as_str(), "additionalProperties" | "$schema")).map(|(k, x)| (k.clone(), gemini_schema(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(gemini_schema).collect()),
        other => other.clone(),
    }
}

fn floats(v: Option<&Value>) -> Result<Vec<f32>, String> {
    let a: Vec<f32> = v.and_then(Value::as_array).ok_or("missing embedding")?.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
    if a.is_empty() { Err("empty embedding".into()) } else { Ok(a) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub latency_ms: u64,
    pub models: Vec<String>,
}

/// Adapter so any configured provider+model plugs into the chat and RAG apps.
pub struct ProviderChat {
    pub client: ProviderClient,
    pub model: String,
    /// For answers read as they are written; without one the answer arrives in one piece.
    pub streamer: Option<Arc<dyn iyakki_proxy::Streamer>>,
}

impl ChatBackend for ProviderChat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.client.chat(&self.model, history)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.client.chat_turn(&self.model, history, tools)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        match &self.streamer {
            Some(s) => self.client.chat_turn_stream(&self.model, history, tools, s.as_ref(), on_text),
            None => self.client.chat_turn(&self.model, history, tools),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;
    use std::sync::Mutex;

    type Call = (String, String, Vec<(String, String)>, String);

    struct Mock {
        calls: Mutex<Vec<Call>>,
        reply: Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>,
    }
    impl Upstream for Mock {
        fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((m.into(), url.into(), h.to_vec(), String::from_utf8_lossy(b).into()));
            let (status, body) = (self.reply)(m, url);
            Ok(UpstreamResponse { status, body: body.into_bytes() })
        }
    }

    fn client(kind: Kind, base: Option<&str>, key: Option<&str>, reply: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> (ProviderClient, Arc<Mock>) {
        std::env::set_var("IYAKKI_TEST_PROVIDER_KEY", "KEY123");
        let mut cfg = ProviderCfg::new("p", kind);
        if let Some(b) = base {
            cfg.base_url = Some(b.into());
        }
        cfg.key_ref = key.map(|_| "env:IYAKKI_TEST_PROVIDER_KEY".to_string());
        let m = Arc::new(Mock { calls: Default::default(), reply: Box::new(reply) });
        (ProviderClient::new(cfg, Secrets::new(None, m.clone()), m.clone()), m)
    }

    fn hdr(c: &Call, k: &str) -> Option<String> {
        c.2.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone())
    }

    fn convo() -> Vec<Message> {
        vec![Message::system("be brief"), Message::user("hi"), Message::assistant("hello"), Message::user("2+2?")]
    }

    #[test]
    fn openai_style_chat_and_models() {
        let (c, m) = client(Kind::OpenAi, None, Some("k"), |meth, url| match (meth, url) {
            ("GET", u) if u.ends_with("/models") => (200, r#"{"data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#.into()),
            _ => (200, r#"{"choices":[{"message":{"content":"4"}}]}"#.into()),
        });
        assert_eq!(c.chat("gpt-4o-mini", &convo()).unwrap(), "4");
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!(call.1, "https://api.openai.com/v1/chat/completions");
        assert_eq!(hdr(&call, "authorization").as_deref(), Some("Bearer KEY123"));
        let body: Value = serde_json::from_str(&call.3).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"].as_array().unwrap().len(), 4);
        assert_eq!(c.list_models().unwrap(), vec!["gpt-4o", "gpt-4o-mini"], "sorted and de-duplicated");
    }

    #[test]
    fn a_picture_in_a_user_message_goes_out_in_each_providers_own_shape() {
        let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let b64 = iyakki_flow::report::base64(&png);
        let msgs = vec![Message::system("s"), Message::user_with_images("what is this?", vec![png.clone()])];
        // OpenAI-style: a content list with an image_url data address
        let (c, m) = client(Kind::OpenAi, None, Some("k"), |_, _| (200, r#"{"choices":[{"message":{"content":"a page"}}]}"#.into()));
        assert_eq!(c.chat("gpt-4o", &msgs).unwrap(), "a page");
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        let parts = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(parts[0], json!({"type": "text", "text": "what is this?"}));
        assert_eq!(parts[1]["image_url"]["url"], format!("data:image/png;base64,{b64}"));
        // Anthropic: an image block with a base64 source, before the text
        let (c, m) = client(Kind::Anthropic, None, Some("k"), |_, _| (200, r#"{"content":[{"type":"text","text":"a page"}]}"#.into()));
        assert_eq!(c.chat("claude-sonnet-4", &msgs).unwrap(), "a page");
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!((blocks[0]["source"]["media_type"].as_str(), blocks[0]["source"]["data"].as_str()), (Some("image/png"), Some(b64.as_str())));
        assert_eq!(blocks[1]["text"], "what is this?");
        // Gemini: inline data beside the text part
        let (c, m) = client(Kind::Gemini, None, Some("k"), |_, _| (200, r#"{"candidates":[{"content":{"parts":[{"text":"a page"}]}}]}"#.into()));
        assert_eq!(c.chat("gemini-2.0-flash", &msgs).unwrap(), "a page");
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        let parts = body["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts[1]["inline_data"], json!({"mime_type": "image/png", "data": b64}));
        // a message without pictures is unchanged
        let (c, m) = client(Kind::OpenAi, None, Some("k"), |_, _| (200, r#"{"choices":[{"message":{"content":"x"}}]}"#.into()));
        c.chat("gpt-4o", &convo()).unwrap();
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        assert_eq!(body["messages"][1]["content"], "hi");
    }

    #[test]
    fn ollama_uses_v1_path_needs_no_key_and_strips_thinking() {
        let (c, m) = client(Kind::Ollama, Some("http://10.0.0.5:11434"), None, |_, _| (200, r#"{"choices":[{"message":{"content":"<think>hmm, let me see</think>The answer is 4."}}]}"#.into()));
        assert_eq!(c.chat("qwen3:32b", &convo()).unwrap(), "The answer is 4.");
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!(call.1, "http://10.0.0.5:11434/v1/chat/completions");
        assert!(hdr(&call, "authorization").is_none());
        assert_eq!(strip_think("a<think>x</think>b<think>y</think>c"), "abc");
        assert_eq!(strip_think("a<think>unterminated"), "a");
    }

    #[test]
    fn openrouter_and_compat_bases() {
        let (c, m) = client(Kind::OpenRouter, None, Some("k"), |_, _| (200, r#"{"choices":[{"message":{"content":"ok"}}]}"#.into()));
        c.chat("anthropic/claude-3.5-sonnet", &convo()).unwrap();
        assert_eq!(m.calls.lock().unwrap()[0].1, "https://openrouter.ai/api/v1/chat/completions");
        let (c, m) = client(Kind::Compat, Some("http://lan:8000/v1/"), Some("k"), |_, _| (200, r#"{"choices":[{"message":{"content":"ok"}}]}"#.into()));
        c.chat("my-model", &convo()).unwrap();
        assert_eq!(m.calls.lock().unwrap()[0].1, "http://lan:8000/v1/chat/completions", "trailing slash handled");
        assert!(ProviderCfg::new("x", Kind::Compat).base().is_err(), "compat requires a URL");
    }

    #[test]
    fn anthropic_messages_api() {
        let (c, m) = client(Kind::Anthropic, None, Some("k"), |meth, url| match (meth, url) {
            ("GET", u) if u.contains("/models") => (200, r#"{"data":[{"id":"claude-sonnet-4-5"},{"id":"claude-haiku-4-5"}]}"#.into()),
            _ => (200, r#"{"content":[{"type":"text","text":"Four"},{"type":"tool_use","name":"x"},{"type":"text","text":"."}]}"#.into()),
        });
        assert_eq!(c.chat("claude-sonnet-4-5", &convo()).unwrap(), "Four.");
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!(call.1, "https://api.anthropic.com/v1/messages");
        assert_eq!(hdr(&call, "x-api-key").as_deref(), Some("KEY123"));
        assert_eq!(hdr(&call, "anthropic-version").as_deref(), Some("2023-06-01"));
        assert!(hdr(&call, "authorization").is_none());
        let body: Value = serde_json::from_str(&call.3).unwrap();
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"].as_array().unwrap().len(), 3, "system prompt is not a message");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(c.list_models().unwrap(), vec!["claude-haiku-4-5", "claude-sonnet-4-5"]);
        assert!(c.embed("m", &["x".into()]).unwrap_err().contains("embeddings"));
    }

    #[test]
    fn gemini_generate_content() {
        let (c, m) = client(Kind::Gemini, None, Some("k"), |meth, url| match (meth, url) {
            ("GET", _) => (200, r#"{"models":[{"name":"models/gemini-2.0-flash","supportedGenerationMethods":["generateContent"]},{"name":"models/embedding-001","supportedGenerationMethods":["embedContent"]}]}"#.into()),
            (_, u) if u.contains("batchEmbedContents") => (200, r#"{"embeddings":[{"values":[0.1,0.2]},{"values":[0.3,0.4]}]}"#.into()),
            _ => (200, r#"{"candidates":[{"content":{"parts":[{"text":"Four"},{"text":"!"}]}}]}"#.into()),
        });
        assert_eq!(c.chat("models/gemini-2.0-flash", &convo()).unwrap(), "Four!");
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!(call.1, "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent");
        assert!(!call.1.contains("KEY123"), "the key is sent as a header, never in the URL");
        assert_eq!(hdr(&call, "x-goog-api-key").as_deref(), Some("KEY123"));
        let body: Value = serde_json::from_str(&call.3).unwrap();
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(body["contents"][1]["role"], "model", "assistant turns are role=model");
        assert_eq!(c.list_models().unwrap(), vec!["gemini-2.0-flash"], "only models that can generate");
        assert_eq!(c.embed("embedding-001", &["a".into(), "b".into()]).unwrap(), vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn errors_carry_the_providers_message_but_not_the_key() {
        let (c, _) = client(Kind::OpenAi, None, Some("k"), |_, _| (401, r#"{"error":{"message":"Incorrect API key provided"}}"#.into()));
        assert_eq!(c.chat("m", &convo()).unwrap_err(), "HTTP 401: Incorrect API key provided");
        let (c, _) = client(Kind::Anthropic, None, Some("k"), |_, _| (429, r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#.into()));
        assert!(c.chat("m", &convo()).unwrap_err().contains("slow down"));
        let (c, _) = client(Kind::Ollama, None, None, |_, _| (500, "<html>boom KEY123</html>".into()));
        let e = c.chat("m", &convo()).unwrap_err();
        assert!(e.starts_with("HTTP 500"));
        let (c, _) = client(Kind::Gemini, None, Some("k"), |_, _| (200, r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#.into()));
        assert!(c.chat("m", &convo()).unwrap_err().contains("SAFETY"));
        let (c, _) = client(Kind::OpenAi, None, Some("k"), |_, _| (200, "not json".into()));
        assert!(c.chat("m", &convo()).is_err());
    }

    #[test]
    fn missing_key_fails_before_any_request() {
        let mut cfg = ProviderCfg::new("p", Kind::OpenAi);
        cfg.key_ref = Some("vault:openai".into()); // no vault at all -> locked
        let m = Arc::new(Mock { calls: Default::default(), reply: Box::new(|_, _| (200, "{}".into())) });
        let c = ProviderClient::new(cfg, Secrets::new(None, m.clone()), m.clone());
        assert!(c.chat("gpt-4o", &convo()).unwrap_err().contains("locked"));
        assert!(m.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn model_names_are_validated() {
        let (c, m) = client(Kind::Gemini, None, Some("k"), |_, _| (200, "{}".into()));
        for bad in ["gemini?key=x", "../../etc", "a b", "", "m#frag"] {
            assert!(c.chat(bad, &convo()).is_err(), "{bad}");
        }
        assert!(m.calls.lock().unwrap().is_empty());
        for ok in ["qwen3:32b", "anthropic/claude-3.5-sonnet", "gpt-4o-mini", "models/gemini-2.0-flash", "llama3.2:3b-instruct-q4_K_M"] {
            assert!(valid_model_name(ok), "{ok}");
        }
    }

    #[test]
    fn config_validation() {
        let mut p = ProviderCfg::new("openai", Kind::OpenAi);
        assert!(p.validate().unwrap_err().contains("API key"));
        p.key_ref = Some("vault:openai-key".into());
        assert!(p.validate().is_ok());
        p.key_ref = Some("sk-plaintext-pasted-by-mistake".into());
        assert!(p.validate().unwrap_err().contains("must start with"), "a raw key is refused, so it can't land in a settings file");
        assert!(ProviderCfg::new("Bad Id", Kind::Ollama).validate().is_err());
        let mut o = ProviderCfg::new("local", Kind::Ollama);
        assert!(o.validate().is_ok(), "ollama needs no key");
        o.base_url = Some("ftp://x".into());
        assert!(o.validate().is_err());
        assert_eq!(Kind::Ollama.cycle().cycle(), Kind::OpenRouter);
    }

    #[test]
    fn adapter_plugs_into_chat() {
        let (c, _) = client(Kind::Ollama, None, None, |_, _| (200, r#"{"choices":[{"message":{"content":"pong"}}]}"#.into()));
        let b: Arc<dyn ChatBackend> = Arc::new(ProviderChat { client: c, model: "qwen3:32b".into(), streamer: None });
        assert_eq!(b.complete(&[Message::user("ping")]).unwrap(), "pong");
    }

    /// Live check against a local Ollama (skipped unless `IYAKKI_LIVE_OLLAMA=http://localhost:11434`).
    #[test]
    fn live_ollama_lists_models() {
        let Ok(url) = std::env::var("IYAKKI_LIVE_OLLAMA") else { return };
        let mut cfg = ProviderCfg::new("live", Kind::Ollama);
        cfg.base_url = Some(url);
        let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(10)));
        let c = ProviderClient::new(cfg, Secrets::new(None, up.clone()), up);
        let r = c.test().unwrap();
        println!("live Ollama: {} model(s) in {} ms: {:?}", r.models.len(), r.latency_ms, r.models);
        assert!(!r.models.is_empty());
    }

    // ------------------------------------------------------------------ tool calling

    fn spec() -> Vec<ToolSpec> {
        vec![ToolSpec { name: "read_file".into(), description: "Read a file".into(), schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}) }]
    }

    /// user → assistant asks for two tools → both results → (model continues)
    fn tool_history() -> Vec<Message> {
        let calls = vec![
            ToolCall { id: "c1".into(), name: "read_file".into(), args: json!({"path": "/a"}) },
            ToolCall { id: "c2".into(), name: "read_file".into(), args: json!({"path": "/b"}) },
        ];
        vec![Message::system("be brief"), Message::user("compare a and b"), Message::assistant_calls("Let me look.", calls.clone()), Message::tool_result(&calls[0], "AAA"), Message::tool_result(&calls[1], "BBB")]
    }

    #[test]
    fn openai_style_tool_calls_in_both_directions() {
        let (c, m) = client(Kind::Ollama, None, None, |_, _| {
            (200, r#"{"choices":[{"message":{"content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"/etc/hosts\"}"}},
                {"id":"call_2","type":"function","function":{"name":"read_file","arguments":"{not json"}},
                {"type":"function","function":{"name":"list_dir","arguments":""}}]}}]}"#.into())
        });
        let turn = c.chat_turn("qwen3:32b", &tool_history(), &spec()).unwrap();
        assert_eq!(turn.text, "");
        assert_eq!(turn.calls.len(), 3);
        assert_eq!((turn.calls[0].id.as_str(), turn.calls[0].args.clone()), ("call_1", json!({"path": "/etc/hosts"})));
        assert_eq!(turn.calls[1].args, json!({"_raw_arguments": "{not json"}), "broken JSON is preserved so the agent can tell the model");
        assert_eq!((turn.calls[2].id.as_str(), turn.calls[2].args.clone()), ("call_2", json!({})), "missing id synthesised, empty args = {{}}");
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["tools"][0]["type"], "function");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["arguments"], "{\"path\":\"/a\"}", "arguments are sent as a JSON string");
        assert_eq!((msgs[3]["role"].as_str(), msgs[3]["tool_call_id"].as_str(), msgs[3]["content"].as_str()), (Some("tool"), Some("c1"), Some("AAA")));
        assert_eq!(msgs[4]["tool_call_id"], "c2");
        // a plain answer after tools still works, and no tools param means no `tools` key
        let (c, m) = client(Kind::OpenAi, None, Some("k"), |_, _| (200, r#"{"choices":[{"message":{"content":"They differ."}}]}"#.into()));
        let t = c.chat_turn("gpt-4o", &tool_history(), &[]).unwrap();
        assert_eq!((t.text.as_str(), t.calls.len()), ("They differ.", 0));
        assert!(serde_json::from_str::<Value>(&m.calls.lock().unwrap()[0].3).unwrap().get("tools").is_none());
    }

    #[test]
    fn anthropic_tool_use_and_grouped_results() {
        let (c, m) = client(Kind::Anthropic, None, Some("k"), |_, _| {
            (200, r#"{"content":[{"type":"text","text":"Checking."},{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"/x"}}]}"#.into())
        });
        let turn = c.chat_turn("claude-sonnet-4-5", &tool_history(), &spec()).unwrap();
        assert_eq!(turn.text, "Checking.");
        assert_eq!(turn.calls, vec![ToolCall { id: "toolu_1".into(), name: "read_file".into(), args: json!({"path": "/x"}) }]);
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        assert_eq!(body["tools"][0]["input_schema"]["required"][0], "path");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "user, assistant(blocks), one user turn holding both results");
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!((msgs[1]["content"][1]["type"].as_str(), msgs[1]["content"][1]["id"].as_str()), (Some("tool_use"), Some("c1")));
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 3);
        assert_eq!(msgs[2]["role"], "user");
        let results = msgs[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!((results[0]["type"].as_str(), results[0]["tool_use_id"].as_str(), results[0]["content"].as_str()), (Some("tool_result"), Some("c1"), Some("AAA")));
        assert_eq!(results[1]["tool_use_id"], "c2");
        // a response with only a tool call (no text) is valid
        let (c, _) = client(Kind::Anthropic, None, Some("k"), |_, _| (200, r#"{"content":[{"type":"tool_use","id":"t","name":"read_file","input":{}}]}"#.into()));
        assert_eq!(c.chat_turn("m", &tool_history(), &spec()).unwrap().calls.len(), 1);
    }

    #[test]
    fn gemini_function_calls_and_responses() {
        let (c, m) = client(Kind::Gemini, None, Some("k"), |_, _| {
            (200, r#"{"candidates":[{"content":{"parts":[{"text":"Looking."},{"functionCall":{"name":"read_file","args":{"path":"/g"}}}]}}]}"#.into())
        });
        let turn = c.chat_turn("gemini-2.0-flash", &tool_history(), &spec()).unwrap();
        assert_eq!(turn.text, "Looking.");
        assert_eq!((turn.calls[0].name.as_str(), turn.calls[0].args.clone(), turn.calls[0].id.as_str()), ("read_file", json!({"path": "/g"}), "gemini_0_read_file"));
        let body: Value = serde_json::from_str(&m.calls.lock().unwrap()[0].3).unwrap();
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "read_file");
        assert!(decl["parameters"].get("additionalProperties").is_none(), "keys Gemini rejects are removed");
        assert_eq!(decl["parameters"]["required"][0], "path");
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3, "user, model, one user turn with both responses");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "read_file");
        let responses = contents[2]["parts"].as_array().unwrap();
        assert_eq!(responses.len(), 2);
        assert_eq!((responses[0]["functionResponse"]["name"].as_str(), responses[0]["functionResponse"]["response"]["result"].as_str()), (Some("read_file"), Some("AAA")));
    }

    #[test]
    fn backends_without_tool_support_fail_clearly_but_plain_chat_still_works() {
        struct Plain;
        impl ChatBackend for Plain {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                Ok("hi".into())
            }
        }
        assert!(Plain.complete_tools(&[Message::user("x")], &spec()).unwrap_err().contains("does not support tool calling"));
        let t = Plain.complete_tools(&[Message::user("x")], &[]).unwrap();
        assert_eq!((t.text.as_str(), t.calls.len()), ("hi", 0));
    }

    #[test]
    fn every_providers_own_token_counts_are_read_and_a_missing_count_stays_unknown() {
        let hist = [Message::user("hi")];
        let openai = r#"{"choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}"#;
        let (c, _) = client(Kind::OpenAi, None, Some("k"), move |_, _| (200, openai.into()));
        assert_eq!(c.chat("gpt-4o", &hist).unwrap(), "hello");
        assert_eq!(reported_usage(), Some((11, 3)));
        let anthropic = r#"{"content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":21,"output_tokens":5}}"#;
        let (c, _) = client(Kind::Anthropic, None, Some("k"), move |_, _| (200, anthropic.into()));
        c.chat("claude-x", &hist).unwrap();
        assert_eq!(reported_usage(), Some((21, 5)));
        let gemini = r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]}}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":2}}"#;
        let (c, _) = client(Kind::Gemini, None, Some("k"), move |_, _| (200, gemini.into()));
        c.chat("gemini-x", &hist).unwrap();
        assert_eq!(reported_usage(), Some((8, 2)));
        // a server that does not report: nothing is claimed, and an earlier call's count is not carried over
        let plain = r#"{"choices":[{"message":{"content":"hello"}}]}"#;
        let (c, _) = client(Kind::Compat, Some("http://x/v1"), None, move |_, _| (200, plain.into()));
        c.chat("m", &hist).unwrap();
        assert_eq!(reported_usage(), None);
        assert_eq!(usage_in(&json!({"prompt_eval_count": 40, "eval_count": 9})), Some((40, 9)), "Ollama's own API");
        assert_eq!(usage_in(&json!({"usage": {"prompt_tokens": 5}})), Some((5, 0)));
        assert_eq!(usage_in(&json!({"nothing": 1})), None);
    }

    // ---------------------------------------------------------------- streaming

    struct Sse {
        status: u16,
        body: String,
        seen: Mutex<Vec<(String, String)>>,
    }
    impl iyakki_proxy::Streamer for Sse {
        fn open(&self, _: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
            Err("no GET".into())
        }
        fn post(&self, url: &str, _: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::StreamHead, String> {
            self.seen.lock().unwrap().push((url.into(), String::from_utf8_lossy(body).into()));
            Ok(iyakki_proxy::StreamHead { status: self.status, headers: vec![], body: Box::new(std::io::Cursor::new(self.body.clone().into_bytes())) })
        }
    }

    fn sse(chunks: &[&str]) -> String {
        chunks.iter().map(|c| format!("data: {c}\n\n")).collect::<String>() + "data: [DONE]\n\n"
    }

    #[test]
    fn a_streamed_answer_arrives_piece_by_piece_with_the_thinking_taken_out_and_the_usage_read() {
        let (c, _) = client(Kind::Ollama, Some("http://h:11434"), None, |_, _| (500, String::new()));
        let st = Sse {
            status: 200,
            body: sse(&[
                r#"{"choices":[{"delta":{"role":"assistant","reasoning":"hmm"}}]}"#,
                r#"{"choices":[{"delta":{"content":"<thi"}}]}"#,
                r#"{"choices":[{"delta":{"content":"nk>secret plan</think>It is "}}]}"#,
                r#"{"choices":[{"delta":{"content":"31 C."}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":9}}"#,
            ]),
            seen: Default::default(),
        };
        let mut pieces: Vec<String> = Vec::new();
        let t = c.chat_turn_stream("qwen3:8b", &convo(), &[], &st, &mut |p| {
            pieces.push(p.to_string());
            true
        }).unwrap();
        assert_eq!(t.text, "It is 31 C.");
        assert_eq!(pieces.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>(), vec!["It is ", "31 C."], "{pieces:?}");
        assert!(pieces.contains(&String::new()), "thinking is reported as activity without text");
        assert_eq!(reported_usage(), Some((120, 9)));
        let (url, body) = st.seen.lock().unwrap()[0].clone();
        assert_eq!(url, "http://h:11434/v1/chat/completions");
        let b: Value = serde_json::from_str(&body).unwrap();
        assert_eq!((b["stream"].clone(), b["stream_options"]["include_usage"].clone(), b["keep_alive"].clone()), (json!(true), json!(true), json!("30m")), "{b}");
    }

    #[test]
    fn streamed_tool_calls_are_assembled_from_their_fragments() {
        let (c, _) = client(Kind::OpenAi, None, Some("k"), |_, _| (500, String::new()));
        let st = Sse {
            status: 200,
            body: sse(&[
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"weather","arguments":""}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"place\": "}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Chennai\"}"}},{"index":1,"id":"call_b","function":{"name":"now","arguments":"{}"}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":2,"function":{"name":"broken","arguments":"{oops"}}]}}]}"#,
            ]),
            seen: Default::default(),
        };
        let spec = ToolSpec { name: "weather".into(), description: "w".into(), schema: json!({"type": "object"}) };
        let t = c.chat_turn_stream("gpt-4o", &convo(), &[spec], &st, &mut |_| true).unwrap();
        assert_eq!(t.calls.len(), 3);
        assert_eq!((t.calls[0].id.as_str(), t.calls[0].name.as_str(), t.calls[0].args.clone()), ("call_a", "weather", json!({"place": "Chennai"})));
        assert_eq!((t.calls[1].name.as_str(), t.calls[1].args.clone()), ("now", json!({})));
        assert_eq!(t.calls[2].args, json!({"_raw_arguments": "{oops"}), "broken JSON is kept for the repair step");
        let body: Value = serde_json::from_str(&st.seen.lock().unwrap()[0].1).unwrap();
        assert!(body.get("keep_alive").is_none(), "only Ollama gets keep_alive");
        assert_eq!(body["tools"][0]["function"]["name"], "weather");
    }

    #[test]
    fn a_stream_stops_when_asked_and_server_errors_carry_the_providers_words() {
        let (c, _) = client(Kind::Ollama, Some("http://h"), None, |_, _| (500, String::new()));
        let st = Sse { status: 200, body: sse(&[r#"{"choices":[{"delta":{"content":"one "}}]}"#, r#"{"choices":[{"delta":{"content":"two "}}]}"#, r#"{"choices":[{"delta":{"content":"three"}}]}"#]), seen: Default::default() };
        let mut n = 0;
        let t = c.chat_turn_stream("m", &convo(), &[], &st, &mut |p| {
            if !p.is_empty() {
                n += 1;
            }
            n < 2
        }).unwrap();
        assert_eq!(t.text, "one two", "reading stopped after the second piece");
        let bad = Sse { status: 400, body: r#"{"error":{"message":"registry.ollama.ai/library/x does not support tools"}}"#.into(), seen: Default::default() };
        let e = c.chat_turn_stream("m", &convo(), &[], &bad, &mut |_| true).unwrap_err();
        assert!(e.starts_with("HTTP 400") && crate::toolrepair::is_no_tools_error(&e), "{e}");
        // Gemini answers in one piece through the normal call
        let (g, _) = client(Kind::Gemini, None, Some("k"), |_, _| (200, r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}"#.into()));
        let never = Sse { status: 500, body: String::new(), seen: Default::default() };
        assert_eq!(g.chat_turn_stream("gemini-x", &convo(), &[], &never, &mut |_| true).unwrap().text, "hi");
        assert!(never.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn anthropic_streams_text_and_tool_inputs_in_pieces() {
        let (c, _) = client(Kind::Anthropic, None, Some("k"), |_, _| (500, String::new()));
        let ev = |t: &str, d: &str| format!("event: {t}\ndata: {d}\n\n");
        let body = [
            ev("message_start", r#"{"type":"message_start","message":{"usage":{"input_tokens":55,"output_tokens":1}}}"#),
            ev("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
            ev("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me "}}"#),
            ev("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"check."}}"#),
            ev("content_block_start", r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_1","name":"weather","input":{}}}"#),
            ev("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"place\": "}}"#),
            ev("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"Chennai\"}"}}"#),
            ev("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":31}}"#),
            ev("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .concat();
        let st = Sse { status: 200, body, seen: Default::default() };
        let spec = ToolSpec { name: "weather".into(), description: "w".into(), schema: json!({"type": "object"}) };
        let mut pieces = Vec::new();
        let t = c.chat_turn_stream("claude-x", &convo(), &[spec], &st, &mut |p| {
            if !p.is_empty() {
                pieces.push(p.to_string());
            }
            true
        }).unwrap();
        assert_eq!(pieces, vec!["Let me ", "check."]);
        assert_eq!(t.text, "Let me check.");
        assert_eq!((t.calls[0].id.as_str(), t.calls[0].name.as_str(), t.calls[0].args.clone()), ("tu_1", "weather", json!({"place": "Chennai"})));
        assert_eq!(reported_usage(), Some((55, 31)));
        let (url, sent) = st.seen.lock().unwrap()[0].clone();
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
        let b: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!((b["stream"].clone(), b["system"].clone()), (json!(true), json!("be brief")));
    }

    #[test]
    fn the_think_filter_handles_tags_split_anywhere() {
        let whole = "<think>reasoning</think>Answer <b>bold</b> here";
        for cut in 1..whole.len() {
            let mut f = ThinkFilter::default();
            let (mut out, mut th) = f.push(&whole[..cut]);
            let (o2, t2) = f.push(&whole[cut..]);
            out.push_str(&o2);
            th.push_str(&t2);
            out.push_str(&f.finish());
            assert_eq!((out.as_str(), th.as_str()), ("Answer <b>bold</b> here", "reasoning"), "split at {cut}");
        }
        let mut f = ThinkFilter::default();
        assert_eq!(f.push("<think>never closed"), (String::new(), "never closed".to_string()));
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn ollama_can_be_warmed_up_and_other_providers_ignore_it() {
        let (c, m) = client(Kind::Ollama, Some("http://h:11434"), None, |_, _| (200, "{}".into()));
        c.warm_up("qwen3:8b").unwrap();
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!((call.0.as_str(), call.1.as_str()), ("POST", "http://h:11434/api/generate"));
        let b: Value = serde_json::from_str(&call.3).unwrap();
        assert_eq!((b["model"].clone(), b["keep_alive"].clone()), (json!("qwen3:8b"), json!("30m")));
        let (o, m2) = client(Kind::OpenAi, None, Some("k"), |_, _| (200, "{}".into()));
        assert_eq!(o.warm_up("gpt-4o").unwrap(), 0);
        assert!(m2.calls.lock().unwrap().is_empty());
    }
}
