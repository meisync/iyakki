use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ExternalRag {
    Qdrant { id: String, url: String, collection: String, #[serde(default)] key_ref: Option<String> },
    Chroma {
        id: String,
        url: String,
        #[serde(default = "default_tenant")]
        tenant: String,
        #[serde(default = "default_database")]
        database: String,
        collection: String,
        #[serde(default)]
        token_ref: Option<String>,
    },
    Http { id: String, url: String, #[serde(default)] key_ref: Option<String> },
}

impl ExternalRag {
    pub fn id(&self) -> &str {
        match self {
            ExternalRag::Qdrant { id, .. } | ExternalRag::Chroma { id, .. } | ExternalRag::Http { id, .. } => id,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ExternalRag::Qdrant { .. } => "Qdrant",
            ExternalRag::Chroma { .. } => "Chroma",
            ExternalRag::Http { .. } => "HTTP search",
        }
    }

    pub fn needs_vector(&self) -> bool {
        !matches!(self, ExternalRag::Http { .. })
    }

    pub fn url(&self) -> &str {
        match self {
            ExternalRag::Qdrant { url, .. } | ExternalRag::Chroma { url, .. } | ExternalRag::Http { url, .. } => url,
        }
    }

    pub fn key_ref(&self) -> Option<&String> {
        match self {
            ExternalRag::Qdrant { key_ref, .. } | ExternalRag::Http { key_ref, .. } => key_ref.as_ref(),
            ExternalRag::Chroma { token_ref, .. } => token_ref.as_ref(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !crate::providers::valid_provider_id(self.id()) {
            return Err("id: lowercase letters, digits, - and _; must start with a letter".into());
        }
        iyakki_sec::url_host(self.url()).ok_or("URL must be http(s)://host[:port]")?;
        match self {
            ExternalRag::Qdrant { collection, .. } if !seg_ok(collection) => return Err("collection name has unsupported characters".into()),
            ExternalRag::Chroma { tenant, database, collection, .. } if !(seg_ok(tenant) && seg_ok(database) && seg_ok(collection)) => return Err("tenant/database/collection have unsupported characters".into()),
            _ => {}
        }
        if let Some(r) = self.key_ref() {
            if !["vault:", "env:", "file:", "ext:"].iter().any(|p| r.starts_with(p)) {
                return Err("credential reference must start with vault:, env:, file: or ext:".into());
            }
        }
        Ok(())
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub fn default_tenant() -> String {
    "default_tenant".into()
}

pub fn default_database() -> String {
    "default_database".into()
}

pub fn seg_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && s != "." && s != ".."
}

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// External RAG backends: query a vector database or retrieval service you already run, and merge
// its results with the local knowledge base.
//
// * **Qdrant** (REST) and **Chroma** (HTTP API v2) take a query *vector*, so the embedding model
//   configured in Settings must match the one the collection was built with.
// * **HTTP** is a generic contract for custom retrieval services:
//   `POST url {"query": "...", "k": 5, "vector": [...]?}` →
//   `{"results": [{"text": "...", "source": "...", "score": 0.9}]}`.
//
// Credentials are secret references (see [`crate::vault`]), resolved per request.

use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde_json::{json, Value};

use crate::kb::Hit;
use crate::vault::{LocalVault, Secrets};





pub struct ExtClient {
    pub cfg: ExternalRag,
    secrets: Secrets,
    upstream: Arc<dyn Upstream>,
}

fn text_of(payload: &Value) -> Option<String> {
    ["text", "content", "document", "chunk", "body"].iter().find_map(|k| payload.get(k)?.as_str()).map(str::to_string)
}

fn source_of(payload: &Value) -> String {
    ["source", "title", "url", "file", "path"].iter().find_map(|k| payload.get(k)?.as_str()).unwrap_or("").to_string()
}

impl ExtClient {
    pub fn new(cfg: ExternalRag, secrets: Secrets, upstream: Arc<dyn Upstream>) -> ExtClient {
        ExtClient { cfg, secrets, upstream }
    }

    fn headers(&self) -> Result<Vec<(String, String)>, String> {
        let mut h = vec![("Content-Type".to_string(), "application/json".to_string())];
        if let Some(r) = self.cfg.key_ref() {
            let k = self.secrets.resolve(r, None::<&LocalVault>).map_err(|e| format!("credential unavailable: {e}"))?;
            match &self.cfg {
                ExternalRag::Qdrant { .. } => h.push(("api-key".into(), k.to_string())),
                ExternalRag::Chroma { .. } => h.push(("Authorization".into(), format!("Bearer {}", &*k))),
                ExternalRag::Http { .. } => h.push(("Authorization".into(), format!("Bearer {}", &*k))),
            }
        }
        Ok(h)
    }

    fn call(&self, method: &str, url: &str, body: Option<Value>) -> Result<Value, String> {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let r = self.upstream.request(method, url, &self.headers()?, &bytes)?;
        if !(200..300).contains(&r.status) {
            let msg: String = String::from_utf8_lossy(&r.body).chars().filter(|c| !c.is_control()).take(160).collect();
            return Err(format!("HTTP {}: {}", r.status, msg.trim()));
        }
        serde_json::from_slice(&r.body).map_err(|_| "service returned something that is not JSON".to_string())
    }

    fn base(&self) -> String {
        self.cfg.url().trim_end_matches('/').to_string()
    }

    fn chroma_collection_url(&self) -> Result<String, String> {
        let ExternalRag::Chroma { tenant, database, collection, .. } = &self.cfg else { return Err("not a Chroma backend".into()) };
        let v = self.call("GET", &format!("{}/api/v2/tenants/{tenant}/databases/{database}/collections/{collection}", self.base()), None)?;
        let id = v.get("id").and_then(Value::as_str).filter(|i| seg_ok(i)).ok_or("collection id missing in response")?;
        Ok(format!("{}/api/v2/tenants/{tenant}/databases/{database}/collections/{id}", self.base()))
    }

    pub fn search(&self, query: &str, vector: Option<&[f32]>, k: usize) -> Result<Vec<Hit>, String> {
        let ns = format!("ext:{}", self.cfg.id());
        let mk = |i: usize, text: String, source: String, score: f32| Hit { ns: ns.clone(), doc_id: format!("{}#{i}", self.cfg.id()), title: source.clone(), source, chunk_idx: i as u32, text, score, bm25: 0.0, vector: score };
        match &self.cfg {
            ExternalRag::Qdrant { collection, .. } => {
                let v = vector.ok_or("Qdrant needs an embedding vector: configure an embedding model in Settings")?;
                let r = self.call("POST", &format!("{}/collections/{collection}/points/search", self.base()), Some(json!({"vector": v, "limit": k, "with_payload": true})))?;
                let pts = r.get("result").and_then(Value::as_array).ok_or("unexpected Qdrant response")?;
                Ok(pts
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| {
                        let payload = p.get("payload")?;
                        Some(mk(i, text_of(payload)?, source_of(payload), p.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32))
                    })
                    .collect())
            }
            ExternalRag::Chroma { .. } => {
                let v = vector.ok_or("Chroma needs an embedding vector: configure an embedding model in Settings")?;
                let url = self.chroma_collection_url()?;
                let r = self.call("POST", &format!("{url}/query"), Some(json!({"query_embeddings": [v], "n_results": k, "include": ["documents", "metadatas", "distances"]})))?;
                let docs = r.pointer("/documents/0").and_then(Value::as_array).ok_or("unexpected Chroma response")?;
                let metas = r.pointer("/metadatas/0").and_then(Value::as_array);
                let dists = r.pointer("/distances/0").and_then(Value::as_array);
                Ok(docs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, d)| {
                        let text = d.as_str()?.to_string();
                        let source = metas.and_then(|m| m.get(i)).map(source_of).unwrap_or_default();
                        let dist = dists.and_then(|d| d.get(i)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                        Some(mk(i, text, source, 1.0 / (1.0 + dist.max(0.0))))
                    })
                    .collect())
            }
            ExternalRag::Http { url, .. } => {
                let mut body = json!({"query": query, "k": k});
                if let Some(v) = vector {
                    body["vector"] = json!(v);
                }
                let r = self.call("POST", url, Some(body))?;
                let items = r.get("results").and_then(Value::as_array).or_else(|| r.as_array()).ok_or("response needs a `results` array")?;
                Ok(items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| Some(mk(i, text_of(it)?, source_of(it), it.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32)))
                    .take(k)
                    .collect())
            }
        }
    }

    /// Check the service is reachable, the credential works, and the collection exists.
    pub fn test(&self) -> Result<String, String> {
        match &self.cfg {
            ExternalRag::Qdrant { collection, .. } => {
                let r = self.call("GET", &format!("{}/collections/{collection}", self.base()), None)?;
                let n = r.pointer("/result/points_count").and_then(Value::as_u64).map_or("?".to_string(), |n| n.to_string());
                Ok(format!("collection `{collection}`, {n} points"))
            }
            ExternalRag::Chroma { collection, .. } => {
                let url = self.chroma_collection_url()?;
                let n = self.call("GET", &format!("{url}/count"), None)?;
                Ok(format!("collection `{collection}`, {} items", n.as_u64().map_or("?".to_string(), |n| n.to_string())))
            }
            ExternalRag::Http { url, .. } => {
                self.call("POST", url, Some(json!({"query": "ping", "k": 1})))?;
                Ok("endpoint responded".into())
            }
        }
    }
}
