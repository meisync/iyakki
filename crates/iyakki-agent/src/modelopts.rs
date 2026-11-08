//! Per-model request options: `max_tokens`, `temperature`, `top_p` and `reasoning_budget`, kept per `provider/model` in
//! `<data>/model-options.json` (versioned: a damaged file is moved aside, a newer one is read but never rewritten).
//!
//! * An option is sent in the request body ONLY when it is set. Nothing set means the body is exactly what it was.
//! * `reasoning_budget` (tokens the model may spend thinking; 0 = off, never sent) is sent only to providers whose own API takes it:
//!   NVIDIA NIM (`reasoning_budget`). OpenAI's `reasoning_effort` takes words, not tokens, and Anthropic's `thinking.budget_tokens`
//!   is not spoken anywhere in this code base yet, so for them it is "not used by this provider" and is not sent.
//! * Gemini has its own `generationConfig`, which this code does not speak yet: no option is sent to it.
//! * Values are checked before they are saved (the table is in [`ModelOptions::set_field`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::providers::{valid_model_name, valid_provider_id, Kind};

pub const FILE: &str = "model-options.json";
pub const SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "model options", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "first layout", run: |_| {} }] };

pub const MAX_TOKENS_MAX: u32 = 1_000_000;
/// The options, in the order the page and the tools list them.
pub const FIELDS: [&str; 4] = ["max_tokens", "temperature", "top_p", "reasoning_budget"];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ModelOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Tokens the model may think with; `Some(0)` is "off".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_budget: Option<u32>,
}

impl ModelOptions {
    pub fn is_empty(&self) -> bool {
        self.max_tokens.is_none() && self.temperature.is_none() && self.top_p.is_none() && self.reasoning_budget.is_none()
    }

    /// Check every set value.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(n) = self.max_tokens {
            if !(1..=MAX_TOKENS_MAX).contains(&n) {
                return Err(format!("max_tokens must be a whole number from 1 to {MAX_TOKENS_MAX}"));
            }
        }
        if let Some(t) = self.temperature {
            if !t.is_finite() || !(0.0..=2.0).contains(&t) {
                return Err("temperature must be a number from 0 to 2".into());
            }
        }
        if let Some(p) = self.top_p {
            if !p.is_finite() || !(0.0..=1.0).contains(&p) {
                return Err("top_p must be a number from 0 to 1".into());
            }
        }
        if let Some(n) = self.reasoning_budget {
            if n > MAX_TOKENS_MAX {
                return Err(format!("reasoning_budget must be a whole number of tokens from 0 (off) to {MAX_TOKENS_MAX}"));
            }
        }
        Ok(())
    }

    /// Read one option as the person types it. Empty text, `none` or `default` clears it. Nothing changes when it is refused.
    pub fn set_field(&mut self, field: &str, text: &str) -> Result<(), String> {
        let t = text.trim();
        let clear = t.is_empty() || ["none", "default", "unset", "off-default"].contains(&t.to_ascii_lowercase().as_str());
        let mut next = self.clone();
        match field {
            "max_tokens" => next.max_tokens = if clear { None } else { Some(t.parse::<u32>().map_err(|_| format!("max_tokens must be a whole number from 1 to {MAX_TOKENS_MAX}"))?) },
            "temperature" => next.temperature = if clear { None } else { Some(t.parse::<f64>().map_err(|_| "temperature must be a number from 0 to 2".to_string())?) },
            "top_p" => next.top_p = if clear { None } else { Some(t.parse::<f64>().map_err(|_| "top_p must be a number from 0 to 1".to_string())?) },
            "reasoning_budget" => next.reasoning_budget = if clear { None } else { Some(t.parse::<u32>().map_err(|_| format!("reasoning_budget must be a whole number of tokens from 0 (off) to {MAX_TOKENS_MAX}"))?) },
            _ => return Err(format!("unknown option `{field}`; the options are {}", FIELDS.join(", "))),
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// The value of one option in words (`not set` when it is not).
    pub fn show(&self, field: &str) -> String {
        let r = match field {
            "max_tokens" => self.max_tokens.map(|n| n.to_string()),
            "temperature" => self.temperature.map(fmt_f),
            "top_p" => self.top_p.map(fmt_f),
            "reasoning_budget" => self.reasoning_budget.map(|n| if n == 0 { "0 (off)".to_string() } else { n.to_string() }),
            _ => None,
        };
        r.unwrap_or_else(|| "not set".into())
    }

    /// One line with every option that is set (`nothing set` when none is).
    pub fn summary(&self) -> String {
        let v: Vec<String> = FIELDS.iter().filter(|f| self.has(f)).map(|f| format!("{f} {}", self.show(f))).collect();
        if v.is_empty() { "nothing set".into() } else { v.join(", ") }
    }

    pub fn has(&self, field: &str) -> bool {
        match field {
            "max_tokens" => self.max_tokens.is_some(),
            "temperature" => self.temperature.is_some(),
            "top_p" => self.top_p.is_some(),
            "reasoning_budget" => self.reasoning_budget.is_some(),
            _ => false,
        }
    }
}

fn fmt_f(x: f64) -> String {
    format!("{x}")
}

/// Whether the provider's API takes this option, and if not why, in words for the page.
pub fn used_by(kind: Kind, field: &str) -> Result<(), &'static str> {
    match (kind, field) {
        (Kind::Gemini, _) => Err("not used by this provider"),
        (Kind::Nvidia, _) => Ok(()),
        (_, "reasoning_budget") => Err("not used by this provider"),
        _ => Ok(()),
    }
}

/// The effort word a light one-shot call (tidying words) sets with `providers::set_thread_reasoning`: Ollama is told `none`, NVIDIA NIM gets
/// a small `reasoning_budget` ([`MINIMAL_BUDGET`]); other providers are sent nothing (they keep their own thinking).
pub const MINIMAL_EFFORT: &str = "minimal";
/// Tokens a model may think with on a light one-shot call (never more than the person's own budget for the model).
pub const MINIMAL_BUDGET: u32 = 128;

/// Cap the thinking of this request body at [`MINIMAL_BUDGET`] tokens (a lower budget the person set stays).
pub fn apply_minimal_thinking(body: &mut Value) {
    let cur = body.get("reasoning_budget").and_then(Value::as_u64).filter(|n| *n > 0);
    body["reasoning_budget"] = json!(cur.map_or(MINIMAL_BUDGET as u64, |n| n.min(MINIMAL_BUDGET as u64)));
}

/// Put the set options into an OpenAI-style request body (also NVIDIA NIM, Ollama, Lemonade, OpenRouter).
pub fn apply_openai(body: &mut Value, kind: Kind, o: &ModelOptions) {
    let on = |f: &str| o.has(f) && used_by(kind, f).is_ok();
    if on("max_tokens") {
        body["max_tokens"] = json!(o.max_tokens);
    }
    if on("temperature") {
        body["temperature"] = json!(o.temperature);
    }
    if on("top_p") {
        body["top_p"] = json!(o.top_p);
    }
    if on("reasoning_budget") && o.reasoning_budget.unwrap_or(0) > 0 {
        body["reasoning_budget"] = json!(o.reasoning_budget);
    }
}

/// Anthropic's Messages API: `max_tokens` (always present there), `temperature`, `top_p`.
pub fn apply_anthropic(body: &mut Value, o: &ModelOptions) {
    if let Some(n) = o.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if let Some(t) = o.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = o.top_p {
        body["top_p"] = json!(p);
    }
}

// ---------------------------------------------------------------------------------------------------- the file

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
struct OptionsFile {
    #[serde(default)]
    schema: u32,
    /// `provider/model` -> options.
    #[serde(default)]
    models: BTreeMap<String, ModelOptions>,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

fn path(data: &Path) -> PathBuf {
    data.join(FILE)
}

fn key(provider: &str, model: &str) -> Result<String, String> {
    if !valid_provider_id(provider) || !valid_model_name(model) {
        return Err("invalid provider or model name".into());
    }
    Ok(format!("{provider}/{model}"))
}

fn load(data: &Path) -> (OptionsFile, Option<String>) {
    let p = path(data);
    let (out, f, _) = iyakki_schema::load_typed::<OptionsFile>(&SCHEMA, &p, iyakki_schema::now_secs());
    let ro = match &out {
        iyakki_schema::Outcome::Newer { found, .. } => Some(iyakki_schema::newer_message(&SCHEMA, *found)),
        iyakki_schema::Outcome::Corrupt { moved_to: None, reason } => Some(format!("The model options file cannot be read ({reason}); it is left as it is.")),
        _ => None,
    };
    (f, ro)
}

/// The options of one model (all unset when none are saved or the names are invalid).
pub fn get(data: &Path, provider: &str, model: &str) -> ModelOptions {
    let Ok(k) = key(provider, model) else { return ModelOptions::default() };
    let (f, _) = load(data);
    let mut o = f.models.get(&k).cloned().unwrap_or_default();
    // a hand-edited file cannot send a wild value
    if o.validate().is_err() {
        o = ModelOptions::default();
    }
    o
}

/// Why the file cannot be changed (newer release, unreadable), if it cannot.
pub fn read_only(data: &Path) -> Option<String> {
    load(data).1
}

/// Save the options of one model (empty options remove the entry).
pub fn set(data: &Path, provider: &str, model: &str, o: &ModelOptions) -> Result<(), String> {
    let k = key(provider, model)?;
    o.validate()?;
    let (mut f, ro) = load(data);
    if let Some(why) = ro {
        return Err(why);
    }
    if o.is_empty() {
        f.models.remove(&k);
    } else {
        f.models.insert(k, o.clone());
    }
    f.schema = SCHEMA.current;
    iyakki_schema::save_typed(&SCHEMA, &path(data), &f, 0o600)
}

/// Every model that has options saved: `(provider/model, options)`.
pub fn all(data: &Path) -> Vec<(String, ModelOptions)> {
    load(data).0.models.into_iter().collect()
}

// ---------------------------------------------------------------------------------------------- the provider's model list

/// Most ids kept from one provider's list (a router such as OpenRouter lists hundreds).
pub const LIST_CAP: usize = 1000;

fn list_cache() -> &'static std::sync::Mutex<BTreeMap<String, Vec<String>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, Vec<String>>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The ids the provider listed the last time they were fetched in this session (none: not fetched yet).
pub fn cached_models(provider: &str) -> Option<Vec<String>> {
    list_cache().lock().unwrap_or_else(|e| e.into_inner()).get(provider).cloned()
}

pub fn forget_cached(provider: &str) {
    list_cache().lock().unwrap_or_else(|e| e.into_inner()).remove(provider);
}

/// Ask the provider for its own list (`GET {base}/models` with the key from the vault) and keep it for the session. The request goes
/// through the provider client (the HTTPS client with its timeout and body cap); the key is never shown. Failures are in plain words.
pub fn fetch_models(client: &crate::providers::ProviderClient) -> Result<Vec<String>, String> {
    let id = client.cfg.id.clone();
    match client.list_models() {
        Ok(mut v) => {
            v.truncate(LIST_CAP);
            list_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(id, v.clone());
            Ok(v)
        }
        Err(e) => Err(crate::providererr::plain_error(&id, &e)),
    }
}

/// The plain sentence for a model that a successfully fetched list does not contain (none when it does, or when nothing was fetched).
pub fn not_offered(provider: &str, model: &str) -> Option<String> {
    let list = cached_models(provider)?;
    if model.is_empty() || list.iter().any(|m| m == model) {
        return None;
    }
    Some(format!("The model {model} is not offered by this provider for this key. Pick one from the list."))
}
