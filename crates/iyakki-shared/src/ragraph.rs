use std::path::Path;

pub fn path(data: &Path) -> std::path::PathBuf {
    data.join("rag-graph.json")
}

/// Whether the person switched Graph RAG on (off until they do).
pub fn is_on(data: &Path) -> bool {
    std::fs::read_to_string(path(data)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.get("on").and_then(|b| b.as_bool())).unwrap_or(false)
}

pub fn set_on(data: &Path, on: bool) -> Result<(), String> {
    iyakki_schema::write_atomic(&path(data), serde_json::json!({"on": on}).to_string().as_bytes(), 0o644)
}
