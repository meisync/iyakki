//! The lead assistant's tools for the model providers and their per-model request options (Settings -> Models):
//! `models_list` and `model_options_get` read (no card, never a key); `model_options_set` and `provider_models_fetch` ask every time.
//!
//! * `model_options_set` shows the old and the new values on its card; a value that does not pass the checks is refused before anyone
//!   is asked. Options that the provider does not take are saved but the result says "not used by this provider".
//! * `provider_models_fetch` contacts the provider with the person's key (it names the host on the card) and returns the ids, capped.
//! * Neither tool takes or returns a key; a call that carries something that looks like one is refused without echoing it.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::modelopts::{self, ModelOptions, FIELDS};
use crate::providers::{valid_model_name, valid_provider_id, Kind};

pub const PROMPT_RULE: &str = "\nMODELS: models_list shows the providers and the chosen models (never a key). model_options_get reads the request options of a model (max_tokens, temperature, top_p, reasoning_budget). model_options_set changes them and provider_models_fetch asks a provider for its model names with the person's key; both ask the person every time. Never ask for or type an API key in the chat.";

pub const READ_TOOLS: [&str; 2] = ["models_list", "model_options_get"];
pub const ASK_TOOLS: [&str; 2] = ["model_options_set", "provider_models_fetch"];
/// Most ids returned to the model from one fetch.
pub const FETCH_RETURN_CAP: usize = 200;

pub fn is_model_tool(n: &str) -> bool {
    READ_TOOLS.contains(&n) || ASK_TOOLS.contains(&n)
}

/// One provider as the tools show it (no key).
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderInfo {
    pub id: String,
    pub kind: Kind,
    pub host: String,
    pub enabled: bool,
    pub key_held: bool,
    pub chat: Option<String>,
    pub vision: Option<String>,
    pub embedding: Option<String>,
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn providers(&self) -> Vec<ProviderInfo>;
    fn options(&self, provider: &str, model: &str) -> ModelOptions;
    fn set_options(&self, provider: &str, model: &str, o: &ModelOptions) -> Result<(), String>;
    /// Contact the provider for its list of model ids (plain-words error).
    fn fetch(&self, provider: &str) -> Result<Vec<String>, String>;
}

pub struct ModelTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn looks_like_key(v: &str) -> bool {
    let t = v.trim();
    ["nvapi-", "sk-", "sk_", "AIza", "Bearer "].iter().any(|k| t.starts_with(k) && t.len() > k.len() + 6) || (t.len() >= 32 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) && t.chars().any(|c| c.is_ascii_digit()) && t.chars().any(|c| c.is_ascii_alphabetic()) && !t.contains('/'))
}

impl ModelTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ModelTools {
        ModelTools { inner, backend }
    }

    fn guard(&self, args: &Value) -> Result<(), String> {
        for (_k, v) in args.as_object().into_iter().flatten() {
            if v.as_str().is_some_and(looks_like_key) {
                return Err("never send a key through a tool; the keys stay in the person's vault".into());
            }
        }
        Ok(())
    }

    fn provider(&self, args: &Value) -> Result<ProviderInfo, String> {
        self.guard(args)?;
        let id = args.get("provider").and_then(Value::as_str).unwrap_or("").trim();
        if !valid_provider_id(id) {
            return Err("name the provider (its id from models_list)".into());
        }
        self.backend.providers().into_iter().find(|p| p.id == id).ok_or_else(|| format!("there is no provider `{id}`; models_list shows them"))
    }

    fn target(&self, args: &Value) -> Result<(ProviderInfo, String), String> {
        let p = self.provider(args)?;
        let m = args.get("model").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !valid_model_name(&m) {
            return Err("name the model (a model id, for example nvidia/nemotron-3-nano-omni-30b-a3b-reasoning)".into());
        }
        Ok((p, m))
    }

    /// The new options: the saved ones with the fields in the call applied (a number sets, null or "" clears).
    fn new_options(&self, p: &ProviderInfo, m: &str, args: &Value) -> Result<(ModelOptions, ModelOptions), String> {
        let old = self.backend.options(&p.id, m);
        let mut new = old.clone();
        let mut any = false;
        for f in FIELDS {
            let Some(v) = args.get(f) else { continue };
            any = true;
            let text = match v {
                Value::Null => String::new(),
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => return Err(format!("{f} must be a number (or null to clear it)")),
            };
            new.set_field(f, &text)?;
        }
        if !any {
            return Err(format!("say which option to set: {}", FIELDS.join(", ")));
        }
        Ok((old, new))
    }

    fn lines(&self, p: &ProviderInfo, o: &ModelOptions) -> String {
        FIELDS
            .iter()
            .map(|f| {
                let note = if o.has(f) { modelopts::used_by(p.kind, f).err().map(|w| format!(" ({w})")).unwrap_or_default() } else { String::new() };
                format!("{f}: {}{note}", o.show(f))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "model_options_set" => {
                let (p, m) = self.target(args)?;
                let (old, new) = self.new_options(&p, &m, args)?;
                let diff: Vec<String> = FIELDS.iter().filter(|f| old.show(f) != new.show(f)).map(|f| format!("{f}: {} -> {}", old.show(f), new.show(f))).collect();
                if diff.is_empty() {
                    return Err("nothing would change".into());
                }
                Ok(format!("Change the request options of {}/{}:\n{}", p.id, m, diff.join("\n")))
            }
            _ => {
                let p = self.provider(args)?;
                Ok(format!("Ask {} ({}) for its list of models, using the key of provider {}. The key goes to that host only and is not shown.", p.host, p.kind.label(), p.id))
            }
        }
    }
}

impl Tools for ModelTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_model_tool(&d.spec.name));
        let prov = json!({"type": "string", "description": "The provider id from models_list."});
        let model = json!({"type": "string", "description": "The model id."});
        v.push(ToolDef { spec: ToolSpec { name: "models_list".into(), description: "List the model providers (kind, host, whether a key is held, enabled) and the chosen chat, vision and embedding models. Never returns a key.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "model_options_get".into(), description: "Read the request options saved for one model: max_tokens, temperature, top_p, reasoning_budget (not set means the provider's own default).".into(), schema: json!({"type": "object", "properties": {"provider": prov, "model": model}, "required": ["provider", "model"]}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "model_options_set".into(), description: "Change the request options of one model. The person approves a card with the old and new values. max_tokens 1..1000000, temperature 0..2, top_p 0..1, reasoning_budget in tokens (0 = off; only some providers take it). null clears an option.".into(), schema: json!({"type": "object", "properties": {"provider": prov, "model": model, "max_tokens": {"type": ["integer", "null"]}, "temperature": {"type": ["number", "null"]}, "top_p": {"type": ["number", "null"]}, "reasoning_budget": {"type": ["integer", "null"]}}, "required": ["provider", "model"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "provider_models_fetch".into(), description: "Ask a provider for the list of model ids it offers for the person's key (contacts the provider; the person approves a card that names the host). Returns at most 200 ids.".into(), schema: json!({"type": "object", "properties": {"provider": prov}, "required": ["provider"]}) }, risk: Risk::Ask, untrusted: true });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "models_list" => {
                let ps = self.backend.providers();
                if ps.is_empty() {
                    return Ok("No model providers yet. Settings -> Models adds one.".into());
                }
                let or = |o: &Option<String>| o.clone().unwrap_or_else(|| "none".into());
                Ok(ps.iter().map(|p| format!("{} ({}, {}{}; key {}): chat {}, vision {}, embedding {}", p.id, p.kind.label(), p.host, if p.enabled { "" } else { ", off" }, if p.key_held { "held in the vault" } else if p.kind.needs_key() { "missing" } else { "not needed" }, or(&p.chat), or(&p.vision), or(&p.embedding))).collect::<Vec<_>>().join("\n"))
            }
            "model_options_get" => {
                let (p, m) = self.target(args)?;
                let o = self.backend.options(&p.id, &m);
                let mut out = format!("{}/{}\n{}", p.id, m, self.lines(&p, &o));
                for f in FIELDS {
                    if let Err(w) = modelopts::used_by(p.kind, f) {
                        out.push_str(&format!("\n{f}: {w} ({})", p.kind.label()));
                    }
                }
                Ok(out)
            }
            "model_options_set" => {
                let (p, m) = self.target(args)?;
                let (_, new) = self.new_options(&p, &m, args)?;
                self.backend.set_options(&p.id, &m, &new)?;
                Ok(format!("saved for {}/{}\n{}", p.id, m, self.lines(&p, &new)))
            }
            "provider_models_fetch" => {
                let p = self.provider(args)?;
                let ids = self.backend.fetch(&p.id)?;
                let n = ids.len();
                let shown: Vec<&String> = ids.iter().take(FETCH_RETURN_CAP).collect();
                let more = if n > shown.len() { format!("\n(and {} more; the Settings -> Models picker has them all)", n - shown.len()) } else { String::new() };
                Ok(format!("{} offers {n} model(s):\n{}{more}", p.id, shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if ASK_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        match name {
            "models_list" => "List the model providers".into(),
            "model_options_get" => "Read a model's request options".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if ASK_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if READ_TOOLS.contains(&name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if ASK_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if READ_TOOLS.contains(&name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_model_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !ASK_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if ASK_TOOLS.contains(&name) { Some("changing model options and contacting a provider with the person's key ask every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: the settings, the data directory and the provider clients.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
}

impl Backend for RealBackend {
    fn providers(&self) -> Vec<ProviderInfo> {
        let st = self.svc.settings.lock().unwrap_or_else(|e| e.into_inner());
        let s = st.get();
        let name = |c: &Option<crate::settings::ModelChoice>, p: &str| c.as_ref().filter(|c| c.provider == p).map(|c| c.model.clone());
        s.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id.clone(),
                kind: p.kind,
                host: p.base().ok().and_then(|b| iyakki_sec::url_host(&b)).unwrap_or_default(),
                enabled: p.enabled,
                key_held: p.key_ref.as_ref().is_some_and(|r| self.svc.secrets.resolve(r, None).is_ok()),
                chat: name(&s.chat, &p.id),
                vision: name(&s.vision, &p.id),
                embedding: name(&s.embedding, &p.id),
            })
            .collect()
    }
    fn options(&self, provider: &str, model: &str) -> ModelOptions {
        modelopts::get(self.svc.dir(), provider, model)
    }
    fn set_options(&self, provider: &str, model: &str, o: &ModelOptions) -> Result<(), String> {
        modelopts::set(self.svc.dir(), provider, model, o)
    }
    fn fetch(&self, provider: &str) -> Result<Vec<String>, String> {
        modelopts::fetch_models(&self.svc.client_for(provider, true)?)
    }
}
