//! The built-in catalog of API-key providers a team may integrate (data, not code paths), the egress domains each needs, key-shape
//! hints (warnings only, a key is never rejected by its shape) and the optional "test connection" call (a trait; the real caller is
//! a documented stub until an HTTP client that respects the egress proxy is bound).

use super::schema::Problem;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Empty: the entry must give its own base URL.
    pub base_url: &'static str,
    /// `bearer`, `x-api-key`, `api-key` or `x-goog-api-key`.
    pub auth: &'static str,
    pub key_prefix: &'static str,
    pub key_min_len: usize,
    /// Path of the cheap authenticated model-list call.
    pub list_path: &'static str,
    /// Static known models (discovery comes later).
    pub models: &'static [&'static str],
    /// Egress domains the entry needs.
    pub domains: &'static [&'static str],
    /// Fields besides the common ones that this provider needs (`!` = required).
    pub extra: &'static [&'static str],
}

pub static CATALOG: &[Provider] = &[
    Provider { id: "anthropic", name: "Anthropic (Claude)", base_url: "https://api.anthropic.com", auth: "x-api-key", key_prefix: "sk-ant-", key_min_len: 20, list_path: "/v1/models", models: &["claude-opus-4", "claude-sonnet-4", "claude-haiku-4"], domains: &["api.anthropic.com"], extra: &[] },
    Provider { id: "openai", name: "OpenAI", base_url: "https://api.openai.com", auth: "bearer", key_prefix: "sk-", key_min_len: 20, list_path: "/v1/models", models: &["gpt-4o", "gpt-4o-mini"], domains: &["api.openai.com"], extra: &[] },
    Provider { id: "gemini", name: "Google Gemini", base_url: "https://generativelanguage.googleapis.com", auth: "x-goog-api-key", key_prefix: "AIza", key_min_len: 30, list_path: "/v1beta/models", models: &["gemini-2.5-pro", "gemini-2.5-flash"], domains: &["generativelanguage.googleapis.com"], extra: &[] },
    Provider { id: "openrouter", name: "OpenRouter", base_url: "https://openrouter.ai/api", auth: "bearer", key_prefix: "sk-or-", key_min_len: 20, list_path: "/v1/models", models: &[], domains: &["openrouter.ai"], extra: &["site_name", "referer"] },
    Provider { id: "nvidia", name: "NVIDIA NIM (free key)", base_url: "https://integrate.api.nvidia.com", auth: "bearer", key_prefix: "nvapi-", key_min_len: 20, list_path: "/v1/models", models: &[], domains: &["integrate.api.nvidia.com"], extra: &[] },
    Provider { id: "azure_openai", name: "Azure OpenAI", base_url: "", auth: "api-key", key_prefix: "", key_min_len: 20, list_path: "/openai/models", models: &[], domains: &[], extra: &["!deployment", "api_version"] },
    Provider { id: "mistral", name: "Mistral", base_url: "https://api.mistral.ai", auth: "bearer", key_prefix: "", key_min_len: 20, list_path: "/v1/models", models: &["mistral-large-latest", "mistral-small-latest"], domains: &["api.mistral.ai"], extra: &[] },
    Provider { id: "openai_compatible", name: "OpenAI-compatible endpoint (vLLM, Ollama, LM Studio, self-hosted)", base_url: "", auth: "bearer", key_prefix: "", key_min_len: 1, list_path: "/v1/models", models: &[], domains: &[], extra: &[] },
    Provider { id: "git_host", name: "Git hosting token", base_url: "", auth: "bearer", key_prefix: "", key_min_len: 8, list_path: "", models: &[], domains: &[], extra: &[] },
    Provider { id: "other", name: "Other API-key service", base_url: "", auth: "bearer", key_prefix: "", key_min_len: 1, list_path: "", models: &[], domains: &[], extra: &[] },
];

/// Local model servers. Ports and styles are SUGGESTIONS the admin can edit; `0` means none is suggested.
pub struct LocalServer {
    pub id: &'static str,
    pub name: &'static str,
    pub port: u16,
    pub style: &'static str,
    pub list_path: &'static str,
    pub chat_path: &'static str,
}

pub static LOCAL: &[LocalServer] = &[
    LocalServer { id: "ollama", name: "Ollama", port: 11434, style: "ollama", list_path: "/api/tags", chat_path: "/api/chat" },
    LocalServer { id: "lemonade", name: "Lemonade Server (AMD)", port: 0, style: "openai_compatible", list_path: "/v1/models", chat_path: "/v1/chat/completions" },
    LocalServer { id: "lmstudio", name: "LM Studio", port: 1234, style: "openai_compatible", list_path: "/v1/models", chat_path: "/v1/chat/completions" },
    LocalServer { id: "vllm", name: "vLLM", port: 8000, style: "openai_compatible", list_path: "/v1/models", chat_path: "/v1/chat/completions" },
    LocalServer { id: "llamacpp", name: "llama.cpp server", port: 8080, style: "openai_compatible", list_path: "/v1/models", chat_path: "/v1/chat/completions" },
];

pub const LOCAL_IDS: &[&str] = &["ollama", "lemonade", "lmstudio", "vllm", "llamacpp"];

pub fn local_server(id: &str) -> Option<&'static LocalServer> {
    LOCAL.iter().find(|s| s.id == id)
}

pub const IDS: &[&str] = &["anthropic", "openai", "gemini", "openrouter", "nvidia", "azure_openai", "mistral", "openai_compatible", "git_host", "other"];

pub fn provider(id: &str) -> Option<&'static Provider> {
    CATALOG.iter().find(|p| p.id == id)
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let auth = rest.split(['/', '?', '#']).next()?;
    Some(auth.rsplit_once(':').map_or(auth, |x| x.0))
}

/// A loopback, private-range or cluster-local host.
pub fn is_private_host(h: &str) -> bool {
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h.ends_with(".svc") || h.ends_with(".cluster.local") || h.ends_with(".internal") {
        return true;
    }
    let p: Vec<u8> = h.split('.').filter_map(|x| x.parse().ok()).collect();
    p.len() == 4 && h.split('.').count() == 4 && (p[0] == 127 || p[0] == 10 || (p[0] == 192 && p[1] == 168) || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 169 && p[1] == 254))
}

/// The base URL an entry uses: its own, or the provider's default.
pub fn base_url(provider_id: &str, values: &BTreeMap<String, Value>) -> Option<String> {
    values.get("base_url").and_then(|v| v.as_str()).map(String::from).or_else(|| provider(provider_id).filter(|p| !p.base_url.is_empty()).map(|p| p.base_url.to_string()))
}

/// The egress domains an enabled entry needs: the provider's own, or the host of its base URL.
pub fn needed_domains(provider_id: &str, values: &BTreeMap<String, Value>) -> Vec<String> {
    let Some(p) = provider(provider_id) else { return vec![] };
    if !p.domains.is_empty() {
        return p.domains.iter().map(|d| d.to_string()).collect();
    }
    base_url(provider_id, values).and_then(|u| host_of(&u).map(String::from)).into_iter().collect()
}

/// Entry-level checks of an `integration.apikey` item: base URL rules per provider and the provider's own fields.
pub fn check_entry(values: &BTreeMap<String, Value>) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut p = |f: &str, m: &str| out.push(Problem { field: f.into(), message: m.into() });
    let Some(prov) = values.get("provider").and_then(|v| v.as_str()).and_then(provider) else { return out };
    let own = values.get("base_url").and_then(|v| v.as_str());
    if prov.base_url.is_empty() && own.is_none() && prov.id != "other" {
        p("base_url", "this provider needs a base URL");
    }
    if !prov.base_url.is_empty() && prov.id != "azure_openai" && own.is_some_and(|u| u != prov.base_url) {
        p("base_url", "the base URL of this provider is fixed; use openai_compatible for another address");
    }
    for e in ["deployment", "api_version", "site_name", "referer"] {
        let wanted = prov.extra.iter().any(|x| x.trim_start_matches('!') == e);
        if values.contains_key(e) && !wanted {
            p(e, "this setting does not belong to this provider");
        }
        if prov.extra.contains(&format!("!{e}").as_str()) && !values.contains_key(e) {
            p(e, "required for this provider");
        }
    }
    out
}

/// A private or loopback endpoint, or plain http: only when the tenant policy says so. `None` when fine.
pub fn endpoint_problem(provider_id: &str, values: &BTreeMap<String, Value>, allow_private: bool) -> Option<String> {
    let url = base_url(provider_id, values)?;
    let private = host_of(&url).is_some_and(is_private_host);
    if !url.starts_with("https://") && !(private && allow_private) {
        return Some("the endpoint must be https".into());
    }
    if private && !allow_private {
        return Some("a private or loopback endpoint is not allowed by the tenant policy".into());
    }
    None
}

/// A hint about a key's shape, shown as a warning. Never a reason to refuse.
pub fn key_warning(provider_id: &str, key: &str) -> Option<String> {
    let p = provider(provider_id)?;
    if !p.key_prefix.is_empty() && !key.starts_with(p.key_prefix) {
        return Some(format!("a {} key usually starts with {}; stored anyway", p.name, p.key_prefix));
    }
    if key.len() < p.key_min_len {
        return Some(format!("a {} key is usually at least {} characters; stored anyway", p.name, p.key_min_len));
    }
    None
}

pub fn describe() -> Value {
    json!({"api_key": CATALOG.iter().map(|p| json!({"id": p.id, "name": p.name, "base_url": p.base_url, "auth": p.auth, "models": p.models, "domains": p.domains, "fields": p.extra, "key_hint": p.key_prefix})).collect::<Vec<_>>(),
           "local": LOCAL.iter().map(|s| json!({"id": s.id, "name": s.name, "suggested_port": s.port, "suggested_style": s.style, "editable": true})).collect::<Vec<_>>()})
}

// ------------------------------------------------------------------------------------------------------------------ test connection

pub struct TestRequest<'a> {
    /// The cheap model-list URL.
    pub url: &'a str,
    pub auth: &'a str,
    pub key: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestResult {
    pub ok: bool,
    /// The HTTP status, 0 when no answer. The body is never passed back.
    pub status: u16,
}

/// A cheap authenticated call to a provider's model-list endpoint, through the egress proxy.
pub trait ConnectionTester: Send + Sync {
    fn test(&self, req: &TestRequest) -> Result<TestResult, String>;
    /// The names of the models the host offers (`style`: `ollama`, `gemini` or OpenAI-compatible); parsed by [`super::discovery`].
    fn list_models(&self, _req: &TestRequest, _style: &str) -> Result<Vec<String>, String> {
        Err("model discovery is not available here".into())
    }
}

/// The real caller is not bound: no HTTP client of the agent crate is tied to the egress proxy yet.
pub struct NoTester;
impl ConnectionTester for NoTester {
    fn test(&self, _: &TestRequest) -> Result<TestResult, String> {
        Err("the connection tester is not bound on this machine".into())
    }
}

type Reg = Mutex<BTreeMap<PathBuf, Arc<dyn ConnectionTester>>>;
fn reg() -> &'static Reg {
    static R: OnceLock<Reg> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub fn bind_tester(dir: &Path, t: Arc<dyn ConnectionTester>) {
    reg().lock().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf(), t);
}
pub fn tester_for(dir: &Path) -> Arc<dyn ConnectionTester> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).get(dir).cloned().unwrap_or_else(|| Arc::new(super::realtest::RealTester { dir: dir.to_path_buf() }))
}
