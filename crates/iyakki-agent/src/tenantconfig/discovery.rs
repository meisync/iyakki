//! Model discovery: pure parsers of the model-list answers of a local host or provider (Ollama `/api/tags`, OpenAI-style `/v1/models`).
//! They take the body as text, never fetch, and return a clean, sorted, bounded list of model names (control characters and over-long
//! names are dropped). The fetch is the [`super::catalog::ConnectionTester::list_models`] method.

use serde_json::Value;

pub const MAX_MODELS: usize = 500;
pub const MAX_NAME: usize = 80;

fn clean(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = names.map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && n.len() <= MAX_NAME && !n.chars().any(|c| c.is_control() || c.is_whitespace())).collect();
    v.sort();
    v.dedup();
    v.truncate(MAX_MODELS);
    v
}

/// Ollama: `{"models":[{"name":"llama3:8b","model":"llama3:8b",...}]}`.
pub fn parse_ollama_tags(body: &str) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the host's answer is not JSON".to_string())?;
    let a = v.get("models").and_then(|m| m.as_array()).ok_or("the answer has no model list")?;
    Ok(clean(a.iter().filter_map(|m| m.get("name").or_else(|| m.get("model")).and_then(|n| n.as_str()).map(String::from))))
}

/// OpenAI-style: `{"object":"list","data":[{"id":"gpt-4o",...}]}` (also vLLM, LM Studio, llama.cpp, OpenRouter, Mistral).
pub fn parse_openai_models(body: &str) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the host's answer is not JSON".to_string())?;
    let a = v.get("data").and_then(|m| m.as_array()).ok_or("the answer has no model list")?;
    Ok(clean(a.iter().filter_map(|m| m.get("id").and_then(|n| n.as_str()).map(String::from))))
}

/// By API style: `ollama` or anything else (OpenAI-compatible). Gemini's `{"models":[{"name":"models/gemini-..."}]}` is read like Ollama's
/// with the `models/` prefix removed.
pub fn parse_models(style: &str, body: &str) -> Result<Vec<String>, String> {
    match style {
        "ollama" => parse_ollama_tags(body),
        "gemini" => parse_ollama_tags(body).map(|v| v.into_iter().map(|n| n.strip_prefix("models/").map(String::from).unwrap_or(n)).collect()),
        _ => parse_openai_models(body),
    }
}
