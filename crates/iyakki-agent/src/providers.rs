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
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Ollama, Kind::OpenAi, Kind::OpenRouter, Kind::Anthropic, Kind::Gemini, Kind::Compat];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ollama => "Ollama",
            Kind::OpenAi => "OpenAI",
            Kind::OpenRouter => "OpenRouter",
            Kind::Anthropic => "Anthropic Claude",
            Kind::Gemini => "Google Gemini",
            Kind::Compat => "OpenAI-compatible",
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
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, Kind::Ollama | Kind::Compat)
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
        ProviderCfg { id: id.into(), kind, base_url: kind.default_base().map(str::to_string), key_ref: None, models: vec![], default_model: None, enabled: true }
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
        Ok(if self.cfg.kind == Kind::Ollama && !b.ends_with("/v1") { format!("{b}/v1") } else { b })
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

    /// Where and how to ask an Ollama server to download a model.
    pub fn pull_request(&self, model: &str) -> Result<(String, Vec<(String, String)>, Vec<u8>), String> {
        if self.cfg.kind != Kind::Ollama {
            return Err("only Ollama servers can download models".into());
        }
        Ok((format!("{}/api/pull", self.cfg.base()?), self.headers()?, json!({"model": model, "stream": true}).to_string().into_bytes()))
    }

    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = match self.cfg.kind {
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
        forget_usage();
        if !valid_model_name(model) {
            return Err("invalid model name".into());
        }
        let system: Vec<&str> = history.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect();
        let turns: Vec<&Message> = history.iter().filter(|m| m.role != Role::System).collect();
        match self.cfg.kind {
            Kind::Anthropic => {
                let mut msgs: Vec<Value> = Vec::new();
                for m in &turns {
                    match m.role {
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
                let msgs: Vec<Value> = history
                    .iter()
                    .map(|m| match m.role {
                        Role::Assistant if !m.tool_calls.is_empty() => json!({
                            "role": "assistant",
                            "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                            "tool_calls": m.tool_calls.iter().map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}})).collect::<Vec<_>>(),
                        }),
                        Role::Tool => json!({"role": "tool", "tool_call_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content}),
                        r => json!({"role": r.as_str(), "content": m.content}),
                    })
                    .collect();
                let mut body = json!({"model": model, "messages": msgs, "stream": false});
                if !tools.is_empty() {
                    body["tools"] = json!(tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}})).collect::<Vec<_>>());
                }
                let v = self.call("POST", &format!("{}/chat/completions", self.openai_base()?), body.to_string().as_bytes())?;
                let msg = v.pointer("/choices/0/message").ok_or("unexpected response shape (no choices[0].message)")?;
                let text = msg.get("content").and_then(Value::as_str).map(strip_think).unwrap_or_default();
                let calls: Vec<ToolCall> = msg
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .enumerate()
                            .filter_map(|(i, c)| {
                                let f = c.get("function")?;
                                let args = match f.get("arguments") {
                                    Some(Value::String(s)) if s.trim().is_empty() => json!({}),
                                    // a model that writes broken JSON: keep the raw text so the agent can report it
                                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_raw_arguments": s})),
                                    Some(o) => o.clone(),
                                    None => json!({}),
                                };
                                Some(ToolCall { id: c.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("call_{i}")), name: f.get("name")?.as_str()?.to_string(), args })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if text.is_empty() && calls.is_empty() && msg.get("content").is_none() {
                    return Err("unexpected response shape (no choices[0].message.content)".into());
                }
                Ok(ModelTurn { text, calls })
            }
        }
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
}

impl ChatBackend for ProviderChat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.client.chat(&self.model, history)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.client.chat_turn(&self.model, history, tools)
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
        let b: Arc<dyn ChatBackend> = Arc::new(ProviderChat { client: c, model: "qwen3:32b".into() });
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
}
